//! The `.airdraw` file format (little-endian), docs/ARCHITECTURE.md §3c and §3h.
//!
//! Format 4 (current):
//!
//! ```text
//! "ADRW"  u16 format version  u16 flags (0)
//! u32 thumbnail length (0 = none), PNG bytes
//! sections (airdraw-pack): META (required, TOML: app version, layers, brush snapshots, Look,
//!   drawn guide), STRK (required, stroke data, see `strokes.rs`); each compressed by its
//!   own codec and CRC-checked
//! ```
//!
//! Sections carry their own checksums, so there's no whole-file checksum: an optional
//! section added later can be damaged and dropped without losing the sketch. A required
//! section this build doesn't know means the file needs a newer app.
//!
//! Formats 1–3 (still read; `to_bytes_format3` writes 3 for tests and benchmarks):
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
//! The thumbnail sits at a fixed place right after the 8-byte header in every format since
//! 2, so OS thumbnail extensions (Quick Look, Windows shell, freedesktop) can read just the
//! first bytes of a file: check "ADRW" and version >= 2, read the u32 at offset 8, then that
//! many PNG bytes. Format 1 (no thumbnail) and 2 (no guide) files open with the default plane.

use std::fmt;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use std::sync::OnceLock;

use airdraw_brush::{BrushDef, InputPoint};
use airdraw_guide::{Guide, Sweep};
use airdraw_look::{LookDef, LookLibrary};
pub use airdraw_pack::Level;
use airdraw_pack::{CodecId, Codecs, PackError, ReadLimits, Section, SectionId, WriteOptions};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::document::{BrushRef, Document, Layer, LayerId, Op, Stroke, StrokeId};

pub const EXTENSION: &str = "airdraw";
const MAGIC: &[u8; 4] = b"ADRW";
/// Newest file format this build reads and the one it writes.
pub const FORMAT_VERSION: u16 = 4;
/// First format made of `airdraw-pack` sections.
const SECTIONS_SINCE: u16 = 4;
const META: SectionId = SectionId(*b"META");
const STRK: SectionId = SectionId(*b"STRK");
const HIST: SectionId = SectionId(*b"HIST");
/// Codec for new files. Reading accepts every codec in [`codecs`], whatever this says.
const WRITE_CODEC: CodecId = CodecId::ZSTD;
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
    /// Readable header, but it needs something this build lacks (a section or codec).
    Unsupported(String),
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
            DocError::Unsupported(why) => write!(f, "{why}"),
            DocError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DocError {}

impl From<PackError> for DocError {
    fn from(e: PackError) -> Self {
        match e {
            PackError::Truncated => DocError::Truncated,
            PackError::UnknownCodec(_) => DocError::Unsupported(format!("file {e}")),
            e => DocError::Corrupt(e.to_string()),
        }
    }
}

/// The codecs files are read and written with. Swapping or adding a codec happens here.
fn codecs() -> &'static Codecs {
    static CODECS: OnceLock<Codecs> = OnceLock::new();
    CODECS.get_or_init(|| {
        let mut c = Codecs::new();
        c.register(std::sync::Arc::new(airdraw_codec_zstd::Zstd));
        c
    })
}

/// How a document is written.
#[derive(Clone, Copy, Debug)]
pub struct SaveOptions {
    /// `Fast` for saves and autosaves, `Max` for copies meant to be shared.
    pub level: Level,
    /// Include undo/redo history (within its size cap). Off for "Save a Copy Without
    /// History": deleted strokes are then gone from the file.
    pub history: bool,
    /// Give the file a new content id: for copies, which become sketches of their own, so they
    /// don't share per-device state (camera, tool) with the original. The content hash, which
    /// covers only the artwork, still matches.
    pub new_content_id: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { level: Level::Fast, history: true, new_content_id: false }
    }
}

#[derive(Serialize, Deserialize)]
struct Meta {
    app_version: String,
    /// The sketch's permanent random id (format 4+; older files get one derived from their
    /// bytes when opened).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_id: Option<String>,
    /// BLAKE3 of the artwork only (see [`Document::content_hash`]); written for tools, which
    /// can compare sketches by reading just this section. Format 4+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    layers: Vec<Layer>,
    #[serde(default)]
    brushes: Vec<BrushDef>,
    /// Look id. Optional, so older apps (which ignore unknown keys) still open these files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    look: Option<String>,
    /// The Look's full definition (format 4+), so it renders the same on a device without it.
    /// Read leniently: a Look this build can't parse (e.g. new fields from a newer app) is
    /// dropped, and the sketch falls back to the built-in Look with that id.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    look_def: Option<LookDef>,
    /// The drawn guide. Format 3 (a format 2 app would drop it and save the file guide-less).
    /// Read leniently: a guide kind from a newer app is dropped, not the sketch.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    guide: Option<GuideDef>,
}

/// Optional data from a newer app must not stop a sketch from opening: if the value doesn't
/// parse as `T`, it reads as absent. (A missing key is handled by `#[serde(default)]`.)
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<Option<T>, D::Error> {
    let value = Option::<toml::Value>::deserialize(d)?;
    Ok(value.and_then(|v| v.try_into().ok()))
}

/// What [`Document::content_hash`] covers: the artwork, not who saved it or when.
#[derive(Serialize)]
struct Content<'a> {
    layers: &'a [Layer],
    brushes: &'a [BrushDef],
    look_def: Option<&'a LookDef>,
    guide: Option<GuideDef>,
}

/// A [`Guide`] as stored in the metadata (`[guide] kind = "sweep"`, …). Kept here so the
/// guide crate stays free of file-format concerns.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum GuideDef {
    Plane { origin: [f32; 3], normal: [f32; 3] },
    Sweep { near: f32, far: f32, profile: Vec<[f32; 3]>, dirs: Vec<[f32; 3]> },
}

impl GuideDef {
    pub(crate) fn new(guide: &Guide) -> Self {
        let arr = |v: &[Vec3]| v.iter().map(|p| p.to_array()).collect();
        match guide {
            Guide::Plane { origin, normal } => GuideDef::Plane { origin: origin.to_array(), normal: normal.to_array() },
            Guide::Sweep(s) => GuideDef::Sweep { near: s.near, far: s.far, profile: arr(&s.profile), dirs: arr(&s.dirs) },
        }
    }

    pub(crate) fn into_guide(self) -> Result<Guide, DocError> {
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
    /// The document as a current-format file, with [`SaveOptions::default`].
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_with(SaveOptions::default())
    }

    pub fn to_bytes_with(&self, opts: SaveOptions) -> Vec<u8> {
        let (undo_from, redo_from) =
            if opts.history { crate::history::kept(&self.undo, &self.redo, &self.strokes) } else { (self.undo.len(), self.redo.len()) };
        let (undo, redo) = (&self.undo[undo_from..], &self.redo[redo_from..]);
        let (brushes, remap, live) = self.used_brushes(undo.iter().chain(redo).flat_map(Op::brushes));
        let file_brush = |r: BrushRef| remap[r.0 as usize].unwrap();
        let strokes = crate::strokes::encode(&self.strokes, |s| file_brush(s.brush));
        let hash = content_hash(&self.content_toml(&brushes[..live]), &strokes);
        let content_id = if opts.new_content_id { crate::document::new_content_id() } else { self.content_id.clone() };
        let meta = self.meta_toml(brushes, Some(hash), content_id);
        let mut w = self.file_head(FORMAT_VERSION);
        let mut sections = vec![Section { id: META, required: true, data: meta.into_bytes() }, Section { id: STRK, required: true, data: strokes }];
        if !undo.is_empty() || !redo.is_empty() {
            sections.push(Section { id: HIST, required: false, data: crate::history::encode(undo, redo, &self.strokes, &file_brush) });
        }
        airdraw_pack::write(&mut w, &sections, codecs(), WriteOptions { codec: WRITE_CODEC, level: opts.level })
            .expect("the write codec is registered and in-memory compression doesn't fail");
        w
    }

    /// The document as a format 3 file (uncompressed, whole-file checksum). For tests and
    /// size comparisons; the app writes the current format.
    #[doc(hidden)]
    pub fn to_bytes_format3(&self) -> Vec<u8> {
        let (brushes, remap, _) = self.used_brushes(std::iter::empty());
        let meta = self.meta_toml(brushes, None, self.content_id.clone());
        let mut w = self.file_head(3);
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

    /// BLAKE3 (hex) of the artwork: layers, brush snapshots, Look, drawn guide and strokes,
    /// as they're saved. Not the thumbnail (it depends on the GPU), the app version, the
    /// content id, history, or compression: two files with the same hash hold the same model.
    pub fn content_hash(&self) -> String {
        let (brushes, remap, _) = self.used_brushes(std::iter::empty());
        content_hash(&self.content_toml(&brushes), &crate::strokes::encode(&self.strokes, |s| remap[s.brush.0 as usize].unwrap()))
    }

    /// The brush snapshots the strokes use, then those only `history` uses, each in
    /// first-use order; each snapshot's new index; and how many the strokes use. Live
    /// strokes' numbering never depends on history, so neither does the content hash.
    fn used_brushes(&self, history: impl Iterator<Item = BrushRef>) -> (Vec<BrushDef>, Vec<Option<u32>>, usize) {
        fn number(r: BrushRef, remap: &mut [Option<u32>], order: &mut Vec<usize>) {
            if remap[r.0 as usize].is_none() {
                remap[r.0 as usize] = Some(order.len() as u32);
                order.push(r.0 as usize);
            }
        }
        let mut remap: Vec<Option<u32>> = vec![None; self.brushes.len()];
        let mut order = Vec::new();
        for s in &self.strokes {
            number(s.brush, &mut remap, &mut order);
        }
        let live = order.len();
        for r in history {
            number(r, &mut remap, &mut order);
        }
        (order.iter().map(|&i| self.brushes[i].clone()).collect(), remap, live)
    }

    fn content_toml(&self, brushes: &[BrushDef]) -> String {
        let content = Content { layers: &self.layers, brushes, look_def: self.look.as_ref(), guide: self.guide().map(|g| GuideDef::new(g)) };
        toml::to_string(&content).expect("document content always serializes")
    }

    fn meta_toml(&self, brushes: Vec<BrushDef>, content_hash: Option<String>, content_id: String) -> String {
        let meta = Meta {
            app_version: env!("CARGO_PKG_VERSION").into(),
            content_id: Some(content_id),
            content_hash,
            layers: self.layers.clone(),
            brushes,
            look: self.look_id().map(str::to_string),
            look_def: self.look.clone(),
            guide: self.guide().map(|g| GuideDef::new(g)),
        };
        toml::to_string(&meta).expect("document metadata always serializes")
    }

    /// Magic, version, flags and the thumbnail: the part every format since 2 shares.
    fn file_head(&self, version: u16) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(MAGIC);
        w.extend_from_slice(&version.to_le_bytes());
        w.extend_from_slice(&0u16.to_le_bytes());
        let thumb = self.thumbnail.as_deref().unwrap_or_default();
        put_u32(&mut w, thumb.len() as u32);
        w.extend_from_slice(thumb);
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
        let (meta, strokes, thumbnail, history) = if version >= SECTIONS_SINCE { read_sections(bytes)? } else { read_format3(bytes, version)? };
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
        // The embedded Look if this build can render it, else the built-in one with that id
        // (a Look from a newer app), else the default.
        let look = meta.look_def.filter(|l| l.validate().is_ok()).or_else(|| LookLibrary::with_builtins().get(meta.look.as_deref()?).cloned());
        let content_id = meta.content_id.filter(|id| valid_content_id(id)).unwrap_or_else(|| blake3::hash(bytes).to_hex()[..32].to_string());
        let brush_count = meta.brushes.len();
        let mut doc = Document::from_parts(meta.layers, strokes, meta.brushes, thumbnail, look, guide, content_id);
        // History is optional: if it doesn't fit the strokes, the sketch opens without it.
        if let Some(Ok((undo, redo))) = history.map(|h| crate::history::decode(&h, &doc.strokes, brush_count)) {
            doc.set_history(undo, redo);
        }
        Ok(doc)
    }

    /// Writes to a temporary file first and renames it over `path`, so a crash or full disk
    /// mid-save never destroys the previous version.
    pub fn save(&self, path: &Path) -> Result<(), DocError> {
        self.save_with(path, SaveOptions::default())
    }

    /// [`Document::save`] with options, e.g. a copy without history.
    pub fn save_with(&self, path: &Path, opts: SaveOptions) -> Result<(), DocError> {
        let io = |e: std::io::Error| DocError::Io(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
        let mut f = std::fs::File::create(&tmp).map_err(io)?;
        f.write_all(&self.to_bytes_with(opts)).and_then(|_| f.sync_all()).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    pub fn load(path: &Path) -> Result<Self, DocError> {
        let bytes = std::fs::read(path).map_err(|e| DocError::Io(format!("{}: {e}", path.display())))?;
        Self::from_bytes(&bytes)
    }
}

/// See [`Document::content_hash`]. Versioned, so a future change to what's hashed can't
/// collide with today's hashes.
fn content_hash(content_toml: &str, strokes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"airdraw content v1\0");
    h.update(&(content_toml.len() as u64).to_le_bytes());
    h.update(content_toml.as_bytes());
    h.update(strokes);
    h.finalize().to_hex().to_string()
}

fn valid_content_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

type Thumbnail = Option<Arc<[u8]>>;
/// Metadata, strokes, thumbnail and the raw `HIST` section, if any.
type Parts = (Meta, Vec<Stroke>, Thumbnail, Option<Vec<u8>>);

/// Reads the thumbnail at offset 8 (format 2+); returns it and where the rest starts.
fn read_head_thumbnail(bytes: &[u8]) -> Result<(Thumbnail, usize), DocError> {
    let mut r = Reader { b: bytes, pos: 8 };
    let len = r.u32()? as usize;
    if len > MAX_THUMBNAIL_BYTES {
        return Err(DocError::Corrupt(format!("thumbnail of {len} bytes")));
    }
    let png = r.take(len)?;
    Ok(((len > 0).then(|| Arc::from(png)), r.pos))
}

fn read_sections(bytes: &[u8]) -> Result<Parts, DocError> {
    let (thumbnail, start) = read_head_thumbnail(bytes)?;
    let (decoded, used) = airdraw_pack::read(&bytes[start..], codecs(), ReadLimits::default())?;
    if start + used != bytes.len() {
        return Err(DocError::Corrupt(format!("{} unexpected bytes after the sections", bytes.len() - start - used)));
    }
    if let Some(s) = decoded.sections.iter().find(|s| s.required && s.id != META && s.id != STRK) {
        return Err(DocError::Unsupported(format!(
            "file uses a feature from a newer AirDraw (section {}). Update the app.",
            s.id
        )));
    }
    let section = |id: SectionId| decoded.get(id).map(|s| &s.data[..]).ok_or_else(|| DocError::Corrupt(format!("no {id} section")));
    let meta = std::str::from_utf8(section(META)?).map_err(|_| DocError::Corrupt("metadata is not UTF-8".into()))?;
    let meta: Meta = toml::from_str(meta).map_err(|e| DocError::Corrupt(format!("metadata: {e}")))?;
    let strokes = crate::strokes::decode(section(STRK)?)?;
    let history = decoded.get(HIST).map(|s| s.data.clone());
    Ok((meta, strokes, thumbnail, history))
}

fn read_format3(bytes: &[u8], version: u16) -> Result<Parts, DocError> {
    if bytes.len() < 12 {
        return Err(DocError::Truncated);
    }
    let (body, sum) = bytes.split_at(bytes.len() - 4);
    let (thumbnail, start) = if version >= THUMBNAIL_SINCE { read_head_thumbnail(body)? } else { (None, 8) };
    let mut r = Reader { b: body, pos: start };
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
    Ok((meta, strokes, thumbnail, None))
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
        assert!(!loaded.is_dirty());
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
        assert_eq!(Document::from_bytes(&d.to_bytes_format3()).unwrap().strokes(), d.strokes());
    }

    /// The sections of a format 4 file.
    fn sections(bytes: &[u8]) -> airdraw_pack::Decoded {
        let thumb = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        airdraw_pack::read(&bytes[12 + thumb..], codecs(), ReadLimits::default()).unwrap().0
    }

    /// `doc` as a format 4 file made of `extra` sections after the usual ones.
    fn with_sections(doc: &Document, extra: &[Section]) -> Vec<u8> {
        let mut all = sections(&doc.to_bytes()).sections;
        all.extend_from_slice(extra);
        let mut w = doc.file_head(FORMAT_VERSION);
        airdraw_pack::write(&mut w, &all, codecs(), WriteOptions { codec: WRITE_CODEC, level: Level::Fast }).unwrap();
        w
    }

    #[test]
    fn format4_is_compressed_sections_and_round_trips_at_every_level() {
        let d = sample_doc();
        for level in [Level::Fast, Level::Default, Level::Max] {
            let bytes = d.to_bytes_with(SaveOptions { level, ..Default::default() });
            assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 4);
            let back = Document::from_bytes(&bytes).unwrap();
            assert_eq!(back.strokes(), d.strokes());
            assert_eq!(back.layers(), d.layers());
        }
        let ids: Vec<_> = sections(&d.to_bytes()).sections.iter().map(|s| (s.id, s.required)).collect();
        assert_eq!(ids, [(META, true), (STRK, true), (HIST, false)]);
        assert!(d.to_bytes().len() < d.to_bytes_format3().len());
    }

    #[test]
    fn saving_is_deterministic() {
        let d = sample_doc();
        assert_eq!(d.to_bytes(), d.to_bytes());
        // Open and save again without edits: the same bytes.
        let bytes = d.to_bytes();
        assert_eq!(Document::from_bytes(&bytes).unwrap().to_bytes(), bytes);
    }

    #[test]
    fn unknown_sections_need_a_newer_app_only_when_required() {
        let d = sample_doc();
        let optional = with_sections(&d, &[Section { id: SectionId(*b"NEWX"), required: false, data: b"later".to_vec() }]);
        assert_eq!(Document::from_bytes(&optional).unwrap().strokes(), d.strokes());
        let required = with_sections(&d, &[Section { id: SectionId(*b"NEWX"), required: true, data: b"later".to_vec() }]);
        assert!(matches!(Document::from_bytes(&required), Err(DocError::Unsupported(m)) if m.contains("NEWX")));
    }

    #[test]
    fn format4_damage_is_reported() {
        let bytes = sample_doc().to_bytes();
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(Document::from_bytes(&longer), Err(DocError::Corrupt(_))));
        for cut in [9, 13, bytes.len() / 2, bytes.len() - 1] {
            assert_eq!(Document::from_bytes(&bytes[..cut]).unwrap_err(), DocError::Truncated, "cut at {cut}");
        }
        // A section written with a codec this build doesn't have.
        let mut unknown_codec = bytes.clone();
        unknown_codec[12 + 4 + 5] = 99;
        assert!(matches!(Document::from_bytes(&unknown_codec), Err(DocError::Unsupported(_))));
    }

    /// Size and speed table (docs/ARCHITECTURE.md §3h). Samples: the fixtures, plus every
    /// `.airdraw` in `$AIRDRAW_BENCH_DIR`. Run with
    /// `cargo test --release -p airdraw-doc file_format_benchmark -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn file_format_benchmark() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut dirs = vec![fixtures];
        dirs.extend(std::env::var_os("AIRDRAW_BENCH_DIR").map(std::path::PathBuf::from));
        let mut samples = Vec::new();
        for dir in dirs {
            let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| Some(e.ok()?.path())).collect();
            files.sort();
            for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == EXTENSION)) {
                samples.push((f.file_name().unwrap().to_string_lossy().into_owned(), Document::load(f).unwrap()));
            }
        }
        println!("{}", crate::bench::file_format(&samples, 1_000_000));
    }

    /// Draw three strokes, re-brush one, delete one, set a guide, undo twice: undo and redo
    /// history with a deleted stroke, a history-only brush and a guide.
    fn edited_doc() -> Document {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let ribbon = d.intern_brush(&brush("com.airdraw.ribbon"));
        let (a, _) = d.add_stroke(ink, points(30, 0.0));
        let (b, _) = d.add_stroke(ink, points(20, 1.0));
        d.add_stroke(ribbon, points(25, 2.0));
        d.set_brush(&[a], ribbon);
        d.remove_strokes(&[b]);
        d.set_guide(Some(sweep()));
        d.undo();
        d.undo();
        d
    }

    /// Ids, brushes and points of the strokes, plus the guide: what undo/redo must reproduce.
    /// Brushes by definition, since saving renumbers snapshots.
    type State = (Vec<(StrokeId, String, Vec<InputPoint>)>, Option<Guide>);

    fn state(d: &Document) -> State {
        let strokes = d.strokes().iter().map(|s| (s.id, d.brush_def(s.brush).unwrap().id.clone(), s.points.to_vec())).collect();
        (strokes, d.guide().map(|g| (**g).clone()))
    }

    #[test]
    fn history_survives_save_and_open() {
        let d = edited_doc();
        assert!(d.can_undo() && d.can_redo());
        let mut back = Document::from_bytes(&d.to_bytes()).unwrap();
        assert!(!back.is_dirty() && back.can_undo() && back.can_redo(), "opening isn't an edit");
        let mut orig = d.clone();
        assert_eq!(state(&back), state(&orig));
        // Every step back and forward matches the original session.
        while orig.can_redo() {
            orig.redo();
            back.redo();
            assert_eq!(state(&back), state(&orig));
        }
        while orig.can_undo() {
            orig.undo();
            back.undo();
            assert_eq!(state(&back), state(&orig));
        }
        assert!(!back.can_undo() && back.strokes().is_empty());
    }

    #[test]
    fn deleted_strokes_come_back_after_reopening() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let (id, _) = d.add_stroke(ink, points(40, 0.0));
        let deleted = d.strokes()[0].clone();
        d.clear();
        let mut back = Document::from_bytes(&d.to_bytes()).unwrap();
        assert!(back.strokes().is_empty());
        back.undo();
        assert_eq!(back.stroke(id), Some(&deleted));
        // New strokes never reuse an id the history knows.
        let mut fresh = Document::from_bytes(&d.to_bytes()).unwrap();
        let ink = fresh.intern_brush(&brush("com.airdraw.ink"));
        let (new_id, _) = fresh.add_stroke(ink, points(3, 0.0));
        assert_ne!(new_id, id);
    }

    #[test]
    fn history_is_its_own_optional_section() {
        let d = edited_doc();
        let with = d.to_bytes();
        let ids: Vec<_> = sections(&with).sections.iter().map(|s| (s.id, s.required)).collect();
        assert_eq!(ids, [(META, true), (STRK, true), (HIST, false)]);
        // Without history: no section, nothing to undo, deleted strokes gone from the file.
        let without = d.to_bytes_with(SaveOptions { history: false, ..Default::default() });
        assert!(sections(&without).get(HIST).is_none());
        let back = Document::from_bytes(&without).unwrap();
        assert!(!back.can_undo() && !back.can_redo());
        assert_eq!(state(&back), state(&d));
        // A sketch without edits has no section either.
        assert!(sections(&Document::from_bytes(&without).unwrap().to_bytes()).get(HIST).is_none());
        // History doesn't change the content hash; it does keep brushes only it uses.
        assert_eq!(Document::from_bytes(&with).unwrap().content_hash(), back.content_hash());
        let meta = |b: &[u8]| String::from_utf8(sections(b).get(META).unwrap().data.clone()).unwrap();
        assert!(meta(&with).matches("[[brushes]]").count() >= meta(&without).matches("[[brushes]]").count());
    }

    /// Splits, moves and duplicates, with some of it undone: every kind of `replace`.
    fn replaced_doc() -> Document {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let ribbon = d.intern_brush(&brush("com.airdraw.ribbon"));
        let (a, _) = d.add_stroke(ink, points(30, 0.0));
        let (b, _) = d.add_stroke(ribbon, points(20, 1.0));
        let p = d.stroke(a).unwrap().points.to_vec();
        let (_, pieces) = d.replace_strokes(vec![(a, vec![p[..10].to_vec(), p[14..].to_vec()])]);
        let moved: Vec<InputPoint> = d.stroke(b).unwrap().points.iter().map(|q| InputPoint { pos: q.pos + Vec3::X, ..*q }).collect();
        d.replace_strokes(vec![(b, vec![moved])]);
        d.duplicate_strokes(&pieces, Vec3::ZERO);
        d.set_brush(&pieces, ribbon);
        d.undo();
        d.undo();
        d
    }

    #[test]
    fn replace_survives_save_and_open() {
        let d = replaced_doc();
        let bytes = d.to_bytes();
        let text = String::from_utf8_lossy(&sections(&bytes).get(HIST).unwrap().data).into_owned();
        assert!(text.contains("op = \"replace\"") && text.contains("removed = [") && text.contains("added = ["));
        let mut back = Document::from_bytes(&bytes).unwrap();
        let mut orig = d.clone();
        assert_eq!(state(&back), state(&orig));
        while orig.can_redo() {
            orig.redo();
            back.redo();
            assert_eq!(state(&back), state(&orig));
        }
        while orig.can_undo() {
            orig.undo();
            back.undo();
            assert_eq!(state(&back), state(&orig));
        }
        assert!(back.strokes().is_empty());
        assert_eq!(Document::from_bytes(&bytes).unwrap().to_bytes(), bytes, "deterministic");
    }

    #[test]
    fn unknown_or_bad_replace_drops_only_the_history() {
        let d = replaced_doc();
        let mut all = sections(&d.to_bytes()).sections;
        let hist = all.pop().unwrap();
        let len = u32::from_le_bytes(hist.data[..4].try_into().unwrap()) as usize;
        let text = String::from_utf8(hist.data[4..4 + len].to_vec()).unwrap();
        let reopen = |t: String| {
            let mut data = (t.len() as u32).to_le_bytes().to_vec();
            data.extend_from_slice(t.as_bytes());
            data.extend_from_slice(&hist.data[4 + len..]);
            let mut w = d.file_head(FORMAT_VERSION);
            let mut secs = all.clone();
            secs.push(Section { id: HIST, required: false, data });
            airdraw_pack::write(&mut w, &secs, codecs(), WriteOptions { codec: WRITE_CODEC, level: Level::Fast }).unwrap();
            Document::from_bytes(&w).unwrap()
        };
        // An op this build doesn't know is what an older build sees in a `replace`.
        let unknown = text.replace("op = \"replace\"", "op = \"teleport\"");
        let misplaced = text.replacen("added = [[0,", "added = [[5,", 1);
        assert_ne!(misplaced, text);
        // The same in the redo steps (the duplicate goes on top, at 3 and 4; 5 is past the end).
        let (undo_part, redo_part) = text.split_at(text.find("[[redo]]").unwrap());
        let misplaced_redo = format!("{undo_part}{}", redo_part.replacen("added = [[3, 6, 0], [4,", "added = [[5, 6, 0], [6,", 1));
        assert_ne!(misplaced_redo, text);
        for bad in [unknown, misplaced, misplaced_redo] {
            let back = reopen(bad);
            assert_eq!(state(&back), state(&d), "the sketch itself still opens");
            assert!(!back.can_undo() && !back.can_redo());
        }
        assert!(reopen(text).can_undo(), "the untouched section still works");
    }

    #[test]
    fn history_cap_counts_what_replace_takes_out() {
        // A 1.6 MB stroke cut down to a short piece: the sketch was that big a step ago, so
        // the whole history fits the budget.
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        d.add_stroke(ink, points(300, 0.0));
        let (big, _) = d.add_stroke(ink, points(40_000, 1.0));
        d.replace_strokes(vec![(big, vec![points(2, 1.0)])]);
        let mut back = Document::from_bytes(&d.to_bytes()).unwrap();
        for _ in 0..3 {
            assert!(back.can_undo());
            back.undo();
        }
        assert!(!back.can_undo() && back.strokes().is_empty());
    }

    #[test]
    fn history_cap_counts_what_a_redone_replace_adds() {
        // Redoing a duplicate of a 1.6 MB stroke makes the sketch 3.2 MB: both redo steps fit.
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        d.add_stroke(ink, points(300, 0.0));
        let (big, _) = d.add_stroke(ink, points(40_000, 1.0));
        d.duplicate_strokes(&[big], Vec3::ZERO);
        d.undo();
        d.undo();
        let mut back = Document::from_bytes(&d.to_bytes()).unwrap();
        back.redo();
        back.redo();
        assert_eq!(back.strokes().len(), 3);
    }

    #[test]
    fn saving_with_history_is_deterministic() {
        let bytes = edited_doc().to_bytes();
        assert_eq!(Document::from_bytes(&bytes).unwrap().to_bytes(), bytes);
    }

    #[test]
    fn bad_history_opens_the_sketch_without_it() {
        let d = edited_doc();
        let mut all = sections(&d.to_bytes()).sections;
        let hist = all.pop().unwrap();
        let reopen = |data: Vec<u8>| {
            let mut w = d.file_head(FORMAT_VERSION);
            let mut secs = all.clone();
            secs.push(Section { id: HIST, required: false, data });
            airdraw_pack::write(&mut w, &secs, codecs(), WriteOptions { codec: WRITE_CODEC, level: Level::Fast }).unwrap();
            Document::from_bytes(&w).unwrap()
        };
        // Garbage, a truncated pool, and ops that don't fit the strokes.
        let text = |h: &[u8]| String::from_utf8_lossy(&h[4..4 + u32::from_le_bytes(h[..4].try_into().unwrap()) as usize]).into_owned();
        let with_text = |t: String| {
            let mut out = (t.len() as u32).to_le_bytes().to_vec();
            out.extend_from_slice(t.as_bytes());
            out.extend_from_slice(&hist.data[4 + text(&hist.data).len()..]);
            out
        };
        let wrong_index = with_text(text(&hist.data).replacen("index = 0", "index = 7", 1).replacen("index = 1", "index = 7", 1).replacen("index = 2", "index = 7", 1));
        let missing_stroke = with_text(text(&hist.data).replace("stroke = [", "stroke = [9999, 0] # ["));
        let missing_brush = with_text(text(&hist.data).replace("stroke = [3, 0]", "stroke = [3, 77]"));
        for bad in [b"garbage".to_vec(), hist.data[..hist.data.len() - 3].to_vec(), wrong_index, missing_stroke, missing_brush] {
            let back = reopen(bad);
            assert_eq!(state(&back), state(&d), "the sketch itself still opens");
            assert!(!back.can_undo() && !back.can_redo());
        }
        assert!(reopen(hist.data.clone()).can_undo(), "the untouched section still works");
    }

    #[test]
    fn undoing_clear_survives_reopening_a_large_sketch() {
        // Far over 1 MB, then cleared: the sketch is empty, but its peak size sets the budget.
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        for i in 0..8 {
            d.add_stroke(ink, points(10_000, i as f32));
        }
        let before = state(&d);
        d.clear();
        let mut back = Document::from_bytes(&d.to_bytes()).unwrap();
        back.undo();
        assert_eq!(state(&back), before);
    }

    #[test]
    fn history_is_capped_oldest_first() {
        // Draw a ~400 KB stroke and delete it, six times: the sketch never holds more than
        // one, so the budget (1 MB) keeps only the newest cycles.
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let mut ids = Vec::new();
        for i in 0..6 {
            let (id, _) = d.add_stroke(ink, points(10_000, i as f32));
            d.remove_strokes(&[id]);
            ids.push(id);
        }
        let mut back = Document::from_bytes(&d.to_bytes()).unwrap();
        let (mut undone, mut seen) = (0, std::collections::HashSet::new());
        while back.can_undo() {
            back.undo();
            undone += 1;
            seen.extend(back.strokes().iter().map(|s| s.id));
        }
        assert!((2..12).contains(&undone), "kept {undone} of 12 steps");
        // The kept steps are the newest: undoing brings back the last-deleted strokes, never
        // the first ones.
        assert!(seen.contains(&ids[5]) && !seen.contains(&ids[0]));
    }

    #[test]
    fn real_ipad_sketch_upgrades_losslessly() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/format2_ipad.airdraw");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2, "fixture must stay format 2");
        let d = Document::from_bytes(&bytes).unwrap();
        assert!(!d.strokes().is_empty() && d.thumbnail().is_some());
        let upgraded = d.to_bytes();
        let back = Document::from_bytes(&upgraded).unwrap();
        assert_eq!(back.strokes(), d.strokes());
        assert_eq!(back.thumbnail(), d.thumbnail());
        assert!(upgraded.len() < bytes.len());
    }

    #[test]
    fn unused_brush_snapshots_are_dropped() {
        let mut d = sample_doc();
        d.intern_brush(&brush("com.airdraw.dots"));
        let first = d.strokes()[0].id;
        d.remove_strokes(&[first]);
        let loaded = Document::from_bytes(&d.to_bytes_with(SaveOptions { history: false, ..Default::default() })).unwrap();
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
        // Without history, so the last bytes are stroke data (damage in history only drops it).
        let bytes = sample_doc().to_bytes_with(SaveOptions { history: false, ..Default::default() });
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

    fn clay() -> LookDef {
        LookLibrary::with_builtins().get("com.airdraw.clay").unwrap().clone()
    }

    #[test]
    fn look_round_trips_and_is_optional() {
        let mut d = Document::new();
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().look(), None);
        d.set_look(Some(clay()));
        assert!(d.is_dirty() && !d.can_undo(), "saved, but not undoable");
        let back = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(back.look(), Some(&clay()));
        // Metadata from before Looks (no key) still parses.
        let meta: Meta = toml::from_str("app_version = \"0.1.0\"\nlayers = []\n").unwrap();
        assert_eq!(meta.look, None);
    }

    /// A newer app may add fields to a Look or new guide kinds. This build can't read them,
    /// but the sketch must still open: the Look falls back to the built-in one with its id,
    /// and the guide is dropped.
    #[test]
    fn newer_look_fields_and_guide_kinds_dont_block_opening() {
        let newer = |meta: &str| meta.replacen("[look_def]\n", "[look_def]\ngrain = 0.5\n", 1);
        let mut d = Document::new();
        d.set_look(Some(clay()));
        let meta = d.meta_toml(Vec::new(), None, d.content_id.clone());
        assert!(newer(&meta).contains("grain = 0.5"));
        let parsed: Meta = toml::from_str(&newer(&meta)).expect("META still parses");
        assert!(parsed.look_def.is_none(), "the unreadable Look is dropped");
        assert_eq!(parsed.look.as_deref(), Some("com.airdraw.clay"), "its id remains for the fallback");

        let with_guide = format!("{meta}\n[guide]\nkind = \"torus\"\nradius = 2.0\n");
        let parsed: Meta = toml::from_str(&with_guide).expect("META still parses");
        assert!(parsed.guide.is_none());

        // End to end through the file reader: a format 3 file (uncompressed META) with both.
        let patched = format!("{}\n[guide]\nkind = \"torus\"\nradius = 2.0\n", newer(&meta));
        let mut w = d.file_head(3); // magic, version, flags, (empty) thumbnail
        put_u32(&mut w, patched.len() as u32);
        w.extend_from_slice(patched.as_bytes());
        put_u32(&mut w, 0); // no strokes
        let sum = adler32(&w);
        put_u32(&mut w, sum);
        let back = Document::from_bytes(&w).expect("the sketch opens");
        assert_eq!(back.look().map(|l| l.id.as_str()), Some("com.airdraw.clay"));
        assert!(back.guide().is_none());
    }

    #[test]
    fn embedded_look_renders_without_the_library() {
        // A custom Look this device doesn't have travels with the sketch.
        let mut custom = clay();
        custom.id = "studio.custom.chalk".into();
        custom.params.insert("wrap".into(), 0.7);
        let mut d = Document::new();
        d.set_look(Some(custom.clone()));
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().look(), Some(&custom));
        // One this build can't render (a shader from a newer app) falls back to the built-in
        // Look with the same id, else to the default.
        let mut newer = clay();
        newer.shader = "hologram".into();
        d.set_look(Some(newer));
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().look(), Some(&clay()));
        let mut unknown = custom;
        unknown.shader = "hologram".into();
        d.set_look(Some(unknown));
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().look(), None);
    }

    #[test]
    fn format3_look_id_still_resolves() {
        let mut d = Document::new();
        d.set_look(Some(clay()));
        let mut bytes = d.to_bytes_format3();
        // Format 3 files had only the id: strip the definition from the metadata.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert!(text.contains("[look_def]"));
        let (body, _) = bytes.split_at(bytes.len() - 4);
        let meta_len = u32::from_le_bytes(body[12..16].try_into().unwrap()) as usize;
        let meta = std::str::from_utf8(&body[16..16 + meta_len]).unwrap();
        let mut meta: Meta = toml::from_str(meta).unwrap();
        meta.look_def = None;
        let meta = toml::to_string(&meta).unwrap();
        let mut w = body[..12].to_vec();
        put_u32(&mut w, meta.len() as u32);
        w.extend_from_slice(meta.as_bytes());
        w.extend_from_slice(&body[16 + meta_len..]);
        let sum = adler32(&w);
        put_u32(&mut w, sum);
        bytes = w;
        assert_eq!(Document::from_bytes(&bytes).unwrap().look(), Some(&clay()));
    }

    /// A copy is a sketch of its own: new id (so it doesn't share per-device state with the
    /// original), same artwork hash. Plain saves keep the id.
    #[test]
    fn copies_get_a_new_content_id_but_the_same_content_hash() {
        let d = sample_doc();
        let copy = Document::from_bytes(&d.to_bytes_with(SaveOptions { history: false, new_content_id: true, ..Default::default() })).unwrap();
        assert_ne!(copy.content_id(), d.content_id());
        assert!(valid_content_id(copy.content_id()));
        assert_eq!(copy.content_hash(), d.content_hash());
        assert_eq!(Document::from_bytes(&d.to_bytes()).unwrap().content_id(), d.content_id());
    }

    #[test]
    fn content_id_is_kept_and_old_files_get_a_stable_one() {
        let d = sample_doc();
        assert_eq!(d.content_id().len(), 32);
        assert_ne!(d.content_id(), Document::new().content_id());
        let back = Document::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(back.content_id(), d.content_id());
        // Edits don't change it.
        let mut edited = back.clone();
        edited.remove_strokes(&[edited.strokes()[0].id]);
        assert_eq!(Document::from_bytes(&edited.to_bytes()).unwrap().content_id(), d.content_id());
        // A format 1 file has none: it gets one from its bytes, the same every time.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/format1_smoke.airdraw");
        let old = std::fs::read(path).unwrap();
        let (a, b) = (Document::from_bytes(&old).unwrap(), Document::from_bytes(&old).unwrap());
        assert_eq!(a.content_id(), b.content_id());
        assert!(valid_content_id(a.content_id()));
    }

    #[test]
    fn content_hash_is_the_artwork_only() {
        let d = sample_doc();
        let hash = d.content_hash();
        let bytes = d.to_bytes();
        let meta = String::from_utf8(sections(&bytes).get(META).unwrap().data.clone()).unwrap();
        assert!(meta.contains(&format!("content_hash = \"{hash}\"")), "stored for tools");
        // Same model: another id, a thumbnail, another compression level, a reload.
        let mut copy = Document::from_bytes(&bytes).unwrap();
        copy.content_id = "another-sketch-id".into();
        copy.set_thumbnail(Some(png())).unwrap();
        assert_eq!(copy.content_hash(), hash);
        assert_eq!(Document::from_bytes(&d.to_bytes_with(SaveOptions { level: Level::Max, ..Default::default() })).unwrap().content_hash(), hash);
        // Any change to the artwork changes it.
        let mut moved = d.clone();
        moved.remove_strokes(&[moved.strokes()[0].id]);
        assert_ne!(moved.content_hash(), hash);
        let mut relooked = d.clone();
        relooked.set_look(Some(clay()));
        assert_ne!(relooked.content_hash(), hash);
        let mut guided = d.clone();
        guided.set_guide(Some(sweep()));
        assert_ne!(guided.content_hash(), hash);
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
            assert!(!back.is_dirty(), "loading the guide is not an edit");
        }
        assert!(Document::from_bytes(&sample_doc().to_bytes()).unwrap().guide().is_none());
    }

    #[test]
    fn guide_is_readable_toml() {
        let mut d = Document::new();
        d.set_guide(Some(sweep()));
        let meta = String::from_utf8(sections(&d.to_bytes()).get(META).unwrap().data.clone()).unwrap();
        assert!(meta.contains("[guide]\nkind = \"sweep\""), "{meta}");
    }

    #[test]
    fn format2_file_opens_with_the_default_plane() {
        let mut bytes = sample_doc().to_bytes_format3();
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
