use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use airdraw_input::{Modifiers, MouseButton, PenAction, PenEvent, PenSample, PointerId, PointerTool, SampleKind};
use airdraw_platform::{QueueInput, QueueSender};
use glam::Vec2;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::NSSet;
use objc2_ui_kit::{
    UIApplication, UIEvent, UIGestureRecognizer, UIGestureRecognizerState, UIHoverGestureRecognizer, UIPencilInteraction,
    UIPencilInteractionDelegate, UIPencilInteractionPhase, UIPencilInteractionSqueeze, UIPencilInteractionTap, UIPencilPreferredAction,
    UITouch, UITouchType, UIView,
};

use crate::IPAD_PENCIL_CAPS;

pub struct ObserverIvars {
    tx: QueueSender,
    /// Touches currently down, by pointer address.
    active: RefCell<HashSet<u64>>,
}

define_class!(
    /// Watches every touch in its view without ever recognizing, so it never cancels or
    /// delays touches for Godot's own view.
    #[unsafe(super(UIGestureRecognizer, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AirDrawTouchObserver"]
    #[ivars = ObserverIvars]
    struct TouchObserver;

    impl TouchObserver {
        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, event: &UIEvent) {
            let _: () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            wake_loop();
            let Some(view) = self.view() else { return };
            for touch in touches.iter() {
                let id = touch_id(&touch);
                self.ivars().active.borrow_mut().insert(id);
                self.ivars().tx.push(PenEvent::Down {
                    id: PointerId(id),
                    tool: tool(&touch),
                    sample: sample(&touch, &view, SampleKind::Real),
                    modifiers: Modifiers::default(),
                });
            }
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, event: &UIEvent) {
            let _: () = unsafe { msg_send![super(self), touchesMoved: touches, withEvent: event] };
            wake_loop();
            let Some(view) = self.view() else { return };
            for touch in touches.iter() {
                // Coalesced touches hold every sample since the last event (240 Hz Pencil
                // data on a 120 Hz display); the last one is the touch itself.
                let mut samples: Vec<PenSample> = event
                    .coalescedTouchesForTouch(&touch)
                    .map(|all| all.iter().map(|t| sample(&t, &view, SampleKind::Coalesced)).collect())
                    .unwrap_or_default();
                match samples.last_mut() {
                    Some(last) => last.kind = SampleKind::Real,
                    None => samples.push(sample(&touch, &view, SampleKind::Real)),
                }
                let predicted = event
                    .predictedTouchesForTouch(&touch)
                    .map(|all| all.iter().map(|t| sample(&t, &view, SampleKind::Predicted)).collect())
                    .unwrap_or_default();
                self.ivars().tx.push(PenEvent::Move { id: PointerId(touch_id(&touch)), samples, predicted });
            }
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, event: &UIEvent) {
            let _: () = unsafe { msg_send![super(self), touchesEnded: touches, withEvent: event] };
            wake_loop();
            let Some(view) = self.view() else { return };
            for touch in touches.iter() {
                let id = touch_id(&touch);
                self.ivars().active.borrow_mut().remove(&id);
                self.ivars().tx.push(PenEvent::Up { id: PointerId(id), sample: sample(&touch, &view, SampleKind::Real) });
            }
            self.finish_if_idle();
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, event: &UIEvent) {
            let _: () = unsafe { msg_send![super(self), touchesCancelled: touches, withEvent: event] };
            wake_loop();
            for touch in touches.iter() {
                let id = touch_id(&touch);
                self.ivars().active.borrow_mut().remove(&id);
                self.ivars().tx.push(PenEvent::Cancel { id: PointerId(id) });
            }
            self.finish_if_idle();
        }

        #[unsafe(method(reset))]
        fn reset(&self) {
            let _: () = unsafe { msg_send![super(self), reset] };
            self.ivars().active.borrow_mut().clear();
        }

        #[unsafe(method(canPreventGestureRecognizer:))]
        fn can_prevent(&self, _other: &UIGestureRecognizer) -> bool {
            false
        }

        #[unsafe(method(canBePreventedByGestureRecognizer:))]
        fn can_be_prevented(&self, _other: &UIGestureRecognizer) -> bool {
            false
        }
    }
);

impl TouchObserver {
    fn new(mtm: MainThreadMarker, tx: QueueSender) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ObserverIvars { tx, active: RefCell::default() });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setCancelsTouchesInView(false);
        this.setDelaysTouchesBegan(false);
        // Defaults to true, which would hold back touch-ends from Godot's view.
        this.setDelaysTouchesEnded(false);
        this
    }

    /// Failing once every touch is up resets the recognizer for the next sequence without
    /// ever having claimed a touch.
    fn finish_if_idle(&self) {
        if self.ivars().active.borrow().is_empty() {
            self.setState(UIGestureRecognizerState::Failed);
        }
    }
}

define_class!(
    /// Receives Pencil hover, squeeze and double-tap.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AirDrawPencilHandler"]
    #[ivars = QueueSender]
    struct PencilHandler;

    unsafe impl NSObjectProtocol for PencilHandler {}

    unsafe impl UIPencilInteractionDelegate for PencilHandler {
        /// iPadOS < 17.5.
        #[unsafe(method(pencilInteractionDidTap:))]
        fn did_tap_legacy(&self, _interaction: &UIPencilInteraction) {
            wake_loop();
            self.tapped();
        }

        #[unsafe(method(pencilInteraction:didReceiveTap:))]
        fn did_tap(&self, _interaction: &UIPencilInteraction, _tap: &UIPencilInteractionTap) {
            wake_loop();
            self.tapped();
        }

        #[unsafe(method(pencilInteraction:didReceiveSqueeze:))]
        fn did_squeeze(&self, _interaction: &UIPencilInteraction, squeeze: &UIPencilInteractionSqueeze) {
            wake_loop();
            if squeeze.phase() == UIPencilInteractionPhase::Ended {
                self.ivars().push(PenEvent::Action(PenAction::Squeeze));
            }
        }
    }

    impl PencilHandler {
        #[unsafe(method(hover:))]
        fn hover(&self, rec: &UIHoverGestureRecognizer) {
            wake_loop();
            let state = rec.state();
            if state == UIGestureRecognizerState::Began || state == UIGestureRecognizerState::Changed {
                let Some(view) = rec.view() else { return };
                let p = rec.locationInView(Some(&view));
                let mut s = PenSample::at(to_viewport(&view, p.x, p.y), now());
                // Pencil hover angles exist from iPadOS 16.4.
                if rec.respondsToSelector(sel!(altitudeAngle)) {
                    s.tilt = Some(rec.altitudeAngle() as f32);
                    s.azimuth = Some(-rec.azimuthAngleInView(Some(&view)) as f32);
                }
                self.ivars().push(PenEvent::Hover { tool: PointerTool::Pen, sample: s });
            } else {
                self.ivars().push(PenEvent::HoverExit);
            }
        }
    }
);

impl PencilHandler {
    fn new(mtm: MainThreadMarker, tx: QueueSender) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(tx);
        unsafe { msg_send![super(this), init] }
    }

    /// A double-tap, unless the user turned it off in Settings › Apple Pencil (the app maps
    /// it to Draw ⇄ Erase).
    fn tapped(&self) {
        let ignore =
            MainThreadMarker::new().is_some_and(|mtm| UIPencilInteraction::preferredTapAction(mtm) == UIPencilPreferredAction::Ignore);
        if !ignore {
            self.ivars().push(PenEvent::Action(PenAction::DoubleTap));
        }
    }
}

thread_local! {
    /// Godot's viewport size in pixels (UIKit callbacks and Godot share the main thread).
    static VIEWPORT: Cell<Option<Vec2>> = const { Cell::new(None) };
    /// Godot's view, told about input that reaches the app's own handlers.
    static GODOT_VIEW: RefCell<Option<Retained<UIView>>> = const { RefCell::new(None) };
}

/// Input arrived: AirDraw's engine patch slows Godot's loop when the app is idle (race to
/// idle, docs/ARCHITECTURE.md §10) and this brings it back to full rate at once. Pencil hover,
/// squeeze and double-tap and file results reach only the app's handlers, so Godot can't see
/// them itself. A no-op on an engine without the patch.
pub(crate) fn wake_loop() {
    GODOT_VIEW.with(|v| {
        if let Some(view) = &*v.borrow()
            && view.respondsToSelector(sel!(airdrawNoteActivity))
        {
            let _: () = unsafe { msg_send![&**view, airdrawNoteActivity] };
        }
    });
}

pub fn set_viewport_size(width: f32, height: f32) {
    VIEWPORT.with(|v| v.set(Some(Vec2::new(width, height))));
}

/// UIKit points in `view` → Godot viewport pixels.
fn to_viewport(view: &UIView, x: f64, y: f64) -> Vec2 {
    let b = view.bounds();
    let scale = match VIEWPORT.with(Cell::get) {
        Some(vp) if b.size.width > 0.0 && b.size.height > 0.0 => Vec2::new(vp.x / b.size.width as f32, vp.y / b.size.height as f32),
        _ => Vec2::splat(view.contentScaleFactor() as f32),
    };
    Vec2::new(x as f32 * scale.x, y as f32 * scale.y)
}

pub fn attach() -> Option<QueueInput> {
    let mtm = MainThreadMarker::new()?;
    let app = UIApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    let window = app.keyWindow().or_else(|| app.windows().firstObject())?;
    let view = window.rootViewController()?.view()?;
    GODOT_VIEW.with(|v| *v.borrow_mut() = Some(view.clone()));

    let input = QueueInput::new(IPAD_PENCIL_CAPS);
    let tx = input.sender();
    view.addGestureRecognizer(&TouchObserver::new(mtm, tx.clone()));

    let handler = PencilHandler::new(mtm, tx);
    let hover: Retained<UIHoverGestureRecognizer> = unsafe {
        UIHoverGestureRecognizer::initWithTarget_action(UIHoverGestureRecognizer::alloc(mtm), Some(&handler), Some(sel!(hover:)))
    };
    view.addGestureRecognizer(&hover);
    let pencil = UIPencilInteraction::new(mtm);
    pencil.setDelegate(Some(ProtocolObject::from_ref(&*handler)));
    view.addInteraction(ProtocolObject::from_ref(&*pencil));
    // UIKit holds recognizer targets and interaction delegates weakly; the handler must live
    // as long as the app.
    std::mem::forget(handler);
    Some(input)
}

fn touch_id(t: &UITouch) -> u64 {
    // A UITouch object is unique and stable for the lifetime of one contact.
    t as *const UITouch as u64
}

fn tool(t: &UITouch) -> PointerTool {
    match t.r#type() {
        UITouchType::Pencil => PointerTool::Pen,
        // Trackpad/mouse clicks on iPad.
        UITouchType::IndirectPointer => PointerTool::Mouse(MouseButton::Left),
        _ => PointerTool::Finger,
    }
}

/// UIKit touch → sample in Godot viewport pixels.
fn sample(t: &UITouch, view: &UIView, kind: SampleKind) -> PenSample {
    let p = t.preciseLocationInView(Some(view));
    let mut s = PenSample::at(to_viewport(view, p.x, p.y), t.timestamp());
    s.kind = kind;
    if t.r#type() == UITouchType::Pencil {
        let max = t.maximumPossibleForce();
        if max > 0.0 {
            s.pressure = Some((t.force() / max).clamp(0.0, 1.0) as f32);
        }
        s.tilt = Some(t.altitudeAngle() as f32);
        // UIKit measures clockwise in a y-down view; the stylus model is counter-clockwise.
        s.azimuth = Some(-t.azimuthAngleInView(Some(view)) as f32);
        // Barrel roll exists on Apple Pencil Pro with iPadOS 17.5+.
        if t.respondsToSelector(sel!(rollAngle)) {
            s.roll = Some(t.rollAngle() as f32);
        }
    }
    s
}

/// Same clock as `UITouch.timestamp` (seconds since boot).
fn now() -> f64 {
    objc2_foundation::NSProcessInfo::processInfo().systemUptime()
}
