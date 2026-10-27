//! Streaming brush pipeline for strokes that grow while they're drawn (docs/ARCHITECTURE.md
//! §3e). Each stage splits its output into a *committed* part, which no later input can
//! change, and a *provisional* part: what the output would be if the stroke ended now.
//! [`Brush::build`](crate::Brush::build) is push-everything + finish, so a live stroke ends
//! exactly as it rebuilds later from its raw points.

use std::sync::Arc;

use glam::Vec3;

use crate::params::Params;
use crate::stage::{Dynamics, Filter, Geometry, InputPoint, MeshBuffers, StrokePoint};

/// Streaming form of a filter or dynamics stage, created per stroke.
pub trait PointStream: Send {
    /// Consumes input that will never change and appends output that will never change.
    /// Must be split-invariant: `commit(a); commit(b)` ≡ `commit(a ++ b)`.
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>);
    /// Appends the rest of the output as if the stroke ended after `tail` (input that was
    /// never committed to this stage). Leaves the stream unchanged.
    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>);
}

/// Streaming form of a geometry stage, created per stroke.
pub trait MeshStream: Send {
    /// Appends geometry that will never change to `out`, which holds all committed geometry
    /// of the stroke so far. Split-invariant like [`PointStream::commit`].
    fn commit(&mut self, input: &[StrokePoint], out: &mut MeshBuffers);
    /// Geometry for the rest of the stroke if it ended after `tail`, into an empty `out`.
    /// Its indices count the committed vertices first, then `out`'s own.
    fn finish(&self, tail: &[StrokePoint], out: &mut MeshBuffers);
}

/// A stroke's pipeline in progress: one stream per stage plus everything committed so far.
pub struct BrushStream {
    /// Filters, then the distance pass, then dynamics.
    points: Vec<Box<dyn PointStream>>,
    geometry: Box<dyn MeshStream>,
    committed_points: Vec<StrokePoint>,
    committed_mesh: MeshBuffers,
}

impl BrushStream {
    pub(crate) fn new(
        filters: &[(Arc<dyn Filter>, Params)],
        dynamics: &[(Arc<dyn Dynamics>, Params)],
        geometry: &(Arc<dyn Geometry>, Params),
    ) -> Self {
        let mut points: Vec<Box<dyn PointStream>> = Vec::with_capacity(filters.len() + dynamics.len() + 1);
        for (f, p) in filters {
            points.push(f.stream(p).unwrap_or_else(|| Box::new(BatchFilter::new(f, p))));
        }
        points.push(Box::new(Distance::default()));
        for (d, p) in dynamics {
            points.push(d.stream(p).unwrap_or_else(|| Box::new(BatchDynamics::new(d, p))));
        }
        let (g, p) = geometry;
        let geometry = g.stream(p).unwrap_or_else(|| Box::new(BatchGeometry::new(g, p)));
        Self { points, geometry, committed_points: Vec::new(), committed_mesh: MeshBuffers::default() }
    }

    /// Adds input points that will never change (real pen samples, not predicted ones).
    pub fn push(&mut self, input: &[InputPoint]) {
        let mut cur: Vec<StrokePoint> = input.iter().map(StrokePoint::from).collect();
        let mut next = Vec::new();
        for s in &mut self.points {
            next.clear();
            s.commit(&cur, &mut next);
            std::mem::swap(&mut cur, &mut next);
        }
        self.geometry.commit(&cur, &mut self.committed_mesh);
        self.committed_points.extend_from_slice(&cur);
    }

    /// Geometry that will never change; it only grows as points are pushed.
    pub fn committed_mesh(&self) -> &MeshBuffers {
        &self.committed_mesh
    }

    /// The rest of the mesh if the stroke ended after `tail` (e.g. predicted points). Indices
    /// count the committed vertices first, then this mesh's own.
    pub fn preview(&self, tail: &[InputPoint]) -> MeshBuffers {
        let pts = self.provisional_points(tail);
        let mut out = MeshBuffers::default();
        self.geometry.finish(&pts, &mut out);
        out
    }

    /// The whole mesh if the stroke ended after `tail`: committed geometry plus [`preview`](Self::preview).
    pub fn mesh(&self, tail: &[InputPoint]) -> MeshBuffers {
        let preview = self.preview(tail);
        if self.committed_mesh.positions.is_empty() {
            return preview;
        }
        let mut out = self.committed_mesh.clone();
        out.positions.extend_from_slice(&preview.positions);
        out.normals.extend_from_slice(&preview.normals);
        out.indices.extend_from_slice(&preview.indices);
        out
    }

    /// The whole processed centerline if the stroke ended after `tail`.
    pub fn centerline(&self, tail: &[InputPoint]) -> Vec<StrokePoint> {
        let mut out = self.committed_points.clone();
        out.extend(self.provisional_points(tail));
        out
    }

    /// Runs `tail` and every stage's held-back input through the stages' finish.
    fn provisional_points(&self, tail: &[InputPoint]) -> Vec<StrokePoint> {
        let mut cur: Vec<StrokePoint> = tail.iter().map(StrokePoint::from).collect();
        let mut next = Vec::new();
        for s in &self.points {
            next.clear();
            s.finish(&cur, &mut next);
            std::mem::swap(&mut cur, &mut next);
        }
        cur
    }
}

/// Distance along the stroke, between filters and dynamics. Causal, so nothing is held back.
#[derive(Default)]
struct Distance {
    /// Position and distance of the last committed point.
    last: Option<(Vec3, f32)>,
}

impl Distance {
    fn run(mut last: Option<(Vec3, f32)>, input: &[StrokePoint], out: &mut Vec<StrokePoint>) -> Option<(Vec3, f32)> {
        for p in input {
            let dist = last.map_or(0.0, |(prev, d)| d + p.pos.distance(prev));
            out.push(StrokePoint { dist, ..*p });
            last = Some((p.pos, dist));
        }
        last
    }
}

impl PointStream for Distance {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        self.last = Self::run(self.last, input, out);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        Self::run(self.last, tail, out);
    }
}

/// Fallback for stages without a streaming form: holds all input back and runs the stage on
/// the whole stroke in `finish`, which costs what a full rebuild does.
struct BatchFilter {
    stage: Arc<dyn Filter>,
    params: Params,
    held: Vec<StrokePoint>,
}

impl BatchFilter {
    fn new(stage: &Arc<dyn Filter>, params: &Params) -> Self {
        Self { stage: stage.clone(), params: params.clone(), held: Vec::new() }
    }
}

impl PointStream for BatchFilter {
    fn commit(&mut self, input: &[StrokePoint], _out: &mut Vec<StrokePoint>) {
        self.held.extend_from_slice(input);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        let mut all = held_and(&self.held, tail);
        self.stage.apply(&mut all, &self.params);
        out.extend(all);
    }
}

/// Batch fallback for dynamics stages; see [`BatchFilter`].
struct BatchDynamics {
    stage: Arc<dyn Dynamics>,
    params: Params,
    held: Vec<StrokePoint>,
}

impl BatchDynamics {
    fn new(stage: &Arc<dyn Dynamics>, params: &Params) -> Self {
        Self { stage: stage.clone(), params: params.clone(), held: Vec::new() }
    }
}

impl PointStream for BatchDynamics {
    fn commit(&mut self, input: &[StrokePoint], _out: &mut Vec<StrokePoint>) {
        self.held.extend_from_slice(input);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        let mut all = held_and(&self.held, tail);
        self.stage.apply(&mut all, &self.params);
        out.extend(all);
    }
}

/// Batch fallback for geometry stages; see [`BatchFilter`]. Commits no geometry, so
/// `finish` output indices start at 0.
struct BatchGeometry {
    stage: Arc<dyn Geometry>,
    params: Params,
    held: Vec<StrokePoint>,
}

impl BatchGeometry {
    fn new(stage: &Arc<dyn Geometry>, params: &Params) -> Self {
        Self { stage: stage.clone(), params: params.clone(), held: Vec::new() }
    }
}

impl MeshStream for BatchGeometry {
    fn commit(&mut self, input: &[StrokePoint], _out: &mut MeshBuffers) {
        self.held.extend_from_slice(input);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut MeshBuffers) {
        self.stage.build(&held_and(&self.held, tail), &self.params, out);
    }
}

fn held_and(held: &[StrokePoint], tail: &[StrokePoint]) -> Vec<StrokePoint> {
    let mut all = Vec::with_capacity(held.len() + tail.len());
    all.extend_from_slice(held);
    all.extend_from_slice(tail);
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushLibrary, StageRegistry};

    fn stroke(n: usize) -> Vec<InputPoint> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 240.0;
                InputPoint {
                    pos: Vec3::new(t.cos() * t * 0.3, t.sin() * t * 0.3, 0.0),
                    normal: Vec3::Z,
                    pressure: 0.5 + 0.5 * (t * 7.0).sin(),
                    tilt: None,
                    t: t as f64,
                }
            })
            .collect()
    }

    /// Deterministic split sizes from 1 to 40, so commits land at many different points.
    fn splits(n: usize) -> Vec<usize> {
        let (mut out, mut x, mut left) = (Vec::new(), 12345u32, n);
        while left > 0 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let k = ((x >> 16) as usize % 40 + 1).min(left);
            out.push(k);
            left -= k;
        }
        out
    }

    #[test]
    fn pushing_in_pieces_matches_build() {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        for brush in lib.iter() {
            for n in [0, 1, 2, 3, 50, 600] {
                let input = stroke(n);
                let expected = brush.build(&input);
                let mut s = brush.stream();
                let mut at = 0;
                for k in splits(n) {
                    s.push(&input[at..at + k]);
                    at += k;
                    // The live view of the stroke so far is the stroke built from it.
                    assert_eq!(s.mesh(&[]), brush.build(&input[..at]), "{} at {at}/{n}", brush.id());
                }
                assert_eq!(s.mesh(&[]), expected, "{} n={n}", brush.id());
                assert_eq!(s.centerline(&[]), brush.centerline(&input), "{} n={n}", brush.id());
            }
        }
    }

    #[test]
    fn preview_matches_build_with_tail() {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        let input = stroke(300);
        for brush in lib.iter() {
            let mut s = brush.stream();
            s.push(&input[..280]);
            assert_eq!(s.mesh(&input[280..]), brush.build(&input), "{}", brush.id());
            // Previewing doesn't change the stream.
            assert_eq!(s.mesh(&[]), brush.build(&input[..280]), "{}", brush.id());
        }
    }
}
