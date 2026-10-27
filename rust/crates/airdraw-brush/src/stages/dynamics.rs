use crate::params::{ParamSpec, Params, ParamsExt};
use crate::stage::{Dynamics, Stage, StrokePoint};

/// Width from pressure (through a curve), thinned by speed.
pub struct Width;

impl Stage for Width {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::float("base", 0.012, 0.0005, 0.5, "Width at full pressure, world units"),
            ParamSpec::curve("pressure_curve", &[[0.0, 0.25], [1.0, 1.0]], "Pressure (x) → width multiplier (y)"),
            ParamSpec::float("speed_thinning", 0.0, 0.0, 1.0, "How much fast strokes get thinner"),
            ParamSpec::float("speed_ref", 2.0, 0.01, 50.0, "Speed (units/s) at which thinning is full"),
        ]
    }
}

impl Dynamics for Width {
    fn apply(&self, pts: &mut [StrokePoint], p: &Params) {
        let (base, curve) = (p.float("base"), p.curve("pressure_curve"));
        let (thin, speed_ref) = (p.float("speed_thinning"), p.float("speed_ref"));
        let mut speed = 0.0f32;
        for i in 0..pts.len() {
            if thin > 0.0 && i > 0 {
                let (a, b) = (pts[i - 1], pts[i]);
                let dt = (b.t - a.t) as f32;
                if dt > 1e-4 {
                    // Light smoothing so one fast sample doesn't pinch the stroke.
                    speed = speed * 0.7 + (b.dist - a.dist) / dt * 0.3;
                }
            }
            let thinning = 1.0 - thin * (speed / speed_ref).min(1.0);
            pts[i].width = base * curve.eval(pts[i].pressure.clamp(0.0, 1.0)) * thinning;
        }
    }
}

/// Narrows the ends of the stroke over a distance.
pub struct Taper;

impl Stage for Taper {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::float("start", 0.03, 0.0, 5.0, "Taper length at the start, world units"),
            ParamSpec::float("end", 0.05, 0.0, 5.0, "Taper length at the end, world units"),
            ParamSpec::float("min", 0.15, 0.0, 1.0, "Width multiplier at the very tips"),
        ]
    }
}

impl Dynamics for Taper {
    fn apply(&self, pts: &mut [StrokePoint], p: &Params) {
        let Some(total) = pts.last().map(|l| l.dist) else { return };
        let (start, end, min) = (p.float("start"), p.float("end"), p.float("min"));
        // Short strokes taper proportionally so both ends still fit.
        let fit = if start + end > total && start + end > 0.0 { total / (start + end) } else { 1.0 };
        let ramp = |d: f32, len: f32| if len <= 0.0 { 1.0 } else { smoothstep((d / len).clamp(0.0, 1.0)) };
        for pt in pts.iter_mut() {
            let k = ramp(pt.dist, start * fit) * ramp(total - pt.dist, end * fit);
            pt.width *= min + (1.0 - min) * k;
        }
    }
}

fn smoothstep(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::InputPoint;
    use glam::Vec3;

    fn pts(pressures: &[f32]) -> Vec<StrokePoint> {
        pressures
            .iter()
            .enumerate()
            .map(|(i, &pressure)| {
                let mut p = StrokePoint::from(&InputPoint {
                    pos: Vec3::new(i as f32 * 0.1, 0.0, 0.0),
                    normal: Vec3::Z,
                    pressure,
                    tilt: None,
                    t: i as f64 * 0.01,
                });
                p.dist = i as f32 * 0.1;
                p
            })
            .collect()
    }

    fn defaults(spec: Vec<ParamSpec>) -> Params {
        spec.into_iter().map(|s| (s.name.to_string(), s.default)).collect()
    }

    #[test]
    fn width_follows_pressure_curve() {
        let mut p = pts(&[0.0, 1.0]);
        Width.apply(&mut p, &defaults(Width.schema()));
        assert!((p[0].width - 0.012 * 0.25).abs() < 1e-6);
        assert!((p[1].width - 0.012).abs() < 1e-6);
    }

    #[test]
    fn taper_narrows_ends_not_middle() {
        let mut p = pts(&[1.0; 11]);
        for pt in p.iter_mut() {
            pt.width = 1.0;
        }
        Taper.apply(&mut p, &defaults(Taper.schema()));
        assert!((p[0].width - 0.15).abs() < 1e-6);
        assert!((p[10].width - 0.15).abs() < 1e-6);
        assert_eq!(p[5].width, 1.0);
    }
}
