use glam::Vec3;

use crate::params::{ParamSpec, Params, ParamsExt};
use crate::stage::{Geometry, MeshBuffers, Stage, StrokePoint};
use crate::stream::MeshStream;

/// Tangent at each point, from its neighbours.
fn tangents(pts: &[StrokePoint]) -> Vec<Vec3> {
    let n = pts.len();
    (0..n).map(|i| tangent(pts[i.saturating_sub(1)].pos, pts[(i + 1).min(n - 1)].pos)).collect()
}

/// Direction from the previous point `a` to the next point `b`.
fn tangent(a: Vec3, b: Vec3) -> Vec3 {
    (b - a).try_normalize().unwrap_or(Vec3::Z)
}

/// Point `i` of a stroke split into committed points and a provisional tail.
fn point_at(committed: &[StrokePoint], tail: &[StrokePoint], i: usize) -> StrokePoint {
    if i < committed.len() { committed[i] } else { tail[i - committed.len()] }
}

/// Appends finish output (whose indices already count the committed vertices) to the
/// committed mesh: a stream's commit + finish as one build.
fn append(m: &mut MeshBuffers, rest: MeshBuffers) {
    m.positions.extend(rest.positions);
    m.normals.extend(rest.normals);
    m.indices.extend(rest.indices);
}

/// Round tube around the stroke, radius = width / 2, with flat end caps.
pub struct Tube;

impl Stage for Tube {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::int("sides", 8, 3, 32, "Segments around the tube"),
            ParamSpec::bool("caps", true, "Close the ends"),
        ]
    }
}

impl Geometry for Tube {
    /// Built through [`TubeStream`], so drawn and rebuilt strokes match: each ring is followed
    /// by its quads to the previous ring, and the start cap's center vertex follows ring 0.
    fn build(&self, pts: &[StrokePoint], p: &Params, m: &mut MeshBuffers) {
        let mut s = TubeStream::new(p);
        s.commit(pts, m);
        let mut rest = MeshBuffers::default();
        s.finish(&[], &mut rest);
        append(m, rest);
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn MeshStream>> {
        Some(Box::new(TubeStream::new(p)))
    }
}

/// [`Tube`] streamed: ring `i` is final once point `i + 1` exists (its tangent needs it); the
/// last ring and the end cap are provisional.
struct TubeStream {
    sides: usize,
    caps: bool,
    /// Cosine and sine of each vertex angle around a ring.
    angles: Vec<(f32, f32)>,
    /// Committed points from `next - 1` on (all a ring still needs), oldest first.
    window: Vec<StrokePoint>,
    /// Stroke index of `window[0]`.
    start: usize,
    /// Next ring to emit.
    next: usize,
    frame: TubeFrame,
    /// Vertices in the committed mesh; finish output indices start here.
    committed: i32,
}

/// What a ring needs from the rings before it.
#[derive(Clone, Copy)]
struct TubeFrame {
    /// Parallel-transported normal, so the tube doesn't twist.
    normal: Vec3,
    /// Index of the previous ring's first vertex.
    last_ring: i32,
}

impl TubeStream {
    fn new(p: &Params) -> Self {
        let sides = p.int("sides").max(3) as usize;
        let angles = (0..sides)
            .map(|j| {
                let a = j as f32 / sides as f32 * std::f32::consts::TAU;
                (a.cos(), a.sin())
            })
            .collect();
        Self {
            sides,
            caps: p.flag("caps"),
            angles,
            window: Vec::new(),
            start: 0,
            next: 0,
            frame: TubeFrame { normal: Vec3::ZERO, last_ring: 0 },
            committed: 0,
        }
    }

    /// Appends ring `i` around `pt` with tangent `t`, its quads to ring `i - 1`, and after
    /// ring 0 the start cap. `offset` is the index of `out`'s first vertex.
    fn ring(&self, frame: &mut TubeFrame, i: usize, pt: &StrokePoint, t: Vec3, out: &mut MeshBuffers, offset: i32) {
        if i == 0 {
            frame.normal = t.any_orthonormal_vector();
        }
        frame.normal = (frame.normal - t * frame.normal.dot(t)).try_normalize().unwrap_or_else(|| t.any_orthonormal_vector());
        let binormal = t.cross(frame.normal);
        let r = pt.width * 0.5;
        let base = offset + out.positions.len() as i32;
        for &(cos, sin) in &self.angles {
            let dir = frame.normal * cos + binormal * sin;
            out.positions.push(pt.pos + dir * r);
            out.normals.push(dir);
        }
        let s = self.sides as i32;
        if i > 0 {
            for j in 0..s {
                let (a, b) = (frame.last_ring + j, frame.last_ring + (j + 1) % s);
                let (c, d) = (base + j, base + (j + 1) % s);
                out.indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        } else if self.caps {
            self.cap(base, pt.pos, -t, false, out, offset);
        }
        frame.last_ring = base;
    }

    /// A fan closing the ring that starts at index `ring`.
    fn cap(&self, ring: i32, center: Vec3, dir: Vec3, end: bool, out: &mut MeshBuffers, offset: i32) {
        let c = offset + out.positions.len() as i32;
        out.positions.push(center);
        out.normals.push(dir);
        let s = self.sides as i32;
        for j in 0..s {
            let (a, b) = (ring + j, ring + (j + 1) % s);
            out.indices.extend_from_slice(&if end { [c, a, b] } else { [c, b, a] });
        }
    }
}

impl MeshStream for TubeStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut MeshBuffers) {
        let rings = input.len() * self.sides;
        out.positions.reserve(rings);
        out.normals.reserve(rings);
        out.indices.reserve(rings * 6);
        // Indices below are relative to `window[0]`; the window is followed by `input`.
        let n = self.window.len() + input.len();
        let at = |i: usize| point_at(&self.window, input, i);
        let (mut frame, mut next) = (self.frame, self.next);
        while next - self.start + 1 < n {
            let i = next - self.start;
            let t = tangent(at(i.saturating_sub(1)).pos, at(i + 1).pos);
            self.ring(&mut frame, next, &at(i), t, out, 0);
            next += 1;
        }
        let keep = next.saturating_sub(1);
        let window: Vec<StrokePoint> = (keep - self.start..n).map(at).collect();
        (self.window, self.start, self.next, self.frame) = (window, keep, next, frame);
        self.committed = out.positions.len() as i32;
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut MeshBuffers) {
        let n = self.window.len() + tail.len();
        if self.start + n < 2 {
            return;
        }
        let at = |i: usize| point_at(&self.window, tail, i);
        let mut frame = self.frame;
        let mut t = Vec3::Z;
        for i in self.next - self.start..n {
            t = tangent(at(i.saturating_sub(1)).pos, at((i + 1).min(n - 1)).pos);
            self.ring(&mut frame, self.start + i, &at(i), t, out, self.committed);
        }
        if self.caps {
            self.cap(frame.last_ring, at(n - 1).pos, t, true, out, self.committed);
        }
    }
}

/// Flat strip lying on the drawing surface, width across the stroke.
pub struct Ribbon;

impl Stage for Ribbon {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![ParamSpec::float("lift", 0.0005, 0.0, 0.05, "Offset along the surface normal to avoid z-fighting")]
    }
}

impl Geometry for Ribbon {
    fn build(&self, pts: &[StrokePoint], p: &Params, m: &mut MeshBuffers) {
        if pts.len() < 2 {
            return;
        }
        let lift = p.float("lift");
        let base = m.positions.len() as i32;
        for (pt, t) in pts.iter().zip(tangents(pts)) {
            let normal = pt.normal.try_normalize().unwrap_or(Vec3::Y);
            let side = t.cross(normal).try_normalize().unwrap_or_else(|| t.any_orthonormal_vector());
            let center = pt.pos + normal * lift;
            let half = side * pt.width * 0.5;
            m.positions.extend_from_slice(&[center - half, center + half]);
            m.normals.extend_from_slice(&[normal, normal]);
        }
        for i in 0..pts.len() as i32 - 1 {
            let a = base + i * 2;
            m.indices.extend_from_slice(&[a, a + 1, a + 2, a + 1, a + 3, a + 2]);
        }
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn MeshStream>> {
        Some(Box::new(RibbonStream { lift: p.float("lift"), window: Vec::new(), start: 0, next: 0, last_pair: 0, committed: 0 }))
    }
}

/// [`Ribbon`] streamed: the vertex pair at point `i` is final once point `i + 1` exists.
struct RibbonStream {
    lift: f32,
    /// Committed points from `next - 1` on, oldest first.
    window: Vec<StrokePoint>,
    /// Stroke index of `window[0]`.
    start: usize,
    /// Next point to emit a pair for.
    next: usize,
    /// Index of the previous pair's first vertex.
    last_pair: i32,
    /// Vertices in the committed mesh; finish output indices start here.
    committed: i32,
}

impl RibbonStream {
    fn pair(&self, last_pair: &mut i32, i: usize, pt: &StrokePoint, t: Vec3, out: &mut MeshBuffers, offset: i32) {
        let normal = pt.normal.try_normalize().unwrap_or(Vec3::Y);
        let side = t.cross(normal).try_normalize().unwrap_or_else(|| t.any_orthonormal_vector());
        let center = pt.pos + normal * self.lift;
        let half = side * pt.width * 0.5;
        let base = offset + out.positions.len() as i32;
        out.positions.extend_from_slice(&[center - half, center + half]);
        out.normals.extend_from_slice(&[normal, normal]);
        if i > 0 {
            let a = *last_pair;
            out.indices.extend_from_slice(&[a, a + 1, base, a + 1, base + 1, base]);
        }
        *last_pair = base;
    }
}

impl MeshStream for RibbonStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut MeshBuffers) {
        out.positions.reserve(input.len() * 2);
        out.normals.reserve(input.len() * 2);
        out.indices.reserve(input.len() * 6);
        // Indices below are relative to `window[0]`; the window is followed by `input`.
        let n = self.window.len() + input.len();
        let at = |i: usize| point_at(&self.window, input, i);
        let (mut last_pair, mut next) = (self.last_pair, self.next);
        while next - self.start + 1 < n {
            let i = next - self.start;
            let t = tangent(at(i.saturating_sub(1)).pos, at(i + 1).pos);
            self.pair(&mut last_pair, next, &at(i), t, out, 0);
            next += 1;
        }
        let keep = next.saturating_sub(1);
        let window: Vec<StrokePoint> = (keep - self.start..n).map(at).collect();
        (self.window, self.start, self.next, self.last_pair) = (window, keep, next, last_pair);
        self.committed = out.positions.len() as i32;
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut MeshBuffers) {
        let n = self.window.len() + tail.len();
        if self.start + n < 2 {
            return;
        }
        let at = |i: usize| point_at(&self.window, tail, i);
        let mut last_pair = self.last_pair;
        for i in self.next - self.start..n {
            let t = tangent(at(i.saturating_sub(1)).pos, at((i + 1).min(n - 1)).pos);
            self.pair(&mut last_pair, self.start + i, &at(i), t, out, self.committed);
        }
    }
}

/// Small spheres placed every `spacing` along the stroke, diameter = width.
pub struct Dots;

impl Stage for Dots {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::float("spacing", 0.03, 0.001, 1.0, "Distance between dot centers, world units"),
            ParamSpec::int("detail", 6, 3, 16, "Segments around each dot"),
        ]
    }
}

impl Geometry for Dots {
    fn build(&self, pts: &[StrokePoint], p: &Params, m: &mut MeshBuffers) {
        let (spacing, detail) = (p.float("spacing"), p.int("detail").max(3) as usize);
        let Some(total) = pts.last().map(|l| l.dist) else { return };
        let mut seg = 0;
        for k in 0..dot_count(total, spacing) {
            let d = k as f32 * spacing;
            while seg + 1 < pts.len() && pts[seg + 1].dist < d {
                seg += 1;
            }
            dot(m, &pts[seg], &pts[(seg + 1).min(pts.len() - 1)], d, detail, 0);
        }
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn MeshStream>> {
        Some(Box::new(DotsStream {
            spacing: p.float("spacing"),
            detail: p.int("detail").max(3) as usize,
            window: Vec::new(),
            start: 0,
            next: 0,
            seg: 0,
            committed: 0,
        }))
    }
}

/// Number of dots on a stroke of length `total`.
fn dot_count(total: f32, spacing: f32) -> usize {
    // Count first, then place by multiplication: repeated addition drifts and drops the last dot.
    (total / spacing + 1e-4).floor() as usize + 1
}

/// The dot at distance `d`, between points `a` and `b`.
fn dot(m: &mut MeshBuffers, a: &StrokePoint, b: &StrokePoint, d: f32, detail: usize, offset: i32) {
    let t = if b.dist > a.dist { (d - a.dist) / (b.dist - a.dist) } else { 0.0 };
    let center = a.pos.lerp(b.pos, t);
    let radius = (a.width + (b.width - a.width) * t) * 0.5;
    sphere(m, center, radius, detail, offset);
}

/// [`Dots`] streamed: dot `k` is final once the committed length already counts it and a
/// committed point lies at or beyond it (so its segment can't change).
struct DotsStream {
    spacing: f32,
    detail: usize,
    /// Committed points from `seg` on, oldest first.
    window: Vec<StrokePoint>,
    /// Stroke index of `window[0]`.
    start: usize,
    /// Next dot to emit.
    next: usize,
    /// Segment the next dot starts searching from.
    seg: usize,
    /// Vertices in the committed mesh; finish output indices start here.
    committed: i32,
}

impl MeshStream for DotsStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut MeshBuffers) {
        // Indices below are relative to `window[0]`; the window is followed by `input`.
        let n = self.window.len() + input.len();
        let at = |i: usize| point_at(&self.window, input, i);
        let mut seg = self.seg - self.start;
        if n > 0 {
            // The count only grows with length, so dots below it stay.
            let count = dot_count(at(n - 1).dist, self.spacing);
            while self.next < count {
                let d = self.next as f32 * self.spacing;
                while seg + 1 < n && at(seg + 1).dist < d {
                    seg += 1;
                }
                if seg + 1 >= n {
                    break; // the dot's end point isn't committed yet
                }
                dot(out, &at(seg), &at(seg + 1), d, self.detail, 0);
                self.next += 1;
            }
        }
        let window: Vec<StrokePoint> = (seg..n).map(at).collect();
        (self.window, self.start, self.seg) = (window, self.start + seg, self.start + seg);
        self.committed = out.positions.len() as i32;
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut MeshBuffers) {
        let n = self.window.len() + tail.len();
        if n == 0 {
            return;
        }
        let at = |i: usize| point_at(&self.window, tail, i);
        let total = at(n - 1).dist;
        let mut seg = self.seg - self.start;
        for k in self.next..dot_count(total, self.spacing) {
            let d = k as f32 * self.spacing;
            while seg + 1 < n && at(seg + 1).dist < d {
                seg += 1;
            }
            dot(out, &at(seg), &at((seg + 1).min(n - 1)), d, self.detail, self.committed);
        }
    }
}

fn sphere(m: &mut MeshBuffers, center: Vec3, r: f32, segments: usize, offset: i32) {
    let rings = (segments / 2).max(2);
    let base = offset + m.positions.len() as i32;
    for i in 0..=rings {
        let phi = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..segments {
            let theta = std::f32::consts::TAU * j as f32 / segments as f32;
            let dir = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
            m.positions.push(center + dir * r);
            m.normals.push(dir);
        }
    }
    let s = segments as i32;
    for i in 0..rings as i32 {
        for j in 0..s {
            let a = base + i * s + j;
            let b = base + i * s + (j + 1) % s;
            m.indices.extend_from_slice(&[a, b, a + s, b, b + s, a + s]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::InputPoint;

    fn line(n: usize) -> Vec<StrokePoint> {
        (0..n)
            .map(|i| {
                let mut p = StrokePoint::from(&InputPoint {
                    pos: Vec3::new(i as f32 * 0.1, 0.0, 0.0),
                    normal: Vec3::Y,
                    pressure: 1.0,
                    tilt: None,
                    t: 0.0,
                });
                p.dist = i as f32 * 0.1;
                p
            })
            .collect()
    }

    fn defaults(spec: Vec<ParamSpec>) -> Params {
        spec.into_iter().map(|s| (s.name.to_string(), s.default)).collect()
    }

    /// Meshes are counter-clockwise with outward normals (glTF/USD/FBX convention): each
    /// triangle's geometric normal must agree with its vertex normals.
    fn assert_ccw_outward(m: &MeshBuffers) {
        for tri in m.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.positions[i as usize]);
            let face = (b - a).cross(c - a);
            if face.length() < 1e-9 {
                continue;
            }
            let n = tri.iter().map(|&i| m.normals[i as usize]).sum::<Vec3>();
            assert!(face.dot(n) > 0.0, "triangle {tri:?} winds against its normals");
        }
    }

    #[test]
    fn all_geometry_is_ccw_outward() {
        let line: Vec<StrokePoint> = (0..6)
            .map(|i| {
                let mut p = line(1)[0];
                p.pos = Vec3::new(i as f32 * 0.1, (i as f32 * 0.7).sin() * 0.05, 0.0);
                p.dist = i as f32 * 0.1;
                p
            })
            .collect();
        for g in [&Tube as &dyn Geometry, &Ribbon, &Dots] {
            let mut m = MeshBuffers::default();
            g.build(&line, &defaults(g.schema()), &mut m);
            assert_ccw_outward(&m);
        }
    }

    fn valid(m: &MeshBuffers) -> bool {
        !m.is_empty() && m.positions.len() == m.normals.len() && m.indices.iter().all(|&i| (i as usize) < m.positions.len())
    }

    #[test]
    fn tube_counts() {
        let mut m = MeshBuffers::default();
        Tube.build(&line(3), &defaults(Tube.schema()), &mut m);
        assert_eq!(m.positions.len(), 3 * 8 + 2);
        assert_eq!(m.indices.len(), 2 * 8 * 6 + 2 * 8 * 3);
        assert!(valid(&m));
    }

    #[test]
    fn ribbon_lies_on_surface() {
        let mut m = MeshBuffers::default();
        Ribbon.build(&line(5), &defaults(Ribbon.schema()), &mut m);
        assert!(valid(&m));
        // Surface normal is +Y, so all vertices sit at the lift height.
        assert!(m.positions.iter().all(|p| (p.y - 0.0005).abs() < 1e-6));
    }

    #[test]
    fn dots_follow_spacing() {
        let mut m = MeshBuffers::default();
        let mut params = defaults(Dots.schema());
        params.insert("spacing".into(), crate::params::ParamValue::Float(0.1));
        Dots.build(&line(11), &params, &mut m);
        assert!(valid(&m));
        let per_dot = (6 / 2 + 1) * 6;
        assert_eq!(m.positions.len() / per_dot, 11);
    }
}
