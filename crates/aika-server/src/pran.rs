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
//! # What is here
//!
//! The record and where it is kept. Hatching one from the stone a quest
//! hands out, with the numbers `FinishQuest` gives it. Naming it, which the
//! client will not let a pran out of the chest without. Summoning and
//! dismissing it with the stone, and drawing it -- an effect for the first
//! form and a body of its own for every one after. Experience from what its
//! owner kills, the levels that buys, the walls at 4, 19 and 49, and the
//! quest that evolves it past one.
//!
//! Not here: food running down, devotion rising, the pran's own bag and the
//! six equipment slots it can wear things in (`PRAN_EQUIP_TYPE`, a fourth
//! container nothing reaches yet), and the ten skills doing anything beyond
//! being counted.
//!
//! # One thing does not work, and it is worth writing down why not
//!
//! The companion's own window -- the panel with its portrait, its name and
//! its "Grau" -- keeps drawing the first form however far the pran has come.
//! The body beside the player is right, the window is not.
//!
//! What has been ruled out, each by trying it rather than by reasoning:
//!
//! - Every packet the original sends for a pran is sent, and no packet it
//!   sends is missing. `SetPranEquipAtributes` and `SetPranPassiveSkill` send
//!   none, and `SendPranDevotionAndFood` is never called by anything.
//! - The order is the original's, and it differs between the two paths:
//!   spawn then describe on arrival, describe then spawn when evolving.
//! - Every field of `0x907` is filled, including the sixteen bytes of skill
//!   levels that were blank for most of this system's life.
//! - The level goes out on `0x116`, which is the only packet that carries
//!   one, at summon as well as on a gain.
//! - The class, the stone in the pran's own slot zero, and the stone the
//!   player wears have all been set together and separately.
//!
//! Two things would settle it and neither is more guessing at this end: a
//! `0x907` captured from the original with a grown pran, to diff against
//! ours; or hooking the client itself through the d3d9 overlay in this
//! repository, to see which field it reads the portrait from.

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

    /// What the skill bar counts from, which is one below the first skill
    /// (`baseSkillPran` in `AddPranLevel`: 5760, 5860, 5960).
    pub fn skill_base(self) -> u32 {
        self.first_skill() - 1
    }
}

/// How many skills a companion carries on its bar (`ItemBar: Array [0..2]`).
pub const BAR_SLOTS: usize = 3;

/// What the bar holds for a skill: the id counted from its element's base, so
/// the same slot is the same number whichever element the pran is -- the
/// fourth skill is 31 for fire, water and air alike.
///
/// The original validates it against `SkillData[SrcIndex + 5760]`, the fire
/// base, whatever element the companion is. It gets away with it because the
/// three elements mirror each other slot for slot, and it is why the number
/// on the wire is an offset rather than an id.
pub fn bar_value(element: Element, id: u32) -> Option<u8> {
    id.checked_sub(element.skill_base()).and_then(|v| u8::try_from(v).ok())
}

/// And back: the skill a bar entry names.
pub fn bar_skill(element: Element, value: u8) -> u32 {
    element.skill_base() + value as u32
}

/// How many of the pran's equipment slots the spawn packet carries. Its
/// record holds sixteen; only the first eight are drawn.
pub const EQUIPMENT_SLOTS: usize = 8;

/// What a newly hatched pran is given to hold, in slot six
/// (`FinishQuest`, with no explanation of what it is).
pub const HATCHLING_HELD_ITEM: u16 = 7780;
/// And in the fortieth slot of its own bag.
pub const HATCHLING_BAG_ITEM: u16 = 5301;

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
    /// The eight equipment slots the spawn carries, by item index.
    ///
    /// The first is what the client draws it as. In the player spawn that
    /// this packet is a copy of, `Equip[0]` is the model; for a pran it is
    /// the summon stone the pran wears, which `FinishQuest` puts there
    /// when it makes one. Leave it zero and the client falls back to a
    /// bare human body.
    pub equipment: [u16; EQUIPMENT_SLOTS],
    /// How far each of the ten has been raised. A hatchling has the first
    /// three at one and the rest at nothing.
    pub skill_levels: [u8; SKILLS],
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
            equipment: [0; EQUIPMENT_SLOTS],
            skill_levels: [0; SKILLS],
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
    /// The three quests that hand one out -- 39 fire, 40 water, 41 air --
    /// differ only in four numbers; everything below them is the same for all
    /// three (`PacketHandlers/NPCHandlers.pas`, in `FinishQuest`). Fire is
    /// the tough one, water the one that thinks, air between them.
    ///
    /// The stone is both halves of what a pran needs: its `Identific` is what
    /// binds them, and its item index is what the pran is *drawn* as, worn in
    /// its own first equipment slot.
    pub fn hatch(element: Element, stone: &Item, now: i64) -> Self {
        let (max_hp, max_mp, def_physical, def_magic) = match element {
            Element::Fire => (383, 235, 239, 104),
            Element::Water => (209, 356, 153, 308),
            Element::Air => (255, 267, 201, 205),
        };

        let mut skills = [0u32; SKILLS];
        for (at, skill) in skills.iter_mut().enumerate() {
            *skill = element.first_skill() + at as u32 * SKILL_STRIDE;
        }

        let mut equipment = [0u16; EQUIPMENT_SLOTS];
        // It wears its own stone, which is what the client draws it from,
        // and holds one other thing the original does not name.
        equipment[0] = stone.index;
        equipment[6] = HATCHLING_HELD_ITEM;

        Self {
            item_id: stone.identific,
            // Zero, and not one. `FinishQuest` sets fifteen fields on a new
            // pran and `Level` is not among them: the record was zeroed and
            // stays zeroed, so a pran is born at nothing and the client is
            // told `Level + 1`, which is the one it shows. Starting at one
            // put every pran a level ahead of itself on the wire for the
            // whole of its life.
            level: 0,
            class: element.first_class(),
            hp: max_hp,
            max_hp,
            mp: max_mp,
            max_mp,
            // Not zero. The original starts the count at one.
            exp: 1,
            def_physical,
            def_magic,
            food: 121,
            devotion: 113,
            // Cute is well past devotion and the rest are well under, so a
            // hatchling reads as cute until it is raised into something else.
            personality: Personality {
                cute: 226,
                smart: 50,
                sexy: 50,
                energetic: 50,
                tough: 50,
                corrupt: 50,
            },
            // Its build. Zero here is what draws the misshapen half-height
            // human that the first version of this put on the field.
            width: 7,
            chest: 100,
            leg: 100,
            equipment,
            skill_levels: {
                let mut levels = [0u8; SKILLS];
                for level in levels.iter_mut().take(SKILLS_AT_BIRTH) {
                    *level = 1;
                }
                levels
            },
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

/// One skill's entry in the sixteen bytes, and how wide it is.
///
/// `TSkillFunctions.GetSkillPranLevel` line for line. The width it returns is
/// how many bytes of the value the caller copies, and it is one below 256 and
/// two above it.
fn skill_level_field(skill: usize, level: u8) -> (u32, usize) {
    let l = (1u32 << level.min(31)) - 1;
    if skill == 0 {
        // The first skill is the value on its own, and the width the original
        // returns for it is the one it started with.
        return (l, 1);
    }
    let mut a = (skill as u32).pow(4);
    if a == 1 {
        a = 4;
    }
    let value = l.saturating_mul(a);
    // The original's `case` names two ranges and its `Result` starts at one,
    // so a value past sixty-five thousand falls through every arm and is
    // written in **one** byte, not two. It is reachable: the fourth power
    // takes the sixth skill past that at level three.
    let width = match value {
        0..=255 => 1,
        256..=65535 => 2,
        _ => 1,
    };
    (value, width)
}

/// One `TItem` (`Data/MiscData.pas:44`), of which the window needs two fields.
///
/// `Index` is the item and `APP` what it is drawn as, and everything that
/// hands a pran an item sets the two to the same number (`FinishQuest`, and
/// each of the three evolutions). The rest of the twenty bytes -- the
/// identific, the effects, the durability, the refine and the licence -- a
/// pran's own gear has never carried.
fn write_item(out: &mut [u8], index: u16) {
    out[0..2].copy_from_slice(&index.to_le_bytes());
    out[2..4].copy_from_slice(&index.to_le_bytes());
}

/// The body of `0x907` for one companion.
///
/// # The sixteen bytes of skill levels
///
/// `GetSkillPranLevel`, transcribed rather than reasoned about, because it is
/// a shape nobody would arrive at twice: `l := 2^Level - 1`; for the first
/// skill the value is `l`, and for any other `a := SkillIndex^4` with one read
/// as four, and the value is `l * a`. It is written at *byte* `SkillIndex`, in
/// one byte if it fits and two if it does not -- so consecutive skills write
/// over each other, and the fourth power means the offsets are not a bit
/// shift either. A skill at level zero is skipped rather than written as zero.
///
/// These were left blank for most of this system's life on the grounds that
/// no skills were granted. They are granted now, and this is the field the
/// window is drawn from.
///
/// # And the gear, which is what draws the picture
///
/// The original copies the pran's sixteen equipment slots in whole
/// (`Move(Pran.Equip, Packet.Equips, ...)`), and the first of them is the
/// summon stone: 100, 101 or 102 for a fairy, then 104, then 105, then 111.
/// That is the same field the *spawn* packet reads to decide what body to
/// draw. Leaving it out of this one is why a companion could walk beside its
/// owner in its grown shape while its own window went on drawing the first
/// one -- the window had never been told which shape it was.
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

    for (at, level) in pran.skill_levels.iter().enumerate() {
        if *level == 0 {
            continue;
        }
        let (value, width) = skill_level_field(at, *level);
        let start = at::SKILL_LEVELS + at;
        let end = (start + width).min(at::EQUIPMENT);
        out[start..end].copy_from_slice(&value.to_le_bytes()[..end - start]);
    }

    // `Move(Pran.Equip, Packet.Equips, 16 * SizeOf(TItem))`. The first of them
    // is the summon stone, and it is the same field that tells the *spawn*
    // what body to draw -- which is why the companion beside the player has
    // been right all along and the window beside it has not.
    for (slot, index) in pran.equipment.iter().enumerate() {
        if *index == 0 {
            continue;
        }
        let start = at::EQUIPMENT + slot * at::ITEM;
        write_item(&mut out[start..start + at::ITEM], *index);
    }

    for (slot, skill) in pran.bar.iter().enumerate() {
        out[at::BAR + slot] = *skill;
    }

    out
}
/// `0x116`, which is the only packet that tells the client a pran's level.
///
/// This is the one that mattered. The pran's own description packet
/// (`OP_WORLD`) has no level field at all -- name, class, food, devotion,
/// hit points, experience, defences, gear, and no level -- and the client
/// does not work one out from the experience either. It waits to be told.
/// Until it is, the window reads level 1 whatever the experience says, and
/// the shape it draws is the shape of level 1.
pub const OP_LEVEL: u16 = 0x116;

/// `TRefreshPranLevelExpPacket`: a level and an experience, and the level
/// goes out one higher than it is held.
///
/// `SendPranLevelAndExp(Pran.Level + 1, Pran.Exp)`, at all four call sites.
/// The same off-by-one the character's own level travels with, which this
/// project already knows to convert at the edge and never to store.
pub fn level_body(level: u8, exp: u32) -> Vec<u8> {
    let mut out = vec![0u8; 12];
    out[0..4].copy_from_slice(&(level as u32 + 1).to_le_bytes());
    out[4..12].copy_from_slice(&(exp as u64).to_le_bytes());
    out
}

/// The highest level a pran reaches, as the original holds it.
///
/// `MAX_PRAN_LEVEL: word = 20` (`Data/GlobalDefs.pas:135`), which sits oddly
/// beside a growth table of 150 entries and forms that run to 69. It is a
/// typed constant rather than a real one, so it was meant to be raised. What
/// `AddPranExp` actually enforces is this number, so this is the number.
pub const MAX_LEVEL: u8 = 20;

/// The experience each level costs, read from `Data/PranExpList.bin`.
///
/// Plain little-endian dwords, one per level, no header and no terminator:
/// `SetLength(PranExpList, FSize div sizeof(DWORD))` and one read
/// (`Functions/Load.pas:809`). Six hundred bytes, so a hundred and fifty
/// levels, of which the game uses seventy.
#[derive(Debug, Default, Clone)]
pub struct ExpCurve {
    thresholds: Vec<u32>,
}

impl ExpCurve {
    pub fn decode(bytes: &[u8]) -> Self {
        Self {
            thresholds: bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().expect("four bytes")))
                .collect(),
        }
    }

    /// What a pran needs to be this level. Past the end of the table the
    /// last entry stands, so a missing file cannot hand out infinite levels.
    pub fn threshold(&self, level: u8) -> u32 {
        self.thresholds.get(level as usize).copied().unwrap_or(u32::MAX)
    }

    pub fn levels(&self) -> usize {
        self.thresholds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.thresholds.is_empty()
    }
}

/// What came of giving a companion a share of a kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    /// Standing at a wall it cannot pass, so nothing was earned at all. The
    /// original says so out loud: "A sua pran precisa evoluir para ganhar
    /// exp".
    MustEvolve,
    /// Earned, and grew this many levels doing it.
    Grew { levels: u8 },
}

/// A companion's share of what its owner killed: a fifth.
///
/// `PranExpAcquired := (ExpAcquired div 5)` -- the same line in all six
/// branches of the switch that hands it out.
pub fn share_of_kill(experience: u64) -> u32 {
    (experience / 5).min(u32::MAX as u64) as u32
}

/// Gives a companion experience, levelling it as far as that reaches.
///
/// Two things stop it. The wall for its form, which it can reach but not
/// pass without evolving, and [`MAX_LEVEL`]. At either it keeps the
/// experience it is standing on rather than a number past the end of what
/// it may hold.
pub fn add_exp(pran: &mut Pran, gained: u32, curve: &ExpCurve) -> Growth {
    if must_evolve(pran.level, pran.class) {
        return Growth::MustEvolve;
    }

    // The wall ahead, not the one underfoot. A pran that has evolved is
    // standing on the wall it just passed, and looking for the first wall at
    // or above its level would find that one and hold it there for ever.
    if let Some(wall) = WALLS.iter().copied().find(|wall| pran.level < *wall) {
        // The band tops out one level short of the next threshold, and a
        // kill that would carry it past lands it exactly on the wall.
        if pran.exp as u64 + gained as u64 > curve.threshold(wall + 1) as u64 {
            let levels = wall - pran.level;
            pran.exp = curve.threshold(wall);
            for _ in 0..levels {
                level_up(pran);
            }
            return Growth::Grew { levels };
        }
    }

    pran.exp = pran.exp.saturating_add(gained);
    let mut levels = 0;
    while pran.level + 1 < MAX_LEVEL && pran.exp > curve.threshold(pran.level + 1) {
        level_up(pran);
        levels += 1;
    }
    if pran.level + 1 == MAX_LEVEL && pran.exp > curve.threshold(pran.level + 1) {
        pran.exp = curve.threshold(pran.level + 1);
    }
    Growth::Grew { levels }
}

/// One level: more of everything, and full again.
///
/// `AddPranLevel`. The two increments are the original's own constants, and
/// a level fills a pran up the way a level fills a character up.
fn level_up(pran: &mut Pran) {
    if pran.level >= MAX_LEVEL {
        return;
    }
    pran.level += 1;
    pran.max_hp = pran.max_hp.saturating_add(HP_PER_LEVEL);
    pran.max_mp = pran.max_mp.saturating_add(MP_PER_LEVEL);
    pran.hp = pran.max_hp;
    pran.mp = pran.max_mp;
    raise_skills(pran);
}

/// Which of the ten skills a level raises.
///
/// Three bands, all of them read off `Level + 1` rather than the level, and
/// the middle one skips the fourth skill for reasons the original does not
/// give (`AddPranLevel`).
fn raise_skills(pran: &mut Pran) {
    let shown = pran.level as usize + 1;
    let raise: Vec<usize> = match shown {
        5..=30 => (0..=(pran.level as usize / 5) + 2).collect(),
        35..=50 => (0..SKILLS).filter(|i| *i != 3).collect(),
        55..=70 => (4..SKILLS).collect(),
        _ => Vec::new(),
    };
    for at in raise {
        if let Some(level) = pran.skill_levels.get_mut(at) {
            *level = level.saturating_add(1);
        }
    }
}
/// Evolving: what the quest at each wall does.
///
/// The level carries the shape and stops at 4, 19 and 49. What lifts it is
/// not levelling harder -- it is a quest, and the original's own comments
/// name them: `406: // isso aqui e a quest Evolucao pran Lv5` and
/// `407: // ... Lv20` (`PacketHandlers/NPCHandlers.pas:1563`). Both belong to
/// NPC 2072, the same one that hands out prans in the first place.
///
/// Nothing else in the whole source ever writes a class of 62, 63 or 64.
/// These two quests are the only way a pran has ever changed shape.
///
/// # The stone changes too, in two places
///
/// This is the part that is easy to miss and impossible to work around.
/// Evolving swaps the summon stone -- 100, 101 or 102 for a fairy, then 104,
/// then 105, then 111 -- and it swaps it *both* in the pran's own first slot
/// **and in equipment slot ten of the player**, which is the one the player
/// is wearing. Changing only the pran's copy leaves the owner holding the
/// stone of a form their companion no longer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evolution {
    /// The class it becomes.
    pub class: u8,
    /// The stone it is now carried in, which the owner wears as well.
    pub stone: u16,
    /// Whether the fairy effect has to be taken off the player. Only the
    /// first evolution does it, because only before it was there one.
    pub clears_the_glow: bool,
}

/// What each wall's quest turns the stone into.
///
/// Three walls, three stones, and the third needs saying. `Quests.csv` has
/// five lines for NPC 2072 -- 39, 40 and 41 to make a pran, 406 and 407 to
/// evolve one -- and no line for level 49. The *code* has one:
/// `408: // quest Evolucao pran lv50 (fazer nos proximos caps)`
/// (`NPCHandlers.pas:1845`), with the same shape as the other two and stone
/// 111, sitting there waiting for a data line that never shipped.
///
/// Evolving here is driven by the NPC's own Quest option rather than by a
/// line of that file, the same substitution promotion makes, so there is no
/// missing line to stop it and an adolescent that would otherwise stand at
/// the last wall for ever can go through it. The numbers are the original's.
const EVOLUTION_STONES: [(u8, u16); 3] = [(4, 104), (19, 105), (49, 111)];

/// What a hatchling holds in slot six, and what the first evolution puts
/// there instead.
pub const CHILD_HELD_ITEM: u16 = 150;

/// How many ranks each of the ten skills has, which is what puts them ten
/// apart: 5761 to 5770 is the first, 5771 the second.
pub const RANKS_PER_SKILL: u32 = 10;

/// Which of the ten a skill id belongs to, for a companion of this element.
pub fn skill_slot(element: Element, id: u32) -> Option<usize> {
    let offset = id.checked_sub(element.first_skill())?;
    let slot = (offset / RANKS_PER_SKILL) as usize;
    (slot < SKILLS).then_some(slot)
}

/// Which of the ten skills the first evolution raises.
///
/// `Inc(Pran1.Skills[3].Level)`, with a comment beside it calling it
/// "transformar" and noting that this one must not grow from kills like the
/// others -- which is why the band that raises skills on a level skips
/// exactly this index.
pub const TRANSFORM_SKILL: usize = 3;

/// Why a companion cannot evolve yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotYet {
    /// Not standing on a wall. It can only be done at the exact level, not
    /// before it and not after.
    NotAtAWall,
    /// At a wall it has already passed, or one the data has no quest for.
    NothingFurther,
}

impl NotYet {
    /// The original's own words where it has them.
    pub fn message(&self) -> &'static str {
        match self {
            NotYet::NotAtAWall => "Sua pran ainda nao esta pronta para evoluir.",
            NotYet::NothingFurther => "Essa pran nao pode ser upada de classe.",
        }
    }
}

/// Evolves a companion standing at a wall.
///
/// `FinishQuest` for 406, 407 and 408, which differ only in the stone and in
/// the two things the first one also does: it raises the transform skill and
/// it puts item 150 in slot six.
pub fn evolve(pran: &mut Pran) -> Result<Evolution, NotYet> {
    let Some(element) = Element::of(pran.class) else {
        return Err(NotYet::NothingFurther);
    };
    let tier = pran.class - element.first_class() + 1;

    let Some((at, stone)) = EVOLUTION_STONES
        .iter()
        .copied()
        .enumerate()
        .find(|(at, (wall, _))| pran.level == *wall && tier == *at as u8 + 1)
        .map(|(at, (_, stone))| (at, stone))
    else {
        // Either it is between walls, or it is at one it has already passed.
        return Err(if WALLS.contains(&pran.level) {
            NotYet::NothingFurther
        } else {
            NotYet::NotAtAWall
        });
    };

    pran.class += 1;
    pran.equipment[0] = stone;

    // Only the first one. The second sets no held item and raises nothing.
    let first = at == 0;
    if first {
        pran.equipment[6] = CHILD_HELD_ITEM;
        if let Some(level) = pran.skill_levels.get_mut(TRANSFORM_SKILL) {
            *level = level.saturating_add(1);
        }
    }

    Ok(Evolution { class: pran.class, stone, clears_the_glow: first })
}
/// What a pran looks like, which is decided by its level and not by its class.
///
/// The original writes the four out in its own comments, in the switch that
/// hands a pran a share of what its owner killed (`Mob/BaseMob.pas:6177`):
///
/// ```text
///  0..3   pran fada                     the fairy, with no body of its own
///  4      pran fada ~ pran crianca      a wall
///  5..18  pran crianca
///  19     pran crianca ~ adolescente    a wall
///  20..48 pran adolescente
///  49     adolescente ~ pran adulta     a wall
///  50..69 pran adulta
/// ```
///
/// This is the thing that took longest to see. The class looks like it should
/// decide the shape -- 61 to 64 per element, four codes for four forms -- and
/// it does not. Setting a pran to class 62 and leaving it at level 1 changes
/// nothing anybody can see: it is still drawn as a fairy, because the level
/// still says fairy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Fairy,
    Child,
    Teenager,
    Adult,
}

/// The last level of each form, in order. Reaching one is a wall.
///
/// A pran standing on it earns nothing until it evolves -- the original says
/// so in as many words: "A sua pran precisa evoluir para ganhar exp". The
/// class is what has to move; the level cannot pass until it does.
pub const WALLS: [u8; 3] = [4, 19, 49];

/// The highest level a pran reaches at all.
pub const LEVEL_CAP: u8 = 69;

impl Form {
    pub fn of_level(level: u8) -> Self {
        match level {
            0..=4 => Form::Fairy,
            5..=19 => Form::Child,
            20..=49 => Form::Teenager,
            _ => Form::Adult,
        }
    }

    /// The tier a pran of this form has to have reached, counted from one.
    /// A fairy is the first tier, an adult the fourth.
    pub fn tier(self) -> u8 {
        match self {
            Form::Fairy => 1,
            Form::Child => 2,
            Form::Teenager => 3,
            Form::Adult => 4,
        }
    }
}

/// Whether a pran is standing at a wall it cannot pass.
///
/// True when the level has reached one of the three and the class is still the
/// one below it. The original tests exactly this and stops the experience:
/// at 4 while the class is 61, 71 or 81; at 19 while it is 62, 72 or 82; at
/// 49 while it is 63, 73 or 83.
pub fn must_evolve(level: u8, class: u8) -> bool {
    let Some(element) = Element::of(class) else {
        return false;
    };
    let tier = class - element.first_class() + 1;
    WALLS.iter().enumerate().any(|(at, wall)| level == *wall && tier == at as u8 + 1)
}

/// The class a pran of this class becomes when it evolves, or `None` when
/// there is nothing further.
pub fn evolved(class: u8) -> Option<u8> {
    let element = Element::of(class)?;
    let tier = class - element.first_class() + 1;
    (tier < WALLS.len() as u8 + 1).then_some(class + 1)
}
/// The stone each of the three quests hands out, and so which element it
/// hatches.
///
/// `Data/Quest/Quests.csv` in the original's own data, three lines that say it
/// outright: NPC 2072, quests 39, 40 and 41, type 21, one reward each -- item
/// 100, 101 and 102. Fire, water, air, in the order `FinishQuest` reads them.
///
/// This is why the element does not have to be chosen. It is written on the
/// stone, and a stone that is not one of the three hatches nothing: the rest of
/// the seventeen are carriers for a pran that already exists, sorted by the
/// tier they fit rather than by element (see [`stone_tier`]).
pub fn element_of_quest_stone(item: u16) -> Option<Element> {
    match item {
        100 => Some(Element::Fire),
        101 => Some(Element::Water),
        102 => Some(Element::Air),
        _ => None,
    }
}

/// The NPC the three of them belong to, which is the one whose menu carries
/// the Pran station.
pub const QUEST_NPC: u16 = 2072;

/// `TRenamePranPacket` (`Data/Packets.pas:679`): the name a player typed.
pub const OP_RENAME: u16 = 0x3E02;

/// A companion is named once and keeps it. The original has no way to change
/// one: `RenamePran` names the first pran that has none and refuses when
/// they all do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    /// The client sends one, and the original never reads it -- it names the
    /// first unnamed pran whatever this says. Kept because the answer is the
    /// same packet sent back.
    pub slot: u32,
    pub name: String,
}

impl Rename {
    pub const BODY_SIZE: usize = 24;
    const NAME_AT: usize = 4;
    const ACCOUNT_AT: usize = 20;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        let raw = &body[Self::NAME_AT..Self::ACCOUNT_AT];
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        Some(Self {
            slot: u32::from_le_bytes(body[0..4].try_into().ok()?),
            name: String::from_utf8_lossy(&raw[..end]).into_owned(),
        })
    }

    /// The answer, which is the question with the account filled in.
    pub fn to_body(&self, account_id: u32) -> Vec<u8> {
        let mut out = vec![0u8; Self::BODY_SIZE];
        out[0..4].copy_from_slice(&self.slot.to_le_bytes());
        let name = self.name.as_bytes();
        let len = name.len().min(NAME_MAX);
        out[Self::NAME_AT..Self::NAME_AT + len].copy_from_slice(&name[..len]);
        out[Self::ACCOUNT_AT..Self::ACCOUNT_AT + 4].copy_from_slice(&account_id.to_le_bytes());
        out
    }
}

/// Sixteen bytes with room for a terminator.
pub const NAME_MAX: usize = 15;

/// Whether a name is one the original would accept.
///
/// `TFunctions.IsLetter` is the whole test, and it does not mean what it is
/// called: its alphabet is `['a'..'z', 'A'..'Z', '0'..'9']`, so digits pass.
/// Empty fails, because the check starts from `Length(Text) > 0`.
///
/// The length cap is ours. The original copies sixteen bytes into a sixteen
/// byte array with `StrPLCopy` and lets the terminator fall off the end; a
/// name that long comes back out running into whatever follows it.
pub fn name_is_allowed(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.chars().all(|c| c.is_ascii_alphanumeric())
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A summon stone with an identific of its own, which is both halves of
    /// what hatching needs: what to bind to, and what to be drawn as.
    fn stone_of(identific: i32) -> Item {
        Item { index: 100, identific, ..Item::default() }
    }

    /// `GetSkillPranLevel`, checked against the numbers it produces rather
    /// than against a reading of what it means -- because nobody knows what it
    /// means. A fourth power and a byte offset that overlap on purpose or by
    /// accident, and either way the client reads what it reads.
    #[test]
    fn a_skills_entry_is_the_original_arithmetic() {
        // the first is the mask on its own
        assert_eq!(skill_level_field(0, 1), (1, 1));
        assert_eq!(skill_level_field(0, 4), (15, 1));

        // one is read as four rather than as one
        assert_eq!(skill_level_field(1, 1), (4, 1));
        assert_eq!(skill_level_field(2, 1), (16, 1), "two to the fourth");
        assert_eq!(skill_level_field(3, 1), (81, 1), "three to the fourth");

        // and past a byte it takes two
        assert_eq!(skill_level_field(4, 1), (256, 2));
        assert_eq!(skill_level_field(9, 2), (3 * 6561, 2));
    }

    /// A level of zero means the skill is not there at all, and the original
    /// skips it rather than writing a zero over whatever the byte held.
    #[test]
    fn a_skill_at_no_level_writes_nothing() {
        let mut pran = Pran::hatch(Element::Fire, &stone_of(1), 0);
        pran.skill_levels = [0; SKILLS];
        pran.skill_levels[5] = 2;

        let body = world_body(&pran);
        assert_eq!(body[at::SKILL_LEVELS], 0, "the first was written anyway");
        assert_ne!(body[at::SKILL_LEVELS + 5], 0, "the one that has a level was not");
    }

    /// The packet is a fixed size the client reads by offset, so the length
    /// is part of the contract and not an implementation detail.
    #[test]
    fn the_world_packet_is_the_size_the_record_declares() {
        // 16 name + 1 class + 1 food + 2 personality + 4 devotion
        // + 16 of hp/mp + 4 exp + 4 defences + 16 skill levels
        // + 16 and 42 items + 3 bar + 41 trailing.
        assert_eq!(WORLD_BODY, 1268);
        assert_eq!(world_body(&Pran::hatch(Element::Fire, &stone_of(1), 0)).len(), WORLD_BODY);
    }

    #[test]
    fn the_world_packet_carries_what_the_window_shows() {
        let mut pran = Pran::hatch(Element::Water, &stone_of(5), 0);
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

    /// The whole of the reported bug: a companion walking beside its owner in
    /// its grown shape while its own window drew the first one. The window is
    /// drawn from the stone in the first equipment slot, exactly as the body
    /// is, and the window packet was not carrying it.
    #[test]
    fn a_grown_companions_window_says_which_shape_it_is() {
        let mut pran = Pran { level: 4, ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
        for (level, class, stone) in [(4u8, 62u8, 104u16), (19, 63, 105), (49, 64, 111)] {
            pran.level = level;
            evolve(&mut pran).expect("it would not evolve");
            assert_eq!(pran.class, class);

            let body = world_body(&pran);
            assert_eq!(body[at::CLASS], class, "the window was told the wrong class");
            assert_eq!(
                u16::from_le_bytes(body[at::EQUIPMENT..at::EQUIPMENT + 2].try_into().unwrap()),
                stone,
                "the window is still holding the stone of a shape it no longer is",
            );
        }
    }

    /// `GetSkillPranLevel` returns its width from a `case` with two arms over
    /// a `Result` that starts at one, so a value past sixty-five thousand
    /// matches neither and is written in one byte. Reachable: the fourth power
    /// takes the sixth skill past that at level three.
    #[test]
    fn a_skill_value_too_big_for_either_arm_is_written_in_one_byte() {
        assert_eq!(skill_level_field(0, 9), (511, 1), "the first skill is never scaled");
        assert_eq!(skill_level_field(1, 1), (4, 1), "one to the fourth is read as four");
        assert_eq!(skill_level_field(2, 9), (8176, 2));

        let (value, width) = skill_level_field(6, 3);
        assert_eq!(value, 7 * 1296);
        assert_eq!(width, 2, "still inside the second arm");

        let (value, width) = skill_level_field(6, 9);
        assert_eq!(value, 511 * 1296);
        assert_eq!(width, 1, "past both arms, so the original writes one byte");
    }

    /// A name at the limit must still leave its terminator, or the client
    /// reads on into the class byte.
    #[test]
    fn a_long_name_is_cut_short_of_its_terminator() {
        let mut pran = Pran::hatch(Element::Air, &stone_of(1), 0);
        pran.name = "aaaaaaaaaaaaaaaaaaaa".into();

        let body = world_body(&pran);
        assert_eq!(body[at::NAME + 15], 0, "the name ran into the class");
        assert_eq!(body[at::CLASS], 81);
    }

    /// The gear is what the window draws the companion's picture from, and a
    /// hatchling's is its own summon stone. This was asserted blank for as
    /// long as the field went out blank, which is how it went unnoticed that
    /// the window was never told which shape it was looking at.
    #[test]
    fn the_window_carries_the_stone_that_says_what_shape_it_is() {
        let stone = 100;
        let body = world_body(&Pran::hatch(Element::Fire, &stone_of(1), 0));

        assert_eq!(
            u16::from_le_bytes(body[at::EQUIPMENT..at::EQUIPMENT + 2].try_into().unwrap()),
            stone,
            "the window has nothing to draw the companion as"
        );
        assert_eq!(
            u16::from_le_bytes(body[at::EQUIPMENT + 2..at::EQUIPMENT + 4].try_into().unwrap()),
            stone,
            "everything that hands a pran an item sets the appearance to match"
        );

        let held = at::EQUIPMENT + 6 * at::ITEM;
        assert_eq!(
            u16::from_le_bytes(body[held..held + 2].try_into().unwrap()),
            HATCHLING_HELD_ITEM,
        );

        // An empty slot is still zero, or the client draws item nought in it.
        let empty = at::EQUIPMENT + at::ITEM;
        assert!(body[empty..empty + at::ITEM].iter().all(|b| *b == 0));
        // The bag is a container this server does not keep yet.
        assert!(body[at::INVENTORY..at::BAR].iter().all(|b| *b == 0));
        // The first three carry a level, so the field is not blank: it is
        // 1, 4, 16 -- `2^1 - 1` times one, four and sixteen.
        assert_eq!(&body[at::SKILL_LEVELS..at::SKILL_LEVELS + 3], &[1, 4, 16]);
        assert!(
            body[at::SKILL_LEVELS + 3..at::EQUIPMENT].iter().all(|b| *b == 0),
            "a skill at level zero is skipped, not written"
        );
    }
    /// The original's test is `IsLetter`, which allows digits despite the
    /// name. Getting this wrong either refuses names the client offered or
    /// lets through something the client cannot draw.
    #[test]
    fn a_name_is_letters_and_digits_and_nothing_else() {
        assert!(name_is_allowed("Nina"));
        assert!(name_is_allowed("Pran2"), "digits pass, whatever the name says");
        assert!(!name_is_allowed(""), "empty is not a name");
        assert!(!name_is_allowed("Ni na"), "a space is not a letter");
        assert!(!name_is_allowed("Nina!"));
        assert!(!name_is_allowed("Niña"), "nor anything outside ascii");
    }

    /// Sixteen bytes with a terminator is fifteen letters. The original lets
    /// the sixteenth push its terminator off the end.
    #[test]
    fn a_name_leaves_room_for_its_terminator() {
        assert!(name_is_allowed(&"a".repeat(NAME_MAX)));
        assert!(!name_is_allowed(&"a".repeat(NAME_MAX + 1)));
    }

    #[test]
    fn the_rename_packet_reads_and_answers() {
        let mut body = vec![0u8; Rename::BODY_SIZE];
        body[4..9].copy_from_slice(b"Alice");

        let asked = Rename::parse(&body).expect("a full packet did not parse");
        assert_eq!(asked.name, "Alice");
        assert_eq!(asked.slot, 0);

        // the answer is the question with the account filled in
        let answer = asked.to_body(7);
        assert_eq!(&answer[4..9], b"Alice");
        assert_eq!(u32::from_le_bytes(answer[20..24].try_into().unwrap()), 7);
        assert_eq!(Rename::parse(&answer).unwrap(), asked);
    }

    #[test]
    fn a_short_rename_packet_is_not_one() {
        assert_eq!(Rename::parse(&[0u8; Rename::BODY_SIZE - 1]), None);
    }
    /// The shipped curve, so the numbers under these tests are the real ones.
    fn curve() -> ExpCurve {
        let mut raw = Vec::new();
        // 0, 855, 2106, 3864, 6253, 9410 ... the first six of the file, then a
        // straight climb, which is enough for anything below the second wall.
        for (at, value) in [0u32, 855, 2106, 3864, 6253, 9410].into_iter().enumerate() {
            let _ = at;
            raw.extend_from_slice(&value.to_le_bytes());
        }
        for level in 6..150u32 {
            raw.extend_from_slice(&(9410 + (level - 5) * 4000).to_le_bytes());
        }
        ExpCurve::decode(&raw)
    }

    /// A fifth of the kill, which is the same line in all six branches.
    #[test]
    fn a_companion_takes_a_fifth_of_the_kill() {
        assert_eq!(share_of_kill(100), 20);
        assert_eq!(share_of_kill(4), 0, "a kill too small to divide");
    }

    /// The level packet is the only one that carries a level, and it carries
    /// it one higher than it is held.
    #[test]
    fn the_level_packet_sends_one_more_than_the_level() {
        let body = level_body(4, 6253);
        assert_eq!(u32::from_le_bytes(body[0..4].try_into().unwrap()), 5);
        assert_eq!(u64::from_le_bytes(body[4..12].try_into().unwrap()), 6253);
    }

    #[test]
    fn experience_carries_a_hatchling_up_through_the_fairy() {
        let curve = curve();
        let mut pran = Pran::hatch(Element::Fire, &stone_of(1), 0);
        let hp = pran.max_hp;

        assert_eq!(add_exp(&mut pran, 2200, &curve), Growth::Grew { levels: 2 });
        assert_eq!(pran.level, 2, "past 855 and past 2106 is two levels");
        assert_eq!(pran.max_hp, hp + 2 * HP_PER_LEVEL, "two levels did not add health twice");
        assert_eq!(pran.hp, pran.max_hp, "and did not fill it up");
    }

    /// The fairy stops at four however big the kill, keeping the experience of
    /// the wall rather than a number past it.
    #[test]
    fn a_fairy_lands_on_its_wall_and_stops() {
        let curve = curve();
        let mut pran = Pran::hatch(Element::Fire, &stone_of(1), 0);

        assert_eq!(add_exp(&mut pran, 1_000_000, &curve), Growth::Grew { levels: 4 });
        assert_eq!(pran.level, WALLS[0]);
        assert_eq!(pran.exp, curve.threshold(WALLS[0]), "it kept more than the wall holds");

        // and from there it earns nothing at all until it evolves
        let before = pran.exp;
        assert_eq!(add_exp(&mut pran, 5000, &curve), Growth::MustEvolve);
        assert_eq!(pran.exp, before, "it earned while standing at the wall");
        assert_eq!(pran.level, WALLS[0]);
    }

    /// Evolving is what lets it move again, and nothing else does.
    #[test]
    fn evolving_is_what_opens_the_next_stretch() {
        let curve = curve();
        let mut pran = Pran { level: WALLS[0], exp: curve.threshold(WALLS[0]),
            ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
        assert_eq!(add_exp(&mut pran, 5000, &curve), Growth::MustEvolve);

        pran.class = evolved(pran.class).unwrap();
        assert_eq!(add_exp(&mut pran, 5000, &curve), Growth::Grew { levels: 1 });
        assert_eq!(pran.level, 5, "the child begins at five");
        assert_eq!(Form::of_level(pran.level), Form::Child);
    }

    /// A level in the first band raises the skills the original raises.
    #[test]
    fn a_level_raises_the_skills_the_band_names() {
        let curve = curve();
        let mut pran = Pran { level: 4, class: 62, exp: curve.threshold(4),
            ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
        assert_eq!(pran.skill_levels[..3], [1, 1, 1], "a hatchling knows three");

        add_exp(&mut pran, 5000, &curve);

        // level five: skills 0 to (4 / 5) + 2 = 0..=2, plus the fourth
        assert!(pran.skill_levels[0] > 1, "the first skill did not grow");
        assert_eq!(pran.skill_levels[9], 0, "the last one is not in this band");
    }

    /// A curve that never loaded must not hand out levels, and must not panic.
    #[test]
    fn no_curve_means_no_growth() {
        let empty = ExpCurve::default();
        let mut pran = Pran::hatch(Element::Fire, &stone_of(1), 0);

        assert_eq!(add_exp(&mut pran, 1_000_000, &empty), Growth::Grew { levels: 0 });
        assert_eq!(pran.level, 0, "it levelled off a table that does not exist");
    }

    /// The four forms and the three walls between them, as the original's own
    /// comments lay them out.
    #[test]
    fn the_level_decides_the_form() {
        assert_eq!(Form::of_level(1), Form::Fairy);
        assert_eq!(Form::of_level(4), Form::Fairy, "still a fairy at the wall");
        assert_eq!(Form::of_level(5), Form::Child);
        assert_eq!(Form::of_level(18), Form::Child);
        assert_eq!(Form::of_level(19), Form::Child, "still a child at the wall");
        assert_eq!(Form::of_level(20), Form::Teenager);
        assert_eq!(Form::of_level(48), Form::Teenager);
        assert_eq!(Form::of_level(49), Form::Teenager);
        assert_eq!(Form::of_level(50), Form::Adult);
        assert_eq!(Form::of_level(LEVEL_CAP), Form::Adult);
    }

    /// A hatchling is a fairy, and the class alone does not change that. This
    /// is the one that cost an evening: setting the class to 62 and leaving
    /// the level at 1 changes nothing anybody can see.
    #[test]
    fn a_class_without_the_level_is_still_a_fairy() {
        assert_eq!(Form::of_level(1), Form::Fairy);
        assert_eq!(Form::of_level(1).tier(), 1);
        // the child needs both halves
        assert_eq!(Form::of_level(5).tier(), 2);
    }

    /// The wall is where the level has caught up with the class and stops.
    #[test]
    fn a_pran_at_a_wall_has_to_evolve_before_it_grows() {
        assert!(must_evolve(4, 61), "a fairy at four");
        assert!(must_evolve(4, 71));
        assert!(must_evolve(19, 62), "a child at nineteen");
        assert!(must_evolve(49, 63), "an adolescent at forty-nine");

        assert!(!must_evolve(3, 61), "short of the wall");
        assert!(!must_evolve(4, 62), "already evolved past it");
        assert!(!must_evolve(19, 63));
        assert!(!must_evolve(4, 0), "not a pran class at all");
    }

    /// Every wall must be passable, and the last form must have no wall after
    /// it, or a pran either stops early or evolves into a class with no skills.
    #[test]
    fn every_wall_leads_somewhere_and_the_last_form_has_none() {
        for element in [Element::Fire, Element::Water, Element::Air] {
            let mut class = element.first_class();
            for wall in WALLS {
                assert!(must_evolve(wall, class), "class {class} does not stop at {wall}");
                class = evolved(class).expect("a wall with nothing past it");
                assert!(stone_tier(class).is_some(), "class {class} fits no stone");
            }
            assert_eq!(class, element.first_class() + 3, "four forms, three walls");
            assert_eq!(evolved(class), None, "the adult evolved again");
            assert!(!must_evolve(LEVEL_CAP, class), "the adult is walled in");
        }
    }
    /// The quest at the first wall, field for field.
    #[test]
    fn the_first_quest_turns_a_fairy_into_a_child() {
        let mut pran = Pran { level: 4, ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
        let before = pran.skill_levels[3];

        let grown = evolve(&mut pran).expect("a fairy at the wall could not evolve");

        assert_eq!(grown.class, 62);
        assert_eq!(grown.stone, 104, "the stone it is carried in did not change");
        assert!(grown.clears_the_glow, "the fairy effect was left on the player");
        assert_eq!(pran.equipment[0], 104, "and the pran is not drawn as it");
        assert_eq!(pran.equipment[6], CHILD_HELD_ITEM);
        assert_eq!(pran.skill_levels[3], before + 1, "the transform skill");
        assert_eq!(pran.level, 4, "evolving is not a level");
    }

    /// The second differs in the stone and in what it leaves alone.
    #[test]
    fn the_second_quest_turns_a_child_into_an_adolescent() {
        let mut pran = Pran {
            level: 19,
            class: 62,
            ..Pran::hatch(Element::Fire, &stone_of(1), 0)
        };
        let held = pran.equipment[6];
        let transform = pran.skill_levels[3];

        let grown = evolve(&mut pran).expect("a child at the wall could not evolve");

        assert_eq!((grown.class, grown.stone), (63, 105));
        assert!(!grown.clears_the_glow, "there was no glow left to clear");
        assert_eq!(pran.equipment[6], held, "the second quest sets no held item");
        assert_eq!(pran.skill_levels[3], transform, "nor raises the transform skill");
    }

    /// Only at the wall. Not one level short of it and not one past it: the
    /// original tests `Level = 4` and `Level = 19` exactly.
    #[test]
    fn evolving_happens_at_the_wall_and_nowhere_else() {
        for level in [3u8, 5, 18, 20] {
            let mut pran = Pran { level, ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
            assert_eq!(evolve(&mut pran), Err(NotYet::NotAtAWall), "at level {level}");
            assert_eq!(pran.class, 61, "it evolved anyway at level {level}");
        }
    }

    /// A wall it has already passed evolves nothing, and neither does the
    /// last form: there is no fifth.
    #[test]
    fn a_wall_already_passed_evolves_nothing() {
        // already a child, standing on the fairy's wall
        let mut pran = Pran { level: 4, class: 62, ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
        assert_eq!(evolve(&mut pran), Err(NotYet::NothingFurther));

        // and the last form, which has nothing left to become
        let mut last = Pran { level: 49, class: 64, ..Pran::hatch(Element::Fire, &stone_of(1), 0) };
        assert_eq!(evolve(&mut last), Err(NotYet::NothingFurther));
        assert_eq!(last.class, 64);
    }

    /// The third wall. Its quest is in the original's code and not in its
    /// data, and evolving here is driven by the NPC rather than by that file,
    /// so an adolescent goes through it instead of standing on it for ever.
    #[test]
    fn the_last_wall_turns_an_adolescent_into_an_adult() {
        let mut pran =
            Pran { level: 49, class: 63, ..Pran::hatch(Element::Fire, &stone_of(1), 0) };

        let grown = evolve(&mut pran).expect("it stopped at the last wall");

        assert_eq!((grown.class, grown.stone), (64, 111));
        assert_eq!(pran.equipment[0], 111, "and it is not drawn as its new stone");
        assert!(!grown.clears_the_glow, "only the first evolution takes the glow off");
    }

    /// Evolving is what lets the level move again, which is the whole point
    /// of the pair. Walked end to end: hatch, grow to the wall, be stopped,
    /// evolve, and grow into the next form.
    #[test]
    fn evolving_at_the_wall_opens_the_way_to_the_next_form() {
        let curve = curve();
        let mut pran = Pran::hatch(Element::Fire, &stone_of(1), 0);

        add_exp(&mut pran, 1_000_000, &curve);
        assert_eq!(pran.level, 4);
        assert_eq!(Form::of_level(pran.level), Form::Fairy);
        assert_eq!(add_exp(&mut pran, 5000, &curve), Growth::MustEvolve);

        evolve(&mut pran).expect("the wall it was stopped at refused to open");

        assert_eq!(add_exp(&mut pran, 5000, &curve), Growth::Grew { levels: 1 });
        assert_eq!(pran.level, 5);
        assert_eq!(Form::of_level(pran.level), Form::Child);
        assert!(pran.has_body(), "a child walks beside its owner");
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
        let fire = Pran::hatch(Element::Fire, &stone_of(7), 1000);
        assert_eq!((fire.class, fire.max_hp, fire.max_mp), (61, 383, 235));
        assert_eq!((fire.def_physical, fire.def_magic), (239, 104));

        let water = Pran::hatch(Element::Water, &stone_of(7), 1000);
        assert_eq!((water.class, water.max_hp, water.max_mp), (71, 209, 356));
        assert_eq!((water.def_physical, water.def_magic), (153, 308));

        let air = Pran::hatch(Element::Air, &stone_of(7), 1000);
        assert_eq!((air.class, air.max_hp, air.max_mp), (81, 255, 267));
        assert_eq!((air.def_physical, air.def_magic), (201, 205));

        for pran in [&fire, &water, &air] {
            assert_eq!(pran.hp, pran.max_hp, "it should not hatch wounded");
            assert_eq!(pran.mp, pran.max_mp);
            assert_eq!(pran.level, 0, "a pran is born at nothing");
            assert!(!pran.has_body(), "a hatchling is only a glow");
        }
    }

    /// Everything below the four numbers that differ by element, which is most
    /// of what `FinishQuest` sets and none of what the first cut of this
    /// ported. Zeros here are not harmless: a build of 0/0/0 is what put a
    /// misshapen half-height naked human on the field with the right name over
    /// its head.
    #[test]
    fn a_hatchling_is_built_the_way_the_quest_builds_one() {
        let pran = Pran::hatch(Element::Fire, &stone_of(4242), 1000);

        assert_eq!((pran.width, pran.chest, pran.leg), (7, 100, 100), "its build");
        assert_eq!(pran.exp, 1, "the count starts at one, not at zero");
        assert_eq!((pran.food, pran.devotion), (121, 113));
        assert_eq!(pran.personality.cute, 226);
        assert_eq!(
            [
                pran.personality.smart,
                pran.personality.sexy,
                pran.personality.energetic,
                pran.personality.tough,
                pran.personality.corrupt,
            ],
            [50; 5]
        );

        // Cute is past devotion and the others are under it, so a hatchling
        // reads as the first of the six until it is raised into another.
        assert_eq!(pran.personality.shown(pran.devotion as u32), 0);
    }

    /// What the client draws it as. In the player spawn this packet is a copy
    /// of, `Equip[0]` is the model, and a pran wears its own summon stone
    /// there.
    #[test]
    fn a_hatchling_wears_the_stone_it_came_out_of() {
        let pran = Pran::hatch(Element::Water, &Item { index: 101, identific: 9, ..Item::default() }, 0);

        assert_eq!(pran.equipment[0], 101, "it has nothing to be drawn as");
        assert_eq!(pran.equipment[6], HATCHLING_HELD_ITEM);
        assert_eq!(pran.item_id, 9, "and it is bound to that same stone");
        assert!(pran.equipment[1..6].iter().all(|i| *i == 0), "it wears nothing else");
    }

    /// Ten skills, ten apart, starting where the element starts.
    #[test]
    fn a_hatchling_carries_its_elements_ten_skills() {
        let fire = Pran::hatch(Element::Fire, &stone_of(1), 0);
        assert_eq!(fire.skills[0], 5761);
        assert_eq!(fire.skills[1], 5771);
        assert_eq!(fire.skills[9], 5851);
        assert_eq!(fire.known_skills(), SKILLS);

        assert_eq!(Pran::hatch(Element::Water, &stone_of(1), 0).skills[0], 5861);
        assert_eq!(Pran::hatch(Element::Air, &stone_of(1), 0).skills[0], 5961);
    }

    /// The three elements must not share a skill, or learning one would teach
    /// another element's.
    #[test]
    fn the_three_elements_do_not_share_a_skill() {
        let mut all: Vec<u32> = [Element::Fire, Element::Water, Element::Air]
            .iter()
            .flat_map(|e| Pran::hatch(*e, &stone_of(1), 0).skills)
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
        let pran = Pran::hatch(Element::Fire, &stone_of(4242), 0);
        let hers = Item { index: 100, identific: 4242, ..Item::default() };
        let his = Item { index: 100, identific: 9999, ..Item::default() };

        assert!(pran.belongs_to(&hers));
        assert!(!pran.belongs_to(&his), "it answered to somebody else's stone");
    }

    /// And a pran with no stone recorded answers to none, rather than to every
    /// item whose identific has not been filled in.
    #[test]
    fn a_pran_with_no_stone_answers_to_nothing() {
        let pran = Pran { item_id: 0, ..Pran::hatch(Element::Fire, &stone_of(0), 0) };
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
