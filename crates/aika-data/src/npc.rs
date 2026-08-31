//! `.npc` — one file per non-player character, in `Data/NPCs/`.
//!
//! Not a designed format: the original server writes the `TNPCFile` record
//! straight out of memory with `File of TNPCFile` (`Functions/Load.pas:1730`),
//! so the file *is* the Delphi record, padding and unused fields and all. It
//! is 5639 bytes, of which the server reads a handful.
//!
//! ```text
//! 0      u8        length of the title that follows
//! 1..36  char[35]  title, the job shown under the name ("Merchant")
//! 36..46 u8[10]    menu entries, in order; a zero ends the list
//! 46..558          Reserved, never read
//! 558    u32       TBasicNpc.Index
//! 562    u32       TCustomNpc.ClientId, the same value again
//! 574    char[16]  name, as the ASCII of an index into the client's
//!                  string table — the client owns the display names
//! 606    u8[4]     Sizes, the body proportions
//! 610    u32       MaxHP, then CurHP, MaxMP, CurMP
//! 902    u16[8]    Equip; the first two hold the model to render
//! 1226   TItem[40] Inventory; only the id of each is read, and it is what
//!                  the NPC sells (`TNPCHandlers.ShowShop`)
//! 4770   u16       SpeedMove
//! 4774   u16       Rotation
//! 4799   f32,f32   LastPos, zero in every shipped file
//! 4807   f32,f32   CurrentPos, where the NPC stands
//! ```
//!
//! Offsets past the header were found by searching all 469 files at once for
//! the value that had to be there: the id appears in the file name, and the
//! position is the only float pair that looks like map coordinates in every
//! file. The record definition in `Data/PlayerData.pas:662` agrees.
//!
//! Everything from 562 on is a `TCustomNpc`, which starts with the same
//! fields as the player record. That is why the offsets inside it are the
//! ones `game::character_offset` already uses, shifted by 562.
//!
//! # The id lives in the file name
//!
//! These files were made by copying one another, and the copies kept the
//! original's id: `[2700] Lilola Hawn.npc` says 2215 inside. The original
//! server patches a hardcoded list of those in `TLoad.InitNPCS` and leaves
//! the rest broken, so several NPCs silently overwrite each other in its
//! array. We take the bracketed id as the truth for every file, which fixes
//! the ones the original never got to, and report the disagreement.

use std::path::Path;

/// Size of `TNPCFile`. Six of the shipped files are ten bytes longer; the
/// original reads a record's worth and ignores the rest, so we do too.
pub const RECORD_SIZE: usize = 5639;

/// Menu entries a single NPC can offer.
pub const MAX_OPTIONS: usize = 10;

/// Slots the shop window has. The original sends all forty whether they hold
/// anything or not (`TNPCHandlers.ShowShop`).
pub const SHOP_SLOTS: usize = 40;

mod offset {
    pub const TITLE_LEN: usize = 0;
    pub const TITLE: usize = 1;
    pub const TITLE_MAX: usize = 35;
    pub const OPTIONS: usize = 36;
    pub const INDEX: usize = 558;
    pub const CLIENT_ID: usize = 562;
    pub const NAME: usize = 574;
    pub const NAME_MAX: usize = 16;
    /// `TCustomNpc` begins at `CLIENT_ID`; these are its own offsets plus it.
    pub const SIZES: usize = CLIENT_ID + 44;
    pub const MAX_HP: usize = CLIENT_ID + 48;
    pub const CUR_HP: usize = CLIENT_ID + 52;
    pub const MAX_MP: usize = CLIENT_ID + 56;
    pub const CUR_MP: usize = CLIENT_ID + 60;
    pub const EQUIP: usize = CLIENT_ID + 340;
    /// 16 equipment slots of 20 bytes, then a spare DWORD.
    pub const INVENTORY: usize = EQUIP + 16 * ITEM_SIZE + 4;
    /// `TItem` (`Data/MiscData.pas:44`): index, appearance, identification,
    /// effects, durability, refine and expiry.
    pub const ITEM_SIZE: usize = 20;
    pub const SPEED_MOVE: usize = 4770;
    pub const ROTATION: usize = 4774;
    pub const POSITION: usize = 4807;
}

#[derive(Debug, PartialEq, Eq)]
pub enum NpcError {
    /// Shorter than one record.
    TooShort(usize),
    /// The position in the file is not on the map.
    BadPosition,
}

impl std::fmt::Display for NpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NpcError::TooShort(size) => {
                write!(f, "{size} bytes, less than the {RECORD_SIZE} of a record")
            }
            NpcError::BadPosition => write!(f, "the position is not on the map"),
        }
    }
}

impl std::error::Error for NpcError {}

/// The map is 8192 units across, and nothing stands at the origin.
const MAP_LIMIT: f32 = 8192.0;

/// One NPC, with the fields the server actually needs to place it in the
/// world and answer when a player talks to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Npc {
    /// What the client is told to spawn, which also picks the model.
    pub id: u16,
    /// The job under the name, already in English in the shipped files.
    pub title: String,
    /// The name from the file name. The client never sees it — it looks the
    /// real one up in its own string table — but a log saying "Diego Nobaro"
    /// beats one saying "2048".
    pub label: String,
    /// Index into the client's string table, where the display name lives.
    /// Stored as text in the file; `None` when it is not a number.
    pub name_index: Option<u16>,
    /// Menu entries, in the order the client should list them. The trailing
    /// zeros of the fixed array are dropped.
    pub options: Vec<u8>,
    /// What the client renders. The first two entries carry the model; the
    /// rest are zero in every shipped file.
    pub equip: [u16; 8],
    /// Height, torso, legs and body.
    pub sizes: [u8; 4],
    /// What this NPC sells, by item id, one per shop slot. Zero is an empty
    /// slot, and an NPC that is not a merchant has forty of them.
    pub shop: [u16; SHOP_SLOTS],
    pub max_hp: u32,
    pub cur_hp: u32,
    pub max_mp: u32,
    pub cur_mp: u32,
    pub x: f32,
    pub y: f32,
    pub rotation: u16,
    pub speed_move: u16,
    /// The id the record carries, when it disagrees with the file name. It is
    /// a copied file that was never fixed, and it is kept only so tools can
    /// report it.
    pub stale_id: Option<u16>,
}

impl Npc {
    /// Whether this NPC sells anything, which is what makes the shop entry in
    /// its menu worth showing.
    pub fn sells(&self) -> bool {
        self.shop.iter().any(|&id| id != 0)
    }

    /// The ids on offer, in slot order, without the empty slots.
    pub fn stock(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.shop.iter().copied().enumerate().filter(|(_, id)| *id != 0)
    }

    /// Reads a record. `id` is the one from the file name, which wins over
    /// the one in the record for the reason in the module documentation;
    /// pass `None` to trust the record.
    pub fn decode(data: &[u8], id: Option<u16>) -> Result<Self, NpcError> {
        Self::decode_named(data, id, String::new())
    }

    /// The same, keeping the name a person would recognise.
    pub fn decode_named(
        data: &[u8],
        id: Option<u16>,
        label: String,
    ) -> Result<Self, NpcError> {
        if data.len() < RECORD_SIZE {
            return Err(NpcError::TooShort(data.len()));
        }

        let record_id = u32::from_le_bytes(read4(data, offset::INDEX)) as u16;
        let id = id.unwrap_or(record_id);
        let stale_id = (id != record_id).then_some(record_id);

        let x = f32::from_le_bytes(read4(data, offset::POSITION));
        let y = f32::from_le_bytes(read4(data, offset::POSITION + 4));
        if !(x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0 && x < MAP_LIMIT && y < MAP_LIMIT)
        {
            return Err(NpcError::BadPosition);
        }

        // A Delphi short string: one length byte, then the text. The length
        // is trusted only as far as the field goes, because a few files carry
        // leftovers from whatever record they were copied from.
        let title_len = (data[offset::TITLE_LEN] as usize).min(offset::TITLE_MAX);
        let title = latin1(&data[offset::TITLE..offset::TITLE + title_len]);

        let name = latin1(nul_terminated(&data[offset::NAME..offset::NAME + offset::NAME_MAX]));
        let name_index = name.parse().ok();

        let options = data[offset::OPTIONS..offset::OPTIONS + MAX_OPTIONS]
            .iter()
            .copied()
            .take_while(|&o| o != 0)
            .collect();

        let mut equip = [0u16; 8];
        for (i, slot) in equip.iter_mut().enumerate() {
            *slot = u16::from_le_bytes(read2(data, offset::EQUIP + i * 2));
        }

        // Only the id of each inventory entry matters: the price, the level
        // requirement and everything else come from the item table.
        let mut shop = [0u16; SHOP_SLOTS];
        for (i, slot) in shop.iter_mut().enumerate() {
            *slot = u16::from_le_bytes(read2(data, offset::INVENTORY + i * offset::ITEM_SIZE));
        }

        Ok(Self {
            id,
            title,
            label,
            name_index,
            options,
            equip,
            sizes: data[offset::SIZES..offset::SIZES + 4].try_into().unwrap(),
            shop,
            max_hp: u32::from_le_bytes(read4(data, offset::MAX_HP)),
            cur_hp: u32::from_le_bytes(read4(data, offset::CUR_HP)),
            max_mp: u32::from_le_bytes(read4(data, offset::MAX_MP)),
            cur_mp: u32::from_le_bytes(read4(data, offset::CUR_MP)),
            x,
            y,
            rotation: u16::from_le_bytes(read2(data, offset::ROTATION)),
            speed_move: u16::from_le_bytes(read2(data, offset::SPEED_MOVE)),
            stale_id,
        })
    }

    /// Reads a file, taking the id from its name.
    pub fn from_file(path: impl AsRef<Path>) -> std::io::Result<Result<Self, NpcError>> {
        let path = path.as_ref();
        let data = std::fs::read(path)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        Ok(Self::decode_named(&data, id_in_file_name(&name), label_in_file_name(&name)))
    }
}

/// Every NPC that could be read from a directory, by id.
///
/// Files that fail to read are reported rather than dropped silently: the
/// shipped set has several that overwrite one another, and a server that
/// quietly loses an NPC is a server where a shop is missing for no visible
/// reason.
#[derive(Debug, Default)]
pub struct NpcSet {
    npcs: Vec<Npc>,
    /// File name and what went wrong, for the ones that did not load.
    pub rejected: Vec<(String, String)>,
}

impl NpcSet {
    /// Reads every `.npc` in a directory. Later files win a clash of ids, and
    /// the loser is reported, which is what the original does silently.
    pub fn load_dir(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let mut set = Self::default();

        let mut paths: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("npc")))
            .collect();
        paths.sort();

        for path in paths {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match Npc::from_file(&path)? {
                Ok(npc) => set.insert(name, npc),
                Err(e) => set.rejected.push((name, e.to_string())),
            }
        }

        set.npcs.sort_by_key(|n| n.id);
        Ok(set)
    }

    fn insert(&mut self, file_name: String, npc: Npc) {
        if let Some(existing) = self.npcs.iter_mut().find(|n| n.id == npc.id) {
            self.rejected.push((file_name, format!("id {} is already taken", npc.id)));
            *existing = npc;
            return;
        }
        self.npcs.push(npc);
    }

    pub fn get(&self, id: u16) -> Option<&Npc> {
        self.npcs.iter().find(|n| n.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Npc> {
        self.npcs.iter()
    }

    pub fn len(&self) -> usize {
        self.npcs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.npcs.is_empty()
    }
}

/// The number between brackets at the start of a file name, which is where
/// the id really lives.
pub fn id_in_file_name(name: &str) -> Option<u16> {
    let open = name.find('[')?;
    let close = name[open..].find(']')? + open;
    name[open + 1..close].parse().ok()
}

/// The part of the file name after the brackets, which is the name a person
/// would recognise. The client does not see it; our logs and tools do.
pub fn label_in_file_name(name: &str) -> String {
    let after = name.find(']').map(|i| &name[i + 1..]).unwrap_or(name);
    after.trim_end_matches(".npc").trim_end_matches(".NPC").trim().to_string()
}

fn read4(data: &[u8], at: usize) -> [u8; 4] {
    data[at..at + 4].try_into().unwrap()
}

fn read2(data: &[u8], at: usize) -> [u8; 2] {
    data[at..at + 2].try_into().unwrap()
}

fn nul_terminated(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    &field[..end]
}

/// The files are Windows-1252 in practice, and latin-1 for the bytes that
/// appear in them. Anything unprintable is dropped rather than shown as a
/// replacement character, since these strings end up in logs.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record built from the offsets we claim, so the test does not need a
    /// file from the original pack.
    fn record(id: u32, title: &str, name: &str, options: &[u8], x: f32, y: f32) -> Vec<u8> {
        let mut data = vec![0u8; RECORD_SIZE];
        data[offset::EQUIP..offset::EQUIP + 4].copy_from_slice(&[234, 0, 234, 0]);
        for (i, id) in [4351u16, 4391, 4616].iter().enumerate() {
            let at = offset::INVENTORY + i * offset::ITEM_SIZE;
            data[at..at + 2].copy_from_slice(&id.to_le_bytes());
        }
        data[offset::SIZES..offset::SIZES + 4].copy_from_slice(&[7, 119, 119, 3]);
        data[offset::MAX_HP..offset::MAX_HP + 4].copy_from_slice(&20000u32.to_le_bytes());
        data[offset::CUR_HP..offset::CUR_HP + 4].copy_from_slice(&20000u32.to_le_bytes());
        data[offset::TITLE_LEN] = title.len() as u8;
        data[offset::TITLE..offset::TITLE + title.len()].copy_from_slice(title.as_bytes());
        data[offset::OPTIONS..offset::OPTIONS + options.len()].copy_from_slice(options);
        data[offset::INDEX..offset::INDEX + 4].copy_from_slice(&id.to_le_bytes());
        data[offset::CLIENT_ID..offset::CLIENT_ID + 4].copy_from_slice(&id.to_le_bytes());
        data[offset::NAME..offset::NAME + name.len()].copy_from_slice(name.as_bytes());
        data[offset::POSITION..offset::POSITION + 4].copy_from_slice(&x.to_le_bytes());
        data[offset::POSITION + 4..offset::POSITION + 8].copy_from_slice(&y.to_le_bytes());
        data
    }

    #[test]
    fn reads_the_fields_the_server_needs() {
        let data = record(2050, "Merchant", "43", &[1, 2, 31, 32, 5, 8], 3468.4, 963.4);
        let npc = Npc::decode(&data, None).unwrap();

        assert_eq!(npc.id, 2050);
        assert_eq!(npc.title, "Merchant");
        assert_eq!(npc.label, "");
        assert_eq!(npc.name_index, Some(43));
        assert_eq!(npc.options, vec![1, 2, 31, 32, 5, 8]);
        assert_eq!((npc.x, npc.y), (3468.4, 963.4));
        assert_eq!(npc.equip, [234, 234, 0, 0, 0, 0, 0, 0], "the model the client draws");
        assert_eq!(npc.sizes, [7, 119, 119, 3]);
        assert_eq!(npc.stock().collect::<Vec<_>>(), vec![(0, 4351), (1, 4391), (2, 4616)]);
        assert!(npc.sells());
        assert_eq!((npc.max_hp, npc.cur_hp), (20000, 20000));
        assert_eq!(npc.stale_id, None);
    }

    /// The fixed array is ten long and a zero ends the menu; the entries
    /// after it are leftovers and must not become menu lines.
    #[test]
    fn the_menu_stops_at_the_first_zero() {
        let data = record(2049, "Farmer", "42", &[1, 2, 8, 0, 99, 99], 3483.4, 967.4);
        assert_eq!(Npc::decode(&data, None).unwrap().options, vec![1, 2, 8]);
    }

    /// A copied file keeps the id of whatever it was copied from. The name
    /// decides, and the stale value is reported rather than thrown away.
    #[test]
    fn the_file_name_wins_over_a_stale_record_id() {
        let data = record(2215, "Merchant", "43", &[1, 8], 3400.0, 900.0);
        let npc = Npc::decode(&data, Some(2700)).unwrap();

        assert_eq!(npc.id, 2700);
        assert_eq!(npc.stale_id, Some(2215));
    }

    #[test]
    fn refuses_a_position_off_the_map() {
        let data = record(2050, "Merchant", "43", &[1], 0.0, 0.0);
        assert_eq!(Npc::decode(&data, None), Err(NpcError::BadPosition));

        let data = record(2050, "Merchant", "43", &[1], f32::NAN, 900.0);
        assert_eq!(Npc::decode(&data, None), Err(NpcError::BadPosition));
    }

    #[test]
    fn refuses_a_short_file() {
        assert_eq!(Npc::decode(&[0u8; 100], None), Err(NpcError::TooShort(100)));
    }

    /// A few titles carry a length byte longer than the text that was copied
    /// in. The length is clamped to the field, so a bad one reads the rest of
    /// the field rather than walking into the menu bytes that follow it.
    #[test]
    fn a_title_cannot_run_past_its_field() {
        let mut data = record(2050, &"T".repeat(35), "43", &[1, 2, 8], 3400.0, 900.0);
        data[offset::TITLE_LEN] = 250;

        let npc = Npc::decode(&data, None).unwrap();
        assert_eq!(npc.title, "T".repeat(offset::TITLE_MAX));
        assert_eq!(npc.options, vec![1, 2, 8], "the menu was read as part of the title");
    }

    #[test]
    fn takes_the_id_and_the_label_from_the_file_name() {
        assert_eq!(id_in_file_name("[2700] Lilola Hawn.npc"), Some(2700));
        assert_eq!(label_in_file_name("[2700] Lilola Hawn.npc"), "Lilola Hawn");
        assert_eq!(id_in_file_name("Lilola.npc"), None);
        assert_eq!(label_in_file_name("[2709].npc"), "");
    }

    /// The real files are not tracked in this repository. When they are
    /// present the parser is held to what they contain.
    #[test]
    fn reads_the_original_files_when_they_are_available() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/NPCs");
        if !dir.is_dir() {
            return;
        }

        let set = NpcSet::load_dir(&dir).unwrap();
        assert!(set.len() > 400, "only {} npcs read", set.len());

        for npc in set.iter() {
            assert!(npc.x > 0.0 && npc.y > 0.0, "npc {} has no position", npc.id);
            assert!(npc.options.len() <= MAX_OPTIONS);
        }

        // the merchant of the starting city, read from a real file
        let merchant = set.get(2050).expect("npc 2050 is missing");
        assert_eq!(merchant.title, "Merchant");
        assert_eq!(merchant.label, "Thomas Henrikson");
        assert_eq!(merchant.equip[0], 234);
        assert!(merchant.sells(), "the merchant has an empty shop");

        // a farmer sells nothing, and must not offer a shop
        let farmer = set.get(2049).expect("npc 2049 is missing");
        assert!(!farmer.sells());
    }
}
