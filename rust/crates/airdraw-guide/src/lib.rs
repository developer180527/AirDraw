//! 3D guides: the surface pen strokes are projected onto (docs/ARCHITECTURE.md §3f).
//!
//! A pen only gives a 2D position, so a stroke's depth comes from the active guide: each
//! sample's screen ray is intersected with it. A guide is a plane (the presets) or a
//! [`Sweep`]: a curve drawn on screen, extended along the camera's rays, so from where it was
//! drawn it looks like the line drawn, and from the side it's a surface to draw on.

use glam::{Vec2, Vec3};

/// A world-space ray from the camera through a screen position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    /// Unit length.
    pub dir: Vec3,
}

/// Where a ray meets a guide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub point: Vec3,
    /// Unit surface normal, facing the ray's origin (guides are two-sided).
    pub normal: Vec3,
    /// Distance along the ray.
    pub t: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Guide {
    /// Infinite plane through `origin`.
    Plane {
        origin: Vec3,
        normal: Vec3,
    },
    Sweep(Sweep),
}

/// How far from the world origin a stroke point may land, world units. A ray that grazes a
/// plane (pen near its horizon) meets it kilometres away; those hits count as misses, or
/// one stroke segment spans the whole view. Comfortably past the floor (20) and the camera's
/// zoom range (50).
pub const REACH: f32 = 100.0;

impl Guide {
    /// Nearest hit within [`REACH`] of the origin; farther hits are treated as misses.
    pub fn intersect_in_reach(&self, ray: Ray) -> Option<Hit> {
        self.intersect(ray).filter(|h| h.point.length() <= REACH)
    }

    /// Nearest hit in front of the ray's origin.
    pub fn intersect(&self, ray: Ray) -> Option<Hit> {
        match self {
            Guide::Plane { origin, normal } => {
                let n = normal.normalize_or(Vec3::Z);
                let denom = n.dot(ray.dir);
                if denom.abs() < 1e-6 {
                    return None;
                }
                let t = n.dot(*origin - ray.origin) / denom;
                (t >= 0.0).then(|| Hit { point: ray.origin + ray.dir * t, normal: facing(n, ray.dir), t })
            }
            Guide::Sweep(s) => s.intersect(ray),
        }
    }
}

/// A curve swept along per-point directions: point `i` spans
/// `profile[i] + dirs[i] * s` for `s` in `-near..=far`.
#[derive(Clone, Debug, PartialEq)]
pub struct Sweep {
    pub profile: Vec<Vec3>,
    /// Unit directions, one per profile point (the camera rays it was drawn along).
    pub dirs: Vec<Vec3>,
    pub near: f32,
    pub far: f32,
}

/// Settings for turning a drawn screen path into a [`Sweep`].
#[derive(Clone, Copy, Debug)]
pub struct SweepOptions {
    /// Distance between profile points, world units.
    pub spacing: f32,
    /// Upper bound on profile points (spacing grows for long curves).
    pub max_points: usize,
    /// Smoothing passes over the profile (3-point average, ends fixed).
    pub smoothing: usize,
    /// How far the surface reaches past and in front of the curve, world units.
    pub extent: f32,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self { spacing: 0.02, max_points: 400, smoothing: 3, extent: 3.0 }
    }
}

impl Sweep {
    /// The guide Feather-style "draw" makes: each screen ray is cut by the plane through
    /// `anchor` facing the camera (`forward`), giving the profile curve; the surface then
    /// runs along those same rays, so it is perpendicular to the view. Perspective rays fan
    /// out, so the field of view shapes the result. `None` for fewer than 2 usable points
    /// or a path that doesn't move.
    pub fn from_view(rays: &[Ray], forward: Vec3, anchor: Vec3, opts: SweepOptions) -> Option<Sweep> {
        let forward = forward.normalize_or(Vec3::NEG_Z);
        let mut pts = Vec::with_capacity(rays.len());
        let mut dirs = Vec::with_capacity(rays.len());
        let mut nearest = f32::INFINITY;
        for r in rays {
            let denom = forward.dot(r.dir);
            if denom < 1e-4 {
                continue;
            }
            let t = forward.dot(anchor - r.origin) / denom;
            if t <= 0.0 {
                continue;
            }
            nearest = nearest.min(t);
            pts.push(r.origin + r.dir * t);
            dirs.push(r.dir);
        }
        let (profile, dirs) = resample(&pts, &dirs, opts)?;
        // Don't reach back past the camera.
        let near = opts.extent.min(nearest * 0.9);
        Some(Sweep { profile, dirs, near, far: opts.extent })
    }

    /// The four corners of the quad between profile points `i` and `i + 1`, in winding order.
    fn quad(&self, i: usize) -> [Vec3; 4] {
        let (p0, p1, d0, d1) = (self.profile[i], self.profile[i + 1], self.dirs[i], self.dirs[i + 1]);
        [p0 - d0 * self.near, p0 + d0 * self.far, p1 + d1 * self.far, p1 - d1 * self.near]
    }

    pub fn intersect(&self, ray: Ray) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for i in 0..self.profile.len().saturating_sub(1) {
            let [a, b, c, d] = self.quad(i);
            for tri in [[a, b, c], [a, c, d]] {
                if let Some(h) = intersect_triangle(ray, tri)
                    && best.is_none_or(|b| h.t < b.t)
                {
                    best = Some(h);
                }
            }
        }
        best
    }

    /// Display mesh: `rows` bands across the sweep, UVs in world units (u along the curve,
    /// v along the rays, 0 at the curve) so a grid shader draws even section lines.
    pub fn mesh(&self, rows: usize) -> GuideMesh {
        let rows = rows.max(1);
        let mut m = GuideMesh::default();
        let mut u = 0.0;
        for (i, (&p, &d)) in self.profile.iter().zip(&self.dirs).enumerate() {
            if i > 0 {
                u += p.distance(self.profile[i - 1]);
            }
            let normal = self.point_normal(i);
            for r in 0..=rows {
                let s = -self.near + (self.near + self.far) * r as f32 / rows as f32;
                m.positions.push(p + d * s);
                m.normals.push(normal);
                m.uvs.push(Vec2::new(u, s));
            }
        }
        let w = rows as u32 + 1;
        for i in 0..self.profile.len().saturating_sub(1) as u32 {
            for r in 0..rows as u32 {
                let (a, b) = (i * w + r, (i + 1) * w + r);
                m.indices.extend_from_slice(&[a, a + 1, b + 1, a, b + 1, b]);
            }
        }
        m
    }

    /// The edge at the start of the curve (Feather marks it in orange): the first point's ray.
    pub fn start_line(&self) -> [Vec3; 2] {
        let (p, d) = (self.profile[0], self.dirs[0]);
        [p - d * self.near, p + d * self.far]
    }

    fn point_normal(&self, i: usize) -> Vec3 {
        let n = self.profile.len();
        let along = self.profile[(i + 1).min(n - 1)] - self.profile[i.saturating_sub(1)];
        along.cross(self.dirs[i]).normalize_or(Vec3::Z)
    }
}

/// Triangles for showing a guide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuideMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub indices: Vec<u32>,
}

/// Möller–Trumbore, two-sided.
fn intersect_triangle(ray: Ray, [a, b, c]: [Vec3; 3]) -> Option<Hit> {
    let (e1, e2) = (b - a, c - a);
    let p = ray.dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    if t <= 1e-6 {
        return None;
    }
    let normal = e1.cross(e2).normalize_or(Vec3::Z);
    Some(Hit { point: ray.origin + ray.dir * t, normal: facing(normal, ray.dir), t })
}

/// `n`, flipped if needed to face against `dir` (toward whoever looks along `dir`).
fn facing(n: Vec3, dir: Vec3) -> Vec3 {
    if n.dot(dir) > 0.0 { -n } else { n }
}

/// Evenly spaced, smoothed profile points with their (interpolated) directions.
fn resample(pts: &[Vec3], dirs: &[Vec3], opts: SweepOptions) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
    let length: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
    if pts.len() < 2 || length < 1e-5 {
        return None;
    }
    let spacing = opts.spacing.max(length / opts.max_points.max(2) as f32);
    let count = ((length / spacing).ceil() as usize).max(1);
    let (mut out_p, mut out_d) = (Vec::with_capacity(count + 1), Vec::with_capacity(count + 1));
    let (mut seg, mut seg_start) = (0, 0.0);
    for k in 0..=count {
        let target = length * k as f32 / count as f32;
        while seg + 1 < pts.len() - 1 && seg_start + pts[seg].distance(pts[seg + 1]) < target {
            seg_start += pts[seg].distance(pts[seg + 1]);
            seg += 1;
        }
        let seg_len = pts[seg].distance(pts[seg + 1]);
        let f = if seg_len > 0.0 { ((target - seg_start) / seg_len).clamp(0.0, 1.0) } else { 0.0 };
        out_p.push(pts[seg].lerp(pts[seg + 1], f));
        out_d.push(dirs[seg].lerp(dirs[seg + 1], f).normalize_or(dirs[seg]));
    }
    for _ in 0..opts.smoothing {
        let prev = out_p.clone();
        for i in 1..prev.len() - 1 {
            out_p[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
        }
    }
    Some((out_p, out_d))
}

#[cfg(test)]
mod tests {
    #[test]
    fn grazing_plane_hits_are_out_of_reach() {
        let plane = Guide::Plane { origin: Vec3::ZERO, normal: Vec3::Z };
        // Camera 5 in front of the plane, ray almost parallel to it: hit is ~50 km away.
        let ray = Ray { origin: Vec3::new(0.0, 0.0, 5.0), dir: Vec3::new(1.0, 0.0, -1e-4).normalize() };
        assert!(plane.intersect(ray).is_some());
        assert_eq!(plane.intersect_in_reach(ray), None);
        let straight = Ray { origin: Vec3::new(0.0, 0.0, 5.0), dir: Vec3::NEG_Z };
        assert_eq!(plane.intersect_in_reach(straight), plane.intersect(straight));
    }

    use super::*;

    /// A perspective camera at `eye` looking down -Z: the ray through normalized screen
    /// position (x, y) on an image plane at distance 1.
    fn ray(eye: Vec3, x: f32, y: f32) -> Ray {
        Ray { origin: eye, dir: Vec3::new(x, y, -1.0).normalize() }
    }

    fn horizontal_line_sweep() -> (Sweep, Vec3) {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let rays: Vec<Ray> = (0..=20).map(|i| ray(eye, -0.4 + i as f32 * 0.04, 0.0)).collect();
        let s = Sweep::from_view(&rays, Vec3::NEG_Z, Vec3::ZERO, SweepOptions::default()).unwrap();
        (s, eye)
    }

    #[test]
    fn plane_hits_and_misses() {
        let g = Guide::Plane { origin: Vec3::new(0.0, 0.0, -2.0), normal: Vec3::Z };
        let h = g.intersect(Ray { origin: Vec3::ZERO, dir: Vec3::NEG_Z }).unwrap();
        assert!((h.point - Vec3::new(0.0, 0.0, -2.0)).length() < 1e-5);
        assert_eq!(h.normal, Vec3::Z);
        // Behind the camera, and parallel: no hit.
        assert!(g.intersect(Ray { origin: Vec3::ZERO, dir: Vec3::Z }).is_none());
        assert!(g.intersect(Ray { origin: Vec3::ZERO, dir: Vec3::X }).is_none());
        // Two-sided: from behind, the normal faces the viewer.
        let back = g.intersect(Ray { origin: Vec3::new(0.0, 0.0, -5.0), dir: Vec3::Z }).unwrap();
        assert_eq!(back.normal, Vec3::NEG_Z);
    }

    #[test]
    fn drawn_sweep_profile_lies_on_the_anchor_plane() {
        let (s, _) = horizontal_line_sweep();
        assert!(s.profile.len() > 10);
        for p in &s.profile {
            assert!(p.z.abs() < 1e-4 && p.y.abs() < 1e-4, "{p}");
        }
        // Doesn't reach back to the camera (5 units away).
        assert!(s.near < 5.0 * 0.95 && s.far == 3.0);
    }

    #[test]
    fn from_the_drawing_view_the_sweep_is_seen_edge_on() {
        // Every ray along the drawn line hits the surface at the profile (or along it): the
        // guide looks like the line drawn.
        let (s, eye) = horizontal_line_sweep();
        let g = Guide::Sweep(s);
        let above = g.intersect(ray(eye, 0.0, 0.05));
        assert!(above.is_none(), "rays off the drawn line must miss");
    }

    #[test]
    fn from_the_side_the_sweep_is_a_surface_with_depth() {
        // A horizontal stroke seen from the front makes a horizontal sheet along the view
        // axis; looking down from above, rays hit it at different depths.
        let (s, _) = horizontal_line_sweep();
        let g = Guide::Sweep(s);
        let down = |x: f32, z: f32| Ray { origin: Vec3::new(x, 4.0, z), dir: Vec3::NEG_Y };
        let near_hit = g.intersect(down(0.0, 1.0)).expect("hit in front of the curve");
        let far_hit = g.intersect(down(0.0, -2.0)).expect("hit behind the curve");
        assert!((near_hit.point.z - 1.0).abs() < 1e-3 && (far_hit.point.z + 2.0).abs() < 1e-3);
        assert!(near_hit.point.y.abs() < 1e-3, "sheet is at the drawn height");
        // The normal faces the viewer above.
        assert!(near_hit.normal.y > 0.99, "{}", near_hit.normal);
        // Past the far extent: miss.
        assert!(g.intersect(down(0.0, -3.5)).is_none());
    }

    #[test]
    fn nearest_of_two_layers_wins() {
        // A curve that doubles back makes two sheets; a ray through both hits the closer one.
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let rays: Vec<Ray> = (0..=20).map(|i| ray(eye, -0.4 + i as f32 * 0.04, 0.0)).collect();
        let s = Sweep::from_view(&rays, Vec3::NEG_Z, Vec3::ZERO, SweepOptions::default()).unwrap();
        let mut lower = s.clone();
        for p in &mut lower.profile {
            p.y -= 1.0;
        }
        let r = Ray { origin: Vec3::new(0.0, 4.0, 0.5), dir: Vec3::NEG_Y };
        let top = Guide::Sweep(s).intersect(r).unwrap();
        let bottom = Guide::Sweep(lower).intersect(r).unwrap();
        assert!(top.t < bottom.t);
    }

    #[test]
    fn degenerate_paths_make_no_guide() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let opts = SweepOptions::default();
        assert!(Sweep::from_view(&[], Vec3::NEG_Z, Vec3::ZERO, opts).is_none());
        assert!(Sweep::from_view(&[ray(eye, 0.0, 0.0)], Vec3::NEG_Z, Vec3::ZERO, opts).is_none());
        let still = [ray(eye, 0.1, 0.1), ray(eye, 0.1, 0.1)];
        assert!(Sweep::from_view(&still, Vec3::NEG_Z, Vec3::ZERO, opts).is_none());
    }

    #[test]
    fn long_paths_are_capped() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let rays: Vec<Ray> = (0..=4000).map(|i| ray(eye, -2.0 + i as f32 * 0.001, (i as f32 * 0.01).sin())).collect();
        let s = Sweep::from_view(&rays, Vec3::NEG_Z, Vec3::ZERO, SweepOptions::default()).unwrap();
        assert!(s.profile.len() <= 401);
    }

    #[test]
    fn mesh_is_valid() {
        let (s, _) = horizontal_line_sweep();
        let m = s.mesh(6);
        assert_eq!(m.positions.len(), s.profile.len() * 7);
        assert_eq!(m.indices.len(), (s.profile.len() - 1) * 6 * 6);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        assert_eq!(m.uvs.len(), m.positions.len());
    }
}
