//! Stroke data and the four stage kinds a brush is built from:
//! input points → [Filter]s → [Dynamics] → [Geometry] → mesh, plus a [Material].

use glam::Vec3;

use crate::params::{ParamSpec, Params};

/// One raw input point in world space, after projection onto the drawing surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputPoint {
    pub pos: Vec3,
    /// Normal of the surface the point was drawn on (flat brushes lie on it).
    pub normal: Vec3,
    pub pressure: f32,
    /// Pen altitude in radians, if known.
    pub tilt: Option<f32>,
    /// Seconds.
    pub t: f64,
}

/// A point as it moves through the pipeline. Filters change positions; dynamics set
/// per-point attributes; geometry turns the result into a mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePoint {
    pub pos: Vec3,
    pub normal: Vec3,
    pub pressure: f32,
    pub tilt: Option<f32>,
    pub t: f64,
    /// Distance along the stroke from its start, filled in after filters run.
    pub dist: f32,
    /// World units. Starts at [`DEFAULT_WIDTH`]; dynamics stages change it.
    pub width: f32,
}

pub const DEFAULT_WIDTH: f32 = 0.01;

impl From<&InputPoint> for StrokePoint {
    fn from(p: &InputPoint) -> Self {
        Self { pos: p.pos, normal: p.normal, pressure: p.pressure, tilt: p.tilt, t: p.t, dist: 0.0, width: DEFAULT_WIDTH }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshBuffers {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub indices: Vec<i32>,
}

impl MeshBuffers {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shading {
    Lit,
    Unshaded,
}

/// Engine-neutral material; the Godot layer (and exporters) translate it.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDesc {
    pub shading: Shading,
    /// sRGB RGBA.
    pub color: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    /// Emission strength; 0 disables emission. Uses `color`.
    pub emission: f32,
    pub double_sided: bool,
}

/// Common to all stages: the parameters they accept.
pub trait Stage: Send + Sync {
    fn schema(&self) -> Vec<ParamSpec>;
}

/// Reshapes the point list: smoothing, resampling, subdivision.
pub trait Filter: Stage {
    fn apply(&self, points: &mut Vec<StrokePoint>, params: &Params);
}

/// Sets per-point attributes such as width.
pub trait Dynamics: Stage {
    fn apply(&self, points: &mut [StrokePoint], params: &Params);
}

/// Turns points into triangles.
pub trait Geometry: Stage {
    fn build(&self, points: &[StrokePoint], params: &Params, out: &mut MeshBuffers);
}

/// Describes how the stroke is shaded.
pub trait Material: Stage {
    fn describe(&self, params: &Params) -> MaterialDesc;
}
