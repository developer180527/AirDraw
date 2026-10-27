//! Minimal uncompressed ("stored") zip writer with 64-byte data alignment, as USDZ requires.

pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<(String, u32, u32, u32)>, // name, crc, size, local header offset
}

/// USDZ requires every file's data to start at a multiple of 64 bytes.
const ALIGN: usize = 64;
/// Extra-field id used for alignment padding (same as Pixar's usdzip).
const PADDING_ID: u16 = 0x1986;

impl ZipWriter {
    pub fn new() -> Self {
        Self { out: Vec::new(), entries: Vec::new() }
    }

    pub fn add(&mut self, name: &str, data: &[u8]) {
        let offset = self.out.len();
        let crc = crc32(data);
        // Data starts after the 30-byte header, the name and the extra field.
        let unpadded = offset + 30 + name.len();
        let mut pad = (ALIGN - unpadded % ALIGN) % ALIGN;
        if pad != 0 && pad < 4 {
            pad += ALIGN; // An extra field needs at least its 4-byte header.
        }
        let o = &mut self.out;
        o.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        o.extend_from_slice(&20u16.to_le_bytes()); // version needed
        o.extend_from_slice(&0u16.to_le_bytes()); // flags
        o.extend_from_slice(&0u16.to_le_bytes()); // stored
        o.extend_from_slice(&0u16.to_le_bytes()); // time
        o.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
        o.extend_from_slice(&crc.to_le_bytes());
        o.extend_from_slice(&(data.len() as u32).to_le_bytes());
        o.extend_from_slice(&(data.len() as u32).to_le_bytes());
        o.extend_from_slice(&(name.len() as u16).to_le_bytes());
        o.extend_from_slice(&(pad as u16).to_le_bytes());
        o.extend_from_slice(name.as_bytes());
        if pad > 0 {
            o.extend_from_slice(&PADDING_ID.to_le_bytes());
            o.extend_from_slice(&((pad - 4) as u16).to_le_bytes());
            o.resize(o.len() + pad - 4, 0);
        }
        debug_assert_eq!(o.len() % ALIGN, 0);
        o.extend_from_slice(data);
        self.entries.push((name.into(), crc, data.len() as u32, offset as u32));
    }

    pub fn finish(mut self) -> Vec<u8> {
        let dir_start = self.out.len();
        for (name, crc, size, offset) in &self.entries {
            let o = &mut self.out;
            o.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            o.extend_from_slice(&20u16.to_le_bytes()); // made by
            o.extend_from_slice(&20u16.to_le_bytes()); // needed
            o.extend_from_slice(&0u16.to_le_bytes());
            o.extend_from_slice(&0u16.to_le_bytes());
            o.extend_from_slice(&0u16.to_le_bytes());
            o.extend_from_slice(&0x21u16.to_le_bytes());
            o.extend_from_slice(&crc.to_le_bytes());
            o.extend_from_slice(&size.to_le_bytes());
            o.extend_from_slice(&size.to_le_bytes());
            o.extend_from_slice(&(name.len() as u16).to_le_bytes());
            o.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, internal attr, external attr
            o.extend_from_slice(&offset.to_le_bytes());
            o.extend_from_slice(name.as_bytes());
        }
        let dir_len = self.out.len() - dir_start;
        let n = self.entries.len() as u16;
        let o = &mut self.out;
        o.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        o.extend_from_slice(&[0; 4]); // disk numbers
        o.extend_from_slice(&n.to_le_bytes());
        o.extend_from_slice(&n.to_le_bytes());
        o.extend_from_slice(&(dir_len as u32).to_le_bytes());
        o.extend_from_slice(&(dir_start as u32).to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn data_is_64_byte_aligned() {
        let mut z = ZipWriter::new();
        z.add("scene.usda", b"#usda 1.0\n");
        z.add("x.bin", &[7; 100]);
        let bytes = z.finish();
        // Each local header: data begins at 30 + name + extra.
        let mut p = 0;
        for _ in 0..2 {
            assert_eq!(&bytes[p..p + 4], &[0x50, 0x4b, 3, 4]);
            let size = u32::from_le_bytes(bytes[p + 18..p + 22].try_into().unwrap()) as usize;
            let name = u16::from_le_bytes([bytes[p + 26], bytes[p + 27]]) as usize;
            let extra = u16::from_le_bytes([bytes[p + 28], bytes[p + 29]]) as usize;
            let data = p + 30 + name + extra;
            assert_eq!(data % 64, 0);
            p = data + size;
        }
        assert_eq!(&bytes[p..p + 4], &[0x50, 0x4b, 1, 2]);
    }
}
