//! The `.airdraw` file format (little-endian):
//!
//! ```text
//! "ADRW"  u16 format version  u16 flags (0)
//! u32 thumbnail length (0 = none), PNG bytes           (format 2+)
//! u32 meta length, meta: UTF-8 TOML (app version, layers, brush snapshots, Look,
//!   drawn guide (format 3+))
//! u32 stroke count, then per stroke:
//!   u64 id  u32 layer  u32 brush  u32 point count n  u8 flags (1 = one shared normal, 2 = tilt)
//!   positions n×3 f32 | normals (1 or n)×3 f32 | pressure n f32 | [tilt n f32, NaN = none] | time n f64
//! u32 Adler-32 of everything above
//! ```
//!
//! Point data is binary (a sketch can hold millions of points); everything else is TOML so
//! it stays readable and reuses the brush file format. Values round-trip exactly.
//!
//! The thumbnail sits at a fixed place right after the 8-byte header so OS thumbnail
//! extensions (Quick Look, Windows shell, freedesktop) can read just the first bytes of a
//! file: check "ADRW" and version >= 2, read the u32 at offset 8, then that many PNG bytes.
//! Format 1 files (no thumbnail) and format 2 files (no guide) are still read; they open with
//! the default plane.

use std::fmt;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use airdraw_brush::{BrushDef, InputPoint};
use airdraw_guide::{Guide, Sweep};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::document::{BrushRef, Document, Layer, LayerId, Stroke, StrokeId};

pub const EXTENSION: &str = "airdraw";
const MAGIC: &[u8; 4] = b"ADRW";
/// Newest file format this build reads and the one it writes.
pub const FORMAT_VERSION: u16 = 3;
/// First format with an embedded thumbnail.
const THUMBNAIL_SINCE: u16 = 2;
/// Sanity cap; a 512×512 preview is ~50–150 KB.
pub const MAX_THUMBNAIL_BYTES: usize = 8 << 20;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

const SHARED_NORMAL: u8 = 1;
const HAS_TILT: u8 = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum DocError {
    NotAirDraw,
    NewerVersion(u16),
    Truncated,
    Corrupt(String),
    Io(String),
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocError::NotAirDraw => write!(f, "not an AirDraw file"),
            DocError::NewerVersion(v) => {
                write!(f, "file was saved by a newer AirDraw (format {v}, this app reads up to {FORMAT_VERSION}). Update the app.")
            }
            DocError::Truncated => write!(f, "file is incomplete (truncated)"),
            DocError::Corrupt(why) => write!(f, "file is damaged: {why}"),
            DocError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DocError {}

#[derive(Serialize, Deserialize)]
struct Meta {
    app_version: String,
    layers: Vec<Layer>,
    #[serde(default)]
    brushes: Vec<BrushDef>,
    /// Look id. Optional, so older apps (which ignore unknown keys) still open these files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    look: Option<String>,
    /// The drawn guide. Format 3 (a format 2 app would drop it and save the file guide-less).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guide: Option<GuideDef>,
    /// Guide opacity, when set (applies to preset planes too, so it's not part of `guide`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guide_opacity: Option<f32>,
}

/// A [`Guide`] as stored in the metadata (`[guide] kind = "sweep"`, …). Kept here so the
/// guide crate stays free of file-format concerns.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum GuideDef {
    Plane { origin: [f32; 3], normal: [f32; 3] },
    Sweep { near: f32, far: f32, profile: Vec<[f32; 3]>, dirs: Vec<[f32; 3]> },
}

impl GuideDef {
    fn new(guide: &Guide) -> Self {
        let arr = |v: &[Vec3]| v.iter().map(|p| p.to_array()).collect();
        match guide {
            Guide::Plane { origin, normal } => GuideDef::Plane { origin: origin.to_array(), normal: normal.to_array() },
            Guide::Sweep(s) => GuideDef::Sweep { near: s.near, far: s.far, profile: arr(&s.profile), dirs: arr(&s.dirs) },
        }
    }

    fn into_guide(self) -> Result<Guide, DocError> {
        let bad = |why: &str| DocError::Corrupt(format!("guide: {why}"));
        let finite = |v: &[[f32; 3]]| v.iter().flatten().all(|x| x.is_finite());
        match self {
            GuideDef::Plane { origin, normal } => {
                if !finite(&[origin, normal]) || Vec3::from(normal).length_squared() < 1e-12 {
                    return Err(bad("invalid plane"));
                }
                Ok(Guide::Plane { origin: origin.into(), normal: normal.into() })
            }
            GuideDef::Sweep { near, far, profile, dirs } => {
                if profile.len() < 2 || profile.len() != dirs.len() {
                    return Err(bad(&format!("{} profile points, {} directions", profile.len(), dirs.len())));
                }
                if !finite(&profile) || !finite(&dirs) || !near.is_finite() || !far.is_finite() {
                    return Err(bad("non-finite values"));
                }
                let v = |a: Vec<[f32; 3]>| a.into_iter().map(Vec3::from).collect();
                Ok(Guide::Sweep(Sweep { profile: v(profile), dirs: v(dirs), near, far }))
            }
        }
    }
}

impl Document {
    pub fn to_bytes(&self) -> Vec<u8> {
        // Keep only snapshots that strokes use, renumbered in first-use order.
        let mut remap: Vec<Option<u32>> = vec![None; self.brushes.len()];
        let mut brushes = Vec::new();
        for s in &self.strokes {
            let slot = &mut remap[s.brush.0 as usize];
            if slot.is_none() {
                *slot = Some(brushes.len() as u32);
                brushes.push(self.brushes[s.brush.0 as usize].clone());
            }
        }
        let meta = Meta {
            app_version: env!("CARGO_PKG_VERSION").into(),
            layers: self.layers.clone(),
            brushes,
            look: self.look.clone(),
            guide: self.guide().map(|g| GuideDef::new(g)),
            guide_opacity: self.guide_opacity,
        };
        let meta = toml::to_string(&meta).expect("document metadata always serializes");

        let mut w = Vec::new();
        w.extend_from_slice(MAGIC);
        w.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        w.extend_from_slice(&0u16.to_le_bytes());
        let thumb = self.thumbnail.as_deref().unwrap_or_default();
        put_u32(&mut w, thumb.len() as u32);
        w.extend_from_slice(thumb);
        put_u32(&mut w, meta.len() as u32);
        w.extend_from_slice(meta.as_bytes());
        put_u32(&mut w, self.strokes.len() as u32);
        for s in &self.strokes {
            write_stroke(&mut w, s, remap[s.brush.0 as usize].unwrap());
        }
        let sum = adler32(&w);
        put_u32(&mut w, sum);
        w
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DocError> {
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err(if bytes.starts_with(&MAGIC[..bytes.len().min(4)]) { DocError::Truncated } else { DocError::NotAirDraw });
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version > FORMAT_VERSION {
            return Err(DocError::NewerVersion(version));
        }
        if bytes.len() < 12 {
            return Err(DocError::Truncated);
        }
        let (body, sum) = bytes.split_at(bytes.len() - 4);
        let mut r = Reader { b: body, pos: 8 };
        let thumbnail = if version >= THUMBNAIL_SINCE {
            let len = r.u32()? as usize;
            if len > MAX_THUMBNAIL_BYTES {
                return Err(DocError::Corrupt(format!("thumbnail of {len} bytes")));
            }
            let png = r.take(len)?;
            (len > 0).then(|| Arc::from(png))
        } else {
            None
        };
        let meta_len = r.u32()? as usize;
        let meta = std::str::from_utf8(r.take(meta_len)?).map_err(|_| DocError::Corrupt("metadata is not UTF-8".into()))?;
        let meta: Meta = toml::from_str(meta).map_err(|e| DocError::Corrupt(format!("metadata: {e}")))?;
        let count = r.u32()? as usize;
        let mut strokes = Vec::with_capacity(count.min(1 << 16));
        for _ in 0..count {
            strokes.push(read_stroke(&mut r)?);
        }
        // Checked after parsing so truncation is reported as such rather than as a bad checksum.
        if r.pos != body.len() || u32::from_le_bytes(sum.try_into().unwrap()) != adler32(body) {
            return Err(DocError::Corrupt("checksum mismatch".into()));
        }
        for s in &strokes {
            if s.brush.0 as usize >= meta.brushes.len() {
                return Err(DocError::Corrupt(format!("stroke {} uses missing brush {}", s.id.0, s.brush.0)));
            }
            if !meta.layers.iter().any(|l| l.id == s.layer) {
                return Err(DocError::Corrupt(format!("stroke {} is on missing layer {}", s.id.0, s.layer.0)));
            }
        }
        if meta.layers.is_empty() {
            return Err(DocError::Corrupt("no layers".into()));
        }
        let guide = meta.guide.map(GuideDef::into_guide).transpose()?;
        Ok(Document::from_parts(meta.layers, strokes, meta.brushes, thumbnail, meta.look, guide, meta.guide_opacity))
    }

    /// Writes to a temporary file first and renames it over `path`, so a crash or full disk
    /// mid-save never destroys the previous version.
    pub fn save(&self, path: &Path) -> Result<(), DocError> {
        let io = |e: std::io::Error| DocError::Io(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        f.write_all(&self.to_bytes()).and_then(|_| f.sync_all()).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    pub fn load(path: &Path) -> Result<Self, DocError> {
        let bytes = std::fs::read(path).map_err(|e| DocError::Io(format!("{}: {e}", path.display())))?;
        Self::from_bytes(&bytes)
    }
}

/// Reads only the embedded PNG preview from the start of a file, without loading strokes.
/// `Ok(None)` for files without one (format 1, or saved before a preview existed).
pub fn read_thumbnail(path: &Path) -> Result<Option<Vec<u8>>, DocError> {
    use std::io::Read;
    let io = |e: std::io::Error| DocError::Io(format!("{}: {e}", path.display()));
    let mut f = std::fs::File::open(path).map_err(io)?;
    let mut head = [0u8; 12];
    let n = f.read(&mut head).map_err(io)?;
    if n < 8 || &head[..4] != MAGIC {
        return Err(if n < 4 && head[..n] == MAGIC[..n] { DocError::Truncated } else { DocError::NotAirDraw });
    }
    let version = u16::from_le_bytes([head[4], head[5]]);
    if version < THUMBNAIL_SINCE {
        return Ok(None);
    }
    if n < 12 {
        f.read_exact(&mut head[n..]).map_err(|_| DocError::Truncated)?;
    }
    let len = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    if len == 0 {
        return Ok(None);
    }
    if len > MAX_THUMBNAIL_BYTES {
        return Err(DocError::Corrupt(format!("thumbnail of {len} bytes")));
    }
    let mut png = vec![0; len];
    f.read_exact(&mut png).map_err(|_| DocError::Truncated)?;
    check_thumbnail(&png)?;
    Ok(Some(png))
}

pub(crate) fn check_thumbnail(png: &[u8]) -> Result<(), DocError> {
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(DocError::Corrupt("thumbnail is not a PNG".into()));
    }
    if png.len() > MAX_THUMBNAIL_BYTES {
        return Err(DocError::Corrupt(format!("thumbnail of {} bytes", png.len())));
    }
    Ok(())
}

fn write_stroke(w: &mut Vec<u8>, s: &Stroke, brush: u32) {
    let pts = &s.points;
    let shared_normal = pts.windows(2).all(|p| p[0].normal == p[1].normal);
    let has_tilt = pts.iter().any(|p| p.tilt.is_some());
    w.extend_from_slice(&s.id.0.to_le_bytes());
    put_u32(w, s.layer.0);
    put_u32(w, brush);
    put_u32(w, pts.len() as u32);
    w.push(if shared_normal { SHARED_NORMAL } else { 0 } | if has_tilt { HAS_TILT } else { 0 });
    for p in pts.iter() {
        put_vec3(w, p.pos);
    }
    for p in pts.iter().take(if shared_normal { 1 } else { pts.len() }) {
        put_vec3(w, p.normal);
    }
    for p in pts.iter() {
        put_f32(w, p.pressure);
    }
    if has_tilt {
        for p in pts.iter() {
            put_f32(w, p.tilt.unwrap_or(f32::NAN));
        }
    }
    for p in pts.iter() {
        w.extend_from_slice(&p.t.to_le_bytes());
    }
}

fn read_stroke(r: &mut Reader) -> Result<Stroke, DocError> {
    let id = StrokeId(r.u64()?);
    let layer = LayerId(r.u32()?);
    let brush = BrushRef(r.u32()?);
    let n = r.u32()? as usize;
    let flags = r.u8()?;
    // Smallest possible point: position 12 + pressure 4 + time 8 bytes (shared normal, no
    // tilt). Reject counts the file can't hold before allocating.
    const MIN_POINT_BYTES: usize = 24;
    if n == 0 || n > r.remaining() / MIN_POINT_BYTES {
        return Err(if n == 0 { DocError::Corrupt(format!("stroke {} has no points", id.0)) } else { DocError::Truncated });
    }
    let positions = (0..n).map(|_| r.vec3()).collect::<Result<Vec<_>, _>>()?;
    let normal_count = if flags & SHARED_NORMAL != 0 { 1 } else { n };
    let normals = (0..normal_count).map(|_| r.vec3()).collect::<Result<Vec<_>, _>>()?;
    let pressure = (0..n).map(|_| r.f32()).collect::<Result<Vec<_>, _>>()?;
    let tilt = if flags & HAS_TILT != 0 {
        (0..n).map(|_| r.f32().map(|v| (!v.is_nan()).then_some(v))).collect::<Result<Vec<_>, _>>()?
    } else {
        vec![None; n]
    };
    let mut points = Vec::with_capacity(n);
    for i in 0..n {
        points.push(InputPoint {
            pos: positions[i],
            normal: normals[if normal_count == 1 { 0 } else { i }],
            pressure: pressure[i],
            tilt: tilt[i],
            t: r.f64()?,
        });
    }
    Ok(Stroke { id, layer, brush, points: Arc::from(points) })
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DocError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len()).ok_or(DocError::Truncated)?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn remaining(&self) -> usize {
        self.b.len() - self.pos
    }
    fn u8(&mut self) -> Result<u8, DocError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, DocError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, DocError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, DocError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, DocError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn vec3(&mut self) -> Result<Vec3, DocError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
}

fn put_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(w: &mut Vec<u8>, v: f32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_vec3(w: &mut Vec<u8>, v: Vec3) {
    put_f32(w, v.x);
    put_f32(w, v.y);
    put_f32(w, v.z);
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest block that can't overflow u32 before reducing.
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::tests::{brush, points};

    fn sample_doc() -> Document {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let ribbon = d.intern_brush(&brush("com.airdraw.ribbon"));
        d.add_stroke(ink, points(50, 0.0));
        let mut curved = points(20, 1.0);
        for (i, p) in curved.iter_mut().enumerate() {
            p.normal = Vec3::new(0.0, (i as f32).cos(), (i as f32).sin());
        }
        d.add_stroke(ribbon, curved);
        d
    }

    #[test]
    fn roundtrip_is_exact() {
        let d = sample_doc();
        let loaded = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(loaded.strokes(), d.strokes());
        assert_eq!(loaded.layers(), d.layers());
        assert_eq!(loaded.brush_def(BrushRef(1)), d.brush_def(BrushRef(1)));
        assert!(!loaded.is_dirty() && !loaded.can_undo());
    }

    #[test]
    fn last_stroke_with_shared_normal_and_no_tilt_loads() {
        // Smallest point encoding at the very end of the file (regression: was rejected).
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let mut pts = points(40, 0.0);
        pts.iter_mut().for_each(|p| p.tilt = None);
        d.add_stroke(ink, pts);
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().strokes(), d.strokes());
    }

    #[test]
    fn unused_brush_snapshots_are_dropped() {
        let mut d = sample_doc();
        d.intern_brush(&brush("com.airdraw.dots"));
        let first = d.strokes()[0].id;
        d.remove_strokes(&[first]);
        let loaded = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(loaded.brushes.len(), 1);
        assert_eq!(loaded.brush_def(loaded.strokes()[0].brush).unwrap().id, "com.airdraw.ribbon");
    }

    #[test]
    fn new_strokes_after_load_get_fresh_ids() {
        let d = sample_doc();
        let mut loaded = Document::from_bytes(&d.to_bytes()).unwrap();
        let b = loaded.intern_brush(&brush("com.airdraw.ink"));
        let (id, _) = loaded.add_stroke(b, points(3, 0.0));
        assert!(d.strokes().iter().all(|s| s.id != id));
    }

    #[test]
    fn bad_files_give_clear_errors() {
        let bytes = sample_doc().to_bytes();
        assert_eq!(Document::from_bytes(b"PK\x03\x04 a zip file").unwrap_err(), DocError::NotAirDraw);
        assert_eq!(Document::from_bytes(&bytes[..bytes.len() / 2]).unwrap_err(), DocError::Truncated);
        let mut newer = bytes.clone();
        newer[4] = 9;
        assert_eq!(Document::from_bytes(&newer).unwrap_err(), DocError::NewerVersion(9));
        let mut flipped = bytes.clone();
        let last_point_byte = bytes.len() - 10;
        flipped[last_point_byte] ^= 0xFF;
        assert!(matches!(Document::from_bytes(&flipped), Err(DocError::Corrupt(_))));
    }

    /// Smallest valid PNG-looking payload for tests (signature + filler).
    fn png() -> Vec<u8> {
        let mut p = PNG_SIGNATURE.to_vec();
        p.extend_from_slice(b"pretend image data");
        p
    }

    #[test]
    fn thumbnail_roundtrips_and_is_not_an_edit() {
        let mut d = sample_doc();
        d.mark_saved();
        d.set_thumbnail(Some(png())).unwrap();
        assert!(!d.is_dirty() && d.can_undo());
        let loaded = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(loaded.thumbnail(), Some(&png()[..]));
        assert_eq!(loaded.strokes(), d.strokes());
        // Re-saving a loaded document keeps its preview.
        assert_eq!(Document::from_bytes(&loaded.to_bytes()).unwrap().thumbnail(), Some(&png()[..]));
        assert!(d.set_thumbnail(Some(b"GIF89a".to_vec())).is_err());
        assert!(Document::from_bytes(&Document::new().to_bytes()).unwrap().thumbnail().is_none());
    }

    #[test]
    fn thumbnail_sits_right_after_the_header() {
        let mut d = sample_doc();
        d.set_thumbnail(Some(png())).unwrap();
        let b = d.to_bytes();
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), FORMAT_VERSION);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, png().len());
        assert_eq!(&b[12..12 + png().len()], &png()[..]);
    }

    #[test]
    fn read_thumbnail_reads_only_the_head() {
        let dir = std::env::temp_dir().join(format!("airdraw-thumb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = sample_doc();
        d.set_thumbnail(Some(png())).unwrap();
        let bytes = d.to_bytes();
        // Cut the file right after the PNG: strokes are never read.
        let head = dir.join("head.airdraw");
        std::fs::write(&head, &bytes[..12 + png().len()]).unwrap();
        assert_eq!(read_thumbnail(&head).unwrap(), Some(png()));
        let none = dir.join("none.airdraw");
        std::fs::write(&none, sample_doc().to_bytes()).unwrap();
        assert_eq!(read_thumbnail(&none).unwrap(), None);
        let cut = dir.join("cut.airdraw");
        std::fs::write(&cut, &bytes[..20]).unwrap();
        assert_eq!(read_thumbnail(&cut).unwrap_err(), DocError::Truncated);
        let other = dir.join("other.airdraw");
        std::fs::write(&other, b"PK\x03\x04 zip").unwrap();
        assert_eq!(read_thumbnail(&other).unwrap_err(), DocError::NotAirDraw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format1_file_from_the_app_still_opens() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/format1_smoke.airdraw");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 1, "fixture must stay format 1");
        let d = Document::from_bytes(&bytes).unwrap();
        assert_eq!(d.strokes().len(), 5);
        assert!(d.thumbnail().is_none());
        assert_eq!(read_thumbnail(&path).unwrap(), None);
        // Saving upgrades it to the current format losslessly.
        let upgraded = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(upgraded.strokes(), d.strokes());
    }

    #[test]
    fn look_round_trips_and_is_optional() {
        let mut d = Document::new();
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().look(), None);
        d.set_look(Some("com.airdraw.clay".into()));
        assert!(d.is_dirty() && !d.can_undo(), "a view setting: dirty, but not undoable");
        let back = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(back.look(), Some("com.airdraw.clay"));
        // Metadata from before Looks (no key) still parses.
        let meta: Meta = toml::from_str("app_version = \"0.1.0\"\nlayers = []\n").unwrap();
        assert_eq!(meta.look, None);
    }

    fn sweep() -> Guide {
        let profile = (0..5).map(|i| Vec3::new(i as f32 * 0.1, (i as f32).sin() * 0.3, 0.7)).collect();
        let dirs = (0..5).map(|i| Vec3::new(0.1 * i as f32, 0.0, -1.0).normalize()).collect();
        Guide::Sweep(Sweep { profile, dirs, near: 1.5, far: 3.0 })
    }

    #[test]
    fn guide_round_trips_exactly() {
        for guide in [sweep(), Guide::Plane { origin: Vec3::new(0.1, 0.2, 0.3), normal: Vec3::Y }] {
            let mut d = sample_doc();
            d.set_guide(Some(guide.clone()));
            let back = Document::from_bytes(&d.to_bytes()).unwrap();
            assert_eq!(back.guide().map(|g| &**g), Some(&guide));
            assert_eq!(back.strokes(), d.strokes());
            assert!(!back.can_undo(), "loading the guide is not an edit");
        }
        assert!(Document::from_bytes(&sample_doc().to_bytes()).unwrap().guide().is_none());
    }

    #[test]
    fn guide_opacity_round_trips_and_is_clamped() {
        use crate::{DEFAULT_GUIDE_OPACITY, MAX_GUIDE_OPACITY};
        let mut d = Document::new();
        assert_eq!(d.guide_opacity(), DEFAULT_GUIDE_OPACITY);
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().guide_opacity(), DEFAULT_GUIDE_OPACITY);
        d.set_guide_opacity(0.35);
        assert!(d.is_dirty() && !d.can_undo(), "a view setting: dirty, but not undoable");
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().guide_opacity(), 0.35);
        d.set_guide_opacity(5.0);
        assert_eq!(d.guide_opacity(), MAX_GUIDE_OPACITY);
        d.set_guide_opacity(-1.0);
        assert_eq!(d.guide_opacity(), 0.0);
        d.set_guide_opacity(f32::NAN);
        assert_eq!(d.guide_opacity(), DEFAULT_GUIDE_OPACITY);
        // Out-of-range values in a file are clamped, not rejected.
        let meta: Meta = toml::from_str("app_version = \"0.1.0\"\nlayers = []\nguide_opacity = 3.0\n").unwrap();
        let loaded = Document::from_parts(meta.layers, Vec::new(), Vec::new(), None, None, None, meta.guide_opacity);
        assert_eq!(loaded.guide_opacity(), MAX_GUIDE_OPACITY);
    }

    #[test]
    fn guide_is_readable_toml() {
        let mut d = Document::new();
        d.set_guide(Some(sweep()));
        let bytes = d.to_bytes();
        let meta_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let meta = std::str::from_utf8(&bytes[16..16 + meta_len]).unwrap();
        assert!(meta.contains("[guide]\nkind = \"sweep\""), "{meta}");
    }

    #[test]
    fn format2_file_opens_with_the_default_plane() {
        let mut bytes = sample_doc().to_bytes();
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        let n = bytes.len() - 4;
        let sum = adler32(&bytes[..n]);
        bytes[n..].copy_from_slice(&sum.to_le_bytes());
        let d = Document::from_bytes(&bytes).unwrap();
        assert!(d.guide().is_none());
        assert_eq!(d.strokes(), sample_doc().strokes());
    }

    #[test]
    fn damaged_guide_is_reported() {
        let parse = |toml: &str| -> Result<Guide, DocError> {
            let meta: Meta = toml::from_str(&format!("app_version = \"0.1.0\"\nlayers = []\n{toml}")).unwrap();
            meta.guide.unwrap().into_guide()
        };
        let mismatched = "[guide]\nkind = \"sweep\"\nnear = 1.0\nfar = 1.0\nprofile = [[0,0,0],[1,0,0]]\ndirs = [[0,0,1]]";
        assert!(matches!(parse(mismatched), Err(DocError::Corrupt(_))));
        let flat = "[guide]\nkind = \"plane\"\norigin = [0,0,0]\nnormal = [0,0,0]";
        assert!(matches!(parse(flat), Err(DocError::Corrupt(_))));
        let nan = "[guide]\nkind = \"plane\"\norigin = [nan,0,0]\nnormal = [0,1,0]";
        assert!(matches!(parse(nan), Err(DocError::Corrupt(_))));
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("airdraw-doc-test-{}", std::process::id()));
        let path = dir.join("sketch.airdraw");
        let d = sample_doc();
        d.save(&path).unwrap();
        d.save(&path).unwrap(); // overwrite goes through the temp file too
        assert_eq!(Document::load(&path).unwrap().strokes(), d.strokes());
        assert!(!path.with_extension("airdraw.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adler32_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
