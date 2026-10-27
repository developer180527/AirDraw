//! Stroke data in format 4 (the `STRK` section), laid out so a general-purpose compressor
//! squeezes it well while every value still round-trips bit for bit.
//!
//! ```text
//! u32 stroke count
//! per stroke: u64 id  u32 layer  u32 brush  u32 point count n  u8 flags (1 = one shared normal, 2 = tilt)
//! then one stream per channel, over all strokes in order:
//!   pos.x  pos.y  pos.z  normal.x  normal.y  normal.z  pressure  tilt  time
//! ```
//!
//! Positions and pressure have one value per point, normals one per stroke (shared) or per
//! point, tilt one per point of strokes that have it (NaN = none), time one f64 per point.
//!
//! Each stream stores a residual per value, computed on the bit patterns with wrapping
//! integer maths (so nothing is lost), then splits the residuals into byte planes: all
//! lowest bytes, then all next bytes… Small residuals make the high planes long runs of
//! zeros, which the codec squeezes.
//!
//! - **Linear** streams (positions, pressure, time) predict each value from the two before
//!   it (`2·prev − prev2`, a straight-line continuation); the residual is zigzag-coded so
//!   small negatives stay small. Pen samples move smoothly and arrive at a steady rate, so
//!   this beats plain XOR: measured on real iPad sketches, positions −20%, time −45%.
//! - **Xor** streams (normals, tilt) store each value XORed with the previous one: they're
//!   mostly constant, where XOR gives zeros.
//!
//! Stream lengths follow from the stroke headers; nothing else is stored.

use std::sync::Arc;

use airdraw_brush::InputPoint;
use glam::Vec3;

use crate::DocError;
use crate::document::{BrushRef, LayerId, Stroke, StrokeId};

const SHARED_NORMAL: u8 = 1;
const HAS_TILT: u8 = 2;
const HEADER: usize = 8 + 4 + 4 + 4 + 1;

/// Encodes `strokes`; `brush` gives the brush index to write for each (snapshots are
/// renumbered on save).
pub(crate) fn encode(strokes: &[Stroke], brush: impl Fn(&Stroke) -> u32) -> Vec<u8> {
    let total: usize = strokes.iter().map(|s| s.points.len()).sum();
    let mut out = Vec::with_capacity(4 + strokes.len() * HEADER + total * 40);
    out.extend_from_slice(&(strokes.len() as u32).to_le_bytes());
    let mut flags = Vec::with_capacity(strokes.len());
    for s in strokes {
        let pts = &s.points;
        let f = if pts.windows(2).all(|p| p[0].normal == p[1].normal) { SHARED_NORMAL } else { 0 }
            | if pts.iter().any(|p| p.tilt.is_some()) { HAS_TILT } else { 0 };
        flags.push(f);
        out.extend_from_slice(&s.id.0.to_le_bytes());
        out.extend_from_slice(&s.layer.0.to_le_bytes());
        out.extend_from_slice(&brush(s).to_le_bytes());
        out.extend_from_slice(&(pts.len() as u32).to_le_bytes());
        out.push(f);
    }
    let all = || strokes.iter().flat_map(|s| s.points.iter());
    let normals = || {
        strokes.iter().zip(&flags).flat_map(|(s, f)| s.points.iter().take(if f & SHARED_NORMAL != 0 { 1 } else { s.points.len() }))
    };
    put_stream32(&mut out, Predict::Linear, all().map(|p| p.pos.x.to_bits()));
    put_stream32(&mut out, Predict::Linear, all().map(|p| p.pos.y.to_bits()));
    put_stream32(&mut out, Predict::Linear, all().map(|p| p.pos.z.to_bits()));
    put_stream32(&mut out, Predict::Xor, normals().map(|p| p.normal.x.to_bits()));
    put_stream32(&mut out, Predict::Xor, normals().map(|p| p.normal.y.to_bits()));
    put_stream32(&mut out, Predict::Xor, normals().map(|p| p.normal.z.to_bits()));
    put_stream32(&mut out, Predict::Linear, all().map(|p| p.pressure.to_bits()));
    let tilted = strokes.iter().zip(&flags).filter(|(_, f)| *f & HAS_TILT != 0).flat_map(|(s, _)| s.points.iter());
    put_stream32(&mut out, Predict::Xor, tilted.map(|p| p.tilt.unwrap_or(f32::NAN).to_bits()));
    put_stream64(&mut out, all().map(|p| p.t.to_bits()));
    out
}

pub(crate) fn decode(data: &[u8]) -> Result<Vec<Stroke>, DocError> {
    let bad = |why: String| DocError::Corrupt(format!("strokes: {why}"));
    let count = u32::from_le_bytes(data.get(..4).ok_or(DocError::Truncated)?.try_into().unwrap()) as usize;
    let headers = data.get(4..).and_then(|d| d.get(..count.checked_mul(HEADER)?)).ok_or(DocError::Truncated)?;
    struct Head {
        id: u64,
        layer: u32,
        brush: u32,
        n: usize,
        flags: u8,
    }
    let heads: Vec<Head> = headers
        .as_chunks::<HEADER>()
        .0
        .iter()
        .map(|h| Head {
            id: u64::from_le_bytes(h[0..8].try_into().unwrap()),
            layer: u32::from_le_bytes(h[8..12].try_into().unwrap()),
            brush: u32::from_le_bytes(h[12..16].try_into().unwrap()),
            n: u32::from_le_bytes(h[16..20].try_into().unwrap()) as usize,
            flags: h[20],
        })
        .collect();
    if let Some(h) = heads.iter().find(|h| h.n == 0) {
        return Err(bad(format!("stroke {} has no points", h.id)));
    }
    // Stream lengths, checked against the data before anything is allocated.
    let sum = |f: &dyn Fn(&Head) -> usize| heads.iter().map(f).sum::<usize>();
    let points = sum(&|h| h.n);
    let normals = sum(&|h| if h.flags & SHARED_NORMAL != 0 { 1 } else { h.n });
    let tilts = sum(&|h| if h.flags & HAS_TILT != 0 { h.n } else { 0 });
    let expected = (points as u128) * (4 * 4 + 8) + (normals as u128) * 12 + (tilts as u128) * 4;
    let mut rest = &data[4 + headers.len()..];
    if expected != rest.len() as u128 {
        return Err(bad(format!("{} bytes of point data, expected {expected}", rest.len())));
    }
    use Predict::{Linear, Xor};
    let f32s = |rest: &mut &[u8], how, n| -> Vec<f32> { take_stream32(rest, how, n).into_iter().map(f32::from_bits).collect() };
    let (px, py, pz) = (f32s(&mut rest, Linear, points), f32s(&mut rest, Linear, points), f32s(&mut rest, Linear, points));
    let (nx, ny, nz) = (f32s(&mut rest, Xor, normals), f32s(&mut rest, Xor, normals), f32s(&mut rest, Xor, normals));
    let pressure = f32s(&mut rest, Linear, points);
    let tilt = f32s(&mut rest, Xor, tilts);
    let time: Vec<f64> = take_stream64(&mut rest, points).into_iter().map(f64::from_bits).collect();

    let (mut p, mut nrm, mut ti) = (0, 0, 0);
    let strokes = heads
        .iter()
        .map(|h| {
            let pts: Vec<InputPoint> = (0..h.n)
                .map(|i| {
                    let k = if h.flags & SHARED_NORMAL != 0 { nrm } else { nrm + i };
                    InputPoint {
                        pos: Vec3::new(px[p + i], py[p + i], pz[p + i]),
                        normal: Vec3::new(nx[k], ny[k], nz[k]),
                        pressure: pressure[p + i],
                        tilt: if h.flags & HAS_TILT != 0 { Some(tilt[ti + i]).filter(|v| !v.is_nan()) } else { None },
                        t: time[p + i],
                    }
                })
                .collect();
            p += h.n;
            nrm += if h.flags & SHARED_NORMAL != 0 { 1 } else { h.n };
            if h.flags & HAS_TILT != 0 {
                ti += h.n;
            }
            Stroke { id: StrokeId(h.id), layer: LayerId(h.layer), brush: BrushRef(h.brush), points: Arc::from(pts) }
        })
        .collect();
    Ok(strokes)
}

#[derive(Clone, Copy)]
enum Predict {
    /// Residual from `2·prev − prev2`, zigzag-coded.
    Linear,
    /// XOR with the previous value.
    Xor,
}

fn put_stream32(out: &mut Vec<u8>, how: Predict, values: impl Iterator<Item = u32>) {
    let (mut a, mut b) = (0u32, 0u32);
    let residuals: Vec<u32> = values
        .map(|v| {
            let r = match how {
                Predict::Linear => zigzag32(v.wrapping_sub(b.wrapping_mul(2).wrapping_sub(a))),
                Predict::Xor => v ^ b,
            };
            (a, b) = (b, v);
            r
        })
        .collect();
    for plane in 0..4 {
        out.extend(residuals.iter().map(|v| (v >> (8 * plane)) as u8));
    }
}

/// Always linear: the only 64-bit stream is time.
fn put_stream64(out: &mut Vec<u8>, values: impl Iterator<Item = u64>) {
    let (mut a, mut b) = (0u64, 0u64);
    let residuals: Vec<u64> = values
        .map(|v| {
            let r = zigzag64(v.wrapping_sub(b.wrapping_mul(2).wrapping_sub(a)));
            (a, b) = (b, v);
            r
        })
        .collect();
    for plane in 0..8 {
        out.extend(residuals.iter().map(|v| (v >> (8 * plane)) as u8));
    }
}

/// Reads `n` values written by [`put_stream32`]; the caller has checked the length.
fn take_stream32(data: &mut &[u8], how: Predict, n: usize) -> Vec<u32> {
    let (bytes, rest) = data.split_at(n * 4);
    *data = rest;
    let (mut a, mut b) = (0u32, 0u32);
    (0..n)
        .map(|i| {
            let r = (0..4).fold(0u32, |v, plane| v | (bytes[plane * n + i] as u32) << (8 * plane));
            let v = match how {
                Predict::Linear => unzigzag32(r).wrapping_add(b.wrapping_mul(2).wrapping_sub(a)),
                Predict::Xor => r ^ b,
            };
            (a, b) = (b, v);
            v
        })
        .collect()
}

fn take_stream64(data: &mut &[u8], n: usize) -> Vec<u64> {
    let (bytes, rest) = data.split_at(n * 8);
    *data = rest;
    let (mut a, mut b) = (0u64, 0u64);
    (0..n)
        .map(|i| {
            let r = (0..8).fold(0u64, |v, plane| v | (bytes[plane * n + i] as u64) << (8 * plane));
            let v = unzigzag64(r).wrapping_add(b.wrapping_mul(2).wrapping_sub(a));
            (a, b) = (b, v);
            v
        })
        .collect()
}

/// Maps small negative residuals (as wrapped u32) to small numbers: 0, -1, 1, -2… → 0, 1, 2, 3…
fn zigzag32(v: u32) -> u32 {
    (v << 1) ^ ((v as i32 >> 31) as u32)
}

fn unzigzag32(v: u32) -> u32 {
    (v >> 1) ^ (v & 1).wrapping_neg()
}

fn zigzag64(v: u64) -> u64 {
    (v << 1) ^ ((v as i64 >> 63) as u64)
}

fn unzigzag64(v: u64) -> u64 {
    (v >> 1) ^ (v & 1).wrapping_neg()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::tests::points;

    fn stroke(id: u64, pts: Vec<InputPoint>) -> Stroke {
        Stroke { id: StrokeId(id), layer: LayerId(1), brush: BrushRef(id as u32 % 2), points: Arc::from(pts) }
    }

    fn mixed() -> Vec<Stroke> {
        let mut curved = points(20, 1.0);
        for (i, p) in curved.iter_mut().enumerate() {
            p.normal = Vec3::new(0.0, (i as f32).cos(), (i as f32).sin());
            p.tilt = (i % 3 != 0).then_some(i as f32 * 0.1);
        }
        let mut flat = points(30, 0.0);
        flat.iter_mut().for_each(|p| p.tilt = None);
        let mut odd = points(1, 2.0);
        odd[0].pos = Vec3::new(-0.0, f32::MIN_POSITIVE / 2.0, f32::MAX);
        odd[0].pressure = f32::NAN;
        odd[0].t = -1e300;
        vec![stroke(3, curved), stroke(7, flat), stroke(8, odd)]
    }

    fn bits(s: &[Stroke]) -> Vec<Vec<u32>> {
        s.iter()
            .map(|s| {
                s.points
                    .iter()
                    .flat_map(|p| {
                        let t = p.t.to_bits();
                        [p.pos.x, p.pos.y, p.pos.z, p.normal.x, p.normal.y, p.normal.z, p.pressure, p.tilt.unwrap_or(-7.0)]
                            .map(f32::to_bits)
                            .into_iter()
                            .chain([t as u32, (t >> 32) as u32])
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn round_trips_bit_for_bit() {
        let s = mixed();
        let back = decode(&encode(&s, |s| s.brush.0)).unwrap();
        // Compared as bits: -0.0, subnormals and NaN payloads survive too.
        assert_eq!(bits(&back), bits(&s));
        assert_eq!(back.iter().map(|s| (s.id, s.layer, s.brush)).collect::<Vec<_>>(), s.iter().map(|s| (s.id, s.layer, s.brush)).collect::<Vec<_>>());
        assert!(back[1].points.iter().all(|p| p.tilt.is_none()));
        assert_eq!(decode(&encode(&[], |_| 0)).unwrap(), Vec::new());
    }

    #[test]
    fn zigzag_round_trips_at_the_edges() {
        for v in [0, 1, 2, u32::MAX, u32::MAX - 1, i32::MAX as u32, i32::MIN as u32] {
            assert_eq!(unzigzag32(zigzag32(v)), v);
            assert_eq!(unzigzag64(zigzag64(v as u64 | (v as u64) << 32)), v as u64 | (v as u64) << 32);
        }
        assert_eq!([0u32, u32::MAX, 1, u32::MAX - 1].map(zigzag32), [0, 1, 2, 3]);
    }

    #[test]
    fn brush_indices_are_remapped() {
        let back = decode(&encode(&mixed(), |s| s.id.0 as u32 + 10)).unwrap();
        assert_eq!(back.iter().map(|s| s.brush.0).collect::<Vec<_>>(), [13, 17, 18]);
    }

    #[test]
    fn wrong_lengths_are_refused_before_allocating() {
        let bytes = encode(&mixed(), |s| s.brush.0);
        for cut in [0, 3, 10, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(decode(&longer), Err(DocError::Corrupt(_))));
        // A stroke claiming billions of points.
        let mut huge = bytes.clone();
        huge[4 + 16..4 + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode(&huge), Err(DocError::Corrupt(_))));
        let mut many = bytes.clone();
        many[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&many).unwrap_err(), DocError::Truncated);
        let mut empty = bytes;
        empty[4 + 16..4 + 20].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(decode(&empty), Err(DocError::Corrupt(_))));
    }
}

