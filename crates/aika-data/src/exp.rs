//! `ExpList.bin` — how much experience each level takes.
//!
//! A flat run of 64-bit numbers and nothing else: no header, no count. Entry
//! `i` is the running total needed to *be* level `i + 1`, so the first is
//! zero and each one after it is larger.
//!
//! The file is 1,220 bytes, which is 152 whole numbers and four bytes over.
//! The original declares an array of 152 and reads the file's whole length
//! into it, overrunning by those four (`Functions/Load.pas:782`). Only the
//! first hundred entries are a curve; past that the numbers stop rising and
//! turn into whatever was on the disk. So the game has a hundred levels, and
//! reading the file to its end gets you a level 101 that costs less than
//! level 3.

pub const ENTRY_SIZE: usize = 8;

/// Levels past this are not in the file, whatever its length suggests.
pub const MAX_LEVEL: u16 = 100;

#[derive(Debug, PartialEq, Eq)]
pub enum ExpError {
    TooShort(usize),
}

impl std::fmt::Display for ExpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpError::TooShort(size) => write!(f, "{size} bytes is not an experience table"),
        }
    }
}

impl std::error::Error for ExpError {}

/// What each level costs.
#[derive(Debug, Default, Clone)]
pub struct ExpTable {
    /// Running totals, one per level, in level order.
    totals: Vec<u64>,
}

impl ExpTable {
    /// Reads the table, stopping at the first entry that *falls*.
    ///
    /// That check is the whole trick: the file has more room than data, and
    /// the only thing separating the curve from the leftovers is that a curve
    /// never goes down. Falling rather than rising is the right test — the
    /// last two levels cost the same, so a plateau at the top is real data
    /// and stopping at it would lose level 100.
    pub fn decode(bytes: &[u8]) -> Result<Self, ExpError> {
        if bytes.len() < ENTRY_SIZE * 2 {
            return Err(ExpError::TooShort(bytes.len()));
        }

        let mut totals = Vec::new();
        for chunk in bytes.chunks_exact(ENTRY_SIZE) {
            let value = u64::from_le_bytes(chunk.try_into().unwrap());
            if totals.last().is_some_and(|last| value < *last) {
                break;
            }
            totals.push(value);
            if totals.len() >= MAX_LEVEL as usize {
                break;
            }
        }
        Ok(Self { totals })
    }

    /// The highest level this table describes.
    pub fn max_level(&self) -> u16 {
        self.totals.len() as u16
    }

    pub fn is_empty(&self) -> bool {
        self.totals.is_empty()
    }

    /// The running total needed to reach a level, or `None` past the end.
    pub fn total_for(&self, level: u16) -> Option<u64> {
        self.totals.get(level.checked_sub(1)? as usize).copied()
    }

    /// What a character of this level has to reach to gain the next one.
    /// `None` when there is no next one.
    pub fn next_at(&self, level: u16) -> Option<u64> {
        self.total_for(level + 1)
    }

    /// The level a running total of experience is worth.
    ///
    /// Levels never fall here: a character that somehow has less experience
    /// than its level implies keeps the level. Taking it away would be a
    /// worse answer to a bad number than leaving it.
    pub fn level_for(&self, exp: u64) -> u16 {
        match self.totals.binary_search(&exp) {
            Ok(i) => i as u16 + 1,
            Err(0) => 1,
            Err(i) => i as u16,
        }
    }

    /// How many levels this much experience is worth beyond the current one.
    pub fn levels_gained(&self, level: u16, exp: u64) -> u16 {
        self.level_for(exp).saturating_sub(level).min(self.max_level())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A short curve, plus the leftovers the real file has past its end.
    fn table() -> ExpTable {
        let mut bytes = Vec::new();
        for total in [0u64, 200, 538, 1171, 2294] {
            bytes.extend_from_slice(&total.to_le_bytes());
        }
        // a plateau at the top, then what a file with more room than data
        // holds after the curve
        bytes.extend_from_slice(&2294u64.to_le_bytes());
        for junk in [0u64, 858_993_459_200, 12] {
            bytes.extend_from_slice(&junk.to_le_bytes());
        }
        ExpTable::decode(&bytes).expect("the fixture is malformed")
    }

    #[test]
    fn the_curve_stops_where_it_stops_rising() {
        let t = table();
        assert_eq!(t.max_level(), 6, "the leftovers were read as levels");
        assert_eq!(t.total_for(1), Some(0));
        assert_eq!(t.total_for(5), Some(2294));
        assert_eq!(t.total_for(6), Some(2294), "the plateau is real data");
        assert_eq!(t.total_for(7), None);
    }

    #[test]
    fn a_level_is_what_the_running_total_is_worth() {
        let t = table();
        assert_eq!(t.level_for(0), 1);
        assert_eq!(t.level_for(199), 1, "one short of the next one");
        assert_eq!(t.level_for(200), 2, "exactly enough");
        assert_eq!(t.level_for(537), 2);
        assert_eq!(t.level_for(538), 3);
        assert_eq!(t.level_for(999_999), 6, "past the end of the curve");
    }

    #[test]
    fn the_next_level_is_the_number_to_reach() {
        let t = table();
        assert_eq!(t.next_at(1), Some(200));
        assert_eq!(t.next_at(4), Some(2294));
        assert_eq!(t.next_at(6), None, "there is no level 7");
    }

    /// One kill can be worth more than one level.
    #[test]
    fn several_levels_at_once() {
        let t = table();
        assert_eq!(t.levels_gained(1, 199), 0);
        assert_eq!(t.levels_gained(1, 200), 1);
        assert_eq!(t.levels_gained(1, 1171), 3);
        assert_eq!(t.levels_gained(1, 999_999), 5, "capped by the end of the curve");
    }

    /// A character whose experience does not match its level keeps the level.
    /// Taking one away is a worse answer to a bad number than leaving it.
    #[test]
    fn a_level_is_never_taken_away() {
        let t = table();
        assert_eq!(t.levels_gained(5, 0), 0);
        assert_eq!(t.levels_gained(3, 200), 0);
    }

    #[test]
    fn something_that_is_not_a_table_is_refused() {
        assert!(matches!(ExpTable::decode(&[0u8; 4]), Err(ExpError::TooShort(4))));
    }

    /// The real file is not in this repository. When it is present the reader
    /// is held to what it holds.
    #[test]
    fn reads_the_original_file_when_it_is_available() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/ExpList.bin");
        if !path.is_file() {
            return;
        }

        let table = ExpTable::decode(&std::fs::read(&path).unwrap()).unwrap();

        assert_eq!(table.max_level(), MAX_LEVEL, "the game has a hundred levels");
        assert_eq!(table.total_for(1), Some(0), "the first level is free");
        assert_eq!(table.total_for(2), Some(200));
        assert_eq!(table.total_for(100), Some(13_780_203_528));

        // and it never falls, which is what separates it from the leftovers
        // the file ends with. It does level off: the last two levels cost the
        // same, which is why this is not a strict rise.
        for level in 2..=table.max_level() {
            assert!(
                table.total_for(level) >= table.total_for(level - 1),
                "level {level} costs less than the one before it"
            );
        }
        assert_eq!(
            table.total_for(99),
            table.total_for(100),
            "the curve levels off at the top, and dropping that plateau loses level 100"
        );
    }
}
