//! The section container inside `.airdraw` files (docs/ARCHITECTURE.md §3h), with
//! swappable compression codecs.
//!
//! ```text
//! u32 section count, then per section:
//!   [u8;4] id  u8 flags (1 = required)  u8 codec
//!   u64 raw length  u64 stored length  u32 CRC-32 of the raw bytes  stored bytes
//! ```
//!
//! This crate knows nothing about sketches (the caller decides what each section means) and
//! links no compression library: codecs implement [`Codec`] in their own crates and are
//! registered in [`Codecs`]. Every section names the codec that wrote it, so changing the
//! default codec never makes old files unreadable, and one file may mix codecs.
//!
//! Reading is strict about required sections (any problem fails the read) and lenient about
//! optional ones (a damaged or undecodable optional section is dropped and reported).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Identifies a compression algorithm in section headers. Ids are permanent: never reuse
/// one for a different algorithm or an incompatible variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodecId(pub u8);

impl CodecId {
    /// No compression; always available.
    pub const STORED: CodecId = CodecId(0);
    /// Zstandard (`airdraw-codec-zstd`).
    pub const ZSTD: CodecId = CodecId(1);
}

/// How hard to compress. Each codec maps these to its own settings; the level changes only
/// the size and speed, never the decoded bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Autosave: fast.
    Fast,
    Default,
    /// "Save a copy": smallest.
    Max,
}

pub trait Codec: Send + Sync {
    fn id(&self) -> CodecId;
    fn name(&self) -> &str;
    /// Must be deterministic: the same input and level always give the same output.
    fn compress(&self, raw: &[u8], level: Level) -> Result<Vec<u8>, PackError>;
    /// `raw_len` is the length recorded in the section header; producing any other length is
    /// an error.
    fn decompress(&self, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackError>;
}

/// The built-in "no compression" codec.
pub struct Stored;

impl Codec for Stored {
    fn id(&self) -> CodecId {
        CodecId::STORED
    }

    fn name(&self) -> &str {
        "stored"
    }

    fn compress(&self, raw: &[u8], _level: Level) -> Result<Vec<u8>, PackError> {
        Ok(raw.to_vec())
    }

    fn decompress(&self, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackError> {
        if stored.len() != raw_len {
            return Err(PackError::Codec(format!("stored section is {} bytes, header says {raw_len}", stored.len())));
        }
        Ok(stored.to_vec())
    }
}

/// The codecs this build can read and write, by id. Starts with [`Stored`]; the app
/// registers the ones it links in.
#[derive(Clone)]
pub struct Codecs {
    by_id: HashMap<CodecId, Arc<dyn Codec>>,
}

impl Default for Codecs {
    fn default() -> Self {
        Self::new()
    }
}

impl Codecs {
    pub fn new() -> Self {
        let mut by_id: HashMap<CodecId, Arc<dyn Codec>> = HashMap::new();
        by_id.insert(CodecId::STORED, Arc::new(Stored));
        Self { by_id }
    }

    /// Adds a codec. Panics if its id is taken: two algorithms sharing an id would make
    /// files ambiguous, which is a programming error.
    pub fn register(&mut self, codec: Arc<dyn Codec>) {
        let id = codec.id();
        if let Some(old) = self.by_id.get(&id) {
            panic!("codec id {} is already used by '{}'", id.0, old.name());
        }
        self.by_id.insert(id, codec);
    }

    pub fn get(&self, id: CodecId) -> Option<&Arc<dyn Codec>> {
        self.by_id.get(&id)
    }
}

/// A four-byte section tag, e.g. `META`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SectionId(pub [u8; 4]);

impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

/// A decoded section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub id: SectionId,
    /// A reader that doesn't understand a required section must refuse the file.
    pub required: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct WriteOptions {
    pub codec: CodecId,
    pub level: Level,
}

/// Limits that keep a damaged or hostile file from making the reader allocate wildly.
#[derive(Clone, Copy, Debug)]
pub struct ReadLimits {
    /// Largest decoded size of one section.
    pub max_section_bytes: u64,
    pub max_sections: u32,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self { max_section_bytes: 1 << 31, max_sections: 1024 }
    }
}

/// An optional section that was skipped while reading, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub id: SectionId,
    pub reason: PackError,
}

#[derive(Debug, Default)]
pub struct Decoded {
    /// In file order.
    pub sections: Vec<Section>,
    pub dropped: Vec<Dropped>,
}

impl Decoded {
    pub fn get(&self, id: SectionId) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PackError {
    Truncated,
    Corrupt(String),
    /// The section was written with a codec this build doesn't have.
    UnknownCodec(CodecId),
    /// A codec failed (bad compressed data, wrong length).
    Codec(String),
    TooLarge(u64),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::Truncated => write!(f, "data is incomplete (truncated)"),
            PackError::Corrupt(why) => write!(f, "{why}"),
            PackError::UnknownCodec(id) => write!(f, "needs compression codec {}, which this app doesn't have. Update the app.", id.0),
            PackError::Codec(why) => write!(f, "decompression failed: {why}"),
            PackError::TooLarge(n) => write!(f, "section of {n} bytes is too large"),
        }
    }
}

impl std::error::Error for PackError {}

const REQUIRED: u8 = 1;
/// id + flags + codec + raw length + stored length + CRC.
const ENTRY_HEADER: usize = 4 + 1 + 1 + 8 + 8 + 4;

/// Appends `sections` to `out`, compressed with `opts.codec`. A section that doesn't get
/// smaller is stored instead, so compression never costs space. Deterministic: the same
/// sections, codec and level always give the same bytes.
pub fn write(out: &mut Vec<u8>, sections: &[Section], codecs: &Codecs, opts: WriteOptions) -> Result<(), PackError> {
    let codec = codecs.get(opts.codec).ok_or(PackError::UnknownCodec(opts.codec))?;
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    for s in sections {
        let packed = if opts.codec == CodecId::STORED { None } else { Some(codec.compress(&s.data, opts.level)?) };
        let (codec_id, stored) = match &packed {
            Some(p) if p.len() < s.data.len() => (opts.codec, &p[..]),
            _ => (CodecId::STORED, &s.data[..]),
        };
        out.extend_from_slice(&s.id.0);
        out.push(if s.required { REQUIRED } else { 0 });
        out.push(codec_id.0);
        out.extend_from_slice(&(s.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&(stored.len() as u64).to_le_bytes());
        out.extend_from_slice(&crc32fast::hash(&s.data).to_le_bytes());
        out.extend_from_slice(stored);
    }
    Ok(())
}

/// Reads sections written by [`write`] from the start of `bytes`; returns them and how many
/// bytes they took. Required sections must decode or the read fails; optional ones that
/// don't are listed in [`Decoded::dropped`]. Framing errors (truncation, impossible
/// lengths, duplicate ids) always fail, since nothing after them can be trusted.
pub fn read(bytes: &[u8], codecs: &Codecs, limits: ReadLimits) -> Result<(Decoded, usize), PackError> {
    let mut pos = 0;
    let take = |pos: &mut usize, n: usize| -> Result<&[u8], PackError> {
        let end = pos.checked_add(n).filter(|&e| e <= bytes.len()).ok_or(PackError::Truncated)?;
        let s = &bytes[*pos..end];
        *pos = end;
        Ok(s)
    };
    let count = u32::from_le_bytes(take(&mut pos, 4)?.try_into().unwrap());
    if count > limits.max_sections {
        return Err(PackError::Corrupt(format!("{count} sections")));
    }
    let mut out = Decoded::default();
    for _ in 0..count {
        let h = take(&mut pos, ENTRY_HEADER)?;
        let id = SectionId(h[0..4].try_into().unwrap());
        let required = h[4] & REQUIRED != 0;
        let codec_id = CodecId(h[5]);
        let raw_len = u64::from_le_bytes(h[6..14].try_into().unwrap());
        let stored_len = u64::from_le_bytes(h[14..22].try_into().unwrap());
        let crc = u32::from_le_bytes(h[22..26].try_into().unwrap());
        let stored = take(&mut pos, usize::try_from(stored_len).map_err(|_| PackError::Truncated)?)?;
        if out.sections.iter().any(|s| s.id == id) || out.dropped.iter().any(|d| d.id == id) {
            return Err(PackError::Corrupt(format!("section {id} appears twice")));
        }
        match decode_one(codecs, codec_id, stored, raw_len, crc, limits) {
            Ok(data) => out.sections.push(Section { id, required, data }),
            Err(e) if required => return Err(e),
            Err(reason) => out.dropped.push(Dropped { id, reason }),
        }
    }
    Ok((out, pos))
}

fn decode_one(codecs: &Codecs, codec_id: CodecId, stored: &[u8], raw_len: u64, crc: u32, limits: ReadLimits) -> Result<Vec<u8>, PackError> {
    if raw_len > limits.max_section_bytes {
        return Err(PackError::TooLarge(raw_len));
    }
    let codec = codecs.get(codec_id).ok_or(PackError::UnknownCodec(codec_id))?;
    let data = codec.decompress(stored, raw_len as usize)?;
    if data.len() as u64 != raw_len {
        return Err(PackError::Codec(format!("{} produced {} bytes, expected {raw_len}", codec.name(), data.len())));
    }
    if crc32fast::hash(&data) != crc {
        return Err(PackError::Corrupt("checksum mismatch".into()));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy run-length codec, so the container is tested without any real compressor.
    struct Rle;

    impl Codec for Rle {
        fn id(&self) -> CodecId {
            CodecId(200)
        }

        fn name(&self) -> &str {
            "test-rle"
        }

        fn compress(&self, raw: &[u8], _level: Level) -> Result<Vec<u8>, PackError> {
            let mut out = Vec::new();
            for run in raw.chunk_by(|a, b| a == b) {
                for part in run.chunks(255) {
                    out.extend_from_slice(&[part.len() as u8, part[0]]);
                }
            }
            Ok(out)
        }

        fn decompress(&self, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackError> {
            if !stored.len().is_multiple_of(2) {
                return Err(PackError::Codec("odd length".into()));
            }
            let mut out = Vec::with_capacity(raw_len);
            for pair in stored.chunks(2) {
                out.extend(std::iter::repeat_n(pair[1], pair[0] as usize));
                if out.len() > raw_len {
                    return Err(PackError::Codec("too long".into()));
                }
            }
            Ok(out)
        }
    }

    fn codecs() -> Codecs {
        let mut c = Codecs::new();
        c.register(Arc::new(Rle));
        c
    }

    const META: SectionId = SectionId(*b"META");
    const HIST: SectionId = SectionId(*b"HIST");
    const RLE: WriteOptions = WriteOptions { codec: CodecId(200), level: Level::Default };

    fn sample() -> Vec<Section> {
        vec![
            Section { id: META, required: true, data: b"app_version = \"0.1.0\"".to_vec() },
            Section { id: SectionId(*b"STRK"), required: true, data: vec![0; 5000] },
            Section { id: HIST, required: false, data: vec![7; 300] },
            Section { id: SectionId(*b"EMPT"), required: false, data: Vec::new() },
        ]
    }

    fn packed(sections: &[Section], opts: WriteOptions) -> Vec<u8> {
        let mut out = Vec::new();
        write(&mut out, sections, &codecs(), opts).unwrap();
        out
    }

    /// Byte offset of section `index`'s header in `packed(sample(), RLE)`.
    fn header_at(bytes: &[u8], index: usize) -> usize {
        let mut pos = 4;
        for _ in 0..index {
            pos += ENTRY_HEADER + u64::from_le_bytes(bytes[pos + 14..pos + 22].try_into().unwrap()) as usize;
        }
        pos
    }

    #[test]
    fn round_trips_with_every_codec() {
        for opts in [RLE, WriteOptions { codec: CodecId::STORED, level: Level::Fast }] {
            let bytes = packed(&sample(), opts);
            let (d, used) = read(&bytes, &codecs(), ReadLimits::default()).unwrap();
            assert_eq!(d.sections, sample());
            assert!(d.dropped.is_empty());
            assert_eq!(used, bytes.len());
        }
        assert!(packed(&sample(), RLE).len() < packed(&sample(), WriteOptions { codec: CodecId::STORED, level: Level::Fast }).len());
    }

    #[test]
    fn incompressible_sections_are_stored() {
        let noisy: Vec<u8> = (0..=255).collect();
        let bytes = packed(&[Section { id: META, required: true, data: noisy.clone() }], RLE);
        assert_eq!(bytes[4 + 5], CodecId::STORED.0);
        assert_eq!(read(&bytes, &codecs(), ReadLimits::default()).unwrap().0.sections[0].data, noisy);
    }

    #[test]
    fn writing_is_deterministic() {
        assert_eq!(packed(&sample(), RLE), packed(&sample(), RLE));
    }

    #[test]
    fn reading_stops_at_the_end_of_the_sections() {
        let mut bytes = packed(&sample(), RLE);
        let len = bytes.len();
        bytes.extend_from_slice(b"checksum");
        assert_eq!(read(&bytes, &codecs(), ReadLimits::default()).unwrap().1, len);
    }

    #[test]
    fn unknown_codec_fails_required_and_drops_optional() {
        let bytes = packed(&sample(), RLE);
        // A reader without the RLE codec.
        assert_eq!(read(&bytes, &Codecs::new(), ReadLimits::default()).unwrap_err(), PackError::UnknownCodec(CodecId(200)));
        let only_optional = packed(&sample()[2..], RLE);
        let (d, _) = read(&only_optional, &Codecs::new(), ReadLimits::default()).unwrap();
        assert_eq!(d.dropped, vec![Dropped { id: HIST, reason: PackError::UnknownCodec(CodecId(200)) }]);
        assert_eq!(d.sections.len(), 1, "the empty section is stored, so it still reads");
    }

    #[test]
    fn damaged_optional_section_is_dropped_but_required_fails() {
        let bytes = packed(&sample(), RLE);
        let mut bad_hist = bytes.clone();
        let h = header_at(&bytes, 2);
        bad_hist[h + ENTRY_HEADER + 1] ^= 1; // a byte value inside the RLE data
        let (d, _) = read(&bad_hist, &codecs(), ReadLimits::default()).unwrap();
        assert_eq!(d.sections.iter().map(|s| s.id).collect::<Vec<_>>(), [META, SectionId(*b"STRK"), SectionId(*b"EMPT")]);
        assert_eq!(d.dropped[0].id, HIST);
        assert!(d.get(HIST).is_none());

        let mut bad_meta = bytes.clone();
        bad_meta[header_at(&bytes, 0) + ENTRY_HEADER] ^= 0xFF;
        assert!(matches!(read(&bad_meta, &codecs(), ReadLimits::default()), Err(PackError::Corrupt(_) | PackError::Codec(_))));
    }

    #[test]
    fn truncation_anywhere_is_reported() {
        let bytes = packed(&sample(), RLE);
        for cut in 0..bytes.len() {
            assert_eq!(read(&bytes[..cut], &codecs(), ReadLimits::default()).unwrap_err(), PackError::Truncated, "cut at {cut}");
        }
    }

    #[test]
    fn impossible_sizes_are_refused_without_allocating() {
        let bytes = packed(&sample(), WriteOptions { codec: CodecId::STORED, level: Level::Fast });
        let mut huge_raw = bytes.clone();
        huge_raw[4 + 6..4 + 14].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(read(&huge_raw, &codecs(), ReadLimits::default()).unwrap_err(), PackError::TooLarge(u64::MAX));
        let mut huge_stored = bytes.clone();
        huge_stored[4 + 14..4 + 22].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(read(&huge_stored, &codecs(), ReadLimits::default()).unwrap_err(), PackError::Truncated);
        let mut many = bytes.clone();
        many[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read(&many, &codecs(), ReadLimits::default()), Err(PackError::Corrupt(_))));
    }

    #[test]
    fn duplicate_sections_are_corrupt() {
        let twice = vec![sample()[0].clone(), sample()[0].clone()];
        assert!(matches!(read(&packed(&twice, RLE), &codecs(), ReadLimits::default()), Err(PackError::Corrupt(_))));
    }

    #[test]
    fn writing_with_a_missing_codec_fails() {
        let mut out = Vec::new();
        let opts = WriteOptions { codec: CodecId(99), level: Level::Fast };
        assert_eq!(write(&mut out, &sample(), &codecs(), opts).unwrap_err(), PackError::UnknownCodec(CodecId(99)));
    }

    #[test]
    #[should_panic(expected = "already used")]
    fn codec_ids_are_unique() {
        codecs().register(Arc::new(Rle));
    }
}
