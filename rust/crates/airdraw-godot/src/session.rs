use std::path::PathBuf;

use airdraw_guide::{Guide, Hit, Ray, Sweep, SweepOptions};
use airdraw_input::{Command, DeviceCaps, DrawTool, InputRouter, PenEvent, PenSample, Routed, RouterConfig, StrokeEvent, ViewEvent};
use airdraw_platform::{InputSource, MergedInput, Platform, QueueInput, QueueSender};
use godot::classes::{Camera3D, INode, InputEvent, Node, Os};
use godot::prelude::*;

use crate::canvas::{SketchCanvas, input_point};
use crate::godot_input::GodotInput;

/// Wires platform input to the sketch: platform events → router → canvas / camera signals.
/// Screen positions are projected onto the active 3D guide here: the canvas's drawn guide if
/// there is one, else the preset plane the app places (`guide_normal`, `guide_origin`). The app
/// layer owns the camera rig, the preset plane and keyboard shortcuts.
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
    /// The next pen stroke draws a 3D guide instead of ink, then this turns off.
    #[var]
    guide_drawing: bool,
    /// The pen erases (the Erase tool). A pen's eraser end erases whatever this is.
    #[var]
    erase_tool: bool,
    /// Eraser radius in viewport pixels.
    #[var]
    eraser_radius: f32,
    /// Where the eraser was last, while an eraser drag is on.
    erase_last: Option<glam::Vec2>,
    /// The guide the current stroke started on; it stays on it until the pen lifts.
    stroke_guide: Option<Guide>,
    /// Screen positions and camera rays of the guide being drawn.
    guide_path: Vec<(Vector2, Ray)>,
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
            guide_drawing: false,
            erase_tool: false,
            eraser_radius: 12.0,
            erase_last: None,
            stroke_guide: None,
            guide_path: Vec::new(),
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
    /// The guide stroke so far, in screen pixels, for a live overlay; empty when it ends.
    #[signal]
    fn guide_path_changed(points: PackedVector2Array);
    /// A guide stroke ended; `created` is false if it was too short to make a guide.
    #[signal]
    fn guide_drawn(created: bool);
    /// The pen (or drawing mouse button) went down on the canvas: sheets close.
    #[signal]
    fn pen_down();
    /// Hide or show all UI (4-finger tap).
    #[signal]
    fn ui_toggle_requested();
    /// Where to draw the eraser's ring (viewport pixels), or `shown` false to hide it.
    #[signal]
    fn eraser_cursor(pos: Vector2, shown: bool);
    /// An eraser drag ended; `erased` strokes went (one undo step).
    #[signal]
    fn erased(count: i64);

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

    /// Where picking puts a world point on screen (tests compare it with Godot's own
    /// `Camera3D.unproject_position`); `(-1, -1)` behind the camera.
    #[func]
    fn pick_project(&self, world: Vector3) -> Vector2 {
        let at = self.pick_camera().and_then(|c| c.project(v3(world))).map(|(p, _)| p);
        at.map_or(Vector2::new(-1.0, -1.0), |p| Vector2::new(p.x, p.y))
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
            Routed::Command(Command::ToggleUi) => self.signals().ui_toggle_requested().emit(),
            Routed::Command(Command::Pen(a)) => {
                let name = GString::from(format!("{a:?}").to_lowercase().as_str());
                self.signals().pen_action().emit(&name);
            }
            // The eraser's ring follows a hovering pen (Pencil hover on iPad).
            Routed::Hover(Some(s)) if self.erase_tool && self.erase_last.is_none() => {
                self.signals().eraser_cursor().emit(Vector2::new(s.pos.x, s.pos.y), true)
            }
            Routed::Hover(None) if self.erase_last.is_none() => self.signals().eraser_cursor().emit(Vector2::ZERO, false),
            Routed::Hover(_) => {}
        }
    }

    fn stroke(&mut self, e: StrokeEvent) {
        if matches!(e, StrokeEvent::Begin { .. }) {
            self.signals().pen_down().emit();
        }
        let erase = match e {
            StrokeEvent::Begin { tool, .. } => tool == DrawTool::Eraser || self.erase_tool,
            _ => self.erase_last.is_some(),
        };
        if erase {
            self.erase_stroke(e);
            return;
        }
        if self.guide_drawing {
            self.guide_stroke(e);
            return;
        }
        match e {
            StrokeEvent::Begin { sample, .. } => {
                self.cancel_drawing();
                self.pending_begin = Some(sample);
                self.last_pressure = 1.0;
                self.stroke_guide = Some(self.active_guide());
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

    /// Guide-draw mode: collects the pen path, then turns it into a guide perpendicular to the
    /// view (Feather's "draw 3D guide").
    fn guide_stroke(&mut self, e: StrokeEvent) {
        match e {
            StrokeEvent::Begin { sample, .. } => {
                self.guide_path.clear();
                self.add_guide_point(sample.pos);
            }
            StrokeEvent::Extend { samples, .. } => {
                for s in samples {
                    self.add_guide_point(s.pos);
                }
            }
            StrokeEvent::End { sample } => {
                self.add_guide_point(sample.pos);
                let guide = self.build_guide();
                self.guide_path.clear();
                self.signals().guide_path_changed().emit(&PackedVector2Array::new());
                self.guide_drawing = false;
                let created = guide.is_some();
                if let Some(g) = guide {
                    self.with_canvas(|c| c.set_drawn_guide(g));
                }
                self.signals().guide_drawn().emit(created);
                return;
            }
            StrokeEvent::Cancel => self.guide_path.clear(),
        }
        let path: PackedVector2Array = self.guide_path.iter().map(|(p, _)| *p).collect();
        self.signals().guide_path_changed().emit(&path);
    }

    /// Eraser drag: removes the front strokes the eraser passes over, all at once at the end.
    fn erase_stroke(&mut self, e: StrokeEvent) {
        match e {
            StrokeEvent::Begin { sample, .. } => {
                self.cancel_drawing();
                self.with_canvas(|c| c.begin_erase());
                self.erase_last = Some(sample.pos);
                self.erase_to(&[sample.pos]);
            }
            StrokeEvent::Extend { samples, .. } => {
                let Some(last) = self.erase_last else { return };
                let path: Vec<glam::Vec2> = std::iter::once(last).chain(samples.iter().map(|s| s.pos)).collect();
                self.erase_to(&path);
            }
            StrokeEvent::End { sample } => {
                if let Some(last) = self.erase_last {
                    self.erase_to(&[last, sample.pos]);
                }
                self.erase_last = None;
                let mut count = 0;
                self.with_canvas(|c| count = c.end_erase());
                self.signals().eraser_cursor().emit(Vector2::ZERO, false);
                self.signals().erased().emit(count as i64);
            }
            StrokeEvent::Cancel => {
                self.erase_last = None;
                self.with_canvas(|c| c.cancel_erase());
                self.signals().eraser_cursor().emit(Vector2::ZERO, false);
            }
        }
    }

    fn erase_to(&mut self, path: &[glam::Vec2]) {
        let Some(&end) = path.last() else { return };
        self.erase_last = Some(end);
        if let Some(cam) = self.pick_camera() {
            let radius = self.eraser_radius;
            self.with_canvas(|c| {
                c.erase_along(&cam, path, radius);
            });
        }
        self.signals().eraser_cursor().emit(Vector2::new(end.x, end.y), true);
    }

    /// The camera as picking sees it: world → camera, projection, viewport size in pixels.
    fn pick_camera(&self) -> Option<airdraw_pick::Camera> {
        let cam = self.camera.as_ref()?;
        let size = cam.get_viewport()?.get_visible_rect().size;
        let t = cam.get_global_transform().affine_inverse();
        let a = |v: Vector3| glam::Vec3A::new(v.x, v.y, v.z);
        let view = glam::Affine3A::from_cols(a(t.basis.col_a()), a(t.basis.col_b()), a(t.basis.col_c()), a(t.origin));
        let p = cam.get_camera_projection();
        let c = |v: Vector4| glam::Vec4::new(v.x, v.y, v.z, v.w);
        let proj = glam::Mat4::from_cols(c(p.cols[0]), c(p.cols[1]), c(p.cols[2]), c(p.cols[3]));
        Some(airdraw_pick::Camera::new(view, proj, glam::Vec2::new(size.x, size.y)))
    }

    fn add_guide_point(&mut self, pos: glam::Vec2) {
        if let Some(ray) = self.ray(pos) {
            self.guide_path.push((Vector2::new(pos.x, pos.y), ray));
        }
    }

    /// A sweep along the camera rays of the drawn path, its curve at the depth where the path
    /// starts on the current guide (or 5 units ahead if it misses).
    fn build_guide(&self) -> Option<Guide> {
        let cam = self.camera.as_ref()?;
        let b = cam.get_global_transform().basis;
        let forward = glam::Vec3::new(-b.col_c().x, -b.col_c().y, -b.col_c().z);
        let rays: Vec<Ray> = self.guide_path.iter().map(|(_, r)| *r).collect();
        let current = self.active_guide();
        let anchor = rays
            .iter()
            .find_map(|r| current.intersect_in_reach(*r).map(|h| h.point))
            .unwrap_or_else(|| rays.first().map_or(glam::Vec3::ZERO, |r| r.origin + r.dir * 5.0));
        Sweep::from_view(&rays, forward, anchor, SweepOptions::default()).map(Guide::Sweep)
    }

    /// The guide strokes land on now: the drawn guide, else the preset plane.
    fn active_guide(&self) -> Guide {
        let drawn = self.canvas.as_ref().and_then(|c| c.bind().drawn_guide());
        match drawn {
            Some(g) => (*g).clone(),
            None => Guide::Plane { origin: v3(self.guide_origin), normal: v3(self.guide_normal) },
        }
    }

    fn cancel_drawing(&mut self) {
        self.pending_begin = None;
        if self.drawing {
            self.with_canvas(|c| c.cancel_stroke());
            self.drawing = false;
        }
    }

    /// Projects samples onto the stroke's guide and hands them to the canvas in one batch (one
    /// mesh update), together with the predicted tail. Samples that miss the guide are
    /// dropped, so a stroke draws only where the guide is.
    fn feed(&mut self, samples: &[PenSample], predicted: &[PenSample]) {
        let mut points = Vec::with_capacity(samples.len());
        for s in samples {
            let Some(hit) = self.project(s.pos) else { continue };
            if let Some(pr) = s.pressure {
                self.last_pressure = pr;
            }
            points.push((hit, self.last_pressure, s.t));
        }
        let tail: Vec<_> = predicted
            .iter()
            .filter_map(|s| {
                let hit = self.project(s.pos)?;
                Some(input_point(gv3(hit.point), gv3(hit.normal), s.pressure.unwrap_or(self.last_pressure), s.t))
            })
            .collect();
        let mut rest = points.as_slice();
        if !self.drawing {
            let Some((&(hit, pressure, t), others)) = rest.split_first() else { return };
            self.with_canvas(|c| c.begin_stroke(gv3(hit.point), gv3(hit.normal), pressure, t));
            self.drawing = true;
            rest = others;
        }
        let batch: Vec<_> = rest.iter().map(|&(h, pr, t)| input_point(gv3(h.point), gv3(h.normal), pr, t)).collect();
        self.with_canvas(|c| c.extend_stroke(&batch, &tail));
    }

    /// Screen position → hit on the current stroke's guide, or `None` if the ray misses it
    /// or meets it out of reach (pen near a plane's horizon).
    fn project(&self, pos: glam::Vec2) -> Option<Hit> {
        self.stroke_guide.as_ref()?.intersect_in_reach(self.ray(pos)?)
    }

    /// The camera ray through a screen position.
    fn ray(&self, pos: glam::Vec2) -> Option<Ray> {
        let cam = self.camera.as_ref()?;
        let screen = Vector2::new(pos.x, pos.y);
        Some(Ray { origin: v3(cam.project_ray_origin(screen)), dir: v3(cam.project_ray_normal(screen)).normalize_or(glam::Vec3::NEG_Z) })
    }

    fn with_canvas(&mut self, f: impl FnOnce(&mut SketchCanvas)) {
        if let Some(mut canvas) = self.canvas.clone() {
            f(&mut canvas.bind_mut());
        }
    }
}

fn v3(v: Vector3) -> glam::Vec3 {
    glam::Vec3::new(v.x, v.y, v.z)
}

fn gv3(v: glam::Vec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}
