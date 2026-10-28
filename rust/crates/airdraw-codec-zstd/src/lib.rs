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

    /// Never reserves `raw_len` up front: a damaged or crafted section header can claim
    /// gigabytes, and on iPad an allocation that size can fail, which aborts the app instead of
    /// reporting a bad file. So the claim is first checked against the size the frame records
    /// itself (we always write it), and the output then grows only as data actually decodes,
    /// stopping one byte past `raw_len`.
    fn decompress(&self, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackError> {
        use std::io::Read;
        let err = |m: String| PackError::Codec(format!("zstd: {m}"));
        match zstd::zstd_safe::get_frame_content_size(stored) {
            Ok(Some(n)) if n != raw_len as u64 => {
                return Err(err(format!("frame holds {n} bytes, section header says {raw_len}")));
            }
            Ok(_) => {}
            Err(_) => return Err(err("not a zstd frame".into())),
        }
        let decoder = zstd::stream::read::Decoder::with_buffer(stored).map_err(|e| err(e.to_string()))?;
        // A first guess from the stored size; read_to_end grows it as needed.
        let mut out = Vec::with_capacity(raw_len.min(stored.len().saturating_mul(4)));
        decoder.take(raw_len as u64 + 1).read_to_end(&mut out).map_err(|e| err(e.to_string()))?;
        if out.len() != raw_len {
            return Err(err(format!("{} bytes, expected {raw_len}", out.len())));
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

    /// Records the largest single allocation, so tests can check what decompression reserves
    /// (on macOS a terabyte reservation "succeeds" lazily, so failing isn't the signal; on iPad
    /// it can fail and abort the app).
    struct Counting;
    static LARGEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            LARGEST.fetch_max(layout.size(), std::sync::atomic::Ordering::Relaxed);
            unsafe { std::alloc::System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
            LARGEST.fetch_max(new_size, std::sync::atomic::Ordering::Relaxed);
            unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
        }
    }
    #[global_allocator]
    static ALLOC: Counting = Counting;

    /// A section header claiming far more than the data holds (a crafted file, or a frame that
    /// doesn't record its size) is an error, and decompressing it never reserves the claim.
    #[test]
    fn huge_claimed_sizes_fail_without_reserving_them() {
        let raw = points(10_000);
        let packed = Zstd.compress(&raw, Level::Fast).unwrap();
        let mut no_size = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
        no_size.include_contentsize(false).unwrap();
        std::io::Write::write_all(&mut no_size, &raw).unwrap();
        let no_size = no_size.finish().unwrap();
        assert_eq!(zstd::zstd_safe::get_frame_content_size(&no_size).unwrap(), None);
        assert_eq!(Zstd.decompress(&no_size, raw.len()).unwrap(), raw);
        assert!(Zstd.decompress(&no_size, raw.len() - 1).is_err());

        let claim = 1usize << 31;
        for frame in [&packed, &no_size] {
            LARGEST.store(0, std::sync::atomic::Ordering::Relaxed);
            assert!(Zstd.decompress(frame, claim).is_err());
            let largest = LARGEST.load(std::sync::atomic::Ordering::Relaxed);
            // Other tests run in parallel and allocate a little; nothing near the claim.
            assert!(largest < 64 << 20, "reserved {largest} bytes for a {claim}-byte claim");
        }
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
