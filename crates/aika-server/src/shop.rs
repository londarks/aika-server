//! Buying from and selling to a non-player character.
//!
//! The stock is not a table on the server: it is the NPC's own inventory,
//! read out of its `.npc` file, and the shop packet sends the forty slots as
//! they are (`TNPCHandlers.ShowShop`). What each of them costs comes from the
//! item table, so a shop is the join of two files we already read.
//!
//! ```text
//! server -> 0x106  { npc: u16, 0x0C, ids: u16[40] }   the shop window
//! client -> 0x313  { npc: u32, slot: u32, amount: u32 }  buy
//! client -> 0x314  { npc: u32, slot: u32 }               sell
//! server -> 0xF0E  { notice, container, slot, item }   one slot changed
//! server -> 0x312  { _, gold: u64, storage gold: u64 } the purse changed
//! ```
//!
//! `0x106` is also the opcode for the skill list, at the same size of 96
//! bytes. The client tells them apart by which window it has open, which is
//! why the shop must never be sent to somebody who did not ask an NPC for it.

use crate::inventory::{Inventory, InventoryError};
use crate::store::Item;
use aika_data::itemlist::ItemList;
use aika_data::npc::{Npc, SHOP_SLOTS};

pub const OP_SHOW_SHOP: u16 = 0x106;
pub const OP_BUY: u16 = 0x313;
pub const OP_SELL: u16 = 0x314;
pub const OP_REFRESH_ITEM: u16 = 0xF0E;
pub const OP_REFRESH_MONEY: u16 = 0x312;

/// `TShowShopPacket`: header, two WORDs, forty item ids.
pub const SHOW_SHOP_SIZE: usize = 12 + 2 + 2 + SHOP_SLOTS * 2;
/// `TRefreshItemPacket`: header, a notice flag, the container, the slot and
/// the twenty bytes of the item.
pub const REFRESH_ITEM_SIZE: usize = 12 + 1 + 1 + 2 + 20;
/// `TRefreshMoneyPacket`: header, a spare DWORD and two 64-bit purses.
pub const REFRESH_MONEY_SIZE: usize = 12 + 4 + 8 + 8;

/// A constant the original writes into the shop packet without explanation
/// (`NPCHandlers.pas:384`).
pub const SHOP_DEF_BYTE: u16 = 0x0C;

/// The rarest items cannot be sold back. The original writes this check as
/// `ItemList[...].TypeItem = 7`, and `TypeItem` is the rarity byte, commented
/// `[0~~7]` in `Data/FilesData.pas:57` — not the item type, which is a
/// different field entirely.
const UNSELLABLE_RARITY: u8 = 7;

/// Why a purchase or a sale was refused. Each carries the sentence the player
/// should read, because a shop that silently does nothing is worse than one
/// that says no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopError {
    /// The client asked a different NPC than the one it has open.
    WrongNpc,
    /// The slot the client named holds nothing.
    EmptySlot,
    /// An id that is not in the item table.
    UnknownItem(u16),
    /// The item has no gold price, so it is not for sale here.
    NotForSale,
    NotEnoughGold { needed: u64, held: u64 },
    BagFull,
    /// A rented item, or one the table marks as unsellable.
    CannotBeSold,
}

impl ShopError {
    /// What the player is told.
    pub fn message(&self) -> String {
        match self {
            ShopError::WrongNpc => "That shop is not open.".into(),
            ShopError::EmptySlot => "There is nothing there.".into(),
            ShopError::UnknownItem(id) => format!("Item {id} does not exist."),
            ShopError::NotForSale => "That is not for sale.".into(),
            ShopError::NotEnoughGold { needed, held } => {
                format!("You need {needed} gold and have {held}.")
            }
            ShopError::BagFull => "Your bag is full.".into(),
            ShopError::CannotBeSold => "That cannot be sold.".into(),
        }
    }
}

impl std::fmt::Display for ShopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ShopError {}

impl From<InventoryError> for ShopError {
    fn from(_: InventoryError) -> Self {
        ShopError::BagFull
    }
}

/// `0x313`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buy {
    pub npc: u32,
    pub slot: u32,
    pub amount: u32,
}

impl Buy {
    pub const BODY_SIZE: usize = 12;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            npc: u32::from_le_bytes(body[0..4].try_into().ok()?),
            slot: u32::from_le_bytes(body[4..8].try_into().ok()?),
            amount: u32::from_le_bytes(body[8..12].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.npc.to_le_bytes());
        body.extend_from_slice(&self.slot.to_le_bytes());
        body.extend_from_slice(&self.amount.to_le_bytes());
        body
    }
}

/// `0x314`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sell {
    pub npc: u32,
    pub slot: u32,
}

impl Sell {
    pub const BODY_SIZE: usize = 8;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            npc: u32::from_le_bytes(body[0..4].try_into().ok()?),
            slot: u32::from_le_bytes(body[4..8].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.npc.to_le_bytes());
        body.extend_from_slice(&self.slot.to_le_bytes());
        body
    }
}

/// What changed, so the caller can tell the client exactly that rather than
/// resending the whole character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The bag slot that now holds something different.
    pub slot: u16,
    /// What is in it now. An empty item means the slot was cleared.
    pub item: Item,
    /// The purse after the trade.
    pub gold: u64,
}

/// The forty ids of a shop window, in slot order.
pub fn stock(npc: &Npc) -> [u16; SHOP_SLOTS] {
    npc.shop
}

/// Buys one slot's worth from an NPC.
///
/// The amount lands in the item's `refine` field, which is where the original
/// puts a stack count for things that stack — the same field means different
/// things for different items, and the item table decides which.
pub fn buy(
    npc: &Npc,
    request: Buy,
    inventory: &mut Inventory,
    gold: u64,
    items: &ItemList,
) -> Result<Change, ShopError> {
    let slot = request.slot as usize;
    let id = npc.shop.get(slot).copied().unwrap_or(0);
    if id == 0 || request.amount == 0 {
        return Err(ShopError::EmptySlot);
    }

    let def = items.get(id as usize).ok_or(ShopError::UnknownItem(id))?;
    let unit = def.price_gold() as u64;
    if unit == 0 {
        return Err(ShopError::NotForSale);
    }

    // A stack is only a stack if the table says the item groups; anything
    // else is one item however many the client asked for.
    let amount = if def.can_group() { request.amount.max(1) } else { 1 };
    let price = unit.saturating_mul(amount as u64);
    if price > gold {
        return Err(ShopError::NotEnoughGold { needed: price, held: gold });
    }

    if inventory.free_slots(crate::inventory::BAG) == 0 {
        return Err(ShopError::BagFull);
    }

    let bought = Item {
        index: id,
        appearance: id,
        refine: if def.can_group() { amount as u16 } else { 0 },
        durability_min: def.durability(),
        durability_max: def.durability(),
        ..Item::default()
    };
    let slot = inventory.add(bought.clone())?;

    let item = inventory.get(crate::inventory::BAG, slot).cloned().unwrap_or(bought);
    Ok(Change { slot, item, gold: gold - price })
}

/// Sells a bag slot back. Returns the emptied slot and the new purse.
pub fn sell(
    request: Sell,
    inventory: &mut Inventory,
    gold: u64,
    items: &ItemList,
) -> Result<Change, ShopError> {
    let slot = request.slot as u16;
    let held = inventory
        .get(crate::inventory::BAG, slot)
        .ok_or(ShopError::EmptySlot)?
        .clone();

    // A rented item is on loan. Selling it would turn borrowed time into
    // permanent gold.
    if held.expires_at > 0 {
        return Err(ShopError::CannotBeSold);
    }

    let def = items.get(held.index as usize).ok_or(ShopError::UnknownItem(held.index))?;
    if def.sell_price() == 0 || def.rarity() == UNSELLABLE_RARITY {
        return Err(ShopError::CannotBeSold);
    }

    // Stacks sell for what they hold, which is the same field the purchase
    // wrote the count into.
    let count = if def.can_group() { held.refine.max(1) as u64 } else { 1 };
    let paid = (def.sell_price() as u64).saturating_mul(count);

    inventory.take(crate::inventory::BAG, slot).map_err(|_| ShopError::EmptySlot)?;

    Ok(Change { slot, item: Item::default(), gold: gold.saturating_add(paid) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::BAG;

    /// An item table with the few entries these tests price against, built
    /// rather than read: the real one is 14 MB and not in this repository.
    fn item_table() -> ItemList {
        // Records start at byte zero and an id is its index among them, so
        // the table has to reach past the highest id these tests use.
        let mut raw = vec![0u8; 9001 * aika_data::itemlist::RECORD_SIZE];

        let mut define = |id: usize, gold: u32, sell: u32, groups: bool, rarity: u8| {
            use aika_data::itemlist::{field, RECORD_SIZE};
            let at = id * RECORD_SIZE;
            let r = &mut raw[at..at + RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::PRICE_GOLD..field::PRICE_GOLD + 4].copy_from_slice(&gold.to_le_bytes());
            r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&sell.to_le_bytes());
            r[field::CAN_GROUP] = groups as u8;
            r[field::TYPE_ITEM] = rarity;
            r[field::DURABILITY] = 60;
        };

        define(1000, 500, 120, false, 1); // a sword
        define(4351, 10, 3, true, 2); // a potion, stacks
        define(4204, 0, 0, false, 1); // not for sale
        define(9000, 100, 40, false, UNSELLABLE_RARITY); // the rarest, unsellable

        ItemList::decode(&raw).expect("the fixture table is malformed")
    }

    fn merchant(stock: &[u16]) -> Npc {
        let mut shop = [0u16; SHOP_SLOTS];
        shop[..stock.len()].copy_from_slice(stock);
        Npc {
            id: 2050,
            title: "Merchant".into(),
            label: "Thomas Henrikson".into(),
            name_index: Some(43),
            options: vec![1, 2, 5, 8],
            equip: [234, 234, 0, 0, 0, 0, 0, 0],
            sizes: [7, 119, 119, 3],
            shop,
            max_hp: 20000,
            cur_hp: 20000,
            max_mp: 20000,
            cur_mp: 0,
            x: 3468.4,
            y: 963.4,
            rotation: 0,
            speed_move: 0,
            stale_id: None,
        }
    }

    fn buy_request(slot: u32, amount: u32) -> Buy {
        Buy { npc: 2050, slot, amount }
    }

    #[test]
    fn packet_bodies_roundtrip() {
        let b = Buy { npc: 2050, slot: 3, amount: 5 };
        assert_eq!(Buy::parse(&b.to_body()), Some(b));
        assert_eq!(Buy::parse(&[0u8; 8]), None);

        let s = Sell { npc: 2050, slot: 3 };
        assert_eq!(Sell::parse(&s.to_body()), Some(s));
        assert_eq!(Sell::parse(&[0u8; 4]), None);
    }

    #[test]
    fn buying_takes_the_gold_and_gives_the_item() {
        let npc = merchant(&[1000]);
        let mut inv = Inventory::new();

        let change = buy(&npc, buy_request(0, 1), &mut inv, 1000, &item_table()).unwrap();

        assert_eq!(change.gold, 500, "the price was not taken");
        assert_eq!(change.item.index, 1000);
        assert_eq!(inv.get(BAG, change.slot).unwrap().index, 1000);
    }

    /// The count only means something for items the table says group.
    #[test]
    fn a_stack_costs_per_unit_and_a_sword_does_not() {
        let table = item_table();
        let npc = merchant(&[4351, 1000]);

        let mut inv = Inventory::new();
        let potions = buy(&npc, buy_request(0, 20), &mut inv, 1000, &table).unwrap();
        assert_eq!(potions.gold, 1000 - 200, "20 potions at 10 each");
        assert_eq!(potions.item.refine, 20, "the count rides in the refine field");

        let mut inv = Inventory::new();
        let sword = buy(&npc, buy_request(1, 20), &mut inv, 1000, &table).unwrap();
        assert_eq!(sword.gold, 500, "a sword is one sword however many were asked for");
        assert_eq!(sword.item.refine, 0);
    }

    #[test]
    fn buying_without_the_gold_is_refused_and_changes_nothing() {
        let npc = merchant(&[1000]);
        let mut inv = Inventory::new();

        let result = buy(&npc, buy_request(0, 1), &mut inv, 100, &item_table());

        assert_eq!(result, Err(ShopError::NotEnoughGold { needed: 500, held: 100 }));
        assert!(inv.is_empty(), "the item arrived without being paid for");
    }

    #[test]
    fn buying_into_a_full_bag_is_refused() {
        let npc = merchant(&[1000]);
        let mut inv = Inventory::new();
        for slot in 0..crate::inventory::BAG_SLOTS {
            inv.put(Item { index: 1, container: BAG, slot, ..Item::default() }).unwrap();
        }

        assert_eq!(
            buy(&npc, buy_request(0, 1), &mut inv, 100000, &item_table()),
            Err(ShopError::BagFull)
        );
    }

    #[test]
    fn an_empty_shop_slot_sells_nothing() {
        let npc = merchant(&[1000]);
        let mut inv = Inventory::new();

        assert_eq!(
            buy(&npc, buy_request(7, 1), &mut inv, 1000, &item_table()),
            Err(ShopError::EmptySlot)
        );
        assert_eq!(
            buy(&npc, buy_request(0, 0), &mut inv, 1000, &item_table()),
            Err(ShopError::EmptySlot),
            "asking for zero of something is not a purchase"
        );
    }

    #[test]
    fn an_item_with_no_gold_price_is_not_for_sale() {
        let npc = merchant(&[4204]);
        let mut inv = Inventory::new();

        assert_eq!(
            buy(&npc, buy_request(0, 1), &mut inv, 100000, &item_table()),
            Err(ShopError::NotForSale)
        );
    }

    #[test]
    fn selling_empties_the_slot_and_pays() {
        let mut inv = Inventory::new();
        inv.put(Item { index: 1000, container: BAG, slot: 4, ..Item::default() }).unwrap();

        let change = sell(Sell { npc: 2050, slot: 4 }, &mut inv, 50, &item_table()).unwrap();

        assert_eq!(change.gold, 170, "50 held plus 120 for the sword");
        assert_eq!(change.slot, 4);
        assert!(change.item.is_empty(), "the client is told the slot is now empty");
        assert!(inv.get(BAG, 4).is_none());
    }

    #[test]
    fn selling_a_stack_pays_for_what_it_holds() {
        let mut inv = Inventory::new();
        inv.put(Item { index: 4351, container: BAG, slot: 0, refine: 12, ..Item::default() })
            .unwrap();

        let change = sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &item_table()).unwrap();
        assert_eq!(change.gold, 36, "12 potions at 3 each");
    }

    /// A rented item is on loan; selling it would turn borrowed time into
    /// permanent gold.
    #[test]
    fn a_rented_item_cannot_be_sold() {
        let mut inv = Inventory::new();
        inv.put(Item { index: 1000, container: BAG, slot: 0, expires_at: 30, ..Item::default() })
            .unwrap();

        assert_eq!(
            sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &item_table()),
            Err(ShopError::CannotBeSold)
        );
        assert!(inv.get(BAG, 0).is_some(), "the item was taken anyway");
    }

    #[test]
    fn an_unsellable_type_is_refused() {
        let mut inv = Inventory::new();
        inv.put(Item { index: 9000, container: BAG, slot: 0, ..Item::default() }).unwrap();

        assert_eq!(
            sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &item_table()),
            Err(ShopError::CannotBeSold)
        );
    }

    #[test]
    fn selling_an_empty_slot_is_refused() {
        let mut inv = Inventory::new();
        assert_eq!(
            sell(Sell { npc: 2050, slot: 3 }, &mut inv, 0, &item_table()),
            Err(ShopError::EmptySlot)
        );
    }
}
