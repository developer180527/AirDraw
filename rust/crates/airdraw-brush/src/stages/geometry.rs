use glam::Vec3;

use crate::params::{ParamSpec, Params, ParamsExt};
use crate::stage::{Geometry, MeshBuffers, Stage, StrokePoint};

/// Tangent at each point, from its neighbours.
fn tangents(pts: &[StrokePoint]) -> Vec<Vec3> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let a = pts[i.saturating_sub(1)].pos;
            let b = pts[(i + 1).min(n - 1)].pos;
            (b - a).try_normalize().unwrap_or(Vec3::Z)
        })
        .collect()
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
    fn build(&self, pts: &[StrokePoint], p: &Params, m: &mut MeshBuffers) {
        let n = pts.len();
        let sides = p.int("sides").max(3) as usize;
        if n < 2 {
            return;
        }
        let tangents = tangents(pts);
        // Parallel-transported frames so the tube doesn't twist.
        let mut normal = tangents[0].any_orthonormal_vector();
        let base = m.positions.len() as i32;
        for (pt, t) in pts.iter().zip(&tangents) {
            normal = (normal - *t * normal.dot(*t)).try_normalize().unwrap_or_else(|| t.any_orthonormal_vector());
            let binormal = t.cross(normal);
            let r = pt.width * 0.5;
            for j in 0..sides {
                let a = j as f32 / sides as f32 * std::f32::consts::TAU;
                let dir = normal * a.cos() + binormal * a.sin();
                m.positions.push(pt.pos + dir * r);
                m.normals.push(dir);
            }
        }
        let s = sides as i32;
        for i in 0..n as i32 - 1 {
            for j in 0..s {
                let a = base + i * s + j;
                let b = base + i * s + (j + 1) % s;
                m.indices.extend_from_slice(&[a, b, a + s, b, b + s, a + s]);
            }
        }
        if p.flag("caps") {
            for (ring, dir) in [(0usize, -tangents[0]), (n - 1, tangents[n - 1])] {
                let center = m.positions.len() as i32;
                m.positions.push(pts[ring].pos);
                m.normals.push(dir);
                let first = base + (ring * sides) as i32;
                for j in 0..s {
                    let (a, b) = (first + j, first + (j + 1) % s);
                    let tri = if ring == 0 { [center, b, a] } else { [center, a, b] };
                    m.indices.extend_from_slice(&tri);
                }
            }
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
        // Count first, then place by multiplication: repeated addition drifts and drops the last dot.
        let count = (total / spacing + 1e-4).floor() as usize + 1;
        let mut seg = 0;
        for k in 0..count {
            let d = k as f32 * spacing;
            while seg + 1 < pts.len() && pts[seg + 1].dist < d {
                seg += 1;
            }
            let a = pts[seg];
            let b = pts[(seg + 1).min(pts.len() - 1)];
            let t = if b.dist > a.dist { (d - a.dist) / (b.dist - a.dist) } else { 0.0 };
            let center = a.pos.lerp(b.pos, t);
            let radius = (a.width + (b.width - a.width) * t) * 0.5;
            sphere(m, center, radius, detail);
        }
    }
}

fn sphere(m: &mut MeshBuffers, center: Vec3, r: f32, segments: usize) {
    let rings = (segments / 2).max(2);
    let base = m.positions.len() as i32;
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
