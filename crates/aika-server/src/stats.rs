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

/// How much a point of strength is worth in attack, and constitution in
/// defence. Placeholders, in the sense that the original derives them from
/// tables we have not read; the shape — attributes matter, gear matters more
/// — is right.
const STRENGTH_TO_ATTACK: u32 = 2;
const INTELLECT_TO_MAGIC: u32 = 2;
const CONSTITUTION_TO_DEFENCE: u32 = 1;

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
    pub max_hp: u32,
    pub max_mp: u32,
}

/// Adds up what a character is wearing and what it is made of.
pub fn of(character: &Character, items: &ItemList) -> Stats {
    let gear = gear_of(&character.items, items);

    let [strength, _agility, intellect, constitution, _luck, _free] = character.attributes;
    let level = character.level as u32;

    Stats {
        attack: gear.attack + strength as u32 * STRENGTH_TO_ATTACK,
        magic_attack: gear.magic_attack + intellect as u32 * INTELLECT_TO_MAGIC,
        defence: gear.defence + constitution as u32 * CONSTITUTION_TO_DEFENCE,
        magic_defence: gear.magic_defence + intellect as u32 * CONSTITUTION_TO_DEFENCE,
        max_hp: BASE_HP + level * HP_PER_LEVEL + gear.hp,
        max_mp: BASE_MP + level * MP_PER_LEVEL + gear.mp,
    }
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
            .put(Item { index, container: crate::inventory::EQUIP, slot, ..Item::default() })
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

    #[test]
    fn attributes_and_gear_both_count() {
        let items = item_table();
        let mut c = character([20, 10, 5, 30, 10, 0], 10);
        wearing(&mut c, 6, 1000);

        let s = of(&c, &items);
        assert_eq!(s.attack, 120 + 20 * STRENGTH_TO_ATTACK);
        assert_eq!(s.defence, 30 * CONSTITUTION_TO_DEFENCE);
        assert_eq!(s.max_hp, BASE_HP + 10 * HP_PER_LEVEL);
    }

    /// A caster and a warrior of the same level are not the same, which is
    /// the whole point of the templates carrying different attributes.
    #[test]
    fn a_caster_and_a_warrior_differ() {
        let items = item_table();

        let mut warrior = character([15, 9, 5, 16, 0, 0], 10);
        wearing(&mut warrior, 6, 1000);
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
