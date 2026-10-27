//! File format size and speed measurements (docs/ARCHITECTURE.md §3h step 2). Shared by
//! the `file_format_benchmark` test on desktop and the in-app benchmark on device
//! (`scripts/bench_ipad.sh file-format`), so both report the same table.

use std::fmt::Write;
use std::time::{Duration, Instant};

use airdraw_brush::InputPoint;
use glam::Vec3;

use crate::document::{Document, Stroke};
use crate::file::{Level, SaveOptions};

/// Sizes for each sample, then times for one large sketch of at least `big_points` points
/// built by tiling the samples' strokes: real pen paths rather than synthetic curves, with
/// per-point jitter so no copy repeats another (exact repeats compress unrealistically well).
pub fn file_format(samples: &[(String, Document)], big_points: usize) -> String {
    let mut out = String::new();
    let fast = SaveOptions { level: Level::Fast };
    let max = SaveOptions { level: Level::Max };
    let _ = writeln!(
        out,
        "{:<26} {:>9} {:>9} {:>10} {:>10} {:>7} {:>10} {:>7} {:>10}",
        "sketch", "points", "thumb", "format 3", "4 fast", "ratio", "4 max", "ratio", "no thumb"
    );
    let (mut sum3, mut sum4, mut sum_thumb) = (0, 0, 0);
    for (name, d) in samples {
        let thumb = d.thumbnail().map_or(0, <[u8]>::len);
        let v3 = d.to_bytes_format3().len();
        let (f, m) = (d.to_bytes_with(fast).len(), d.to_bytes_with(max).len());
        (sum3, sum4, sum_thumb) = (sum3 + v3, sum4 + f, sum_thumb + thumb);
        let _ = writeln!(
            out,
            "{name:<26} {:>9} {thumb:>9} {v3:>10} {f:>10} {:>6.1}× {m:>10} {:>6.1}× {:>9.1}×",
            points(d),
            v3 as f64 / f as f64,
            v3 as f64 / m as f64,
            (v3 - thumb) as f64 / (f - thumb) as f64
        );
    }
    if !samples.is_empty() {
        let _ = writeln!(
            out,
            "{:<26} {:>9} {sum_thumb:>9} {sum3:>10} {sum4:>10} {:>6.1}× {:>10} {:>7} {:>9.1}×",
            "all samples",
            "",
            sum3 as f64 / sum4 as f64,
            "",
            "",
            (sum3 - sum_thumb) as f64 / (sum4 - sum_thumb) as f64
        );
    }
    let Some(big) = tiled(samples, big_points) else {
        let _ = writeln!(out, "(no strokes to build a large sketch from)");
        return out;
    };
    let v3 = big.to_bytes_format3();
    let v4 = big.to_bytes_with(fast);
    let v4_max = big.to_bytes_with(max);
    let _ = writeln!(
        out,
        "{:<26} {:>9} {:>9} {:>10} {:>10} {:>6.1}× {:>10} {:>6.1}×   (jittered copies: worst case)",
        format!("tiled ({} strokes)", big.strokes().len()),
        points(&big),
        big.thumbnail().map_or(0, <[u8]>::len),
        v3.len(),
        v4.len(),
        v3.len() as f64 / v4.len() as f64,
        v4_max.len(),
        v3.len() as f64 / v4_max.len() as f64
    );
    let _ = writeln!(out, "\ntimes for the tiled sketch, median of 5 (ms):");
    let time = |f: &dyn Fn()| {
        let mut runs: Vec<Duration> = (0..5)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed()
            })
            .collect();
        runs.sort();
        runs[2].as_secs_f64() * 1000.0
    };
    let rows: [(&str, f64); 5] = [
        ("save format 3", time(&|| drop(big.to_bytes_format3()))),
        ("save format 4 fast (autosave)", time(&|| drop(big.to_bytes_with(fast)))),
        ("save format 4 max (copy)", time(&|| drop(big.to_bytes_with(max)))),
        ("open format 3", time(&|| drop(Document::from_bytes(&v3).unwrap()))),
        ("open format 4", time(&|| drop(Document::from_bytes(&v4).unwrap()))),
    ];
    for (what, ms) in rows {
        let _ = writeln!(out, "  {what:<30} {ms:>8.1}");
    }
    out
}

fn points(d: &Document) -> usize {
    d.strokes().iter().map(|s| s.points.len()).sum()
}

/// The largest sample, with copies of all samples' strokes added until it has at least
/// `target` points. Each copy is shifted, and every point gets its own small jitter
/// (±0.5 mm in position, ±0.002 pressure, ±0.1 ms time: about a pen's own noise).
fn tiled(samples: &[(String, Document)], target: usize) -> Option<Document> {
    let (_, base) = samples.iter().max_by_key(|(_, d)| points(d))?;
    let source: Vec<(&Document, &Stroke)> = samples.iter().flat_map(|(_, d)| d.strokes().iter().map(move |s| (d, s))).collect();
    if source.is_empty() {
        return None;
    }
    let mut big = base.clone();
    let mut copy = 1;
    let mut seed = 1u32;
    while points(&big) < target {
        let shift = Vec3::new(0.37, -0.21, 0.13) * copy as f32;
        for (doc, s) in &source {
            let brush = big.intern_brush(doc.brush_def(s.brush).unwrap());
            let pts: Vec<_> = s
                .points
                .iter()
                .map(|p| {
                    let base = seed;
                    seed = seed.wrapping_add(4);
                    let j = |k: u32| noise(base.wrapping_add(k));
                    InputPoint {
                        pos: p.pos + shift + Vec3::new(j(0), j(1), j(2)) * 0.0005,
                        pressure: (p.pressure + j(3) * 0.002).clamp(0.0, 1.0),
                        t: p.t + (j(0) * 0.0001) as f64,
                        ..*p
                    }
                })
                .collect();
            big.add_stroke(brush, pts);
        }
        copy += 1;
    }
    big.mark_saved();
    Some(big)
}

/// Deterministic noise in -1..1 (a hash, so runs and devices get the same sketch).
fn noise(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}
