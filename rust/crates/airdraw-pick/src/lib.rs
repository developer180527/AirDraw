//! Stroke picking (docs/ARCHITECTURE.md §3i): which strokes, and which parts of them, are
//! under the eraser, a tap or a lasso. Everything is measured in screen pixels through the
//! camera, so the eraser works on what you see at any zoom.
//!
//! Strokes are tested as their raw input points with a half width (the brush's widest).
//! Filters move points by far less than a stroke's width, and raw points need no brush run.
//! A world box per stroke skips strokes whose projected box is nowhere near the query.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use airdraw_brush::InputPoint;
use airdraw_doc::{LayerId, Stroke, StrokeId};
use glam::{Affine3A, Mat4, Vec2, Vec3};

/// Nearer than this (metres, camera space) counts as behind the camera.
const NEAR: f32 = 1e-4;

/// How a world point lands on screen.
#[derive(Clone, Debug)]
pub struct Camera {
    view: Affine3A,
    proj: Mat4,
    size: Vec2,
}

#[derive(Clone, Copy, Debug)]
struct Projected {
    at: Vec2,
    /// Distance in front of the camera, metres.
    depth: f32,
    /// Screen pixels per metre at this depth.
    px_per_m: f32,
}

impl Camera {
    /// `view` maps world to camera space (the camera looks down −Z, as in Godot), `proj` is
    /// the camera's projection and `size` the viewport in pixels, y down.
    pub fn new(view: Affine3A, proj: Mat4, size: Vec2) -> Self {
        Self { view, proj, size }
    }

    /// Screen position (pixels, y down) and depth (metres) of `p`, or `None` behind the camera.
    pub fn project(&self, p: Vec3) -> Option<(Vec2, f32)> {
        self.project_full(p).map(|q| (q.at, q.depth))
    }

    fn project_full(&self, p: Vec3) -> Option<Projected> {
        let v = self.view.transform_point3(p);
        let clip = self.proj * v.extend(1.0);
        let depth = -v.z;
        if depth <= NEAR || clip.w <= 1e-6 {
            return None;
        }
        let ndc = Vec2::new(clip.x, clip.y) / clip.w;
        let at = Vec2::new((ndc.x + 1.0) * 0.5 * self.size.x, (1.0 - ndc.y) * 0.5 * self.size.y);
        Some(Projected { at, depth, px_per_m: self.proj.y_axis.y * self.size.y * 0.5 / clip.w })
    }

    /// Screen rectangle covering a world box, or `None` when part of it is behind the camera
    /// (then it may be anywhere).
    fn rect(&self, min: Vec3, max: Vec3) -> Option<(Vec2, Vec2)> {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            let p = self.project_full(c)?;
            lo = lo.min(p.at);
            hi = hi.max(p.at);
        }
        Some((lo, hi))
    }
}

struct Entry {
    id: StrokeId,
    layer: LayerId,
    points: Arc<[InputPoint]>,
    half_width: f32,
    /// World box of the points, grown by the half width.
    min: Vec3,
    max: Vec3,
}

impl Entry {
    /// Could the stroke reach the screen box `lo..hi` grown by `pad` pixels?
    fn near(&self, cam: &Camera, lo: Vec2, hi: Vec2, pad: f32) -> bool {
        match cam.rect(self.min, self.max) {
            Some((a, b)) => a.x <= hi.x + pad && a.y <= hi.y + pad && b.x >= lo.x - pad && b.y >= lo.y - pad,
            None => true,
        }
    }

    fn project(&self, cam: &Camera) -> Vec<Option<Projected>> {
        self.points.iter().map(|p| cam.project_full(p.pos)).collect()
    }
}

/// A stroke segment (or a lone point, `p == q`) that touches a query path.
#[derive(Clone, Copy, Debug)]
struct Contact {
    id: StrokeId,
    p: Projected,
    q: Projected,
    half_width: f32,
    /// Where it comes closest: pixels along the query path from its start, and its distance
    /// and depth there.
    along: f32,
    dist: f32,
    depth: f32,
}

impl Contact {
    /// Pixel distance and depth of the segment's closest point to screen point `x`, and
    /// whether it's within `radius` plus the stroke's half width.
    fn at(&self, x: Vec2, radius: f32) -> Option<(f32, f32)> {
        let (s, dist) = closest_on_segment(self.p.at, self.q.at, x);
        let px_per_m = self.p.px_per_m + (self.q.px_per_m - self.p.px_per_m) * s;
        (dist <= radius + self.half_width * px_per_m).then_some((dist, self.p.depth + (self.q.depth - self.p.depth) * s))
    }
}

/// Strokes that can be picked, kept in step with the document.
#[derive(Default)]
pub struct PickIndex {
    entries: Vec<Entry>,
    slot: HashMap<StrokeId, usize>,
    /// Hidden or locked layers: never picked.
    skipped: HashSet<LayerId>,
}

impl PickIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a stroke. `half_width` is its widest half width, metres.
    pub fn insert(&mut self, stroke: &Stroke, half_width: f32) {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in stroke.points.iter() {
            min = min.min(p.pos);
            max = max.max(p.pos);
        }
        let entry = Entry {
            id: stroke.id,
            layer: stroke.layer,
            points: stroke.points.clone(),
            half_width,
            min: min - Vec3::splat(half_width),
            max: max + Vec3::splat(half_width),
        };
        match self.slot.get(&stroke.id) {
            Some(&i) => self.entries[i] = entry,
            None => {
                self.slot.insert(stroke.id, self.entries.len());
                self.entries.push(entry);
            }
        }
    }

    pub fn remove(&mut self, id: StrokeId) {
        let Some(i) = self.slot.remove(&id) else { return };
        self.entries.swap_remove(i);
        if let Some(moved) = self.entries.get(i) {
            self.slot.insert(moved.id, i);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.slot.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, id: StrokeId) -> bool {
        self.slot.contains_key(&id)
    }

    /// Layers whose strokes are never picked (hidden or locked); replaces the previous set.
    pub fn set_skipped_layers(&mut self, layers: impl IntoIterator<Item = LayerId>) {
        self.skipped = layers.into_iter().collect();
    }

    fn candidates<'a>(&'a self, cam: &'a Camera, lo: Vec2, hi: Vec2, pad: f32) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries.iter().filter(move |e| !self.skipped.contains(&e.layer) && e.near(cam, lo, hi, pad))
    }

    /// Every place a stroke comes within `radius` pixels (plus its half width) of the path
    /// `a`→`b`. One contact per touching segment.
    fn contacts(&self, cam: &Camera, a: Vec2, b: Vec2, radius: f32) -> Vec<Contact> {
        let mut out = Vec::new();
        for e in self.candidates(cam, a.min(b), a.max(b), radius) {
            let pts = e.project(cam);
            let mut touch = |p: Projected, q: Projected, s: f32, t: f32, dist: f32| {
                let px_per_m = p.px_per_m + (q.px_per_m - p.px_per_m) * s;
                if dist <= radius + e.half_width * px_per_m {
                    let depth = p.depth + (q.depth - p.depth) * s;
                    out.push(Contact { id: e.id, p, q, half_width: e.half_width, along: t * a.distance(b), dist, depth });
                }
            };
            if let [Some(p)] = pts[..] {
                let (t, dist) = closest_on_segment(a, b, p.at);
                touch(p, p, 0.0, t, dist);
            }
            for w in pts.windows(2) {
                if let [Some(p), Some(q)] = *w {
                    let (s, t, dist) = closest_segments(p.at, q.at, a, b);
                    touch(p, q, s, t, dist);
                }
            }
        }
        out
    }

    /// Strokes the eraser touches moving from `a` to `b` (pixels) with `radius`. With
    /// `front_only`, a stroke counts only if, at some eraser position along the way, it's
    /// about as near as the nearest stroke under the eraser there. Sorted by id.
    pub fn under_capsule(&self, cam: &Camera, a: Vec2, b: Vec2, radius: f32, front_only: bool) -> Vec<StrokeId> {
        let contacts = self.contacts(cam, a, b, radius);
        let mut ids: Vec<StrokeId> = if front_only { front(&contacts, a, b, radius) } else { contacts.iter().map(|c| c.id).collect() };
        ids.sort();
        ids.dedup();
        ids
    }

    /// The stroke at a tap: the frontmost within `tolerance` pixels; among strokes about as
    /// near, the closest on screen.
    pub fn nearest(&self, cam: &Camera, p: Vec2, tolerance: f32) -> Option<StrokeId> {
        let contacts = self.contacts(cam, p, p, tolerance);
        let front = contacts.iter().min_by(|x, y| x.depth.total_cmp(&y.depth))?;
        contacts
            .iter()
            .filter(|c| c.depth <= front.depth + band(front.depth, front.half_width, c.half_width))
            .min_by(|x, y| x.dist.total_cmp(&y.dist).then(x.depth.total_cmp(&y.depth)))
            .map(|c| c.id)
    }

    /// Strokes with at least half their points inside the lasso (pixels), at any depth.
    /// Sorted by id.
    pub fn in_lasso(&self, cam: &Camera, polygon: &[Vec2]) -> Vec<StrokeId> {
        if polygon.len() < 3 {
            return Vec::new();
        }
        let lo = polygon.iter().fold(Vec2::splat(f32::MAX), |m, p| m.min(*p));
        let hi = polygon.iter().fold(Vec2::splat(f32::MIN), |m, p| m.max(*p));
        let mut ids: Vec<StrokeId> = self
            .candidates(cam, lo, hi, 0.0)
            .filter(|e| {
                let inside = e.points.iter().filter(|p| cam.project(p.pos).is_some_and(|(at, _)| inside(polygon, at))).count();
                !e.points.is_empty() && inside * 2 >= e.points.len()
            })
            .map(|e| e.id)
            .collect();
        ids.sort();
        ids
    }
}

/// How far behind the nearest stroke (at `depth`, half width `hw`) another may be and still
/// count as in front: about their widths, plus 1% of the distance for depth imprecision.
/// Metres.
fn band(depth: f32, hw: f32, other_hw: f32) -> f32 {
    2.0 * (hw + other_hw) + 0.01 * depth
}

/// Strokes in front at some eraser position: positions every quarter radius along `a`→`b`,
/// each looking at the contacts under the eraser there. Every contact also counts at the
/// position nearest where it touches, so one between positions is never missed.
fn front(contacts: &[Contact], a: Vec2, b: Vec2, radius: f32) -> Vec<StrokeId> {
    let len = a.distance(b);
    let steps = ((len / (radius * 0.25).max(0.5)).ceil() as usize).min(4096);
    let mut under: Vec<Vec<(StrokeId, f32, f32)>> = vec![Vec::new(); steps + 1];
    let dir = if len > 0.0 { (b - a) / len } else { Vec2::ZERO };
    let step = |along: f32| if len > 0.0 { along / len * steps as f32 } else { 0.0 };
    for c in contacts {
        under[(step(c.along).round() as usize).min(steps)].push((c.id, c.depth, c.half_width));
        // Only positions the segment can reach.
        let pad = radius + c.half_width * c.p.px_per_m.max(c.q.px_per_m);
        let (s0, s1) = ((c.p.at - a).dot(dir), (c.q.at - a).dot(dir));
        let from = step(s0.min(s1) - pad).floor().max(0.0) as usize;
        let to = (step(s0.max(s1) + pad).ceil().max(0.0) as usize).min(steps);
        for (k, list) in under.iter_mut().enumerate().take(to + 1).skip(from) {
            let x = if steps == 0 { a } else { a.lerp(b, k as f32 / steps as f32) };
            if let Some((_, depth)) = c.at(x, radius) {
                list.push((c.id, depth, c.half_width));
            }
        }
    }
    let mut out = Vec::new();
    for list in under {
        let Some(&(_, depth, hw)) = list.iter().min_by(|x, y| x.1.total_cmp(&y.1)) else { continue };
        out.extend(list.iter().filter(|c| c.1 <= depth + band(depth, hw, c.2)).map(|c| c.0));
    }
    out
}

/// Cuts a stroke where the eraser passed: `capsules` are eraser moves (pixels) of `radius`.
/// Returns `None` if the stroke isn't touched, otherwise its kept pieces (maybe none), each
/// ending exactly at the eraser's edge with interpolated points. Pieces with fewer than two
/// points or shorter than `min_len` metres are dropped.
pub fn cut(
    cam: &Camera,
    points: &[InputPoint],
    half_width: f32,
    capsules: &[(Vec2, Vec2)],
    radius: f32,
    min_len: f32,
) -> Option<Vec<Vec<InputPoint>>> {
    let pts: Vec<Option<Projected>> = points.iter().map(|p| cam.project_full(p.pos)).collect();
    let reach = |p: &Projected, q: &Projected| radius + half_width * 0.5 * (p.px_per_m + q.px_per_m);
    if let [Some(p)] = pts[..] {
        let hit = capsules.iter().any(|&(a, b)| closest_on_segment(a, b, p.at).1 <= reach(&p, &p));
        return hit.then(Vec::new);
    }
    // Erased stretches in stroke parameter: point i is at i, halfway to the next at i + 0.5.
    let mut erased: Vec<(f32, f32)> = Vec::new();
    for (i, w) in pts.windows(2).enumerate() {
        let [Some(p), Some(q)] = *w else { continue };
        for &(a, b) in capsules {
            if let Some((t0, t1)) = segment_in_capsule(p.at, q.at, a, b, reach(&p, &q))
                && t1 - t0 > 1e-6
            {
                erased.push((i as f32 + t0, i as f32 + t1));
            }
        }
    }
    if erased.is_empty() {
        return None;
    }
    erased.sort_by(|x, y| x.0.total_cmp(&y.0));
    let end = (points.len() - 1) as f32;
    let mut kept = Vec::new();
    let mut from = 0.0f32;
    for (lo, hi) in erased {
        if lo > from {
            kept.push((from, lo));
        }
        from = from.max(hi);
    }
    if from < end {
        kept.push((from, end));
    }
    let pieces = kept
        .into_iter()
        .map(|(s0, s1)| {
            let mut piece = vec![point_at(points, s0)];
            piece.extend((s0.floor() as usize + 1..).take_while(|&k| (k as f32) < s1).map(|k| points[k]));
            piece.push(point_at(points, s1));
            piece
        })
        .filter(|p| p.len() >= 2 && p.windows(2).map(|w| w[0].pos.distance(w[1].pos)).sum::<f32>() >= min_len)
        .collect();
    Some(pieces)
}

/// The stroke at parameter `s` (point `i` at `i`), interpolated between points.
fn point_at(points: &[InputPoint], s: f32) -> InputPoint {
    let i = (s.floor() as usize).min(points.len() - 1);
    let t = s - i as f32;
    if t <= 0.0 || i + 1 >= points.len() {
        return points[i];
    }
    let (a, b) = (&points[i], &points[i + 1]);
    InputPoint {
        pos: a.pos.lerp(b.pos, t),
        normal: a.normal.lerp(b.normal, t).try_normalize().unwrap_or(a.normal),
        pressure: a.pressure + (b.pressure - a.pressure) * t,
        tilt: match (a.tilt, b.tilt) {
            (Some(x), Some(y)) => Some(x + (y - x) * t),
            (x, y) => {
                if t < 0.5 {
                    x
                } else {
                    y
                }
            }
        },
        t: a.t + (b.t - a.t) * t as f64,
    }
}

/// Parameter on `a`→`b` closest to `p`, and the distance.
fn closest_on_segment(a: Vec2, b: Vec2, p: Vec2) -> (f32, f32) {
    let d = b - a;
    let len2 = d.length_squared();
    let t = if len2 > 0.0 { ((p - a).dot(d) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (t, p.distance(a + d * t))
}

/// Closest points of segments `p0`→`p1` and `q0`→`q1`: parameters on each, and the distance.
fn closest_segments(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> (f32, f32, f32) {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    const EPS: f32 = 1e-12;
    let (s, t) = if a <= EPS && e <= EPS {
        (0.0, 0.0)
    } else if a <= EPS {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom > EPS { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    (s, t, (p0 + d1 * s).distance(q0 + d2 * t))
}

/// The part of segment `p0`→`p1` (as parameters 0..1) inside the capsule around `a`→`b` with
/// radius `r`. A capsule is convex, so that part is one interval: the union of what's inside
/// the two end circles and the rectangle between them.
fn segment_in_capsule(p0: Vec2, p1: Vec2, a: Vec2, b: Vec2, r: f32) -> Option<(f32, f32)> {
    let d = p1 - p0;
    let mut parts = vec![in_circle(p0, d, a, r), in_circle(p0, d, b, r)];
    let len = a.distance(b);
    if len > 1e-6 {
        let u = (b - a) / len;
        let v = u.perp();
        let (x0, dx, y0, dy) = ((p0 - a).dot(u), d.dot(u), (p0 - a).dot(v), d.dot(v));
        parts.push(clip_slab(x0, dx, 0.0, len, (0.0, 1.0)).and_then(|t| clip_slab(y0, dy, -r, r, t)));
    }
    let (lo, hi) = parts.into_iter().flatten().fold((f32::MAX, f32::MIN), |(lo, hi), (x, y)| (lo.min(x), hi.max(y)));
    let (lo, hi) = (lo.max(0.0), hi.min(1.0));
    (lo <= hi).then_some((lo, hi))
}

/// Parameters of `p0 + t·d` inside the circle at `c` with radius `r`.
fn in_circle(p0: Vec2, d: Vec2, c: Vec2, r: f32) -> Option<(f32, f32)> {
    let m = p0 - c;
    let (a, b, k) = (d.length_squared(), m.dot(d), m.length_squared() - r * r);
    if a <= 1e-12 {
        return (k <= 0.0).then_some((0.0, 1.0));
    }
    let disc = b * b - a * k;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    Some(((-b - root) / a, (-b + root) / a))
}

/// Narrows `t` to where `x0 + t·dx` lies in `lo..=hi`.
fn clip_slab(x0: f32, dx: f32, lo: f32, hi: f32, (mut t0, mut t1): (f32, f32)) -> Option<(f32, f32)> {
    if dx.abs() <= 1e-12 {
        return (x0 >= lo && x0 <= hi).then_some((t0, t1));
    }
    let (mut a, mut b) = ((lo - x0) / dx, (hi - x0) / dx);
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    t0 = t0.max(a);
    t1 = t1.min(b);
    (t0 <= t1).then_some((t0, t1))
}

/// Even-odd point-in-polygon.
fn inside(polygon: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests;
