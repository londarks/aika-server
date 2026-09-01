//! What a character can do, and whether it may do it right now.
//!
//! `aika_data::skills` says what every skill is. This says which of them a
//! given character has, what using one costs, and how long it has to wait
//! before using it again.
//!
//! # The table is a grid
//!
//! Every class owns 960 consecutive ids, every one of its sixty slots owns
//! sixteen of them, and the rank picks one. So a skill id says which class,
//! which slot and which rank without a lookup, and whether a client may cast
//! something is a range check rather than a search.
//!
//! # Known skills are derived, not stored
//!
//! In the original a character *learns* skills: `0x31C` spends skill points
//! and gold at a trainer, and the list is saved with the account. That is not
//! built yet, so for now a character knows every skill its class and level
//! allow, worked out from the table each time it is asked.
//!
//! This is a deliberate placeholder and it shows in one visible way: nobody
//! can be missing a spell they should have, and nobody has to visit a
//! trainer. When `0x31C` lands, `known_by` becomes a database read and this
//! function becomes what the trainer offers rather than what the character
//! has.

use aika_data::skills::SkillTable;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// `TSendSkillUse`.
pub const OP_USE_SKILL: u16 = 0x320;
/// `TSendSkillsPacket`, the same opcode and size as the shop window. The
/// client tells them apart by what it asked for.
pub const OP_SKILL_LIST: u16 = 0x106;

/// Skills the list packet carries. The original sends the forty "others" and
/// leaves the six basics out (`Mob/Player.pas:7121`).
pub const SKILL_SLOTS: usize = 40;

/// A skill with no range of its own is used on the caster; one that reaches
/// still needs the target to be in front of you, and the original checks the
/// skill's own range. This is the floor for a skill whose range reads zero
/// but which is aimed at something.
pub const MINIMUM_REACH: f32 = 15.0;

/// Why a skill could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastError {
    /// No such skill in the table.
    NoSuchSkill(u32),
    /// The character does not have it: wrong class, or not high enough level.
    NotLearned(u32),
    NotEnoughMana { needed: u32, held: u32 },
    /// Still on cooldown, with how much of it is left.
    Cooling { left: Duration },
    OutOfRange,
    /// Aimed at something that cannot be hit with it.
    BadTarget,
}

impl CastError {
    pub fn message(&self) -> String {
        match self {
            CastError::NoSuchSkill(id) => format!("Skill {id} does not exist."),
            CastError::NotLearned(id) => format!("You have not learned skill {id}."),
            CastError::NotEnoughMana { needed, held } => {
                format!("You need {needed} mana and have {held}.")
            }
            CastError::Cooling { left } => {
                format!("Not ready for another {:.1}s.", left.as_secs_f32())
            }
            CastError::OutOfRange => "That is too far away.".into(),
            CastError::BadTarget => "You cannot use that on this.".into(),
        }
    }
}

impl std::fmt::Display for CastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CastError {}

/// `0x320`: the client used a skill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UseSkill {
    pub skill: u32,
    pub target: u32,
    pub at: (f32, f32),
}

impl UseSkill {
    pub const BODY_SIZE: usize = 16;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            skill: u32::from_le_bytes(body[0..4].try_into().ok()?),
            target: u32::from_le_bytes(body[4..8].try_into().ok()?),
            at: (
                f32::from_le_bytes(body[8..12].try_into().ok()?),
                f32::from_le_bytes(body[12..16].try_into().ok()?),
            ),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.skill.to_le_bytes());
        body.extend_from_slice(&self.target.to_le_bytes());
        body.extend_from_slice(&self.at.0.to_le_bytes());
        body.extend_from_slice(&self.at.1.to_le_bytes());
        body
    }
}

/// Skills the table gives each class: sixty slots of sixteen ranks.
pub const SLOTS_PER_CLASS: usize = 60;
pub const RANKS_PER_SLOT: usize = 16;
/// So a class owns this many consecutive ids.
pub const IDS_PER_CLASS: usize = SLOTS_PER_CLASS * RANKS_PER_SLOT;

/// The six basic slots, which the client draws apart from the rest.
pub const BASIC_SLOTS: usize = 6;

/// Where a class's skills start.
///
/// The first class begins at 1 and the rest at whole multiples of 960. That
/// off-by-one is the original's, not a mistake here: `GetSkillIndex` opens
/// with `Result := 1` and only overwrites it when the class is not the first
/// (`Functions/SkillFunctions.pas:85`).
pub fn class_block(class_number: u32) -> usize {
    if class_number <= 1 {
        1
    } else {
        (class_number as usize - 1) * IDS_PER_CLASS
    }
}

/// The id of one skill: which class, which of its sixty slots, which rank.
///
/// This is `TSkillFunctions.GetSkillIndex`, and it is a grid rather than a
/// lookup: every class owns 960 consecutive ids, every slot owns sixteen of
/// them, and the rank picks one. Checked against the six character templates
/// the original ships, which carry the ids this produces.
pub fn skill_index(class_number: u32, slot: usize, rank: u32) -> usize {
    let mut id = class_block(class_number);
    if slot > 1 {
        id += (slot - 1) * RANKS_PER_SLOT;
    }
    // Rank 1 and rank 2 land on the same id. That is what the original does.
    id + if rank > 1 { rank as usize - 1 } else { 1 }
}

/// Whether an id is one of this class's, which is what stops a client asking
/// for another class's spell by number.
pub fn belongs_to(class_number: u32, id: usize) -> bool {
    let start = class_block(class_number);
    (start..start + IDS_PER_CLASS).contains(&id)
}

/// Which slot of the record's sixty-word skill list a skill id belongs to.
///
/// The sixteen ranks of one slot are consecutive ids starting one past the
/// slot's base, so the slot is the id's distance from the class block divided
/// by sixteen. Slots zero to five are the basic skills and six to forty-five
/// the advanced ones, which is exactly how the record is laid out. `None` for
/// an id that is not this class's or lands past the sixty slots.
pub fn record_slot(class_number: u32, id: usize) -> Option<usize> {
    if !belongs_to(class_number, id) {
        return None;
    }
    let offset = id.checked_sub(class_block(class_number) + 1)?;
    let slot = offset / RANKS_PER_SLOT;
    (slot < SLOTS_PER_CLASS).then_some(slot)
}

/// The skills a character of this class starts with, in slot order: the six
/// basic ones, then the forty the bar carries.
///
/// Every slot at rank one, which is what the templates hold. Slots the table
/// has nothing in are dropped rather than sent as an id that resolves to
/// nothing.
pub fn known_by(table: &SkillTable, class_number: u32, level: u32) -> Vec<usize> {
    (1..=BASIC_SLOTS + SKILL_SLOTS)
        .map(|slot| skill_index(class_number, slot, 1))
        .filter(|id| table.get(*id).is_some_and(|s| s.min_level() <= level))
        .collect()
}

/// Just the forty the bar carries, which is what `0x106` sends.
pub fn bar_of(table: &SkillTable, class_number: u32, level: u32) -> Vec<usize> {
    (BASIC_SLOTS + 1..=BASIC_SLOTS + SKILL_SLOTS)
        .map(|slot| skill_index(class_number, slot, 1))
        .filter(|id| table.get(*id).is_some_and(|s| s.min_level() <= level))
        .collect()
}

/// When each skill may be used again.
///
/// Keyed by the spell rather than the rank: learning a better rank must not
/// hand somebody a fresh cooldown on the same spell.
#[derive(Debug, Default)]
pub struct Cooldowns {
    ready_at: HashMap<u32, Instant>,
}

impl Cooldowns {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long is left, or `None` if it can be used now.
    pub fn remaining(&self, family: u32, now: Instant) -> Option<Duration> {
        let at = self.ready_at.get(&family)?;
        (*at > now).then(|| *at - now)
    }

    pub fn start(&mut self, family: u32, cooldown: Duration, now: Instant) {
        if cooldown.is_zero() {
            self.ready_at.remove(&family);
        } else {
            self.ready_at.insert(family, now + cooldown);
        }
    }
}

/// Everything a cast has to be checked against, gathered in one place so the
/// decision is one function and can be tested without a socket.
pub struct Caster {
    /// Which of the six, counted from one.
    pub class_number: u32,
    pub level: u32,
    pub mana: u32,
    pub at: (f32, f32),
}

/// What a successful cast costs and does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cast {
    pub skill: usize,
    pub family: u32,
    pub mana: u32,
    pub damage: u32,
    pub cooldown: Duration,
    pub animation: u32,
    pub is_aggressive: bool,
}

/// Decides whether a cast may happen, and what it costs if it does.
///
/// `target` is where the thing being aimed at stands, or `None` when the
/// skill is used on the caster.
pub fn check(
    table: &SkillTable,
    caster: &Caster,
    cooldowns: &Cooldowns,
    id: u32,
    target: Option<(f32, f32)>,
    now: Instant,
) -> Result<Cast, CastError> {
    let skill = table.get(id as usize).ok_or(CastError::NoSuchSkill(id))?;

    // The id has to be one of this class's sixty slots. Checking the block
    // rather than the skill's own class column is what the grid makes
    // possible, and it is stricter: the column would let a client ask for a
    // higher tier of its own class that it has not earned.
    if !belongs_to(caster.class_number, id as usize) || skill.min_level() > caster.level {
        return Err(CastError::NotLearned(id));
    }

    if skill.mana() > caster.mana {
        return Err(CastError::NotEnoughMana { needed: skill.mana(), held: caster.mana });
    }

    if let Some(left) = cooldowns.remaining(skill.family(), now) {
        return Err(CastError::Cooling { left });
    }

    if let Some(target) = target {
        // A skill with no range of its own still has to be within arm's
        // reach; the alternative is hitting things across the map with any
        // spell whose range column happens to read zero.
        let reach = (skill.range() as f32).max(MINIMUM_REACH);
        let (dx, dy) = (caster.at.0 - target.0, caster.at.1 - target.1);
        if dx * dx + dy * dy > reach * reach {
            return Err(CastError::OutOfRange);
        }
    }

    Ok(Cast {
        skill: id as usize,
        family: skill.family(),
        mana: skill.mana(),
        damage: skill.damage(),
        cooldown: Duration::from_millis(skill.cooldown_ms() as u64),
        animation: skill.animation(),
        is_aggressive: skill.is_aggressive(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aika_data::skills::{field, RECORD_SIZE};

    /// The third class, which is the Atirador.
    const CLASS: u32 = 3;

    /// A table with the third class's first ten slots filled in, at every
    /// rank, laid out the way the real file lays them out.
    fn table() -> SkillTable {
        let mut raw = vec![0u8; 4000 * RECORD_SIZE];

        for slot in 1..=10usize {
            for rank in 1..=RANKS_PER_SLOT as u32 {
                let id = class_block(CLASS) + (slot - 1) * RANKS_PER_SLOT + rank as usize;
                let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
                let put = |r: &mut [u8], at: usize, v: u32| {
                    r[at..at + 4].copy_from_slice(&v.to_le_bytes());
                };
                put(r, field::FAMILY, slot as u32);
                put(r, field::RANK, rank);
                // later slots need a higher character level
                put(r, field::MIN_LEVEL, if slot <= 6 { 1 } else { slot as u32 * 5 });
                put(r, field::CLASS, (CLASS - 1) * 10 + 1);
                put(r, field::MANA, 10 * rank);
                put(r, field::DAMAGE, 100 * rank);
                put(r, field::RANGE, 300);
                put(r, field::COOLDOWN, 3000);
                put(r, field::AGGRESSIVE, 1);
                r[field::NAME_ENGLISH.start] = b'x';
            }
        }
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    }

    fn caster(class_number: u32, level: u32, mana: u32) -> Caster {
        Caster { class_number, level, mana, at: (100.0, 100.0) }
    }

    #[test]
    fn use_skill_body_roundtrip() {
        let original = UseSkill { skill: 1921, target: 3048, at: (100.0, 200.0) };
        assert_eq!(UseSkill::parse(&original.to_body()), Some(original));
        assert_eq!(UseSkill::parse(&[0u8; 8]), None);
    }

    /// The grid, against the ids the original's own character templates
    /// carry. Getting this wrong hands out somebody else's spells.
    #[test]
    fn the_grid_matches_the_templates() {
        // Guerreiro: the first class starts at 1, not at 0
        assert_eq!(
            (1..=6).map(|s| skill_index(1, s, 1)).collect::<Vec<_>>(),
            vec![2, 18, 34, 50, 66, 82]
        );
        // Atirador
        assert_eq!(
            (1..=6).map(|s| skill_index(3, s, 1)).collect::<Vec<_>>(),
            vec![1921, 1937, 1953, 1969, 1985, 2001]
        );
        // Cleriga
        assert_eq!(
            (1..=6).map(|s| skill_index(6, s, 1)).collect::<Vec<_>>(),
            vec![4801, 4817, 4833, 4849, 4865, 4881]
        );
    }

    #[test]
    fn a_class_owns_nine_hundred_and_sixty_consecutive_ids() {
        assert_eq!(IDS_PER_CLASS, 960);
        assert!(belongs_to(CLASS, 1920) && belongs_to(CLASS, 2879));
        assert!(!belongs_to(CLASS, 1919), "the class before it");
        assert!(!belongs_to(CLASS, 2880), "the class after it");
    }

    /// Ranks are consecutive within a slot, so a rank is a step and not a
    /// lookup — with one wrinkle that is the original's and not ours.
    ///
    /// `GetSkillIndex` adds `Level` when the level is one and `Level - 1`
    /// otherwise, so ranks one and two land on the same id. Ranks three and
    /// up then trail the rank by one. Copying the arithmetic exactly matters
    /// more than tidying it: the ids in the shipped character templates are
    /// what this produces.
    #[test]
    fn ranks_sit_next_to_each_other_inside_a_slot() {
        let first = skill_index(CLASS, 1, 1);

        assert_eq!(skill_index(CLASS, 1, 2), first, "ranks one and two share an id");
        assert_eq!(skill_index(CLASS, 1, 3), first + 1);
        assert_eq!(skill_index(CLASS, 1, 16), first + 14);
        assert_eq!(skill_index(CLASS, 2, 1), first + RANKS_PER_SLOT);
    }

    /// The bar carries forty, and a level too low to have a slot leaves it
    /// out rather than sending an id that resolves to nothing.
    #[test]
    fn the_bar_holds_what_the_level_allows() {
        let t = table();

        let low = bar_of(&t, CLASS, 1);
        assert!(low.is_empty(), "a level 1 has none of the bar slots yet");

        let mid = bar_of(&t, CLASS, 40);
        assert_eq!(mid, vec![skill_index(CLASS, 7, 1), skill_index(CLASS, 8, 1)]);

        assert!(bar_of(&t, CLASS, 999).len() <= SKILL_SLOTS);
    }

    /// The six basic ones come before the bar, and everybody has them from
    /// the first level.
    #[test]
    fn the_basics_are_the_first_six_slots() {
        let t = table();
        let known = known_by(&t, CLASS, 1);

        assert_eq!(known.len(), BASIC_SLOTS);
        assert_eq!(known[0], skill_index(CLASS, 1, 1));
        assert_eq!(known[5], skill_index(CLASS, 6, 1));
    }

    #[test]
    fn a_cast_that_is_allowed_reports_what_it_costs() {
        let t = table();
        let id = skill_index(CLASS, 1, 1) as u32;
        let cast = check(
            &t,
            &caster(CLASS, 20, 100),
            &Cooldowns::new(),
            id,
            Some((110.0, 100.0)),
            Instant::now(),
        )
        .unwrap();

        assert_eq!(cast.mana, 10);
        assert_eq!(cast.damage, 100);
        assert_eq!(cast.cooldown, Duration::from_millis(3000));
        assert!(cast.is_aggressive);
    }

    /// Asking for a spell by a number outside your class is the obvious way
    /// to try to cast something you should not have.
    #[test]
    fn another_classs_id_is_refused() {
        let t = table();
        let id = skill_index(CLASS, 1, 1) as u32;

        assert_eq!(
            check(&t, &caster(1, 99, 999), &Cooldowns::new(), id, None, Instant::now()),
            Err(CastError::NotLearned(id)),
            "the first class cast the third class's spell"
        );
    }

    #[test]
    fn a_level_too_low_cannot_use_it() {
        let t = table();
        let id = skill_index(CLASS, 8, 1) as u32; // needs level 40
        assert_eq!(
            check(&t, &caster(CLASS, 3, 100), &Cooldowns::new(), id, None, Instant::now()),
            Err(CastError::NotLearned(id))
        );
    }

    #[test]
    fn a_skill_that_does_not_exist_is_refused() {
        let t = table();
        assert_eq!(
            check(&t, &caster(CLASS, 20, 100), &Cooldowns::new(), 2500, None, Instant::now()),
            Err(CastError::NoSuchSkill(2500)),
            "an empty slot inside the class block"
        );
    }

    #[test]
    fn not_enough_mana_says_how_much_short() {
        let t = table();
        let id = skill_index(CLASS, 1, 1) as u32;
        assert_eq!(
            check(&t, &caster(CLASS, 20, 4), &Cooldowns::new(), id, None, Instant::now()),
            Err(CastError::NotEnoughMana { needed: 10, held: 4 })
        );
    }

    #[test]
    fn a_target_out_of_the_skills_range_is_refused() {
        let t = table();
        let id = skill_index(CLASS, 1, 1) as u32;
        assert_eq!(
            check(
                &t,
                &caster(CLASS, 20, 100),
                &Cooldowns::new(),
                id,
                Some((9000.0, 9000.0)),
                Instant::now()
            ),
            Err(CastError::OutOfRange)
        );
    }

    #[test]
    fn a_skill_still_cooling_is_refused_and_says_how_long() {
        let t = table();
        let id = skill_index(CLASS, 1, 1) as u32;
        let now = Instant::now();
        let family = t.get(id as usize).unwrap().family();

        let mut cooldowns = Cooldowns::new();
        cooldowns.start(family, Duration::from_millis(3000), now);

        let err = check(&t, &caster(CLASS, 20, 100), &cooldowns, id, None, now).unwrap_err();
        assert!(matches!(err, CastError::Cooling { .. }), "got {err}");

        assert!(check(
            &t,
            &caster(CLASS, 20, 100),
            &cooldowns,
            id,
            None,
            now + Duration::from_millis(3001)
        )
        .is_ok());
    }

    /// Cooldowns are per spell, not per rank: a better rank of the same
    /// spell must not come up ready.
    #[test]
    fn a_better_rank_shares_the_cooldown_of_its_slot() {
        let t = table();
        let now = Instant::now();
        let rank1 = skill_index(CLASS, 1, 1) as u32;
        let rank3 = skill_index(CLASS, 1, 3) as u32;

        let mut cooldowns = Cooldowns::new();
        cooldowns.start(t.get(rank1 as usize).unwrap().family(), Duration::from_millis(3000), now);

        assert!(matches!(
            check(&t, &caster(CLASS, 20, 100), &cooldowns, rank3, None, now),
            Err(CastError::Cooling { .. })
        ));
    }

    #[test]
    fn a_cooldown_of_zero_never_holds_anything_up() {
        let now = Instant::now();
        let mut cooldowns = Cooldowns::new();
        cooldowns.start(5, Duration::ZERO, now);
        assert_eq!(cooldowns.remaining(5, now), None);
    }
}
