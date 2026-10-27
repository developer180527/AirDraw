use glam::Vec3;

use crate::params::{ParamSpec, Params, ParamsExt};
use crate::stage::{Filter, Stage, StrokePoint};
use crate::stream::PointStream;

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

    fn stream(&self, p: &Params) -> Option<Box<dyn PointStream>> {
        Some(Box::new(ResampleStream {
            spacing: p.float("spacing"),
            seen: 0,
            head: Vec::new(),
            kept: 0,
            held: Vec::new(),
            last: None,
        }))
    }
}

/// [`Resample`] streamed: a kept point is final once a newer point is kept, because the end
/// rule only ever drops the newest kept point.
#[derive(Clone)]
struct ResampleStream {
    spacing: f32,
    /// Input points so far.
    seen: usize,
    /// The first two input points: strokes of fewer than 3 points pass through unchanged.
    head: Vec<StrokePoint>,
    /// Points kept so far.
    kept: usize,
    /// Kept points not emitted yet. The newest kept point is always among them.
    held: Vec<StrokePoint>,
    /// The newest input point.
    last: Option<StrokePoint>,
}

impl ResampleStream {
    fn feed(&mut self, input: &[StrokePoint]) {
        for pt in input {
            if self.head.len() < 2 {
                self.head.push(*pt);
            }
            self.seen += 1;
            self.last = Some(*pt);
            match self.held.last() {
                Some(prev) if prev.pos.distance(pt.pos) < self.spacing => {}
                _ => {
                    self.held.push(*pt);
                    self.kept += 1;
                }
            }
        }
    }
}

impl PointStream for ResampleStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        if self.spacing <= 0.0 {
            out.extend_from_slice(input);
            return;
        }
        self.feed(input);
        if self.seen >= 3 && self.held.len() > 1 {
            out.extend(self.held.drain(..self.held.len() - 1));
        }
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        if self.spacing <= 0.0 {
            out.extend_from_slice(tail);
            return;
        }
        let mut s = self.clone();
        s.feed(tail);
        if s.seen < 3 {
            out.extend(s.head);
            return;
        }
        let Some(last) = s.last else { return };
        if s.held.last() != Some(&last) {
            if s.kept > 1 && s.held.last().unwrap().pos.distance(last.pos) < s.spacing * 0.5 {
                s.held.pop();
            }
            s.held.push(last);
        }
        out.extend(s.held);
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

    fn stream(&self, p: &Params) -> Option<Box<dyn PointStream>> {
        Some(Box::new(ExponentialStream { k: 1.0 - p.float("amount"), snap_end: p.flag("snap_end"), prev: None, held: None }))
    }
}

/// [`Exponential`] streamed: causal; only the newest point is held back for `snap_end`.
#[derive(Clone)]
struct ExponentialStream {
    k: f32,
    snap_end: bool,
    /// The previous filtered point.
    prev: Option<StrokePoint>,
    /// The newest point, raw and filtered, not emitted yet.
    held: Option<(StrokePoint, StrokePoint)>,
}

impl ExponentialStream {
    fn feed(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        for pt in input {
            let mut cur = *pt;
            if let Some(prev) = self.prev {
                cur.pos = prev.pos.lerp(cur.pos, self.k);
                cur.pressure = prev.pressure + (cur.pressure - prev.pressure) * self.k;
            }
            self.prev = Some(cur);
            if let Some((_, filtered)) = self.held.replace((*pt, cur)) {
                out.push(filtered);
            }
        }
    }
}

impl PointStream for ExponentialStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        self.feed(input, out);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        let mut s = self.clone();
        s.feed(tail, out);
        if let Some((raw, filtered)) = s.held {
            out.push(if s.snap_end { raw } else { filtered });
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
        let (min_cutoff, beta) = (p.float("min_cutoff"), p.float("beta"));
        let raw_last = pts.last().copied();
        let Some(first) = pts.first() else { return };
        let mut state = EuroState::new(first);
        for pt in pts.iter_mut().skip(1) {
            state.step(pt, min_cutoff, beta);
        }
        if p.flag("snap_end")
            && let (Some(raw), Some(last)) = (raw_last, pts.last_mut())
        {
            *last = raw;
        }
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn PointStream>> {
        Some(Box::new(OneEuroStream {
            min_cutoff: p.float("min_cutoff"),
            beta: p.float("beta"),
            snap_end: p.flag("snap_end"),
            state: None,
            held: None,
        }))
    }
}

/// 1€ filter state between points.
#[derive(Clone, Copy)]
struct EuroState {
    prev_raw: Vec3,
    prev_hat: Vec3,
    d_hat: Vec3,
    prev_t: f64,
    prev_pressure: f32,
}

impl EuroState {
    fn new(first: &StrokePoint) -> Self {
        Self { prev_raw: first.pos, prev_hat: first.pos, d_hat: Vec3::ZERO, prev_t: first.t, prev_pressure: first.pressure }
    }

    /// Filters `pt` in place (every point after the first).
    fn step(&mut self, pt: &mut StrokePoint, min_cutoff: f32, beta: f32) {
        const D_CUTOFF: f32 = 1.0;
        // Batched events can share a timestamp; assume at least a 1 kHz report rate.
        const MIN_DT: f32 = 1.0 / 1000.0;
        let alpha = |cutoff: f32, dt: f32| {
            let tau = 1.0 / (std::f32::consts::TAU * cutoff);
            1.0 / (1.0 + tau / dt)
        };
        let dt = ((pt.t - self.prev_t) as f32).max(MIN_DT);
        let d = (pt.pos - self.prev_raw) / dt;
        self.d_hat = self.d_hat.lerp(d, alpha(D_CUTOFF, dt));
        let a = alpha(min_cutoff + beta * self.d_hat.length(), dt);
        self.prev_raw = pt.pos;
        pt.pos = self.prev_hat.lerp(pt.pos, a);
        pt.pressure = self.prev_pressure + (pt.pressure - self.prev_pressure) * a;
        self.prev_hat = pt.pos;
        self.prev_pressure = pt.pressure;
        self.prev_t = pt.t;
    }
}

/// [`OneEuro`] streamed: causal; only the newest point is held back for `snap_end`.
#[derive(Clone)]
struct OneEuroStream {
    min_cutoff: f32,
    beta: f32,
    snap_end: bool,
    state: Option<EuroState>,
    /// The newest point, raw and filtered, not emitted yet.
    held: Option<(StrokePoint, StrokePoint)>,
}

impl OneEuroStream {
    fn feed(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        for pt in input {
            let mut cur = *pt;
            match &mut self.state {
                None => self.state = Some(EuroState::new(pt)),
                Some(state) => state.step(&mut cur, self.min_cutoff, self.beta),
            }
            if let Some((_, filtered)) = self.held.replace((*pt, cur)) {
                out.push(filtered);
            }
        }
    }
}

impl PointStream for OneEuroStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        self.feed(input, out);
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        let mut s = self.clone();
        s.feed(tail, out);
        if let Some((raw, filtered)) = s.held {
            out.push(if s.snap_end { raw } else { filtered });
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
            let i = i as isize;
            catmull_rom([get(i - 1), get(i), get(i + 1), get(i + 2)], steps, pts);
        }
        pts.push(src[n - 1]);
    }

    fn stream(&self, p: &Params) -> Option<Box<dyn PointStream>> {
        Some(Box::new(SplineStream { steps: p.int("steps").max(1) as usize, src: Vec::new(), next: 0 }))
    }
}

/// The `steps` points of the segment from `p[1]` to `p[2]` (excluding `p[2]`).
fn catmull_rom(p: [StrokePoint; 4], steps: usize, out: &mut Vec<StrokePoint>) {
    let [p0, p1, p2, p3] = p;
    for s in 0..steps {
        let t = s as f32 / steps as f32;
        let (t2, t3) = (t * t, t * t * t);
        let pos = 0.5
            * ((2.0 * p1.pos)
                + (p2.pos - p0.pos) * t
                + (2.0 * p0.pos - 5.0 * p1.pos + 4.0 * p2.pos - p3.pos) * t2
                + (3.0 * p1.pos - p0.pos - 3.0 * p2.pos + p3.pos) * t3);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        out.push(StrokePoint {
            pos,
            normal: p1.normal.lerp(p2.normal, t).normalize_or(p1.normal),
            pressure: lerp(p1.pressure, p2.pressure),
            tilt: p1.tilt.zip(p2.tilt).map(|(a, b)| lerp(a, b)).or(p1.tilt),
            t: p1.t + (p2.t - p1.t) * t as f64,
            ..p1
        });
    }
}

/// [`Spline`] streamed: segment `i` is final once point `i + 2` exists (only the stroke's end
/// clamps indices); the last two segments and the end point are provisional.
struct SplineStream {
    steps: usize,
    /// Committed input.
    src: Vec<StrokePoint>,
    /// Next segment to emit.
    next: usize,
}

impl PointStream for SplineStream {
    fn commit(&mut self, input: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        if self.steps < 2 {
            out.extend_from_slice(input);
            return;
        }
        self.src.extend_from_slice(input);
        let n = self.src.len();
        while n >= 3 && self.next + 2 < n {
            let i = self.next;
            catmull_rom([self.src[i.saturating_sub(1)], self.src[i], self.src[i + 1], self.src[i + 2]], self.steps, out);
            self.next += 1;
        }
    }

    fn finish(&self, tail: &[StrokePoint], out: &mut Vec<StrokePoint>) {
        if self.steps < 2 {
            out.extend_from_slice(tail);
            return;
        }
        let n = self.src.len() + tail.len();
        if n < 3 {
            // Nothing was emitted yet: the stroke passes through unchanged.
            out.extend_from_slice(&self.src);
            out.extend_from_slice(tail);
            return;
        }
        let get = |i: isize| {
            let i = i.clamp(0, n as isize - 1) as usize;
            if i < self.src.len() { self.src[i] } else { tail[i - self.src.len()] }
        };
        for i in self.next..n - 1 {
            let i = i as isize;
            catmull_rom([get(i - 1), get(i), get(i + 1), get(i + 2)], self.steps, out);
        }
        out.push(get(n as isize - 1));
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
