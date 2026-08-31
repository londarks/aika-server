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

pub const OP_CREATE_CHARACTER: u16 = 0x3E04;
pub const OP_DELETE_CHARACTER: u16 = 0x3E01;

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

/// Everyone starts with a stack of these.
pub const STARTING_ITEM: u16 = 5300;
pub const STARTING_ITEM_SLOTS: u16 = 3;

/// The weapon each class is handed, in equipment slot 15
/// (`PacketHandlers.pas:640`).
pub const WEAPON_SLOT: u16 = 15;
const MELEE_WEAPON: u16 = 4615;
const RANGED_WEAPON: u16 = 4600;
/// The original writes a refine of 1000 on the starting weapon, which is how
/// it marks a piece of gear that cannot be sold or refined further.
const STARTING_REFINE: u16 = 1000;

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
pub fn create(
    request: &CreateCharacter,
    existing: &[Character],
    taken: impl Fn(&str) -> bool,
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
    };

    for slot in 0..STARTING_ITEM_SLOTS {
        let _ = character.items.put(Item {
            container: crate::inventory::BAG,
            slot,
            index: STARTING_ITEM,
            appearance: STARTING_ITEM,
            refine: 1,
            ..Item::default()
        });
    }

    // Three of the six classes fight at range; the original hands each group
    // a different starting weapon (`PacketHandlers.pas:640`).
    let weapon = match class_of(request.class_index) {
        0 | 1 | 2 => MELEE_WEAPON,
        _ => RANGED_WEAPON,
    };
    let _ = character.items.put(Item {
        container: crate::inventory::EQUIP,
        slot: WEAPON_SLOT,
        index: weapon,
        appearance: weapon,
        refine: STARTING_REFINE,
        ..Item::default()
    });

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
        assert_eq!(create(&second, &[], nobody).unwrap().x, TOWN_SECOND.0);
        assert_eq!(create(&request("Athus", 0), &[], nobody).unwrap().x, TOWN_FIRST.0);
    }

    #[test]
    fn a_new_character_is_level_one_with_something_to_carry() {
        let character = create(&request("Athus", 0), &[], nobody).unwrap();

        assert_eq!(character.level, 1);
        assert_eq!(character.name, "Athus");
        assert_eq!(character.class_index, 20);
        assert_eq!(character.id, 0, "the database has not seen it yet");

        assert_eq!(
            character.items.in_container(crate::inventory::BAG).count(),
            STARTING_ITEM_SLOTS as usize
        );
        let weapon = character
            .items
            .get(crate::inventory::EQUIP, WEAPON_SLOT)
            .expect("no starting weapon");
        assert_eq!(weapon.index, MELEE_WEAPON, "class 1 fights up close");
    }

    #[test]
    fn the_ranged_classes_get_a_different_weapon() {
        let mut ranged = request("Athus", 0);
        ranged.class_index = 40;
        let character = create(&ranged, &[], nobody).unwrap();

        let weapon = character.items.get(crate::inventory::EQUIP, WEAPON_SLOT).unwrap();
        assert_eq!(weapon.index, RANGED_WEAPON);
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
        let result = create(&request("Athus", 0), &[], |name| name == "Athus");
        assert_eq!(result, Err(CreateError::NameTaken("Athus".into())));
    }

    /// A name with punctuation in it is what a client sends when somebody is
    /// trying to break the character list, so it is refused outright.
    #[test]
    fn a_name_has_to_be_letters_and_digits() {
        assert_eq!(
            create(&request("Ath us", 0), &[], nobody),
            Err(CreateError::NameNotAlphanumeric)
        );
        assert_eq!(
            create(&request("Ath\u{1}s", 0), &[], nobody),
            Err(CreateError::NameNotAlphanumeric)
        );
        assert_eq!(create(&request("", 0), &[], nobody), Err(CreateError::NameEmpty));
        assert!(create(&request("Athus99", 0), &[], nobody).is_ok());
    }

    #[test]
    fn a_name_longer_than_the_client_shows_is_refused() {
        let long = "A".repeat(MAX_NAME + 1);
        assert_eq!(
            create(&request(&long, 0), &[], nobody),
            Err(CreateError::NameTooLong(MAX_NAME + 1))
        );
        assert!(create(&request(&"A".repeat(MAX_NAME), 0), &[], nobody).is_ok());
    }

    #[test]
    fn a_slot_that_is_taken_or_does_not_exist_is_refused() {
        let existing = vec![create(&request("First", 0), &[], nobody).unwrap()];

        assert_eq!(
            create(&request("Second", 0), &existing, nobody),
            Err(CreateError::BadSlot(0)),
            "slot 0 is taken"
        );
        assert_eq!(
            create(&request("Second", 3), &existing, nobody),
            Err(CreateError::BadSlot(3))
        );
        assert!(create(&request("Second", 1), &existing, nobody).is_ok());
    }

    #[test]
    fn a_fourth_character_is_refused() {
        let existing: Vec<Character> = (0..MAX_CHARACTERS as u32)
            .map(|slot| create(&request(&format!("N{slot}"), slot), &[], nobody).unwrap())
            .collect();

        assert_eq!(
            create(&request("Fourth", 0), &existing, nobody),
            Err(CreateError::NoFreeSlot)
        );
    }

    #[test]
    fn a_class_or_hair_the_client_cannot_draw_is_refused() {
        let mut bad_class = request("Athus", 0);
        bad_class.class_index = 5;
        assert_eq!(create(&bad_class, &[], nobody), Err(CreateError::BadClass(5)));

        bad_class.class_index = 70;
        assert_eq!(create(&bad_class, &[], nobody), Err(CreateError::BadClass(70)));

        let mut bad_hair = request("Athus", 0);
        bad_hair.hair = 1;
        assert_eq!(create(&bad_hair, &[], nobody), Err(CreateError::BadHair(1)));

        bad_hair.hair = MAX_HAIR + 1;
        assert_eq!(create(&bad_hair, &[], nobody), Err(CreateError::BadHair(MAX_HAIR + 1)));
    }
}
