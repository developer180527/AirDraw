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
        let mut out = self.committed_mesh.clone();
        out.append(self.preview(tail));
        out
    }

    /// [`mesh`](Self::mesh) for a stroke that's done, without copying the committed geometry.
    pub fn into_mesh(mut self, tail: &[InputPoint]) -> MeshBuffers {
        let preview = self.preview(tail);
        self.committed_mesh.append(preview);
        self.committed_mesh
    }

    /// The whole processed centerline if the stroke ended after `tail`.
    pub fn centerline(&self, tail: &[InputPoint]) -> Vec<StrokePoint> {
        let mut out = self.committed_points.clone();
        out.extend(self.provisional_points(tail));
        out
    }

    /// [`centerline`](Self::centerline) for a stroke that's done, without copying.
    pub fn into_centerline(mut self, tail: &[InputPoint]) -> Vec<StrokePoint> {
        let rest = self.provisional_points(tail);
        self.committed_points.extend(rest);
        self.committed_points
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
    use crate::{Brush, BrushDef, BrushLibrary, StageRegistry};

    fn stroke(n: usize) -> Vec<InputPoint> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 240.0;
                InputPoint {
                    pos: Vec3::new(t.cos() * t * 0.3, t.sin() * t * 0.3, (t * 3.0).sin() * 0.05),
                    normal: Vec3::Z,
                    pressure: 0.5 + 0.5 * (t * 7.0).sin(),
                    tilt: if i % 3 == 0 { None } else { Some(0.8) },
                    // Pairs share a timestamp, as batched events do.
                    t: (i / 2) as f64 / 120.0,
                }
            })
            .collect()
    }

    /// Split sizes from 1 to `max`, deterministic per seed.
    fn splits(n: usize, max: usize, seed: u32) -> Vec<usize> {
        let (mut out, mut x, mut left) = (Vec::new(), seed, n);
        while left > 0 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let k = ((x >> 16) as usize % max + 1).min(left);
            out.push(k);
            left -= k;
        }
        out
    }

    /// Built-in brushes plus brushes using every stage, with defaults and edge parameters.
    fn brushes() -> Vec<Arc<Brush>> {
        const FILTERS: [&str; 2] = [
            "[[filters]]\ntype = \"resample\"\nspacing = 0.002\n[[filters]]\ntype = \"exponential\"\namount = 0.3\n\
             [[filters]]\ntype = \"one_euro\"\n[[filters]]\ntype = \"spline\"\nsteps = 4",
            "[[filters]]\ntype = \"resample\"\nspacing = 0.0\n[[filters]]\ntype = \"exponential\"\nsnap_end = false\n\
             [[filters]]\ntype = \"one_euro\"\nsnap_end = false\n[[filters]]\ntype = \"spline\"\nsteps = 1",
        ];
        const DYNAMICS: [&str; 2] = [
            "[[dynamics]]\ntype = \"width\"\nspeed_thinning = 0.5\n[[dynamics]]\ntype = \"taper\"",
            "[[dynamics]]\ntype = \"width\"\n[[dynamics]]\ntype = \"taper\"\nstart = 0.0\nend = 0.0\n\
             [[dynamics]]\ntype = \"taper\"\nstart = 2.0\nend = 3.0",
        ];
        const GEOMETRY: [&str; 5] = [
            "type = \"tube\"",
            "type = \"tube\"\ncaps = false\nsides = 3",
            "type = \"ribbon\"",
            "type = \"dots\"",
            "type = \"dots\"\nspacing = 0.001",
        ];
        let reg = StageRegistry::with_builtins();
        let mut out: Vec<Arc<Brush>> = BrushLibrary::new(reg.clone()).iter().cloned().collect();
        for (i, (f, d)) in [(FILTERS[0], DYNAMICS[0]), (FILTERS[1], DYNAMICS[1])].into_iter().enumerate() {
            for (j, g) in GEOMETRY.iter().enumerate() {
                let toml = format!(
                    "schema = 1\nid = \"test.{i}.{j}\"\nname = \"t\"\n{f}\n{d}\n[geometry]\n{g}\n[material]\ntype = \"solid\"\n"
                );
                out.push(Arc::new(Brush::compile(BrushDef::from_toml(&toml).unwrap(), &reg).unwrap()));
            }
        }
        out
    }

    /// The pipeline run directly through each stage's batch `apply`/`build`.
    fn reference(b: &Brush, input: &[InputPoint]) -> (MeshBuffers, Vec<StrokePoint>) {
        let mut pts: Vec<StrokePoint> = input.iter().map(StrokePoint::from).collect();
        for (f, p) in &b.filters {
            f.apply(&mut pts, p);
        }
        let mut dist = 0.0;
        for i in 0..pts.len() {
            if i > 0 {
                dist += pts[i].pos.distance(pts[i - 1].pos);
            }
            pts[i].dist = dist;
        }
        for (d, p) in &b.dynamics {
            d.apply(&mut pts, p);
        }
        let mut m = MeshBuffers::default();
        b.geometry.0.build(&pts, &b.geometry.1, &mut m);
        (m, pts)
    }

    fn is_prefix(a: &MeshBuffers, b: &MeshBuffers) -> bool {
        b.positions.starts_with(&a.positions)
            && b.normals.starts_with(&a.normals)
            && b.uvs.starts_with(&a.uvs)
            && b.tangents.starts_with(&a.tangents)
            && b.indices.starts_with(&a.indices)
    }

    #[test]
    fn pushing_in_pieces_matches_the_batch_stages() {
        for brush in brushes() {
            for n in [0, 1, 2, 3, 4, 5, 9, 60, 400] {
                let input = stroke(n);
                let (expected, line) = reference(&brush, &input);
                for (seed, max) in [(1, 1), (7, 4), (12345, 40)] {
                    let mut s = brush.stream();
                    let mut at = 0;
                    for k in splits(n, max, seed) {
                        let before = s.committed_mesh().clone();
                        s.push(&input[at..at + k]);
                        at += k;
                        let id = format!("{} n={n} seed={seed} at={at}", brush.id());
                        assert!(is_prefix(&before, s.committed_mesh()), "committed geometry changed: {id}");
                        if max > 1 || at % 25 == 0 {
                            assert_eq!(s.mesh(&[]), reference(&brush, &input[..at]).0, "{id}");
                        }
                    }
                    assert_eq!(s.mesh(&[]), expected, "{} n={n} seed={seed}", brush.id());
                    assert_eq!(s.centerline(&[]), line, "{} n={n} seed={seed}", brush.id());
                }
            }
        }
    }

    #[test]
    fn preview_matches_the_batch_stages_with_tail() {
        let input = stroke(300);
        for brush in brushes() {
            let mut s = brush.stream();
            s.push(&input[..280]);
            assert_eq!(s.mesh(&input[280..]), reference(&brush, &input).0, "{}", brush.id());
            // Previewing doesn't change the stream.
            assert_eq!(s.mesh(&[]), reference(&brush, &input[..280]).0, "{}", brush.id());
        }
    }

    /// The point of streaming: what's rebuilt per update stays small however long the stroke.
    #[test]
    fn provisional_tail_stays_small() {
        let lib = BrushLibrary::new(StageRegistry::with_builtins());
        let input = stroke(4800);
        for brush in lib.iter() {
            let mut s = brush.stream();
            let mut worst = 0;
            for chunk in input.chunks(2) {
                s.push(chunk);
                worst = worst.max(s.preview(&[]).positions.len());
            }
            let total = s.mesh(&[]).positions.len();
            assert!(worst <= 2000 && worst * 20 <= total, "{}: preview up to {worst} of {total} vertices", brush.id());
        }
    }
}
