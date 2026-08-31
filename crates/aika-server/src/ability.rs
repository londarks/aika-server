//! What a character can do, and whether it may do it right now.
//!
//! `aika_data::skills` says what every skill is. This says which of them a
//! given character has, what using one costs, and how long it has to wait
//! before using it again.
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

/// The ids a character of this class and level can use, best rank of each
/// spell first, at most as many as the list packet carries.
///
/// # Class zero is not "everybody"
///
/// It reads that way and it is not. The 412 spells the file files under class
/// zero are monster abilities, siege weapon abilities and the effects items
/// grant when used — `Siege Boss AOE`, `BIG Head Potion`, `Plain Verband
/// Soup`. A player's own skills always carry their class, basic attack
/// included: `Attack` is class 21 for the third class and class 11 for the
/// second.
///
/// Treating class zero as common filled thirty-four of the forty slots on the
/// skill bar with soup.
pub fn known_by(table: &SkillTable, base_class: u32, level: u32) -> Vec<usize> {
    let mut best: HashMap<u32, (usize, u32)> = HashMap::new();

    for (id, skill) in table.defined() {
        if skill.base_class() != Some(base_class) || skill.min_level() > level {
            continue;
        }
        // One entry per spell, the highest rank the level allows.
        let slot = best.entry(skill.family()).or_insert((id, skill.rank()));
        if skill.rank() >= slot.1 {
            *slot = (id, skill.rank());
        }
    }

    let mut ids: Vec<usize> = best.into_values().map(|(id, _)| id).collect();
    ids.sort_unstable();
    ids.truncate(SKILL_SLOTS);
    ids
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
    pub base_class: u32,
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

    // Same rule as `known_by`: a player's skills carry their class, and the
    // class-zero pile belongs to monsters and items.
    if skill.base_class() != Some(caster.base_class) || skill.min_level() > caster.level {
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

    /// A handful of skills, so the tests do not need the 8 MB file.
    fn table() -> SkillTable {
        let mut raw = vec![0u8; 40 * RECORD_SIZE];

        let mut define = |id: usize,
                          family: u32,
                          rank: u32,
                          min_level: u32,
                          class: u32,
                          mana: u32,
                          damage: u32,
                          range: u32,
                          cooldown: u32| {
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
            put(r, field::COOLDOWN, cooldown);
            put(r, field::AGGRESSIVE, 1);
            r[field::NAME_ENGLISH.start] = b'x';
        };

        // class zero: a monster or item effect, never a player's
        define(1, 0, 1, 1, 0, 0, 1, 0, 400);
        define(2, 0, 2, 10, 0, 0, 1, 0, 400);
        define(17, 5, 1, 4, 21, 10, 120, 300, 3000); // class 2 only
        define(18, 5, 2, 14, 21, 18, 260, 300, 3000);
        define(30, 9, 1, 1, 51, 5, 40, 200, 1000); // class 5 only

        SkillTable::decode(&raw).expect("the fixture table is malformed")
    }

    fn caster(base_class: u32, level: u32, mana: u32) -> Caster {
        Caster { base_class, level, mana, at: (100.0, 100.0) }
    }

    #[test]
    fn use_skill_body_roundtrip() {
        let original = UseSkill { skill: 17, target: 3048, at: (100.0, 200.0) };
        assert_eq!(UseSkill::parse(&original.to_body()), Some(original));
        assert_eq!(UseSkill::parse(&[0u8; 8]), None);
    }

    /// A character has the best rank of each spell its class and level allow,
    /// and nothing from anyone else's class.
    #[test]
    fn what_a_character_knows_is_the_best_rank_of_each_of_its_spells() {
        let t = table();

        assert!(known_by(&t, 2, 1).is_empty(), "nothing is learnable at level 1");
        assert_eq!(known_by(&t, 2, 12), vec![17], "rank 1 of the class spell");
        assert_eq!(known_by(&t, 2, 20), vec![18], "the better rank replaces the earlier one");
        assert_eq!(known_by(&t, 5, 20), vec![30], "another class gets its own");
        assert!(known_by(&t, 9, 20).is_empty(), "a class with nothing of its own");

        // and the class-zero pile never reaches anybody
        assert!(
            !known_by(&t, 2, 999).contains(&1),
            "a class-zero entry reached the bar; those are monster and item effects"
        );
    }

    /// The list packet holds forty, so the answer must never be longer.
    #[test]
    fn the_list_never_outgrows_the_packet() {
        let t = table();
        assert!(known_by(&t, 2, 999).len() <= SKILL_SLOTS);
    }

    /// A class-zero skill is not the player's, whatever their class.
    #[test]
    fn nobody_can_cast_a_class_zero_skill() {
        let t = table();
        assert_eq!(
            check(&t, &caster(0, 99, 999), &Cooldowns::new(), 1, None, Instant::now()),
            Err(CastError::NotLearned(1))
        );
    }

    #[test]
    fn a_cast_that_is_allowed_reports_what_it_costs() {
        let t = table();
        let cast = check(
            &t,
            &caster(2, 20, 100),
            &Cooldowns::new(),
            17,
            Some((110.0, 100.0)),
            Instant::now(),
        )
        .unwrap();

        assert_eq!(cast.mana, 10);
        assert_eq!(cast.damage, 120);
        assert_eq!(cast.cooldown, Duration::from_millis(3000));
        assert_eq!(cast.family, 5);
        assert!(cast.is_aggressive);
    }

    #[test]
    fn another_class_cannot_use_it() {
        let t = table();
        assert_eq!(
            check(&t, &caster(5, 20, 100), &Cooldowns::new(), 17, None, Instant::now()),
            Err(CastError::NotLearned(17))
        );
    }

    #[test]
    fn a_level_too_low_cannot_use_it() {
        let t = table();
        assert_eq!(
            check(&t, &caster(2, 3, 100), &Cooldowns::new(), 17, None, Instant::now()),
            Err(CastError::NotLearned(17))
        );
    }

    #[test]
    fn a_skill_that_does_not_exist_is_refused() {
        let t = table();
        assert_eq!(
            check(&t, &caster(2, 20, 100), &Cooldowns::new(), 999, None, Instant::now()),
            Err(CastError::NoSuchSkill(999))
        );
    }

    #[test]
    fn not_enough_mana_says_how_much_short() {
        let t = table();
        assert_eq!(
            check(&t, &caster(2, 20, 4), &Cooldowns::new(), 17, None, Instant::now()),
            Err(CastError::NotEnoughMana { needed: 10, held: 4 })
        );
    }

    #[test]
    fn a_target_out_of_the_skills_range_is_refused() {
        let t = table();
        let far = check(
            &t,
            &caster(2, 20, 100),
            &Cooldowns::new(),
            17,
            Some((9000.0, 9000.0)),
            Instant::now(),
        );
        assert_eq!(far, Err(CastError::OutOfRange));
    }

    /// A skill whose range column reads zero must not become a way to hit
    /// anything anywhere.
    #[test]
    fn a_skill_with_no_range_still_has_to_reach() {
        let t = table();
        let now = Instant::now();

        // skill 30 has a range of 200; from 700 away it cannot reach
        assert!(check(&t, &caster(5, 20, 50), &Cooldowns::new(), 30, Some((105.0, 100.0)), now).is_ok());
        assert_eq!(
            check(&t, &caster(5, 20, 50), &Cooldowns::new(), 30, Some((900.0, 100.0)), now),
            Err(CastError::OutOfRange)
        );
    }

    #[test]
    fn a_skill_still_cooling_is_refused_and_says_how_long() {
        let t = table();
        let now = Instant::now();
        let mut cooldowns = Cooldowns::new();
        cooldowns.start(5, Duration::from_millis(3000), now);

        let err = check(&t, &caster(2, 20, 100), &cooldowns, 17, None, now).unwrap_err();
        assert!(matches!(err, CastError::Cooling { .. }), "got {err}");

        // and it is usable again once the time is up
        assert!(check(
            &t,
            &caster(2, 20, 100),
            &cooldowns,
            17,
            None,
            now + Duration::from_millis(3001)
        )
        .is_ok());
    }

    /// Cooldowns are per spell, not per rank: learning a better rank must not
    /// hand somebody a fresh one.
    #[test]
    fn a_better_rank_shares_the_cooldown_of_its_spell() {
        let t = table();
        let now = Instant::now();
        let mut cooldowns = Cooldowns::new();

        // rank 1 was cast, so rank 2 of the same spell is cooling too
        cooldowns.start(t.get(17).unwrap().family(), Duration::from_millis(3000), now);
        assert!(matches!(
            check(&t, &caster(2, 20, 100), &cooldowns, 18, None, now),
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
