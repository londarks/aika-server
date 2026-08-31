//! `Data/Drops/Monsters_*_DropList.csv` — what monsters leave behind.
//!
//! Five files, one per band of monster levels, and three columns each:
//! a line number nobody reads, an item id, and which of four qualities it is.
//!
//! ```text
//! 1,5100,1     a normal item
//! 2,5101,1
//! 3,4630,1
//! ```
//!
//! A monster's `DropIndex` picks the band, not its level: the bands are named
//! after level ranges and the column is what the table is actually keyed on
//! (`Mob/BaseMob.pas:6920`).

use std::path::Path;

/// Qualities, in the order the file numbers them
/// (`Data/GlobalDefs.pas:259`).
pub const NORMAL: u8 = 1;
pub const SUPERIOR: u8 = 2;
pub const RARE: u8 = 3;
pub const LEGENDARY: u8 = 4;

/// The five bands, in the order `DropIndex` counts them.
pub const BAND_FILES: [&str; 5] = [
    "Monsters_0_20_DropList.csv",
    "Monsters_21_40_DropList.csv",
    "Monsters_41_60_DropList.csv",
    "Monsters_61_80_DropList.csv",
    "Monsters_81_99_DropList.csv",
];

pub const BANDS: usize = 5;

/// How often a kill leaves anything at all.
///
/// The original rolls 1 to 100 and drops when the roll is over 70
/// (`Mob/BaseMob.pas:6850`), which is a bit under a third of kills.
pub const DROP_ABOVE: u32 = 70;

/// Which quality a drop is, from a roll of 1 to 100
/// (`Mob/BaseMob.pas:6908`). One in a hundred is legendary.
pub fn quality_for(roll: u32) -> u8 {
    match roll {
        1 => LEGENDARY,
        2..=13 => RARE,
        14..=33 => SUPERIOR,
        _ => NORMAL,
    }
}

/// What one band of monsters can leave, by quality.
#[derive(Debug, Default, Clone)]
pub struct Band {
    pub normal: Vec<u16>,
    pub superior: Vec<u16>,
    pub rare: Vec<u16>,
    pub legendary: Vec<u16>,
}

impl Band {
    pub fn of(&self, quality: u8) -> &[u16] {
        match quality {
            LEGENDARY => &self.legendary,
            RARE => &self.rare,
            SUPERIOR => &self.superior,
            _ => &self.normal,
        }
    }

    /// The list for a quality, falling back to the normal one when that
    /// quality has nothing in this band.
    ///
    /// The original does the same: a quality with an empty list drops to
    /// normal rather than dropping nothing, so a lucky roll is never worse
    /// than an ordinary one.
    pub fn of_or_normal(&self, quality: u8) -> &[u16] {
        let chosen = self.of(quality);
        if chosen.is_empty() {
            &self.normal
        } else {
            chosen
        }
    }

    pub fn is_empty(&self) -> bool {
        self.normal.is_empty()
            && self.superior.is_empty()
            && self.rare.is_empty()
            && self.legendary.is_empty()
    }

    pub fn len(&self) -> usize {
        self.normal.len() + self.superior.len() + self.rare.len() + self.legendary.len()
    }
}

/// The five bands.
#[derive(Debug, Default)]
pub struct DropTable {
    bands: [Band; BANDS],
}

impl DropTable {
    /// Reads whichever of the five files are present. A missing one leaves
    /// its band empty rather than failing the rest.
    pub fn load_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let mut table = Self::default();
        for (i, file) in BAND_FILES.iter().enumerate() {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                table.bands[i] = parse_band(&latin1(&bytes));
            }
        }
        table
    }

    pub fn band(&self, index: usize) -> Option<&Band> {
        self.bands.get(index)
    }

    /// Which band a monster's drop index names, clamped so an index past the
    /// end still leaves something rather than nothing.
    pub fn band_for(&self, drop_index: u16) -> &Band {
        &self.bands[(drop_index as usize).min(BANDS - 1)]
    }

    pub fn is_empty(&self) -> bool {
        self.bands.iter().all(Band::is_empty)
    }

    pub fn len(&self) -> usize {
        self.bands.iter().map(Band::len).sum()
    }
}

/// Rolls one kill. `roll` and `quality_roll` are 1 to 100.
///
/// Returns the item to leave, or `None` when the kill drops nothing. Taking
/// the rolls as arguments keeps this a plain function: the caller brings the
/// randomness and a test brings certainty.
pub fn roll(band: &Band, roll: u32, quality_roll: u32, pick: usize) -> Option<u16> {
    if roll <= DROP_ABOVE {
        return None;
    }
    let list = band.of_or_normal(quality_for(quality_roll));
    if list.is_empty() {
        return None;
    }
    Some(list[pick % list.len()])
}

fn parse_band(text: &str) -> Band {
    let mut band = Band::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 3 {
            continue;
        }
        let (Ok(item), Ok(quality)) = (f[1].parse::<u16>(), f[2].parse::<u8>()) else {
            continue;
        };
        if item == 0 {
            continue;
        }
        match quality {
            LEGENDARY => band.legendary.push(item),
            RARE => band.rare.push(item),
            SUPERIOR => band.superior.push(item),
            NORMAL => band.normal.push(item),
            _ => {}
        }
    }
    band
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band() -> Band {
        parse_band(
            "1,5100,1\n\
             2,5101,1\n\
             3,4630,1\n\
             4,7000,2\n\
             5,8000,3\n\
             6,9000,4\n\
             \n\
             not,a,line,at,all\n\
             7,0,1\n",
        )
    }

    #[test]
    fn the_third_column_sorts_the_items_by_quality() {
        let b = band();
        assert_eq!(b.normal, vec![5100, 5101, 4630]);
        assert_eq!(b.superior, vec![7000]);
        assert_eq!(b.rare, vec![8000]);
        assert_eq!(b.legendary, vec![9000]);
    }

    #[test]
    fn a_line_that_is_not_a_drop_is_skipped_rather_than_fatal() {
        let b = band();
        assert_eq!(b.len(), 6, "a malformed line or a zero item got in");
    }

    /// The thresholds are the original's, and they are what makes a legendary
    /// worth something: one roll in a hundred.
    #[test]
    fn the_quality_thresholds_are_the_originals() {
        assert_eq!(quality_for(1), LEGENDARY);
        assert_eq!(quality_for(2), RARE);
        assert_eq!(quality_for(13), RARE);
        assert_eq!(quality_for(14), SUPERIOR);
        assert_eq!(quality_for(33), SUPERIOR);
        assert_eq!(quality_for(34), NORMAL);
        assert_eq!(quality_for(100), NORMAL);
    }

    #[test]
    fn most_kills_leave_nothing() {
        let b = band();
        assert_eq!(roll(&b, 1, 50, 0), None);
        assert_eq!(roll(&b, 70, 50, 0), None, "seventy is not over seventy");
        assert!(roll(&b, 71, 50, 0).is_some());
    }

    #[test]
    fn a_lucky_roll_takes_from_the_better_list() {
        let b = band();
        assert_eq!(roll(&b, 100, 1, 0), Some(9000), "legendary");
        assert_eq!(roll(&b, 100, 5, 0), Some(8000), "rare");
        assert_eq!(roll(&b, 100, 20, 0), Some(7000), "superior");
        assert_eq!(roll(&b, 100, 50, 1), Some(5101), "normal");
    }

    /// A lucky roll on a band with nothing rare in it must not be worse than
    /// an ordinary one.
    #[test]
    fn a_quality_with_nothing_in_it_falls_back_to_normal() {
        let mut b = band();
        b.legendary.clear();
        assert_eq!(roll(&b, 100, 1, 0), Some(5100));
    }

    #[test]
    fn a_band_with_nothing_in_it_drops_nothing() {
        assert_eq!(roll(&Band::default(), 100, 1, 0), None);
    }

    /// A drop index past the end still names a band, so a monster with a bad
    /// one leaves something rather than nothing.
    #[test]
    fn a_drop_index_past_the_end_is_clamped() {
        let table = DropTable::default();
        assert!(table.band_for(999).is_empty());
        assert!(table.band(0).is_some());
        assert!(table.band(BANDS).is_none());
    }

    /// The real files are not in this repository. When they are present the
    /// reader is held to what they hold.
    #[test]
    fn reads_the_original_files_when_they_are_available() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/drops");
        if !dir.join(BAND_FILES[0]).is_file() {
            return;
        }

        let table = DropTable::load_dir(&dir);
        assert!(table.len() > 100, "only {} drops in all five bands", table.len());

        for (i, file) in BAND_FILES.iter().enumerate() {
            let band = table.band(i).unwrap();
            assert!(!band.normal.is_empty(), "{file} has no ordinary drops");
        }
    }
}
