//! When an item runs out.
//!
//! Some items are lent rather than given: a thirty-day saddle, a three-hour
//! potion. The item table says which ones with a flag of its own, and how long
//! they last in hours; the item itself then carries the moment it dies.
//!
//! # Two encodings in the same two bytes
//!
//! The record has one field for this, `Time`, two bytes wide at the very end
//! of the twenty-byte item. The original writes two entirely different things
//! into it depending on what the item is (`TItem.SetExpire`,
//! `Data/MiscData.pas:404`):
//!
//! - a **mount** — item type 9 — stores whole days between the expiry and a
//!   fixed base date, and uses those two bytes and nothing else;
//! - **everything else** stores the unix time of the expiry shifted right by
//!   eight bits, three bytes of it, written from one byte *before* `Time`.
//!
//! That second one deliberately reaches back into the byte above, which is the
//! top half of the stack count beside it. It is not a mistake to tidy up: the
//! client reads those three bytes back the same way, so an item written any
//! other way comes out expired. It does mean a stack of more than 255 cannot
//! also carry an expiry, which is the original's problem and not ours to fix.
//!
//! Shifting by eight is what buys the third byte: it costs the low bits of the
//! second, so an expiry is only accurate to about four minutes.
//!
//! # One number for both
//!
//! Rather than carry the item's type everywhere an item is written, the value
//! kept on the item is the three bytes *as they sit at offsets 17, 18 and 19*.
//! A mount's day count is therefore stored shifted up a byte, which leaves its
//! low byte zero, so writing all three is the same as writing the two the
//! original writes. That holds because the byte being zeroed is the top half
//! of the stack count, and a mount does not stack — the one case where the two
//! encodings could disagree cannot happen.

use crate::store::Item;
use aika_data::itemlist::ItemList;
use chrono::{Duration, NaiveDate, NaiveDateTime};

/// Mounts, which are the one item type that counts in days.
const ITEM_TYPE_MOUNT: u16 = 9;

/// The date the mount countdown is measured from: `01/01/2023 22:00`
/// (`BASE_DATETIME`, `Data/GlobalDefs.pas:81`).
fn base_datetime() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2023, 1, 1)
        .and_then(|d| d.and_hms_opt(22, 0, 0))
        .expect("the base date is a constant and is valid")
}

/// The two hours the original adds on top of the item's own duration, without
/// saying why (`SetItemDuration`).
const GRACE_HOURS: i64 = 2;

/// When an item handed over now would run out.
pub fn expiry_of(now: NaiveDateTime, duration_hours: u32) -> NaiveDateTime {
    now + Duration::hours(duration_hours as i64 + GRACE_HOURS)
}

/// The three bytes the record carries for an expiry, as they sit at offsets
/// 17, 18 and 19 of the item.
pub fn encode(item_type: u16, expiry: NaiveDateTime) -> u32 {
    if item_type == ITEM_TYPE_MOUNT {
        // Whole days, in the two bytes of `Time`, which is one byte up.
        let days = (expiry - base_datetime()).num_days().clamp(0, u16::MAX as i64);
        return (days as u32) << 8;
    }
    let seconds = expiry.and_utc().timestamp().max(0);
    // Three bytes of it, so the low eight bits go.
    ((seconds >> 8) & 0x00FF_FFFF) as u32
}

/// Stamps an item with its expiry, if it is one of the ones that runs out.
///
/// This is `PutItem` calling `SetItemDuration`: the original stamps an item as
/// it goes into the inventory, not when it is defined, so the clock starts
/// when the player receives it. An item that does not expire is left alone.
pub fn stamp(item: &mut Item, items: &ItemList, now: NaiveDateTime) {
    let Some(def) = items.get(item.index as usize) else {
        return;
    };
    if !def.expires() {
        return;
    }
    item.expires_at = encode(def.item_type(), expiry_of(now, def.duration()));
}

/// The local wall clock, which is what the original stamps items with.
pub fn now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

/// Writes the expiry into the twenty bytes of an item.
///
/// Called after the count, because the two overlap by a byte and the expiry is
/// the one that wins — which is the original's own layout and not a mistake.
pub fn write_into(out: &mut [u8], item: &Item) {
    if item.expires_at == 0 {
        return;
    }
    let bytes = item.expires_at.to_le_bytes();
    out[17..20].copy_from_slice(&bytes[0..3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aika_data::itemlist::{field, RECORD_SIZE};

    const SADDLE: u16 = 4503;
    const HORSE: u16 = 963;

    /// A table with one timed saddle and one timed mount, so both branches
    /// have something real to work on.
    fn table() -> ItemList {
        let mut raw = vec![0u8; 5000 * RECORD_SIZE];
        let mut define = |id: u16, item_type: u16, hours: u32, expires: bool| {
            let r = &mut raw[id as usize * RECORD_SIZE..(id as usize + 1) * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::ITEM_TYPE..field::ITEM_TYPE + 2].copy_from_slice(&item_type.to_le_bytes());
            r[field::DURATION..field::DURATION + 4].copy_from_slice(&hours.to_le_bytes());
            r[field::EXPIRES] = expires as u8;
        };
        // the saddle really is 720 hours, which is the thirty days its name promises
        define(SADDLE, 715, 720, true);
        define(HORSE, 9, 720, true);
        define(1000, 1, 0, false);
        ItemList::decode(&raw).expect("the fixture table is malformed")
    }

    fn at(y: i32, m: u32, d: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, 0, 0).unwrap()
    }

    #[test]
    fn a_thirty_day_item_runs_out_thirty_days_later() {
        let expiry = expiry_of(at(2026, 9, 1, 0), 720);
        // Thirty days, and the two hours the original adds on top.
        assert_eq!(expiry, at(2026, 10, 1, 2));
    }

    /// A mount counts days from the base date and touches only its own two
    /// bytes, because the byte above it is somebody else's.
    #[test]
    fn a_mount_counts_whole_days_from_the_base_date() {
        let mut item = Item { index: HORSE, refine: 0x1234, ..Item::default() };
        stamp(&mut item, &table(), at(2026, 9, 1, 0));

        let days = (at(2026, 10, 1, 2) - base_datetime()).num_days();
        assert_eq!(item.expires_at, (days as u32) << 8, "the days are not where Time is");

        // A mount does not stack, so the count beside it is one byte wide and
        // the zero the day count leaves behind lands on nothing.
        let mut out = [0u8; 20];
        out[16..18].copy_from_slice(&1u16.to_le_bytes());
        write_into(&mut out, &item);

        assert_eq!(u16::from_le_bytes(out[18..20].try_into().unwrap()), days as u16);
        assert_eq!(u16::from_le_bytes(out[16..18].try_into().unwrap()), 1, "the count was eaten");
    }

    /// Everything else carries the unix time shifted right eight bits, and
    /// takes the byte above `Time` to fit the third byte in.
    #[test]
    fn everything_else_carries_three_bytes_of_unix_time() {
        let mut item = Item { index: SADDLE, refine: 0xABCD, ..Item::default() };
        let now = at(2026, 9, 1, 0);
        stamp(&mut item, &table(), now);

        let expected = expiry_of(now, 720).and_utc().timestamp() >> 8;
        assert_eq!(item.expires_at as i64, expected);

        let mut out = [0u8; 20];
        out[16..18].copy_from_slice(&item.refine.to_le_bytes());
        write_into(&mut out, &item);

        let read_back =
            u32::from_le_bytes([out[17], out[18], out[19], 0]) as i64;
        assert_eq!(read_back, expected, "the client would read a different moment");
        assert_eq!(out[16], 0xCD, "the low half of the count is not the expiry's to take");
        assert_ne!(out[17], 0xAB, "the expiry did not reach the byte it shares");
    }

    /// An item that does not run out is left alone, count and all.
    #[test]
    fn an_ordinary_item_is_not_stamped() {
        let mut item = Item { index: 1000, refine: 0xABCD, ..Item::default() };
        stamp(&mut item, &table(), at(2026, 9, 1, 0));
        assert_eq!(item.expires_at, 0);

        let mut out = [0u8; 20];
        out[16..18].copy_from_slice(&item.refine.to_le_bytes());
        write_into(&mut out, &item);
        assert_eq!(u16::from_le_bytes(out[16..18].try_into().unwrap()), 0xABCD);
    }
}
