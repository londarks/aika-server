//! Making a character.
//!
//! `0x3E04` carries the name, the class, the hair and which of the two
//! starting towns to appear in. Everything the server has to refuse is in
//! `TPacketHandlers.CreateCharacter` (`PacketHandlers.pas:529`), and each
//! refusal has a sentence the player reads, so the rules live here as an
//! error type rather than as a bare `false`.
//!
//! ```text
//! 0   u32     account id, which we ignore: the connection already knows it
//! 4   u32     slot, 0 to 2
//! 8   char[16] name
//! 24  u16     class index, 10 to 69
//! 26  u16     hair, 7700 to 7731
//! 28  u32     starting town
//! ```
//!
//! The record declares twelve spare bytes between the hair and the town, but
//! the client sends 44 bytes in total, which leaves no room for them: the
//! town follows the hair directly on the wire. Same trap as the login packet,
//! and the same rule applies — the wire decides, not the record.

use crate::store::{Character, Item, DEFAULT_SIZES, DEFAULT_SPEED_MOVE, MAX_CHARACTERS};
use aika_data::template::Template;

pub const OP_CREATE_CHARACTER: u16 = 0x3E04;
pub const OP_DELETE_CHARACTER: u16 = 0x3E01;
/// The opcode our client actually sends to delete a character.
///
/// It is nowhere in the original's source, which is consistent with what that
/// server does: its `DeleteChar` is disabled, so nobody ever noticed that the
/// client had stopped using `0x3E01`. The body is the first twelve bytes of
/// `TDeleteChar` — a spare DWORD, the slot, and the four character PIN — with
/// the record's trailing 32 bytes absent from the wire, the same way the
/// login and creation packets are shorter than their records.
pub const OP_DELETE_CHARACTER_ALT: u16 = 0x3F33;

/// `TDeleteChar` (`Data/Packets.pas:284`): a spare DWORD, the slot, and a
/// four character PIN.
///
/// The original refuses this outright — `TPacketHandlers.DeleteChar` opens
/// with a message reading "disabled until the risk of breaking your account
/// is analysed" and returns. The code behind that early exit still shows what
/// it meant to do, and it is worth doing properly: mark the row deleted
/// rather than remove it, so a mistake is recoverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteCharacter {
    pub slot: u32,
    /// The PIN the client asks for before deleting. We have nowhere to store
    /// one yet, so it is carried and not checked; when accounts grow a PIN
    /// this is where it gets compared.
    pub pin: String,
}

impl DeleteCharacter {
    pub const MIN_BODY: usize = 8;
    pub const BODY_SIZE: usize = 44;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::MIN_BODY {
            return None;
        }
        let pin = body
            .get(8..12)
            .map(|bytes| {
                bytes.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>()
            })
            .unwrap_or_default();

        Some(Self { slot: u32::from_le_bytes(body[4..8].try_into().ok()?), pin })
    }

    pub fn to_body(&self) -> Vec<u8> {
        let mut body = vec![0u8; Self::BODY_SIZE];
        body[4..8].copy_from_slice(&self.slot.to_le_bytes());
        let pin = self.pin.as_bytes();
        let len = pin.len().min(4);
        body[8..8 + len].copy_from_slice(&pin[..len]);
        body
    }
}

/// The longest name the client will show.
pub const MAX_NAME: usize = 14;

/// Class indices run in tens: 10-19 is the first class, 20-29 the second, up
/// to 60-69 (`PacketHandlers.pas:596`).
pub const MIN_CLASS_INDEX: u16 = 10;
pub const MAX_CLASS_INDEX: u16 = 69;

/// The hair styles the client can draw.
pub const MIN_HAIR: u16 = 7700;
pub const MAX_HAIR: u16 = 7731;

/// Where a new character wakes up, chosen on the creation screen.
pub const TOWN_FIRST: (u32, u32) = (3450, 690);
pub const TOWN_SECOND: (u32, u32) = (3470, 935);

/// Bags, which go in the last six bag slots rather than the first
/// (`PacketHandlers.pas:622`).
pub const BAG_ITEM: u16 = 5300;
pub const BAG_SLOTS: std::ops::RangeInclusive<u16> = 120..=125;

/// The vaults that unlock the chest's four pages, put there in the same breath
/// as the bags (`PacketHandlers.pas:630`). Note the asymmetry, which is the
/// original's: a bag is given an appearance and a refine of one, a vault only
/// its index.
pub const VAULT_ITEM: u16 = 5310;

/// The chest as an account starts with it: four vaults and nothing else.
///
/// It sits here rather than in `create` because the chest belongs to the
/// account, not to the character being made — but this is where the original
/// fills it, so a chest with no vaults in it is one nobody could ever put
/// anything into.
pub fn starting_storage() -> crate::inventory::Inventory {
    crate::inventory::STORAGE_PAGE_ITEMS
        .map(|slot| Item {
            container: crate::inventory::STORAGE,
            slot,
            index: VAULT_ITEM,
            ..Item::default()
        })
        .collect()
}

/// Ammunition, and the two classes that get it.
///
/// Only the two that shoot: the Atirador takes rifle rounds and the
/// Pistoleira pistol rounds (`PacketHandlers.pas:632`). Handing a thousand
/// rifle bullets to a Feiticeiro, which an earlier version of this did, is
/// not something the original ever does.
pub const AMMO_SLOT: u16 = 15;
pub const AMMO_BAG_SLOTS: [u16; 2] = [5, 6];
const RIFLE_AMMO: u16 = 4615;
const PISTOL_AMMO: u16 = 4600;
const AMMO_COUNT: u16 = 1000;

/// Which class number gets which ammunition, or none.
fn ammunition_for(class_number: u16) -> Option<u16> {
    match class_number {
        3 => Some(RIFLE_AMMO),
        4 => Some(PISTOL_AMMO),
        _ => None,
    }
}

/// The marker the original writes for a learned basic skill
/// (`SetPlayerSkills`). The client reads this out of the record to decide the
/// skill may be cast; anything else and it treats the skill as unlearned.
const BASIC_LEARNED: u16 = 2;

/// Builds the record's skill list the way `TPlayer.SetPlayerSkills` does,
/// rather than copying the template's stored bytes.
///
/// The template's `skills()` table says, for each of the six basic and forty
/// advanced slots, what rank the character has learned. This turns that into
/// the sixty-slot array the client reads: a learned basic is `2`, a learned
/// advanced skill carries its level in slots six and up, and everything else
/// stays zero.
fn skill_list_from(template: &Template) -> [u16; 60] {
    let mut list = [0u16; 60];
    let learned = template.skills();

    for i in 0..aika_data::template::BASIC_SKILLS {
        if learned[i].rank != 0 {
            list[i] = BASIC_LEARNED;
        }
    }
    for i in 0..aika_data::template::OTHER_SKILLS {
        let entry = learned[aika_data::template::BASIC_SKILLS + i];
        if entry.rank != 0 {
            list[aika_data::template::BASIC_SKILLS + i] = entry.rank;
        }
    }
    list
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    NoFreeSlot,
    BadSlot(u32),
    NameTooLong(usize),
    NameEmpty,
    NameNotAlphanumeric,
    NameTaken(String),
    BadClass(u16),
    BadHair(u16),
}

impl CreateError {
    /// What the player is told. These are the sentences the original sends,
    /// in English.
    pub fn message(&self) -> String {
        match self {
            CreateError::NoFreeSlot => format!("You already have {MAX_CHARACTERS} characters."),
            CreateError::BadSlot(slot) => format!("Slot {slot} does not exist."),
            CreateError::NameTooLong(n) => {
                format!("A name is at most {MAX_NAME} characters, and that one is {n}.")
            }
            CreateError::NameEmpty => "A name cannot be empty.".into(),
            CreateError::NameNotAlphanumeric => "A name can only use letters and digits.".into(),
            CreateError::NameTaken(name) => format!("{name} is already taken."),
            CreateError::BadClass(index) => format!("Class {index} does not exist."),
            CreateError::BadHair(hair) => format!("Hair {hair} does not exist."),
        }
    }
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CreateError {}

/// `0x3E04`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCharacter {
    pub slot: u32,
    pub name: String,
    pub class_index: u16,
    pub hair: u16,
    pub town: u32,
}

impl CreateCharacter {
    /// Everything up to and including the hair. The town is read when it is
    /// there and defaults to the first one when it is not.
    pub const MIN_BODY: usize = 28;
    pub const BODY_SIZE: usize = 32;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::MIN_BODY {
            return None;
        }
        let name = body[8..24]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect::<String>();

        Some(Self {
            slot: u32::from_le_bytes(body[4..8].try_into().ok()?),
            name,
            class_index: u16::from_le_bytes(body[24..26].try_into().ok()?),
            hair: u16::from_le_bytes(body[26..28].try_into().ok()?),
            town: if body.len() >= Self::BODY_SIZE {
                u32::from_le_bytes(body[28..32].try_into().ok()?)
            } else {
                0
            },
        })
    }

    pub fn to_body(&self) -> Vec<u8> {
        let mut body = vec![0u8; Self::BODY_SIZE];
        body[4..8].copy_from_slice(&self.slot.to_le_bytes());
        let name = self.name.as_bytes();
        let len = name.len().min(15);
        body[8..8 + len].copy_from_slice(&name[..len]);
        body[24..26].copy_from_slice(&self.class_index.to_le_bytes());
        body[26..28].copy_from_slice(&self.hair.to_le_bytes());
        body[28..32].copy_from_slice(&self.town.to_le_bytes());
        body
    }

    /// Where this character wakes up.
    pub fn spawn(&self) -> (u32, u32) {
        match self.town {
            1 => TOWN_SECOND,
            _ => TOWN_FIRST,
        }
    }
}

/// Base class, 0 to 5, from the index range the client sends.
pub fn class_of(class_index: u16) -> u16 {
    class_index / 10 - 1
}

/// Turns a request into a character, or says why not.
///
/// `taken` decides whether a name is already in use; it is a closure so this
/// stays a pure function and the database lookup stays at the edge.
/// `template` is the class's starting record, which is where its armour,
/// attributes and consumables come from.
pub fn create(
    request: &CreateCharacter,
    existing: &[Character],
    taken: impl Fn(&str) -> bool,
    template: Option<&Template>,
) -> Result<Character, CreateError> {
    if existing.len() >= MAX_CHARACTERS {
        return Err(CreateError::NoFreeSlot);
    }
    if request.slot as usize >= MAX_CHARACTERS {
        return Err(CreateError::BadSlot(request.slot));
    }
    if existing.iter().any(|c| c.slot == request.slot as usize) {
        return Err(CreateError::BadSlot(request.slot));
    }

    let name = request.name.trim();
    if name.is_empty() {
        return Err(CreateError::NameEmpty);
    }
    if name.len() > MAX_NAME {
        return Err(CreateError::NameTooLong(name.len()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(CreateError::NameNotAlphanumeric);
    }
    if taken(name) {
        return Err(CreateError::NameTaken(name.to_string()));
    }

    if !(MIN_CLASS_INDEX..=MAX_CLASS_INDEX).contains(&request.class_index) {
        return Err(CreateError::BadClass(request.class_index));
    }
    if !(MIN_HAIR..=MAX_HAIR).contains(&request.hair) {
        return Err(CreateError::BadHair(request.hair));
    }

    let (x, y) = request.spawn();
    let mut character = Character {
        id: 0,
        slot: request.slot as usize,
        name: name.to_string(),
        nation: 0,
        class_index: request.class_index,
        hair: request.hair,
        level: 1,
        exp: 0,
        gold: 0,
        sizes: DEFAULT_SIZES,
        speed_move: DEFAULT_SPEED_MOVE,
        attributes: [10, 10, 10, 10, 10, 0],
        x,
        y,
        items: Default::default(),
        skill_list: [0; 60],
        item_bar: [0; 40],
        skill_points: crate::store::skill_points_for(1),
    };

    // Everything a class is born with comes from its template. Without one
    // the character is still playable, just naked and with flat attributes.
    if let Some(template) = template {
        character.level = template.level().max(1);
        character.skill_points = crate::store::skill_points_for(character.level);
        character.sizes = template.sizes();
        character.attributes = template.attributes();
        character.gold = template.gold();
        // The icons already on its bar, straight from the template.
        character.item_bar = template.item_bar();
        // And the record's skill list, which is not copied but *built* — the
        // original computes it in `TPlayer.SetPlayerSkills` rather than
        // trusting the stored bytes. A learned basic is marked `2`; a learned
        // advanced skill carries its level. Without these markers the client
        // treats the skill as unlearned and cancels the cast, which is why a
        // fresh character's basic attack did nothing.
        character.skill_list = skill_list_from(template);

        for item in template.equipment() {
            let _ = character.items.put(from_template(item, crate::inventory::EQUIP));
        }
        for item in template.inventory() {
            let _ = character.items.put(from_template(item, crate::inventory::BAG));
        }
    }

    // And then what creation adds on top of it, in the order the original
    // adds it (`PacketHandlers.pas:616`).
    for slot in BAG_SLOTS {
        let _ = character.items.put(Item {
            container: crate::inventory::BAG,
            slot,
            index: BAG_ITEM,
            appearance: BAG_ITEM,
            refine: 1,
            ..Item::default()
        });
    }

    if let Some(ammo) = ammunition_for(character.class_number()) {
        let round = |container, slot| Item {
            container,
            slot,
            index: ammo,
            appearance: ammo,
            refine: AMMO_COUNT,
            ..Item::default()
        };
        let _ = character.items.put(round(crate::inventory::EQUIP, AMMO_SLOT));
        for slot in AMMO_BAG_SLOTS {
            let _ = character.items.put(round(crate::inventory::BAG, slot));
        }
    }

    Ok(character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str, slot: u32) -> CreateCharacter {
        CreateCharacter {
            slot,
            name: name.into(),
            class_index: 20,
            hair: 7702,
            town: 0,
        }
    }

    fn nobody(_: &str) -> bool {
        false
    }

    #[test]
    fn delete_body_roundtrip() {
        let original = DeleteCharacter { slot: 2, pin: "1234".into() };
        assert_eq!(DeleteCharacter::parse(&original.to_body()), Some(original));
        assert_eq!(DeleteCharacter::parse(&[0u8; 4]), None);
    }

    /// A client that sends only the slot and no PIN still names a slot.
    #[test]
    fn a_delete_without_a_pin_still_parses() {
        let mut body = vec![0u8; DeleteCharacter::MIN_BODY];
        body[4..8].copy_from_slice(&1u32.to_le_bytes());

        let parsed = DeleteCharacter::parse(&body).unwrap();
        assert_eq!(parsed.slot, 1);
        assert_eq!(parsed.pin, "");
    }

    #[test]
    fn body_roundtrip() {
        let original = request("Athus", 1);
        assert_eq!(CreateCharacter::parse(&original.to_body()), Some(original));
        assert_eq!(CreateCharacter::parse(&[0u8; 20]), None);
    }

    /// The client sends 44 bytes, which is 32 of body: the twelve spare bytes
    /// the record declares are not on the wire, and the town comes right
    /// after the hair.
    #[test]
    fn the_body_is_the_size_the_client_sends() {
        assert_eq!(CreateCharacter::BODY_SIZE + 12, 44);

        let mut body = request("Athus", 0).to_body();
        body[28..32].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(CreateCharacter::parse(&body).unwrap().town, 1);
    }

    /// An older client that stops after the hair still creates a character,
    /// in the first town.
    #[test]
    fn a_body_without_a_town_still_works() {
        let body = request("Athus", 0).to_body();
        let short = CreateCharacter::parse(&body[..CreateCharacter::MIN_BODY]).unwrap();
        assert_eq!(short.town, 0);
        assert_eq!(short.spawn(), TOWN_FIRST);
    }

    #[test]
    fn a_new_character_starts_in_the_town_it_chose() {
        let mut second = request("Athus", 0);
        second.town = 1;
        assert_eq!(create(&second, &[], nobody, None).unwrap().x, TOWN_SECOND.0);
        assert_eq!(create(&request("Athus", 0), &[], nobody, None).unwrap().x, TOWN_FIRST.0);
    }

    /// A character born without a template is still playable: it just has
    /// nothing on and flat attributes.
    #[test]
    fn without_a_template_a_character_is_playable_but_bare() {
        let character = create(&request("Athus", 0), &[], nobody, None).unwrap();

        assert_eq!(character.level, 1);
        assert_eq!(character.name, "Athus");
        assert_eq!(character.class_index, 20);
        assert_eq!(character.id, 0, "the database has not seen it yet");
        assert_eq!(character.attributes, [10, 10, 10, 10, 10, 0]);
    }

    /// The six bags creation hands out go in the *last* six bag slots, not
    /// the first. Putting them at the front, which an earlier version did,
    /// buries them under everything the template already put there.
    #[test]
    fn the_bags_go_in_the_last_six_slots() {
        let character = create(&request("Athus", 0), &[], nobody, None).unwrap();

        for slot in BAG_SLOTS {
            let bag = character
                .items
                .get(crate::inventory::BAG, slot)
                .unwrap_or_else(|| panic!("no bag in slot {slot}"));
            assert_eq!(bag.index, BAG_ITEM);
        }
        assert!(
            character.items.get(crate::inventory::BAG, 0).is_none(),
            "a bag landed in the first slot"
        );
    }

    /// Only the two classes that shoot get ammunition. Handing a thousand
    /// rifle rounds to a Feiticeiro is what this is here to stop.
    #[test]
    fn only_the_shooters_are_given_ammunition() {
        let ammo_of = |class_index: u16| {
            let mut r = request("Athus", 0);
            r.class_index = class_index;
            let c = create(&r, &[], nobody, None).unwrap();
            c.items.get(crate::inventory::EQUIP, AMMO_SLOT).map(|i| i.index)
        };

        assert_eq!(ammo_of(30), Some(4615), "the Atirador takes rifle rounds");
        assert_eq!(ammo_of(40), Some(4600), "the Pistoleira takes pistol rounds");

        for class_index in [10, 20, 50, 60] {
            assert_eq!(
                ammo_of(class_index),
                None,
                "class index {class_index} was handed ammunition it cannot use"
            );
        }
    }

    /// With a template the character is born wearing what its class wears,
    /// carrying what it carries, and with the attributes it should have.
    #[test]
    fn a_template_decides_what_a_class_starts_as() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/templates");
        if !dir.join("Atirador.acc").is_file() {
            return;
        }
        let all = aika_data::template::load_all(&dir);
        let atirador = all[2].as_ref().expect("no Atirador template");

        let mut r = request("Athus", 0);
        r.class_index = 30;
        let character = create(&r, &[], nobody, Some(atirador)).unwrap();

        assert_eq!(character.attributes, atirador.attributes());
        assert_eq!(character.sizes, atirador.sizes());
        assert!(
            character.items.in_container(crate::inventory::EQUIP).count() > 1,
            "it was born wearing nothing"
        );

        // and the additions still land on top of the template
        assert!(character.items.get(crate::inventory::BAG, 125).is_some(), "no bag");
        assert_eq!(
            character.items.get(crate::inventory::EQUIP, AMMO_SLOT).map(|i| i.index),
            Some(4615)
        );

        // The bar carries the template's icon, and the record marks the basic
        // skills learned so the client lets the player cast them. The
        // Atirador template learns its six basics at rank one.
        assert_ne!(character.item_bar, [0; 40], "the action bar came up empty");
        assert_eq!(
            &character.skill_list[0..6],
            &[2, 2, 2, 2, 2, 2],
            "the basic skills are not marked learned, so the client cancels the cast"
        );
    }

    /// The skill list is computed, not copied: a learned basic is `2` and the
    /// advanced slots stay empty until they are earned.
    #[test]
    fn the_skill_list_marks_learned_basics() {
        use aika_data::template::{Template, BASIC_SKILLS, CHARACTER_AT, FILE_SIZE, SKILLS_AT};

        let mut raw = vec![0u8; FILE_SIZE];
        raw[CHARACTER_AT + aika_data::template::field::CLASS_INFO] = 21;
        // four basics learned at rank one, two left unlearned
        for i in 0..4 {
            raw[SKILLS_AT + i * 4..SKILLS_AT + i * 4 + 2].copy_from_slice(&(1921u16).to_le_bytes());
            raw[SKILLS_AT + i * 4 + 2..SKILLS_AT + i * 4 + 4].copy_from_slice(&1u16.to_le_bytes());
        }
        let template = Template::decode(&raw).unwrap();

        let list = skill_list_from(&template);
        assert_eq!(&list[0..BASIC_SKILLS], &[2, 2, 2, 2, 0, 0], "only learned basics are marked");
        assert!(list[BASIC_SKILLS..].iter().all(|&v| v == 0), "no advanced skill is learned");
    }

    #[test]
    fn class_ranges_map_in_tens() {
        assert_eq!(class_of(10), 0);
        assert_eq!(class_of(19), 0);
        assert_eq!(class_of(20), 1);
        assert_eq!(class_of(69), 5);
    }

    #[test]
    fn a_name_already_in_use_is_refused() {
        let result = create(&request("Athus", 0), &[], |name| name == "Athus", None);
        assert_eq!(result, Err(CreateError::NameTaken("Athus".into())));
    }

    /// A name with punctuation in it is what a client sends when somebody is
    /// trying to break the character list, so it is refused outright.
    #[test]
    fn a_name_has_to_be_letters_and_digits() {
        assert_eq!(
            create(&request("Ath us", 0), &[], nobody, None),
            Err(CreateError::NameNotAlphanumeric)
        );
        assert_eq!(
            create(&request("Ath\u{1}s", 0), &[], nobody, None),
            Err(CreateError::NameNotAlphanumeric)
        );
        assert_eq!(create(&request("", 0), &[], nobody, None), Err(CreateError::NameEmpty));
        assert!(create(&request("Athus99", 0), &[], nobody, None).is_ok());
    }

    #[test]
    fn a_name_longer_than_the_client_shows_is_refused() {
        let long = "A".repeat(MAX_NAME + 1);
        assert_eq!(
            create(&request(&long, 0), &[], nobody, None),
            Err(CreateError::NameTooLong(MAX_NAME + 1))
        );
        assert!(create(&request(&"A".repeat(MAX_NAME), 0), &[], nobody, None).is_ok());
    }

    #[test]
    fn a_slot_that_is_taken_or_does_not_exist_is_refused() {
        let existing = vec![create(&request("First", 0), &[], nobody, None).unwrap()];

        assert_eq!(
            create(&request("Second", 0), &existing, nobody, None),
            Err(CreateError::BadSlot(0)),
            "slot 0 is taken"
        );
        assert_eq!(
            create(&request("Second", 3), &existing, nobody, None),
            Err(CreateError::BadSlot(3))
        );
        assert!(create(&request("Second", 1), &existing, nobody, None).is_ok());
    }

    #[test]
    fn a_fourth_character_is_refused() {
        let existing: Vec<Character> = (0..MAX_CHARACTERS as u32)
            .map(|slot| create(&request(&format!("N{slot}"), slot), &[], nobody, None).unwrap())
            .collect();

        assert_eq!(
            create(&request("Fourth", 0), &existing, nobody, None),
            Err(CreateError::NoFreeSlot)
        );
    }

    #[test]
    fn a_class_or_hair_the_client_cannot_draw_is_refused() {
        let mut bad_class = request("Athus", 0);
        bad_class.class_index = 5;
        assert_eq!(create(&bad_class, &[], nobody, None), Err(CreateError::BadClass(5)));

        bad_class.class_index = 70;
        assert_eq!(create(&bad_class, &[], nobody, None), Err(CreateError::BadClass(70)));

        let mut bad_hair = request("Athus", 0);
        bad_hair.hair = 1;
        assert_eq!(create(&bad_hair, &[], nobody, None), Err(CreateError::BadHair(1)));

        bad_hair.hair = MAX_HAIR + 1;
        assert_eq!(create(&bad_hair, &[], nobody, None), Err(CreateError::BadHair(MAX_HAIR + 1)));
    }
}

/// A template's item, put in one of our containers.
fn from_template(item: aika_data::template::Item, container: u8) -> Item {
    Item {
        container,
        slot: item.slot,
        index: item.index,
        appearance: item.appearance,
        identific: item.identific,
        effect_index: item.effect_index,
        effect_value: item.effect_value,
        durability_min: item.durability_min,
        durability_max: item.durability_max,
        refine: item.refine,
        expires_at: item.expires_at as u32,
    }
}
