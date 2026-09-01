//! What a character carries.
//!
//! Two containers matter while playing: the sixteen equipment slots and the
//! hundred and twenty-six bag slots. Both live inside the `TCharacter` record
//! the client receives on entering the world (`Data/PlayerData.pas:208`), at
//! offsets 340 and 664, with the gold right after the bag at 3184. The sizes
//! are not guesses: 664 + 126 × 20 lands exactly on the gold, which is the
//! arithmetic that proves the bag holds 126 and not the 60 the comment beside
//! it claims.
//!
//! Slots are addressed, not ordered: slot 7 is a place on the screen, and an
//! item that moves from slot 7 to slot 3 has to leave 7 empty. That is why
//! this is a sparse list rather than a packed one — the same shape the
//! database stores, so nothing is translated on the way in or out.

use crate::store::Item;

/// Equipment slots, from `Equip: Array [0 .. 15] of TITEM`.
pub const EQUIP_SLOTS: u16 = 16;
/// Bag slots, from `Inventory: Array [0 .. 125] of TITEM`.
pub const BAG_SLOTS: u16 = 126;
/// Storage slots, from `Itens: Array [0 .. 85] of TITEM`
/// (`TStoragePlayer`, `Data/PlayerData.pas:376`).
pub const STORAGE_SLOTS: u16 = 86;

/// Which container a slot belongs to. The numbers are the `TypeSlot` the
/// protocol carries and the `container` column the database stores.
pub const EQUIP: u8 = 0;
pub const BAG: u8 = 1;
pub const STORAGE: u8 = 2;

/// How many slots a container has, or `None` for one we do not model yet.
pub fn capacity(container: u8) -> Option<u16> {
    match container {
        EQUIP => Some(EQUIP_SLOTS),
        BAG => Some(BAG_SLOTS),
        STORAGE => Some(STORAGE_SLOTS),
        _ => None,
    }
}

/// How many slots one page of a container holds. Both the bag and the storage
/// are unlocked twenty at a time.
const PAGE: u16 = 20;

/// The bag and the storage each keep the items that unlock their pages in the
/// slots past the usable ones: the six bags at 120 to 125 and the four vaults
/// at 80 to 83 (`MoveItem`, `PacketHandlers.pas:5376`). Those slots are not
/// places to put things; they are the reason the places exist.
pub const BAG_PAGE_ITEMS: std::ops::RangeInclusive<u16> = 120..=125;
pub const STORAGE_PAGE_ITEMS: std::ops::RangeInclusive<u16> = 80..=83;
/// The last two storage slots hold prans and nothing else.
pub const STORAGE_PRAN_SLOTS: [u16; 2] = [84, 85];

/// Which slot holds the item that unlocks the page this one is on, or `None`
/// for a slot that needs no unlocking.
///
/// This is what stops an item being dropped into a page the player has not
/// bought: the original looks the unlocking item up on both sides of a move
/// and refuses when its slot is empty. Equipment has no pages.
pub fn page_item_for(container: u8, slot: u16) -> Option<u16> {
    match container {
        BAG if slot < *BAG_PAGE_ITEMS.start() => {
            Some(BAG_PAGE_ITEMS.start() + slot / PAGE)
        }
        STORAGE if slot < *STORAGE_PAGE_ITEMS.start() => {
            Some(STORAGE_PAGE_ITEMS.start() + slot / PAGE)
        }
        _ => None,
    }
}

/// The equipment slot ammunition goes in, and the one every weapon goes in,
/// whatever kind of weapon it is.
pub const AMMO_SLOT: u16 = 15;
pub const WEAPON_SLOT: u16 = 6;

/// Which equipment slot an item belongs in, or `None` for one that is not
/// equipment at all (`TItemFunctions.GetItemEquipSlot`,
/// `Functions/ItemFunctions.pas:605`).
///
/// For most gear the slot *is* the item type — armour of type 3 goes in slot
/// 3 — which reads like a coincidence and is not: the table was built that
/// way. Weapons are the exception, a whole range of types that all go in the
/// hand, and ammunition another.
///
/// The original returns zero for anything else, and zero is its way of saying
/// "this belongs in the bag" rather than "slot 0"; slot 0 is the body and slot
/// 1 the hair, and neither is an item anybody may move.
pub fn equip_slot_for(item_type: u16) -> Option<u16> {
    match item_type {
        50 | 52 | 102 | 103 => Some(AMMO_SLOT),
        1000..=1011 | 1019 => Some(WEAPON_SLOT),
        1..=16 => Some(item_type),
        _ => None,
    }
}

/// Whether a slot holds a page-unlocking item, which cannot itself be dragged
/// anywhere: the original leaves the range that reaches them unhandled and
/// falls out of `MoveItem`.
pub fn is_page_item(container: u8, slot: u16) -> bool {
    match container {
        BAG => BAG_PAGE_ITEMS.contains(&slot),
        STORAGE => STORAGE_PAGE_ITEMS.contains(&slot),
        _ => false,
    }
}

/// Why an item could not be moved or added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryError {
    /// The container is not one we handle, or the slot is past its end.
    NoSuchSlot { container: u8, slot: u16 },
    /// There was nothing in the slot the client named.
    SlotEmpty { container: u8, slot: u16 },
    /// Every bag slot is taken.
    Full,
}

impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InventoryError::NoSuchSlot { container, slot } => {
                write!(f, "container {container} has no slot {slot}")
            }
            InventoryError::SlotEmpty { container, slot } => {
                write!(f, "slot {slot} of container {container} is empty")
            }
            InventoryError::Full => write!(f, "the bag is full"),
        }
    }
}

impl std::error::Error for InventoryError {}

/// Everything a character carries, across every container.
///
/// Sparse on purpose: only occupied slots are stored, which is also how the
/// database keeps them, so loading and saving move rows rather than translate
/// a layout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    items: Vec<Item>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Item> {
        self.items.iter()
    }

    /// Everything in one container, in no particular order.
    pub fn in_container(&self, container: u8) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |item| item.container == container)
    }

    pub fn get(&self, container: u8, slot: u16) -> Option<&Item> {
        self.items.iter().find(|i| i.container == container && i.slot == slot)
    }

    /// The lowest empty slot in a container, which is where a bought or
    /// picked up item goes.
    pub fn first_free(&self, container: u8) -> Option<u16> {
        let capacity = capacity(container)?;
        (0..capacity).find(|slot| self.get(container, *slot).is_none())
    }

    /// Puts an item in the slot it names, replacing whatever was there.
    ///
    /// Used when loading from the database and when the caller has already
    /// decided the slot. To add something without caring where, use `add`.
    pub fn put(&mut self, item: Item) -> Result<(), InventoryError> {
        let within = capacity(item.container).is_some_and(|c| item.slot < c);
        if !within {
            return Err(InventoryError::NoSuchSlot {
                container: item.container,
                slot: item.slot,
            });
        }
        self.items.retain(|i| !(i.container == item.container && i.slot == item.slot));
        self.items.push(item);
        Ok(())
    }

    /// Puts an item in the first free bag slot and says which one it took.
    pub fn add(&mut self, mut item: Item) -> Result<u16, InventoryError> {
        let slot = self.first_free(BAG).ok_or(InventoryError::Full)?;
        item.container = BAG;
        item.slot = slot;
        self.put(item)?;
        Ok(slot)
    }

    /// Removes what is in a slot and hands it back.
    pub fn take(&mut self, container: u8, slot: u16) -> Result<Item, InventoryError> {
        let at = self
            .items
            .iter()
            .position(|i| i.container == container && i.slot == slot)
            .ok_or(InventoryError::SlotEmpty { container, slot })?;
        Ok(self.items.remove(at))
    }

    /// Moves an item between slots, swapping when the destination is taken.
    ///
    /// Swapping is what the client expects: dragging one item onto another
    /// exchanges them rather than destroying either. The two-step through
    /// `take` is deliberate — writing the destination before removing the
    /// source would lose the item if the slot numbers were the same.
    pub fn move_item(
        &mut self,
        from: (u8, u16),
        to: (u8, u16),
    ) -> Result<(), InventoryError> {
        let within = capacity(to.0).is_some_and(|c| to.1 < c);
        if !within {
            return Err(InventoryError::NoSuchSlot { container: to.0, slot: to.1 });
        }
        if from == to {
            return Ok(());
        }

        let mut moving = self.take(from.0, from.1)?;
        let displaced = self.take(to.0, to.1).ok();

        moving.container = to.0;
        moving.slot = to.1;
        self.put(moving)?;

        if let Some(mut displaced) = displaced {
            displaced.container = from.0;
            displaced.slot = from.1;
            self.put(displaced)?;
        }
        Ok(())
    }

    /// Moves an item out of this inventory into another one, swapping when the
    /// destination is taken.
    ///
    /// The storage belongs to the account and the bag to the character, so a
    /// move between the two crosses two of these. Same two-step as
    /// [`Inventory::move_item`]: both sides come out before either goes back,
    /// so nothing can end up in two places at once.
    pub fn move_into(
        &mut self,
        from: (u8, u16),
        other: &mut Inventory,
        to: (u8, u16),
    ) -> Result<(), InventoryError> {
        let within = capacity(to.0).is_some_and(|c| to.1 < c);
        if !within {
            return Err(InventoryError::NoSuchSlot { container: to.0, slot: to.1 });
        }

        let mut moving = self.take(from.0, from.1)?;
        let displaced = other.take(to.0, to.1).ok();

        moving.container = to.0;
        moving.slot = to.1;
        other.put(moving)?;

        if let Some(mut displaced) = displaced {
            displaced.container = from.0;
            displaced.slot = from.1;
            self.put(displaced)?;
        }
        Ok(())
    }

    /// How many free slots the bag has, which is what a purchase checks.
    pub fn free_slots(&self, container: u8) -> u16 {
        let Some(capacity) = capacity(container) else {
            return 0;
        };
        capacity - self.in_container(container).count() as u16
    }
}

impl From<Vec<Item>> for Inventory {
    fn from(items: Vec<Item>) -> Self {
        Self { items }
    }
}

impl FromIterator<Item> for Inventory {
    fn from_iter<T: IntoIterator<Item = Item>>(iter: T) -> Self {
        Self { items: iter.into_iter().collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(index: u16, container: u8, slot: u16) -> Item {
        Item { index, container, slot, ..Item::default() }
    }

    #[test]
    fn a_slot_holds_one_thing() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 3)).unwrap();
        inv.put(item(2000, BAG, 3)).unwrap();

        assert_eq!(inv.len(), 1, "the second put has to replace the first");
        assert_eq!(inv.get(BAG, 3).unwrap().index, 2000);
    }

    #[test]
    fn adding_takes_the_lowest_free_slot() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 0)).unwrap();
        inv.put(item(1001, BAG, 2)).unwrap();

        assert_eq!(inv.add(item(1002, BAG, 999)).unwrap(), 1, "slot 1 was free");
        assert_eq!(inv.add(item(1003, BAG, 999)).unwrap(), 3);
    }

    /// The slot in an added item is whatever the caller left there; the bag
    /// decides, not the caller.
    #[test]
    fn adding_ignores_the_slot_it_was_handed() {
        let mut inv = Inventory::new();
        let slot = inv.add(item(1000, EQUIP, 55)).unwrap();

        assert_eq!(slot, 0);
        assert_eq!(inv.get(BAG, 0).unwrap().index, 1000);
        assert!(inv.get(EQUIP, 55).is_none(), "it must not land in equipment");
    }

    #[test]
    fn a_full_bag_refuses_more() {
        let mut inv = Inventory::new();
        for slot in 0..BAG_SLOTS {
            inv.put(item(1000 + slot, BAG, slot)).unwrap();
        }

        assert_eq!(inv.free_slots(BAG), 0);
        assert_eq!(inv.add(item(9999, BAG, 0)), Err(InventoryError::Full));
    }

    #[test]
    fn moving_to_an_empty_slot_leaves_the_old_one_empty() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 7)).unwrap();

        inv.move_item((BAG, 7), (BAG, 3)).unwrap();

        assert!(inv.get(BAG, 7).is_none(), "slot 7 still holds something");
        assert_eq!(inv.get(BAG, 3).unwrap().index, 1000);
        assert_eq!(inv.len(), 1, "the item was duplicated");
    }

    /// Dragging one item onto another exchanges them. Anything else loses an
    /// item, which players notice immediately.
    #[test]
    fn moving_onto_a_taken_slot_swaps() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 1)).unwrap();
        inv.put(item(2000, BAG, 2)).unwrap();

        inv.move_item((BAG, 1), (BAG, 2)).unwrap();

        assert_eq!(inv.get(BAG, 2).unwrap().index, 1000);
        assert_eq!(inv.get(BAG, 1).unwrap().index, 2000);
        assert_eq!(inv.len(), 2);
    }

    #[test]
    fn moving_between_containers_carries_the_container_across() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 4)).unwrap();

        inv.move_item((BAG, 4), (EQUIP, 5)).unwrap();

        let equipped = inv.get(EQUIP, 5).expect("not equipped");
        assert_eq!((equipped.container, equipped.slot), (EQUIP, 5));
    }

    /// A move onto itself is what the client sends when a drag is dropped
    /// where it started. It must not empty the slot.
    #[test]
    fn moving_a_slot_onto_itself_keeps_the_item() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 4)).unwrap();

        inv.move_item((BAG, 4), (BAG, 4)).unwrap();

        assert_eq!(inv.get(BAG, 4).unwrap().index, 1000);
    }

    #[test]
    fn refuses_slots_that_do_not_exist() {
        let mut inv = Inventory::new();

        assert_eq!(
            inv.put(item(1000, BAG, BAG_SLOTS)),
            Err(InventoryError::NoSuchSlot { container: BAG, slot: BAG_SLOTS })
        );
        assert_eq!(
            inv.put(item(1000, EQUIP, EQUIP_SLOTS)),
            Err(InventoryError::NoSuchSlot { container: EQUIP, slot: EQUIP_SLOTS })
        );
        assert!(inv.put(item(1000, STORAGE, STORAGE_SLOTS)).is_err(), "past the end of the chest");
    }

    #[test]
    fn moving_from_an_empty_slot_is_an_error_not_a_silent_nothing() {
        let mut inv = Inventory::new();
        assert_eq!(
            inv.move_item((BAG, 4), (BAG, 5)),
            Err(InventoryError::SlotEmpty { container: BAG, slot: 4 })
        );
    }

    /// A failed move must not have taken the item out on the way.
    #[test]
    fn a_move_to_a_bad_destination_leaves_the_item_alone() {
        let mut inv = Inventory::new();
        inv.put(item(1000, BAG, 4)).unwrap();

        assert!(inv.move_item((BAG, 4), (BAG, BAG_SLOTS)).is_err());
        assert_eq!(inv.get(BAG, 4).unwrap().index, 1000, "the item vanished");
    }
}
