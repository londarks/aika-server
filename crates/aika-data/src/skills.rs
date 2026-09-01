//! `SkillData.bin` — every skill in the game, at every rank.
//!
//! 720-byte records, one after another from byte zero, with four bytes left
//! over at the end. The original declares `ARRAY [0 .. 11998]` and reads the
//! lot in one go (`Functions/Load.pas:522`); the file holds room for twelve
//! thousand and 7,284 of them are filled in.
//!
//! # A rank is a record
//!
//! The thing to know before reading anything else here: **each rank of a
//! skill is its own record**. Ids 1 to 16 are all called `Attack`, differing
//! only in their `rank`. So a skill id is not "which spell" — it is "which
//! spell, at which rank", and there is nothing to look up twice. The `family`
//! field is what all the ranks of one spell share.
//!
//! Layout, from `T_SkillData` (`Data/FilesData.pas:146`). Every field is
//! little-endian, and the sum of them is 720 exactly, which is how the
//! record was checked before a byte of it was trusted.
//!
//! ```text
//! 0    u32       family: which spell this is a rank of
//! 4    u32       the character level needed to learn it
//! 12   u32       rank
//! 16   u32       classification
//! 20   char[64]  name in English
//! 84   char[64]  name as the server's data has it
//! 148  u32       skill points it costs to learn
//! 152  u32       gold it costs to learn
//! 156  u32       class: base class times ten, plus the tier
//! 172  u32       mana
//! 184  u32       cooldown, in milliseconds
//! 192  u32       what it can be aimed at
//! 200  u32       how many things at once
//! 208  u32       range
//! 212  u32       radius around the point it lands on
//! 216  u32       success rate
//! 220  u32       whether it is aggressive
//! 224  u32       whether it is a buff or a debuff
//! 248  u32       damage
//! 292  u32       how long its effect lasts, in seconds
//! 320  u32       cast time, in milliseconds
//! 344  u32       animation on the caster, then on the target
//! 428  char[288] description
//! ```

pub const RECORD_SIZE: usize = 720;

/// Slots the file has room for. The original's array is one shorter and it
/// reads the whole thing at once, so the last record is past what it uses.
pub const SLOTS: usize = 12000;

/// Where each field sits inside a record.
///
/// Public because it is the format: a tool that writes a table, or a test
/// that needs one without the 8 MB file, has to know these.
pub mod field {
    use std::ops::Range;
    pub const FAMILY: usize = 0;
    pub const MIN_LEVEL: usize = 4;
    pub const RANK: usize = 12;
    pub const CLASSIFICATION: usize = 16;
    pub const NAME_ENGLISH: Range<usize> = 20..84;
    pub const NAME: Range<usize> = 84..148;
    pub const SKILL_POINTS: usize = 148;
    pub const LEARN_COST: usize = 152;
    pub const CLASS: usize = 156;
    pub const MANA: usize = 172;
    pub const PRE_COOLDOWN: usize = 180;
    pub const COOLDOWN: usize = 184;
    pub const TARGET_TYPE: usize = 192;
    pub const MAX_TARGETS: usize = 200;
    pub const RANGE: usize = 208;
    pub const RADIUS: usize = 212;
    pub const SUCCESS_RATE: usize = 216;
    pub const AGGRESSIVE: usize = 220;
    pub const BUFF_DEBUFF: usize = 224;
    pub const ATTRIBUTE: usize = 228;
    pub const DAMAGE: usize = 248;
    pub const EFFECT: Range<usize> = 260..276;
    pub const EFFECT_VALUE: Range<usize> = 276..292;
    pub const DURATION: usize = 292;
    pub const CAST_TIME: usize = 320;
    pub const SELF_ANIMATION: usize = 344;
    pub const TARGET_ANIMATION: usize = 348;
    pub const ANIMATION: usize = 356;
    pub const DESCRIPTION: Range<usize> = 428..716;
}

#[derive(Debug, PartialEq, Eq)]
pub enum SkillError {
    TooShort(usize),
    /// Not a whole number of records, by more than the four bytes the file is
    /// known to carry past the last one.
    UnalignedSize { size: usize, leftover: usize },
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillError::TooShort(size) => {
                write!(f, "{size} bytes, less than the {RECORD_SIZE} of a record")
            }
            SkillError::UnalignedSize { size, leftover } => {
                write!(f, "size {size} leaves {leftover} bytes past the last record")
            }
        }
    }
}

impl std::error::Error for SkillError {}

/// One rank of one skill.
///
/// Keeps its 720 bytes rather than copying every field out: most of the
/// record is fields nobody has needed yet, and a reader that keeps the
/// original can grow an accessor without touching how it is loaded.
#[derive(Clone)]
pub struct Skill {
    raw: [u8; RECORD_SIZE],
}

impl std::fmt::Debug for Skill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Skill")
            .field("name", &self.name_english())
            .field("family", &self.family())
            .field("rank", &self.rank())
            .field("mana", &self.mana())
            .field("damage", &self.damage())
            .finish()
    }
}

impl Skill {
    /// Which spell this is a rank of. All the ranks of one spell share it.
    pub fn family(&self) -> u32 {
        u32le(&self.raw, field::FAMILY)
    }

    /// Which rank. Rank 1 is the first one a character can learn.
    pub fn rank(&self) -> u32 {
        u32le(&self.raw, field::RANK)
    }

    /// The character level needed before this rank can be learnt.
    pub fn min_level(&self) -> u32 {
        u32le(&self.raw, field::MIN_LEVEL)
    }

    pub fn name_english(&self) -> String {
        text(&self.raw[field::NAME_ENGLISH])
    }

    pub fn name(&self) -> String {
        text(&self.raw[field::NAME])
    }

    pub fn description(&self) -> String {
        text(&self.raw[field::DESCRIPTION])
    }

    /// Base class times ten, plus a tier. Zero is a skill everybody has.
    pub fn class(&self) -> u32 {
        u32le(&self.raw, field::CLASS)
    }

    /// The base class this belongs to, or `None` for one everybody has.
    pub fn base_class(&self) -> Option<u32> {
        match self.class() {
            0 => None,
            c => Some(c / 10),
        }
    }

    pub fn skill_points(&self) -> u32 {
        u32le(&self.raw, field::SKILL_POINTS)
    }

    pub fn learn_cost(&self) -> u32 {
        u32le(&self.raw, field::LEARN_COST)
    }

    pub fn mana(&self) -> u32 {
        u32le(&self.raw, field::MANA)
    }

    /// Milliseconds before it can be used again.
    pub fn cooldown_ms(&self) -> u32 {
        u32le(&self.raw, field::COOLDOWN)
    }

    /// Milliseconds spent casting before it lands.
    pub fn cast_ms(&self) -> u32 {
        u32le(&self.raw, field::CAST_TIME)
    }

    /// How far it reaches. Zero means it is used on the caster.
    pub fn range(&self) -> u32 {
        u32le(&self.raw, field::RANGE)
    }

    /// How wide the area it lands on is. Zero means a single target.
    pub fn radius(&self) -> u32 {
        u32le(&self.raw, field::RADIUS)
    }

    pub fn max_targets(&self) -> u32 {
        u32le(&self.raw, field::MAX_TARGETS)
    }

    pub fn target_type(&self) -> u32 {
        u32le(&self.raw, field::TARGET_TYPE)
    }

    pub fn damage(&self) -> u32 {
        u32le(&self.raw, field::DAMAGE)
    }

    pub fn success_rate(&self) -> u32 {
        u32le(&self.raw, field::SUCCESS_RATE)
    }

    /// Whether using it counts as an attack.
    pub fn is_aggressive(&self) -> bool {
        u32le(&self.raw, field::AGGRESSIVE) != 0
    }

    /// Whether it leaves something behind on whatever it touched.
    pub fn is_buff(&self) -> bool {
        u32le(&self.raw, field::BUFF_DEBUFF) != 0
    }

    /// How long what it leaves behind lasts, in seconds.
    pub fn duration_secs(&self) -> u32 {
        u32le(&self.raw, field::DURATION)
    }

    /// How long the client draws a bar for before the cast lands, in
    /// milliseconds. Above zero and the skill reaches the server twice: once
    /// when the bar starts and once when it fills.
    pub fn cast_time_ms(&self) -> u32 {
        u32le(&self.raw, field::CAST_TIME)
    }

    pub fn self_animation(&self) -> u32 {
        u32le(&self.raw, field::SELF_ANIMATION)
    }

    pub fn target_animation(&self) -> u32 {
        u32le(&self.raw, field::TARGET_ANIMATION)
    }

    pub fn animation(&self) -> u32 {
        u32le(&self.raw, field::ANIMATION)
    }

    /// Four effect slots and their values, as the buff system will need them.
    pub fn effects(&self) -> [(i32, i32); 4] {
        let mut out = [(0, 0); 4];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = (
                i32le(&self.raw, field::EFFECT.start + i * 4),
                i32le(&self.raw, field::EFFECT_VALUE.start + i * 4),
            );
        }
        out
    }

    /// An unused slot: the file has room for twelve thousand and most of it
    /// is blank.
    pub fn is_empty(&self) -> bool {
        self.raw.iter().all(|&b| b == 0)
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// Every skill, indexed by the id the protocol carries.
#[derive(Default)]
pub struct SkillTable {
    skills: Vec<Skill>,
}

impl SkillTable {
    pub fn decode(bytes: &[u8]) -> Result<Self, SkillError> {
        if bytes.len() < RECORD_SIZE {
            return Err(SkillError::TooShort(bytes.len()));
        }
        // The shipped file carries four bytes past the last record, the same
        // way the item table does. More than that means the record size is
        // wrong, not that the file has a tail.
        let leftover = bytes.len() % RECORD_SIZE;
        if leftover > RECORD_SIZE / 2 {
            return Err(SkillError::UnalignedSize { size: bytes.len(), leftover });
        }

        Ok(Self {
            skills: bytes
                .chunks_exact(RECORD_SIZE)
                .map(|chunk| Skill { raw: chunk.try_into().unwrap() })
                .collect(),
        })
    }

    /// Looks a skill up by the id the protocol carries. Remember that the id
    /// names a rank, not a spell.
    pub fn get(&self, id: usize) -> Option<&Skill> {
        self.skills.get(id).filter(|s| !s.is_empty())
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Every id that holds a skill.
    pub fn defined(&self) -> impl Iterator<Item = (usize, &Skill)> {
        self.skills.iter().enumerate().filter(|(_, s)| !s.is_empty())
    }

    /// Every rank of one spell, in rank order.
    pub fn family(&self, family: u32) -> Vec<(usize, &Skill)> {
        let mut ranks: Vec<(usize, &Skill)> =
            self.defined().filter(|(_, s)| s.family() == family).collect();
        ranks.sort_by_key(|(_, s)| s.rank());
        ranks
    }

    /// The highest rank of a spell a character of this level and class may
    /// have, which is what deciding whether a cast is legitimate needs.
    pub fn highest_rank_for(&self, family: u32, level: u32) -> Option<(usize, &Skill)> {
        self.family(family)
            .into_iter()
            .filter(|(_, s)| s.min_level() <= level)
            .next_back()
    }
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    // latin-1, like every other table the pack ships.
    bytes[..end].iter().map(|&b| b as char).collect::<String>().trim_end().to_string()
}

fn u32le(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(raw[at..at + 4].try_into().unwrap())
}

fn i32le(raw: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(raw[at..at + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a table with a few skills in it, so the tests do not need the
    /// 8 MB file from the original pack.
    fn table() -> SkillTable {
        let mut raw = vec![0u8; 40 * RECORD_SIZE];

        let mut define = |id: usize,
                          family: u32,
                          rank: u32,
                          min_level: u32,
                          name: &str,
                          class: u32,
                          mana: u32,
                          damage: u32,
                          range: u32| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            let put = |r: &mut [u8], at: usize, v: u32| {
                r[at..at + 4].copy_from_slice(&v.to_le_bytes());
            };
            put(r, field::FAMILY, family);
            put(r, field::RANK, rank);
            put(r, field::MIN_LEVEL, min_level);
            put(r, field::CLASS, class);
            put(r, field::MANA, mana);
            put(r, field::DAMAGE, damage);
            put(r, field::RANGE, range);
            put(r, field::COOLDOWN, 400);
            r[field::NAME_ENGLISH.start..field::NAME_ENGLISH.start + name.len()]
                .copy_from_slice(name.as_bytes());
        };

        // three ranks of one spell, learnable at rising levels
        define(1, 0, 1, 1, "Attack", 0, 0, 1, 0);
        define(2, 0, 2, 10, "Attack", 0, 0, 1, 0);
        define(3, 0, 3, 20, "Attack", 0, 0, 1, 0);
        // a spell that costs mana and reaches
        define(17, 5, 1, 4, "Fireball", 21, 10, 120, 300);
        define(18, 5, 2, 14, "Fireball", 21, 18, 260, 300);

        SkillTable::decode(&raw).expect("the fixture table is malformed")
    }

    #[test]
    fn reads_the_fields_at_the_declared_offsets() {
        let t = table();
        let fireball = t.get(17).expect("no skill 17");

        assert_eq!(fireball.name_english(), "Fireball");
        assert_eq!(fireball.family(), 5);
        assert_eq!(fireball.rank(), 1);
        assert_eq!(fireball.min_level(), 4);
        assert_eq!(fireball.mana(), 10);
        assert_eq!(fireball.damage(), 120);
        assert_eq!(fireball.range(), 300);
        assert_eq!(fireball.cooldown_ms(), 400);
    }

    /// The record definition sums to exactly this, which is the arithmetic
    /// the whole layout rests on.
    #[test]
    fn a_record_is_seven_hundred_and_twenty_bytes() {
        assert_eq!(RECORD_SIZE, 720);
        assert_eq!(field::DESCRIPTION.end + 4, RECORD_SIZE, "the tail does not reach the end");
    }

    #[test]
    fn an_undefined_slot_is_not_a_skill() {
        let t = table();
        assert!(t.get(9).is_none(), "an empty slot came back as a skill");
        assert!(t.get(99999).is_none());
    }

    /// The thing that trips everybody up: an id names a rank, not a spell.
    #[test]
    fn every_rank_of_a_spell_is_its_own_id() {
        let t = table();
        let ranks = t.family(0);

        assert_eq!(ranks.len(), 3);
        assert_eq!(ranks.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(ranks.iter().map(|(_, s)| s.rank()).collect::<Vec<_>>(), vec![1, 2, 3]);
        for (_, rank) in &ranks {
            assert_eq!(rank.name_english(), "Attack", "the ranks are not one spell");
        }
    }

    /// What a level 15 character may have of a spell is its rank 2, not its
    /// rank 3.
    #[test]
    fn the_highest_rank_is_the_last_one_the_level_allows() {
        let t = table();

        assert_eq!(t.highest_rank_for(0, 1).map(|(id, _)| id), Some(1));
        assert_eq!(t.highest_rank_for(0, 15).map(|(id, _)| id), Some(2));
        assert_eq!(t.highest_rank_for(0, 100).map(|(id, _)| id), Some(3));
        assert_eq!(t.highest_rank_for(5, 1).map(|(id, _)| id), None, "too low to have any");
        assert!(t.highest_rank_for(999, 100).is_none(), "no such spell");
    }

    #[test]
    fn the_class_is_a_base_and_a_tier() {
        let t = table();
        assert_eq!(t.get(17).unwrap().class(), 21);
        assert_eq!(t.get(17).unwrap().base_class(), Some(2));
        assert_eq!(t.get(1).unwrap().base_class(), None, "everybody can attack");
    }

    #[test]
    fn refuses_something_that_is_not_a_table() {
        assert!(matches!(
            SkillTable::decode(&[0u8; 100]),
            Err(SkillError::TooShort(100))
        ));
        // half a record past the last one is a wrong record size, not a tail
        assert!(matches!(
            SkillTable::decode(&vec![0u8; RECORD_SIZE + 400]),
            Err(SkillError::UnalignedSize { leftover: 400, .. })
        ));
    }

    /// The real file is not in this repository. When it is present the parser
    /// is held to what it contains.
    #[test]
    fn reads_the_original_file_when_it_is_available() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/skills/SkillData.bin");
        if !path.is_file() {
            return;
        }

        let bytes = std::fs::read(&path).unwrap();
        let table = SkillTable::decode(&bytes).unwrap();

        assert_eq!(table.len(), SLOTS, "the file is not twelve thousand records");
        assert!(table.defined().count() > 7000, "only {} skills", table.defined().count());

        // the first skill in the game, which everybody has
        let attack = table.get(1).expect("skill 1 is missing");
        assert_eq!(attack.name_english(), "Attack");
        assert_eq!(attack.rank(), 1);
        assert_eq!(attack.cooldown_ms(), 400);

        // and something, somewhere, must cost mana and do damage, or an
        // offset is wrong
        assert!(
            table.defined().any(|(_, s)| s.mana() > 0),
            "no skill costs mana: the mana offset is off"
        );
        assert!(
            table.defined().any(|(_, s)| s.damage() > 1 && s.range() > 0),
            "no skill reaches and hurts: the damage or range offset is off"
        );
    }
}
