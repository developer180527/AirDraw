use glam::Vec3;

use crate::params::{ParamSpec, Params, ParamsExt};
use crate::stage::{Filter, Stage, StrokePoint};

/// Drops points closer than `spacing` to the previously kept one. Always keeps the last
/// point so the stroke ends exactly at the pen.
pub struct Resample;

impl Stage for Resample {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![ParamSpec::float("spacing", 0.004, 0.0, 1.0, "Minimum distance between points, world units")]
    }
}

impl Filter for Resample {
    fn apply(&self, pts: &mut Vec<StrokePoint>, p: &Params) {
        let spacing = p.float("spacing");
        if pts.len() < 3 || spacing <= 0.0 {
            return;
        }
        let last = *pts.last().unwrap();
        let mut kept: Vec<StrokePoint> = Vec::with_capacity(pts.len());
        for pt in pts.iter() {
            match kept.last() {
                Some(prev) if prev.pos.distance(pt.pos) < spacing => {}
                _ => kept.push(*pt),
            }
        }
        if kept.last() != Some(&last) {
            if kept.len() > 1 && kept.last().unwrap().pos.distance(last.pos) < spacing * 0.5 {
                kept.pop();
            }
            kept.push(last);
        }
        *pts = kept;
    }
}

/// Exponential moving average of positions and pressure. Simple and frame-rate independent
/// enough for a fallback; `one_euro` adapts to speed and is the better default.
pub struct Exponential;

impl Stage for Exponential {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::float("amount", 0.5, 0.0, 0.95, "0 = none, higher = smoother but laggier"),
            ParamSpec::bool("snap_end", true, "Keep the last point on the pen so the stroke never trails"),
        ]
    }
}

impl Filter for Exponential {
    fn apply(&self, pts: &mut Vec<StrokePoint>, p: &Params) {
        let k = 1.0 - p.float("amount");
        let raw_last = pts.last().copied();
        for i in 1..pts.len() {
            let prev = pts[i - 1];
            let cur = &mut pts[i];
            cur.pos = prev.pos.lerp(cur.pos, k);
            cur.pressure = prev.pressure + (cur.pressure - prev.pressure) * k;
        }
        if p.flag("snap_end")
            && let (Some(raw), Some(last)) = (raw_last, pts.last_mut())
        {
            *last = raw;
        }
    }
}

/// The 1€ filter (Casiez et al.): heavy smoothing when slow, little lag when fast.
pub struct OneEuro;

impl Stage for OneEuro {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::float("min_cutoff", 3.0, 0.01, 60.0, "Hz; lower = smoother at slow speeds"),
            ParamSpec::float("beta", 20.0, 0.0, 1000.0, "How quickly smoothing drops as speed rises"),
            ParamSpec::bool("snap_end", true, "Keep the last point on the pen so the stroke never trails"),
        ]
    }
}

impl Filter for OneEuro {
    fn apply(&self, pts: &mut Vec<StrokePoint>, p: &Params) {
        const D_CUTOFF: f32 = 1.0;
        // Batched events can share a timestamp; assume at least a 1 kHz report rate.
        const MIN_DT: f32 = 1.0 / 1000.0;
        let (min_cutoff, beta) = (p.float("min_cutoff"), p.float("beta"));
        let alpha = |cutoff: f32, dt: f32| {
            let tau = 1.0 / (std::f32::consts::TAU * cutoff);
            1.0 / (1.0 + tau / dt)
        };
        let raw_last = pts.last().copied();
        let Some(first) = pts.first() else { return };
        let (mut prev_raw, mut prev_hat, mut d_hat, mut prev_t) = (first.pos, first.pos, Vec3::ZERO, first.t);
        let mut prev_pressure = first.pressure;
        for pt in pts.iter_mut().skip(1) {
            let dt = ((pt.t - prev_t) as f32).max(MIN_DT);
            let d = (pt.pos - prev_raw) / dt;
            d_hat = d_hat.lerp(d, alpha(D_CUTOFF, dt));
            let a = alpha(min_cutoff + beta * d_hat.length(), dt);
            prev_raw = pt.pos;
            pt.pos = prev_hat.lerp(pt.pos, a);
            pt.pressure = prev_pressure + (pt.pressure - prev_pressure) * a;
            prev_hat = pt.pos;
            prev_pressure = pt.pressure;
            prev_t = pt.t;
        }
        if p.flag("snap_end")
            && let (Some(raw), Some(last)) = (raw_last, pts.last_mut())
        {
            *last = raw;
        }
    }
}

/// Catmull-Rom subdivision: inserts `steps - 1` points between each pair.
pub struct Spline;

impl Stage for Spline {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![ParamSpec::int("steps", 3, 1, 16, "Segments per input segment")]
    }
}

impl Filter for Spline {
    fn apply(&self, pts: &mut Vec<StrokePoint>, p: &Params) {
        let steps = p.int("steps").max(1) as usize;
        let n = pts.len();
        if n < 3 || steps < 2 {
            return;
        }
        let src = std::mem::take(pts);
        let get = |i: isize| src[i.clamp(0, n as isize - 1) as usize];
        pts.reserve((n - 1) * steps + 1);
        for i in 0..n - 1 {
            let (p0, p1, p2, p3) = (get(i as isize - 1), get(i as isize), get(i as isize + 1), get(i as isize + 2));
            for s in 0..steps {
                let t = s as f32 / steps as f32;
                let (t2, t3) = (t * t, t * t * t);
                let pos = 0.5
                    * ((2.0 * p1.pos)
                        + (p2.pos - p0.pos) * t
                        + (2.0 * p0.pos - 5.0 * p1.pos + 4.0 * p2.pos - p3.pos) * t2
                        + (3.0 * p1.pos - p0.pos - 3.0 * p2.pos + p3.pos) * t3);
                let lerp = |a: f32, b: f32| a + (b - a) * t;
                pts.push(StrokePoint {
                    pos,
                    normal: p1.normal.lerp(p2.normal, t).normalize_or(p1.normal),
                    pressure: lerp(p1.pressure, p2.pressure),
                    tilt: p1.tilt.zip(p2.tilt).map(|(a, b)| lerp(a, b)).or(p1.tilt),
                    t: p1.t + (p2.t - p1.t) * t as f64,
                    ..p1
                });
            }
        }
        pts.push(src[n - 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::InputPoint;

    fn line(n: usize, step: f32) -> Vec<StrokePoint> {
        (0..n)
            .map(|i| {
                let ip = InputPoint {
                    pos: Vec3::new(i as f32 * step, 0.0, 0.0),
                    normal: Vec3::Z,
                    pressure: 1.0,
                    tilt: None,
                    t: i as f64 / 120.0,
                };
                StrokePoint::from(&ip)
            })
            .collect()
    }

    fn params(spec: Vec<ParamSpec>) -> Params {
        spec.into_iter().map(|s| (s.name.to_string(), s.default)).collect()
    }

    #[test]
    fn resample_drops_close_points_keeps_end() {
        let mut pts = line(101, 0.001);
        Resample.apply(&mut pts, &params(Resample.schema()));
        assert!(pts.len() < 30);
        assert_eq!(pts.last().unwrap().pos.x, 0.1);
    }

    #[test]
    fn smoothing_filters_end_on_the_pen() {
        for f in [&Exponential as &dyn Filter, &OneEuro] {
            let mut pts = line(20, 0.1);
            f.apply(&mut pts, &params(f.schema()));
            assert_eq!(pts.last().unwrap().pos.x, 1.9);
        }
    }

    #[test]
    fn one_euro_smooths_jitter() {
        let mut pts = line(60, 0.002);
        for (i, p) in pts.iter_mut().enumerate() {
            p.pos.y = if i % 2 == 0 { 0.003 } else { -0.003 };
        }
        OneEuro.apply(&mut pts, &params(OneEuro.schema()));
        let max_y = pts[10..59].iter().map(|p| p.pos.y.abs()).fold(0.0, f32::max);
        assert!(max_y < 0.002, "jitter not reduced: {max_y}");
    }

    #[test]
    fn spline_subdivides() {
        let mut pts = line(4, 1.0);
        Spline.apply(&mut pts, &params(Spline.schema()));
        assert_eq!(pts.len(), 3 * 3 + 1);
        assert_eq!(pts.last().unwrap().pos.x, 3.0);
    }
}
