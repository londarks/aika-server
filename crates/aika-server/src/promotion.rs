//! Class promotion: the tier a character is at, and how far it may level.
//!
//! `ClassInfo` is one byte holding two things: the class in the tens and the
//! tier in the units, which is why `GetMobClass` is a `div 10`
//! (`Mob/BaseMob.pas:2064`). A Guerreiro is 1; 2 and 3 are the same class
//! further along, and the client names each of the three differently.
//!
//! # What the live game did
//!
//! Promotion was a chain of quests, and each one lifted a wall.
//!
//! A character stopped dead at level 50. With a Pran equipped and standing in
//! its own nation it took the first chain from a trainer, came back renamed --
//! a Templaria returned a Paladino -- and could then reach 89. The second
//! chain began with Lilola Hawn, went through Nick Lily to Moa Chrost in
//! Termes, and ended by gathering nine Certificates of Glory; finishing it
//! granted "ten more levels, without passing 99".
//!
//! Both walls are visible in the data rather than only in what players wrote
//! down. The tier-two skills start at level 51, one level the far side of the
//! first wall. The tier-three ones are stranger and just as clear: every one
//! of them is a *seventeenth* rank of a skill that only has sixteen, and every
//! one asks for level 99.
//!
//! # What the original server did, which is nothing
//!
//! It never advances the digit. The six templates set 1, 11, 21, 31, 41 and
//! 51, the login loads the column, the save writes it back, and no handler,
//! quest or operator command in the whole source assigns it. Its quest system
//! is half a system -- `GetQuest` accepts and `FinishQuest` pays out rewards
//! and prans, and neither so much as reads `ClassInfo` -- and there is no
//! level cap anywhere either. So a character there levelled to 99 still
//! called what it was called on the day it was made.
//!
//! # What this does until the quests exist
//!
//! The walls are real here: [`level_cap`] stops a character at its tier's
//! ceiling, and [`Promotion`] moves the tier. What is missing is the chain
//! that ought to grant it, so for now the quest option on an NPC that offers
//! quests grants it instead, once the character is standing at the wall.
//!
//! That is deliberately the loosest part and the first to be replaced. When
//! quests and prans land, the requirement becomes finishing the chain, and the
//! only thing that changes here is [`Promotion::offered`] -- the tier, the
//! ceilings and everything reading them stay as they are.

/// The tier every character is created at.
pub const FIRST_TIER: u16 = 1;

/// The highest tier the skill table has anything for.
pub const LAST_TIER: u16 = 3;

/// The level a character of each tier cannot pass, tier one first.
///
/// 50 and 89 are the two promotion walls. 99 is the end of the road: it is
/// where the third chain was said to leave a character ("ten more levels,
/// without passing 99"), and it is what the rest of the data agrees with.
/// `ExpList.bin` holds a hundred, but the item table stops at ninety-nine --
/// a saddle is `10..99`, and the best earned gear of every class is the tier
/// at ninety-six. A character past ninety-nine finds its own mount refused,
/// because that range is enforced by the client and the client will not
/// budge. So the curve is allowed to run out one short of the file.
const CEILINGS: [u16; LAST_TIER as usize] = [50, 89, 99];

/// How far a character of this tier may level.
///
/// A tier past the last behaves like the last rather than lifting the ceiling
/// entirely, so a bad value in the database cannot hand out a level 65535.
pub fn level_cap(tier: u16) -> u16 {
    let index = tier.clamp(FIRST_TIER, LAST_TIER) as usize - 1;
    CEILINGS[index]
}

/// The tier a character at this level must already have reached.
///
/// The lowest tier whose ceiling can hold the level: 50 is still tier one,
/// 51 has to be tier two, 90 has to be tier three. Used to fill the column in
/// for characters that predate it, who levelled when nothing stopped them and
/// whose level is now the only evidence of how far they got.
pub fn tier_for_level(level: u16) -> u16 {
    (FIRST_TIER..=LAST_TIER).find(|&tier| level <= level_cap(tier)).unwrap_or(LAST_TIER)
}

/// Why a character may not be promoted yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Already as far along as the data goes.
    NothingFurther,
    /// Not at the wall yet, and the level it takes to reach it.
    NotThereYet { at: u16 },
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::NothingFurther => "There is nothing further for you here.".into(),
            Refusal::NotThereYet { at } => {
                format!("Come back when you have reached level {at}.")
            }
        }
    }
}

/// A promotion a character has earned: which tier it moves to, and how far
/// that lets it level afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Promotion {
    pub tier: u16,
    pub level_cap: u16,
}

impl Promotion {
    /// Whether a character of this tier standing at this level has earned the
    /// next one.
    ///
    /// The level test is the whole test, which is the stand-in: what belongs
    /// here is the quest chain having been finished, and a Pran. Being *at*
    /// the ceiling is what counts rather than being past it, because the
    /// ceiling is exactly where a character who has not been promoted stops.
    pub fn offered(tier: u16, level: u16) -> Result<Self, Refusal> {
        let tier = tier.max(FIRST_TIER);
        if tier >= LAST_TIER {
            return Err(Refusal::NothingFurther);
        }
        let wall = level_cap(tier);
        if level < wall {
            return Err(Refusal::NotThereYet { at: wall });
        }
        Ok(Self { tier: tier + 1, level_cap: level_cap(tier + 1) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tier_has_its_own_ceiling() {
        assert_eq!(level_cap(1), 50);
        assert_eq!(level_cap(2), 89);
        assert_eq!(level_cap(3), 99);
    }

    /// A tier the data does not have must not lift the ceiling. A zero or a
    /// number out of the blue is a database that has been edited by hand,
    /// which is exactly how tiers were set on the original.
    #[test]
    fn a_tier_outside_the_three_is_pinned_to_them() {
        assert_eq!(level_cap(0), 50, "below the first");
        assert_eq!(level_cap(9), 99, "above the last");
        assert_eq!(level_cap(u16::MAX), 99);
    }

    #[test]
    fn the_level_says_which_tier_a_character_must_have_had() {
        assert_eq!(tier_for_level(1), 1);
        assert_eq!(tier_for_level(50), 1, "fifty is still the first tier");
        assert_eq!(tier_for_level(51), 2, "past fifty it had to have promoted");
        assert_eq!(tier_for_level(89), 2);
        assert_eq!(tier_for_level(90), 3);
        assert_eq!(tier_for_level(99), 3);
    }

    /// Every level the curve has must land on a tier that can hold it, or a
    /// character would load with a cap below its own level.
    #[test]
    fn no_level_seeds_a_tier_too_small_to_hold_it() {
        for level in 1..=99u16 {
            let tier = tier_for_level(level);
            assert!(
                level <= level_cap(tier),
                "level {level} seeded tier {tier}, whose cap is {}",
                level_cap(tier)
            );
        }
    }

    #[test]
    fn the_wall_is_where_the_promotion_is_offered() {
        assert_eq!(Promotion::offered(1, 49), Err(Refusal::NotThereYet { at: 50 }));
        assert_eq!(Promotion::offered(1, 50), Ok(Promotion { tier: 2, level_cap: 89 }));
        assert_eq!(Promotion::offered(2, 88), Err(Refusal::NotThereYet { at: 89 }));
        assert_eq!(Promotion::offered(2, 89), Ok(Promotion { tier: 3, level_cap: 99 }));
    }

    #[test]
    fn there_is_no_fourth_tier() {
        assert_eq!(Promotion::offered(3, 99), Err(Refusal::NothingFurther));
        assert_eq!(Promotion::offered(9, 99), Err(Refusal::NothingFurther));
    }

    /// The two together have to actually let a character climb: promoted at
    /// each wall, a character reaches 99 and stops there.
    #[test]
    fn promoting_at_each_wall_reaches_the_end_of_the_curve() {
        let mut tier = FIRST_TIER;
        let mut walls = Vec::new();
        loop {
            let level = level_cap(tier);
            match Promotion::offered(tier, level) {
                Ok(next) => {
                    walls.push(level);
                    tier = next.tier;
                }
                Err(Refusal::NothingFurther) => break,
                Err(other) => panic!("standing at the wall and refused: {other:?}"),
            }
        }
        assert_eq!(walls, vec![50, 89], "the walls a promotion is needed to pass");
        assert_eq!(tier, LAST_TIER);
        assert_eq!(level_cap(tier), 99, "and the last tier reaches the end of the curve");
    }
}
