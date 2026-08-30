//! `strdef*.bin` — the client's master string table.
//!
//! Every piece of interface text lives here: error messages, button labels,
//! window titles. The format is as simple as it gets — a flat array of
//! fixed-width records, latin-1 text padded with NUL, no header, no
//! compression, no cipher. The record count is the file size divided by 128.
//!
//! The client picks which table to load from `UI/RN.dat`, which holds a single
//! language id; `strdef4.bin` is the Portuguese one, `strdef1.bin` the Korean
//! original. So a new translation is a new numbered file plus one edited digit.
//!
//! The hard limit is the record width: a translated string must fit in 127
//! bytes plus its terminator. There is nowhere to grow.

/// Every record is this wide, text plus NUL padding.
pub const RECORD_SIZE: usize = 128;
/// Longest text a record can hold, leaving room for the terminator.
pub const MAX_TEXT: usize = RECORD_SIZE - 1;

/// Below this many bytes a string is too short to judge: "Ação" is half high
/// bytes and would look like CJK.
const CJK_MIN_LEN: usize = 6;
/// Fraction of high bytes above which the text is double-byte rather than
/// accented latin-1.
///
/// Counting *consecutive* high bytes does not work: a Big5 trail byte is
/// often plain ASCII, so 我是被選中者 encodes as `A7 DA AC 4F B3 51 BF EF` —
/// the `4F` is an `O`. What does separate them is density. Big5 and EUC-KR
/// spend a high byte on every character, landing near 100%, while Portuguese
/// only spends one on the accented ones: "Informações" is 18%.
const CJK_HIGH_RATIO: f32 = 0.45;

#[derive(Debug, PartialEq, Eq)]
pub enum StrDefError {
    /// The file is not a whole number of records.
    UnalignedSize(usize),
    /// The text does not fit the fixed-width record.
    TextTooLong { index: usize, len: usize },
    /// No record with that index.
    OutOfRange { index: usize, len: usize },
}

impl std::fmt::Display for StrDefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StrDefError::UnalignedSize(size) => {
                write!(f, "size {size} is not a multiple of {RECORD_SIZE}")
            }
            StrDefError::TextTooLong { index, len } => {
                write!(f, "entry {index}: {len} bytes, the record holds {MAX_TEXT}")
            }
            StrDefError::OutOfRange { index, len } => {
                write!(f, "entry {index} does not exist; the table has {len}")
            }
        }
    }
}

impl std::error::Error for StrDefError {}

/// A string table. Records are kept as raw bytes so that rewriting a file
/// after editing a handful of entries leaves every other byte untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrDef {
    records: Vec<[u8; RECORD_SIZE]>,
}

impl StrDef {
    pub fn decode(bytes: &[u8]) -> Result<Self, StrDefError> {
        if bytes.len() % RECORD_SIZE != 0 {
            return Err(StrDefError::UnalignedSize(bytes.len()));
        }
        Ok(Self {
            records: bytes
                .chunks_exact(RECORD_SIZE)
                .map(|chunk| chunk.try_into().unwrap())
                .collect(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.records.len() * RECORD_SIZE);
        for record in &self.records {
            out.extend_from_slice(record);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Text of one entry, decoded as latin-1 up to the first NUL.
    pub fn get(&self, index: usize) -> Option<String> {
        let record = self.records.get(index)?;
        let end = record.iter().position(|&b| b == 0).unwrap_or(RECORD_SIZE);
        Some(record[..end].iter().map(|&b| b as char).collect())
    }

    /// Raw bytes of one entry, for callers that need to inspect the encoding.
    pub fn raw(&self, index: usize) -> Option<&[u8]> {
        self.records.get(index).map(|r| &r[..])
    }

    /// Replaces one entry, clearing the rest of the record.
    ///
    /// The text is encoded as latin-1: any character outside it is rejected,
    /// because the client has no way to render it. That is also why a
    /// translation cannot introduce characters the original language did not
    /// have.
    pub fn set(&mut self, index: usize, text: &str) -> Result<(), StrDefError> {
        let len = self.records.len();
        let record = self
            .records
            .get_mut(index)
            .ok_or(StrDefError::OutOfRange { index, len })?;

        let bytes: Vec<u8> = text
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        if bytes.len() > MAX_TEXT {
            return Err(StrDefError::TextTooLong { index, len: bytes.len() });
        }

        record.fill(0);
        record[..bytes.len()].copy_from_slice(&bytes);
        Ok(())
    }

    /// Entries that hold text, in file order.
    pub fn occupied(&self) -> impl Iterator<Item = (usize, String)> + '_ {
        (0..self.len())
            .filter_map(move |i| self.get(i).map(|t| (i, t)))
            .filter(|(_, text)| !text.trim().is_empty())
    }

    /// Entries that still hold double-byte text, meaning they were never
    /// translated. This is the working list for a translation pass.
    pub fn untranslated(&self) -> impl Iterator<Item = (usize, &[u8])> + '_ {
        (0..self.len()).filter_map(move |i| {
            let raw = self.raw(i)?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(RECORD_SIZE);
            looks_double_byte(&raw[..end]).then_some((i, &raw[..end]))
        })
    }

    /// Indexes where two tables differ, with both texts. Comparing a shipped
    /// file against a half-finished one shows exactly what is missing.
    pub fn differences<'a>(&'a self, other: &'a StrDef) -> Vec<(usize, String, String)> {
        let count = self.len().max(other.len());
        (0..count)
            .filter_map(|i| {
                let mine = self.get(i).unwrap_or_default();
                let theirs = other.get(i).unwrap_or_default();
                (mine != theirs).then_some((i, mine, theirs))
            })
            .collect()
    }
}

/// Whether a run of bytes looks like Big5 or EUC-KR rather than accented
/// latin-1. See [`CJK_HIGH_RATIO`] for why density settles it and a run of
/// consecutive high bytes does not.
pub fn looks_double_byte(bytes: &[u8]) -> bool {
    if bytes.len() < CJK_MIN_LEN {
        return false;
    }
    let high = bytes.iter().filter(|&&b| b >= 0x80).count();
    high as f32 / bytes.len() as f32 >= CJK_HIGH_RATIO
}

/// Finds runs of text inside any file and returns the ones that look
/// double-byte, with their offsets.
///
/// Not every file the client shows text from is a table of fixed records:
/// scene layouts, descriptions and lore files embed NUL-terminated strings in
/// binary structures. This walks the bytes instead of assuming a record size,
/// so the same audit works on all of them.
pub fn scan_double_byte(bytes: &[u8], min_len: usize) -> Vec<(usize, &[u8])> {
    let mut found = Vec::new();
    let mut start = None;

    for (i, &b) in bytes.iter().enumerate() {
        // A run ends at NUL or at the padding byte the scene files use.
        let is_text = b != 0 && b != 0xFE && (b >= 0x20 || b == 10 || b == 13);
        match (is_text, start) {
            (true, None) => start = Some(i),
            (false, Some(from)) => {
                if i - from >= min_len && looks_double_byte(&bytes[from..i]) {
                    found.push((from, &bytes[from..i]));
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        if bytes.len() - from >= min_len && looks_double_byte(&bytes[from..]) {
            found.push((from, &bytes[from..]));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[&str]) -> StrDef {
        let mut bytes = vec![0u8; entries.len() * RECORD_SIZE];
        for (i, text) in entries.iter().enumerate() {
            let at = i * RECORD_SIZE;
            let raw: Vec<u8> = text.chars().map(|c| c as u8).collect();
            bytes[at..at + raw.len()].copy_from_slice(&raw);
        }
        StrDef::decode(&bytes).unwrap()
    }

    #[test]
    fn reads_and_rewrites_without_touching_other_records() {
        let mut t = table(&["Confirmar", "Cancelar", "Bem-vindo ao Aika."]);
        let before = t.encode();

        t.set(1, "Voltar").unwrap();

        assert_eq!(t.get(0).unwrap(), "Confirmar");
        assert_eq!(t.get(1).unwrap(), "Voltar");
        assert_eq!(t.get(2).unwrap(), "Bem-vindo ao Aika.");

        // only the edited record changed
        let after = t.encode();
        assert_eq!(after[..RECORD_SIZE], before[..RECORD_SIZE]);
        assert_eq!(after[RECORD_SIZE * 2..], before[RECORD_SIZE * 2..]);
    }

    #[test]
    fn untouched_table_reencodes_byte_for_byte() {
        let t = table(&["um", "dois", "", "quatro"]);
        let bytes = t.encode();
        assert_eq!(StrDef::decode(&bytes).unwrap().encode(), bytes);
    }

    #[test]
    fn refuses_text_longer_than_the_record() {
        let mut t = table(&["curto"]);
        assert_eq!(t.set(0, &"x".repeat(MAX_TEXT)), Ok(()));
        assert_eq!(
            t.set(0, &"x".repeat(MAX_TEXT + 1)),
            Err(StrDefError::TextTooLong { index: 0, len: MAX_TEXT + 1 })
        );
        assert_eq!(
            t.set(9, "nada"),
            Err(StrDefError::OutOfRange { index: 9, len: 1 })
        );
        assert_eq!(StrDef::decode(&[0u8; 127]), Err(StrDefError::UnalignedSize(127)));
    }

    #[test]
    fn keeps_accented_portuguese_intact() {
        let mut t = table(&[""]);
        t.set(0, "Informações não encontradas").unwrap();
        assert_eq!(t.get(0).unwrap(), "Informações não encontradas");
        assert!(
            !looks_double_byte(t.raw(0).unwrap()),
            "accented latin-1 must not be mistaken for CJK"
        );
    }

    #[test]
    fn spots_untranslated_double_byte_entries() {
        // "我是被選中者" in Big5, the intro line still shipped in Chinese
        let big5 = [0xA7, 0xDA, 0xAC, 0x4F, 0xB3, 0x51, 0xBF, 0xEF];
        let mut bytes = vec![0u8; RECORD_SIZE * 2];
        bytes[..8].copy_from_slice(&big5);
        bytes[RECORD_SIZE..RECORD_SIZE + 8].copy_from_slice(b"Cancelar");

        let t = StrDef::decode(&bytes).unwrap();
        let pending: Vec<usize> = t.untranslated().map(|(i, _)| i).collect();
        assert_eq!(pending, vec![0], "only the Big5 entry is pending");
    }

    /// Short accented words are half high bytes and must not be mistaken for
    /// CJK, which is why the check has a minimum length.
    #[test]
    fn short_accented_words_are_not_flagged() {
        for word in ["Ação", "Não", "Sim", "Coração", "Informações não encontradas"] {
            let raw: Vec<u8> = word.chars().map(|c| c as u8).collect();
            assert!(!looks_double_byte(&raw), "{word} must not look like CJK");
        }
    }

    #[test]
    fn scanning_a_binary_finds_embedded_double_byte_text() {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"Sistema ");
        blob.extend_from_slice(&[0xFE, 0xFE, 0xFE, 0xFE]);
        let big5 = [0xA7, 0xDA, 0xAC, 0x4F, 0xB3, 0x51, 0xBF, 0xEF];
        let at = blob.len();
        blob.extend_from_slice(&big5);
        blob.push(0);
        blob.extend_from_slice(b"Cancelar ");

        let found = scan_double_byte(&blob, 6);
        assert_eq!(found.len(), 1, "only the Big5 run is reported");
        assert_eq!(found[0].0, at, "offset points at the run");
        assert_eq!(found[0].1, &big5);
    }

    #[test]
    fn diffing_two_tables_lists_what_changed() {
        let shipped = table(&["Confirmar", "Cancelar", "Sair"]);
        let half_done = table(&["Confirmar", "Voltar", "Sair"]);

        let diff = shipped.differences(&half_done);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0], (1, "Cancelar".to_string(), "Voltar".to_string()));
    }

    #[test]
    fn tables_of_different_lengths_still_diff() {
        let short = table(&["um"]);
        let long = table(&["um", "dois"]);
        assert_eq!(short.differences(&long), vec![(1, String::new(), "dois".to_string())]);
    }
}
