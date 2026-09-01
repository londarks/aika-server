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

/// Only a freely tradable item can be sold back (`TypeTrade = 0`).
const TRADE_FREE: u8 = 0;

/// Two item types keep more of their value when sold back: a quarter instead
/// of a fifth (`PacketHandlers.pas:5134`).
const BETTER_RESALE_TYPES: [u16; 2] = [60, 61];

/// What an item costs, and in what.
///
/// The order is the original's, in `TItemFunctions.GetBuyItemPrice`
/// (`Functions/ItemFunctions.pas:215`), and it is not the order anyone would
/// guess: gold is the fallback, and the gold amount comes from the field
/// called `SellPrince`, not from the one called `PriceGold`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Price {
    /// Paid for with another item: which one, and how many.
    Item { id: u16, amount: u32 },
    Honor(u32),
    Medal(u32),
    Gold(u64),
}

/// What one purchase costs.
pub fn price_of(def: &aika_data::itemlist::ItemDef, quantity: u32) -> Price {
    let n = quantity.max(1);
    if def.price_item() > 0 {
        return Price::Item {
            id: def.price_item(),
            amount: def.price_item_value() as u32 * n,
        };
    }
    // Honor only wins when there is no gold price at all, which is the one
    // condition in the chain that looks at two fields at once.
    if def.price_honor() > 0 && def.base_price() == 0 {
        return Price::Honor(def.price_honor() * n);
    }
    if def.price_medal() > 0 {
        return Price::Medal(def.price_medal() * n);
    }
    Price::Gold(def.base_price() as u64 * n as u64)
}

/// What a shop pays for an item coming back.
///
/// Cheap things are refunded in full and everything else is divided down, so
/// buying and selling in a loop loses money rather than making it. The rules
/// are in `TPacketHandlers.SellNPCItens` (`PacketHandlers.pas:5126`).
pub fn resale_value(def: &aika_data::itemlist::ItemDef, item: &Item, stacks: bool) -> u64 {
    let base = def.base_price() as u64;

    if stacks {
        let count = item.refine.max(1) as u64;
        if base < 5 {
            return base * count;
        }
        let divisor = if BETTER_RESALE_TYPES.contains(&def.item_type()) { 4 } else { 5 };
        return (base / divisor) * count;
    }

    // A single item is worth its share of what is left of its durability.
    let full = base / 5;
    if item.durability_max == 0 {
        return full;
    }
    let wear = item.durability_min as f64 / item.durability_max as f64;
    (full as f64 * wear).round() as u64
}

/// Why a purchase or a sale was refused. Each carries the sentence the player
/// should read, because a shop that silently does nothing is worse than one
/// that says no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopError {
    /// No such NPC, or the player is not standing near enough to trade.
    WrongNpc,
    /// The slot the client named holds nothing.
    EmptySlot,
    /// An id the shop offers that the server's item table does not define.
    ///
    /// This is not a client lying: the client draws the shop from its own
    /// item table, so it shows a name and a description for something our
    /// table has never heard of. The two files are out of step in the data
    /// they were taken from, and the server cannot price, level-gate or even
    /// name what it does not have.
    ///
    /// The original refuses these too, though not on purpose: a missing
    /// record reads as a row of zeros, its price chain ends at "gold, zero",
    /// and the gold branch exits on any price of one or less.
    UnknownItem(u16),
    /// The item has no price at all, which is how the shipped tables mark
    /// something that is not really on sale.
    NotForSale,
    /// Priced in something this server does not keep yet.
    PaidInSomethingElse(&'static str),
    NotEnoughGold { needed: u64, held: u64 },
    /// Priced in an item the player does not have enough of (`BuyNPCItens`,
    /// the `PRICE_ITEM` branch).
    NotEnoughCurrency,
    BagFull,
    /// A rented item, or one the table marks as unsellable.
    CannotBeSold,
}

impl ShopError {
    /// What the player is told.
    pub fn message(&self) -> String {
        match self {
            ShopError::WrongNpc => "You are too far from the shop.".into(),
            ShopError::EmptySlot => "There is nothing there.".into(),
            ShopError::UnknownItem(id) => {
                format!("This server does not have item {id}, even though your client draws it.")
            }
            ShopError::NotForSale => "That is not for sale.".into(),
            ShopError::PaidInSomethingElse(what) => {
                format!("That is paid for with {what}, which is not in yet.")
            }
            ShopError::NotEnoughGold { needed, held } => {
                format!("You need {needed} gold and have {held}.")
            }
            ShopError::NotEnoughCurrency => {
                "Você não possui a quantidade de itens necessária.".into()
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
    /// Bag slots whose currency was spent to pay, and what is left in each.
    /// Empty for a gold purchase; one or more for an item-currency one.
    pub spent: Vec<(u16, Item)>,
}

/// The forty ids of a shop window, in slot order.
pub fn stock(npc: &Npc) -> [u16; SHOP_SLOTS] {
    npc.shop
}

/// Buys one slot's worth from an NPC.
///
/// The amount lands in the item's `refine` field, and it lands there for
/// *everything*, not only for things that stack: the original writes
/// `BuyItem.Refi := Packet.Quantidade` before it looks at what the item is
/// (`PacketHandlers.pas:5048`). It matters. A card or a scroll that arrives
/// with a count of zero is a stack of none, and the client refuses to use it
/// — which looks exactly like the item being broken.
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

    // A stack is only a stack if the table says the item groups; anything
    // else is one item however many the client asked for.
    let amount = if def.can_group() { request.amount.max(1) } else { 1 };

    // What the item costs, and the change made to pay it. Gold is the common
    // case; an item currency (the "moeda da determinação" and its kin) is paid
    // out of the bag instead, which the original handles in its own branch.
    let mut new_gold = gold;
    let mut spent: Vec<(u16, Item)> = Vec::new();
    match price_of(def, amount) {
        // The original refuses a gold price of one or less, not just of zero
        // (`PacketHandlers.pas:5042`). An undefined item reads as a row of
        // zeros and lands here, which is how that check keeps items the
        // server does not know about out of the game.
        Price::Gold(price) if price <= 1 => return Err(ShopError::NotForSale),
        Price::Gold(price) => {
            if price > gold {
                return Err(ShopError::NotEnoughGold { needed: price, held: gold });
            }
            new_gold = gold - price;
        }
        Price::Item { id: currency, amount: needed } => {
            spent = spend_currency(inventory, currency, needed)?;
        }
        // Honor and medals are account-level currencies we do not keep yet.
        Price::Honor(_) => return Err(ShopError::PaidInSomethingElse("honor")),
        Price::Medal(_) => return Err(ShopError::PaidInSomethingElse("medals")),
    }

    if inventory.free_slots(crate::inventory::BAG) == 0 {
        return Err(ShopError::BagFull);
    }

    let bought = Item {
        index: id,
        appearance: id,
        refine: amount as u16,
        durability_min: def.durability(),
        durability_max: def.durability(),
        ..Item::default()
    };
    let slot = inventory.add(bought.clone())?;

    let item = inventory.get(crate::inventory::BAG, slot).cloned().unwrap_or(bought);
    Ok(Change { slot, item, gold: new_gold, spent })
}

/// Takes `needed` of a currency item out of the bag, across as many stacks as
/// it sits in, and says what each drained slot holds afterwards.
///
/// The original looks in one slot (`GetItemSlot2`) and refuses if that stack
/// alone is short; draining across stacks is friendlier and cannot pay more
/// than the player has, so the outcome is the same when the currency is in one
/// pile, which it almost always is.
fn spend_currency(
    inventory: &mut Inventory,
    currency: u16,
    needed: u32,
) -> Result<Vec<(u16, Item)>, ShopError> {
    let held: u32 = inventory
        .in_container(crate::inventory::BAG)
        .filter(|i| i.index == currency)
        .map(|i| i.refine.max(1) as u32)
        .sum();
    if held < needed {
        return Err(ShopError::NotEnoughCurrency);
    }

    let mut left = needed;
    let mut slots: Vec<u16> = inventory
        .in_container(crate::inventory::BAG)
        .filter(|i| i.index == currency)
        .map(|i| i.slot)
        .collect();
    slots.sort_unstable();

    let mut changed = Vec::new();
    for slot in slots {
        if left == 0 {
            break;
        }
        let mut stack = inventory.get(crate::inventory::BAG, slot).cloned().unwrap();
        let take = left.min(stack.refine.max(1) as u32) as u16;
        stack.refine -= take;
        left -= take as u32;

        if stack.refine == 0 {
            let _ = inventory.take(crate::inventory::BAG, slot);
            changed.push((slot, Item { container: crate::inventory::BAG, slot, ..Item::default() }));
        } else {
            let _ = inventory.put(stack.clone());
            changed.push((slot, stack));
        }
    }
    Ok(changed)
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
    if def.base_price() == 0 || def.rarity() == UNSELLABLE_RARITY {
        return Err(ShopError::CannotBeSold);
    }

    // A stack goes back whole; a single item has to be tradable to go back at
    // all, which is the check that keeps bound gear out of the shop.
    let stacks = def.can_group();
    if !stacks && def.trade_kind() != TRADE_FREE {
        return Err(ShopError::CannotBeSold);
    }

    let paid = resale_value(def, &held, stacks);
    inventory.take(crate::inventory::BAG, slot).map_err(|_| ShopError::EmptySlot)?;

    Ok(Change { slot, item: Item::default(), gold: gold.saturating_add(paid), spent: Vec::new() })
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

        // `base` is what the shop asks and what the resale is divided from:
        // one field does both jobs, which is the whole trap here.
        let mut define = |id: usize, base: u32, groups: bool, rarity: u8| {
            use aika_data::itemlist::{field, RECORD_SIZE};
            let at = id * RECORD_SIZE;
            let r = &mut raw[at..at + RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&base.to_le_bytes());
            r[field::CAN_GROUP] = groups as u8;
            r[field::TYPE_ITEM] = rarity;
            r[field::DURABILITY] = 60;
        };

        define(1000, 500, false, 1); // a sword
        define(4351, 10, true, 2); // a potion, stacks
        define(4616, 2, true, 1); // ammunition, too cheap to divide
        define(4204, 0, false, 1); // no price at all
        define(5000, 1, false, 1); // a price of one, which the original refuses
        define(9000, 100, false, UNSELLABLE_RARITY); // the rarest, unsellable

        // priced in something other than gold
        let mut currency = |id: usize, honor: u32, medal: u32, base: u32| {
            use aika_data::itemlist::{field, RECORD_SIZE};
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::PRICE_HONOR..field::PRICE_HONOR + 4].copy_from_slice(&honor.to_le_bytes());
            r[field::PRICE_MEDAL..field::PRICE_MEDAL + 4].copy_from_slice(&medal.to_le_bytes());
            r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&base.to_le_bytes());
        };
        currency(6000, 90, 0, 0); // honor only
        currency(6001, 90, 0, 300); // honor loses to a gold price
        currency(6002, 0, 300, 50); // medals beat gold

        {
            use aika_data::itemlist::{field, RECORD_SIZE};
            let r = &mut raw[6003 * RECORD_SIZE..6004 * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::PRICE_ITEM..field::PRICE_ITEM + 2].copy_from_slice(&4204u16.to_le_bytes());
            r[field::PRICE_ITEM_VALUE..field::PRICE_ITEM_VALUE + 2]
                .copy_from_slice(&2u16.to_le_bytes());

            // bound gear: worth something, but not tradable
            let r = &mut raw[7000 * RECORD_SIZE..7001 * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&900u32.to_le_bytes());
            r[field::TYPE_TRADE] = 1;
            r[field::DURABILITY] = 60;
        }

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
    /// Everything bought carries a count, stackable or not. A card that
    /// arrives with a count of zero is a stack of none, and the client will
    /// not use it.
    #[test]
    fn everything_bought_carries_a_count() {
        let table = item_table();
        let npc = merchant(&[1000, 4351]);

        let mut inv = Inventory::new();
        let sword = buy(&npc, buy_request(0, 1), &mut inv, 100_000, &table).unwrap();
        assert_eq!(sword.item.refine, 1, "a sword arrived as a stack of none");

        let mut inv = Inventory::new();
        let potions = buy(&npc, buy_request(1, 20), &mut inv, 100_000, &table).unwrap();
        assert_eq!(potions.item.refine, 20);
    }

    /// The original exits on a gold price of one or less, which is what keeps
    /// an item the table does not define out of the game: a missing record is
    /// a row of zeros.
    #[test]
    fn something_priced_at_a_penny_or_nothing_is_not_for_sale() {
        let table = item_table();
        let npc = merchant(&[4204, 5000]);
        let mut inv = Inventory::new();

        assert_eq!(
            buy(&npc, buy_request(0, 1), &mut inv, 100_000, &table),
            Err(ShopError::NotForSale),
            "a price of zero"
        );
        assert_eq!(
            buy(&npc, buy_request(1, 1), &mut inv, 100_000, &table),
            Err(ShopError::NotForSale),
            "a price of one"
        );
    }

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
        assert_eq!(sword.item.refine, 1, "and it arrives as one of them, not none");
    }

    /// Item 6003 is priced in two of item 4204, the way the "moeda da
    /// determinação" items are: paid out of the bag, not out of gold.
    #[test]
    fn an_item_priced_in_a_currency_is_paid_from_the_bag() {
        let npc = merchant(&[6003]);
        let mut inv = Inventory::new();
        inv.put(Item {
            container: BAG,
            slot: 0,
            index: 4204, // the currency
            refine: 5,
            ..Item::default()
        })
        .unwrap();

        let change = buy(&npc, buy_request(0, 1), &mut inv, 0, &item_table()).unwrap();

        assert_eq!(change.gold, 0, "gold must not be touched for a currency purchase");
        assert_eq!(change.item.index, 6003, "the item was not handed over");
        assert_eq!(
            inv.get(BAG, 0).unwrap().refine,
            3,
            "two of the currency were not taken"
        );
        assert_eq!(change.spent.len(), 1, "the drained currency slot was not reported");
    }

    /// Not enough of the currency refuses, and takes nothing.
    #[test]
    fn buying_a_currency_item_without_the_currency_is_refused() {
        let npc = merchant(&[6003]);
        let mut inv = Inventory::new();
        inv.put(Item { container: BAG, slot: 0, index: 4204, refine: 1, ..Item::default() })
            .unwrap();

        assert_eq!(
            buy(&npc, buy_request(0, 1), &mut inv, 0, &item_table()),
            Err(ShopError::NotEnoughCurrency)
        );
        assert_eq!(inv.get(BAG, 0).unwrap().refine, 1, "the currency was taken on a refusal");
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
        inv.put(Item {
            index: 1000,
            container: BAG,
            slot: 4,
            durability_min: 60,
            durability_max: 60,
            ..Item::default()
        })
        .unwrap();

        let change = sell(Sell { npc: 2050, slot: 4 }, &mut inv, 50, &item_table()).unwrap();

        assert_eq!(change.gold, 150, "50 held plus a fifth of the sword price");
        assert_eq!(change.slot, 4);
        assert!(change.item.is_empty(), "the client is told the slot is now empty");
        assert!(inv.get(BAG, 4).is_none());
    }

    #[test]
    fn selling_a_stack_pays_a_fifth_for_what_it_holds() {
        let mut inv = Inventory::new();
        inv.put(Item { index: 4351, container: BAG, slot: 0, refine: 12, ..Item::default() })
            .unwrap();

        let change = sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &item_table()).unwrap();
        assert_eq!(change.gold, 24, "12 potions at a fifth of 10 each");
    }

    /// Cheap things are refunded whole. Dividing a price of one down would
    /// round it to nothing, and a shop that pays zero looks broken.
    #[test]
    fn something_cheap_is_refunded_in_full() {
        let table = item_table();
        let mut inv = Inventory::new();
        inv.put(Item { index: 4616, container: BAG, slot: 0, refine: 10, ..Item::default() })
            .unwrap();

        let change = sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &table).unwrap();
        assert_eq!(change.gold, 20, "10 rounds at 2 each, not divided down");
    }

    /// A worn item is worth less than a new one, in proportion.
    #[test]
    fn a_damaged_item_fetches_less() {
        let table = item_table();

        let mut inv = Inventory::new();
        inv.put(Item {
            index: 1000,
            container: BAG,
            slot: 0,
            durability_min: 30,
            durability_max: 60,
            ..Item::default()
        })
        .unwrap();

        let change = sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &table).unwrap();
        assert_eq!(change.gold, 50, "half worn, so half of the hundred");
    }

    /// The one economic invariant that matters: a shop must never be a way to
    /// make money. Buy anything and sell it straight back, and you are poorer.
    #[test]
    fn buying_and_selling_back_always_loses_money() {
        let table = item_table();
        let npc = merchant(&[1000, 4351]);

        for slot in [0, 1] {
            let mut inv = Inventory::new();
            let bought = buy(&npc, buy_request(slot, 1), &mut inv, 100_000, &table).unwrap();
            let spent = 100_000 - bought.gold;

            let back = sell(Sell { npc: 2050, slot: bought.slot as u32 }, &mut inv, 0, &table).unwrap();
            assert!(
                back.gold < spent,
                "slot {slot}: paid {spent} and got {} back",
                back.gold
            );
        }
    }

    /// Bound gear cannot be sold, however valuable it is.
    #[test]
    fn an_item_that_cannot_be_traded_cannot_be_sold() {
        let mut inv = Inventory::new();
        inv.put(Item { index: 7000, container: BAG, slot: 0, ..Item::default() }).unwrap();

        assert_eq!(
            sell(Sell { npc: 2050, slot: 0 }, &mut inv, 0, &item_table()),
            Err(ShopError::CannotBeSold)
        );
    }

    /// The order in `GetBuyItemPrice` is not the order anyone would guess, so
    /// each branch of it is pinned here.
    #[test]
    fn the_price_of_an_item_follows_the_original_order() {
        let table = item_table();
        let price = |id: usize, n| price_of(table.get(id).unwrap(), n);

        assert_eq!(price(1000, 1), Price::Gold(500), "gold is the fallback");
        assert_eq!(price(4351, 20), Price::Gold(200), "and it multiplies");
        assert_eq!(price(6000, 1), Price::Honor(90), "honor wins with no gold price");
        assert_eq!(
            price(6001, 2),
            Price::Gold(600),
            "honor loses to a gold price, however large it is"
        );
        assert_eq!(price(6002, 3), Price::Medal(900), "medals beat gold");
        assert_eq!(
            price(6003, 2),
            Price::Item { id: 4204, amount: 4 },
            "paying with an item beats every currency"
        );
    }

    /// The account-level currencies we do not keep yet — honor and medals —
    /// are refused rather than silently charged in gold. An item currency is
    /// not among them: it is paid from the bag, and has its own test.
    #[test]
    fn a_price_in_an_account_currency_is_refused_rather_than_charged_in_gold() {
        let table = item_table();
        let npc = merchant(&[6000, 6002]);
        let mut inv = Inventory::new();

        assert_eq!(
            buy(&npc, buy_request(0, 1), &mut inv, 100_000, &table),
            Err(ShopError::PaidInSomethingElse("honor"))
        );
        assert_eq!(
            buy(&npc, buy_request(1, 1), &mut inv, 100_000, &table),
            Err(ShopError::PaidInSomethingElse("medals"))
        );
        assert!(inv.is_empty(), "something was handed over anyway");
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
