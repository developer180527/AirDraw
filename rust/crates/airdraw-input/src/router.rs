//! Decides what each pointer does: pen draws, fingers move the camera, mouse buttons follow
//! bindings. Works purely on [`PenEvent`]s, so every platform gets identical behavior.

use std::collections::{HashMap, HashSet};

use glam::Vec2;

use crate::{Modifiers, MouseButton, PenAction, PenEvent, PenSample, PointerId, PointerTool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawTool {
    Brush,
    Eraser,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrokeEvent {
    Begin { tool: DrawTool, sample: PenSample },
    Extend { samples: Vec<PenSample>, predicted: Vec<PenSample> },
    End { sample: PenSample },
    Cancel,
}

/// Camera movement in screen pixels; the camera decides how pixels map to angles/distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewEvent {
    Orbit(Vec2),
    Pan(Vec2),
    /// Multiply the camera distance by `factor` (< 1 zooms in), centered on `at`.
    Zoom { factor: f32, at: Vec2 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Undo,
    Redo,
    /// Move the drawing guide along its normal, in wheel notches.
    NudgeGuide(f32),
    Pen(PenAction),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Routed {
    Stroke(StrokeEvent),
    View(ViewEvent),
    Command(Command),
    /// Pen hovering at a sample, or `None` when it left.
    Hover(Option<PenSample>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragAction {
    Draw,
    Orbit,
    Pan,
    None,
}

#[derive(Clone, Debug)]
pub struct RouterConfig {
    /// A single finger draws (phones/tablets without a stylus). Two fingers still move the view.
    pub finger_draws: bool,
    pub mouse_left: DragAction,
    pub mouse_right: DragAction,
    pub mouse_middle: DragAction,
    /// Holding Shift turns an orbit drag into a pan.
    pub shift_orbit_pans: bool,
    /// A multi-finger tap must lift within this many seconds...
    pub tap_max_duration: f64,
    /// ...and move less than this many pixels in total.
    pub tap_max_travel: f32,
    /// Camera distance multiplier per wheel notch toward the user.
    pub wheel_zoom_step: f32,
    /// Trackpad scroll pixels → drag pixels.
    pub trackpad_scale: f32,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            finger_draws: false,
            mouse_left: DragAction::Draw,
            mouse_right: DragAction::Orbit,
            mouse_middle: DragAction::Pan,
            shift_orbit_pans: true,
            tap_max_duration: 0.3,
            tap_max_travel: 20.0,
            wheel_zoom_step: 0.9,
            trackpad_scale: 8.0,
        }
    }
}

impl RouterConfig {
    fn mouse_action(&self, button: MouseButton, m: Modifiers) -> DragAction {
        let action = match button {
            MouseButton::Left => self.mouse_left,
            MouseButton::Right => self.mouse_right,
            MouseButton::Middle => self.mouse_middle,
        };
        if action == DragAction::Orbit && m.shift && self.shift_orbit_pans { DragAction::Pan } else { action }
    }
}

/// Tracks one multi-finger contact sequence to detect 2/3-finger taps.
struct TapGesture {
    start_t: f64,
    max_fingers: usize,
    travel: f32,
}

pub struct InputRouter {
    pub config: RouterConfig,
    /// Pointer that owns the current stroke.
    stroke: Option<PointerId>,
    pens: HashSet<PointerId>,
    /// Mouse drags: action and last position.
    drags: HashMap<PointerId, (DragAction, Vec2)>,
    /// Fingers in arrival order with their last position.
    fingers: Vec<(PointerId, Vec2)>,
    /// Contacts ignored until they lift (palm rejection).
    rejected: HashSet<PointerId>,
    tap: Option<TapGesture>,
    /// Centroid and spread of the first two fingers at the last update.
    pinch_ref: Option<(Vec2, f32)>,
    /// Screen rectangles (min, max) covered by app UI; contacts starting there are ignored.
    ui_regions: Vec<(Vec2, Vec2)>,
}

impl InputRouter {
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config,
            stroke: None,
            pens: HashSet::new(),
            drags: HashMap::new(),
            fingers: Vec::new(),
            rejected: HashSet::new(),
            tap: None,
            pinch_ref: None,
            ui_regions: Vec::new(),
        }
    }

    /// Sets the screen areas covered by UI. Needed when a backend sees touches before (or
    /// independently of) the UI toolkit, as the native touch backends do.
    pub fn set_ui_regions(&mut self, regions: Vec<(Vec2, Vec2)>) {
        self.ui_regions = regions;
    }

    fn over_ui(&self, p: Vec2) -> bool {
        self.ui_regions.iter().any(|(min, max)| p.cmpge(*min).all() && p.cmplt(*max).all())
    }

    pub fn route(&mut self, event: PenEvent, out: &mut Vec<Routed>) {
        match event {
            PenEvent::Down { id, tool, sample, modifiers } => self.down(id, tool, sample, modifiers, out),
            PenEvent::Move { id, samples, predicted } => self.moved(id, samples, predicted, out),
            PenEvent::Up { id, sample } => self.up(id, sample, out),
            PenEvent::Cancel { id } => self.cancel(id, out),
            PenEvent::Hover { sample, .. } => out.push(Routed::Hover(Some(sample))),
            PenEvent::HoverExit => out.push(Routed::Hover(None)),
            PenEvent::Action(a) => out.push(Routed::Command(Command::Pen(a))),
            PenEvent::Scroll { pos, delta, precise, modifiers } => self.scroll(pos, delta, precise, modifiers, out),
            PenEvent::Pinch { pos, factor } => {
                if factor > 0.0 {
                    out.push(Routed::View(ViewEvent::Zoom { factor: 1.0 / factor, at: pos }));
                }
            }
        }
    }

    fn down(&mut self, id: PointerId, tool: PointerTool, sample: PenSample, m: Modifiers, out: &mut Vec<Routed>) {
        if self.over_ui(sample.pos) {
            self.rejected.insert(id);
            return;
        }
        match tool {
            PointerTool::Pen | PointerTool::Eraser => {
                // Palm rejection: a pen touching down ends every finger contact.
                if let Some(owner) = self.stroke
                    && self.fingers.iter().any(|(f, _)| *f == owner)
                {
                    out.push(Routed::Stroke(StrokeEvent::Cancel));
                    self.stroke = None;
                }
                self.rejected.extend(self.fingers.drain(..).map(|(f, _)| f));
                self.tap = None;
                self.pinch_ref = None;
                self.pens.insert(id);
                let tool = if tool == PointerTool::Eraser { DrawTool::Eraser } else { DrawTool::Brush };
                self.begin_stroke(id, tool, sample, out);
            }
            PointerTool::Mouse(button) => {
                let action = self.config.mouse_action(button, m);
                if action == DragAction::Draw {
                    self.begin_stroke(id, DrawTool::Brush, sample, out);
                }
                self.drags.insert(id, (action, sample.pos));
            }
            PointerTool::Finger => {
                if !self.pens.is_empty() {
                    self.rejected.insert(id);
                    return;
                }
                match &mut self.tap {
                    Some(tap) => tap.max_fingers = tap.max_fingers.max(self.fingers.len() + 1),
                    None => self.tap = Some(TapGesture { start_t: sample.t, max_fingers: 1, travel: 0.0 }),
                }
                self.fingers.push((id, sample.pos));
                if self.config.finger_draws && self.fingers.len() == 1 {
                    self.begin_stroke(id, DrawTool::Brush, sample, out);
                } else if let Some(owner) = self.stroke
                    && self.fingers.iter().any(|(f, _)| *f == owner)
                {
                    // A second finger turns a finger stroke into a view gesture.
                    out.push(Routed::Stroke(StrokeEvent::Cancel));
                    self.stroke = None;
                }
                self.pinch_ref = self.pinch();
            }
        }
    }

    fn moved(&mut self, id: PointerId, samples: Vec<PenSample>, predicted: Vec<PenSample>, out: &mut Vec<Routed>) {
        if self.rejected.contains(&id) {
            return;
        }
        let Some(last) = samples.last().map(|s| s.pos) else { return };

        if let Some(idx) = self.fingers.iter().position(|(f, _)| *f == id) {
            let delta = last - self.fingers[idx].1;
            self.fingers[idx].1 = last;
            if let Some(tap) = &mut self.tap {
                tap.travel += delta.length();
            }
            if self.stroke == Some(id) {
                out.push(Routed::Stroke(StrokeEvent::Extend { samples, predicted }));
            } else if self.fingers.len() == 1 {
                push_view(out, ViewEvent::Orbit(delta));
            } else if let Some((c, spread)) = self.pinch() {
                if let Some((c0, spread0)) = self.pinch_ref {
                    push_view(out, ViewEvent::Pan(c - c0));
                    if spread0 > 0.0 && spread > 0.0 && spread != spread0 {
                        out.push(Routed::View(ViewEvent::Zoom { factor: spread0 / spread, at: c }));
                    }
                }
                self.pinch_ref = Some((c, spread));
            }
            return;
        }

        if self.stroke == Some(id) {
            out.push(Routed::Stroke(StrokeEvent::Extend { samples, predicted }));
        }
        if let Some((action, prev)) = self.drags.get_mut(&id) {
            let delta = last - *prev;
            *prev = last;
            match action {
                DragAction::Orbit => push_view(out, ViewEvent::Orbit(delta)),
                DragAction::Pan => push_view(out, ViewEvent::Pan(delta)),
                DragAction::Draw | DragAction::None => {}
            }
        }
    }

    fn up(&mut self, id: PointerId, sample: PenSample, out: &mut Vec<Routed>) {
        if self.rejected.remove(&id) {
            return;
        }
        if self.stroke == Some(id) {
            out.push(Routed::Stroke(StrokeEvent::End { sample }));
            self.stroke = None;
        }
        self.pens.remove(&id);
        self.drags.remove(&id);
        if let Some(idx) = self.fingers.iter().position(|(f, _)| *f == id) {
            let (_, pos) = self.fingers.remove(idx);
            if let Some(tap) = &mut self.tap {
                tap.travel += (sample.pos - pos).length();
            }
            if self.fingers.is_empty() {
                if let Some(tap) = self.tap.take()
                    && sample.t - tap.start_t <= self.config.tap_max_duration
                    && tap.travel <= self.config.tap_max_travel
                {
                    match tap.max_fingers {
                        2 => out.push(Routed::Command(Command::Undo)),
                        3 => out.push(Routed::Command(Command::Redo)),
                        _ => {}
                    }
                }
            }
            self.pinch_ref = self.pinch();
        }
    }

    fn cancel(&mut self, id: PointerId, out: &mut Vec<Routed>) {
        self.rejected.remove(&id);
        if self.stroke == Some(id) {
            out.push(Routed::Stroke(StrokeEvent::Cancel));
            self.stroke = None;
        }
        self.pens.remove(&id);
        self.drags.remove(&id);
        if let Some(idx) = self.fingers.iter().position(|(f, _)| *f == id) {
            self.fingers.remove(idx);
            self.tap = None;
            self.pinch_ref = self.pinch();
        }
    }

    fn scroll(&mut self, pos: Vec2, delta: Vec2, precise: bool, m: Modifiers, out: &mut Vec<Routed>) {
        if m.alt {
            if delta.y != 0.0 {
                out.push(Routed::Command(Command::NudgeGuide(delta.y)));
            }
        } else if precise {
            let d = delta * self.config.trackpad_scale;
            push_view(out, if m.shift { ViewEvent::Pan(d) } else { ViewEvent::Orbit(d) });
        } else if delta.y != 0.0 {
            out.push(Routed::View(ViewEvent::Zoom { factor: self.config.wheel_zoom_step.powf(delta.y), at: pos }));
        }
    }

    fn begin_stroke(&mut self, id: PointerId, tool: DrawTool, sample: PenSample, out: &mut Vec<Routed>) {
        if self.stroke.is_none() {
            self.stroke = Some(id);
            out.push(Routed::Stroke(StrokeEvent::Begin { tool, sample }));
        }
    }

    fn pinch(&self) -> Option<(Vec2, f32)> {
        match self.fingers.as_slice() {
            [(_, a), (_, b), ..] => Some(((*a + *b) * 0.5, a.distance(*b))),
            _ => None,
        }
    }
}

fn push_view(out: &mut Vec<Routed>, v: ViewEvent) {
    let zero = match v {
        ViewEvent::Orbit(d) | ViewEvent::Pan(d) => d == Vec2::ZERO,
        ViewEvent::Zoom { factor, .. } => factor == 1.0,
    };
    if !zero {
        out.push(Routed::View(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f32, y: f32, t: f64) -> PenSample {
        PenSample::at(Vec2::new(x, y), t)
    }

    fn down(id: u64, tool: PointerTool, x: f32, y: f32, t: f64) -> PenEvent {
        PenEvent::Down { id: PointerId(id), tool, sample: s(x, y, t), modifiers: Modifiers::default() }
    }

    fn mv(id: u64, x: f32, y: f32, t: f64) -> PenEvent {
        PenEvent::Move { id: PointerId(id), samples: vec![s(x, y, t)], predicted: vec![] }
    }

    fn up(id: u64, x: f32, y: f32, t: f64) -> PenEvent {
        PenEvent::Up { id: PointerId(id), sample: s(x, y, t) }
    }

    fn run(r: &mut InputRouter, events: Vec<PenEvent>) -> Vec<Routed> {
        let mut out = Vec::new();
        for e in events {
            r.route(e, &mut out);
        }
        out
    }

    #[test]
    fn pen_draws_a_stroke() {
        let mut r = InputRouter::new(RouterConfig::default());
        let out = run(&mut r, vec![down(1, PointerTool::Pen, 0., 0., 0.), mv(1, 5., 0., 0.01), up(1, 6., 0., 0.02)]);
        assert!(matches!(out[0], Routed::Stroke(StrokeEvent::Begin { tool: DrawTool::Brush, .. })));
        assert!(matches!(out[1], Routed::Stroke(StrokeEvent::Extend { .. })));
        assert!(matches!(out[2], Routed::Stroke(StrokeEvent::End { .. })));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn one_finger_orbits() {
        let mut r = InputRouter::new(RouterConfig::default());
        let out = run(&mut r, vec![down(1, PointerTool::Finger, 0., 0., 0.), mv(1, 10., 4., 0.1)]);
        assert_eq!(out, vec![Routed::View(ViewEvent::Orbit(Vec2::new(10., 4.)))]);
    }

    #[test]
    fn two_fingers_pan_and_zoom() {
        let mut r = InputRouter::new(RouterConfig::default());
        let out = run(
            &mut r,
            vec![down(1, PointerTool::Finger, 0., 0., 0.), down(2, PointerTool::Finger, 100., 0., 0.), mv(2, 200., 0., 0.1)],
        );
        // Centroid moved 50px right, spread doubled → zoom in by half.
        assert!(out.contains(&Routed::View(ViewEvent::Pan(Vec2::new(50., 0.)))));
        assert!(out.iter().any(|e| matches!(e, Routed::View(ViewEvent::Zoom { factor, .. }) if *factor == 0.5)));
    }

    #[test]
    fn two_finger_tap_undoes_three_finger_tap_redoes() {
        let mut r = InputRouter::new(RouterConfig::default());
        let out = run(
            &mut r,
            vec![down(1, PointerTool::Finger, 0., 0., 0.), down(2, PointerTool::Finger, 50., 0., 0.01), up(1, 0., 0., 0.1), up(2, 50., 0., 0.1)],
        );
        assert_eq!(out, vec![Routed::Command(Command::Undo)]);
        let out = run(
            &mut r,
            vec![
                down(3, PointerTool::Finger, 0., 0., 1.0),
                down(4, PointerTool::Finger, 50., 0., 1.0),
                down(5, PointerTool::Finger, 90., 0., 1.0),
                up(3, 0., 0., 1.1),
                up(4, 50., 0., 1.1),
                up(5, 90., 0., 1.1),
            ],
        );
        assert_eq!(out, vec![Routed::Command(Command::Redo)]);
    }

    #[test]
    fn slow_or_moving_taps_are_not_commands() {
        let mut r = InputRouter::new(RouterConfig::default());
        let slow = run(
            &mut r,
            vec![down(1, PointerTool::Finger, 0., 0., 0.), down(2, PointerTool::Finger, 50., 0., 0.), up(1, 0., 0., 1.0), up(2, 50., 0., 1.0)],
        );
        assert!(!slow.iter().any(|e| matches!(e, Routed::Command(_))));
        let moved = run(
            &mut r,
            vec![
                down(1, PointerTool::Finger, 0., 0., 2.0),
                down(2, PointerTool::Finger, 50., 0., 2.0),
                mv(2, 150., 0., 2.05),
                up(1, 0., 0., 2.1),
                up(2, 150., 0., 2.1),
            ],
        );
        assert!(!moved.iter().any(|e| matches!(e, Routed::Command(_))));
    }

    #[test]
    fn palm_is_rejected_while_pen_is_down() {
        let mut r = InputRouter::new(RouterConfig::default());
        let out = run(
            &mut r,
            vec![
                down(9, PointerTool::Finger, 0., 0., 0.),   // palm lands first
                down(1, PointerTool::Pen, 10., 10., 0.01),  // pen arrives, palm rejected
                mv(9, 40., 40., 0.02),                      // palm moves: ignored
                down(8, PointerTool::Finger, 0., 0., 0.03), // another palm contact: ignored
                mv(1, 20., 10., 0.04),
                up(9, 40., 40., 0.05),
                up(1, 20., 10., 0.06),
            ],
        );
        assert!(!out.iter().any(|e| matches!(e, Routed::View(_) | Routed::Command(_))));
        assert_eq!(out.iter().filter(|e| matches!(e, Routed::Stroke(_))).count(), 3);
    }

    #[test]
    fn finger_draws_mode_and_second_finger_cancels() {
        let mut r = InputRouter::new(RouterConfig { finger_draws: true, ..Default::default() });
        let out = run(&mut r, vec![down(1, PointerTool::Finger, 0., 0., 0.), mv(1, 5., 0., 0.1), down(2, PointerTool::Finger, 50., 0., 0.2)]);
        assert!(matches!(out[0], Routed::Stroke(StrokeEvent::Begin { .. })));
        assert!(matches!(out[1], Routed::Stroke(StrokeEvent::Extend { .. })));
        assert_eq!(out[2], Routed::Stroke(StrokeEvent::Cancel));
    }

    #[test]
    fn mouse_bindings_and_shift_pan() {
        let mut r = InputRouter::new(RouterConfig::default());
        let right = PointerTool::Mouse(MouseButton::Right);
        let out = run(&mut r, vec![down(100, right, 0., 0., 0.), mv(100, 3., 0., 0.1), up(100, 3., 0., 0.2)]);
        assert_eq!(out, vec![Routed::View(ViewEvent::Orbit(Vec2::new(3., 0.)))]);
        let shifted = PenEvent::Down {
            id: PointerId(100),
            tool: right,
            sample: s(0., 0., 1.),
            modifiers: Modifiers { shift: true, ..Default::default() },
        };
        let out = run(&mut r, vec![shifted, mv(100, 0., 7., 1.1)]);
        assert_eq!(out, vec![Routed::View(ViewEvent::Pan(Vec2::new(0., 7.)))]);
    }

    #[test]
    fn wheel_zooms_alt_wheel_nudges_trackpad_orbits() {
        let mut r = InputRouter::new(RouterConfig::default());
        let m = Modifiers::default();
        let out = run(
            &mut r,
            vec![
                PenEvent::Scroll { pos: Vec2::ZERO, delta: Vec2::new(0., 1.), precise: false, modifiers: m },
                PenEvent::Scroll { pos: Vec2::ZERO, delta: Vec2::new(0., -1.), precise: false, modifiers: Modifiers { alt: true, ..m } },
                PenEvent::Scroll { pos: Vec2::ZERO, delta: Vec2::new(1., 0.), precise: true, modifiers: m },
            ],
        );
        assert_eq!(
            out,
            vec![
                Routed::View(ViewEvent::Zoom { factor: 0.9, at: Vec2::ZERO }),
                Routed::Command(Command::NudgeGuide(-1.)),
                Routed::View(ViewEvent::Orbit(Vec2::new(8., 0.))),
            ]
        );
    }

    #[test]
    fn contacts_starting_on_ui_are_ignored_until_lifted() {
        let mut r = InputRouter::new(RouterConfig::default());
        r.set_ui_regions(vec![(Vec2::ZERO, Vec2::new(1000., 60.))]);
        let out = run(&mut r, vec![down(1, PointerTool::Pen, 50., 30., 0.), mv(1, 50., 300., 0.1), up(1, 50., 300., 0.2)]);
        assert!(out.is_empty(), "pen pressed on the toolbar must not draw: {out:?}");
        let out = run(&mut r, vec![down(2, PointerTool::Pen, 50., 300., 1.0), mv(2, 50., 30., 1.1)]);
        assert_eq!(out.len(), 2, "a stroke that starts on the canvas may cross the toolbar");
    }

    #[test]
    fn os_cancel_discards_stroke() {
        let mut r = InputRouter::new(RouterConfig::default());
        let out = run(&mut r, vec![down(1, PointerTool::Pen, 0., 0., 0.), PenEvent::Cancel { id: PointerId(1) }]);
        assert_eq!(out[1], Routed::Stroke(StrokeEvent::Cancel));
    }
}
