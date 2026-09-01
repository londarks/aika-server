//! The Pran: the companion that belongs to the account rather than to any one
//! character.
//!
//! It is not a pet on the side. The class promotion at level 50 requires one
//! equipped, three of the six skill-table class groups are its, and the second
//! chest page's last two slots exist to hold it. Nothing else in the game is
//! wired into as much.
//!
//! # How one reaches the world
//!
//! Through a **Pran Summon Stone**, item type 10, which goes in equipment slot
//! ten -- `Equip[10]` in the original, and the same slot here because
//! [`crate::inventory::equip_slot_for`] already sends type ten there. The stone
//! is the carrier: `Pran.ItemID` matches the stone's `Identific`, so a stored
//! pran belongs to one particular stone and not merely to a kind of item
//! (`Mob/Player.pas:5190`).
//!
//! Which stone a pran fits is its class: `GetPranClassStoneItem` gives 100 for
//! the first two tiers, 101 for the third and 102 for the fourth, and those are
//! the numbers the stones carry in their own `Classe` field. See [`stone_tier`].
//!
//! # The first form has no body
//!
//! A pran grows through forms: the first is only a glow, and the ones after it
//! are a companion that walks beside its owner. Classes 61, 71 and 81 -- the
//! first tier of each element -- are drawn as an effect on the player and
//! nothing else: 2 for fire, 4 for water, 8 for air (`Mob/Player.pas:3730`).
//! Every form after that gets a body and its own client id, out of a range of
//! its own: 44241 to 45240 (`Connections/ServerSocket.pas:48`), a fourth id
//! space beside players, NPCs and objects.
//!
//! The original calls the first form a fairy -- `PranIsFairy`, and the branch
//! it guards is commented "pran modo elfa". That is worth knowing and not
//! worth copying: to anyone who has played, the fairy is the *winged* form at
//! the end, and a function called `is_fairy` returning true for a formless
//! glow is a trap. See [`has_body`], which is the same test named for what it
//! decides.
//!
//! # What is here and what is not
//!
//! Here: the record, where it is kept, hatching one, and putting it into and
//! out of the world with the stone. Not here: its ten skills, food running
//! down, devotion, levelling, evolving, and its own equipment. Those are named
//! in the record because the packet carries them and a zero has to mean
//! "none", not "we forgot".

use crate::store::Item;

/// The client ids a pran with a body may take.
///
/// A fourth range beside players (1..2000), NPCs (2048..3048) and objects
/// (10148..11147). Nothing here overlaps, which is what keeps a companion from
/// being drawn on top of a townsperson.
pub const IDS: std::ops::RangeInclusive<u32> = 44241..=45240;

/// What each level adds (`PRAN_HP_INC_PER_LEVEL`, `PRAN_MP_INC_PER_LEVEL`).
pub const HP_PER_LEVEL: u32 = 209;
pub const MP_PER_LEVEL: u32 = 356;

/// The item type of a Pran Summon Stone, and so the equipment slot it takes.
///
/// The two are the same number by the original's own rule: a type between one
/// and sixteen is worn in the slot of the same number.
pub const STONE_ITEM_TYPE: u16 = 10;
pub const STONE_SLOT: u16 = 10;

/// The three elements, which are the tens digit of the class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Fire,
    Water,
    Air,
}

impl Element {
    /// The element of a class code, or `None` for a code that is not a pran's.
    pub fn of(class: u8) -> Option<Self> {
        match class / 10 {
            6 => Some(Element::Fire),
            7 => Some(Element::Water),
            8 => Some(Element::Air),
            _ => None,
        }
    }

    /// The class a newly hatched pran of this element is.
    pub fn first_class(self) -> u8 {
        match self {
            Element::Fire => 61,
            Element::Water => 71,
            Element::Air => 81,
        }
    }

    /// The effect the client plays for a fairy of this element, which is all a
    /// first-tier pran is drawn as (`SendEffect(2 | 4 | 8)`).
    pub fn fairy_effect(self) -> u32 {
        match self {
            Element::Fire => 2,
            Element::Water => 4,
            Element::Air => 8,
        }
    }

    /// Where this element's ten skills start. They run ten apart: fire is
    /// 5761, 5771, ... 5851.
    pub fn first_skill(self) -> u32 {
        match self {
            Element::Fire => 5761,
            Element::Water => 5861,
            Element::Air => 5961,
        }
    }
}

/// How many skills a pran carries. The original's own comment says the array
/// is ten and may one day be twelve.
pub const SKILLS: usize = 10;

/// How far apart consecutive pran skills sit.
const SKILL_STRIDE: u32 = 10;

/// How many of them a freshly hatched pran knows.
const SKILLS_AT_BIRTH: usize = 3;

/// Which stone a pran of this class can be summoned with.
///
/// `TPlayer.GetPranClassStoneItem`: the first two tiers of every element share
/// one stone, the third has its own and the fourth another. The numbers are
/// not item ids -- they are what the stones carry in their `Classe` field, and
/// the table has seventeen stones spread across the three.
pub fn stone_tier(class: u8) -> Option<u16> {
    match class {
        61 | 62 | 71 | 72 | 81 | 82 => Some(100),
        63 | 73 | 83 => Some(101),
        64 | 74 | 84 => Some(102),
        _ => None,
    }
}

/// Whether a pran of this class walks beside its owner, rather than being an
/// effect on them.
///
/// This is `not PranIsFairy` (`Mob/Player.pas`), inverted and renamed: the
/// original is true for the first tier of each element, which is the form with
/// no body. Its name points the other way from what it means -- the winged
/// fairy is the form at the end of the line, not the start of it -- so the
/// test is kept and the name is not.
///
/// The original also treats a pran as bodiless while its owner is in
/// `FaericForm`, a player state we do not have. This is the class half.
pub fn has_body(class: u8) -> bool {
    !matches!(class, 61 | 71 | 81)
}

/// The six personalities, in the order the world packet numbers them.
///
/// The original picks the first whose score has reached the pran's devotion
/// (`SendPranToWorld`), which makes the order itself the tie-break.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Personality {
    pub cute: u16,
    pub smart: u16,
    pub sexy: u16,
    pub energetic: u16,
    pub tough: u16,
    pub corrupt: u16,
}

impl Personality {
    /// Which of the six the client is told about: the first that has caught up
    /// with devotion. A pran nobody has raised is none of them, and the
    /// original leaves the field at zero, which is also "cute".
    pub fn shown(&self, devotion: u32) -> u16 {
        let scores =
            [self.cute, self.smart, self.sexy, self.energetic, self.tough, self.corrupt];
        scores
            .iter()
            .position(|score| *score as u32 >= devotion)
            .map(|at| at as u16)
            .unwrap_or(0)
    }
}

/// One companion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pran {
    /// Row id, zero before it has been stored.
    pub id: i64,
    /// The `Identific` of the stone this one belongs to. A pran whose stone is
    /// not worn stays where it is; it is not summoned by kind.
    pub item_id: i32,
    pub name: String,
    pub level: u8,
    /// Element in the tens, tier in the units: 61 is a fire fairy, 64 the last
    /// fire form.
    pub class: u8,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub exp: u32,
    pub def_physical: u16,
    pub def_magic: u16,
    /// Counts down as the pran is out, and the digestive item halves it.
    pub food: u8,
    pub devotion: u8,
    pub personality: Personality,
    /// Build, the same three the character carries.
    pub width: u8,
    pub chest: u8,
    pub leg: u8,
    /// The ten skills, by id, zero for one it does not know.
    pub skills: [u32; SKILLS],
    /// Which three it has on its bar.
    pub bar: [u8; 3],
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for Pran {
    fn default() -> Self {
        Self {
            id: 0,
            item_id: 0,
            name: String::new(),
            level: 1,
            class: 0,
            hp: 0,
            max_hp: 0,
            mp: 0,
            max_mp: 0,
            exp: 0,
            def_physical: 0,
            def_magic: 0,
            food: 0,
            devotion: 0,
            personality: Personality::default(),
            width: 0,
            chest: 0,
            leg: 0,
            skills: [0; SKILLS],
            bar: [0; 3],
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl Pran {
    /// A newly hatched pran, exactly as `FinishQuest` builds one.
    ///
    /// The three quests that hand one out -- 39 fire, 40 water, 41 air -- each
    /// set the same fields to different numbers, and these are those numbers
    /// (`PacketHandlers/NPCHandlers.pas`, in `FinishQuest`). It knows its ten
    /// skills by id and has learned the first three.
    ///
    /// Fire is the tough one, water the one that thinks, air between them.
    pub fn hatch(element: Element, item_id: i32, now: i64) -> Self {
        let (max_hp, max_mp, def_physical, def_magic) = match element {
            Element::Fire => (383, 235, 239, 104),
            Element::Water => (209, 356, 153, 308),
            Element::Air => (255, 267, 201, 205),
        };

        let mut skills = [0u32; SKILLS];
        for (at, skill) in skills.iter_mut().enumerate() {
            *skill = element.first_skill() + at as u32 * SKILL_STRIDE;
        }

        Self {
            item_id,
            level: 1,
            class: element.first_class(),
            hp: max_hp,
            max_hp,
            mp: max_mp,
            max_mp,
            def_physical,
            def_magic,
            skills,
            created_at: now,
            updated_at: now,
            ..Self::default()
        }
    }

    pub fn element(&self) -> Option<Element> {
        Element::of(self.class)
    }

    /// Whether it walks beside its owner or is only a glow on them.
    pub fn has_body(&self) -> bool {
        has_body(self.class)
    }

    /// How many of its ten skills it has learned. A hatchling knows three.
    pub fn known_skills(&self) -> usize {
        self.skills.iter().take_while(|s| **s != 0).count().min(SKILLS)
    }

    /// Whether this stone is the one it belongs to.
    pub fn belongs_to(&self, stone: &Item) -> bool {
        self.item_id != 0 && self.item_id == stone.identific
    }
}

/// Whether an item is a Pran Summon Stone.
pub fn is_stone(item_type: u16) -> bool {
    item_type == STONE_ITEM_TYPE
}

/// How many of the ten skills a hatchling has learned, which the record shows
/// as the first three carrying a level.
pub fn skills_at_birth() -> usize {
    SKILLS_AT_BIRTH
}

/// `TSendPranToWorld` (`Data/Packets.pas:715`): the whole companion, which is
/// what draws its window.
pub const OP_WORLD: u16 = 0x907;

/// `TSendCreatePranPacket` (`Data/Packets.pas:380`): the companion standing
/// beside its owner. The same opcode a player or an NPC is spawned with.
pub const OP_SPAWN: u16 = 0x349;

/// Where each field sits in the body, the header already past.
pub mod at {
    pub const NAME: usize = 0;
    pub const CLASS: usize = 16;
    pub const FOOD: usize = 17;
    pub const PERSONALITY: usize = 18;
    pub const DEVOTION: usize = 20;
    pub const MAX_HP: usize = 24;
    pub const CUR_HP: usize = 28;
    pub const MAX_MP: usize = 32;
    pub const CUR_MP: usize = 36;
    pub const EXP: usize = 40;
    pub const DEF_PHYSICAL: usize = 44;
    pub const DEF_MAGIC: usize = 46;
    /// Sixteen bytes the original packs its skill levels into. See
    /// [`super::world_body`].
    pub const SKILL_LEVELS: usize = 48;
    /// Sixteen `TItem`, the pran's own gear.
    pub const EQUIPMENT: usize = 64;
    /// Forty-two more: forty slots and two bags.
    pub const INVENTORY: usize = EQUIPMENT + 16 * ITEM;
    pub const BAR: usize = INVENTORY + 42 * ITEM;
    /// `TItem`, the same twenty bytes it is everywhere else.
    pub const ITEM: usize = 20;
}

/// How long the body is: everything above, plus forty-one trailing bytes the
/// original leaves zeroed.
pub const WORLD_BODY: usize = at::BAR + 3 + 41;

/// The body of `0x907` for one companion.
///
/// # The sixteen bytes of skill levels are left alone
///
/// The original fills them from `GetSkillPranLevel`, which is a shape nobody
/// should reproduce from memory: `l := 2^Level - 1`, then for any skill past
/// the first `a := SkillIndex^4` (with one read as four), and `Level := l * a`
/// is written at *byte* `SkillIndex` in one or two bytes depending on whether
/// it fits. Consecutive skills therefore write over each other, and the
/// fourth power means the offsets are not even a bit shift.
///
/// None of the ten skills are granted yet, so zero is the truthful value:
/// the client draws no levels because there are none. The transcription
/// above is here so whoever grants them does not have to find it again.
pub fn world_body(pran: &Pran) -> Vec<u8> {
    let mut out = vec![0u8; WORLD_BODY];

    let name = pran.name.as_bytes();
    let len = name.len().min(15);
    out[at::NAME..at::NAME + len].copy_from_slice(&name[..len]);

    out[at::CLASS] = pran.class;
    out[at::FOOD] = pran.food;
    let put16 = |out: &mut [u8], offset: usize, value: u16| {
        out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    };
    let put32 = |out: &mut [u8], offset: usize, value: u32| {
        out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };

    put16(&mut out, at::PERSONALITY, pran.personality.shown(pran.devotion as u32));
    put32(&mut out, at::DEVOTION, pran.devotion as u32);
    put32(&mut out, at::MAX_HP, pran.max_hp);
    put32(&mut out, at::CUR_HP, pran.hp);
    put32(&mut out, at::MAX_MP, pran.max_mp);
    put32(&mut out, at::CUR_MP, pran.mp);
    put32(&mut out, at::EXP, pran.exp);
    put16(&mut out, at::DEF_PHYSICAL, pran.def_physical);
    put16(&mut out, at::DEF_MAGIC, pran.def_magic);

    for (slot, skill) in pran.bar.iter().enumerate() {
        out[at::BAR + slot] = *skill;
    }

    out
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The packet is a fixed size the client reads by offset, so the length
    /// is part of the contract and not an implementation detail.
    #[test]
    fn the_world_packet_is_the_size_the_record_declares() {
        // 16 name + 1 class + 1 food + 2 personality + 4 devotion
        // + 16 of hp/mp + 4 exp + 4 defences + 16 skill levels
        // + 16 and 42 items + 3 bar + 41 trailing.
        assert_eq!(WORLD_BODY, 1268);
        assert_eq!(world_body(&Pran::hatch(Element::Fire, 1, 0)).len(), WORLD_BODY);
    }

    #[test]
    fn the_world_packet_carries_what_the_window_shows() {
        let mut pran = Pran::hatch(Element::Water, 5, 0);
        pran.name = "Nina".into();
        pran.food = 90;
        pran.devotion = 12;
        pran.hp = 100;
        pran.exp = 4242;
        pran.bar = [1, 2, 3];

        let body = world_body(&pran);
        let u32_at = |offset: usize| {
            u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap())
        };
        let u16_at = |offset: usize| {
            u16::from_le_bytes(body[offset..offset + 2].try_into().unwrap())
        };

        assert_eq!(&body[at::NAME..at::NAME + 4], b"Nina");
        assert_eq!(body[at::NAME + 4], 0, "the name is not terminated");
        assert_eq!(body[at::CLASS], 71);
        assert_eq!(body[at::FOOD], 90);
        assert_eq!(u32_at(at::DEVOTION), 12);
        assert_eq!((u32_at(at::MAX_HP), u32_at(at::CUR_HP)), (209, 100));
        assert_eq!((u32_at(at::MAX_MP), u32_at(at::CUR_MP)), (356, 356));
        assert_eq!(u32_at(at::EXP), 4242);
        assert_eq!((u16_at(at::DEF_PHYSICAL), u16_at(at::DEF_MAGIC)), (153, 308));
        assert_eq!(&body[at::BAR..at::BAR + 3], &[1, 2, 3]);
    }

    /// A name at the limit must still leave its terminator, or the client
    /// reads on into the class byte.
    #[test]
    fn a_long_name_is_cut_short_of_its_terminator() {
        let mut pran = Pran::hatch(Element::Air, 1, 0);
        pran.name = "aaaaaaaaaaaaaaaaaaaa".into();

        let body = world_body(&pran);
        assert_eq!(body[at::NAME + 15], 0, "the name ran into the class");
        assert_eq!(body[at::CLASS], 81);
    }

    /// The gear and the bags are zero because a hatchling has neither, and a
    /// zero there has to read as an empty slot rather than as item zero.
    #[test]
    fn a_hatchling_carries_nothing() {
        let body = world_body(&Pran::hatch(Element::Fire, 1, 0));
        assert!(body[at::EQUIPMENT..at::BAR].iter().all(|b| *b == 0));
        assert!(
            body[at::SKILL_LEVELS..at::EQUIPMENT].iter().all(|b| *b == 0),
            "skill levels are not ours to fill in yet"
        );
    }
    #[test]
    fn the_element_is_the_tens_digit() {
        assert_eq!(Element::of(61), Some(Element::Fire));
        assert_eq!(Element::of(64), Some(Element::Fire));
        assert_eq!(Element::of(71), Some(Element::Water));
        assert_eq!(Element::of(84), Some(Element::Air));
        assert_eq!(Element::of(0), None, "no pran at all");
        assert_eq!(Element::of(51), None, "that is a Cleriga");
    }

    /// Only the first tier of each element is the bodiless glow. Drawing one
    /// as a companion would put a second character on the field that the
    /// client has no model for; not drawing the others leaves the player with
    /// a pran that shows as nothing at all.
    #[test]
    fn only_the_first_form_of_each_element_lacks_a_body() {
        for class in [61u8, 71, 81] {
            assert!(!has_body(class), "class {class} is the glow");
        }
        for class in [62u8, 63, 64, 72, 73, 74, 82, 83, 84] {
            assert!(has_body(class), "class {class} walks beside its owner");
        }
    }

    /// Every class the elements have must fit a stone, or a pran exists that
    /// nothing can summon.
    #[test]
    fn every_pran_class_has_a_stone() {
        for element in [Element::Fire, Element::Water, Element::Air] {
            for tier in 1..=4u8 {
                let class = element.first_class() + tier - 1;
                assert!(
                    stone_tier(class).is_some(),
                    "class {class} has no stone to be summoned with"
                );
            }
        }
        assert_eq!(stone_tier(61), Some(100));
        assert_eq!(stone_tier(63), Some(101));
        assert_eq!(stone_tier(64), Some(102));
        assert_eq!(stone_tier(51), None, "not a pran class");
    }

    /// The numbers are the original's, and each element is shaped differently:
    /// fire takes hits, water casts, air is between them.
    #[test]
    fn hatching_gives_the_numbers_the_quest_gives() {
        let fire = Pran::hatch(Element::Fire, 7, 1000);
        assert_eq!((fire.class, fire.max_hp, fire.max_mp), (61, 383, 235));
        assert_eq!((fire.def_physical, fire.def_magic), (239, 104));

        let water = Pran::hatch(Element::Water, 7, 1000);
        assert_eq!((water.class, water.max_hp, water.max_mp), (71, 209, 356));
        assert_eq!((water.def_physical, water.def_magic), (153, 308));

        let air = Pran::hatch(Element::Air, 7, 1000);
        assert_eq!((air.class, air.max_hp, air.max_mp), (81, 255, 267));
        assert_eq!((air.def_physical, air.def_magic), (201, 205));

        for pran in [&fire, &water, &air] {
            assert_eq!(pran.hp, pran.max_hp, "it should not hatch wounded");
            assert_eq!(pran.mp, pran.max_mp);
            assert_eq!(pran.level, 1);
            assert!(!pran.has_body(), "a hatchling is only a glow");
        }
    }

    /// Ten skills, ten apart, starting where the element starts.
    #[test]
    fn a_hatchling_carries_its_elements_ten_skills() {
        let fire = Pran::hatch(Element::Fire, 1, 0);
        assert_eq!(fire.skills[0], 5761);
        assert_eq!(fire.skills[1], 5771);
        assert_eq!(fire.skills[9], 5851);
        assert_eq!(fire.known_skills(), SKILLS);

        assert_eq!(Pran::hatch(Element::Water, 1, 0).skills[0], 5861);
        assert_eq!(Pran::hatch(Element::Air, 1, 0).skills[0], 5961);
    }

    /// The three elements must not share a skill, or learning one would teach
    /// another element's.
    #[test]
    fn the_three_elements_do_not_share_a_skill() {
        let mut all: Vec<u32> = [Element::Fire, Element::Water, Element::Air]
            .iter()
            .flat_map(|e| Pran::hatch(*e, 1, 0).skills)
            .collect();
        let before = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), before, "two elements share a skill id");
    }

    /// A pran belongs to one stone, not to a kind of stone. Two stones of the
    /// same item are two different homes.
    #[test]
    fn a_pran_belongs_to_the_one_stone_it_was_hatched_in() {
        let pran = Pran::hatch(Element::Fire, 4242, 0);
        let hers = Item { index: 100, identific: 4242, ..Item::default() };
        let his = Item { index: 100, identific: 9999, ..Item::default() };

        assert!(pran.belongs_to(&hers));
        assert!(!pran.belongs_to(&his), "it answered to somebody else's stone");
    }

    /// And a pran with no stone recorded answers to none, rather than to every
    /// item whose identific has not been filled in.
    #[test]
    fn a_pran_with_no_stone_answers_to_nothing() {
        let pran = Pran { item_id: 0, ..Pran::hatch(Element::Fire, 0, 0) };
        assert!(!pran.belongs_to(&Item { identific: 0, ..Item::default() }));
    }

    #[test]
    fn the_personality_shown_is_the_first_to_reach_devotion() {
        let p = Personality { cute: 3, smart: 10, sexy: 20, ..Personality::default() };
        assert_eq!(p.shown(5), 1, "smart is the first at or above five");
        assert_eq!(p.shown(3), 0, "cute reaches three exactly");
        assert_eq!(p.shown(50), 0, "none of them, which reads as the first");
    }

    /// The id range is its own. A pran drawn on a player's id would be drawn
    /// as that player.
    #[test]
    fn pran_ids_do_not_meet_anybody_elses() {
        assert_eq!(IDS.clone().count(), 1000);
        for taken in [1u32, 2000, 2048, 3048, 10148, 11147] {
            assert!(!IDS.contains(&taken), "{taken} belongs to somebody else");
        }
    }
}
