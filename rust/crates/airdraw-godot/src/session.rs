use std::path::PathBuf;

use airdraw_input::{Command, DeviceCaps, InputRouter, PenEvent, PenSample, Routed, RouterConfig, StrokeEvent, ViewEvent};
use airdraw_platform::{InputSource, MergedInput, Platform, QueueInput, QueueSender};
use godot::classes::{Camera3D, INode, InputEvent, Node, Os};
use godot::prelude::*;

use crate::canvas::{SketchCanvas, input_point};
use crate::godot_input::GodotInput;

/// Wires platform input to the sketch: platform events → router → canvas / camera signals.
/// Screen positions are projected onto the guide plane here; the app layer only owns the
/// camera rig, the guide's placement and keyboard shortcuts.
#[derive(GodotClass)]
#[class(base = Node)]
pub struct SketchSession {
    base: Base<Node>,
    #[export]
    canvas: Option<Gd<SketchCanvas>>,
    #[export]
    camera: Option<Gd<Camera3D>>,
    #[export]
    guide_normal: Vector3,
    #[export]
    guide_origin: Vector3,
    /// One finger draws instead of orbiting (devices without a stylus).
    #[export]
    finger_draws: bool,
    platform: Platform,
    /// Feeds the platform's input source with converted Godot events.
    tx: QueueSender,
    godot_input: GodotInput,
    router: InputRouter,
    /// Stroke begun by the router but not yet started on the canvas: the down sample may lack
    /// pressure, so the canvas stroke starts with the first move.
    pending_begin: Option<PenSample>,
    drawing: bool,
    last_pressure: f32,
    /// A native touch backend (UIKit on iPad) is feeding input.
    native: bool,
}

#[godot_api]
impl INode for SketchSession {
    fn init(base: Base<Node>) -> Self {
        let caps = DeviceCaps { pressure: true, tilt: true, azimuth: true, ..DeviceCaps::MOUSE };
        let input = QueueInput::new(caps);
        let tx = input.sender();
        let data_root = PathBuf::from(Os::singleton().get_user_data_dir().to_string());
        Self {
            base,
            canvas: None,
            camera: None,
            guide_normal: Vector3::FORWARD,
            guide_origin: Vector3::ZERO,
            finger_draws: false,
            platform: Platform::builder(data_root).input(input).build(),
            tx,
            godot_input: GodotInput::default(),
            router: InputRouter::new(RouterConfig::default()),
            pending_begin: None,
            drawing: false,
            last_pressure: 1.0,
            native: false,
        }
    }

    fn ready(&mut self) {
        // Native touch backend where one exists; Godot events still cover keyboard, mouse
        // and trackpad.
        if let Some(native) = airdraw_ios::attach() {
            let fallback = QueueInput::new(self.platform.input.caps());
            self.tx = fallback.sender();
            let sources: Vec<Box<dyn InputSource>> = vec![Box::new(native), Box::new(fallback)];
            self.platform.input = Box::new(MergedInput(sources));
            self.godot_input.skip_touch = true;
            self.native = true;
            godot_print!("AirDraw: native UIKit touch input active");
            // Godot's print doesn't reach the device console on iOS; stdout does.
            println!("AirDraw: native UIKit touch input active");
        } else if cfg!(target_os = "ios") {
            println!("AirDraw: native UIKit touch input unavailable; using Godot touch events");
        }
    }

    fn process(&mut self, _delta: f64) {
        // Native backends queue events between frames; route them once per frame.
        if self.native {
            if let Some(vp) = self.base().get_viewport() {
                let size = vp.get_visible_rect().size;
                airdraw_ios::set_viewport_size(size.x, size.y);
            }
            self.pump();
        }
    }

    /// Motion of a pointer that's already down is taken before the GUI sees it, so a stroke
    /// that started on the canvas continues when it passes under panels.
    fn input(&mut self, event: Gd<InputEvent>) {
        if self.godot_input.is_active_motion(&event) && self.godot_input.convert(&event, &self.tx) {
            if let Some(mut vp) = self.base().get_viewport() {
                vp.set_input_as_handled();
            }
            self.pump();
        }
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if self.godot_input.convert(&event, &self.tx) {
            if let Some(mut vp) = self.base().get_viewport() {
                vp.set_input_as_handled();
            }
            // Drain immediately rather than next frame: no added latency on the fallback path.
            self.pump();
        }
    }
}

#[godot_api]
impl SketchSession {
    /// Screen-space drag in pixels.
    #[signal]
    fn view_orbit(delta: Vector2);
    #[signal]
    fn view_pan(delta: Vector2);
    /// Multiply camera distance by `factor` (< 1 zooms in).
    #[signal]
    fn view_zoom(factor: f32, at: Vector2);
    /// Wheel notches to move the guide along its normal.
    #[signal]
    fn guide_nudged(amount: f32);
    /// "squeeze", "doubletap" or "barrelbutton".
    #[signal]
    fn pen_action(name: GString);

    /// Screen rectangles covered by UI (toolbar, open dialogs). Contacts starting there are
    /// ignored, since native backends see touches the GUI also handles.
    #[func]
    fn set_ui_regions(&mut self, rects: Array<Rect2>) {
        let regions = rects
            .iter_shared()
            .map(|r| (glam::Vec2::new(r.position.x, r.position.y), glam::Vec2::new(r.end().x, r.end().y)))
            .collect();
        self.router.set_ui_regions(regions);
    }

    #[func]
    fn native_input_active(&self) -> bool {
        self.native
    }

    fn pump(&mut self) {
        self.router.config.finger_draws = self.finger_draws;
        let mut events: Vec<PenEvent> = Vec::new();
        self.platform.input.poll(&mut events);
        let mut routed = Vec::new();
        for e in events {
            self.router.route(e, &mut routed);
        }
        for r in routed {
            self.dispatch(r);
        }
    }

    fn dispatch(&mut self, r: Routed) {
        match r {
            Routed::Stroke(s) => self.stroke(s),
            Routed::View(ViewEvent::Orbit(d)) => self.signals().view_orbit().emit(Vector2::new(d.x, d.y)),
            Routed::View(ViewEvent::Pan(d)) => self.signals().view_pan().emit(Vector2::new(d.x, d.y)),
            Routed::View(ViewEvent::Zoom { factor, at }) => {
                self.signals().view_zoom().emit(factor, Vector2::new(at.x, at.y))
            }
            Routed::Command(Command::Undo) => self.with_canvas(|c| c.undo()),
            Routed::Command(Command::Redo) => self.with_canvas(|c| c.redo()),
            Routed::Command(Command::NudgeGuide(n)) => self.signals().guide_nudged().emit(n),
            Routed::Command(Command::Pen(a)) => {
                let name = GString::from(format!("{a:?}").to_lowercase().as_str());
                self.signals().pen_action().emit(&name);
            }
            // Hover cursor comes with the native pen backends.
            Routed::Hover(_) => {}
        }
    }

    fn stroke(&mut self, e: StrokeEvent) {
        match e {
            // Eraser behaves like the brush until erasing is implemented.
            StrokeEvent::Begin { sample, .. } => {
                self.cancel_drawing();
                self.pending_begin = Some(sample);
                self.last_pressure = 1.0;
            }
            StrokeEvent::Extend { samples, predicted } => {
                let mut points = Vec::with_capacity(samples.len() + 1);
                if let Some(mut first) = self.pending_begin.take() {
                    first.pressure = first.pressure.or(samples.first().and_then(|s| s.pressure));
                    points.push(first);
                }
                points.extend(samples);
                self.feed(&points, &predicted);
            }
            StrokeEvent::End { sample } => {
                // A click without movement makes no stroke.
                if self.pending_begin.take().is_none() && self.drawing {
                    self.feed(&[sample], &[]);
                    self.with_canvas(|c| c.end_stroke());
                    self.drawing = false;
                }
            }
            StrokeEvent::Cancel => self.cancel_drawing(),
        }
    }

    fn cancel_drawing(&mut self) {
        self.pending_begin = None;
        if self.drawing {
            self.with_canvas(|c| c.cancel_stroke());
            self.drawing = false;
        }
    }

    /// Projects samples onto the guide and hands them to the canvas in one batch (one mesh
    /// rebuild), together with the predicted tail.
    fn feed(&mut self, samples: &[PenSample], predicted: &[PenSample]) {
        let normal = self.guide_normal.normalized();
        let mut points = Vec::with_capacity(samples.len());
        for s in samples {
            let Some(p) = self.project(s.pos) else { continue };
            if let Some(pr) = s.pressure {
                self.last_pressure = pr;
            }
            points.push((p, self.last_pressure, s.t));
        }
        let tail: Vec<_> = predicted
            .iter()
            .filter_map(|s| {
                let p = self.project(s.pos)?;
                Some(input_point(p, normal, s.pressure.unwrap_or(self.last_pressure), s.t))
            })
            .collect();
        let mut rest = points.as_slice();
        if !self.drawing {
            let Some((&(p, pressure, t), others)) = rest.split_first() else { return };
            self.with_canvas(|c| c.begin_stroke(p, normal, pressure, t));
            self.drawing = true;
            rest = others;
        }
        let batch: Vec<_> = rest.iter().map(|&(p, pr, t)| input_point(p, normal, pr, t)).collect();
        self.with_canvas(|c| c.extend_stroke(&batch, &tail));
    }

    /// Screen position → point on the guide plane, or `None` if the ray misses it.
    fn project(&self, pos: glam::Vec2) -> Option<Vector3> {
        let cam = self.camera.as_ref()?;
        let screen = Vector2::new(pos.x, pos.y);
        let from = cam.project_ray_origin(screen);
        let dir = cam.project_ray_normal(screen);
        let n = self.guide_normal.normalized();
        let denom = n.dot(dir);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = n.dot(self.guide_origin - from) / denom;
        (t >= 0.0).then(|| from + dir * t)
    }

    fn with_canvas(&mut self, f: impl FnOnce(&mut SketchCanvas)) {
        if let Some(mut canvas) = self.canvas.clone() {
            f(&mut canvas.bind_mut());
        }
    }
}
