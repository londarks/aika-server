//! `ItemList.bin` — every item the game knows: names, prices, stats.
//!
//! This is the server's copy, and unlike the client's `ItemList4.bin` it is
//! **not encrypted**. The original reads it with a bare `Read(f, ItemList)`
//! (`Functions/Load.pas:436`), a straight array of `TItemFromList` records
//! poured into memory, and so does this.
//!
//! Fixed 464-byte records with no header. The record layout is declared in
//! `Data/FilesData.pas:13`, and the stride is confirmed against the shipped
//! file: 14,384,004 bytes hold 31,000 records with 4 bytes left over.
//!
//! The field offsets were checked against the shipped table rather than
//! trusted: `DelayUse` reads zero on every piece of gear and four on a
//! potion, which is what a use cooldown should look like and pins the whole
//! price block that follows it.
//!
//! Do not read too much into the prices themselves. This table came from a
//! server that ran in production with hand-edited values, and it contains
//! items that cost 70 gold and sell back for 165,000. That is the data, not a
//! parsing error.
//!
//! Two mismatches with the original worth knowing. It declares
//! `TItemList = ARRAY [0..25998]`, which is 5,000 records short of what the
//! file actually holds, so the original silently ignores the tail. And the
//! first ten records are empty: item ids start at 10.

/// Every record is this wide. Sum of the declared fields, and the stride
/// measured between item names in the shipped file.
pub const RECORD_SIZE: usize = 464;

mod field {
    use std::ops::Range;
    pub const NAME: Range<usize> = 0..64;
    pub const NAME_ENGLISH: Range<usize> = 64..128;
    pub const DESCRIPTION: Range<usize> = 128..256;
    pub const CAN_GROUP: usize = 256;
    pub const ITEM_TYPE: usize = 258;
    pub const USE_EFFECT: usize = 268;
    pub const DELAY_USE: usize = 276;
    pub const PRICE_HONOR: usize = 280;
    pub const PRICE_MEDAL: usize = 284;
    pub const PRICE_GOLD: usize = 288;
    pub const SELL_PRICE: usize = 292;
    pub const CLASS: usize = 300;
    pub const LEVEL: usize = 330;
    pub const DURATION: usize = 336;
    pub const ATTACK: usize = 358;
    pub const DEFENSE: usize = 360;
    pub const MAGIC_ATTACK: usize = 362;
    pub const MAGIC_DEFENSE: usize = 364;
    pub const HP: usize = 372;
    pub const MP: usize = 374;
    /// Rarity, 0 to 7.
    pub const TYPE_ITEM: usize = 390;
    /// 0 tradable, 1 not tradable, 2 trade reverted.
    pub const TYPE_TRADE: usize = 393;
    pub const DURABILITY: usize = 406;
    pub const MAX_LEVEL: usize = 432;
}

#[derive(Debug, PartialEq, Eq)]
pub enum ItemListError {
    /// The file does not hold a whole number of records.
    UnalignedSize { size: usize, leftover: usize },
    TooShort(usize),
}

impl std::fmt::Display for ItemListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemListError::UnalignedSize { size, leftover } => write!(
                f,
                "{size} bytes leaves {leftover} over after {RECORD_SIZE}-byte records"
            ),
            ItemListError::TooShort(n) => write!(f, "only {n} bytes, not even one record"),
        }
    }
}

impl std::error::Error for ItemListError {}

/// One item definition. Kept as raw bytes: most of the record is fields whose
/// meaning nobody wrote down, and reading only what we understand means the
/// rest survives untouched.
#[derive(Clone)]
pub struct ItemDef {
    raw: [u8; RECORD_SIZE],
}

impl ItemDef {
    /// Portuguese name, the one the client shows.
    pub fn name(&self) -> String {
        text(&self.raw[field::NAME])
    }

    /// Original name, useful when the translation is missing or wrong.
    pub fn name_english(&self) -> String {
        text(&self.raw[field::NAME_ENGLISH])
    }

    pub fn description(&self) -> String {
        text(&self.raw[field::DESCRIPTION])
    }

    /// Whether copies stack in one inventory slot.
    pub fn can_group(&self) -> bool {
        self.raw[field::CAN_GROUP] != 0
    }

    pub fn item_type(&self) -> u16 {
        u16le(&self.raw, field::ITEM_TYPE)
    }

    pub fn use_effect(&self) -> u16 {
        u16le(&self.raw, field::USE_EFFECT)
    }

    pub fn delay_use(&self) -> u32 {
        u32le(&self.raw, field::DELAY_USE)
    }

    pub fn price_honor(&self) -> u32 {
        u32le(&self.raw, field::PRICE_HONOR)
    }

    pub fn price_medal(&self) -> u32 {
        u32le(&self.raw, field::PRICE_MEDAL)
    }

    pub fn price_gold(&self) -> u32 {
        u32le(&self.raw, field::PRICE_GOLD)
    }

    /// What a shop pays for it. The original divides this further depending on
    /// the item type before paying out.
    pub fn sell_price(&self) -> u32 {
        u32le(&self.raw, field::SELL_PRICE)
    }

    pub fn class_mask(&self) -> u16 {
        u16le(&self.raw, field::CLASS)
    }

    pub fn level(&self) -> u16 {
        u16le(&self.raw, field::LEVEL)
    }

    pub fn duration(&self) -> u32 {
        u32le(&self.raw, field::DURATION)
    }

    pub fn attack(&self) -> u16 {
        u16le(&self.raw, field::ATTACK)
    }

    pub fn defense(&self) -> u16 {
        u16le(&self.raw, field::DEFENSE)
    }

    pub fn magic_attack(&self) -> u16 {
        u16le(&self.raw, field::MAGIC_ATTACK)
    }

    pub fn magic_defense(&self) -> u16 {
        u16le(&self.raw, field::MAGIC_DEFENSE)
    }

    pub fn hp(&self) -> u16 {
        u16le(&self.raw, field::HP)
    }

    pub fn mp(&self) -> u16 {
        u16le(&self.raw, field::MP)
    }

    /// Rarity, 0 to 7.
    pub fn rarity(&self) -> u8 {
        self.raw[field::TYPE_ITEM]
    }

    /// 0 tradable, 1 not tradable, 2 trade reverted.
    pub fn trade_kind(&self) -> u8 {
        self.raw[field::TYPE_TRADE]
    }

    pub fn durability(&self) -> u8 {
        self.raw[field::DURABILITY]
    }

    pub fn max_level(&self) -> u32 {
        u32le(&self.raw, field::MAX_LEVEL)
    }

    /// An unused id: the table is sparse and most slots are blank.
    pub fn is_empty(&self) -> bool {
        self.name().trim().is_empty() && self.name_english().trim().is_empty()
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

impl std::fmt::Debug for ItemDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemDef")
            .field("name", &self.name())
            .field("price_gold", &self.price_gold())
            .field("level", &self.level())
            .finish()
    }
}

pub struct ItemList {
    items: Vec<ItemDef>,
}

impl ItemList {
    pub fn decode(bytes: &[u8]) -> Result<Self, ItemListError> {
        if bytes.len() < RECORD_SIZE {
            return Err(ItemListError::TooShort(bytes.len()));
        }
        // The shipped file carries four bytes past the last record. The
        // original never notices, because it reads a fixed-size array and
        // stops well before the end; we read every whole record and say so.
        let leftover = bytes.len() % RECORD_SIZE;
        if leftover > RECORD_SIZE / 2 {
            return Err(ItemListError::UnalignedSize { size: bytes.len(), leftover });
        }

        Ok(Self {
            items: bytes
                .chunks_exact(RECORD_SIZE)
                .map(|chunk| ItemDef { raw: chunk.try_into().unwrap() })
                .collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Looks an item up by the id the protocol carries.
    pub fn get(&self, index: usize) -> Option<&ItemDef> {
        self.items.get(index).filter(|item| !item.is_empty())
    }

    /// Every id that actually holds an item.
    pub fn defined(&self) -> impl Iterator<Item = (usize, &ItemDef)> {
        self.items.iter().enumerate().filter(|(_, item)| !item.is_empty())
    }
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    // The tables are latin-1, same as everything else the client reads.
    bytes[..end].iter().map(|&b| b as char).collect::<String>().trim_end().to_string()
}

fn u16le(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

fn u32le(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's `ItemList.bin`, if one was dropped into `testdata/`.
    /// Game data is not redistributed here, so the checks against real bytes
    /// are opt-in, the same way `SL.bin` works.
    fn real_file() -> Option<Vec<u8>> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/ItemList.bin")).ok()
    }

    fn record(name: &str, gold: u32, level: u16) -> Vec<u8> {
        let mut raw = vec![0u8; RECORD_SIZE];
        let bytes: Vec<u8> = name.chars().map(|c| c as u8).collect();
        raw[..bytes.len()].copy_from_slice(&bytes);
        raw[field::PRICE_GOLD..field::PRICE_GOLD + 4].copy_from_slice(&gold.to_le_bytes());
        raw[field::LEVEL..field::LEVEL + 2].copy_from_slice(&level.to_le_bytes());
        raw[field::CAN_GROUP] = 1;
        raw
    }

    #[test]
    fn reads_fields_at_the_declared_offsets() {
        let mut bytes = vec![0u8; RECORD_SIZE]; // id 0 stays empty
        bytes.extend(record("Poção de Vida", 250, 12));

        let list = ItemList::decode(&bytes).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.get(0).is_none(), "an unused id reads as absent");

        let item = list.get(1).expect("id 1 holds an item");
        assert_eq!(item.name(), "Poção de Vida");
        assert_eq!(item.price_gold(), 250);
        assert_eq!(item.level(), 12);
        assert!(item.can_group());
    }

    #[test]
    fn skips_the_blank_ids_the_table_is_full_of() {
        let mut bytes = vec![0u8; RECORD_SIZE * 3];
        bytes.extend(record("Espada", 100, 1));

        let list = ItemList::decode(&bytes).unwrap();
        let defined: Vec<usize> = list.defined().map(|(id, _)| id).collect();
        assert_eq!(defined, vec![3], "only the filled id is reported");
    }

    #[test]
    fn rejects_a_file_that_is_not_records() {
        assert!(matches!(ItemList::decode(&[0u8; 10]), Err(ItemListError::TooShort(10))));
        let ragged = vec![0u8; RECORD_SIZE + 400];
        assert!(matches!(
            ItemList::decode(&ragged),
            Err(ItemListError::UnalignedSize { .. })
        ));
    }

    /// The shipped file: 14,384,004 bytes, 31,000 records, four bytes over.
    #[test]
    fn reads_the_real_table() {
        let Some(bytes) = real_file() else {
            return; // no game data dropped in; the synthetic tests cover the codec
        };

        let list = ItemList::decode(&bytes).unwrap();
        assert!(list.len() > 25_000, "the table is large, got {}", list.len());

        let defined = list.defined().count();
        assert!(defined > 10_000, "most ids are blank but thousands are not, got {defined}");

        // ids start at 10: the first records are empty
        assert!(list.get(0).is_none());
        let (first_id, first) = list.defined().next().expect("some item exists");
        assert_eq!(first_id, 10, "the table starts at id 10");
        assert!(!first.name().is_empty());

        // something, somewhere, must cost gold, or the price offset is wrong
        assert!(
            list.defined().any(|(_, item)| item.price_gold() > 0),
            "no item has a gold price: the price offset is off"
        );
        assert!(
            list.defined().any(|(_, item)| item.level() > 0),
            "no item has a level requirement: the level offset is off"
        );
    }
}
