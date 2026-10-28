//! The unified stylus model. Every platform backend (UIKit, Android MotionEvent, Godot events,
//! Windows Ink) converts its native events into these types; nothing above this layer knows
//! which OS produced them.

mod router;

pub use router::{Command, DragAction, DrawTool, InputRouter, Routed, RouterConfig, StrokeEvent, ViewEvent};

use glam::Vec2;

/// Identifies one pointer for the duration of a contact (one pen, or one finger).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointerId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Each held mouse button is its own pointer contact, so one mouse can drive two drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerTool {
    Pen,
    Eraser,
    Finger,
    Mouse(MouseButton),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    /// Space held: a left drag pans (as in most design apps).
    pub space: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    /// The sample the OS delivered for this event.
    Real,
    /// An intermediate sample between frames (e.g. 240 Hz Pencil data on a 120 Hz display).
    Coalesced,
    /// Where the OS estimates the pointer will be; replaced once real samples arrive.
    Predicted,
}

/// One pointer measurement. Optional fields are `None` when the device can't measure them,
/// so consumers fall back to defaults (e.g. a mouse has no pressure).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    /// Position in physical screen pixels, origin top-left.
    pub pos: Vec2,
    /// Seconds on a monotonic clock shared by all backends.
    pub t: f64,
    /// Normalized 0..1.
    pub pressure: Option<f32>,
    /// Altitude angle in radians: 0 = flat against the screen, PI/2 = perpendicular.
    pub tilt: Option<f32>,
    /// Direction the pen leans, radians, 0 = pointing right, counter-clockwise.
    pub azimuth: Option<f32>,
    /// Barrel roll in radians (Apple Pencil Pro).
    pub roll: Option<f32>,
    pub kind: SampleKind,
}

impl PenSample {
    /// A sample with only position and time, as a mouse would produce.
    pub fn at(pos: Vec2, t: f64) -> Self {
        Self { pos, t, pressure: None, tilt: None, azimuth: None, roll: None, kind: SampleKind::Real }
    }

    /// Pressure, or `default` when the device doesn't report it.
    pub fn pressure_or(&self, default: f32) -> f32 {
        self.pressure.unwrap_or(default)
    }
}

/// Discrete actions from pen hardware. What each one does is decided by the input router.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenAction {
    Squeeze,
    DoubleTap,
    BarrelButton,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PenEvent {
    Down {
        id: PointerId,
        tool: PointerTool,
        sample: PenSample,
        modifiers: Modifiers,
    },
    /// `samples` are in time order and end with the latest real sample.
    /// `predicted` replaces any previously delivered prediction for this pointer.
    Move {
        id: PointerId,
        samples: Vec<PenSample>,
        predicted: Vec<PenSample>,
    },
    Up {
        id: PointerId,
        sample: PenSample,
    },
    /// The OS took the contact away (palm rejection, system gesture). Discard the stroke.
    Cancel {
        id: PointerId,
    },
    /// Pen near the screen but not touching.
    Hover {
        tool: PointerTool,
        sample: PenSample,
    },
    HoverExit,
    Action(PenAction),
    /// Mouse wheel (`precise == false`, delta in notches, +y = away from user) or trackpad
    /// two-finger scroll (`precise == true`, delta in pixels).
    Scroll {
        pos: Vec2,
        delta: Vec2,
        precise: bool,
        modifiers: Modifiers,
    },
    /// Trackpad pinch. `factor > 1` means fingers spreading apart.
    Pinch {
        pos: Vec2,
        factor: f32,
    },
}

impl PenEvent {
    pub fn pointer(&self) -> Option<PointerId> {
        match self {
            PenEvent::Down { id, .. } | PenEvent::Move { id, .. } | PenEvent::Up { id, .. } | PenEvent::Cancel { id } => Some(*id),
            _ => None,
        }
    }
}

/// What the current input device can report. The UI uses it to hide settings that do nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceCaps {
    pub pressure: bool,
    pub tilt: bool,
    pub azimuth: bool,
    pub roll: bool,
    pub hover: bool,
    pub predicted: bool,
    pub coalesced: bool,
    /// Samples per second the device reports, 0 if unknown.
    pub report_rate_hz: u32,
}

impl DeviceCaps {
    pub const MOUSE: Self = Self {
        pressure: false,
        tilt: false,
        azimuth: false,
        roll: false,
        hover: false,
        predicted: false,
        coalesced: false,
        report_rate_hz: 0,
    };

    /// Combines capabilities of several devices (e.g. mouse and tablet both attached).
    pub fn union(self, o: Self) -> Self {
        Self {
            pressure: self.pressure || o.pressure,
            tilt: self.tilt || o.tilt,
            azimuth: self.azimuth || o.azimuth,
            roll: self.roll || o.roll,
            hover: self.hover || o.hover,
            predicted: self.predicted || o.predicted,
            coalesced: self.coalesced || o.coalesced,
            report_rate_hz: self.report_rate_hz.max(o.report_rate_hz),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_pressure_falls_back() {
        let s = PenSample::at(Vec2::ZERO, 0.0);
        assert_eq!(s.pressure_or(1.0), 1.0);
        let s = PenSample { pressure: Some(0.3), ..s };
        assert_eq!(s.pressure_or(1.0), 0.3);
    }

    #[test]
    fn caps_union() {
        let pen = DeviceCaps { pressure: true, report_rate_hz: 240, ..DeviceCaps::MOUSE };
        let both = DeviceCaps::MOUSE.union(pen);
        assert!(both.pressure);
        assert_eq!(both.report_rate_hz, 240);
    }

    #[test]
    fn pointer_id_only_for_contact_events() {
        let id = PointerId(7);
        assert_eq!(PenEvent::Cancel { id }.pointer(), Some(id));
        assert_eq!(PenEvent::HoverExit.pointer(), None);
    }
}
