//! Everything a player does with what they carry.
//!
//! Dragging a piece from one slot to another, wearing it, drinking it,
//! throwing it away, stacking two piles or splitting one, the action bar,
//! and the chest -- which is the same subject seen from the account rather
//! than the character. They came out of `game.rs` when it reached eight
//! thousand lines and stopped being a file anybody could read.
//!
//! What is *in* an inventory lives in [`crate::inventory`]; this is the
//! packets that move it about.

use super::*;
/// `0x70F`: an item dragged from one slot to another.
///
/// The chest makes this a move between two owners rather than inside one: the
/// bag belongs to the character and the chest to the account. Both sides are
/// checked the way `MoveItem` checks them — the page a slot is on has to be
/// unlocked, the item that unlocks a page cannot itself be dragged, only prans
/// go in the last two chest slots, and putting something into the chest needs
/// the chest window to be open.
pub(crate) fn handle_move_item(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = MoveItem::parse(&message.body) else {
        warn!(size = message.body.len(), "0x70F packet too short");
        return Action::Ignore;
    };
    let (from, to) = (request.from(), request.to());

    if session.character.is_none() || session.account.is_none() {
        return Action::Ignore;
    }
    if let Err(reason) = check_move(state, session, from, to) {
        debug!(?from, ?to, reason, "item not moved");
        return refresh_both(session, from, to);
    }

    let (Some(character), Some(account)) =
        (session.character.as_mut(), session.account.as_mut())
    else {
        return Action::Ignore;
    };
    let (bag, chest) = (&mut character.items, &mut account.storage);

    let moved = match (from.0, to.0) {
        (inventory::STORAGE, inventory::STORAGE) => chest.move_item(from, to),
        (inventory::STORAGE, _) => chest.move_into(from, bag, to),
        (_, inventory::STORAGE) => bag.move_into(from, chest, to),
        _ => bag.move_item(from, to),
    };
    let equipped = moved.is_ok() && (from.0 == inventory::EQUIP || to.0 == inventory::EQUIP);
    match moved {
        Ok(()) => session.dirty = true,
        Err(e) => debug!(?from, ?to, error = %e, "item not moved"),
    }

    let mut frames = match refresh_both(session, from, to) {
        Action::Reply(frames) => frames,
        other => return other,
    };
    if equipped {
        frames.extend(restat(state, session, from, to));

        // Slot ten is the companion, and it is the one slot where equipping
        // does more than change a number.
        let stone_slot = |side: (u8, u16)| {
            side.0 == inventory::EQUIP && side.1 == crate::pran::STONE_SLOT
        };
        if stone_slot(from) || stone_slot(to) {
            let summoned = super::pran_frames(state, session);
            if summoned.is_empty() {
                // The stone came off, so whatever it brought goes with it.
                frames.extend(super::dismiss_pran(session));
            } else {
                frames.extend(summoned);
            }
        }
    }
    Action::Reply(frames)
}

/// Says why a move is refused, or nothing when it is allowed.
///
/// Split out because the reasons are the interesting part and they are the
/// original's, not ours (`MoveItem`, `PacketHandlers.pas:5376`).
pub(crate) fn check_move(
    state: &State,
    session: &Session,
    from: (u8, u16),
    to: (u8, u16),
) -> Result<(), &'static str> {
    // A bag or a vault is the reason its page exists; it is not cargo.
    if inventory::is_page_item(from.0, from.1) || inventory::is_page_item(to.0, to.1) {
        return Err("that slot holds the thing that unlocks a page");
    }

    // Slot 0 is the body and slot 1 the hair. They are drawn from the
    // equipment array but they are not items, and the original refuses any
    // move that names them (`srcSlot > 1`, `destSlot > 1`).
    for side in [from, to] {
        if side.0 == inventory::EQUIP && side.1 < 2 {
            return Err("the body and the hair are not items");
        }
    }

    // Putting something in needs the window open. Taking something out does
    // not: the original has that check commented out on the source side, and
    // a player pulling from their own chest robs nobody.
    // Either window is the chest open: the Pran station is the same
    // eighty-six slots with a different face on them.
    let chest_open = matches!(session.opened_option, OPTION_STORAGE | OPTION_PRAN_STATION)
        && session.opened_npc.is_some();
    if to.0 == inventory::STORAGE && !chest_open {
        return Err("the chest is not open");
    }

    let (Some(character), Some(account)) = (session.character.as_ref(), session.account.as_ref())
    else {
        return Err("nobody is in the world");
    };
    let held = |side: (u8, u16)| {
        let owner = if side.0 == inventory::STORAGE { &account.storage } else { &character.items };
        owner.get(side.0, side.1).cloned()
    };

    // An item goes in the slot its type says and in no other. The original
    // works this out in `GetItemEquipSlot`, and the client normally respects
    // it; when it does not, the damage is real — a saddle dropped into the
    // mount slot is drawn as the mount, and the client hangs for ever trying
    // to draw a horse out of something that is not one.
    if to.0 == inventory::EQUIP {
        let Some(moving) = held(from) else {
            return Err("there is nothing to equip");
        };
        let item_type = state.items.get(moving.index as usize).map_or(0, |d| d.item_type());
        match inventory::equip_slot_for(item_type) {
            Some(slot) if slot == to.1 => {}
            Some(_) => return Err("that is not the slot this item goes in"),
            None => return Err("that is not equipment"),
        }
    }

    // A locked page is not a place to take from or put into.
    for side in [from, to] {
        if let Some(page) = inventory::page_item_for(side.0, side.1) {
            let owner =
                if side.0 == inventory::STORAGE { &account.storage } else { &character.items };
            if owner.get(side.0, page).is_none_or(|i| i.is_empty()) {
                return Err("that page has not been unlocked");
            }
        }
    }

    // And the last two chest slots hold prans and nothing else.
    if to.0 == inventory::STORAGE && inventory::STORAGE_PRAN_SLOTS.contains(&to.1) {
        let is_pran = held(from)
            .and_then(|item| state.items.get(item.index as usize))
            .is_some_and(|def| def.item_type() == ITEM_TYPE_PRAN);
        if !is_pran {
            return Err("only a pran goes in the pran slots");
        }
    }

    Ok(())
}

/// Sends both slots back as they really are.
///
/// The client has already drawn the item in its new place, so a refusal has to
/// say what is actually there, and a move has to clear the slot it came from.
pub(crate) fn refresh_both(session: &Session, from: (u8, u16), to: (u8, u16)) -> Action {
    let (Some(character), Some(account)) = (session.character.as_ref(), session.account.as_ref())
    else {
        return Action::Ignore;
    };
    let owner = |container: u8| {
        if container == inventory::STORAGE {
            &account.storage
        } else {
            &character.items
        }
    };

    Action::Reply(vec![
        encode_refresh_item(to.0, to.1, &slot_item(owner(to.0), to.0, to.1), false),
        encode_refresh_item(from.0, from.1, &slot_item(owner(from.0), from.0, from.1), false),
    ])
}

/// What the original sends after a piece of gear moves: the recomputed
/// numbers, and a fresh spawn when the piece is one that shows.
///
/// Without this the character sheet keeps whatever it was told on the way into
/// the world, so taking a sword off left the window still claiming its attack.
pub(crate) fn restat(
    state: &State,
    session: &mut Session,
    from: (u8, u16),
    to: (u8, u16),
) -> Vec<Vec<u8>> {
    let client_id = session.client_id;
    let Some(character) = session.character.as_ref() else {
        return Vec::new();
    };

    let (max_hp, max_mp) = vitals(character);
    session.cur_hp = session.cur_hp.min(max_hp);
    session.cur_mp = session.cur_mp.min(max_mp);
    let effects = session.effects(state);
    let character = session.character.as_ref().expect("checked above");

    let mut frames = vec![
        encode_refresh_status(character, &state.items, &effects),
        encode_refresh_point(character),
        encode_hp_mp(character, client_id, session.cur_hp, session.cur_mp),
    ];

    let shows = [from, to]
        .iter()
        .any(|side| side.0 == inventory::EQUIP && WORN_ON_THE_BODY.contains(&side.1));
    if shows {
        let spawn = encode_spawn(character, client_id, stats::of(character, &state.items, &effects).speed_move);
        state.world.send_to_visible(client_id, spawn.clone());
        frames.push(spawn);
    }
    frames
}

/// `0x332`: stack one pile onto another (`AgroupItem`).
///
/// Both slots are in the bag. The original merges when the two hold the same
/// item — it adds the source's count onto the destination and empties the
/// source — and does nothing otherwise. We add one guard the original leaves
/// to the client: the item has to be one that stacks at all, so dragging two
/// identical swords together cannot silently add their refine levels.
pub(crate) fn handle_group_item(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 8 {
        warn!(size = message.body.len(), "0x332 packet too short");
        return Action::Ignore;
    }
    let src = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as u16;
    let dest = u32::from_le_bytes(message.body[4..8].try_into().unwrap()) as u16;

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    let (Some(src_item), Some(dest_item)) = (
        character.items.get(inventory::BAG, src).cloned(),
        character.items.get(inventory::BAG, dest).cloned(),
    ) else {
        return Action::Ignore;
    };

    // Same item, and one that stacks. The original checks only the first;
    // the table check keeps refine levels of gear from being added up.
    let groups = state.items.get(src_item.index as usize).is_some_and(|d| d.can_group());
    if src_item.index == 0 || src_item.index != dest_item.index || !groups {
        return Action::Ignore;
    }

    let mut merged = dest_item;
    merged.refine = merged.refine.saturating_add(src_item.refine.max(1));
    let _ = character.items.put(merged.clone());
    let _ = character.items.take(inventory::BAG, src);
    session.dirty = true;

    Action::Reply(vec![
        encode_refresh_item(inventory::BAG, src, &slot_item(&character.items, inventory::BAG, src), false),
        encode_refresh_item(inventory::BAG, dest, &merged, false),
    ])
}

/// `0x333`: split a pile in two (`UngroupItem`).
///
/// Only the bag can be split, and only for a count smaller than the pile — you
/// cannot split off the whole thing, and an item that expires cannot be split
/// at all. The taken-off count goes into the first free slot; a full bag
/// refuses with the same message the original sends.
pub(crate) fn handle_ungroup_item(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 12 {
        warn!(size = message.body.len(), "0x333 packet too short");
        return Action::Ignore;
    }
    let slot = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as u16;
    let amount = u32::from_le_bytes(message.body[4..8].try_into().unwrap()) as u16;
    let slot_type = u32::from_le_bytes(message.body[8..12].try_into().unwrap()) as u8;

    // The original splits the bag only; equipment, storage and pran gear just
    // return without doing anything.
    if slot_type != inventory::BAG {
        return Action::Ignore;
    }

    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    let Some(source) = character.items.get(inventory::BAG, slot).cloned() else {
        return Action::Ignore;
    };
    // Nothing to split off, or a whole-stack "split", or an item that expires.
    let expires = state
        .items
        .get(source.index as usize)
        .is_some_and(|d| d.duration() != 0);
    if amount == 0 || amount >= source.refine || expires {
        return Action::Ignore;
    }

    let Some(free) = character.items.first_free(inventory::BAG) else {
        return Action::Reply(vec![encode_client_message(client_id, "Inventário cheio.")]);
    };

    let mut taken = source.clone();
    taken.slot = free;
    taken.refine = amount;
    let mut left = source;
    left.refine -= amount;

    let _ = character.items.put(left.clone());
    let _ = character.items.put(taken.clone());
    session.dirty = true;

    Action::Reply(vec![
        encode_refresh_item(inventory::BAG, slot, &left, false),
        encode_refresh_item(inventory::BAG, free, &taken, false),
    ])
}

/// `0x32C`: throw an item away.
pub(crate) fn handle_delete_item(session: &mut Session, message: &Message) -> Action {
    let Some(request) = DeleteItem::parse(&message.body) else {
        warn!(size = message.body.len(), "0x32C packet too short");
        return Action::Ignore;
    };
    let (container, slot) = (request.container as u8, request.slot as u16);

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    match character.items.take(container, slot) {
        Ok(gone) => {
            session.dirty = true;
            info!(item = gone.index, container, slot, "item thrown away");
            Action::Reply(vec![encode_refresh_item(container, slot, &Item::default(), true)])
        }
        Err(e) => {
            debug!(error = %e, "nothing to throw away");
            Action::Ignore
        }
    }
}

/// `0x31D`: use what is in a slot.
///
/// Only the potions are here. The original's `TItemFunctions.UseItem` is a
/// nine hundred line switch over every kind of item in the game, and the rest
/// of it needs systems that do not exist yet; a scroll that teleports you
/// needs teleporting. What every branch shares is the shape: check the level,
/// do the effect, take one off the stack, tell the client about the slot.
pub(crate) fn handle_use_item(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = UseItem::parse(&message.body) else {
        warn!(size = message.body.len(), "0x31D packet too short");
        return Action::Ignore;
    };
    let (container, slot) = (request.container as u8, request.slot as u16);

    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    let Some(item) = character.items.get(container, slot).cloned() else {
        debug!(container, slot, "0x31D on an empty slot");
        return Action::Ignore;
    };

    let Some(def) = state.items.get(item.index as usize) else {
        debug!(item = item.index, "0x31D on an item that is not in the table");
        return Action::Ignore;
    };

    if character.level < def.level() {
        return Action::Reply(vec![encode_client_message(
            client_id,
            &format!("You need level {} to use that.", def.level()),
        )]);
    }

    let effect = def.use_effect() as u32;
    let (heals_hp, heals_mp) = match def.item_type() {
        ITEM_TYPE_HP_POTION => (true, false),
        ITEM_TYPE_MP_POTION => (false, true),
        ITEM_TYPE_HPMP_POTION | ITEM_TYPE_TEARS => (true, true),
        // The chest opens wherever the player is standing, and the item is
        // not spent doing it — the original returns before the stack is
        // touched. It names the player as the NPC whose window is open,
        // because there is no NPC.
        // A lasting potion starts a buff and is spent doing it, unlike the
        // saddle. The item names the skill; the skill is the buff.
        ITEM_TYPE_POTION_BUFF => {
            let frames = grant_buff(state, session, def.use_effect() as usize);
            if frames.is_empty() {
                return Action::Reply(vec![encode_client_message(
                    client_id,
                    "That cannot be used yet.",
                )]);
            }
            let mut frames = frames;
            frames.extend(spend_one(session, container, slot, &item));
            return Action::Reply(frames);
        }
        ITEM_TYPE_STORAGE_OPEN => {
            session.opened_option = OPTION_STORAGE;
            session.opened_npc = Some(client_id);
            let Some(account) = session.account.as_ref() else {
                return Action::Ignore;
            };
            info!(item = item.index, "the chest was opened with an item");
            return Action::Reply(open_storage(
                client_id,
                account.storage_gold,
                &account.storage,
                STORAGE_TYPE_PLAYER,
            ));
        }
        other => {
            debug!(item = item.index, item_type = other, "this kind of item is not usable yet");
            return Action::Reply(vec![encode_client_message(
                client_id,
                "That cannot be used yet.",
            )]);
        }
    };

    let (max_hp, max_mp) = vitals(character);
    if heals_hp {
        session_heal(&mut session.cur_hp, effect, max_hp);
    }
    if heals_mp {
        session_heal(&mut session.cur_mp, effect, max_mp);
    }

    let mut frames = spend_one(session, container, slot, &item);
    let character = session.character.as_ref().expect("checked above");
    frames.insert(
        0,
        encode_hp_mp(character, session.client_id, session.cur_hp, session.cur_mp),
    );
    Action::Reply(frames)
}

/// Takes one off a stack and says what the slot holds afterwards.
///
/// The slot goes empty when the last one is used, and either way the client
/// is told, because it has already drawn the item as gone.
pub(crate) fn spend_one(session: &mut Session, container: u8, slot: u16, item: &Item) -> Vec<Vec<u8>> {
    let Some(character) = session.character.as_mut() else {
        return Vec::new();
    };
    let left = item.refine.saturating_sub(1);
    let remaining = if left == 0 {
        let _ = character.items.take(container, slot);
        Item { container, slot, ..Item::default() }
    } else {
        let mut kept = item.clone();
        kept.refine = left;
        let _ = character.items.put(kept.clone());
        kept
    };

    session.dirty = true;
    info!(item = item.index, container, slot, left, "item used");
    vec![encode_refresh_item(container, slot, &remaining, false)]
}

/// Raises a pool without letting it past its ceiling.
pub(crate) fn session_heal(current: &mut u32, by: u32, ceiling: u32) {
    *current = current.saturating_add(by).min(ceiling);
}

/// `0x31E`: the player rearranged the action bar (`ChangeItemBar`).
///
/// The three dwords are the slot that changed, what kind of thing was dropped
/// on it, and that thing's id. The original stores an encoded value per kind:
/// a skill becomes `id * 16 + 2`, a usable item is kept as its id, and a drop
/// of nothing clears the slot; kinds it keeps on the pran rather than the
/// character (1 and 3) leave the character's bar untouched. It then echoes the
/// packet straight back, which is how the client confirms the change.
pub(crate) fn handle_change_item_bar(
    state: &State,
    session: &mut Session,
    message: &Message,
) -> Action {
    if message.body.len() < 12 {
        warn!(size = message.body.len(), "0x31E packet too short");
        return Action::Ignore;
    }
    let dest = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as usize;
    let kind = u32::from_le_bytes(message.body[4..8].try_into().unwrap());
    let src = u32::from_le_bytes(message.body[8..12].try_into().unwrap());

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    if dest >= character.item_bar.len() {
        warn!(dest, "0x31E slot out of range");
        return Action::Ignore;
    }

    match kind {
        // Cleared, a skill, or a usable item — the three the original keeps on
        // the character's own bar (`ChangeItemBar`).
        0 => character.item_bar[dest] = 0,
        2 => character.item_bar[dest] = ability::on_bar(src as usize),
        6 => character.item_bar[dest] = src,
        // Three is the *companion's* bar, which is a different bar with three
        // slots of its own, and it is the only way a pran skill is ever cast.
        PRAN_BAR => return put_on_the_pran_bar(state, session, dest, src),
        other => {
            debug!(kind = other, "item-bar change for a kind the original refuses");
            return Action::Ignore;
        }
    }
    session.dirty = true;

    // The confirmation the client waits for is the same packet back.
    let echo = frame::encode(
        &Message {
            sender: session.client_id,
            opcode: OP_CHANGE_ITEM_BAR,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    Action::Reply(vec![echo])
}

/// The kind that names the companion's bar rather than the player's
/// (`ChangeItemBar`, `$3`).
const PRAN_BAR: u32 = 3;

/// `0x31E` with kind three: a skill dragged onto the companion's own bar.
///
/// A different bar from the player's, three slots wide, kept on the pran and
/// not on the character -- and the only way a pran skill is ever cast, which
/// is why a companion whose bar was never stored could not use one of the ten
/// skills it had been growing all along.
///
/// What travels is not the skill id but the id counted from its element's
/// base, so the fourth skill is 31 whether the companion is fire, water or
/// air. The original validates it against `SkillData[SrcIndex + 5760]` --
/// the fire base, whatever the element -- and gets away with it because the
/// three elements mirror each other slot for slot.
///
/// # Only five of the ten may go there
///
/// `if (SkillData[SrcIndex + 5760].Duration = 0) and (SrcIndex <> 0) then Exit`.
/// Five of a pran's skills carry a duration and five do not, and the five that
/// do not are the passives, which work by being known rather than by being
/// used. Nought is let through because that is the clear.
fn put_on_the_pran_bar(
    state: &State,
    session: &mut Session,
    dest: usize,
    src: u32,
) -> Action {
    if dest >= pran::BAR_SLOTS {
        warn!(dest, "0x31E for a companion bar slot that does not exist");
        return Action::Ignore;
    }
    let Some(at) = session.pran_out else {
        debug!("0x31E for a companion bar with no companion out");
        return Action::Ignore;
    };

    // A passive has no duration and cannot be put on a bar. Clearing the slot
    // is the one thing that skips the test.
    if src != 0 {
        let lasts = state
            .skills
            .get((src + pran::Element::Fire.skill_base()) as usize)
            .is_some_and(|skill| skill.duration_secs() != 0);
        if !lasts {
            debug!(src, "a companion skill with no duration cannot go on a bar");
            return Action::Ignore;
        }
    }

    let Some(account) = session.account.as_mut() else {
        return Action::Ignore;
    };
    let Some(pran) = account.prans.get_mut(at) else {
        return Action::Ignore;
    };
    let Ok(value) = u8::try_from(src) else {
        warn!(src, "0x31E companion bar entry out of range");
        return Action::Ignore;
    };
    pran.bar[dest] = value;
    // The companions are written wherever the chest is, and that is this flag.
    session.dirty = true;
    info!(slot = dest, skill = src, "companion bar set");

    Action::Reply(vec![encode_item_bar_slot(dest, PRAN_BAR, src)])
}

/// `RefreshItemBarSlot` (`Mob/Player.pas:4387`): the server changed a hotbar
/// slot on its own.
///
/// The same `0x31E` the client sends, sent the other way, and the one field
/// that differs is the header: the original puts the fixed `0x7535` there
/// rather than the client id, the way it does for every packet that is the
/// server talking about the character rather than the character acting.
///
/// The id goes in raw. It is the *packet* that carries the kind separately;
/// the `id * 16 + 2` packing is only how the value is kept in the record.
pub(crate) fn encode_item_bar_slot(slot: usize, kind: u32, id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&(slot as u32).to_le_bytes());
    body.extend_from_slice(&kind.to_le_bytes());
    body.extend_from_slice(&id.to_le_bytes());
    frame::encode(
        &Message {
            sender: dialog::FIXED_INDEX,
            opcode: OP_CHANGE_ITEM_BAR,
            time: PACKET_TIME,
            body,
        },
        rand::random(),
    )
}

/// `TStoragePacket` (`0x137`): the chest, gold and every slot of it.
///
/// The original copies the whole `TStoragePlayer` into the packet in one
/// `Move`, so the layout is exactly the record's: the gold, then eighty-six
/// twenty-byte items. Empty slots go out as zeroes, which is what tells the
/// client the space is there but nothing is in it.
pub(crate) fn encode_storage(client_id: u16, gold: u64, storage: &Inventory) -> Vec<u8> {
    let mut body = vec![0u8; STORAGE_SIZE - MIN_FRAME];
    body[4..12].copy_from_slice(&gold.min(GOLD_CAP).to_le_bytes());

    for slot in 0..inventory::STORAGE_SLOTS {
        if let Some(item) = storage.get(inventory::STORAGE, slot) {
            let at = 12 + slot as usize * character_offset::ITEM_SIZE;
            write_item(&mut body[at..at + character_offset::ITEM_SIZE], item);
        }
    }

    debug_assert_eq!(body.len() + MIN_FRAME, STORAGE_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_STORAGE, time: 0, body },
        rand::random(),
    )
}

/// Everything `SendStorage` sends: the chest, the signal that opens its
/// window, and the two pran slots on their own.
///
/// The two extra refreshes are the original's and look redundant until you
/// notice which slots they name — 84 and 85 sit past the four pages, and the
/// client draws the pran centre from a different part of its interface.
pub(crate) fn open_storage(
    client_id: u16,
    gold: u64,
    storage: &Inventory,
    storage_type: u32,
) -> Vec<Vec<u8>> {
    let mut frames = vec![
        encode_storage(client_id, gold, storage),
        encode_signal(OP_STORAGE_OPEN, client_id, 0, storage_type),
    ];
    for slot in inventory::STORAGE_PRAN_SLOTS {
        let item = slot_item(storage, inventory::STORAGE, slot);
        frames.push(encode_refresh_item(inventory::STORAGE, slot, &item, false));
    }
    frames
}

/// `0xF59`: move gold between the purse and the chest (`ChangeGold`).
///
/// The amount is signed: positive puts gold in, negative takes it out. Both
/// sides are checked against the two-billion cap the original stops at rather
/// than letting either wrap, and a transfer of nothing is refused outright so
/// a stuck client cannot make the chest redraw for ever.
pub(crate) fn handle_chest_gold(session: &mut Session, message: &Message) -> Action {
    if message.body.len() < CHEST_GOLD_SIZE - MIN_FRAME {
        warn!(size = message.body.len(), "0xF59 packet too short");
        return Action::Ignore;
    }
    let chest = u32::from_le_bytes(message.body[0..4].try_into().unwrap());
    let value = i64::from_le_bytes(message.body[4..12].try_into().unwrap());

    // The guild chest is the other half of this packet and waits on guilds.
    if chest != CHEST_TYPE_STORAGE || value == 0 {
        debug!(chest, value, "0xF59 for a chest we do not keep");
        return Action::Ignore;
    }

    let client_id = session.client_id;
    let (Some(character), Some(account)) =
        (session.character.as_mut(), session.account.as_mut())
    else {
        return Action::Ignore;
    };

    let amount = value.unsigned_abs();
    let (from, to) = if value > 0 {
        (&mut character.gold, &mut account.storage_gold)
    } else {
        (&mut account.storage_gold, &mut character.gold)
    };
    if *from < amount || *to + amount > GOLD_CAP {
        debug!(value, "not enough gold, or the other side is full");
        return Action::Ignore;
    }
    *from -= amount;
    *to += amount;

    let (gold, chest_gold) = (character.gold, account.storage_gold);
    let storage = account.storage.clone();
    session.dirty = true;
    info!(value, gold, chest_gold, "gold moved to or from the chest");

    let mut frames = vec![encode_refresh_money(gold, chest_gold)];
    frames.extend(open_storage(client_id, chest_gold, &storage, STORAGE_TYPE_PLAYER));
    Action::Reply(frames)
}

pub(crate) fn slot_item(inventory: &Inventory, container: u8, slot: u16) -> Item {
    inventory
        .get(container, slot)
        .cloned()
        .unwrap_or(Item { container, slot, ..Item::default() })
}

/// What is in a slot, or an empty item addressed to it when nothing is.
/// What the account keeps in the chest, or nothing when there is no account
/// yet. Every packet that shows the purse shows this beside it.
pub(crate) fn chest_gold(session: &Session) -> u64 {
    session.account.as_ref().map_or(0, |account| account.storage_gold)
}
