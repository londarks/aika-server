//! What buffs and worn gear add on top of a character.
//!
//! The original keeps one flat array, `MOB_EF`, indexed by an effect number,
//! and `GetMobAbility(n)` is a lookup into it and nothing else
//! (`Mob/BaseMob.pas:3162`). Two things fill it:
//!
//! - `AddBuffEffect` copies the four pairs a *skill* carries, straight out of
//!   the skill table (`Mob/BaseMob.pas:4010`);
//! - `SetEquipEffect` adds the three pairs an *item instance* carries, each
//!   one **doubled**, and skips prans and anything expired
//!   (`Mob/BaseMob.pas:1900`).
//!
//! Nothing here interprets an effect. The array is a vocabulary of nearly four
//! hundred numbers; what any one of them means is decided where it is read,
//! which for now is [`crate::stats`] working through `GetCurrentScore`.
//!
//! # The cap that never fires
//!
//! `AddBuffEffect` looks like it clamps run speed at thirteen:
//!
//! ```text
//! if (i = EF_RUNSPEED) and (MOB_EF[EF_RUNSPEED] + EFV[i] >= 13) then ...
//! ```
//!
//! `i` is the loop counter, nought to three, and `EF_RUNSPEED` is forty-six,
//! so the test is never true and the `else` always runs. Copied as it behaves,
//! not as it reads: a mount really does add its whole thirty.

use crate::buffs::Buffs;
use crate::inventory::Inventory;
use crate::store::Character;
use aika_data::itemlist::ItemList;
use aika_data::skills::SkillTable;
use std::collections::BTreeMap;

/// The effect numbers this server reads, from `Data/GlobalDefs.pas`. The file
/// defines close to four hundred; these are the ones `GetCurrentScore` asks
/// for, which is what makes them worth naming.
pub mod id {
    pub const DAMAGE1: u16 = 2;
    pub const DAMAGE2: u16 = 3;
    pub const RESISTANCE1: u16 = 8;
    pub const RESISTANCE2: u16 = 9;
    pub const STR: u16 = 15;
    pub const DEX: u16 = 16;
    pub const INT: u16 = 17;
    pub const CON: u16 = 18;
    pub const SPI: u16 = 19;
    pub const RESISTANCE6: u16 = 20;
    pub const RESISTANCE7: u16 = 21;
    pub const CRITICAL_POWER: u16 = 31;
    pub const RUNSPEED: u16 = 46;
    pub const DOUBLE: u16 = 49;
    pub const CRITICAL: u16 = 50;
    pub const PARRY: u16 = 51;
    pub const HIT: u16 = 53;
    pub const PER_DAMAGE1: u16 = 54;
    pub const PER_DAMAGE2: u16 = 55;
    pub const PER_RESISTANCE1: u16 = 59;
    pub const PER_RESISTANCE2: u16 = 60;
    pub const SKILL_DAMAGE: u16 = 66;
    pub const STATE_RESISTANCE: u16 = 80;
    pub const PIERCING_RESISTANCE1: u16 = 86;
    pub const PIERCING_RESISTANCE2: u16 = 87;
    pub const CRITICAL_DEFENCE: u16 = 89;
    pub const UNARMOR: u16 = 159;
    pub const PRAN_DAMAGE1: u16 = 182;
    pub const PRAN_DAMAGE2: u16 = 183;
    pub const PRAN_RESISTANCE1: u16 = 186;
    pub const PRAN_RESISTANCE2: u16 = 187;
    pub const PRAN_PARRY: u16 = 193;
    pub const DECREASE_PER_DAMAGE1: u16 = 319;
    pub const DECREASE_PER_DAMAGE2: u16 = 320;
}

/// Prans are worn but contribute nothing through this path: `SetEquipEffect`
/// returns immediately for item type 10.
const ITEM_TYPE_PRAN: u16 = 10;

/// Everything currently adding to a character, by effect number.
///
/// Sparse: a character has a handful of effects and the numbers run to nearly
/// four hundred, so a map costs less than an array and reads the same.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    table: BTreeMap<u16, i32>,
}

impl Effects {
    /// Nothing at all, which is what a character with no buffs and bare hands
    /// has. Every formula falls back to its plain form.
    pub fn none() -> Self {
        Self::default()
    }

    /// Everything working on this character: its buffs and what it wears.
    pub fn of(
        character: &Character,
        items: &ItemList,
        buffs: &Buffs,
        skills: &SkillTable,
    ) -> Self {
        let mut out = Self::default();
        out.add_buffs(buffs, skills);
        out.add_worn(&character.items, items);
        out
    }

    /// `AddBuffEffect`: the four pairs of every running buff.
    pub fn add_buffs(&mut self, buffs: &Buffs, skills: &SkillTable) {
        for (skill, _) in buffs.running(skills) {
            let Some(def) = skills.get(skill) else { continue };
            for (effect, value) in def.effects() {
                self.add(effect, value);
            }
        }
    }

    /// `SetEquipEffect`: the three pairs each worn piece carries, doubled.
    ///
    /// They come from the *item*, not from the item table: two swords of the
    /// same id can carry different effects, which is what enchanting is for.
    pub fn add_worn(&mut self, worn: &Inventory, items: &ItemList) {
        for item in worn.in_container(crate::inventory::EQUIP) {
            let is_pran = items
                .get(item.index as usize)
                .is_some_and(|def| def.item_type() == ITEM_TYPE_PRAN);
            if is_pran {
                continue;
            }
            for i in 0..3 {
                let effect = item.effect_index[i] as i32;
                if effect > 0 {
                    self.add(effect, item.effect_value[i] as i32 * 2);
                }
            }
        }
    }

    fn add(&mut self, effect: i32, value: i32) {
        if effect <= 0 || effect > u16::MAX as i32 {
            return;
        }
        *self.table.entry(effect as u16).or_insert(0) += value;
    }

    /// `GetMobAbility`: what this effect is worth, or nothing.
    pub fn get(&self, effect: u16) -> i32 {
        self.table.get(&effect).copied().unwrap_or(0)
    }

    /// The same as a positive number, which is what every formula that adds
    /// one wants. A debuff that drove a total below zero would otherwise
    /// subtract from an unsigned field and wrap.
    pub fn plus(&self, effect: u16) -> u32 {
        self.get(effect).max(0) as u32
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Item;
    use aika_data::itemlist::{field as ifield, RECORD_SIZE as ITEM_RECORD};
    use aika_data::skills::{field as sfield, RECORD_SIZE as SKILL_RECORD, SLOTS};

    /// The real saddle: run speed thirty, which is what a mount is for.
    const SADDLE_SKILL: usize = 7259;
    const RUNSPEED_GIVEN: i32 = 30;

    fn skills() -> SkillTable {
        let mut raw = vec![0u8; SLOTS * SKILL_RECORD + 4];
        let r = &mut raw[SADDLE_SKILL * SKILL_RECORD..(SADDLE_SKILL + 1) * SKILL_RECORD];
        r[sfield::FAMILY..sfield::FAMILY + 4]
            .copy_from_slice(&crate::buffs::FAMILY_MOUNTED.to_le_bytes());
        r[sfield::DURATION..sfield::DURATION + 4].copy_from_slice(&3600u32.to_le_bytes());
        // Two of the four pairs used, as the real one has them.
        r[sfield::EFFECT.start..sfield::EFFECT.start + 4].copy_from_slice(&262u32.to_le_bytes());
        r[sfield::EFFECT_VALUE.start..sfield::EFFECT_VALUE.start + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        r[sfield::EFFECT.start + 4..sfield::EFFECT.start + 8]
            .copy_from_slice(&(id::RUNSPEED as u32).to_le_bytes());
        r[sfield::EFFECT_VALUE.start + 4..sfield::EFFECT_VALUE.start + 8]
            .copy_from_slice(&(RUNSPEED_GIVEN as u32).to_le_bytes());
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    }

    fn items() -> ItemList {
        let mut raw = vec![0u8; 100 * ITEM_RECORD];
        let mut define = |id: usize, item_type: u16| {
            let r = &mut raw[id * ITEM_RECORD..(id + 1) * ITEM_RECORD];
            r[ifield::NAME.start] = b'x';
            r[ifield::ITEM_TYPE..ifield::ITEM_TYPE + 2].copy_from_slice(&item_type.to_le_bytes());
        };
        define(10, 3); // armour
        define(20, ITEM_TYPE_PRAN);
        ItemList::decode(&raw).expect("the fixture table is malformed")
    }

    fn worn(index: u16, slot: u16, effect: u8, value: u8) -> Item {
        Item {
            container: crate::inventory::EQUIP,
            slot,
            index,
            effect_index: [effect, 0, 0],
            effect_value: [value, 0, 0],
            ..Item::default()
        }
    }

    /// A mount's speed is an effect and nothing else. This is the one the
    /// player feels.
    #[test]
    fn a_saddle_gives_its_run_speed() {
        let skills = skills();
        let mut buffs = Buffs::new();
        buffs.add(&skills, SADDLE_SKILL, std::time::SystemTime::now());

        let mut effects = Effects::none();
        effects.add_buffs(&buffs, &skills);

        assert_eq!(effects.get(id::RUNSPEED), RUNSPEED_GIVEN);
        assert_eq!(effects.get(262), 1, "the other pair went missing");
        assert_eq!(effects.get(id::CRITICAL), 0, "an effect nobody gave");
    }

    /// Worn gear counts double, which is the original's own arithmetic.
    #[test]
    fn a_worn_piece_counts_twice_over() {
        let mut character = crate::store::Character::from(&crate::config::DevCharacter {
            name: "x".into(),
            slot: 0,
            level: 1,
            class_index: 10,
            hair: 7700,
            nation: 2,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        });
        character.items.put(worn(10, 3, id::CRITICAL as u8, 7)).unwrap();

        let mut effects = Effects::none();
        effects.add_worn(&character.items, &items());

        assert_eq!(effects.get(id::CRITICAL), 14, "seven twice is what the original adds");
    }

    /// A pran is worn and contributes nothing through this path.
    #[test]
    fn a_pran_adds_nothing() {
        let mut character = crate::store::Character::from(&crate::config::DevCharacter {
            name: "x".into(),
            slot: 0,
            level: 1,
            class_index: 10,
            hair: 7700,
            nation: 2,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        });
        character.items.put(worn(20, 10, id::CRITICAL as u8, 7)).unwrap();

        let mut effects = Effects::none();
        effects.add_worn(&character.items, &items());

        assert!(effects.is_empty(), "a pran was counted as gear");
    }
}
