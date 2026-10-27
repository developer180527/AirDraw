//! Zstandard for `.airdraw` sections: codec id 1 in `airdraw-pack`. Wraps libzstd (the
//! `zstd` crate builds it from source with `cc` for every target), single-threaded so the
//! output for a given input and level is always the same.

use airdraw_pack::{Codec, CodecId, Level, PackError};

pub struct Zstd;

impl Zstd {
    fn level(level: Level) -> i32 {
        match level {
            Level::Fast => 3,
            Level::Default => 9,
            Level::Max => 19,
        }
    }
}

impl Codec for Zstd {
    fn id(&self) -> CodecId {
        CodecId::ZSTD
    }

    fn name(&self) -> &str {
        "zstd"
    }

    fn compress(&self, raw: &[u8], level: Level) -> Result<Vec<u8>, PackError> {
        zstd::bulk::compress(raw, Self::level(level)).map_err(|e| PackError::Codec(format!("zstd: {e}")))
    }

    fn decompress(&self, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackError> {
        // `raw_len` is the capacity, so a bad frame can't make it allocate more.
        let out = zstd::bulk::decompress(stored, raw_len).map_err(|e| PackError::Codec(format!("zstd: {e}")))?;
        if out.len() != raw_len {
            return Err(PackError::Codec(format!("zstd: {} bytes, expected {raw_len}", out.len())));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use airdraw_pack::{Codecs, ReadLimits, Section, SectionId, WriteOptions, read, write};

    use super::*;

    /// Something stroke-like: slowly changing floats.
    fn points(n: usize) -> Vec<u8> {
        (0..n).flat_map(|i| [(i as f32 * 0.01).sin(), (i as f32 * 0.013).cos(), 0.5]).flat_map(f32::to_le_bytes).collect()
    }

    #[test]
    fn round_trips_at_every_level() {
        let raw = points(10_000);
        for level in [Level::Fast, Level::Default, Level::Max] {
            let packed = Zstd.compress(&raw, level).unwrap();
            assert!(packed.len() < raw.len(), "{level:?}");
            assert_eq!(Zstd.decompress(&packed, raw.len()).unwrap(), raw);
        }
        assert_eq!(Zstd.decompress(&Zstd.compress(&[], Level::Fast).unwrap(), 0).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn output_is_deterministic() {
        let raw = points(10_000);
        assert_eq!(Zstd.compress(&raw, Level::Fast).unwrap(), Zstd.compress(&raw, Level::Fast).unwrap());
    }

    #[test]
    fn bad_data_and_wrong_lengths_are_errors() {
        let raw = points(1000);
        let packed = Zstd.compress(&raw, Level::Fast).unwrap();
        assert!(Zstd.decompress(b"not zstd at all", raw.len()).is_err());
        assert!(Zstd.decompress(&packed, raw.len() - 1).is_err(), "longer than the header says");
        assert!(Zstd.decompress(&packed, raw.len() + 1).is_err(), "shorter than the header says");
        assert!(Zstd.decompress(&packed[..packed.len() / 2], raw.len()).is_err());
    }

    #[test]
    fn works_inside_the_container() {
        let mut codecs = Codecs::new();
        codecs.register(Arc::new(Zstd));
        let sections = vec![Section { id: SectionId(*b"STRK"), required: true, data: points(5000) }];
        let mut bytes = Vec::new();
        write(&mut bytes, &sections, &codecs, WriteOptions { codec: CodecId::ZSTD, level: Level::Fast }).unwrap();
        assert_eq!(bytes[4 + 5], CodecId::ZSTD.0);
        assert!(bytes.len() < sections[0].data.len());
        assert_eq!(read(&bytes, &codecs, ReadLimits::default()).unwrap().0.sections, sections);
    }
}
