//! What a character is worth in a fight.
//!
//! Two sources: the attributes it was born with and levelled into, and the
//! gear it is wearing. The item table already carries every piece's attack,
//! defence and the rest; this adds them up.
//!
//! # What is and is not the original's
//!
//! The gear is read exactly as the table gives it. The *formula* that turns
//! attack and defence into a number of points off a target is not the
//! original's: `GetDamage` runs its inputs through critical, block, immune
//! and miss tables — thirty-three outcomes — and several of the inputs are
//! buffs and resistances this server does not keep yet. What is here uses the
//! real attack and the real defence and reads as a fight; when the missing
//! systems land, this is the one function to replace.

use crate::inventory::Inventory;
use crate::store::Character;
use aika_data::itemlist::ItemList;

/// Equipment slots that carry gear. Slot 0 is the body and slot 1 the hair,
/// which are appearance and not armour.
const GEAR_SLOTS: std::ops::Range<u16> = 2..16;

/// Attack comes from the weapon and from nothing else
/// (`GetEquipDamage(Equip[6])`), and armour from slots two to seven with the
/// weapon skipped (`GetEquipsDefense`). Rings and the rest carry their worth
/// as effects, which is a system this server does not keep yet.
const WEAPON_SLOT: u16 = 6;
const ARMOUR_SLOTS: std::ops::RangeInclusive<u16> = 2..=7;

/// What a point of each attribute is worth, straight out of `GetCurrentScore`
/// (`Mob/BaseMob.pas:3457`). They were guesses before — two points of attack
/// to a point of strength — because the file that owns them had not been read.
/// Every one of them is truncated, as the original truncates, except the
/// resistance, which it rounds.
const STRENGTH_TO_ATTACK: f32 = 2.6;
const AGILITY_TO_ATTACK: f32 = 2.6;
const INTELLECT_TO_MAGIC: f32 = 3.2;
const AGILITY_TO_CRITICAL: f32 = 0.13;
const AGILITY_TO_ACCURACY: f32 = 0.5;
const AGILITY_TO_DODGE: f32 = 0.021;
const STRENGTH_TO_DOUBLE: f32 = 0.21;
const LUCK_TO_RESISTANCE: f32 = 0.1;

/// Movement speed, which the original does not read off the character at all:
/// it starts from forty and adds what effects say (`IncSpeedMove(SpeedMove,
/// 40 + GetMobAbility(EF_RUNSPEED))`).
pub const BASE_SPEED_MOVE: u16 = 40;

/// Health and mana a level is worth, on top of what the character starts
/// with.
const HP_PER_LEVEL: u32 = 10;
const MP_PER_LEVEL: u32 = 5;
const BASE_HP: u32 = 100;
const BASE_MP: u32 = 50;

/// Everything a fight needs to know about somebody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub attack: u32,
    pub magic_attack: u32,
    pub defence: u32,
    pub magic_defence: u32,
    /// The five the character sheet shows beside the four above and that
    /// nothing computed before, so the window drew them all as zero.
    pub critical: u32,
    pub accuracy: u32,
    pub dodge: u32,
    pub double_attack: u32,
    pub resistance: u32,
    pub max_hp: u32,
    pub max_mp: u32,
}

/// Adds up what a character is wearing and what it is made of.
///
/// The arithmetic is `GetCurrentScore`'s: the weapon's own attack plus
/// strength and agility, the armour's defence, and five more worked out from
/// agility, strength and luck alone. What is missing from it is the effects —
/// `GetMobAbility` reads the buffs, pran and relic bonuses this server does
/// not keep — so every line here is the original's minus its effect term.
pub fn of(character: &Character, items: &ItemList) -> Stats {
    let gear = gear_of(&character.items, items);
    let weapon = weapon_of(&character.items, items);
    let armour = armour_of(&character.items, items);

    let [strength, agility, intellect, _constitution, luck, _free] = character.attributes;
    let (strength, agility, intellect, luck) =
        (strength as f32, agility as f32, intellect as f32, luck as f32);
    let level = character.level as u32;

    Stats {
        attack: weapon.0 + (strength * STRENGTH_TO_ATTACK) as u32
            + (agility * AGILITY_TO_ATTACK) as u32,
        magic_attack: weapon.1 + (intellect * INTELLECT_TO_MAGIC) as u32,
        defence: armour.0,
        magic_defence: armour.1,
        critical: (agility * AGILITY_TO_CRITICAL) as u32,
        accuracy: (agility * AGILITY_TO_ACCURACY) as u32,
        dodge: (agility * AGILITY_TO_DODGE) as u32,
        double_attack: (strength * STRENGTH_TO_DOUBLE) as u32,
        resistance: (luck * LUCK_TO_RESISTANCE).round() as u32,
        // Health and mana are still ours: the original grows them from tables
        // this has not read.
        max_hp: BASE_HP + level * HP_PER_LEVEL + gear.hp,
        max_mp: BASE_MP + level * MP_PER_LEVEL + gear.mp,
    }
}

/// The physical and magical attack of the weapon in hand, or nothing when
/// there is none.
///
/// One slot, not a sum across the gear: the original passes `Equip[6]` alone.
/// A piece with no durability left is skipped, which is the original's
/// `if Equip.MIN = 0 then Exit`.
fn weapon_of(inventory: &Inventory, items: &ItemList) -> (u32, u32) {
    let Some(worn) = inventory.get(crate::inventory::EQUIP, WEAPON_SLOT) else {
        return (0, 0);
    };
    if worn.durability_min == 0 {
        return (0, 0);
    }
    let Some(def) = items.get(worn.index as usize) else {
        return (0, 0);
    };
    (def.attack() as u32, def.magic_attack() as u32)
}

/// The physical and magical defence of the armour, which is slots two to
/// seven with the weapon skipped.
fn armour_of(inventory: &Inventory, items: &ItemList) -> (u32, u32) {
    let mut defence = (0, 0);
    for slot in ARMOUR_SLOTS {
        if slot == WEAPON_SLOT {
            continue;
        }
        let Some(worn) = inventory.get(crate::inventory::EQUIP, slot) else {
            continue;
        };
        if worn.durability_min == 0 {
            continue;
        }
        let Some(def) = items.get(worn.index as usize) else {
            continue;
        };
        defence.0 += def.defense() as u32;
        defence.1 += def.magic_defense() as u32;
    }
    defence
}

/// What the worn gear alone is worth.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Gear {
    pub attack: u32,
    pub magic_attack: u32,
    pub defence: u32,
    pub magic_defence: u32,
    pub hp: u32,
    pub mp: u32,
}

pub fn gear_of(inventory: &Inventory, items: &ItemList) -> Gear {
    let mut gear = Gear::default();

    for slot in GEAR_SLOTS {
        let Some(worn) = inventory.get(crate::inventory::EQUIP, slot) else {
            continue;
        };
        let Some(def) = items.get(worn.index as usize) else {
            continue;
        };
        gear.attack += def.attack() as u32;
        gear.magic_attack += def.magic_attack() as u32;
        gear.defence += def.defense() as u32;
        gear.magic_defence += def.magic_defense() as u32;
        gear.hp += def.hp() as u32;
        gear.mp += def.mp() as u32;
    }
    gear
}

/// What one blow takes off, before any roll.
///
/// Defence subtracts rather than divides, with a floor: a target you cannot
/// hurt at all is indistinguishable from a broken server, and the original
/// keeps a floor under its damage for the same reason.
pub fn base_damage(attack: u32, defence: u32) -> u32 {
    attack.saturating_sub(defence / 2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DevCharacter;
    use crate::store::Item;
    use aika_data::itemlist::{field, RECORD_SIZE};

    fn item_table() -> ItemList {
        let mut raw = vec![0u8; 5000 * RECORD_SIZE];
        let mut define = |id: usize, atk: u16, def: u16, matk: u16, hp: u16| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::ATTACK..field::ATTACK + 2].copy_from_slice(&atk.to_le_bytes());
            r[field::DEFENSE..field::DEFENSE + 2].copy_from_slice(&def.to_le_bytes());
            r[field::MAGIC_ATTACK..field::MAGIC_ATTACK + 2].copy_from_slice(&matk.to_le_bytes());
            r[field::HP..field::HP + 2].copy_from_slice(&hp.to_le_bytes());
        };
        define(1000, 120, 0, 0, 0); // a sword
        define(2000, 0, 80, 0, 50); // a breastplate
        define(3000, 0, 0, 200, 0); // a staff
        ItemList::decode(&raw).expect("the fixture table is malformed")
    }

    fn character(attributes: [u16; 6], level: u16) -> Character {
        let mut c = Character::from(&DevCharacter {
            name: "x".into(),
            slot: 0,
            level,
            class_index: 20,
            hair: 7700,
            nation: 2,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        });
        c.attributes = attributes;
        c
    }

    fn wearing(character: &mut Character, slot: u16, index: u16) {
        character
            .items
            .put(Item {
                index,
                container: crate::inventory::EQUIP,
                slot,
                // Worn gear has durability left; without it the original
                // counts the piece for nothing.
                durability_min: 255,
                ..Item::default()
            })
            .unwrap();
    }

    #[test]
    fn gear_adds_up_across_the_slots() {
        let items = item_table();
        let mut c = character([10, 10, 10, 10, 10, 0], 1);
        wearing(&mut c, 6, 1000);
        wearing(&mut c, 2, 2000);

        let gear = gear_of(&c.items, &items);
        assert_eq!(gear.attack, 120);
        assert_eq!(gear.defence, 80);
        assert_eq!(gear.hp, 50);
    }

    /// The body and the hair are appearance, not armour. Counting them would
    /// give every character the stats of whatever its class model happens to
    /// collide with in the item table.
    #[test]
    fn the_body_and_the_hair_are_not_gear() {
        let items = item_table();
        let mut c = character([10, 10, 10, 10, 10, 0], 1);
        wearing(&mut c, 0, 1000);
        wearing(&mut c, 1, 1000);

        assert_eq!(gear_of(&c.items, &items), Gear::default());
    }

    /// The arithmetic is `GetCurrentScore`'s and is pinned here rather than
    /// described, because every one of these numbers was a guess before.
    #[test]
    fn the_numbers_are_the_originals() {
        let items = item_table();
        let mut c = character([20, 40, 5, 30, 25, 0], 10);
        wearing(&mut c, 6, 1000);
        wearing(&mut c, 2, 2000);

        let s = of(&c, &items);
        assert_eq!(s.attack, 120 + 52 + 104, "weapon, then strength and agility at 2.6");
        assert_eq!(s.magic_attack, 16, "five intellect at 3.2");
        assert_eq!(s.defence, 80, "the breastplate, and nothing from the attributes");
        assert_eq!(s.critical, 5, "forty agility at 0.13");
        assert_eq!(s.accuracy, 20, "forty agility at a half");
        assert_eq!(s.dodge, 0, "forty agility at 0.021 truncates to nothing");
        assert_eq!(s.double_attack, 4, "twenty strength at 0.21");
        assert_eq!(s.resistance, 3, "twenty-five luck at a tenth, rounded");
        assert_eq!(s.max_hp, BASE_HP + 10 * HP_PER_LEVEL + 50);
    }

    /// Attack is the weapon's alone. A sword worn on the head is not a sword.
    #[test]
    fn only_the_weapon_slot_carries_attack() {
        let items = item_table();
        let mut c = character([0, 0, 0, 0, 0, 0], 1);
        wearing(&mut c, 8, 1000);

        assert_eq!(of(&c, &items).attack, 0, "a sword in the wrong slot armed the character");

        wearing(&mut c, 6, 1000);
        assert_eq!(of(&c, &items).attack, 120);
    }

    /// A piece worn down to nothing is worth nothing, which is the original's
    /// `if Equip.MIN = 0 then Exit`.
    #[test]
    fn a_broken_piece_is_worth_nothing() {
        let items = item_table();
        let mut c = character([0, 0, 0, 0, 0, 0], 1);
        c.items
            .put(Item {
                index: 1000,
                container: crate::inventory::EQUIP,
                slot: 6,
                durability_min: 0,
                ..Item::default()
            })
            .unwrap();

        assert_eq!(of(&c, &items).attack, 0, "a broken weapon still hit");
    }

    /// A caster and a warrior of the same level are not the same, which is
    /// the whole point of the templates carrying different attributes.
    #[test]
    fn a_caster_and_a_warrior_differ() {
        let items = item_table();

        // The warrior is the tougher one because it is the one in armour:
        // constitution buys no defence in the original, gear does.
        let mut warrior = character([15, 9, 5, 16, 0, 0], 10);
        wearing(&mut warrior, 6, 1000);
        wearing(&mut warrior, 2, 2000);
        let mut caster = character([7, 9, 16, 8, 10, 0], 10);
        wearing(&mut caster, 6, 3000);

        let w = of(&warrior, &items);
        let c = of(&caster, &items);

        assert!(w.attack > c.attack, "the warrior does not hit harder");
        assert!(c.magic_attack > w.magic_attack, "the caster is not the better mage");
        assert!(w.defence > c.defence, "the warrior is not the tougher one");
    }

    #[test]
    fn gear_the_table_does_not_know_is_ignored_rather_than_fatal() {
        let items = item_table();
        let mut c = character([10, 10, 10, 10, 10, 0], 1);
        wearing(&mut c, 6, 9999);

        assert_eq!(gear_of(&c.items, &items), Gear::default());
    }

    /// A target you cannot hurt at all reads as a broken server.
    #[test]
    fn damage_never_falls_to_nothing() {
        assert_eq!(base_damage(10, 9999), 1);
        assert_eq!(base_damage(0, 0), 1);
    }

    #[test]
    fn defence_takes_the_edge_off_without_removing_it() {
        assert_eq!(base_damage(100, 0), 100);
        assert_eq!(base_damage(100, 40), 80);
        assert!(base_damage(100, 40) < base_damage(100, 0));
    }
}
