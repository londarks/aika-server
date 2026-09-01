//! `Data/BaseAccs/*.acc` — what a character of each class starts as.
//!
//! Six files, one per class, 7,249 bytes each. The original reads them into
//! `InitialAccounts` at startup and copies the whole record over a new
//! character before it changes a thing (`PacketHandlers.pas:616`), so this is
//! where a starting character's armour, attributes and consumables really
//! come from — not from anything the creation handler builds.
//!
//! The file is a `TBasicCharacter`, which opens with a spare DWORD and then
//! holds a whole `TCharacter`:
//!
//! ```text
//! 0     u32           Index, unused
//! 4     TCharacter    6384 bytes; the offsets inside it are the usual ones
//! 6388  u16 x4        SpeedMove, DuploAtk, Rotation, Resistence
//! 6396  f64 x2        LastAction, LastLogin
//! 6412  u32           LoggedTime
//! 6416  u8            PlayerKill
//! 6417  f32 x2        LastPos
//! 6425  f32 x2        CurrentPos
//! 6433  TSkillsList   6 basic and 40 other {index, rank} pairs
//! ```
//!
//! That arithmetic is not a reading of the record — it is how the layout was
//! confirmed. The skill list was found first, by searching all six files for
//! the ids `GetSkillIndex` says each class should start with; it turned up at
//! 6433 in every one of them, which is exactly where the fields above put it.

use std::path::Path;

/// A `TBasicCharacter`.
pub const FILE_SIZE: usize = 7249;
/// Where the `TCharacter` inside it starts.
pub const CHARACTER_AT: usize = 4;
/// And how long it is.
pub const CHARACTER_SIZE: usize = 6384;
/// Where the skill list sits, past the end of the character.
pub const SKILLS_AT: usize = 6433;

pub const BASIC_SKILLS: usize = 6;
pub const OTHER_SKILLS: usize = 40;

/// Offsets inside the `TCharacter`, the same ones the protocol uses.
pub mod field {
    pub const NATION: usize = 28;
    pub const CLASS_INFO: usize = 29;
    pub const ATTRIBUTES: usize = 32;
    pub const SIZES: usize = 44;
    pub const MAX_HP: usize = 48;
    pub const MAX_MP: usize = 56;
    pub const LEVEL: usize = 184;
    pub const EQUIP: usize = 340;
    pub const INVENTORY: usize = 664;
    pub const GOLD: usize = 3184;
    /// Sixty words the client reads as the skills the character knows
    /// (`TCharacter.SkillList`, `Data/PlayerData.pas:236`), inside the record.
    pub const SKILL_LIST: usize = 4596;
    pub const SKILL_LIST_SLOTS: usize = 60;
    /// Forty dwords: the hotbar, what sits on the action bar
    /// (`TCharacter.ItemBar`, `Data/PlayerData.pas:238`, bytes 4716..4875).
    pub const ITEM_BAR: usize = 4716;
    pub const ITEM_BAR_SLOTS: usize = 40;
    pub const ITEM_SIZE: usize = 20;
    pub const EQUIP_SLOTS: usize = 16;
    pub const INVENTORY_SLOTS: usize = 126;
}

#[derive(Debug, PartialEq, Eq)]
pub enum TemplateError {
    TooShort { size: usize, wanted: usize },
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::TooShort { size, wanted } => {
                write!(f, "{size} bytes, less than the {wanted} of a template")
            }
        }
    }
}

impl std::error::Error for TemplateError {}

/// One item out of a template, in the fields `TItem` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Item {
    pub slot: u16,
    pub index: u16,
    pub appearance: u16,
    pub identific: i32,
    pub effect_index: [u8; 3],
    pub effect_value: [u8; 3],
    pub durability_min: u8,
    pub durability_max: u8,
    /// Doubles as a stack count for things that stack.
    pub refine: u16,
    pub expires_at: u16,
}

/// One entry of the starting skill list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartingSkill {
    pub index: u16,
    pub rank: u16,
}

/// What a character of one class starts as.
pub struct Template {
    raw: Vec<u8>,
}

impl std::fmt::Debug for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Template")
            .field("class_info", &self.class_info())
            .field("level", &self.level())
            .field("equipped", &self.equipment().len())
            .field("carried", &self.inventory().len())
            .finish()
    }
}

impl Template {
    pub fn decode(bytes: &[u8]) -> Result<Self, TemplateError> {
        // Only as far as the skill list is needed; a longer file is fine.
        let wanted = SKILLS_AT + (BASIC_SKILLS + OTHER_SKILLS) * 4;
        if bytes.len() < wanted {
            return Err(TemplateError::TooShort { size: bytes.len(), wanted });
        }
        Ok(Self { raw: bytes.to_vec() })
    }

    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Result<Self, TemplateError>> {
        Ok(Self::decode(&std::fs::read(path)?))
    }

    fn at(&self, offset: usize) -> usize {
        CHARACTER_AT + offset
    }

    /// The code the client reads to name a class: 1, 11, 21, 31, 41 or 51.
    pub fn class_info(&self) -> u8 {
        self.raw[self.at(field::CLASS_INFO)]
    }

    pub fn nation(&self) -> u8 {
        self.raw[self.at(field::NATION)]
    }

    pub fn level(&self) -> u16 {
        u16le(&self.raw, self.at(field::LEVEL))
    }

    /// Strength, agility, intellect, constitution, luck and free points.
    pub fn attributes(&self) -> [u16; 6] {
        let mut out = [0u16; 6];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u16le(&self.raw, self.at(field::ATTRIBUTES) + i * 2);
        }
        out
    }

    /// Height, torso, legs and body.
    pub fn sizes(&self) -> [u8; 4] {
        let at = self.at(field::SIZES);
        self.raw[at..at + 4].try_into().unwrap()
    }

    pub fn max_hp(&self) -> u32 {
        u32le(&self.raw, self.at(field::MAX_HP))
    }

    pub fn max_mp(&self) -> u32 {
        u32le(&self.raw, self.at(field::MAX_MP))
    }

    pub fn gold(&self) -> u64 {
        let at = self.at(field::GOLD);
        u64::from_le_bytes(self.raw[at..at + 8].try_into().unwrap())
    }

    /// What the class starts wearing. Slots 0 and 1 are the body and the
    /// hair, which creation overwrites, and they are empty here.
    pub fn equipment(&self) -> Vec<Item> {
        self.items(self.at(field::EQUIP), field::EQUIP_SLOTS)
    }

    /// What it starts carrying.
    pub fn inventory(&self) -> Vec<Item> {
        self.items(self.at(field::INVENTORY), field::INVENTORY_SLOTS)
    }

    fn items(&self, base: usize, slots: usize) -> Vec<Item> {
        (0..slots)
            .filter_map(|slot| {
                let at = base + slot * field::ITEM_SIZE;
                let index = u16le(&self.raw, at);
                (index != 0).then(|| Item {
                    slot: slot as u16,
                    index,
                    appearance: u16le(&self.raw, at + 2),
                    identific: i32::from_le_bytes(
                        self.raw[at + 4..at + 8].try_into().unwrap(),
                    ),
                    effect_index: self.raw[at + 8..at + 11].try_into().unwrap(),
                    effect_value: self.raw[at + 11..at + 14].try_into().unwrap(),
                    durability_min: self.raw[at + 14],
                    durability_max: self.raw[at + 15],
                    refine: u16le(&self.raw, at + 16),
                    expires_at: u16le(&self.raw, at + 18),
                })
            })
            .collect()
    }

    /// The skills the character is born knowing, straight out of the record.
    ///
    /// Sixty slots; the class ones sit near the end (52 on). Zero means empty,
    /// and the client packs them itself, so the gaps are kept.
    pub fn skill_list(&self) -> [u16; field::SKILL_LIST_SLOTS] {
        std::array::from_fn(|i| u16le(&self.raw, self.at(field::SKILL_LIST) + i * 2))
    }

    /// The hotbar as the template lays it out: forty action-bar slots, most of
    /// them empty. This is what puts an icon on the bar the moment a character
    /// is made, rather than an empty row.
    pub fn item_bar(&self) -> [u32; field::ITEM_BAR_SLOTS] {
        std::array::from_fn(|i| u32le(&self.raw, self.at(field::ITEM_BAR) + i * 4))
    }

    /// The six basic skills and the forty the bar carries, in slot order,
    /// with the ranks the template gives them.
    pub fn skills(&self) -> Vec<StartingSkill> {
        (0..BASIC_SKILLS + OTHER_SKILLS)
            .map(|i| {
                let at = SKILLS_AT + i * 4;
                StartingSkill {
                    index: u16le(&self.raw, at),
                    rank: u16le(&self.raw, at + 2),
                }
            })
            .collect()
    }
}

/// The six, in the order the creation screen offers them. These are the file
/// names the original passes to `LoadBasicCharacter`, spelling and all.
pub const CLASS_FILES: [&str; 6] = [
    "Guerreiro",
    "Templaria",
    "Atirador",
    "Pistoleira",
    "Feiticeiro",
    "Cleriga",
];

/// Reads all six out of a directory, in class order.
///
/// A class whose file is missing comes back as `None` rather than failing the
/// lot: five playable classes beat none.
pub fn load_all(dir: impl AsRef<Path>) -> [Option<Template>; 6] {
    let dir = dir.as_ref();
    std::array::from_fn(|i| {
        let path = dir.join(format!("{}.acc", CLASS_FILES[i]));
        Template::load(path).ok().and_then(Result::ok)
    })
}

fn u16le(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(raw[at..at + 2].try_into().unwrap())
}

fn u32le(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(raw[at..at + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built() -> Template {
        let mut raw = vec![0u8; FILE_SIZE];
        let put16 = |raw: &mut [u8], at: usize, v: u16| {
            raw[at..at + 2].copy_from_slice(&v.to_le_bytes());
        };

        raw[CHARACTER_AT + field::CLASS_INFO] = 21;
        put16(&mut raw, CHARACTER_AT + field::LEVEL, 1);
        raw[CHARACTER_AT + field::SIZES..CHARACTER_AT + field::SIZES + 4]
            .copy_from_slice(&[7, 119, 119, 0]);
        for (i, v) in [8u16, 16, 9, 12, 5, 0].iter().enumerate() {
            put16(&mut raw, CHARACTER_AT + field::ATTRIBUTES + i * 2, *v);
        }

        // a breastplate in equipment slot 2
        let at = CHARACTER_AT + field::EQUIP + 2 * field::ITEM_SIZE;
        put16(&mut raw, at, 3074);
        put16(&mut raw, at + 16, 192);

        // ten potions in the first bag slot
        let at = CHARACTER_AT + field::INVENTORY;
        put16(&mut raw, at, 4350);
        put16(&mut raw, at + 16, 10);

        // the first two skills
        put16(&mut raw, SKILLS_AT, 1921);
        put16(&mut raw, SKILLS_AT + 2, 1);
        put16(&mut raw, SKILLS_AT + 4, 1937);
        put16(&mut raw, SKILLS_AT + 6, 1);

        // a class skill known in record slot 52, and one icon on the bar
        put16(&mut raw, CHARACTER_AT + field::SKILL_LIST + 52 * 2, 15378);
        raw[CHARACTER_AT + field::ITEM_BAR + 3 * 4
            ..CHARACTER_AT + field::ITEM_BAR + 3 * 4 + 4]
            .copy_from_slice(&30994u32.to_le_bytes());

        Template::decode(&raw).expect("the fixture is malformed")
    }

    #[test]
    fn reads_the_character_out_of_the_wrapper() {
        let t = built();
        assert_eq!(t.class_info(), 21, "the class code the client reads");
        assert_eq!(t.level(), 1);
        assert_eq!(t.sizes(), [7, 119, 119, 0]);
        assert_eq!(t.attributes(), [8, 16, 9, 12, 5, 0]);
    }

    #[test]
    fn reads_what_the_class_wears_and_carries() {
        let t = built();

        let worn = t.equipment();
        assert_eq!(worn.len(), 1);
        assert_eq!((worn[0].slot, worn[0].index, worn[0].refine), (2, 3074, 192));

        let carried = t.inventory();
        assert_eq!(carried.len(), 1);
        assert_eq!((carried[0].slot, carried[0].index, carried[0].refine), (0, 4350, 10));
    }

    /// The skill list is past the end of the character record, which is the
    /// arithmetic the module note explains.
    #[test]
    fn reads_the_starting_skills() {
        let t = built();
        let skills = t.skills();

        assert_eq!(skills.len(), BASIC_SKILLS + OTHER_SKILLS);
        assert_eq!(skills[0], StartingSkill { index: 1921, rank: 1 });
        assert_eq!(skills[1], StartingSkill { index: 1937, rank: 1 });
        assert_eq!(skills[2], StartingSkill { index: 0, rank: 0 }, "an empty slot");
    }

    #[test]
    fn reads_the_hotbar_and_known_skills_out_of_the_record() {
        let t = built();

        let bar = t.item_bar();
        assert_eq!(bar[3], 30994, "the one icon the template puts on the bar");
        assert_eq!(bar[0], 0, "the rest of the bar is empty");

        let known = t.skill_list();
        assert_eq!(known[52], 15378, "the class skill it is born knowing");
        assert_eq!(known[0], 0, "the early slots are empty, as the client packs them");
    }

    #[test]
    fn a_short_file_is_refused() {
        assert!(matches!(
            Template::decode(&[0u8; 100]),
            Err(TemplateError::TooShort { size: 100, .. })
        ));
    }

    /// The real files are not in this repository. When they are present the
    /// reader is held to what they contain — and what they contain is the
    /// answer to what a character starts as.
    #[test]
    fn reads_the_original_templates_when_they_are_available() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/templates");
        if !dir.join("Guerreiro.acc").is_file() {
            return;
        }

        let all = load_all(&dir);
        let codes: Vec<u8> =
            all.iter().filter_map(|t| t.as_ref().map(|t| t.class_info())).collect();
        assert_eq!(
            codes,
            vec![1, 11, 21, 31, 41, 51],
            "the class codes are not the ones the templates carry"
        );

        for (i, template) in all.iter().enumerate() {
            let Some(template) = template else {
                panic!("{} is missing", CLASS_FILES[i]);
            };
            assert_eq!(template.level(), 1, "{} does not start at level 1", CLASS_FILES[i]);
            assert!(
                !template.equipment().is_empty(),
                "{} starts wearing nothing",
                CLASS_FILES[i]
            );
            assert!(
                !template.inventory().is_empty(),
                "{} starts carrying nothing",
                CLASS_FILES[i]
            );
            assert!(
                template.attributes().iter().take(4).all(|&a| a > 0),
                "{} has an attribute at zero",
                CLASS_FILES[i]
            );
        }

        // the classes are not interchangeable: a warrior is stronger than a
        // caster, and the caster is cleverer
        let warrior = all[0].as_ref().unwrap().attributes();
        let caster = all[4].as_ref().unwrap().attributes();
        assert!(warrior[0] > caster[0], "the warrior is not the stronger one");
        assert!(caster[2] > warrior[2], "the caster is not the cleverer one");
    }
}
