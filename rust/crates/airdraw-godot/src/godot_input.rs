//! Fallback input backend: converts Godot input events into the unified stylus model.
//! Used on desktop, and on mobile until a native backend replaces it.
//!
//! Godot's touch events don't say whether a contact is a pen or a finger, and only drag events
//! carry pressure. So touch contacts are held back until their first drag and classified by
//! tilt: a stylus is almost always tilted, a finger reports none. Native backends (step 6)
//! get the real tool type from the OS instead.

use std::collections::HashMap;

use airdraw_input::{Modifiers, MouseButton, PenEvent, PenSample, PointerId, PointerTool};
use airdraw_platform::QueueSender;
use glam::Vec2;
use godot::classes::{
    InputEvent, InputEventMagnifyGesture, InputEventMouseButton, InputEventMouseMotion, InputEventPanGesture,
    InputEventScreenDrag, InputEventScreenTouch, InputEventWithModifiers, Time,
};
use godot::global::{MouseButton as GdButton, MouseButtonMask};
use godot::prelude::*;

const MOUSE_ID_BASE: u64 = 1 << 32;
const TOUCH_ID_BASE: u64 = 2 << 32;
const MOUSE_BUTTONS: [(MouseButton, GdButton, MouseButtonMask); 3] = [
    (MouseButton::Left, GdButton::LEFT, MouseButtonMask::LEFT),
    (MouseButton::Right, GdButton::RIGHT, MouseButtonMask::RIGHT),
    (MouseButton::Middle, GdButton::MIDDLE, MouseButtonMask::MIDDLE),
];

enum Touch {
    /// Down seen, tool not known until the first drag.
    Pending { id: PointerId, sample: PenSample },
    Active { id: PointerId, tool: PointerTool },
}

#[derive(Default)]
pub struct GodotInput {
    /// Keyed by Godot's touch index, which is reused between contacts.
    touches: HashMap<i32, Touch>,
    next_touch: u64,
    /// A native backend delivers touches; Godot's touch events are left to the GUI.
    pub skip_touch: bool,
}

impl GodotInput {
    /// Whether `event` moves a pointer that is already pressed (a stroke or drag in progress).
    /// Such motion must keep flowing when it passes over UI, which would otherwise swallow it.
    pub fn is_active_motion(&self, event: &Gd<InputEvent>) -> bool {
        if let Ok(e) = event.clone().try_cast::<InputEventMouseMotion>() {
            return e.get_button_mask().ord() != 0;
        }
        if let Ok(e) = event.clone().try_cast::<InputEventScreenDrag>() {
            return !self.skip_touch && self.touches.contains_key(&e.get_index());
        }
        false
    }
}

impl GodotInput {
    /// Pushes the converted event(s) into `tx`. Returns false for events this backend ignores
    /// (keyboard etc.), so the app layer can handle them.
    pub fn convert(&mut self, event: &Gd<InputEvent>, tx: &QueueSender) -> bool {
        let t = now();
        if let Ok(e) = event.clone().try_cast::<InputEventMouseButton>() {
            return mouse_button(&e, t, tx);
        }
        if let Ok(e) = event.clone().try_cast::<InputEventMouseMotion>() {
            return mouse_motion(&e, t, tx);
        }
        if let Ok(e) = event.clone().try_cast::<InputEventScreenTouch>() {
            if !self.skip_touch {
                self.screen_touch(&e, t, tx);
            }
            return !self.skip_touch;
        }
        if let Ok(e) = event.clone().try_cast::<InputEventScreenDrag>() {
            if !self.skip_touch {
                self.screen_drag(&e, t, tx);
            }
            return !self.skip_touch;
        }
        if let Ok(e) = event.clone().try_cast::<InputEventPanGesture>() {
            let modifiers = modifiers(&e.clone().upcast());
            tx.push(PenEvent::Scroll { pos: v2(e.get_position()), delta: v2(e.get_delta()), precise: true, modifiers });
            return true;
        }
        if let Ok(e) = event.clone().try_cast::<InputEventMagnifyGesture>() {
            tx.push(PenEvent::Pinch { pos: v2(e.get_position()), factor: e.get_factor() });
            return true;
        }
        false
    }

    fn screen_touch(&mut self, e: &Gd<InputEventScreenTouch>, t: f64, tx: &QueueSender) {
        let index = e.get_index();
        let sample = PenSample::at(v2(e.get_position()), t);
        if e.is_pressed() {
            let id = PointerId(TOUCH_ID_BASE + self.next_touch);
            self.next_touch += 1;
            if self.touches.is_empty() {
                self.touches.insert(index, Touch::Pending { id, sample });
                return;
            }
            // A second contact means a multi-finger gesture: resolve waiting contacts as
            // fingers now, or a quick two-finger tap would arrive as two separate taps.
            let finger = PointerTool::Finger;
            for touch in self.touches.values_mut() {
                if let Touch::Pending { id, sample } = *touch {
                    tx.push(PenEvent::Down { id, tool: finger, sample, modifiers: Modifiers::default() });
                    *touch = Touch::Active { id, tool: finger };
                }
            }
            tx.push(PenEvent::Down { id, tool: finger, sample, modifiers: Modifiers::default() });
            self.touches.insert(index, Touch::Active { id, tool: finger });
            return;
        }
        match self.touches.remove(&index) {
            Some(Touch::Pending { id, sample: start }) => {
                // Lifted without moving: a tap. Fingers are the only thing that taps meaningfully.
                if !e.is_canceled() {
                    let modifiers = Modifiers::default();
                    tx.push(PenEvent::Down { id, tool: PointerTool::Finger, sample: start, modifiers });
                    tx.push(PenEvent::Up { id, sample });
                }
            }
            Some(Touch::Active { id, .. }) => {
                tx.push(if e.is_canceled() { PenEvent::Cancel { id } } else { PenEvent::Up { id, sample } });
            }
            None => {}
        }
    }

    fn screen_drag(&mut self, e: &Gd<InputEventScreenDrag>, t: f64, tx: &QueueSender) {
        let Some(touch) = self.touches.get_mut(&e.get_index()) else { return };
        let tilt = e.get_tilt();
        let (id, tool) = match *touch {
            Touch::Pending { id, sample } => {
                let tool = if tilt.length_squared() > 1e-6 { PointerTool::Pen } else { PointerTool::Finger };
                let start = PenSample { pressure: pen_pressure(tool, e.get_pressure()), ..sample };
                tx.push(PenEvent::Down { id, tool, sample: start, modifiers: Modifiers::default() });
                *touch = Touch::Active { id, tool };
                (id, tool)
            }
            Touch::Active { id, tool } => (id, tool),
        };
        let mut sample = PenSample::at(v2(e.get_position()), t);
        sample.pressure = pen_pressure(tool, e.get_pressure());
        if tool == PointerTool::Pen {
            (sample.tilt, sample.azimuth) = tilt_angles(tilt);
        }
        tx.push(PenEvent::Move { id, samples: vec![sample], predicted: vec![] });
    }
}

fn mouse_button(e: &Gd<InputEventMouseButton>, t: f64, tx: &QueueSender) -> bool {
    let pos = v2(e.get_position());
    let modifiers = modifiers(&e.clone().upcast());
    let index = e.get_button_index();
    if let Some(&(button, ..)) = MOUSE_BUTTONS.iter().find(|(_, gd, _)| *gd == index) {
        let id = mouse_id(button);
        let sample = PenSample::at(pos, t);
        tx.push(if e.is_pressed() {
            PenEvent::Down { id, tool: PointerTool::Mouse(button), sample, modifiers }
        } else {
            PenEvent::Up { id, sample }
        });
        return true;
    }
    let dir = if index == GdButton::WHEEL_UP {
        Vec2::Y
    } else if index == GdButton::WHEEL_DOWN {
        -Vec2::Y
    } else if index == GdButton::WHEEL_LEFT {
        -Vec2::X
    } else if index == GdButton::WHEEL_RIGHT {
        Vec2::X
    } else {
        return false;
    };
    if e.is_pressed() {
        // Some platforms report 0 for wheels without fine-grained steps.
        let factor = if e.get_factor() > 0.0 { e.get_factor() } else { 1.0 };
        tx.push(PenEvent::Scroll { pos, delta: dir * factor, precise: false, modifiers });
    }
    true
}

fn mouse_motion(e: &Gd<InputEventMouseMotion>, t: f64, tx: &QueueSender) -> bool {
    let mask = e.get_button_mask().ord();
    if mask == 0 {
        return false;
    }
    let mut sample = PenSample::at(v2(e.get_position()), t);
    // Plain mice report 0; drawing tablets report real pressure while a button is held.
    let pressure = e.get_pressure();
    if pressure > 0.0 {
        sample.pressure = Some(pressure);
        (sample.tilt, sample.azimuth) = tilt_angles(e.get_tilt());
    }
    for (button, _, bit) in MOUSE_BUTTONS {
        if mask & bit.ord() != 0 {
            tx.push(PenEvent::Move { id: mouse_id(button), samples: vec![sample], predicted: vec![] });
        }
    }
    true
}

fn mouse_id(button: MouseButton) -> PointerId {
    PointerId(MOUSE_ID_BASE + button as u64)
}

fn pen_pressure(tool: PointerTool, p: f32) -> Option<f32> {
    (tool == PointerTool::Pen && p.is_finite() && p > 0.0).then(|| p.min(1.0))
}

/// Godot encodes tilt as the lean direction scaled by cos(altitude).
fn tilt_angles(v: Vector2) -> (Option<f32>, Option<f32>) {
    let len = v.length();
    if len < 1e-6 {
        return (None, None);
    }
    (Some(len.min(1.0).acos()), Some((-v.y).atan2(v.x)))
}

fn modifiers(e: &Gd<InputEventWithModifiers>) -> Modifiers {
    Modifiers {
        shift: e.is_shift_pressed(),
        ctrl: e.is_ctrl_pressed(),
        alt: e.is_alt_pressed(),
        meta: e.is_meta_pressed(),
    }
}

fn now() -> f64 {
    Time::singleton().get_ticks_usec() as f64 / 1e6
}

fn v2(v: Vector2) -> Vec2 {
    Vec2::new(v.x, v.y)
}
