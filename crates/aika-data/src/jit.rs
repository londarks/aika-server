//! `.jit` — the client's texture format, which is where the whole interface
//! lives: buttons, panels, icons, login and loading screens.
//!
//! It is a twelve-byte header in front of raw DXT blocks, and nothing else.
//! No compression of its own, no cipher, no index.
//!
//! ```text
//! 0..4   char[4]  magic, "JT" followed by two digits
//! 4..8   u32      width
//! 8..12  u32      height
//! 12..            DXT blocks, optionally a full mipmap chain
//! ```
//!
//! The last digit of the magic is the DXT number: `JT31` and `JT41` are DXT1,
//! `JT33`/`JT43` are DXT3, `JT35`/`JT45` are DXT5. The first digit varies
//! between files of the same layout and is preserved rather than interpreted.
//!
//! Because the payload is untouched DXT, converting to and from DDS is a
//! matter of swapping one header for another: the pixel data is copied
//! verbatim, so a round trip is lossless.

/// The header the client reads before the blocks.
pub const HEADER_SIZE: usize = 12;
/// A DDS header is always this long, magic included.
pub const DDS_HEADER_SIZE: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dxt {
    Dxt1,
    Dxt3,
    Dxt5,
}

impl Dxt {
    /// Bytes per 4x4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            Dxt::Dxt1 => 8,
            Dxt::Dxt3 | Dxt::Dxt5 => 16,
        }
    }

    pub fn four_cc(self) -> [u8; 4] {
        match self {
            Dxt::Dxt1 => *b"DXT1",
            Dxt::Dxt3 => *b"DXT3",
            Dxt::Dxt5 => *b"DXT5",
        }
    }

    pub fn from_four_cc(cc: &[u8]) -> Option<Self> {
        match cc {
            b"DXT1" => Some(Dxt::Dxt1),
            b"DXT3" => Some(Dxt::Dxt3),
            b"DXT5" => Some(Dxt::Dxt5),
            _ => None,
        }
    }

    /// From the last digit of a `JT` magic.
    fn from_magic(magic: &[u8]) -> Option<Self> {
        match magic.last()? {
            b'1' => Some(Dxt::Dxt1),
            b'3' => Some(Dxt::Dxt3),
            b'5' => Some(Dxt::Dxt5),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum JitError {
    TooShort(usize),
    /// The magic is not a `JT` tag this code understands.
    UnknownMagic([u8; 4]),
    /// The payload does not match any whole number of mip levels.
    SizeMismatch { expected_base: usize, actual: usize },
    NotDds,
    /// The DDS is not DXT compressed, so it cannot go back into a `.jit`.
    UnsupportedDds([u8; 4]),
    /// Replacing a texture must keep the client's expectations intact.
    Mismatch(&'static str),
}

impl std::fmt::Display for JitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JitError::TooShort(n) => write!(f, "only {n} bytes, too short for a header"),
            JitError::UnknownMagic(m) => {
                write!(f, "unknown magic {:?}", String::from_utf8_lossy(m))
            }
            JitError::SizeMismatch { expected_base, actual } => write!(
                f,
                "payload is {actual} bytes; the top mip level alone needs {expected_base}"
            ),
            JitError::NotDds => write!(f, "not a DDS file"),
            JitError::UnsupportedDds(cc) => {
                write!(f, "DDS is {:?}, only DXT1/3/5 can become a .jit", String::from_utf8_lossy(cc))
            }
            JitError::Mismatch(what) => write!(f, "does not match the original texture: {what}"),
        }
    }
}

impl std::error::Error for JitError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jit {
    /// Kept as read so a rewritten file is indistinguishable from the original.
    pub magic: [u8; 4],
    pub width: u32,
    pub height: u32,
    pub format: Dxt,
    /// How many mip levels the payload holds, at least one.
    pub levels: u32,
    pub data: Vec<u8>,
}

impl Jit {
    pub fn decode(bytes: &[u8]) -> Result<Self, JitError> {
        if bytes.len() < HEADER_SIZE {
            return Err(JitError::TooShort(bytes.len()));
        }
        let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        if &magic[0..2] != b"JT" {
            return Err(JitError::UnknownMagic(magic));
        }
        let format = Dxt::from_magic(&magic).ok_or(JitError::UnknownMagic(magic))?;

        let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let data = bytes[HEADER_SIZE..].to_vec();

        let base = level_bytes(width, height, format);
        if data.len() < base {
            return Err(JitError::SizeMismatch { expected_base: base, actual: data.len() });
        }
        let levels = count_levels(width, height, format, data.len());

        Ok(Self { magic, width, height, format, levels, data })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_SIZE + self.data.len());
        out.extend_from_slice(&self.magic);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Wraps the blocks in a DDS header. The pixel data is copied untouched,
    /// so any image tool that reads DDS sees exactly what the client sees.
    pub fn to_dds(&self) -> Vec<u8> {
        const DDSD_REQUIRED: u32 = 0x1 | 0x2 | 0x4 | 0x1000; // caps, height, width, pixelformat
        const DDSD_LINEARSIZE: u32 = 0x0008_0000;
        const DDSD_MIPMAPCOUNT: u32 = 0x0002_0000;
        const DDPF_FOURCC: u32 = 0x4;
        const DDSCAPS_TEXTURE: u32 = 0x1000;
        const DDSCAPS_COMPLEX: u32 = 0x8;
        const DDSCAPS_MIPMAP: u32 = 0x0040_0000;

        let has_mips = self.levels > 1;
        let mut flags = DDSD_REQUIRED | DDSD_LINEARSIZE;
        let mut caps = DDSCAPS_TEXTURE;
        if has_mips {
            flags |= DDSD_MIPMAPCOUNT;
            caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
        }

        let mut out = Vec::with_capacity(DDS_HEADER_SIZE + self.data.len());
        out.extend_from_slice(b"DDS ");
        let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
        put(124); // header size, magic excluded
        put(flags);
        put(self.height);
        put(self.width);
        put(level_bytes(self.width, self.height, self.format) as u32);
        put(0); // depth
        put(self.levels);
        for _ in 0..11 {
            put(0); // reserved
        }
        // pixel format
        put(32);
        put(DDPF_FOURCC);
        out.extend_from_slice(&self.format.four_cc());
        for _ in 0..5 {
            let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
            put(0);
        }
        let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
        put(caps);
        put(0); // caps2
        put(0); // caps3
        put(0); // caps4
        put(0); // reserved
        debug_assert_eq!(out.len(), DDS_HEADER_SIZE);

        out.extend_from_slice(&self.data);
        out
    }

    /// Rebuilds a `.jit` from an edited DDS, keeping this texture's identity.
    ///
    /// The replacement must have the same size and compression as the original:
    /// the client allocates from the header it already knows, and a texture
    /// that grew would not fit. Taking the original as the template is also
    /// what preserves the `JT` magic, whose first digit we do not interpret.
    pub fn replace_from_dds(&self, dds: &[u8]) -> Result<Jit, JitError> {
        if dds.len() < DDS_HEADER_SIZE || &dds[0..4] != b"DDS " {
            return Err(JitError::NotDds);
        }
        let height = u32::from_le_bytes(dds[12..16].try_into().unwrap());
        let width = u32::from_le_bytes(dds[16..20].try_into().unwrap());
        let four_cc = &dds[84..88];

        let format = Dxt::from_four_cc(four_cc)
            .ok_or_else(|| JitError::UnsupportedDds(four_cc.try_into().unwrap()))?;

        if width != self.width || height != self.height {
            return Err(JitError::Mismatch("different dimensions"));
        }
        if format != self.format {
            return Err(JitError::Mismatch("different DXT compression"));
        }

        let data = dds[DDS_HEADER_SIZE..].to_vec();
        let base = level_bytes(width, height, format);
        if data.len() < base {
            return Err(JitError::SizeMismatch { expected_base: base, actual: data.len() });
        }

        Ok(Jit {
            magic: self.magic,
            width,
            height,
            format,
            levels: count_levels(width, height, format, data.len()),
            data,
        })
    }
}

/// Bytes one mip level occupies. DXT works on 4x4 blocks, so anything smaller
/// than a block still costs a whole one.
pub fn level_bytes(width: u32, height: u32, format: Dxt) -> usize {
    let blocks_w = width.div_ceil(4).max(1) as usize;
    let blocks_h = height.div_ceil(4).max(1) as usize;
    blocks_w * blocks_h * format.block_bytes()
}

/// How many mip levels fit in a payload of this size.
fn count_levels(width: u32, height: u32, format: Dxt, payload: usize) -> u32 {
    let (mut w, mut h, mut used, mut levels) = (width, height, 0usize, 0u32);
    loop {
        let step = level_bytes(w, h, format);
        if used + step > payload {
            break;
        }
        used += step;
        levels += 1;
        if w == 1 && h == 1 {
            break;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    levels.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jit(magic: &[u8; 4], w: u32, h: u32, format: Dxt, levels: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(magic);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        let (mut ww, mut hh) = (w, h);
        for _ in 0..levels {
            out.extend(std::iter::repeat(0xAB).take(level_bytes(ww, hh, format)));
            ww = (ww / 2).max(1);
            hh = (hh / 2).max(1);
        }
        out
    }

    #[test]
    fn reads_the_shapes_the_client_ships() {
        // sizes taken from real UI textures
        for (magic, w, h, format, expect) in [
            (b"JT31", 1024u32, 1024u32, Dxt::Dxt1, 524_300usize),
            (b"JT41", 1024, 768, Dxt::Dxt1, 393_228),
            (b"JT35", 1024, 1024, Dxt::Dxt5, 1_048_588),
        ] {
            let bytes = jit(magic, w, h, format, 1);
            assert_eq!(bytes.len(), expect, "{:?} {w}x{h}", String::from_utf8_lossy(magic));

            let t = Jit::decode(&bytes).unwrap();
            assert_eq!((t.width, t.height, t.format, t.levels), (w, h, format, 1));
        }
    }

    #[test]
    fn counts_a_mipmap_chain() {
        let bytes = jit(b"JT35", 64, 64, Dxt::Dxt5, 7);
        let t = Jit::decode(&bytes).unwrap();
        assert_eq!(t.levels, 7, "64x64 down to 1x1");
    }

    #[test]
    fn reencodes_byte_for_byte() {
        let bytes = jit(b"JT33", 256, 128, Dxt::Dxt3, 1);
        assert_eq!(Jit::decode(&bytes).unwrap().encode(), bytes);
    }

    #[test]
    fn dds_round_trip_keeps_the_pixels_and_the_magic() {
        let bytes = jit(b"JT41", 256, 256, Dxt::Dxt1, 1);
        let original = Jit::decode(&bytes).unwrap();

        let dds = original.to_dds();
        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(&dds[84..88], b"DXT1");
        assert_eq!(u32::from_le_bytes(dds[12..16].try_into().unwrap()), 256, "height");
        assert_eq!(dds.len(), DDS_HEADER_SIZE + original.data.len());

        let back = original.replace_from_dds(&dds).unwrap();
        assert_eq!(back, original, "the round trip must be lossless");
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn refuses_a_replacement_that_would_not_fit() {
        let original = Jit::decode(&jit(b"JT31", 256, 256, Dxt::Dxt1, 1)).unwrap();

        let bigger = Jit::decode(&jit(b"JT31", 512, 512, Dxt::Dxt1, 1)).unwrap().to_dds();
        assert_eq!(original.replace_from_dds(&bigger), Err(JitError::Mismatch("different dimensions")));

        let other = Jit::decode(&jit(b"JT35", 256, 256, Dxt::Dxt5, 1)).unwrap().to_dds();
        assert_eq!(
            original.replace_from_dds(&other),
            Err(JitError::Mismatch("different DXT compression"))
        );

        assert_eq!(original.replace_from_dds(b"not a dds"), Err(JitError::NotDds));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Jit::decode(&[1, 2, 3]), Err(JitError::TooShort(3)));
        assert_eq!(
            Jit::decode(b"XXXX\0\0\0\0\0\0\0\0"),
            Err(JitError::UnknownMagic(*b"XXXX"))
        );
        // a header promising more pixels than the payload holds
        let mut short = jit(b"JT31", 256, 256, Dxt::Dxt1, 1);
        short.truncate(HEADER_SIZE + 16);
        assert!(matches!(Jit::decode(&short), Err(JitError::SizeMismatch { .. })));
    }
}
