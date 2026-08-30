//! `SL.bin` — the channel list the client shows on its server selection
//! screen. This is the file that points the client at a different server.
//!
//! It is a flat sequence of 72-byte records, obscured by a trivial positional
//! cipher: every byte is added to its own offset modulo 5 (`EncDecSL` in the
//! Delphi server). No header and no count — the number of channels is the
//! file size divided by 72.
//!
//! Record layout (`TChannelFromList`, `Data/FilesData.pas:425`):
//!
//! ```text
//! 0..32   char[32]  IP, NUL terminated
//! 32..36  u32       unknown (the original source calls it Unk_0)
//! 36..60  char[24]  display name
//! 60..64  u32       Check, always 0xFFFFFFFF in real files
//! 64..68  u32       NationIndex
//! 68..72  u32       ChannelNationIndex
//! ```

pub const RECORD_SIZE: usize = 72;

const IP: std::ops::Range<usize> = 0..32;
const UNKNOWN: std::ops::Range<usize> = 32..36;
const NAME: std::ops::Range<usize> = 36..60;
const CHECK: std::ops::Range<usize> = 60..64;
const NATION_INDEX: std::ops::Range<usize> = 64..68;
const CHANNEL_NATION_INDEX: std::ops::Range<usize> = 68..72;

#[derive(Debug, PartialEq, Eq)]
pub enum SlError {
    /// The file is not a multiple of the record size.
    UnalignedSize(usize),
    /// Text longer than the fixed-size field can hold.
    FieldTooLong { field: &'static str, max: usize },
}

impl std::fmt::Display for SlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlError::UnalignedSize(size) => {
                write!(f, "size {size} is not a multiple of {RECORD_SIZE}")
            }
            SlError::FieldTooLong { field, max } => {
                write!(f, "field '{field}' exceeds {max} bytes")
            }
        }
    }
}

impl std::error::Error for SlError {}

/// One channel in the list. It keeps the original 72 bytes so that rewriting
/// a file after touching a single field leaves every other byte alone — the
/// fields nobody understands included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    raw: [u8; RECORD_SIZE],
}

impl Channel {
    pub fn empty() -> Self {
        let mut raw = [0u8; RECORD_SIZE];
        raw[CHECK].copy_from_slice(&u32::MAX.to_le_bytes());
        Self { raw }
    }

    pub fn ip(&self) -> String {
        read_str(&self.raw[IP])
    }

    pub fn set_ip(&mut self, ip: &str) -> Result<(), SlError> {
        write_str(&mut self.raw[IP], ip, "ip")
    }

    pub fn name(&self) -> String {
        read_str(&self.raw[NAME])
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), SlError> {
        write_str(&mut self.raw[NAME], name, "name")
    }

    pub fn nation_index(&self) -> u32 {
        read_u32(&self.raw[NATION_INDEX])
    }

    pub fn set_nation_index(&mut self, value: u32) {
        self.raw[NATION_INDEX].copy_from_slice(&value.to_le_bytes());
    }

    pub fn channel_nation_index(&self) -> u32 {
        read_u32(&self.raw[CHANNEL_NATION_INDEX])
    }

    pub fn check(&self) -> u32 {
        read_u32(&self.raw[CHECK])
    }

    pub fn unknown(&self) -> u32 {
        read_u32(&self.raw[UNKNOWN])
    }

    /// A channel with neither IP nor name is an empty slot.
    pub fn is_empty(&self) -> bool {
        self.ip().is_empty() && self.name().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerList {
    pub channels: Vec<Channel>,
}

impl ServerList {
    /// Reads an `SL.bin` as it sits on disk, still obscured.
    pub fn decode(bytes: &[u8]) -> Result<Self, SlError> {
        if bytes.len() % RECORD_SIZE != 0 {
            return Err(SlError::UnalignedSize(bytes.len()));
        }
        let plain = decipher(bytes);
        let channels = plain
            .chunks_exact(RECORD_SIZE)
            .map(|chunk| Channel { raw: chunk.try_into().unwrap() })
            .collect();
        Ok(Self { channels })
    }

    /// Returns the obscured bytes, ready to be written out.
    pub fn encode(&self) -> Vec<u8> {
        let mut plain = Vec::with_capacity(self.channels.len() * RECORD_SIZE);
        for channel in &self.channels {
            plain.extend_from_slice(&channel.raw);
        }
        encipher(&plain)
    }

    /// Occupied channels, along with the slot they take in the file.
    pub fn occupied(&self) -> impl Iterator<Item = (usize, &Channel)> {
        self.channels.iter().enumerate().filter(|(_, c)| !c.is_empty())
    }
}

/// Decodes: every byte loses its own offset modulo 5.
pub fn decipher(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(i, &b)| b.wrapping_sub((i % 5) as u8))
        .collect()
}

/// Encodes: the inverse operation.
pub fn encipher(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(i, &b)| b.wrapping_add((i % 5) as u8))
        .collect()
}

fn read_str(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    // The client speaks latin-1.
    field[..end].iter().map(|&b| b as char).collect()
}

/// Writes the text and clears the rest of the field. The original editor left
/// remnants of the previous value after the NUL; writing a clean field is
/// equivalent for the client, which reads up to the NUL, and avoids leaking
/// stale names.
fn write_str(field: &mut [u8], value: &str, name: &'static str) -> Result<(), SlError> {
    let bytes: Vec<u8> = value.chars().map(|c| c as u8).collect();
    if bytes.len() >= field.len() {
        return Err(SlError::FieldTooLong { field: name, max: field.len() - 1 });
    }
    field.fill(0);
    field[..bytes.len()].copy_from_slice(&bytes);
    Ok(())
}

fn read_u32(field: &[u8]) -> u32 {
    u32::from_le_bytes(field.try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `SL.bin` from a client, if one was dropped into `testdata/`.
    ///
    /// Client files are not redistributed with this repository, so this is
    /// opt-in: copy `SL.bin` from a client folder into
    /// `crates/aika-data/testdata/` and the round-trip tests below start
    /// checking against real bytes. The codec was verified byte for byte
    /// against a 16 KB file this way.
    fn real_file() -> Option<Vec<u8>> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/SL.bin")).ok()
    }

    /// A list built entirely by us, so the codec is always exercised.
    fn synthetic() -> Vec<u8> {
        let mut channels = Vec::new();
        for (i, name) in ["Alpha", "Beta", "Gamma"].iter().enumerate() {
            let mut channel = Channel::empty();
            channel.set_ip(&format!("127.0.0.{}", i + 1)).unwrap();
            channel.set_name(name).unwrap();
            channel.set_nation_index(i as u32 + 1);
            channels.push(channel);
        }
        channels.push(Channel::empty());
        ServerList { channels }.encode()
    }

    #[test]
    fn cipher_roundtrips() {
        let data: Vec<u8> = (0..=255u8).cycle().take(720).collect();
        assert_eq!(decipher(&encipher(&data)), data);
    }

    #[test]
    fn roundtrips_a_synthetic_list() {
        let bytes = synthetic();
        let list = ServerList::decode(&bytes).unwrap();

        assert_eq!(list.channels.len(), 4);
        assert_eq!(list.occupied().count(), 3, "the trailing empty slot is not occupied");
        assert_eq!(list.channels[0].ip(), "127.0.0.1");
        assert_eq!(list.channels[1].name(), "Beta");
        assert_eq!(list.channels[2].nation_index(), 3);
        assert_eq!(list.encode(), bytes, "re-encoding must reproduce the bytes");
    }

    #[test]
    fn reads_the_real_server_list() {
        let Some(bytes) = real_file() else {
            return; // no client file dropped in; the synthetic test covers the codec
        };
        let list = ServerList::decode(&bytes).unwrap();
        assert_eq!(list.channels.len(), bytes.len() / RECORD_SIZE);

        let occupied: Vec<_> = list.occupied().collect();
        assert!(!occupied.is_empty(), "a real file has at least one channel");
        for (_, channel) in &occupied {
            assert_eq!(channel.check(), 0xFFFF_FFFF, "Check is 0xFFFFFFFF in real files");
        }
    }

    #[test]
    fn reencodes_untouched_file_byte_for_byte() {
        let Some(bytes) = real_file() else {
            return;
        };
        let list = ServerList::decode(&bytes).unwrap();
        assert_eq!(list.encode(), bytes, "rewriting without edits must reproduce the file");
    }

    #[test]
    fn changing_ip_leaves_other_fields_alone() {
        let bytes = real_file().unwrap_or_else(synthetic);
        let mut list = ServerList::decode(&bytes).unwrap();
        let before = list.channels[0].clone();

        list.channels[0].set_ip("192.168.0.50").unwrap();

        let after = &list.channels[0];
        assert_eq!(after.ip(), "192.168.0.50");
        assert_eq!(after.name(), before.name());
        assert_eq!(after.nation_index(), before.nation_index());
        assert_eq!(after.unknown(), before.unknown());
        assert_eq!(after.check(), before.check());

        // and the file is still readable
        let rewritten = ServerList::decode(&list.encode()).unwrap();
        assert_eq!(rewritten.channels[0].ip(), "192.168.0.50");
        assert_eq!(rewritten.channels[1], ServerList::decode(&bytes).unwrap().channels[1]);
    }

    #[test]
    fn builds_a_list_from_scratch() {
        let mut channel = Channel::empty();
        channel.set_ip("127.0.0.1").unwrap();
        channel.set_name("Meu Servidor").unwrap();
        channel.set_nation_index(1);

        let list = ServerList { channels: vec![channel] };
        let decoded = ServerList::decode(&list.encode()).unwrap();

        assert_eq!(decoded.channels.len(), 1);
        assert_eq!(decoded.channels[0].ip(), "127.0.0.1");
        assert_eq!(decoded.channels[0].name(), "Meu Servidor");
        assert_eq!(decoded.channels[0].check(), 0xFFFF_FFFF);
    }

    #[test]
    fn rejects_oversized_fields_and_bad_sizes() {
        let mut channel = Channel::empty();
        assert_eq!(
            channel.set_name(&"n".repeat(24)),
            Err(SlError::FieldTooLong { field: "name", max: 23 })
        );
        assert_eq!(ServerList::decode(&[0u8; 71]), Err(SlError::UnalignedSize(71)));
    }
}
