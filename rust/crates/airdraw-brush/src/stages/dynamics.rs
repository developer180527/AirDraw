use crate::params::{Curve, ParamSpec, Params, ParamsExt};
use crate::stage::{Dynamics, Stage, StrokePoint};
use crate::stream::PointStream;

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
        let mut state = WidthStream::new(p);
        for i in 0..pts.len() {
            let prev = i.checked_sub(1).map(|j| pts[j]);
            state.width(&mut pts[i], prev);
        }
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn PointStream>> {
        Some(Box::new(WidthStream::new(p)))
    }
}

/// [`Width`] state; causal, so the stream emits every point immediately.
#[derive(Clone)]
struct WidthStream {
    base: f32,
    curve: Vec<[f32; 2]>,
    thin: f32,
    speed_ref: f32,
    speed: f32,
    prev: Option<StrokePoint>,
}

impl WidthStream {
    fn new(p: &Params) -> Self {
        Self {
            base: p.float("base"),
            curve: p.curve("pressure_curve").0.to_vec(),
            thin: p.float("speed_thinning"),
            speed_ref: p.float("speed_ref"),
            speed: 0.0,
            prev: None,
        }
    }

    /// Sets `pt.width`; `prev` is the point before it, if any.
    fn width(&mut self, pt: &mut StrokePoint, prev: Option<StrokePoint>) {
        if self.thin > 0.0
            && let Some(a) = prev
        {
            let dt = (pt.t - a.t) as f32;
            if dt > 1e-4 {
                // Light smoothing so one fast sample doesn't pinch the stroke.
                self.speed = self.speed * 0.7 + (pt.dist - a.dist) / dt * 0.3;
            }
        }
        let thinning = 1.0 - self.thin * (self.speed / self.speed_ref).min(1.0);
        pt.width = self.base * Curve(&self.curve).eval(pt.pressure.clamp(0.0, 1.0)) * thinning;
    }

    fn feed(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        for pt in input {
            let mut cur = *pt;
            let prev = self.prev;
            self.width(&mut cur, prev);
            self.prev = Some(cur);
            out.push(cur);
        }
    }
}

impl PointStream for WidthStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        self.feed(input, out);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        self.clone().feed(tail, out);
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
        let t = TaperStream::new(p);
        let fit = t.fit(total);
        for pt in pts.iter_mut() {
            t.taper(pt, total, fit);
        }
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn PointStream>> {
        Some(Box::new(TaperStream::new(p)))
    }
}

/// [`Taper`] streamed. A point is final once the stroke is long enough that `fit` can't
/// change and the point is at least `end` from the tip (its end ramp is exactly 1 for good).
struct TaperStream {
    start: f32,
    end: f32,
    min: f32,
    /// Points not emitted yet, oldest first.
    held: Vec<StrokePoint>,
}

impl TaperStream {
    fn new(p: &Params) -> Self {
        Self { start: p.float("start"), end: p.float("end"), min: p.float("min"), held: Vec::new() }
    }

    /// Short strokes taper proportionally so both ends still fit.
    fn fit(&self, total: f32) -> f32 {
        let (start, end) = (self.start, self.end);
        if start + end > total && start + end > 0.0 { total / (start + end) } else { 1.0 }
    }

    fn taper(&self, pt: &mut StrokePoint, total: f32, fit: f32) {
        let ramp = |d: f32, len: f32| if len <= 0.0 { 1.0 } else { smoothstep((d / len).clamp(0.0, 1.0)) };
        let k = ramp(pt.dist, self.start * fit) * ramp(total - pt.dist, self.end * fit);
        pt.width *= self.min + (1.0 - self.min) * k;
    }
}

impl PointStream for TaperStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        self.held.extend_from_slice(input);
        let Some(total) = self.held.last().map(|l| l.dist) else { return };
        // `dist` only grows, so once fit is 1 it stays 1.
        if self.fit(total) != 1.0 {
            return;
        }
        let done = self.held.iter().take_while(|p| self.end <= 0.0 || total - p.dist >= self.end).count();
        let mut held = std::mem::take(&mut self.held);
        for pt in &mut held[..done] {
            // The end ramp is 1 at this total and every later one, so this is the final width.
            self.taper(pt, total, 1.0);
        }
        out.extend(held.drain(..done));
        self.held = held;
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        let Some(total) = tail.last().or(self.held.last()).map(|l| l.dist) else { return };
        let fit = self.fit(total);
        for pt in self.held.iter().chain(tail) {
            let mut pt = *pt;
            self.taper(&mut pt, total, fit);
            out.push(pt);
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
