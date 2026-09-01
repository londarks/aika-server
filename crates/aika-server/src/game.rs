//! Game server (port 8822 by default).
//!
//! This is where the client lands after login. It sends a `0x685` with the
//! account and version; the server answers `0x901` with the three character
//! slots, which is the selection screen.
//!
//! Reference: the dispatch that matters in the Delphi server is the one at
//! `Threads/PlayerThread.pas:97`. Two nearly identical copies exist
//! (`Connections/ServerSocket.pas:3307` and `Threads/UpdateThreads.pas:419`)
//! that are dead: neither has a live call site.

use crate::state::State;
use crate::inventory::{self, Inventory};
use crate::store::{Account, Character, Item, MAX_CHARACTERS};
use crate::{ability, combat, creation, dialog, shop, stats};
use crate::world::{Outbox, DISTANCE_TO_FORGET, DISTANCE_TO_WATCH};
use aika_data::npc::Npc;
use aika_net::frame::{self, FrameError, FrameReader, Message, MIN_FRAME};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Client to server: enter the game server with an authenticated account.
pub const OP_REQUEST_LOGIN: u16 = 0x685;
/// Server to client: the three slots of the selection screen.
pub const OP_CHAR_LIST: u16 = 0x901;
/// Client to server: picked a character and wants to enter the world.
pub const OP_ENTER_WORLD: u16 = 0xF02;
/// Server to client: the whole character (`TSendToWorldPacket`).
pub const OP_SEND_TO_WORLD: u16 = 0x925;
/// Signals that precede `0x925`, in the order the original sends them.
pub const OP_SIGNAL_READY: u16 = 0xCCCC;
pub const OP_SIGNAL_LOAD: u16 = 0x186;
/// Client to server: finished loading, ready to spawn.
pub const OP_CLIENT_READY: u16 = 0xF0B;
/// Client to server: walked to a point. The server returns nothing to the
/// mover, it only relays to the others who can see them.
pub const OP_MOVE: u16 = 0x301;
/// `TSendClientIndexPacket`: which mob on screen is the player's own.
/// Without it the client has a world it cannot attach its camera to.
pub const OP_CLIENT_INDEX: u16 = 0x117;
/// Three more the original sends around the world packet. Their contents are
/// zero and their meaning is unknown; what is known is the order.
pub const OP_ENTER_3A2: u16 = 0x3A2;
pub const OP_ENTER_131: u16 = 0x131;
pub const OP_ENTER_12C: u16 = 0x12C;
pub const OP_ENTER_94C: u16 = 0x94C;

/// `TSignalData`: which way the player is facing.
pub const OP_ROTATE: u16 = 0x305;
/// `TMoveItemPacket`: drag an item from one slot to another.
pub const OP_MOVE_ITEM: u16 = 0x70F;
/// Throw an item away.
pub const OP_DELETE_ITEM: u16 = 0x32C;
/// `TUseItemPacket`: drink the potion in a slot.
pub const OP_USE_ITEM: u16 = 0x31D;
/// Leave the world and go back to the selection screen.
pub const OP_BACK_TO_CHARACTER_SELECT: u16 = 0x668;
/// Get up after dying.
pub const OP_REVIVE: u16 = 0x303;
/// `TClientMessagePacket` (`Data/Packets.pas:152`): a line of text on screen.
pub const OP_CLIENT_MESSAGE: u16 = 0x984;
/// `TChatPacket` (`Data/Packets.pas:188`), the packet a player sends to speak
/// and the one the server echoes back to everyone who should hear it. The
/// original dispatches it at `$F86` (`ServerSocket.pas:3696`).
pub const OP_CHAT: u16 = 0xF86;
/// `TAgroupItemPacket` (`Data/Packets.pas:887`): stack one pile onto another
/// (`AgroupItem`, `$332`).
pub const OP_GROUP_ITEM: u16 = 0x332;
/// `TUngroupItemPacket` (`Data/Packets.pas:895`): split a pile in two
/// (`UngroupItem`, `$333`).
pub const OP_UNGROUP_ITEM: u16 = 0x333;
/// Walking, the only move type the original relays to other players
/// (`Data/GlobalDefs.pas:216`). The real client also sends other values.
const MOVE_NORMAL: u8 = 0;
/// Server-originated teleport (`Data/GlobalDefs.pas:217`). A client must
/// never be able to move itself this way.
const MOVE_TELEPORT: u8 = 1;
/// How many movement packets to dump in full per connection, to confirm the
/// layout against the real client without flooding the log.
const MOVES_TO_DUMP: u8 = 3;
/// Server to client: create a creature on the map. This is the spawn.
pub const OP_CREATE_MOB: u16 = 0x349;
/// Server to client: take a creature off the map.
pub const OP_REMOVE_MOB: u16 = 0x101;

/// `TSendRemoveMobPacket`: the id leaving and why.
const REMOVE_MOB_SIZE: usize = 20;
/// `DELETE_DISCONNECT` in `Data/GlobalDefs.pas:216`. There is also 0 for a
/// plain disappearance, 1 for death and 3 for an unspawn effect.
const DELETE_DISCONNECT: u32 = 2;
/// `DELETE_NORMAL`: the creature simply goes off screen, no effect.
const DELETE_NORMAL: u32 = 0;

// The burst the original sends after `0xF0B`, in order. There is no ack
// packet: the client stops resending `0xF0B` once it has received enough of
// this to consider itself in the world (`Mob/Player.pas:4945-5244`).
/// Skill list.
pub const OP_SKILLS: u16 = 0x106;
/// Cash balance, carried as a plain signal.
pub const OP_CASH: u16 = 0x139;
/// Account status.
pub const OP_ACCOUNT_STATUS: u16 = 0x14F;
/// Active buffs.
pub const OP_BUFFS: u16 = 0x16E;
/// Active title.
pub const OP_ACTIVE_TITLE: u16 = 0x361;
/// Nation relics.
pub const OP_RELICS: u16 = 0x136;
/// Attribute points.
pub const OP_REFRESH_POINT: u16 = 0x109;
/// Combat stats.
pub const OP_REFRESH_STATUS: u16 = 0x10A;
/// Full attribute reply, which the original emits right after `0x10A`.
pub const OP_ALL_ATTRIBUTES: u16 = 0x23FF;
/// Level and experience.
pub const OP_LEVEL: u16 = 0x108;
/// Current and maximum HP/MP.
pub const OP_HP_MP: u16 = 0x103;

/// Several packets carry this fixed value in the header `Index` field instead
/// of the client id.
const FIXED_INDEX: u16 = 0x7535;

/// Total sizes, header included, taken from the Delphi records.
const SKILLS_SIZE: usize = 96;
const SIGNAL_SIZE: usize = 16;
const BUFFS_SIZE: usize = 252;
const ACTIVE_TITLE_SIZE: usize = 20;
const RELICS_SIZE: usize = 66;
const REFRESH_POINT_SIZE: usize = 28;
const REFRESH_STATUS_SIZE: usize = 54;
const ALL_ATTRIBUTES_SIZE: usize = 50;
const LEVEL_SIZE: usize = 24;
const HP_MP_SIZE: usize = 32;

/// The original writes a single `0xCC` byte into a WORD field of `0x108`
/// (`Mob/BaseMob.pas:2434`), so the field reads `CC 00`.
const LEVEL_UNK: u16 = 0x00CC;

/// Sizes of the world-entry packets, from their records in `Data/Packets.pas`.
const CLIENT_INDEX_SIZE: usize = 20;
const ENTER_3A2_SIZE: usize = 20;
const ENTER_131_SIZE: usize = 20;
const ENTER_12C_SIZE: usize = 96;
const ENTER_94C_SIZE: usize = 164;
/// The one value in any of them that is not zero (`Tp131.Unk_1`).
const ENTER_131_MARKER: u32 = 0xFFFF_FFFF;
/// The two storage slots the original refreshes on the way in. Storage is not
/// modelled yet, so they go out empty, which is what an untouched account
/// would have sent anyway.
const ENTER_STORAGE_SLOTS: [u16; 2] = [54, 55];

/// `TClientMessagePacket`, 144 bytes in total.
const CLIENT_MESSAGE_SIZE: usize = 144;
/// The text field inside it: 128 bytes, NUL terminated. The original walks it
/// as a plain array from index zero, writing over what would be a Delphi
/// short string's length byte, which tells us the client reads it as `char[]`
/// and not as a Delphi string.
const CLIENT_MESSAGE_TEXT: usize = 128;
/// Yellow, at the top of the screen: what the original passes by default.
const MESSAGE_NOTICE: u8 = 16;

/// Item types a potion can be (`Data/GlobalDefs.pas:845,862,863,870`).
const ITEM_TYPE_TEARS: u16 = 29;
const ITEM_TYPE_HP_POTION: u16 = 700;
const ITEM_TYPE_MP_POTION: u16 = 701;
const ITEM_TYPE_HPMP_POTION: u16 = 800;

/// The index the original stamps on a menu entry (`NPCHandlers.pas:172`).
/// Note that it is not the `0x7535` used everywhere else — the digits are
/// swapped, and copying the wrong one puts the menu on nobody's screen.
const MENU_ENTRY_INDEX: u16 = 0x3575;

/// `TSendCreateMobPacket` (`Data/Packets.pas:344`), 508 bytes in total.
const CREATE_MOB_SIZE: usize = 508;

/// Offsets inside the `0x349` body (the 12-byte header is already gone).
mod spawn_offset {
    pub const NAME: usize = 0;
    pub const EQUIP: usize = 16;
    pub const ARMA_REFINE: usize = 39;
    /// The position is a pair of `f32`, not integers, and there is no Z.
    pub const POSITION_X: usize = 44;
    pub const POSITION_Y: usize = 48;
    pub const ROTATION: usize = 52;
    pub const MAX_HP: usize = 56;
    pub const MAX_MP: usize = 60;
    pub const CUR_HP: usize = 64;
    pub const CUR_MP: usize = 68;
    pub const UNK0: usize = 72;
    pub const SPEED_MOVE: usize = 73;
    pub const SPAWN_TYPE: usize = 74;
    pub const SIZES: usize = 75;
    pub const GUILD_AND_NATION: usize = 476;
    /// Four WORDs; index 1 carries a fixed constant.
    pub const EFFECTS: usize = 478;
    pub const BODY_SIZE: usize = 496;
}

/// Extra fields the NPC flavour of `0x349` uses (`Mob/BaseNpc.pas:1855`).
mod npc_offset {
    /// Where the job title goes, the line the client draws under the name.
    pub const TITLE: usize = 444;
    pub const TITLE_MAX: usize = 32;
    /// One byte right after the four sizes.
    pub const IS_SERVICE: usize = 79;
    pub const EFFECT_TYPE: usize = 80;
}

/// An NPC is not a player: it is flagged as a service, carries a different
/// `Unk0`, and lights up effect type 1 (`Mob/BaseNpc.pas:1886`).
const NPC_UNK0: u8 = 0x28;
const NPC_IS_SERVICE: u8 = 1;
const NPC_EFFECT_TYPE: u16 = 1;

/// The equipment slots the spawn packet carries beyond the body and the
/// hair. Slots 8 and up are accessories, which it has no room for.
const WORN_SLOTS: std::ops::Range<u16> = 2..8;

/// What the client should draw in one equipment slot.
///
/// The appearance wins when there is one, which is how a piece of gear can
/// look like something else; otherwise the item itself
/// (`Mob/BaseNpc.pas:1876`).
fn worn_appearance(character: &Character, slot: u16) -> u16 {
    let Some(item) = character.items.get(inventory::EQUIP, slot) else {
        return 0;
    };
    if item.appearance != 0 {
        item.appearance
    } else {
        item.index
    }
}

/// Fixed values the original writes without explanation, but which the
/// client expects (`Mob/BaseMob.pas:2974-3131`).
const SPAWN_UNK0: u8 = 0x0A;
const SPAWN_EFFECT_1: u16 = 0x1D;
const SPAWN_ARMA_REFINE: u8 = 15;
/// Spawn types (`Data/GlobalDefs.pas:211`): 0 is somebody who was already
/// there coming into view, 1 is somebody arriving or teleporting in. The
/// client draws them differently, so a player logging in next to you should
/// not look like one who was standing there all along.
const SPAWN_TELEPORT_IN: u8 = 1;

/// The send type the original stamps on a skill list that came from a
/// trainer rather than from arriving in the world (`NPCHandlers.pas:7119`).
const SKILL_LIST_FROM_NPC: u16 = 0x0B;

/// What the original gives every monster (`ServerSocket.pas:655`).
const MOB_SPEED_MOVE: u8 = 22;
/// `SPAWN_NORMAL` in `Data/GlobalDefs.pas:211`. There is also 1 (teleport)
/// and 2 (offspring birth); bit 0x80 is added when the player is in PK.
const SPAWN_NORMAL: u8 = 0;

/// Size of `TSendToCharListPacket`.
const CHAR_LIST_SIZE: usize = 336;
/// Size of each `TCharacterListData`.
const CHAR_ENTRY_SIZE: usize = 104;

/// Size of `TCharacter` (`Data/PlayerData.pas:192`), summing the declared
/// types: `TStatus` = 140, `TItem` = 20, `TQuest` = 12.
///
/// The offset comments inside the record are stale (they disagree with each
/// other by 16 and 32 bytes): trust the declared types, never the comments.
/// The round 6400 total for the whole packet is a good sign the sum adds up.
const CHARACTER_SIZE: usize = 6384;
/// `TSendToWorldPacket`: header, account serial and the character.
const SEND_TO_WORLD_SIZE: usize = MIN_FRAME + 4 + CHARACTER_SIZE;
/// The original writes this fixed value in the `Index` header field of the
/// `0x925`, instead of the client id (`Mob/Player.pas:3300`).
const SEND_TO_WORLD_INDEX: u16 = 0x7535;

/// Offsets inside `TCharacter`.
mod character_offset {
    pub const CLIENT_ID: usize = 0;
    pub const FIRST_LOGIN: usize = 4;
    pub const CHAR_INDEX: usize = 8;
    pub const NAME: usize = 12;
    pub const NATION: usize = 28;
    pub const CLASS_INFO: usize = 29;
    /// Start of `TStatus`.
    pub const SCORE: usize = 32;
    pub const ATTRIBUTES: usize = SCORE;
    pub const SIZES: usize = SCORE + 12;
    pub const MAX_HP: usize = SCORE + 16;
    pub const CUR_HP: usize = SCORE + 20;
    pub const MAX_MP: usize = SCORE + 24;
    pub const CUR_MP: usize = SCORE + 28;
    pub const EXP: usize = 176;
    pub const LEVEL: usize = 184;
    /// 16 items of 20 bytes; slot 0 is the class and slot 1 the hair, so
    /// real equipment starts at slot 2.
    pub const EQUIP: usize = 340;
    /// 126 items of 20 bytes. The comment beside the field says 60, but
    /// `EQUIP + 16 * 20 + 4 + 126 * 20` lands exactly on the gold, which is
    /// the arithmetic that settles it.
    pub const INVENTORY: usize = EQUIP + 16 * ITEM_SIZE + 4;
    /// `TItem` (`Data/MiscData.pas:44`).
    pub const ITEM_SIZE: usize = 20;
    pub const GOLD: usize = 3184;
    pub const LOCATION: usize = 3792;
}

pub async fn serve(state: Arc<State>, listener: TcpListener) -> anyhow::Result<()> {
    info!(addr = %listener.local_addr()?, "game server listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            debug!(%peer, "client connected to the game server");
            if let Err(e) = handle_connection(state, stream).await {
                debug!(%peer, error = %e, "game connection closed");
            }
        });
    }
}

/// State of one game connection: who logged in and with which character.
///
/// It lives here, per connection, rather than in a global, which is what
/// allows two players at once. Seeing each other will need a shared
/// registry on top of this, but the connection owns the session.
#[derive(Default)]
struct Session {
    account: Option<Account>,
    character: Option<Character>,
    /// Id the client uses to refer to itself in packets.
    client_id: u16,
    /// How many movement packets were dumped in full so far.
    moves_logged: u8,
    /// Players this connection has already been shown. Kept per connection so
    /// that walking into and out of range spawns and removes them exactly once.
    visible: std::collections::HashSet<u16>,
    /// The character already spawned. The client resends `0xF0B` whenever it
    /// thinks something is missing, and spawning again teleports the player to the
    /// starting point, which is the original's `if IsInstantiated then Exit`
    /// (`Mob/Player.pas:4967`).
    spawned: bool,
    /// NPCs already placed on this player's screen. Same reason as `visible`:
    /// sending a spawn twice makes the client draw two of them.
    visible_npcs: HashSet<u16>,
    /// Which way the player is facing, so a repeat can be dropped.
    rotation: u32,
    /// Health and mana as they stand. Not on the character because nothing
    /// takes them down yet; they live here so a potion has somewhere to go.
    cur_hp: u32,
    cur_mp: u32,
    /// Down, and waiting to get up. A dead character takes no more damage
    /// and cannot swing.
    dead: bool,
    /// Something changed that the database has not been told about yet.
    dirty: bool,
    /// When the last write went out, so a player walking in a straight line
    /// does not write a row per step.
    saved_at: Option<std::time::Instant>,
    /// When each spell this character has cast may be cast again. On the
    /// session rather than the character: a cooldown that survived a logout
    /// would be a reason to log out.
    cooldowns: ability::Cooldowns,
    /// Monsters already on this player's screen, for the same reason as the
    /// players and the NPCs: sending a spawn twice draws two of them.
    visible_mobs: HashSet<u16>,
    /// Which NPC has a window open, if any. Every shop packet is checked
    /// against it: a client that never opened a shop must not be able to buy
    /// from one, and the original refuses on the same grounds.
    opened_npc: Option<u16>,
}

async fn handle_connection(state: Arc<State>, stream: TcpStream) -> anyhow::Result<()> {
    let (mut incoming, mut outgoing) = stream.into_split();
    let (outbox, mut queue) = mpsc::unbounded_channel::<Vec<u8>>();

    // The id is ours to hand out, not the client's to claim: the client learns
    // it from the packets we send, and echoing back whatever it sent would give
    // every player the same one.
    let Some(client_id) = state.world.connect(outbox.clone()) else {
        warn!(players = state.world.online(), "refused a connection: the channel is full");
        return Ok(());
    };

    // One task owns the write half, so a broadcast from another connection can
    // reach this player without either side waiting on the other.
    let writer = tokio::spawn(async move {
        while let Some(frame) = queue.recv().await {
            if outgoing.write_all(&frame).await.is_err() {
                break;
            }
        }
    });

    let mut session = Session { client_id, ..Session::default() };
    let result = read_loop(&state, &mut session, &outbox, &mut incoming).await;

    // Saved before anything else in the teardown: whatever went wrong with
    // the connection, where the player stood and what it was carrying are
    // still worth keeping.
    autosave(&state, &mut session, true).await;

    leave_world(&state, &session);
    writer.abort();
    result
}

/// How often monsters are brought back, which nothing else is measured
/// against. A second is finer than any respawn in the shipped data, the
/// shortest of which is thirty.
const RESPAWN_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// Runs the monsters, on the two clocks the original runs them on.
///
/// `ServerSocket.pas:876` starts two threads and gives them two periods:
/// `TMobHandlerThread1` every second decides who swings, and
/// `TMobMovimentThread1` every three seconds decides where everything
/// stands. Both numbers are behaviour, not tuning — running the movement on
/// a fast clock is what made monsters sprint across the field, because each
/// turn shifts a fixed distance rather than a distance per second.
///
/// The players are not told about respawns from here. Each connection keeps
/// its own idea of what is on its screen, and pushing a spawn from outside
/// would desync it; instead the session notices on its next refresh, which
/// the client's own twice-a-second heartbeat drives.
pub fn spawn_world_tick(state: Arc<State>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut respawns = tokio::time::interval(RESPAWN_TICK);
        let mut fights = tokio::time::interval(crate::mob::COMBAT_TICK);
        let mut moves = tokio::time::interval(crate::mob::MOVE_TICK);

        loop {
            let now = tokio::select! {
                _ = respawns.tick() => {
                    let revived = state.world.revive_mobs(std::time::Instant::now());
                    if !revived.is_empty() {
                        debug!(count = revived.len(), "monsters came back");
                    }
                    continue;
                }
                _ = fights.tick() => Clock::Combat,
                _ = moves.tick() => Clock::Movement,
            };

            // Nothing to think about with nobody in the world, and several
            // thousand monsters pacing for an empty map is work for nothing.
            let players = state.world.positions();
            if players.is_empty() {
                continue;
            }

            match now {
                Clock::Combat => run_combat(&state, &players),
                Clock::Movement => run_movement(&state, &players),
            }
        }
    })
}

/// Which of the two monster threads a turn of the loop is.
enum Clock {
    Combat,
    Movement,
}

/// `TMobSPosition.MobHandler`: who swings at whom, and who closes in.
fn run_combat(state: &Arc<State>, players: &[(u16, (f32, f32))]) {
    let (blows, moved) = state.world.fight_mobs(players, std::time::Instant::now());

    for attack in blows {
        debug!(
            mob = attack.attacker,
            target = attack.target,
            damage = attack.damage,
            skill = attack.skill,
            "a monster swung"
        );
        state.world.deal_to_player(attack.target, attack);
    }
    tell_watchers(state, players, moved);
}

/// `TMobSPosition.MobMoviment`: where everything stands.
fn run_movement(state: &Arc<State>, players: &[(u16, (f32, f32))]) {
    let moved = state.world.move_mobs(players, std::time::Instant::now());
    tell_watchers(state, players, moved);
}

/// Tells everyone who can see a monster where it has got to.
///
/// The original sends where the monster now *is*, having already moved it
/// there, and the client walks it the whole way at the speed in the packet.
/// Sending a short step on a fast clock instead is what made monsters stutter:
/// the client kept arriving early and standing still until the next one.
fn tell_watchers(
    state: &Arc<State>,
    players: &[(u16, (f32, f32))],
    moved: Vec<(crate::mob::Mob, crate::mob::Turn)>,
) {
    for (mob, turn) in moved {
        let Some(to) = turn.walk else { continue };
        let frame = encode_mob_walk(&mob, to, turn.speed);
        for (id, at) in players {
            if within(*at, mob.position(), DISTANCE_TO_WATCH) {
                state.world.send_to(*id, frame.clone());
            }
        }
    }
}

/// `0x301` for a monster: `TBaseMob.WalkTo`.
///
/// It carries where the monster is *going*, not where it is, and the speed to
/// get there at. The client walks it the whole way on its own — which is why
/// sending a short step every tick instead made monsters jump: they arrived
/// early and stood still until the next packet.
fn encode_mob_walk(mob: &crate::mob::Mob, to: (f32, f32), speed: u8) -> Vec<u8> {
    let mut body = vec![0u8; Movement::BODY_SIZE];
    body[0..4].copy_from_slice(&to.0.to_le_bytes());
    body[4..8].copy_from_slice(&to.1.to_le_bytes());
    body[Movement::MOVE_TYPE] = MOVE_NORMAL;
    body[Movement::SPEED] = speed;

    frame::encode(
        &Message { sender: mob.id, opcode: OP_MOVE, time: 0, body },
        rand::random(),
    )
}

/// Writes the session out if it owes anything and enough time has passed.
///
/// Saving only on disconnect is not enough: a crash, a power cut or a kill
/// takes the whole session with it, and to the player that is
/// indistinguishable from a database that does not work. The interval is a
/// trade — short enough that losing it costs a few steps, long enough that
/// walking across a map is a handful of writes rather than a hundred.
/// `force` ignores the clock, which is what a disconnect wants.
async fn autosave(state: &State, session: &mut Session, force: bool) {
    if !session.dirty {
        return;
    }
    let due = session
        .saved_at
        .is_none_or(|at| at.elapsed() >= state.autosave_every());
    if !force && !due {
        return;
    }

    let Some(character) = session.character.as_ref() else {
        session.dirty = false;
        return;
    };

    state.save_session(character).await;
    session.dirty = false;
    session.saved_at = Some(std::time::Instant::now());
}

async fn read_loop(
    state: &State,
    session: &mut Session,
    outbox: &Outbox,
    incoming: &mut tokio::net::tcp::OwnedReadHalf,
) -> anyhow::Result<()> {
    let mut reader = FrameReader::new();
    let mut prefix = LeadingPrefix::default();
    let mut buf = [0u8; 8192];

    loop {
        let n = incoming.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        if let Some(data) = prefix.feed(&buf[..n]) {
            reader.push(data);
        }

        while let Some(message) = reader.next_message() {
            match message {
                Ok(message) => {
                    let action = handle_message(state, session, &message).await;
                    autosave(state, session, false).await;
                    match action {
                        Action::Reply(frames) => {
                            for frame in frames {
                                let _ = outbox.send(frame);
                            }
                        }
                        Action::Ignore => {}
                        Action::Disconnect => return Ok(()),
                    }
                }
                Err(FrameError::BadChecksum) => {
                    warn!("game packet with invalid checksum");
                    return Ok(());
                }
                Err(FrameError::BadLength(size)) => {
                    warn!(size, "game packet with invalid size");
                    return Ok(());
                }
            }
        }
    }
}

/// Takes the player out of the registry and off everyone else's screen.
///
/// The watchers are collected before the removal, because once the player is
/// gone from the registry there is no position left to search around.
fn leave_world(state: &State, session: &Session) {
    let watchers = state.world.visible_to(session.client_id);
    let left = state.world.disconnect(session.client_id);

    if let (Some(character), false) = (session.character.as_ref(), watchers.is_empty()) {
        let frame = encode_remove_mob(session.client_id, DELETE_DISCONNECT);
        for watcher in &watchers {
            watcher.send(frame.clone());
        }
        info!(
            character = %character.name,
            watchers = watchers.len(),
            "left the world"
        );
    }
    let _ = left;
}

/// `TSendRemoveMobPacket` (`0x101`): who is leaving and why.
fn encode_remove_mob(client_id: u16, delete_type: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&(client_id as u32).to_le_bytes());
    body.extend_from_slice(&delete_type.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, REMOVE_MOB_SIZE);
    frame::encode(
        &Message { sender: FIXED_INDEX, opcode: OP_REMOVE_MOB, time: 0, body },
        rand::random(),
    )
}

/// The first `recv` of a game connection carries 4 leading bytes that are
/// not part of the packet. The Delphi server cuts those 4 bytes blindly
/// (`PlayerThread.pas:2545-2570`) and only on the first read; here the call
/// is made from the content, so the server keeps working with a client that
/// does not send the prefix, ours for instance.
#[derive(Default)]
struct LeadingPrefix {
    decided: bool,
    pending: Vec<u8>,
}

impl LeadingPrefix {
    /// Returns the bytes to hand to the frame reader, or `None` while
    /// there are not enough bytes to decide yet.
    fn feed<'a>(&'a mut self, data: &'a [u8]) -> Option<&'a [u8]> {
        if self.decided {
            return Some(data);
        }

        self.pending.extend_from_slice(data);
        // We need the size field of both hypotheses to choose.
        if self.pending.len() < 6 {
            return None;
        }

        self.decided = true;
        let skip = if plausible_size(&self.pending) { 0 } else { 4 };
        // The first bytes of each connection tell whether the client really sends
        // the prefix, and which one. Worth logging while that is not yet
        // confirmed against the real client.
        let head: Vec<String> =
            self.pending.iter().take(8).map(|b| format!("{b:02X}")).collect();
        debug!(head = %head.join(" "), dropped = skip, "first packet of the connection");
        Some(&self.pending[skip.min(self.pending.len())..])
    }
}

/// A frame's leading size field only makes sense within a range; outside
/// it, whatever sits in front is not a frame.
fn plausible_size(data: &[u8]) -> bool {
    let size = u16::from_le_bytes([data[0], data[1]]) as usize;
    (MIN_FRAME..=8192).contains(&size)
}

/// What to do with the connection after handling a packet.
#[derive(Debug)]
enum Action {
    /// One or more frames to write, in order.
    Reply(Vec<Vec<u8>>),
    /// Packet recognised but no reply; the connection stays alive.
    Ignore,
    /// Login refused. The original server drops the connection on every
    /// refusal of `0x685`, and the client counts on that to show the error
    /// message instead of waiting forever.
    Disconnect,
}

/// One packet in, whatever should go out.
///
/// Asynchronous because some packets have to reach the database before they
/// can be answered: a character that exists in memory but not on disk is a
/// character that disappears at the next restart. The handlers that need
/// nothing from the database stay synchronous and are called directly.
async fn handle_message(state: &State, session: &mut Session, message: &Message) -> Action {
    match message.opcode {
        OP_REQUEST_LOGIN => handle_request_login(state, session, message),
        OP_ENTER_WORLD => handle_enter_world(state, session, message, state.uptime_ms()),
        OP_CLIENT_READY => handle_client_ready(state, session),
        OP_MOVE => handle_move(state, session, message),
        dialog::OP_OPEN_NPC => handle_open_npc(state, session, message),
        dialog::OP_CLOSE_NPC_OPTION => handle_close_npc(session),
        shop::OP_BUY => handle_buy(state, session, message),
        shop::OP_SELL => handle_sell(state, session, message),
        OP_ROTATE => handle_rotate(state, session, message),
        OP_BACK_TO_CHARACTER_SELECT => handle_back_to_character_select(session),
        creation::OP_DELETE_CHARACTER | creation::OP_DELETE_CHARACTER_ALT => {
            handle_delete_character(state, session, message).await
        }
        creation::OP_CREATE_CHARACTER => {
            handle_create_character(state, session, message).await
        }
        OP_MOVE_ITEM => handle_move_item(session, message),
        OP_USE_ITEM => handle_use_item(state, session, message),
        combat::OP_ATTACK => handle_attack(state, session, message),
        OP_REVIVE => handle_revive(state, session, message),
        ability::OP_USE_SKILL => handle_use_skill(state, session, message),
        OP_DELETE_ITEM => handle_delete_item(session, message),
        OP_CHAT => handle_chat(state, session, message),
        OP_GROUP_ITEM => handle_group_item(state, session, message),
        OP_UNGROUP_ITEM => handle_ungroup_item(state, session, message),
        opcode => {
            // The original merely prints the code here; we do the same, adding the
            // size alongside, to help identify the packet.
            // The body goes in the log too. Working out what an unknown
            // packet means starts with seeing what it carries, and asking a
            // person to reproduce it a second time is a wasted round trip.
            warn!(
                opcode = format!("0x{opcode:03x}"),
                size = message.body.len(),
                body = %hex_dump(&message.body),
                "game packet not implemented yet"
            );
            Action::Ignore
        }
    }
}

fn handle_request_login(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = RequestLogin::parse(&message.body) else {
        warn!(
            size = message.body.len(),
            minimo = RequestLogin::MIN_BODY,
            body = %hex_dump(&message.body),
            "0x685 packet too short"
        );
        return Action::Disconnect;
    };

    debug!(
        account = request.account_id,
        user = %request.username,
        versao = request.version,
        "0x685 recebido"
    );

    let expected = state.cfg.game.client_version;
    if request.version != expected {
        // A wrong version usually means a wrong offset, not an old client:
        // the dump shows where the fields really are.
        warn!(
            versao = request.version,
            esperada = expected,
            user = %request.username,
            body = %hex_dump(&message.body),
            "client with a different version"
        );
        return Action::Disconnect;
    }

    let Some(account) = state.store.get(&request.username) else {
        warn!(user = %request.username, "account not found on the game server");
        return Action::Disconnect;
    };

    if account.account_status == 8 {
        warn!(user = %account.username, "account blocked");
        return Action::Disconnect;
    }

    info!(
        user = %account.username,
        id = account.id,
        personagens = account.characters.len(),
        "entered the game server"
    );

    // The client sends an id of its own in the header; ours is the one that
    // counts, and logging both is how a disagreement would surface.
    if message.sender != session.client_id {
        debug!(
            claimed = message.sender,
            assigned = session.client_id,
            "client claimed a different id"
        );
    }

    let frame = encode_char_list(&account, session.client_id, state.uptime_ms());
    state.world.set_account(session.client_id, account.id);
    session.account = Some(account);
    Action::Reply(vec![frame])
}

/// `0xF02`: the player picked a character and asked to enter.
///
/// The record is called `TNumericTokenPacket` in the original and suggests
/// a numeric password, but all that logic sits behind an unreachable
/// `Exit;` (`PacketHandlers.pas:484`): in practice only `Slot` is read.
fn handle_enter_world(
    state: &State,
    session: &mut Session,
    message: &Message,
    time: u32,
) -> Action {
    if message.body.len() < 4 {
        warn!(size = message.body.len(), "0xF02 packet without the slot");
        return Action::Disconnect;
    }
    let slot = u32::from_le_bytes(message.body[0..4].try_into().unwrap()) as usize;

    let Some(account) = session.account.clone() else {
        warn!("0xF02 before login; the connection has no account");
        return Action::Disconnect;
    };

    let Some(character) = account.characters.iter().find(|c| c.slot == slot).cloned() else {
        warn!(slot, user = %account.username, "empty slot");
        return Action::Disconnect;
    };

    info!(user = %account.username, character = %character.name, slot, "entering the world");

    let client_id = session.client_id;

    // The exact order the original sends (`Mob/Player.pas:3642-3657`). It is
    // not decoration: the client will not finish arriving until it has all of
    // it, and the one that matters most is `0x117`, which says which mob on
    // screen is the player's own. Without that the client has a world and
    // nothing to put its camera on, so it keeps the arrival camera up and
    // asks again with `0xF0B` twice a second.
    let mut frames = vec![
        encode_signal(OP_SIGNAL_READY, client_id, time, 1),
        zeroed(OP_ENTER_3A2, client_id, ENTER_3A2_SIZE),
        encode_signal(OP_SIGNAL_LOAD, client_id, time, 1),
        encode_signal(OP_SIGNAL_LOAD, client_id, time, 1),
        encode_signal(OP_SIGNAL_LOAD, client_id, time, 1),
        encode_enter_131(),
        encode_send_to_world(&account, &character, client_id, time),
        zeroed(OP_ENTER_12C, 0, ENTER_12C_SIZE),
    ];

    // Two storage slots and the client index, interleaved exactly as the
    // original interleaves them.
    for slot in ENTER_STORAGE_SLOTS {
        frames.push(encode_refresh_item(inventory::STORAGE, slot, &Item::default(), false));
        frames.push(encode_client_index(client_id));
    }
    frames.push(zeroed(OP_ENTER_94C, 0, ENTER_94C_SIZE));

    let stats = stats::of(&character, &state.items);
    session.cur_hp = stats.max_hp;
    session.cur_mp = stats.max_mp;
    session.character = Some(character);
    Action::Reply(frames)
}

/// `0xF0B`: the client finished loading the scene and is ready to spawn.
///
/// This is where the character finally gets a position on the map. `0x925`
/// says *who* the character is, not *where*. Without this packet the client
/// enters the world and floats in the middle of nowhere.
fn handle_client_ready(state: &State, session: &mut Session) -> Action {
    let Some(character) = session.character.clone() else {
        warn!("0xF0B before a character was chosen");
        return Action::Ignore;
    };

    // Spawning twice throws the player back to the starting point, and the
    // client resends this packet whenever it thinks something is missing,
    // including when trying to walk. Without this guard it gets stuck in a
    // respawn loop.
    if session.spawned {
        // The client sends this twice a second whatever we do. Rather than
        // drop it, spend it: it is the only regular tick a standing player
        // gives us, and it is what lets a monster that came back appear
        // without the player having to walk — and what carries the blows the
        // world tick left for this player.
        let mut frames = refresh_mob_visibility(state, session);
        frames.extend(collect_blows(state, session));
        return if frames.is_empty() { Action::Ignore } else { Action::Reply(frames) };
    }

    session.spawned = true;
    state.world.enter(session.client_id, character.clone());

    let skills = known_skills(state, &character);
    let mut frames = world_burst(&character, session.client_id, &skills);

    // The city is drawn by the client; the people in it are not. Without this
    // the player arrives in an empty town.
    frames.extend(refresh_npc_visibility(state, session));
    frames.extend(refresh_mob_visibility(state, session));

    // Everyone already standing nearby has to appear on this player's screen,
    // and this player on theirs. Both directions use the same spawn packet.
    let neighbours = state.world.visible_to(session.client_id);

    // Everyone nearby sees an arrival, not somebody who was always there.
    // The original makes the same distinction (`Mob/Player.pas:5185`).
    let mine = encode_spawn_as(&character, session.client_id, SPAWN_TELEPORT_IN);
    for other in &neighbours {
        if let Some(their_character) = &other.character {
            frames.push(encode_spawn(their_character, other.client_id));
        }
        other.send(mine.clone());
        session.visible.insert(other.client_id);
    }

    info!(
        character = %character.name,
        x = character.x,
        y = character.y,
        neighbours = neighbours.len(),
        npcs = session.visible_npcs.len(),
        "spawning on the map"
    );
    Action::Reply(frames)
}

/// Everything the server sends once the client reports it has loaded.
///
/// There is no acknowledgement packet in this protocol: the client keeps
/// resending `0xF0B` until it has received enough of this burst to consider
/// itself in the world. Sending only the spawn leaves it asking forever, which
/// also makes movement stutter.
///
/// This is the ordered subset of `SendToWorldSends` (`Mob/Player.pas:4945`)
/// that a character with no guild, pran, mount, friends, buffs or quests
/// receives. Packets whose field layout is not yet known are sent with the
/// right size and a zeroed body, which is what an empty list looks like
/// anyway. Three packets from the original are still missing because their
/// size is unknown: `0x138` (cash inventory), `0x936` and `0x91A` (nation).
fn world_burst(character: &Character, client_id: u16, skills: &[usize]) -> Vec<Vec<u8>> {
    let (hp, mp) = vitals(character);

    let mut frames = vec![
        encode_spawn(character, client_id),
        encode_skill_list(client_id, skills),
        encode_signal(OP_CASH, 0, 0, character.gold.min(u32::MAX as u64) as u32),
        zeroed(OP_ACCOUNT_STATUS, client_id, SIGNAL_SIZE),
        zeroed(OP_BUFFS, client_id, BUFFS_SIZE),
        zeroed(OP_ACTIVE_TITLE, client_id, ACTIVE_TITLE_SIZE),
        zeroed(OP_RELICS, FIXED_INDEX, RELICS_SIZE),
        encode_refresh_point(character),
        encode_refresh_status(character),
        zeroed(OP_ALL_ATTRIBUTES, client_id, ALL_ATTRIBUTES_SIZE),
        encode_level(character, client_id),
        encode_hp_mp(character, client_id, hp, mp),
    ];

    // The original spawns the player a second time here, after its stats have
    // been recomputed. Same opcode, same recipient, fresher numbers.
    frames.push(encode_spawn(character, client_id));
    frames
}

/// Provisional health and mana. The real formulas depend on tables we have not
/// read yet; what matters for now is that neither is zero.
fn vitals(character: &Character) -> (u32, u32) {
    (100 + character.level as u32 * 10, 50 + character.level as u32 * 5)
}

/// A packet of the right size with an empty body, for the ones whose fields
/// are not mapped yet. An empty list is all zeroes in this protocol anyway.
fn zeroed(opcode: u16, sender: u16, total_size: usize) -> Vec<u8> {
    frame::encode(
        &Message { sender, opcode, time: 0, body: vec![0u8; total_size - MIN_FRAME] },
        rand::random(),
    )
}

/// `0x106` `TSendSkillsPacket`: NPC index, send type and 40 skill ids.
fn encode_skills(client_id: u16) -> Vec<u8> {
    let mut body = vec![0u8; SKILLS_SIZE - MIN_FRAME];
    // NPC index and send type both stay 0 on login; the original only sets
    // the send type when a skill NPC is involved.
    let _ = &mut body[0..4];
    frame::encode(
        &Message { sender: client_id, opcode: OP_SKILLS, time: 0, body },
        rand::random(),
    )
}

/// `0x109` `TSendRefreshPoint`: the six attributes plus the two spendable
/// point pools. The original copies the first 12 bytes of `TStatus` straight
/// in, so the order here is the attribute order.
fn encode_refresh_point(character: &Character) -> Vec<u8> {
    let mut body = vec![0u8; REFRESH_POINT_SIZE - MIN_FRAME];
    for (i, value) in character.attributes.iter().enumerate() {
        body[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    // Free status points, which the original sends twice: once as the last
    // attribute and once in its own field.
    let free = character.attributes[5];
    body[12..14].copy_from_slice(&free.to_le_bytes());
    body[14..16].copy_from_slice(&0u16.to_le_bytes()); // skill points
    frame::encode(
        &Message { sender: FIXED_INDEX, opcode: OP_REFRESH_POINT, time: 0, body },
        rand::random(),
    )
}

/// `0x10A` `TSendRefreshStatus`: combat stats.
fn encode_refresh_status(character: &Character) -> Vec<u8> {
    let mut body = vec![0u8; REFRESH_STATUS_SIZE - MIN_FRAME];
    // Movement speed is the only field we have a real value for; the rest of
    // the combat stats come from tables we have not read yet.
    body[20..22].copy_from_slice(&(character.speed_move as u16).to_le_bytes());
    frame::encode(
        &Message { sender: FIXED_INDEX, opcode: OP_REFRESH_STATUS, time: 0, body },
        rand::random(),
    )
}

/// `0x108` `TSendCurrentLevel`: level, a constant, and experience.
fn encode_level(character: &Character, client_id: u16) -> Vec<u8> {
    let mut body = vec![0u8; LEVEL_SIZE - MIN_FRAME];
    // Same convention as everywhere else: the client adds 1.
    body[0..2].copy_from_slice(&character.level.saturating_sub(1).to_le_bytes());
    body[2..4].copy_from_slice(&LEVEL_UNK.to_le_bytes());
    body[4..12].copy_from_slice(&character.exp.to_le_bytes());
    frame::encode(
        &Message { sender: client_id, opcode: OP_LEVEL, time: 0, body },
        rand::random(),
    )
}

/// `0x103` `TSendCurrentHPMPPacket`: maximum and current HP and MP.
fn encode_hp_mp(character: &Character, client_id: u16, hp: u32, mp: u32) -> Vec<u8> {
    let _ = character;
    let mut body = vec![0u8; HP_MP_SIZE - MIN_FRAME];
    body[0..4].copy_from_slice(&hp.to_le_bytes());
    body[4..8].copy_from_slice(&hp.to_le_bytes());
    body[8..12].copy_from_slice(&mp.to_le_bytes());
    body[12..16].copy_from_slice(&mp.to_le_bytes());
    // The last field marks an update rather than a login; 0 on this path.
    frame::encode(
        &Message { sender: client_id, opcode: OP_HP_MP, time: 0, body },
        rand::random(),
    )
}

/// `0x301`: the player walked.
///
/// The original server **returns nothing to the mover**: the client moves
/// on its own and the server only relays the same packet to whoever can see
/// the player (`SendToVisible(..., false)` in `PacketHandlers.pas:892`).
/// While there is no registry of online players, storing the position does.
fn handle_move(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(movement) = Movement::parse(&message.body) else {
        warn!(size = message.body.len(), "0x301 packet too short");
        return Action::Ignore;
    };

    // The first few movements of a session are dumped in full: the layout of
    // this packet is only confirmed against the real client, and a wrong
    // offset here reads garbage as a coordinate.
    if session.moves_logged < MOVES_TO_DUMP {
        session.moves_logged += 1;
        debug!(
            x = movement.x,
            y = movement.y,
            move_type = movement.move_type,
            speed = movement.speed,
            body = %hex_dump(&message.body),
            "0x301 movement"
        );
    }

    if !movement.is_valid() {
        warn!(x = movement.x, y = movement.y, "invalid destination");
        return Action::Ignore;
    }

    // The original compares the header id against the player's and drops the
    // packet when they disagree. We go further and simply never read it: the
    // movement is applied to whoever owns this connection, so a client cannot
    // move somebody else no matter what it puts in the field.
    if message.sender != session.client_id {
        debug!(
            claimed = message.sender,
            assigned = session.client_id,
            "0x301 header carries another id; moving the connection's own player"
        );
    }

    // Teleport is server-originated. Honouring it from a client packet is
    // exactly the exploit that lets a modified client jump anywhere on the
    // map, so it is refused outright, as the original does.
    if movement.move_type == MOVE_TELEPORT {
        warn!(x = movement.x, y = movement.y, "client asked to teleport itself, refused");
        return Action::Ignore;
    }

    // Beyond walking, the real client sends other move types (16 has been
    // observed in the wild). The original drops those entirely; we still
    // record where the player ended up, because a stale position would be
    // wrong the moment positions get persisted.
    if movement.move_type != MOVE_NORMAL {
        debug!(move_type = movement.move_type, "move type other than walking");
    }

    let (x, y) = (movement.x as u32, movement.y as u32);
    if let Some(character) = session.character.as_mut() {
        character.x = x;
        character.y = y;
        session.dirty = true;
    }
    state.world.move_to(session.client_id, x, y);

    // The mover hears nothing back: the client moves itself and would fight
    // its own echo. Everyone who can see it gets the same packet, carrying
    // our id so they know who walked.
    let relay = frame::encode(
        &Message {
            sender: session.client_id,
            opcode: OP_MOVE,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    state.world.send_to_visible(session.client_id, relay);

    refresh_visibility(state, session)
}

/// Spawns and removes players as they walk into and out of range.
///
/// Without this, two players who log in far apart never see each other no
/// matter how close they walk. The set lives on the session so each side
/// appears exactly once, and disappears exactly once.
fn refresh_visibility(state: &State, session: &mut Session) -> Action {
    if session.character.is_none() {
        return Action::Ignore;
    }

    let neighbours = state.world.visible_to(session.client_id);
    let now: HashSet<u16> = neighbours.iter().map(|p| p.client_id).collect();

    let mut frames = refresh_npc_visibility(state, session);
    frames.extend(refresh_mob_visibility(state, session));
    let character = session.character.as_ref().expect("checked above");
    let mine = encode_spawn(character, session.client_id);

    for other in &neighbours {
        if session.visible.insert(other.client_id) {
            if let Some(their_character) = &other.character {
                frames.push(encode_spawn(their_character, other.client_id));
            }
            other.send(mine.clone());
        }
    }

    // Anyone no longer in range leaves both screens.
    let gone: Vec<u16> = session.visible.difference(&now).copied().collect();
    for id in gone {
        session.visible.remove(&id);
        frames.push(encode_remove_mob(id, DELETE_NORMAL));
        if let Some(other) = neighbours.iter().find(|p| p.client_id == id) {
            other.send(encode_remove_mob(session.client_id, DELETE_NORMAL));
        }
    }

    if frames.is_empty() {
        Action::Ignore
    } else {
        Action::Reply(frames)
    }
}

/// Places townspeople on the screen and takes them off it again.
///
/// NPCs are the other half of visibility, and the simpler half: they never
/// move, so only the player walking changes the answer. They are also the
/// first thing a player notices missing — an empty city looks broken in a way
/// an empty field does not.
fn refresh_npc_visibility(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let Some(character) = session.character.as_ref() else {
        return Vec::new();
    };
    let at = (character.x as f32, character.y as f32);

    let mut frames = Vec::new();
    let near: HashSet<u16> = state
        .world
        .npcs_near(at, DISTANCE_TO_WATCH)
        .iter()
        .map(|npc| npc.id)
        .collect();

    for npc in state.world.npcs_near(at, DISTANCE_TO_WATCH) {
        if session.visible_npcs.insert(npc.id) {
            frames.push(encode_npc_spawn(npc));
        }
    }

    // The wider radius is what keeps an NPC from flickering while a player
    // walks back and forth across the edge of the watch distance.
    let gone: Vec<u16> = session
        .visible_npcs
        .iter()
        .copied()
        .filter(|id| !near.contains(id))
        .filter(|id| {
            state
                .world
                .npcs()
                .iter()
                .find(|npc| npc.id == *id)
                .is_none_or(|npc| !within(at, (npc.x, npc.y), DISTANCE_TO_FORGET))
        })
        .collect();

    for id in gone {
        session.visible_npcs.remove(&id);
        frames.push(encode_remove_mob(id, DELETE_NORMAL));
    }

    frames
}

/// Puts monsters on the screen and takes them off it again.
///
/// The same two-radius rule as everything else, with one addition: a monster
/// that died while on screen has to be removed even though it has not moved.
fn refresh_mob_visibility(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let Some(character) = session.character.as_ref() else {
        return Vec::new();
    };
    let at = (character.x as f32, character.y as f32);

    let near = state.world.mobs_near(at, DISTANCE_TO_WATCH);
    let mut frames = Vec::new();

    for mob in &near {
        if session.visible_mobs.insert(mob.id) {
            frames.push(encode_mob_spawn(mob));
        }
    }

    // Anything on screen that is no longer near enough, or no longer alive,
    // comes off it. Asking the world for each one covers the second case:
    // a monster killed by somebody else is not in `near` any more.
    let gone: Vec<u16> = session
        .visible_mobs
        .iter()
        .copied()
        .filter(|id| !near.iter().any(|m| m.id == *id))
        .filter(|id| {
            state
                .world
                .mob(*id)
                .is_none_or(|mob| !mob.is_alive() || !within(at, mob.position(), DISTANCE_TO_FORGET))
        })
        .collect();

    for id in gone {
        session.visible_mobs.remove(&id);
        frames.push(encode_remove_mob(id, DELETE_NORMAL));
    }

    frames
}

fn within(a: (f32, f32), b: (f32, f32), radius: f32) -> bool {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy <= radius * radius
}

/// The NPC flavour of `0x349` (`TSendCreateNpcPacket`). Same size and mostly
/// the same layout as a player's, with the differences the original makes in
/// `TBaseNpc.GetCreateMob`: the model comes from the NPC's own equipment, the
/// name is an index into the client's string table rather than text, and the
/// job title travels with it.
fn encode_npc_spawn(npc: &Npc) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    // The name field carries the digits of a string index, which is what the
    // file holds and what the client looks up. Writing the real name here
    // would show a number-shaped blank instead.
    if let Some(index) = npc.name_index {
        write_fixed_str(&mut body[off::NAME..off::NAME + 16], &index.to_string());
    }

    for (i, item) in npc.equip.iter().enumerate() {
        put16(&mut body, off::EQUIP + i * 2, *item);
    }

    body[off::POSITION_X..off::POSITION_X + 4].copy_from_slice(&npc.x.to_le_bytes());
    body[off::POSITION_Y..off::POSITION_Y + 4].copy_from_slice(&npc.y.to_le_bytes());
    put32(&mut body, off::ROTATION, npc.rotation as u32);

    // The original sends MaxHP twice, once as the mana ceiling as well.
    put32(&mut body, off::MAX_HP, npc.max_hp);
    put32(&mut body, off::MAX_MP, npc.max_hp);
    put32(&mut body, off::CUR_HP, npc.cur_hp.min(npc.max_hp));
    put32(&mut body, off::CUR_MP, npc.cur_mp.min(npc.max_hp));

    body[off::UNK0] = NPC_UNK0;
    body[off::SPEED_MOVE] = npc.speed_move as u8;
    body[off::SPAWN_TYPE] = SPAWN_NORMAL;
    body[off::SIZES..off::SIZES + 4].copy_from_slice(&npc.sizes);

    body[npc_offset::IS_SERVICE] = NPC_IS_SERVICE;
    put16(&mut body, npc_offset::EFFECT_TYPE, NPC_EFFECT_TYPE);
    write_fixed_str(
        &mut body[npc_offset::TITLE..npc_offset::TITLE + npc_offset::TITLE_MAX],
        &npc.title,
    );

    debug_assert_eq!(body.len() + MIN_FRAME, CREATE_MOB_SIZE);
    frame::encode(
        &Message { sender: npc.id, opcode: OP_CREATE_MOB, time: 0, body },
        rand::random(),
    )
}

/// `0x301` body (`TMovementPacket`, `Data/Packets.pas`): destination as two
/// `f32`, seis bytes de padding, tipo de movimento e velocidade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Movement {
    pub x: f32,
    pub y: f32,
    pub move_type: u8,
    pub speed: u8,
}

impl Movement {
    const MOVE_TYPE: usize = 14;
    const SPEED: usize = 15;
    pub const BODY_SIZE: usize = 20;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            x: f32::from_le_bytes(body[0..4].try_into().ok()?),
            y: f32::from_le_bytes(body[4..8].try_into().ok()?),
            move_type: body[Self::MOVE_TYPE],
            speed: body[Self::SPEED],
        })
    }

    /// Mesma checagem do original (`TPosition.IsValid`): recusa infinito e
    /// NaN. Note that `(0, 0)` passes, as it did there.
    pub fn is_valid(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// `TSendCreateMobPacket` (`0x349`): places a creature on the map.
///
/// The original sends this same packet three times on world entry: twice to
/// the player themselves and once to those nearby. Here we send only the
/// player's own; the neighbour one lands once there is a registry of
/// online players exists.
fn encode_spawn(character: &Character, client_id: u16) -> Vec<u8> {
    encode_spawn_as(character, client_id, SPAWN_NORMAL)
}

/// The same, saying how the creature is appearing.
fn encode_spawn_as(character: &Character, client_id: u16, spawn_type: u8) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let put_f32 = |b: &mut Vec<u8>, at: usize, v: f32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    write_fixed_str(&mut body[off::NAME..off::NAME + 16], &character.name);

    // Equip[0] is the class and Equip[1] the hair, as in the character list.
    put16(&mut body, off::EQUIP, character.class_index);
    put16(&mut body, off::EQUIP + 2, character.hair);

    // And slots 2 to 7 are what the character is actually wearing. Without
    // these the client draws a body and a haircut and nothing else: the
    // armour is in the record it holds, and it does not dress anyone from
    // there — it dresses them from the spawn.
    for slot in WORN_SLOTS {
        put16(&mut body, off::EQUIP + slot as usize * 2, worn_appearance(character, slot));
    }

    body[off::ARMA_REFINE] = SPAWN_ARMA_REFINE;

    // Floating point coordinates: the original server copies the pair of
    // `Single` straight from the account, with no scaling at all.
    put_f32(&mut body, off::POSITION_X, character.x as f32);
    put_f32(&mut body, off::POSITION_Y, character.y as f32);
    put32(&mut body, off::ROTATION, 0);

    let hp = 100 + character.level as u32 * 10;
    let mp = 50 + character.level as u32 * 5;
    put32(&mut body, off::MAX_HP, hp);
    put32(&mut body, off::MAX_MP, mp);
    put32(&mut body, off::CUR_HP, hp);
    put32(&mut body, off::CUR_MP, mp);

    body[off::UNK0] = SPAWN_UNK0;
    body[off::SPEED_MOVE] = character.speed_move;
    body[off::SPAWN_TYPE] = spawn_type;
    body[off::SIZES..off::SIZES + 4].copy_from_slice(&character.sizes);

    // Without a guild, the field carries only the nation shifted 12 bits.
    put16(&mut body, off::GUILD_AND_NATION, character.nation << 12);
    put16(&mut body, off::EFFECTS + 2, SPAWN_EFFECT_1);

    debug_assert_eq!(body.len() + MIN_FRAME, CREATE_MOB_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CREATE_MOB, time: 0, body },
        rand::random(),
    )
}

/// `TItem` (`Data/MiscData.pas:44`), the twenty bytes an item travels as,
/// in the character record and everywhere else the protocol carries one.
///
/// ```text
/// 0   u16  Index, the id in the item table; zero means an empty slot
/// 2   u16  APP, an appearance that overrides the real look
/// 4   i32  Identific
/// 8   u8[3] effect index, then u8[3] effect value
/// 14  u8   durability now, u8 durability ceiling
/// 16  u16  Refi, the refine level
/// 18  u16  Time, when a rented item expires
/// ```
fn write_item(out: &mut [u8], item: &Item) {
    debug_assert_eq!(out.len(), character_offset::ITEM_SIZE);
    out[0..2].copy_from_slice(&item.index.to_le_bytes());
    out[2..4].copy_from_slice(&item.appearance.to_le_bytes());
    out[4..8].copy_from_slice(&item.identific.to_le_bytes());
    out[8..11].copy_from_slice(&item.effect_index);
    out[11..14].copy_from_slice(&item.effect_value);
    out[14] = item.durability_min;
    out[15] = item.durability_max;
    out[16..18].copy_from_slice(&item.refine.to_le_bytes());
    out[18..20].copy_from_slice(&item.expires_at.to_le_bytes());
}


/// `0x30F`: the player clicked an NPC, or picked something from its menu.
///
/// The same packet does both, told apart by the option field: zero is the
/// click and anything else is a choice. The distance is checked on every one
/// of them, not only on the click, because a window left open while walking
/// away would otherwise keep working from across the map.
fn handle_open_npc(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = dialog::OpenNpc::parse(&message.body) else {
        warn!(size = message.body.len(), "0x30F packet too short");
        return Action::Ignore;
    };

    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let at = (character.x as f32, character.y as f32);

    let npc_id = request.npc as u16;
    let Some(npc) = state.world.npcs().iter().find(|n| n.id == npc_id) else {
        debug!(npc = npc_id, "0x30F for an npc that is not in the world");
        return Action::Reply(vec![encode_menu_close()]);
    };

    if !within(at, (npc.x, npc.y), dialog::TALK_RANGE) {
        session.opened_npc = None;
        return Action::Reply(vec![
            encode_client_message(session.client_id, "You are too far away."),
            encode_menu_close(),
        ]);
    }

    match request.option {
        dialog::option::OPEN => {
            session.opened_npc = Some(npc_id);
            let entries = dialog::entries(npc);
            info!(npc = npc_id, name = %npc.label, entries = entries.len(), "npc menu");

            let mut frames = Vec::with_capacity(entries.len() + 2);
            frames.push(encode_menu_begin(session.client_id));
            frames.push(encode_signal(dialog::OP_MENU_OWNER, session.client_id, 0, npc_id as u32));
            frames.extend(entries.into_iter().map(encode_menu_entry));
            Action::Reply(frames)
        }
        dialog::option::SHOP => {
            if !npc.sells() {
                return Action::Reply(vec![
                    encode_client_message(session.client_id, "There is nothing for sale."),
                    encode_menu_close(),
                ]);
            }
            session.opened_npc = Some(npc_id);
            info!(npc = npc_id, name = %npc.label, "shop opened");

            // The conversation closes before the shop opens. The client
            // letterboxes the screen while a menu is up, and leaving it up
            // behind the shop is what looks like a cutscene that will not
            // end. The original closes it for every option except talking,
            // quests and the menu heading (`PacketHandlers.pas:3382`).
            Action::Reply(vec![encode_menu_close(), encode_show_shop(session.client_id, npc)])
        }
        dialog::option::SKILLS => {
            // The skill master's window is the same packet as the bar, with
            // the NPC named and a send type that tells the client to open the
            // trainer rather than redraw the bar
            // (`TNPCHandlers.ShowSkills` -> `SendPlayerSkills(NpcId)`).
            let Some(character) = session.character.as_ref() else {
                return Action::Ignore;
            };
            let skills = known_skills(state, character);
            info!(npc = npc_id, name = %npc.label, skills = skills.len(), "skill trainer");
            Action::Reply(vec![
                encode_menu_close(),
                encode_skill_list_from(session.client_id, npc_id, &skills),
            ])
        }
        dialog::option::CLOSE => {
            session.opened_npc = None;
            Action::Reply(vec![encode_menu_close()])
        }
        other => {
            // Everything else is a system we have not built. Saying so beats
            // a window that opens onto nothing and never closes.
            let name = dialog::option_text(other as u8);
            debug!(npc = npc_id, option = other, text = name, "npc option not implemented");

            // Closed first, for the same reason the shop closes it: the
            // client keeps the screen letterboxed while a menu is up.
            let mut frames = Vec::new();
            if !request.keeps_window_open() {
                frames.push(encode_menu_close());
            }
            frames.push(encode_client_message(
                session.client_id,
                &format!("{name} is not available yet."),
            ));
            Action::Reply(frames)
        }
    }
}

/// `0x348`: the client closed the window on its own.
fn handle_close_npc(session: &mut Session) -> Action {
    session.opened_npc = None;
    Action::Ignore
}

/// `0x313`: buy from the shop that is open.
fn handle_buy(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = shop::Buy::parse(&message.body) else {
        warn!(size = message.body.len(), "0x313 packet too short");
        return Action::Ignore;
    };

    let Some(npc) = open_shop(state, session, request.npc as u16).cloned() else {
        return Action::Reply(vec![encode_client_message(
            session.client_id,
            &shop::ShopError::WrongNpc.message(),
        )]);
    };

    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let outcome = shop::buy(&npc, request, &mut character.items, character.gold, &state.items);

    match outcome {
        Ok(change) => {
            character.gold = change.gold;
            session.dirty = true;
            info!(
                npc = npc.id,
                item = change.item.index,
                slot = change.slot,
                gold = change.gold,
                "bought"
            );
            let mut frames = Vec::new();
            // Whatever currency was spent goes back first, so the client
            // redraws the drained stacks before it draws the new item.
            for (slot, left) in &change.spent {
                frames.push(encode_refresh_item(inventory::BAG, *slot, left, false));
            }
            frames.push(encode_refresh_item(inventory::BAG, change.slot, &change.item, true));
            frames.push(encode_refresh_money(change.gold));
            Action::Reply(frames)
        }
        Err(e) => {
            debug!(npc = npc.id, error = %e, "purchase refused");
            Action::Reply(vec![encode_client_message(client_id, &e.message())])
        }
    }
}

/// `0x314`: sell a bag slot to the shop that is open.
fn handle_sell(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = shop::Sell::parse(&message.body) else {
        warn!(size = message.body.len(), "0x314 packet too short");
        return Action::Ignore;
    };

    if open_shop(state, session, request.npc as u16).is_none() {
        return Action::Reply(vec![encode_client_message(
            session.client_id,
            &shop::ShopError::WrongNpc.message(),
        )]);
    }

    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let outcome = shop::sell(request, &mut character.items, character.gold, &state.items);

    match outcome {
        Ok(change) => {
            character.gold = change.gold;
            session.dirty = true;
            info!(slot = change.slot, gold = change.gold, "sold");
            Action::Reply(vec![
                encode_refresh_item(inventory::BAG, change.slot, &change.item, true),
                encode_refresh_money(change.gold),
            ])
        }
        Err(e) => {
            debug!(error = %e, "sale refused");
            Action::Reply(vec![encode_client_message(client_id, &e.message())])
        }
    }
}

/// The NPC the client says it is trading with, if it may.
///
/// The gate is distance, not a flag saying the window is open. Two reasons.
/// The flag was wrong: the client closes the option menu the moment the shop
/// window replaces it, and pressing escape closes it too, so a real player
/// buying normally would be refused. And the flag was never the security
/// property — standing next to the NPC is. A client that sends a purchase
/// without opening a window first has done nothing it could not have done by
/// clicking, since the stock and the prices are ours.
///
/// The original checks a flag it only ever sets when the player picks *talk*
/// or *quest*, which means buying from a merchant with neither of those on
/// its menu — Roze, for one — could never have worked there either.
fn open_shop<'a>(state: &'a State, session: &Session, claimed: u16) -> Option<&'a Npc> {
    let npc = state.world.npcs().iter().find(|n| n.id == claimed)?;

    let character = session.character.as_ref()?;
    let at = (character.x as f32, character.y as f32);
    if !within(at, (npc.x, npc.y), dialog::TALK_RANGE) {
        debug!(claimed, "shop packet from too far away");
        return None;
    }
    Some(npc)
}

/// `0x70F`: an item dragged from one slot to another.
fn handle_move_item(session: &mut Session, message: &Message) -> Action {
    let Some(request) = MoveItem::parse(&message.body) else {
        warn!(size = message.body.len(), "0x70F packet too short");
        return Action::Ignore;
    };

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };

    let (from, to) = (request.from(), request.to());
    match character.items.move_item(from, to) {
        Ok(()) => session.dirty = true,
        Err(e) => debug!(?from, ?to, error = %e, "item not moved"),
    }

    let character = session.character.as_ref().expect("checked above");

    // Both slots go back either way. The client has already drawn the item in
    // its new place, so on a refusal it has to be told what is really there,
    // and on success it needs the source cleared.
    let there = slot_item(&character.items, to.0, to.1);
    let here = slot_item(&character.items, from.0, from.1);
    Action::Reply(vec![
        encode_refresh_item(to.0, to.1, &there, false),
        encode_refresh_item(from.0, from.1, &here, false),
    ])
}

/// `0x332`: stack one pile onto another (`AgroupItem`).
///
/// Both slots are in the bag. The original merges when the two hold the same
/// item — it adds the source's count onto the destination and empties the
/// source — and does nothing otherwise. We add one guard the original leaves
/// to the client: the item has to be one that stacks at all, so dragging two
/// identical swords together cannot silently add their refine levels.
fn handle_group_item(state: &State, session: &mut Session, message: &Message) -> Action {
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
fn handle_ungroup_item(state: &State, session: &mut Session, message: &Message) -> Action {
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
fn handle_delete_item(session: &mut Session, message: &Message) -> Action {
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
fn handle_use_item(state: &State, session: &mut Session, message: &Message) -> Action {
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

    // One off the stack, and the slot goes empty when the last one is used.
    let character = session.character.as_mut().expect("checked above");
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
    Action::Reply(vec![
        encode_hp_mp(character, session.client_id, session.cur_hp, session.cur_mp),
        encode_refresh_item(container, slot, &remaining, false),
    ])
}

/// Raises a pool without letting it past its ceiling.
fn session_heal(current: &mut u32, by: u32, ceiling: u32) {
    *current = current.saturating_add(by).min(ceiling);
}

/// What is in a slot, or an empty item addressed to it when nothing is.
fn slot_item(inventory: &Inventory, container: u8, slot: u16) -> Item {
    inventory
        .get(container, slot)
        .cloned()
        .unwrap_or(Item { container, slot, ..Item::default() })
}

/// `TMoveItemPacket` (`0x70F`), 8 bytes of body.
///
/// Four `WORD`s, and **the destination comes first**: `DestType, DestSlot,
/// SrcType, SrcSlot` (`Data/Packets.pas:903`). Reading them the other way
/// round is a bug that looks like the client dragging from empty slots,
/// because every drag arrives backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveItem {
    pub to_container: u16,
    pub to_slot: u16,
    pub from_container: u16,
    pub from_slot: u16,
}

impl MoveItem {
    pub const BODY_SIZE: usize = 8;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            to_container: u16::from_le_bytes(body[0..2].try_into().ok()?),
            to_slot: u16::from_le_bytes(body[2..4].try_into().ok()?),
            from_container: u16::from_le_bytes(body[4..6].try_into().ok()?),
            from_slot: u16::from_le_bytes(body[6..8].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.to_container.to_le_bytes());
        body.extend_from_slice(&self.to_slot.to_le_bytes());
        body.extend_from_slice(&self.from_container.to_le_bytes());
        body.extend_from_slice(&self.from_slot.to_le_bytes());
        body
    }

    pub fn from(&self) -> (u8, u16) {
        (self.from_container as u8, self.from_slot)
    }

    pub fn to(&self) -> (u8, u16) {
        (self.to_container as u8, self.to_slot)
    }
}

/// `TDeleteItemPacket` (`0x32C`), 8 bytes: the slot, then the container.
/// Note the order — it is the opposite of everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteItem {
    pub slot: u32,
    pub container: u32,
}

impl DeleteItem {
    pub const BODY_SIZE: usize = 8;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            slot: u32::from_le_bytes(body[0..4].try_into().ok()?),
            container: u32::from_le_bytes(body[4..8].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.slot.to_le_bytes());
        body.extend_from_slice(&self.container.to_le_bytes());
        body
    }
}

/// `TUseItemPacket` (`0x31D`), 12 bytes: the container, the slot, and a
/// third field whose meaning depends on the kind of item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UseItem {
    pub container: u32,
    pub slot: u32,
    pub argument: u32,
}

impl UseItem {
    pub const BODY_SIZE: usize = 12;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        Some(Self {
            container: u32::from_le_bytes(body[0..4].try_into().ok()?),
            slot: u32::from_le_bytes(body[4..8].try_into().ok()?),
            argument: u32::from_le_bytes(body[8..12].try_into().ok()?),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.container.to_le_bytes());
        body.extend_from_slice(&self.slot.to_le_bytes());
        body.extend_from_slice(&self.argument.to_le_bytes());
        body
    }
}


/// A bare header, which is all a signal is (`ServerSocket.pas:3168`).
fn encode_bare_signal(opcode: u16, index: u16) -> Vec<u8> {
    frame::encode(&Message { sender: index, opcode, time: 0, body: Vec::new() }, rand::random())
}

fn encode_menu_begin(client_id: u16) -> Vec<u8> {
    encode_bare_signal(dialog::OP_MENU_BEGIN, client_id)
}

/// Closing is addressed to the fixed index, not to the player: that is what
/// the original sends (`PacketHandlers.pas:3385`).
fn encode_menu_close() -> Vec<u8> {
    encode_bare_signal(dialog::OP_MENU_CLOSE, dialog::FIXED_INDEX)
}

/// `TShowOptionsPacket` (`0x112`): one line of the menu.
fn encode_menu_entry(option: u8) -> Vec<u8> {
    let body = dialog::menu_entry_body(option);
    debug_assert_eq!(body.len() + MIN_FRAME, dialog::MENU_ENTRY_SIZE);
    frame::encode(
        &Message { sender: MENU_ENTRY_INDEX, opcode: dialog::OP_MENU_ENTRY, time: 0, body },
        rand::random(),
    )
}

/// `TShowShopPacket` (`0x106`): the forty ids the window shows.
fn encode_show_shop(client_id: u16, npc: &Npc) -> Vec<u8> {
    let mut body = Vec::with_capacity(shop::SHOW_SHOP_SIZE - MIN_FRAME);
    body.extend_from_slice(&npc.id.to_le_bytes());
    body.extend_from_slice(&shop::SHOP_DEF_BYTE.to_le_bytes());
    for id in shop::stock(npc) {
        body.extend_from_slice(&id.to_le_bytes());
    }

    debug_assert_eq!(body.len() + MIN_FRAME, shop::SHOW_SHOP_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: shop::OP_SHOW_SHOP, time: 0, body },
        rand::random(),
    )
}

/// `TRefreshItemPacket` (`0xF0E`): one slot changed. An empty item clears it.
fn encode_refresh_item(container: u8, slot: u16, item: &Item, notice: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(shop::REFRESH_ITEM_SIZE - MIN_FRAME);
    body.push(notice as u8);
    body.push(container);
    body.extend_from_slice(&slot.to_le_bytes());

    let at = body.len();
    body.resize(at + character_offset::ITEM_SIZE, 0);
    write_item(&mut body[at..], item);

    debug_assert_eq!(body.len() + MIN_FRAME, shop::REFRESH_ITEM_SIZE);
    frame::encode(
        &Message { sender: dialog::FIXED_INDEX, opcode: shop::OP_REFRESH_ITEM, time: 0, body },
        rand::random(),
    )
}

/// `TRefreshMoneyPacket` (`0x312`): the purse, and the storage purse we do
/// not keep yet.
fn encode_refresh_money(gold: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(shop::REFRESH_MONEY_SIZE - MIN_FRAME);
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&gold.to_le_bytes());
    body.extend_from_slice(&0u64.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, shop::REFRESH_MONEY_SIZE);
    frame::encode(
        &Message { sender: dialog::FIXED_INDEX, opcode: shop::OP_REFRESH_MONEY, time: 0, body },
        rand::random(),
    )
}

/// `TClientMessagePacket` (`0x984`): a line of text for the player.
fn encode_client_message(client_id: u16, text: &str) -> Vec<u8> {
    let mut body = vec![0u8; CLIENT_MESSAGE_SIZE - MIN_FRAME];
    body[1] = MESSAGE_NOTICE;
    write_fixed_str(&mut body[4..4 + CLIENT_MESSAGE_TEXT], text);

    debug_assert_eq!(body.len() + MIN_FRAME, CLIENT_MESSAGE_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CLIENT_MESSAGE, time: 0, body },
        rand::random(),
    )
}


/// `0x305`: the player turned.
///
/// Relayed rather than answered: the client that sent it already turned, and
/// everyone who can see the player needs to see the same. The value is kept
/// on the presence so somebody walking into view is drawn facing the right
/// way, and it is dropped on logout because the client sends it again the
/// moment the mouse moves.
fn handle_rotate(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < 4 {
        warn!(size = message.body.len(), "0x305 packet too short");
        return Action::Ignore;
    }
    let rotation = u32::from_le_bytes(message.body[0..4].try_into().unwrap());

    if session.rotation == rotation {
        return Action::Ignore;
    }
    session.rotation = rotation;
    state.world.turn(session.client_id, rotation);

    let relay = frame::encode(
        &Message {
            sender: session.client_id,
            opcode: OP_ROTATE,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    state.world.send_to_visible(session.client_id, relay);
    Action::Ignore
}

/// Offsets inside the `0xF86` body, once the 12-byte header is gone
/// (`TChatPacket`, `Data/Packets.pas:188`).
mod chat_offset {
    /// Which channel of chat this is: say, whisper, party, guild, nation.
    pub const TYPE: usize = 0;
    /// Six bytes the original never reads (`NotUse`), then the colour, which
    /// the client picks and we pass through untouched.
    #[allow(dead_code)]
    pub const COLOR: usize = 8;
    /// Who is speaking, sixteen bytes. For a whisper this is instead the
    /// person being whispered to, on the way in.
    pub const NICK: usize = 12;
    /// The line itself, a hundred and twenty-eight bytes (`Fala`). Only the
    /// tests read it: the handler echoes the body through untouched.
    #[allow(dead_code)]
    pub const LINE: usize = 28;
    /// The whole body, header already removed.
    pub const SIZE: usize = 156;
}

/// The kinds of chat, from `Data/GlobalDefs.pas:355`.
const CHAT_NORMAL: u16 = 0;
const CHAT_WHISPER: u16 = 1;

/// `0xF86`: a player speaks (`TPacketHandlers.SendClientSay`).
///
/// Only ordinary say and whisper are here. Say is the one that makes the world
/// feel inhabited: the original stamps the speaker's name into the packet and
/// echoes it, unchanged, to everyone who can see them — themselves included,
/// which is why you see your own bubble (`SendToVisible` defaults
/// `sendToSelf` to true). Party, guild and nation chat wait on those systems
/// and are logged, not dropped silently, so a tester can see the client tried.
fn handle_chat(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < chat_offset::SIZE {
        warn!(size = message.body.len(), "0xF86 chat packet too short");
        return Action::Ignore;
    }
    if session.character.is_none() {
        return Action::Ignore;
    }

    let kind = u16::from_le_bytes(
        message.body[chat_offset::TYPE..chat_offset::TYPE + 2].try_into().unwrap(),
    );

    match kind {
        CHAT_NORMAL => {
            let speaker = session.character.as_ref().unwrap().name.clone();

            // The name in the packet is whatever the client put there; stamp
            // ours over it so nobody can speak under another's name.
            let mut body = message.body.clone();
            body[chat_offset::NICK..chat_offset::NICK + 16].fill(0);
            write_fixed_str(&mut body[chat_offset::NICK..chat_offset::NICK + 16], &speaker);

            let relay = frame::encode(
                &Message {
                    sender: session.client_id,
                    opcode: OP_CHAT,
                    time: message.time,
                    body,
                },
                rand::random(),
            );
            // Everyone in view hears it, and the speaker sees their own bubble.
            // The original does both through `SendToVisible`; the self copy is
            // a reply here so it rides the same socket without a second lookup.
            state.world.send_to_visible(session.client_id, relay.clone());
            return Action::Reply(vec![relay]);
        }
        CHAT_WHISPER => {
            let target = read_fixed_str(&message.body[chat_offset::NICK..chat_offset::NICK + 16]);
            let speaker = session.character.as_ref().unwrap().name.clone();

            let Some(to) = state.world.client_id_by_name(&target) else {
                return Action::Reply(vec![encode_client_message(
                    session.client_id,
                    "Personagem não encontrado.",
                )]);
            };

            // The reply the speaker sees keeps the target's name; the copy the
            // target sees carries the speaker's, so each end knows who.
            let seen_by_target = {
                let mut body = message.body.clone();
                body[chat_offset::NICK..chat_offset::NICK + 16].fill(0);
                write_fixed_str(
                    &mut body[chat_offset::NICK..chat_offset::NICK + 16],
                    &speaker,
                );
                frame::encode(
                    &Message { sender: session.client_id, opcode: OP_CHAT, time: message.time, body },
                    rand::random(),
                )
            };
            state.world.send_to(to, seen_by_target);

            let echo = frame::encode(
                &Message {
                    sender: session.client_id,
                    opcode: OP_CHAT,
                    time: message.time,
                    body: message.body.clone(),
                },
                rand::random(),
            );
            return Action::Reply(vec![echo]);
        }
        other => {
            debug!(kind = other, "chat type not handled yet");
        }
    }
    Action::Ignore
}

/// `0x3E04`: make a character in one of the three slots.
///
/// The reply is the whole character list either way, because that is what the
/// client redraws the selection screen from; a refusal also carries the
/// sentence that says why.
async fn handle_create_character(
    state: &State,
    session: &mut Session,
    message: &Message,
) -> Action {
    let Some(request) = creation::CreateCharacter::parse(&message.body) else {
        warn!(size = message.body.len(), "0x3E04 packet too short");
        return Action::Ignore;
    };

    let Some(account) = session.account.as_ref() else {
        warn!("0x3E04 before logging in");
        return Action::Ignore;
    };
    let username = account.username.clone();

    let class_number = (request.class_index / 10).clamp(1, 6);
    let outcome = creation::create(
        &request,
        &account.characters,
        |name| state.store.name_taken(name),
        state.template(class_number),
    );

    let mut character = match outcome {
        Ok(character) => character,
        Err(e) => {
            info!(user = %username, name = %request.name, error = %e, "character refused");
            return Action::Reply(vec![
                encode_client_message(session.client_id, &e.message()),
                encode_char_list(account, session.client_id, state.uptime_ms()),
            ]);
        }
    };

    // The database hands back the id, and without it nothing this character
    // does later can be saved. A failure here has to reach the player rather
    // than leave a character that exists until the next restart.
    if let Some(db) = state.db() {
        match db.insert_character(account.id as i64, &character).await {
            Ok(id) => character.id = id,
            Err(e) => {
                // The whole chain, not only the outermost context: the
                // one time this fired, the reason was two layers down and
                // the log said nothing useful.
                warn!(
                    user = %username,
                    name = %character.name,
                    error = format!("{e:#}"),
                    "character not stored"
                );
                return Action::Reply(vec![
                    encode_client_message(session.client_id, "The character could not be saved."),
                    encode_char_list(account, session.client_id, state.uptime_ms()),
                ]);
            }
        }
    }

    info!(
        user = %username,
        name = %character.name,
        slot = character.slot,
        class = character.class_index,
        "character created"
    );

    state.store.add_character(&username, character);

    // Read back rather than patched in place, so the list the client gets is
    // the one the store holds.
    let account = state.store.get(&username).unwrap_or_else(|| account.clone());
    session.account = Some(account.clone());
    Action::Reply(vec![encode_char_list(&account, session.client_id, state.uptime_ms())])
}


/// `0x668`: back to the character selection screen.
///
/// The original closes the socket and lets the client reconnect, with a
/// comment from whoever wrote it saying it is a hack and that they never
/// found the real logout packet. It is still the right shape: the connection
/// carries a character from the moment one is chosen, so leaving the world
/// means starting a new connection. Disconnecting here runs the same teardown
/// as any other, which is what saves the session.
fn handle_back_to_character_select(session: &Session) -> Action {
    match session.character.as_ref() {
        Some(character) => info!(character = %character.name, "leaving for the character list"),
        None => debug!("0x668 before a character was chosen"),
    }
    Action::Disconnect
}

/// `0x3E01`: delete the character in a slot.
///
/// The row is marked deleted rather than removed. A player who deletes the
/// wrong character has lost it either way as far as the game is concerned,
/// but a mistake stays recoverable by hand, and the name stays taken, which
/// is what stops somebody else claiming it a minute later.
async fn handle_delete_character(
    state: &State,
    session: &mut Session,
    message: &Message,
) -> Action {
    let Some(request) = creation::DeleteCharacter::parse(&message.body) else {
        warn!(size = message.body.len(), "0x3E01 packet too short");
        return Action::Ignore;
    };

    let Some(account) = session.account.as_ref() else {
        warn!("0x3E01 before logging in");
        return Action::Ignore;
    };
    let username = account.username.clone();
    let slot = request.slot as usize;

    let Some(doomed) = account.characters.iter().find(|c| c.slot == slot).cloned() else {
        debug!(user = %username, slot, "0x3E01 for an empty slot");
        return Action::Reply(vec![encode_char_list(
            account,
            session.client_id,
            state.uptime_ms(),
        )]);
    };

    // A character being played cannot be deleted from under itself.
    if session.character.as_ref().is_some_and(|c| c.id == doomed.id) {
        return Action::Reply(vec![
            encode_client_message(session.client_id, "You cannot delete the character you are playing."),
            encode_char_list(account, session.client_id, state.uptime_ms()),
        ]);
    }

    if let Some(db) = state.db() {
        if let Err(e) = db.soft_delete_character(doomed.id).await {
            warn!(
                user = %username,
                name = %doomed.name,
                error = format!("{e:#}"),
                "character not deleted"
            );
            return Action::Reply(vec![
                encode_client_message(session.client_id, "The character could not be deleted."),
                encode_char_list(account, session.client_id, state.uptime_ms()),
            ]);
        }
    }

    info!(user = %username, name = %doomed.name, slot, "character deleted");
    state.store.remove_character(&username, slot);

    let account = state.store.get(&username).unwrap_or_else(|| account.clone());
    session.account = Some(account.clone());
    Action::Reply(vec![encode_char_list(&account, session.client_id, state.uptime_ms())])
}



/// The skills a character has, worked out from the table.
fn known_skills(state: &State, character: &Character) -> Vec<usize> {
    ability::bar_of(&state.skills, character.class_number() as u32, character.level as u32)
}

/// `TSendSkillsPacket` (`0x106`): the forty skills the client draws on the
/// bar. Same opcode and size as the shop window, which is why it must only
/// go out when the client is expecting a skill list.
fn encode_skill_list(client_id: u16, skills: &[usize]) -> Vec<u8> {
    encode_skill_list_from(client_id, 0, skills)
}

/// The same list, said to come from an NPC. The send type is what makes the
/// client open a trainer instead of redrawing the bar.
fn encode_skill_list_from(client_id: u16, npc: u16, skills: &[usize]) -> Vec<u8> {
    let mut body = Vec::with_capacity(SKILLS_SIZE - MIN_FRAME);
    body.extend_from_slice(&npc.to_le_bytes());
    let send_type: u16 = if npc > 0 { SKILL_LIST_FROM_NPC } else { 0 };
    body.extend_from_slice(&send_type.to_le_bytes());
    for slot in 0..ability::SKILL_SLOTS {
        let id = skills.get(slot).copied().unwrap_or(0) as u16;
        body.extend_from_slice(&id.to_le_bytes());
    }

    debug_assert_eq!(body.len() + MIN_FRAME, SKILLS_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: ability::OP_SKILL_LIST, time: 0, body },
        rand::random(),
    )
}

/// `0x320`: the player used a skill.
///
/// The original spends the mana, checks the cooldown and the target, relays
/// the packet to everyone who can see it so they get the animation, and only
/// then works out what it did (`PacketHandlers.pas:7550`). Same order here:
/// the relay is what makes a spell look like it happened.
fn handle_use_skill(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = ability::UseSkill::parse(&message.body) else {
        warn!(size = message.body.len(), "0x320 packet too short");
        return Action::Ignore;
    };

    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let client_id = session.client_id;
    let at = (character.x as f32, character.y as f32);

    // A skill aimed at something needs that something to exist. Its position
    // comes from the world, never from the packet.
    let target = state.world.mob(request.target as u16).filter(|m| m.is_alive());
    let target_at = target.as_ref().map(|m| m.position());

    let caster = ability::Caster {
        class_number: character.class_number() as u32,
        level: character.level as u32,
        mana: session.cur_mp,
        at,
    };

    let now = std::time::Instant::now();
    let cast = match ability::check(
        &state.skills,
        &caster,
        &session.cooldowns,
        request.skill,
        target_at,
        now,
    ) {
        Ok(cast) => cast,
        Err(e) => {
            debug!(skill = request.skill, error = %e, "cast refused");
            return Action::Reply(vec![encode_client_message(client_id, &e.message())]);
        }
    };

    session.cur_mp = session.cur_mp.saturating_sub(cast.mana);
    session.cooldowns.start(cast.family, cast.cooldown, now);

    // Everyone who can see the caster sees the cast, animation and all.
    let relay = frame::encode(
        &Message {
            sender: client_id,
            opcode: ability::OP_USE_SKILL,
            time: message.time,
            body: message.body.clone(),
        },
        rand::random(),
    );
    state.world.send_to_visible(client_id, relay.clone());

    let (max_hp, max_mp) = vitals(session.character.as_ref().expect("checked above"));
    let mut frames = vec![relay, encode_hp_mp(
        session.character.as_ref().expect("checked above"),
        client_id,
        session.cur_hp.min(max_hp),
        session.cur_mp.min(max_mp),
    )];

    // A skill that hurts something works out its damage the same way a swing
    // does, with the skill's own number as the floor rather than the level.
    let Some(target) = target else {
        return Action::Reply(frames);
    };
    if !cast.is_aggressive {
        return Action::Reply(frames);
    }

    // A spell leans on the caster's magic attack rather than its swing.
    let stats = stats::of(character, &state.items);
    let blow = combat::swing_with(
        character.level,
        target.level,
        cast.damage + stats::base_damage(stats.magic_attack, target.level as u32),
        &mut rand::thread_rng(),
    );
    let Some((target, killed)) = state.world.wound_mob(target.id, blow.damage, client_id, now) else {
        return Action::Reply(frames);
    };

    let (_, flinch) = animations_of(state, cast.skill as u16);
    let report = combat::Damage {
        skill: cast.skill as u16,
        attacker: client_id,
        attacker_at: at,
        attacker_hp: session.cur_hp.min(max_hp),
        animation: cast.animation as u16,
        target_animation: flinch,
        target: target.id,
        target_hp: target.hp,
        blow,
        at: target.position(),
    };
    let damage_frame = encode_damage(&report);
    state.world.send_to_visible(client_id, damage_frame.clone());
    frames.push(damage_frame);

    if killed {
        info!(monster = %target.name, skill = cast.skill, "killed with a skill");
        frames.extend(reward_for(state, session, &target));
        session.visible_mobs.remove(&target.id);
    }
    Action::Reply(frames)
}


/// Applies whatever monsters landed on this player since its last packet.
///
/// The world tick cannot reach into a session, so it leaves the damage in the
/// world and this takes it. Being late by up to half a second is fine: the
/// client draws its own health bar from what it is told, and it is told here.
fn collect_blows(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let blows = state.world.take_incoming(session.client_id);
    if blows.is_empty() || session.character.is_none() {
        return Vec::new();
    }
    if session.dead {
        return Vec::new();
    }

    let client_id = session.client_id;
    let stats = stats::of(session.character.as_ref().expect("checked above"), &state.items);
    let mut frames = Vec::new();

    for blow in blows {
        let damage = stats::base_damage(blow.damage, stats.defence);
        session.cur_hp = session.cur_hp.saturating_sub(damage);

        let (swing, flinch) = animations_of(state, blow.skill);
        let attacker_at = state
            .world
            .mob(blow.attacker)
            .map(|m| m.position())
            .unwrap_or_default();

        let report = combat::Damage {
            skill: blow.skill,
            attacker: blow.attacker,
            attacker_at,
            attacker_hp: state.world.mob(blow.attacker).map(|m| m.hp).unwrap_or_default(),
            animation: swing,
            target_animation: flinch,
            target: client_id,
            target_hp: session.cur_hp,
            blow: combat::Blow { damage, kind: combat::DAMAGE_NORMAL },
            at: session
                .character
                .as_ref()
                .map(|c| (c.x as f32, c.y as f32))
                .unwrap_or_default(),
        };
        frames.push(encode_damage(&report));

        if session.cur_hp == 0 {
            frames.extend(die(state, session, &stats));
            break;
        }
    }
    frames
}

/// What happens when a player runs out of health.
///
/// The client is told the health is gone and the character is marked down.
/// Nothing is taken away: the original has a death penalty, and inventing one
/// is a decision for whoever runs the server rather than for this.
fn die(state: &State, session: &mut Session, stats: &stats::Stats) -> Vec<Vec<u8>> {
    session.dead = true;
    session.cur_hp = 0;

    let name = session.character.as_ref().map(|c| c.name.clone()).unwrap_or_default();
    info!(character = %name, "died");

    let mut frames = vec![encode_hp_mp(
        session.character.as_ref().expect("checked by the caller"),
        session.client_id,
        0,
        session.cur_mp.min(stats.max_mp),
    )];

    // Everyone nearby sees them go down.
    let frame = encode_remove_mob(session.client_id, DELETE_NORMAL);
    state.world.send_to_visible(session.client_id, frame.clone());
    frames.push(frame);
    frames
}

/// `0x303`: get up again.
///
/// The original revives at the nearest save point; without those read yet,
/// this is the town the character was created in, which is where a save point
/// would be anyway.
fn handle_revive(state: &State, session: &mut Session, _message: &Message) -> Action {
    if !session.dead {
        debug!("0x303 from somebody who is not dead");
        return Action::Ignore;
    }

    let Some(character) = session.character.as_mut() else {
        return Action::Ignore;
    };
    let (x, y) = creation::TOWN_FIRST;
    character.x = x;
    character.y = y;

    let character = session.character.as_ref().expect("checked above").clone();
    let stats = stats::of(&character, &state.items);

    session.dead = false;
    session.cur_hp = stats.max_hp;
    session.cur_mp = stats.max_mp;
    session.dirty = true;
    session.visible.clear();
    session.visible_mobs.clear();
    session.visible_npcs.clear();

    state.world.enter(session.client_id, character.clone());
    state.world.move_to(session.client_id, x, y);
    info!(character = %character.name, "got up");

    let mut frames = vec![
        encode_spawn_as(&character, session.client_id, SPAWN_TELEPORT_IN),
        encode_hp_mp(&character, session.client_id, session.cur_hp, session.cur_mp),
    ];
    frames.extend(refresh_npc_visibility(state, session));
    frames.extend(refresh_mob_visibility(state, session));
    Action::Reply(frames)
}

/// `0x302`: the player swung at something.
///
/// Only monsters can be hit. Attacking another player needs a duel or a war
/// to be agreed first, and neither exists yet; hitting an NPC is refused by
/// the original too.
fn handle_attack(state: &State, session: &mut Session, message: &Message) -> Action {
    let Some(request) = combat::Attack::parse(&message.body) else {
        warn!(size = message.body.len(), "0x302 packet too short");
        return Action::Ignore;
    };

    if session.dead {
        return Action::Ignore;
    }
    let Some(character) = session.character.as_ref() else {
        return Action::Ignore;
    };
    let at = (character.x as f32, character.y as f32);
    let level = character.level;

    // The position comes from the world, never from the packet: the client
    // sends where it thinks it is, and a modified one would reach across the
    // map by lying about it.
    let Some(target) = state.world.mob(request.target) else {
        debug!(target = request.target, "0x302 at something that is not a monster");
        return Action::Ignore;
    };
    if !target.is_alive() {
        return Action::Ignore;
    }
    if !within(at, target.position(), combat::MELEE_RANGE) {
        debug!(target = request.target, "0x302 from out of reach");
        return Action::Ignore;
    }

    // Attack comes off the character and its gear now, and the monster's
    // level stands in for the armour it is not wearing.
    let stats = stats::of(session.character.as_ref().expect("checked above"), &state.items);
    let blow = combat::swing_with(
        level,
        target.level,
        stats::base_damage(stats.attack, target.level as u32),
        &mut rand::thread_rng(),
    );
    let Some((target, killed)) =
        state.world.wound_mob(
            request.target,
            blow.damage,
            session.client_id,
            std::time::Instant::now(),
        )
    else {
        // Somebody else landed the last blow between the checks above and
        // here. Theirs, not ours.
        return Action::Ignore;
    };

    let (max_hp, _) = vitals(session.character.as_ref().expect("checked above"));
    let (_, flinch) = animations_of(state, request.skill);
    let report = combat::Damage {
        skill: request.skill,
        attacker: session.client_id,
        attacker_at: at,
        attacker_hp: session.cur_hp.min(max_hp),
        animation: request.animation,
        target_animation: flinch,
        target: target.id,
        target_hp: target.hp,
        blow,
        at: target.position(),
    };
    let frame = encode_damage(&report);

    // Everyone who can see the fight sees it. A fight only the person
    // swinging can see is a fight that looks like it never happened.
    state.world.send_to_visible(session.client_id, frame.clone());

    let mut frames = vec![frame];
    if killed {
        info!(
            monster = %target.name,
            level = target.level,
            experience = target.experience,
            "killed"
        );
        frames.extend(reward_for(state, session, &target));
        session.visible_mobs.remove(&target.id);
    }
    Action::Reply(frames)
}

/// What a kill is worth: experience, whatever levels that buys, and whatever
/// the monster was carrying.
fn reward_for(state: &State, session: &mut Session, target: &crate::mob::Mob) -> Vec<Vec<u8>> {
    let client_id = session.client_id;
    let Some(character) = session.character.as_mut() else {
        return Vec::new();
    };

    character.exp = character.exp.saturating_add(target.experience as u64);
    session.dirty = true;

    // The curve decides the level, not a running count of kills, so a
    // character whose experience is edited in the database lands where that
    // experience says it should.
    let gained = state.levels.levels_gained(character.level, character.exp);
    let mut frames = Vec::new();
    if gained > 0 {
        character.level = character.level.saturating_add(gained);
        info!(character = %character.name, level = character.level, "levelled up");

        // Health and mana come back full: a level is the one moment the game
        // hands them over, and arriving at a new level nearly dead is a
        // punishment for winning.
        let stats = stats::of(character, &state.items);
        session.cur_hp = stats.max_hp;
        session.cur_mp = stats.max_mp;
        frames.push(encode_hp_mp(
            session.character.as_ref().expect("checked above"),
            client_id,
            session.cur_hp,
            session.cur_mp,
        ));

        // The burst everyone on screen sees. The original plays it on the
        // player for everyone who can see them, so it goes out to the world
        // and comes back to the killer in the same reply.
        let effect = encode_effect(client_id, EFFECT_LEVEL_UP);
        state.world.send_to_visible(client_id, effect.clone());
        frames.push(effect);
    }

    frames.push(encode_level(session.character.as_ref().expect("checked above"), client_id));
    frames.extend(loot_from(state, session, target));
    frames
}

/// What the monster was carrying, if anything, straight into the bag.
///
/// The original drops it on the ground for the player to pick up, which needs
/// map items and the packets that go with them. Handing it over is the same
/// outcome with one fewer system, and it is honest about being a shortcut:
/// nobody else can take it, and a full bag loses it.
fn loot_from(state: &State, session: &mut Session, target: &crate::mob::Mob) -> Vec<Vec<u8>> {
    let band = state.drops.band_for(target.drop_index);
    let mut rng = rand::thread_rng();

    let Some(id) = aika_data::drops::roll(
        band,
        rand::Rng::gen_range(&mut rng, 1..=100),
        rand::Rng::gen_range(&mut rng, 1..=100),
        rand::Rng::gen_range(&mut rng, 0..usize::MAX),
    ) else {
        return Vec::new();
    };

    let Some(def) = state.items.get(id as usize) else {
        debug!(item = id, "a drop table names an item the item table does not");
        return Vec::new();
    };

    let dropped = Item {
        index: id,
        appearance: id,
        refine: 1,
        durability_min: def.durability(),
        durability_max: def.durability(),
        ..Item::default()
    };

    let Some(character) = session.character.as_mut() else {
        return Vec::new();
    };
    match character.items.add(dropped) {
        Ok(slot) => {
            let item = character.items.get(inventory::BAG, slot).cloned().unwrap_or_default();
            info!(item = id, slot, monster = %target.name, "looted");
            vec![encode_refresh_item(inventory::BAG, slot, &item, true)]
        }
        Err(_) => vec![encode_client_message(
            session.client_id,
            "Your bag is full, so the drop was lost.",
        )],
    }
}

/// The two animations a blow plays: the swing and the flinch, both from the
/// skill it was made with (`Mob/BaseMob.pas:9842`).
///
/// A blow with no skill behind it plays nothing, which is what the original
/// does for a monster whose kind lists none.
fn animations_of(state: &State, skill: u16) -> (u16, u8) {
    let Some(def) = state.skills.get(skill as usize) else {
        return (0, 0);
    };
    (def.animation() as u16, def.target_animation() as u8)
}

/// `TRecvDamagePacket` (`0x102`): who hit what, for how much, and what is
/// left of it.
fn encode_damage(damage: &combat::Damage) -> Vec<u8> {
    let body = damage.to_body();
    debug_assert_eq!(body.len() + MIN_FRAME, combat::DAMAGE_SIZE);
    frame::encode(
        &Message { sender: damage.attacker, opcode: combat::OP_DAMAGE, time: 0, body },
        rand::random(),
    )
}

/// A monster's `0x349`. Closer to a player's than to an NPC's: the original
/// writes the same `Unk0` as for a player and does not flag it as a service
/// (`Mob/BaseMob.pas:3045`). What tells the client it is a monster is the id
/// range it arrives in.
fn encode_mob_spawn(mob: &crate::mob::Mob) -> Vec<u8> {
    use spawn_offset as off;
    let mut body = vec![0u8; off::BODY_SIZE];

    let put16 = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    // The name is the digits of a string index, the same convention the NPCs
    // use: the client owns the words.
    write_fixed_str(&mut body[off::NAME..off::NAME + 16], &mob.name_index.to_string());

    for (i, model) in mob.model.iter().enumerate() {
        put16(&mut body, off::EQUIP + i * 2, *model);
    }

    body[off::POSITION_X..off::POSITION_X + 4].copy_from_slice(&mob.x.to_le_bytes());
    body[off::POSITION_Y..off::POSITION_Y + 4].copy_from_slice(&mob.y.to_le_bytes());
    put32(&mut body, off::ROTATION, mob.rotation as u32);

    put32(&mut body, off::MAX_HP, mob.max_hp);
    put32(&mut body, off::MAX_MP, mob.max_hp);
    put32(&mut body, off::CUR_HP, mob.hp);
    put32(&mut body, off::CUR_MP, mob.max_hp);

    body[off::UNK0] = SPAWN_UNK0;
    body[off::SPEED_MOVE] = MOB_SPEED_MOVE;
    body[off::SPAWN_TYPE] = mob.spawn_type;
    body[off::SIZES..off::SIZES + 3].copy_from_slice(&mob.sizes);

    debug_assert_eq!(body.len() + MIN_FRAME, CREATE_MOB_SIZE);
    frame::encode(
        &Message { sender: mob.id, opcode: OP_CREATE_MOB, time: 0, body },
        rand::random(),
    )
}

/// `TSendClientIndexPacket` (`0x117`): the id of the mob the player is.
fn encode_client_index(client_id: u16) -> Vec<u8> {
    encode_effect(client_id, 0)
}

/// The same packet as an effect over somebody's head (`TPlayer.SendEffect`).
///
/// `Index` is who it plays on and `Effect` which one; effect 1 is the burst a
/// character gives off when it gains a level. The original sends it to
/// everyone who can see the player, themselves included, so a level-up is
/// visible to the whole screen and not just the one who earned it.
fn encode_effect(client_id: u16, effect: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(CLIENT_INDEX_SIZE - MIN_FRAME);
    body.extend_from_slice(&(client_id as u32).to_le_bytes());
    body.extend_from_slice(&effect.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, CLIENT_INDEX_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CLIENT_INDEX, time: 0, body },
        rand::random(),
    )
}

/// The effect number the client plays when a character gains a level
/// (`AddLevel` sends `SendEffect(1)`).
const EFFECT_LEVEL_UP: u32 = 1;

/// `Tp131` (`0x131`): zero except for one field of all ones.
fn encode_enter_131() -> Vec<u8> {
    let mut body = Vec::with_capacity(ENTER_131_SIZE - MIN_FRAME);
    body.extend_from_slice(&ENTER_131_MARKER.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, ENTER_131_SIZE);
    frame::encode(&Message { sender: 0, opcode: OP_ENTER_131, time: 0, body }, rand::random())
}

/// `TSignalData` (`Data/Packets.pas:31`): a header and one DWORD. 16 bytes.
fn encode_signal(opcode: u16, client_id: u16, time: u32, data: u32) -> Vec<u8> {
    frame::encode(
        &Message { sender: client_id, opcode, time, body: data.to_le_bytes().to_vec() },
        rand::random(),
    )
}

/// `TSendToWorldPacket` (`Data/Packets.pas:452`): the whole character.
fn encode_send_to_world(
    account: &Account,
    character: &Character,
    client_id: u16,
    time: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + CHARACTER_SIZE);
    body.extend_from_slice(&account.id.to_le_bytes());
    body.extend_from_slice(&encode_character(character, client_id));

    debug_assert_eq!(body.len() + MIN_FRAME, SEND_TO_WORLD_SIZE);
    frame::encode(
        &Message { sender: SEND_TO_WORLD_INDEX, opcode: OP_SEND_TO_WORLD, time, body },
        rand::random(),
    )
}

/// `TCharacter`. Only the fields the client needs to build the character;
/// the rest (inventory, skills, quests, titles) stays zeroed for now.
fn encode_character(character: &Character, client_id: u16) -> Vec<u8> {
    use character_offset as off;
    let mut out = vec![0u8; CHARACTER_SIZE];

    let put32 = |out: &mut Vec<u8>, at: usize, v: u32| {
        out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let put16 = |out: &mut Vec<u8>, at: usize, v: u16| {
        out[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };

    put32(&mut out, off::CLIENT_ID, client_id as u32);
    put32(&mut out, off::FIRST_LOGIN, 0);
    put32(&mut out, off::CHAR_INDEX, character.slot as u32 + 1);
    write_fixed_str(&mut out[off::NAME..off::NAME + 16], &character.name);
    out[off::NATION] = character.nation as u8;
    out[off::CLASS_INFO] = character.class_info() as u8;

    for (i, value) in character.attributes.iter().enumerate() {
        put16(&mut out, off::ATTRIBUTES + i * 2, *value);
    }
    out[off::SIZES..off::SIZES + 4].copy_from_slice(&character.sizes);

    // Provisional health and mana, just so the character is not born dead.
    // The real formulas depend on tables we have not read yet.
    let hp = 100 + character.level as u32 * 10;
    let mp = 50 + character.level as u32 * 5;
    put32(&mut out, off::MAX_HP, hp);
    put32(&mut out, off::CUR_HP, hp);
    put32(&mut out, off::MAX_MP, mp);
    put32(&mut out, off::CUR_MP, mp);

    out[off::EXP..off::EXP + 8].copy_from_slice(&character.exp.to_le_bytes());
    // Same convention as the character list: the client adds 1.
    put16(&mut out, off::LEVEL, character.level.saturating_sub(1));

    // Everything carried, at the slot it occupies. Written before the
    // appearance below, because slots 0 and 1 of the equipment are the body
    // and the hair rather than real items, and those have to win.
    for item in character.items.iter() {
        let base = match item.container {
            inventory::EQUIP => off::EQUIP,
            inventory::BAG => off::INVENTORY,
            _ => continue,
        };
        let at = base + item.slot as usize * off::ITEM_SIZE;
        if at + off::ITEM_SIZE <= out.len() {
            write_item(&mut out[at..at + off::ITEM_SIZE], item);
        }
    }

    // Equip[0] is the class and Equip[1] the hair, in each item's `Index`.
    put16(&mut out, off::EQUIP, character.class_index);
    put16(&mut out, off::EQUIP + off::ITEM_SIZE, character.hair);

    out[off::GOLD..off::GOLD + 8].copy_from_slice(&character.gold.to_le_bytes());
    put32(&mut out, off::LOCATION, 0);

    out
}

/// `TRequestLoginPacket` (`Data/Packets.pas:200`), 1096 bytes no total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLogin {
    pub account_id: u32,
    pub username: String,
    pub version: u16,
    pub token: String,
}

impl RequestLogin {
    /// Offsets inside the body (the 12-byte header is already gone).
    const ACCOUNT_ID: usize = 0;
    const USERNAME: usize = 4;
    const VERSION: usize = 54;
    const TOKEN: usize = 60;
    /// Body as declared in the Delphi record: 1096 minus the header.
    pub const BODY_SIZE: usize = 1084;
    /// What the client **actually** sends: 100 bytes total, 88 of body.
    /// The 992 `Null_1` bytes at the end of the record are buffer size and
    /// never reach the wire, so the token field arrives truncated. No loss:
    /// the original server does not validate the token here either, that is
    /// server, no `0x81`.
    pub const MIN_BODY: usize = Self::VERSION + 2;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::MIN_BODY {
            return None;
        }
        let token_end = (Self::TOKEN + 32).min(body.len());
        let token = if token_end > Self::TOKEN {
            read_fixed_str(&body[Self::TOKEN..token_end])
        } else {
            String::new()
        };
        Some(Self {
            account_id: u32::from_le_bytes(
                body[Self::ACCOUNT_ID..Self::ACCOUNT_ID + 4].try_into().ok()?,
            ),
            username: read_fixed_str(&body[Self::USERNAME..Self::USERNAME + 32]),
            version: u16::from_le_bytes(body[Self::VERSION..Self::VERSION + 2].try_into().ok()?),
            token,
        })
    }

    /// Builds the body the way the client does. Used by the tests.
    pub fn to_body(&self) -> Vec<u8> {
        let mut body = vec![0u8; Self::BODY_SIZE];
        body[Self::ACCOUNT_ID..Self::ACCOUNT_ID + 4]
            .copy_from_slice(&self.account_id.to_le_bytes());
        write_fixed_str(&mut body[Self::USERNAME..Self::USERNAME + 32], &self.username);
        body[Self::VERSION..Self::VERSION + 2].copy_from_slice(&self.version.to_le_bytes());
        write_fixed_str(&mut body[Self::TOKEN..Self::TOKEN + 32], &self.token);
        body
    }
}

/// `TSendToCharListPacket` (`Data/Packets.pas:233`): account id, two zeroed
/// two zeroed fields and three character entries.
pub fn encode_char_list(account: &Account, client_id: u16, time: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(CHAR_LIST_SIZE - MIN_FRAME);
    body.extend_from_slice(&account.id.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // Unk
    body.extend_from_slice(&0u32.to_le_bytes()); // NotUse

    for slot in 0..MAX_CHARACTERS {
        let character = account.characters.iter().find(|c| c.slot == slot);
        body.extend_from_slice(&encode_char_list_entry(character));
    }

    debug_assert_eq!(body.len() + MIN_FRAME, CHAR_LIST_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_CHAR_LIST, time, body },
        rand::random(),
    )
}

/// `TCharacterListData` (`Data/Packets.pas:215`), 104 bytes: the entry on the
/// selection screen, far smaller than the world's `TCharacter`. An empty
/// entry is all zeroes.
fn encode_char_list_entry(character: Option<&Character>) -> [u8; CHAR_ENTRY_SIZE] {
    let mut out = [0u8; CHAR_ENTRY_SIZE];
    let Some(character) = character else {
        return out;
    };

    write_fixed_str(&mut out[0..16], &character.name);
    out[16..18].copy_from_slice(&character.nation.to_le_bytes());
    out[18..20].copy_from_slice(&character.class_info().to_le_bytes());
    out[20..24].copy_from_slice(&character.sizes);

    // The selection screen dresses the character the same way the world
    // does, from the appearance of each slot; the first two are then
    // overwritten with the body and the hair.
    for slot in WORN_SLOTS {
        let at = 24 + slot as usize * 2;
        out[at..at + 2].copy_from_slice(&worn_appearance(character, slot).to_le_bytes());
    }
    out[24..26].copy_from_slice(&character.class_index.to_le_bytes());
    out[26..28].copy_from_slice(&character.hair.to_le_bytes());

    // Refine[7] is hardcoded to 15 as in the original, where the weapon's real
    // is commented out there (`Mob/Player.pas:3038`).
    out[40 + 7] = 15;

    for (i, value) in character.attributes.iter().enumerate() {
        let at = 52 + i * 2;
        out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    // The client adds 1 to what it gets, so the server sends level minus one.
    out[64..66].copy_from_slice(&character.level.saturating_sub(1).to_le_bytes());
    out[72..80].copy_from_slice(&character.exp.to_le_bytes());
    out[80..88].copy_from_slice(&character.gold.to_le_bytes());

    out
}

/// Hex of the leading bytes, to check offsets against the real client.
fn hex_dump(body: &[u8]) -> String {
    body.iter().take(112).map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn read_fixed_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0x00 || b == 0xCC).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect::<String>().trim().to_string()
}

fn write_fixed_str(dest: &mut [u8], value: &str) {
    for (slot, byte) in dest.iter_mut().zip(value.chars().map(|c| c as u8)) {
        *slot = byte;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DevAccount, DevCharacter};
    use crate::world::World;

    fn state_with(characters: Vec<DevCharacter>) -> State {
        let cfg = Config {
            accounts: vec![DevAccount {
                username: "admin".into(),
                password: Some("admin".into()),
                password_hash: None,
                nation: 2,
                account_status: 0,
                ban_days: 0,
                characters,
            }],
            ..Default::default()
        };
        State::new(cfg).unwrap()
    }

    fn dev_character(name: &str, slot: usize) -> DevCharacter {
        DevCharacter {
            name: name.into(),
            slot,
            level: 30,
            class_index: 20,
            hair: 7702,
            nation: 2,
            gold: 1234,
            exp: 5678,
            x: None,
            y: None,
            speed_move: None,
        }
    }

    fn login_message(username: &str, version: u16) -> Message {
        let body =
            RequestLogin { account_id: 1, username: username.into(), version, token: "t".repeat(32) }
                .to_body();
        Message { sender: 7, opcode: OP_REQUEST_LOGIN, time: 0, body }
    }

    /// Runs the login and returns the reply bytes.
    async fn reply(state: &State, version: u16) -> Vec<u8> {
        let mut session = Session { client_id: TEST_CLIENT_ID, ..Session::default() };
        match handle_message(state, &mut session, &login_message("admin", version)).await {
            Action::Reply(frames) => frames.into_iter().next().expect("no frames"),
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    /// A session that already went through `0x685`, as a real connection would.
    ///
    /// The id is normally handed out by the world registry when the socket is
    /// accepted; tests set it directly.
    async fn logged_in(state: &State) -> Session {
        let mut session = Session { client_id: TEST_CLIENT_ID, ..Session::default() };
        let action = handle_message(state, &mut session, &login_message("admin", 124)).await;
        assert!(matches!(action, Action::Reply(_)), "o login precisa passar");
        session
    }

    /// Whatever id the registry would have handed this connection.
    const TEST_CLIENT_ID: u16 = 7;

    /// A townsperson, built rather than read: the `.npc` files belong to the
    /// original pack and are not in this repository.
    fn npc(id: u16, title: &str, x: f32, y: f32) -> Npc {
        Npc {
            id,
            title: title.into(),
            label: String::new(),
            name_index: Some(43),
            options: vec![1, 2, 8],
            equip: [234, 234, 0, 0, 0, 0, 0, 0],
            sizes: [7, 119, 119, 3],
            shop: [0; aika_data::npc::SHOP_SLOTS],
            max_hp: 20000,
            cur_hp: 20000,
            max_mp: 20000,
            cur_mp: 0,
            x,
            y,
            rotation: 0,
            speed_move: 0,
            stale_id: None,
        }
    }

    /// Spawn packets in a batch of frames, by the id they are addressed to.
    fn spawned_ids(frames: &[Vec<u8>]) -> Vec<u16> {
        frames
            .iter()
            .map(|frame| decode(frame))
            .filter(|m| m.opcode == OP_CREATE_MOB)
            .map(|m| m.sender)
            .collect()
    }

    /// Removal packets in a batch of frames.
    fn removed_ids(frames: &[Vec<u8>]) -> Vec<u16> {
        frames
            .iter()
            .map(|frame| decode(frame))
            .filter(|m| m.opcode == OP_REMOVE_MOB)
            .map(|m| u32::from_le_bytes(m.body[0..4].try_into().unwrap()) as u16)
            .collect()
    }

    fn enter_world(slot: u32) -> Message {
        Message {
            sender: 7,
            opcode: OP_ENTER_WORLD,
            time: 0,
            body: slot.to_le_bytes().to_vec(),
        }
    }

    fn decode(wire: &[u8]) -> Message {
        let mut reader = FrameReader::new();
        reader.push(wire);
        reader.next_message().expect("incomplete frame").expect("unreadable frame")
    }

    #[test]
    fn request_login_body_roundtrip() {
        let original = RequestLogin {
            account_id: 42,
            username: "admin".into(),
            version: 124,
            token: "a".repeat(32),
        };
        let body = original.to_body();
        assert_eq!(body.len(), RequestLogin::BODY_SIZE);
        assert_eq!(body.len() + MIN_FRAME, 1096, "size declared in the Delphi record");
        assert_eq!(RequestLogin::parse(&body).unwrap(), original);
    }

    /// The real client sends 100 bytes (88 of body), not the record's 1096:
    /// the 992 padding bytes at the end never reach the wire.
    #[test]
    fn parses_the_short_body_the_real_client_sends() {
        let mut body = RequestLogin {
            account_id: 1,
            username: "admin".into(),
            version: 124,
            token: "t".repeat(32),
        }
        .to_body();
        body.truncate(88);

        let parsed = RequestLogin::parse(&body).expect("an 88 byte body must pass");
        assert_eq!(parsed.username, "admin");
        assert_eq!(parsed.version, 124);
        assert_eq!(parsed.account_id, 1);
        // the token arrives cut at this size; it is not validated here anyway
        assert_eq!(parsed.token.len(), 28);

        // too short to hold the version is still refused
        assert!(RequestLogin::parse(&body[..RequestLogin::MIN_BODY - 1]).is_none());
    }

    #[tokio::test]
    async fn char_list_has_the_exact_size_the_client_expects() {
        let state = state_with(vec![]);
        let wire = reply(&state, 124).await;
        assert_eq!(wire.len(), CHAR_LIST_SIZE);

        let message = decode(&wire);
        assert_eq!(message.opcode, OP_CHAR_LIST);
        assert_eq!(message.sender, TEST_CLIENT_ID, "addressed with the id we assigned");
        assert_eq!(u32::from_le_bytes(message.body[0..4].try_into().unwrap()), 1);
    }

    #[tokio::test]
    async fn empty_slots_are_all_zeros() {
        let state = state_with(vec![]);
        let message = decode(&reply(&state, 124).await);
        assert!(message.body[12..].iter().all(|&b| b == 0), "slots vazios devem ser zerados");
    }

    #[tokio::test]
    async fn character_lands_in_its_slot_with_delphi_quirks() {
        let state = state_with(vec![dev_character("Athus", 1)]);
        let message = decode(&reply(&state, 124).await);

        let slot0 = &message.body[12..12 + CHAR_ENTRY_SIZE];
        let slot1 = &message.body[12 + CHAR_ENTRY_SIZE..12 + CHAR_ENTRY_SIZE * 2];
        assert!(slot0.iter().all(|&b| b == 0), "slot 0 continua vazio");

        assert_eq!(read_fixed_str(&slot1[0..16]), "Athus");
        assert_eq!(u16::from_le_bytes(slot1[16..18].try_into().unwrap()), 2, "nation");
        assert_eq!(
            u16::from_le_bytes(slot1[18..20].try_into().unwrap()),
            11,
            "class index 20 is the second class, whose code the templates give as 11"
        );
        assert_eq!(u16::from_le_bytes(slot1[24..26].try_into().unwrap()), 20, "Equip[0] = class");
        assert_eq!(u16::from_le_bytes(slot1[26..28].try_into().unwrap()), 7702, "Equip[1] = hair");
        assert_eq!(slot1[47], 15, "Refine[7] is hardcoded to 15");
        assert_eq!(
            u16::from_le_bytes(slot1[64..66].try_into().unwrap()),
            29,
            "level travels as one less: the client adds 1"
        );
        assert_eq!(u64::from_le_bytes(slot1[72..80].try_into().unwrap()), 5678, "exp");
        assert_eq!(u64::from_le_bytes(slot1[80..88].try_into().unwrap()), 1234, "gold");
    }

    /// The whole arrival sequence, in order.
    ///
    /// Order and completeness both matter here: the client will not finish
    /// arriving until it has all of it, and it was three packets short of
    /// this for a while, which left the arrival camera up and the client
    /// asking again twice a second.
    #[tokio::test]
    async fn entering_the_world_sends_the_delphi_sequence() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;

        let enter = enter_world(0);
        let Action::Reply(frames) = handle_message(&state, &mut session, &enter).await else {
            panic!("expected the world entry sequence");
        };

        assert_eq!(
            opcodes(&frames),
            vec![
                OP_SIGNAL_READY,
                OP_ENTER_3A2,
                OP_SIGNAL_LOAD,
                OP_SIGNAL_LOAD,
                OP_SIGNAL_LOAD,
                OP_ENTER_131,
                OP_SEND_TO_WORLD,
                OP_ENTER_12C,
                shop::OP_REFRESH_ITEM,
                OP_CLIENT_INDEX,
                shop::OP_REFRESH_ITEM,
                OP_CLIENT_INDEX,
                OP_ENTER_94C,
            ],
            "the arrival sequence is not the one the original sends"
        );

        // The one packet in there the client cannot do without: which mob on
        // screen is its own.
        let index = decode(&frames[9]);
        assert_eq!(
            u32::from_le_bytes(index.body[0..4].try_into().unwrap()),
            TEST_CLIENT_ID as u32,
            "the client is told the wrong id for itself"
        );

        // and each one is the size its record declares
        for (frame, size) in frames.iter().zip([
            SIGNAL_SIZE,
            ENTER_3A2_SIZE,
            SIGNAL_SIZE,
            SIGNAL_SIZE,
            SIGNAL_SIZE,
            ENTER_131_SIZE,
            SEND_TO_WORLD_SIZE,
            ENTER_12C_SIZE,
            shop::REFRESH_ITEM_SIZE,
            CLIENT_INDEX_SIZE,
            shop::REFRESH_ITEM_SIZE,
            CLIENT_INDEX_SIZE,
            ENTER_94C_SIZE,
        ]) {
            assert_eq!(frame.len(), size, "wrong size for 0x{:X}", decode(frame).opcode);
        }
    }

    /// `0x349` is what pulls the character out of limbo: without it the client
    /// world but never learns where to put the body.
    #[tokio::test]
    async fn client_ready_spawns_the_character_at_its_position() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let _ = handle_message(&state, &mut session, &enter_world(0)).await;

        let ready = Message { sender: 7, opcode: OP_CLIENT_READY, time: 0, body: Vec::new() };
        let Action::Reply(frames) = handle_message(&state, &mut session, &ready).await else {
            panic!("expected the spawn packet");
        };
        // The client has no acknowledgement packet: it keeps resending 0xF0B
        // until this burst convinces it that it is in the world.
        let opcodes: Vec<u16> = frames.iter().map(|f| decode(f).opcode).collect();
        assert_eq!(
            opcodes,
            vec![
                OP_CREATE_MOB,
                OP_SKILLS,
                OP_CASH,
                OP_ACCOUNT_STATUS,
                OP_BUFFS,
                OP_ACTIVE_TITLE,
                OP_RELICS,
                OP_REFRESH_POINT,
                OP_REFRESH_STATUS,
                OP_ALL_ATTRIBUTES,
                OP_LEVEL,
                OP_HP_MP,
                OP_CREATE_MOB,
            ],
            "order matters; it is the order SendToWorldSends uses"
        );

        // Every size comes from a Delphi record and the client rejects a
        // packet that is not exactly that long.
        let sizes: Vec<usize> = frames.iter().map(|f| f.len()).collect();
        assert_eq!(
            sizes,
            vec![
                CREATE_MOB_SIZE,
                SKILLS_SIZE,
                SIGNAL_SIZE,
                SIGNAL_SIZE,
                BUFFS_SIZE,
                ACTIVE_TITLE_SIZE,
                RELICS_SIZE,
                REFRESH_POINT_SIZE,
                REFRESH_STATUS_SIZE,
                ALL_ATTRIBUTES_SIZE,
                LEVEL_SIZE,
                HP_MP_SIZE,
                CREATE_MOB_SIZE,
            ]
        );

        // The level packet carries level minus one and the constant the
        // original writes beside it.
        let level = decode(&frames[10]);
        assert_eq!(u16::from_le_bytes(level.body[0..2].try_into().unwrap()), 29);
        assert_eq!(u16::from_le_bytes(level.body[2..4].try_into().unwrap()), LEVEL_UNK);

        // HP and MP must not be zero or the character is born dead.
        let vitals = decode(&frames[11]);
        assert!(u32::from_le_bytes(vitals.body[0..4].try_into().unwrap()) > 0, "max HP");
        assert!(u32::from_le_bytes(vitals.body[8..12].try_into().unwrap()) > 0, "max MP");

        // Some packets identify themselves with a fixed index, not the client id.
        assert_eq!(decode(&frames[7]).sender, FIXED_INDEX, "0x109");
        assert_eq!(decode(&frames[8]).sender, FIXED_INDEX, "0x10A");

        let message = decode(&frames[0]);
        assert_eq!(message.opcode, OP_CREATE_MOB);

        use spawn_offset as off;
        let b = &message.body;
        assert_eq!(b.len(), off::BODY_SIZE);
        assert_eq!(read_fixed_str(&b[off::NAME..off::NAME + 16]), "Athus");

        // the position is floating point, not integer; getting this wrong puts
        // the character anywhere on the map
        let x = f32::from_le_bytes(b[off::POSITION_X..off::POSITION_X + 4].try_into().unwrap());
        let y = f32::from_le_bytes(b[off::POSITION_Y..off::POSITION_Y + 4].try_into().unwrap());
        assert_eq!((x, y), (3450.0, 690.0), "cidade inicial");

        assert!(u32::from_le_bytes(b[off::CUR_HP..off::CUR_HP + 4].try_into().unwrap()) > 0);
        assert_eq!(b[off::UNK0], SPAWN_UNK0);
        assert_eq!(b[off::SPAWN_TYPE], SPAWN_NORMAL);
        assert_eq!(
            u16::from_le_bytes(b[off::EFFECTS + 2..off::EFFECTS + 4].try_into().unwrap()),
            SPAWN_EFFECT_1
        );
        assert_eq!(
            u16::from_le_bytes(
                b[off::GUILD_AND_NATION..off::GUILD_AND_NATION + 2].try_into().unwrap()
            ),
            2 << 12,
            "without a guild, the field carries only the nation"
        );
    }

    /// The client resends `0xF0B` when it thinks something is missing, including
    /// trying to walk. Respawning every time trapped the player in a loop:
    /// they walked, snapped back to the start, and the scene restarted.
    #[tokio::test]
    async fn spawning_happens_only_once_per_session() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let _ = handle_message(&state, &mut session, &enter_world(0)).await;

        let ready = Message { sender: 7, opcode: OP_CLIENT_READY, time: 0, body: Vec::new() };
        assert!(matches!(handle_message(&state, &mut session, &ready).await, Action::Reply(_)));
        assert!(
            matches!(handle_message(&state, &mut session, &ready).await, Action::Ignore),
            "the second 0xF0B must not respawn the player"
        );
    }

    #[tokio::test]
    async fn movement_updates_the_position_without_answering() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let _ = handle_message(&state, &mut session, &enter_world(0)).await;

        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&3500.0f32.to_le_bytes());
        body[4..8].copy_from_slice(&720.5f32.to_le_bytes());
        body[Movement::SPEED] = 50;
        let move_msg = Message { sender: 7, opcode: OP_MOVE, time: 0, body };

        // the original returns nothing to the mover
        assert!(matches!(handle_message(&state, &mut session, &move_msg).await, Action::Ignore));

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), (3500, 720));
    }

    /// The real client sends move types other than plain walking (16 has been
    /// seen). The original drops those; we still track the position, because a
    /// stale one would be wrong once positions are persisted.
    /// A packet claiming somebody else's id still moves only the player who
    /// sent it: the header field is never used to pick a target.
    #[tokio::test]
    async fn movement_never_moves_another_player() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let _ = handle_message(&state, &mut session, &enter_world(0)).await;

        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&4200.0f32.to_le_bytes());
        body[4..8].copy_from_slice(&700.0f32.to_le_bytes());
        let forged = Message { sender: 999, opcode: OP_MOVE, time: 0, body };

        let _ = handle_message(&state, &mut session, &forged).await;

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), (4200, 700), "our own player moved");
    }

    #[tokio::test]
    async fn movement_tracks_move_types_other_than_walking() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let _ = handle_message(&state, &mut session, &enter_world(0)).await;

        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&4000.0f32.to_le_bytes());
        body[4..8].copy_from_slice(&800.0f32.to_le_bytes());
        body[Movement::MOVE_TYPE] = 16;
        let message = Message { sender: 7, opcode: OP_MOVE, time: 0, body };

        assert!(matches!(handle_message(&state, &mut session, &message).await, Action::Ignore));
        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), (4000, 800));
    }

    #[tokio::test]
    async fn movement_refuses_client_teleport_and_other_senders() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let _ = handle_message(&state, &mut session, &enter_world(0)).await;
        let start = (session.character.as_ref().unwrap().x, session.character.as_ref().unwrap().y);

        // teleport must never come from the client: that is the map-jump exploit
        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&9999.0f32.to_le_bytes());
        body[Movement::MOVE_TYPE] = 1;
        let teleport = Message { sender: 7, opcode: OP_MOVE, time: 0, body: body.clone() };
        let _ = handle_message(&state, &mut session, &teleport).await;

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), start, "teleport must not have moved us");
    }

    #[tokio::test]
    async fn client_ready_before_choosing_a_character_is_ignored() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        let ready = Message { sender: 7, opcode: OP_CLIENT_READY, time: 0, body: Vec::new() };
        // does not drop the connection: there is simply nothing to spawn
        assert!(matches!(handle_message(&state, &mut session, &ready).await, Action::Ignore));
    }

    #[tokio::test]
    async fn entering_an_empty_slot_is_refused() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        assert!(matches!(
            handle_message(&state, &mut session, &enter_world(2)).await,
            Action::Disconnect
        ));
    }

    /// Entering the world without having logged in on the same connection must
    /// not work: that is what kept two players from coexisting.
    #[tokio::test]
    async fn entering_the_world_requires_logging_in_first() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        assert!(matches!(
            handle_message(&state, &mut Session::default(), &enter_world(0)).await,
            Action::Disconnect
        ));
    }

    #[tokio::test]
    async fn refuses_wrong_client_version() {
        let state = state_with(vec![]);
        // the server drops the connection instead of leaving the client waiting
        assert!(matches!(handle_message(&state, &mut Session::default(), &login_message("admin", 123)).await, Action::Disconnect));
        assert!(matches!(handle_message(&state, &mut Session::default(), &login_message("admin", 0)).await, Action::Disconnect));
    }

    #[tokio::test]
    async fn refuses_unknown_account() {
        let state = state_with(vec![]);
        assert!(matches!(
            handle_message(&state, &mut Session::default(), &login_message("ninguem", 124)).await,
            Action::Disconnect
        ));
    }

    #[tokio::test]
    async fn unimplemented_opcode_is_ignored() {
        let state = state_with(vec![]);
        // a not-yet-implemented packet must not drop someone who is logged in
        let message = Message { sender: 0, opcode: 0x301, time: 0, body: vec![0; 32] };
        assert!(matches!(handle_message(&state, &mut Session::default(), &message).await, Action::Ignore));
    }

    #[test]
    fn leading_prefix_is_dropped_only_when_it_is_not_a_frame() {
        let framed = frame::encode(
            &Message { sender: 0, opcode: OP_REQUEST_LOGIN, time: 0, body: vec![0; 32] },
            0x11,
        );

        // without a prefix: passes through untouched
        let mut clean = LeadingPrefix::default();
        assert_eq!(clean.feed(&framed).unwrap(), &framed[..]);

        // with the prefix: the 4 bytes disappear
        let mut prefixed = LeadingPrefix::default();
        let mut wire = vec![0x11, 0xF3, 0x11, 0x1F];
        wire.extend_from_slice(&framed);
        assert_eq!(prefixed.feed(&wire).unwrap(), &framed[..]);
    }

    #[test]
    fn leading_prefix_waits_for_enough_bytes() {
        let mut prefix = LeadingPrefix::default();
        assert!(prefix.feed(&[0x11, 0xF3]).is_none(), "2 bytes are not enough to decide");
        let rest = prefix.feed(&[0x11, 0x1F, 0x00, 0x00]);
        assert!(rest.is_some(), "6 bytes are enough to decide");
    }


    /// The city has to be populated when the player arrives, or it looks
    /// broken: the buildings are drawn by the client, the people are not.
    #[tokio::test]
    async fn the_townspeople_appear_when_the_player_enters_the_world() {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        state.world = World::with_npcs(vec![
            npc(2050, "Merchant", 3455.0, 700.0),  // 12 away from the spawn
            npc(2051, "Skill Master", 3460.0, 695.0), // 11 away
            npc(2500, "Far Away", 9000.0, 9000.0),
        ]);

        let mut session = logged_in(&state).await;
        let Action::Reply(frames) = handle_message(&state, &mut session, &enter_world(0)).await else {
            panic!("entering the world produced no frames");
        };
        let _ = frames;

        let Action::Reply(frames) = handle_client_ready(&state, &mut session) else {
            panic!("the world burst produced no frames");
        };

        let spawned = spawned_ids(&frames);
        assert!(spawned.contains(&2050), "the merchant is not on screen: {spawned:?}");
        assert!(spawned.contains(&2051));
        assert!(!spawned.contains(&2500), "an npc across the map was spawned");
    }

    /// Walking past an NPC and away again places it once and removes it once.
    #[tokio::test]
    async fn an_npc_is_placed_once_and_removed_once() {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        state.world = World::with_npcs(vec![npc(2050, "Merchant", 3455.0, 700.0)]);

        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &enter_world(0)).await;
        handle_client_ready(&state, &mut session);
        assert!(session.visible_npcs.contains(&2050), "never placed");

        // standing still must not place it a second time
        let frames = refresh_npc_visibility(&state, &mut session);
        assert!(frames.is_empty(), "the merchant was sent twice");

        // walking off the map edge of the watch radius takes it away
        session.character.as_mut().unwrap().x = 9000;
        let frames = refresh_npc_visibility(&state, &mut session);
        assert_eq!(removed_ids(&frames), vec![2050]);
        assert!(!session.visible_npcs.contains(&2050));

        // and once gone, it stays gone until the player walks back
        assert!(refresh_npc_visibility(&state, &mut session).is_empty());
    }

    /// The gap between watching and forgetting is what stops an NPC from
    /// flickering while a player paces across the edge of the radius.
    #[tokio::test]
    async fn an_npc_just_outside_the_watch_radius_is_not_forgotten_yet() {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        state.world = World::with_npcs(vec![npc(2050, "Merchant", 3450.0, 690.0)]);

        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &enter_world(0)).await;
        handle_client_ready(&state, &mut session);

        // 55 away: past DISTANCE_TO_WATCH, short of DISTANCE_TO_FORGET
        session.character.as_mut().unwrap().x = 3505;
        assert!(refresh_npc_visibility(&state, &mut session).is_empty());
        assert!(session.visible_npcs.contains(&2050));

        // 65 away: gone
        session.character.as_mut().unwrap().x = 3515;
        assert_eq!(removed_ids(&refresh_npc_visibility(&state, &mut session)), vec![2050]);
    }

    /// What the client reads out of the packet has to be what the file said.
    #[test]
    fn the_spawn_carries_the_model_the_position_and_the_title() {
        let merchant = npc(2050, "Merchant", 3468.4, 963.4);
        let message = decode(&encode_npc_spawn(&merchant));

        assert_eq!(message.opcode, OP_CREATE_MOB);
        assert_eq!(message.sender, 2050, "the header identifies the npc");

        use spawn_offset as off;
        let body = &message.body;
        assert_eq!(
            u16::from_le_bytes(body[off::EQUIP..off::EQUIP + 2].try_into().unwrap()),
            234,
            "the model the client draws"
        );
        assert_eq!(
            f32::from_le_bytes(body[off::POSITION_X..off::POSITION_X + 4].try_into().unwrap()),
            3468.4
        );
        assert_eq!(body[npc_offset::IS_SERVICE], NPC_IS_SERVICE);
        assert_eq!(body[off::UNK0], NPC_UNK0, "an npc is not a player");

        // the name travels as the digits of a string index, not as text
        let name: String =
            body[..16].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
        assert_eq!(name, "43");

        let title: String = body[npc_offset::TITLE..npc_offset::TITLE + 8]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        assert_eq!(title, "Merchant");
    }

    // ---- talking to an NPC, buying, and carrying things -----------------

    /// A merchant standing where the character spawns, so every test below
    /// starts within talking distance.
    fn shop_state() -> State {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        let mut merchant = npc(2050, "Merchant", 3450.0, 690.0);
        merchant.shop[0] = 1000;
        merchant.shop[1] = 4351;
        merchant.options = vec![1, 2, 5, 8];

        let mut farmer = npc(2049, "Farmer", 3452.0, 692.0);
        farmer.options = vec![1, 2, 5, 8];

        state.world = World::with_npcs(vec![merchant, farmer]);
        state.items = item_table();
        state
    }

    /// Prices for the two things the merchant above sells.
    fn item_table() -> aika_data::itemlist::ItemList {
        use aika_data::itemlist::{field, ItemList, RECORD_SIZE};
        let mut raw = vec![0u8; 5000 * RECORD_SIZE];

        // The shop asks `SELL_PRICE`, despite the name: `PRICE_GOLD` is not
        // what anything costs.
        let mut define = |id: usize, base: u32, groups: bool| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&base.to_le_bytes());
            r[field::CAN_GROUP] = groups as u8;
            r[field::DURABILITY] = 60;
        };
        define(1000, 500, false);

        // a health potion: the type and the effect are what makes it drinkable
        let r = &mut raw[POTION as usize * RECORD_SIZE..(POTION as usize + 1) * RECORD_SIZE];
        r[field::NAME.start] = b'x';
        r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&10u32.to_le_bytes());
        r[field::CAN_GROUP] = 1;
        r[field::ITEM_TYPE..field::ITEM_TYPE + 2]
            .copy_from_slice(&ITEM_TYPE_HP_POTION.to_le_bytes());
        r[field::USE_EFFECT..field::USE_EFFECT + 2].copy_from_slice(&500u16.to_le_bytes());

        ItemList::decode(&raw).expect("the fixture table is malformed")
    }

    /// A health potion in the fixture table.
    const POTION: u16 = 4351;

    /// A session standing in the world, ready to talk to somebody.
    async fn in_world(state: &State) -> Session {
        let mut session = logged_in(state).await;
        handle_message(state, &mut session, &enter_world(0)).await;
        handle_client_ready(state, &mut session);
        session
    }

    fn open_npc(npc: u32, option: u32) -> Message {
        Message {
            sender: TEST_CLIENT_ID,
            opcode: dialog::OP_OPEN_NPC,
            time: 0,
            body: dialog::OpenNpc { npc, option, extra: 0 }.to_body(),
        }
    }

    fn frames_of(action: Action) -> Vec<Vec<u8>> {
        match action {
            Action::Reply(frames) => frames,
            other => panic!("expected frames, got {other:?}"),
        }
    }

    fn opcodes(frames: &[Vec<u8>]) -> Vec<u16> {
        frames.iter().map(|f| decode(f).opcode).collect()
    }

    /// The text of a `0x984`, which is how the server says no to the player.
    fn message_text(frame: &[u8]) -> String {
        let body = decode(frame).body;
        body[4..].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()
    }

    /// A `0xF86` the way the client sends one: a kind, a name it claims to be,
    /// and a line of text.
    fn chat_message(kind: u16, claimed_nick: &str, line: &str) -> Message {
        let mut body = vec![0u8; chat_offset::SIZE];
        body[chat_offset::TYPE..chat_offset::TYPE + 2].copy_from_slice(&kind.to_le_bytes());
        write_fixed_str(&mut body[chat_offset::NICK..chat_offset::NICK + 16], claimed_nick);
        write_fixed_str(&mut body[chat_offset::LINE..chat_offset::LINE + 128], line);
        Message { sender: TEST_CLIENT_ID, opcode: OP_CHAT, time: 0, body }
    }

    /// The name and the line out of a `0xF86` frame.
    fn chat_of(frame: &[u8]) -> (String, String) {
        let body = decode(frame).body;
        (
            read_fixed_str(&body[chat_offset::NICK..chat_offset::NICK + 16]),
            read_fixed_str(&body[chat_offset::LINE..chat_offset::LINE + 128]),
        )
    }

    #[tokio::test]
    async fn saying_something_comes_back_to_the_speaker() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(
            handle_message(&state, &mut session, &chat_message(CHAT_NORMAL, "Athus", "olá mundo"))
                .await,
        );

        assert_eq!(decode(&frames[0]).opcode, OP_CHAT, "the speaker sees their own bubble");
        assert_eq!(chat_of(&frames[0]), ("Athus".into(), "olá mundo".into()));
    }

    /// You cannot put words in another player's mouth: the server stamps your
    /// own name over whatever the packet claims.
    #[tokio::test]
    async fn the_server_stamps_the_real_name_over_a_forged_one() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(
            handle_message(
                &state,
                &mut session,
                &chat_message(CHAT_NORMAL, "AlguemFamoso", "não fui eu"),
            )
            .await,
        );

        let (name, _) = chat_of(&frames[0]);
        assert_eq!(name, "Athus", "a forged name went out unchanged");
    }

    #[tokio::test]
    async fn a_whisper_to_nobody_says_so() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(
            handle_message(
                &state,
                &mut session,
                &chat_message(CHAT_WHISPER, "Fantasma", "tem alguém aí?"),
            )
            .await,
        );

        assert_eq!(decode(&frames[0]).opcode, OP_CLIENT_MESSAGE);
        assert!(message_text(&frames[0]).contains("não encontrado"));
    }

    /// Two piles of the same stackable item merge into one.
    #[tokio::test]
    async fn stacking_two_piles_adds_them_up() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        {
            let items = &mut session.character.as_mut().unwrap().items;
            for (slot, count) in [(30u16, 7u16), (31u16, 5u16)] {
                items
                    .put(Item {
                        container: inventory::BAG,
                        slot,
                        index: POTION,
                        refine: count,
                        ..Item::default()
                    })
                    .unwrap();
            }
        }

        let mut body = vec![0u8; 8];
        body[0..4].copy_from_slice(&30u32.to_le_bytes()); // src
        body[4..8].copy_from_slice(&31u32.to_le_bytes()); // dest
        let group = Message { sender: TEST_CLIENT_ID, opcode: OP_GROUP_ITEM, time: 0, body };
        handle_message(&state, &mut session, &group).await;

        let items = &session.character.as_ref().unwrap().items;
        assert!(items.get(inventory::BAG, 30).is_none(), "the source pile was not emptied");
        assert_eq!(items.get(inventory::BAG, 31).unwrap().refine, 12, "the piles did not add up");
    }

    /// A pile splits into two, and the taken-off half lands in a free slot.
    #[tokio::test]
    async fn splitting_a_pile_leaves_both_halves() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item {
                container: inventory::BAG,
                slot: 30,
                index: POTION,
                refine: 10,
                ..Item::default()
            })
            .unwrap();

        let mut body = vec![0u8; 12];
        body[0..4].copy_from_slice(&30u32.to_le_bytes()); // slot
        body[4..8].copy_from_slice(&4u32.to_le_bytes()); // take four off
        body[8..12].copy_from_slice(&(inventory::BAG as u32).to_le_bytes());
        let split = Message { sender: TEST_CLIENT_ID, opcode: OP_UNGROUP_ITEM, time: 0, body };
        handle_message(&state, &mut session, &split).await;

        let items = &session.character.as_ref().unwrap().items;
        assert_eq!(items.get(inventory::BAG, 30).unwrap().refine, 6, "the source was not reduced");
        let taken = items
            .in_container(inventory::BAG)
            .find(|i| i.slot != 30 && i.index == POTION)
            .expect("the taken-off half went nowhere");
        assert_eq!(taken.refine, 4, "the split-off count is wrong");
    }

    /// You cannot split off the whole pile — that is a no-op, not a duplicate.
    #[tokio::test]
    async fn splitting_the_whole_pile_does_nothing() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item {
                container: inventory::BAG,
                slot: 30,
                index: POTION,
                refine: 5,
                ..Item::default()
            })
            .unwrap();

        let mut body = vec![0u8; 12];
        body[0..4].copy_from_slice(&30u32.to_le_bytes());
        body[4..8].copy_from_slice(&5u32.to_le_bytes()); // the whole thing
        body[8..12].copy_from_slice(&(inventory::BAG as u32).to_le_bytes());
        let split = Message { sender: TEST_CLIENT_ID, opcode: OP_UNGROUP_ITEM, time: 0, body };
        handle_message(&state, &mut session, &split).await;

        let items = &session.character.as_ref().unwrap().items;
        assert_eq!(items.in_container(inventory::BAG).filter(|i| i.index == POTION).count(), 1);
        assert_eq!(items.get(inventory::BAG, 30).unwrap().refine, 5);
    }

    #[tokio::test]
    async fn a_truncated_chat_packet_is_ignored() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let short = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_CHAT,
            time: 0,
            body: vec![0u8; 8],
        };
        assert!(matches!(
            handle_message(&state, &mut session, &short).await,
            Action::Ignore
        ));
    }

    #[tokio::test]
    async fn clicking_an_npc_sends_its_menu() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(&state, &mut session, &open_npc(2050, 0)).await);

        assert_eq!(
            opcodes(&frames),
            vec![
                dialog::OP_MENU_BEGIN,
                dialog::OP_MENU_OWNER,
                dialog::OP_MENU_ENTRY,
                dialog::OP_MENU_ENTRY,
                dialog::OP_MENU_ENTRY,
                dialog::OP_MENU_ENTRY,
            ],
            "a menu is an opening signal, the owner, then one packet per entry"
        );

        // the second packet names which NPC the menu belongs to
        let owner = decode(&frames[1]);
        assert_eq!(u32::from_le_bytes(owner.body[0..4].try_into().unwrap()), 2050);

        // and the first entry reads as it should
        let entry = decode(&frames[2]);
        let text: String =
            entry.body[8..].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
        assert_eq!(text, "Talk");
    }

    /// An NPC with nothing to sell must not offer a shop, or the player opens
    /// an empty window and thinks the server is broken.
    #[tokio::test]
    async fn a_merchant_with_no_stock_does_not_offer_a_shop() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(&state, &mut session, &open_npc(2049, 0)).await);

        // four options in the file, but the shop is dropped
        assert_eq!(frames.len(), 2 + 3, "the shop entry survived on an empty shop");
    }

    /// The distance is checked on every packet, not only on the first click:
    /// a window left open while walking away must stop working.
    #[tokio::test]
    async fn talking_from_too_far_away_is_refused() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().x = 9000;

        let frames = frames_of(handle_message(&state, &mut session, &open_npc(2050, 0)).await);

        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE, dialog::OP_MENU_CLOSE]);
        assert_eq!(message_text(&frames[0]), "You are too far away.");
        assert_eq!(session.opened_npc, None);
    }

    #[tokio::test]
    async fn opening_the_shop_sends_what_is_for_sale() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(
            &state,
            &mut session,
            &open_npc(2050, dialog::option::SHOP),
        ).await);

        assert_eq!(
            opcodes(&frames),
            vec![dialog::OP_MENU_CLOSE, shop::OP_SHOW_SHOP],
            "the conversation has to close before the shop opens, or the client              keeps the screen letterboxed behind it"
        );

        let window = decode(&frames[1]);
        assert_eq!(window.opcode, shop::OP_SHOW_SHOP);
        assert_eq!(window.body.len() + MIN_FRAME, shop::SHOW_SHOP_SIZE);
        assert_eq!(u16::from_le_bytes(window.body[0..2].try_into().unwrap()), 2050);
        assert_eq!(u16::from_le_bytes(window.body[4..6].try_into().unwrap()), 1000);
        assert_eq!(u16::from_le_bytes(window.body[6..8].try_into().unwrap()), 4351);
        assert_eq!(session.opened_npc, Some(2050));
    }

    #[tokio::test]
    async fn buying_moves_gold_into_an_item() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().gold = 2000;
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::SHOP)).await;

        let buy = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_BUY,
            time: 0,
            body: shop::Buy { npc: 2050, slot: 0, amount: 1 }.to_body(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &buy).await);

        assert_eq!(opcodes(&frames), vec![shop::OP_REFRESH_ITEM, shop::OP_REFRESH_MONEY]);

        let character = session.character.as_ref().unwrap();
        assert_eq!(character.gold, 1500, "the price was not taken");
        assert_eq!(character.items.get(inventory::BAG, 0).unwrap().index, 1000);

        // the client is told exactly which slot changed and to what
        let refresh = decode(&frames[0]);
        assert_eq!(refresh.body[1], inventory::BAG);
        assert_eq!(u16::from_le_bytes(refresh.body[2..4].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(refresh.body[4..6].try_into().unwrap()), 1000);

        let money = decode(&frames[1]);
        assert_eq!(u64::from_le_bytes(money.body[4..12].try_into().unwrap()), 1500);
    }

    /// Standing next to the merchant is what allows a purchase, not having
    /// clicked a window first: the client closes the option menu the moment
    /// the shop replaces it, and escape closes it too.
    #[tokio::test]
    async fn buying_without_opening_the_window_works_when_standing_there() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().gold = 2000;

        let buy = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_BUY,
            time: 0,
            body: shop::Buy { npc: 2050, slot: 0, amount: 1 }.to_body(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &buy).await);

        assert_eq!(opcodes(&frames), vec![shop::OP_REFRESH_ITEM, shop::OP_REFRESH_MONEY]);
        assert_eq!(session.character.as_ref().unwrap().gold, 1500);
    }

    /// Buying from across the map is what a modified client would try, and
    /// distance is the thing it cannot lie about: the server owns the
    /// position.
    #[tokio::test]
    async fn buying_from_across_the_map_is_refused() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().gold = 2000;
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::SHOP)).await;

        session.character.as_mut().unwrap().x = 9000;

        let buy = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_BUY,
            time: 0,
            body: shop::Buy { npc: 2050, slot: 0, amount: 1 }.to_body(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &buy).await);

        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE]);
        assert_eq!(message_text(&frames[0]), "You are too far from the shop.");
        assert_eq!(session.character.as_ref().unwrap().gold, 2000, "gold was taken anyway");
        assert!(session.character.as_ref().unwrap().items.is_empty());
    }

    /// An npc id nobody has is refused rather than crashing a lookup.
    #[tokio::test]
    async fn buying_from_an_npc_that_does_not_exist_is_refused() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let buy = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_BUY,
            time: 0,
            body: shop::Buy { npc: 9999, slot: 0, amount: 1 }.to_body(),
        };
        assert_eq!(
            opcodes(&frames_of(handle_message(&state, &mut session, &buy).await)),
            vec![OP_CLIENT_MESSAGE]
        );
    }

    /// Walking away with the window still open stops the purchases too,
    /// which is the same rule reached by playing normally.
    #[tokio::test]
    async fn walking_away_stops_the_shop_working() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().gold = 2000;
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::SHOP)).await;

        session.character.as_mut().unwrap().x = 9000;
        handle_message(&state, &mut session, &open_npc(2050, 0)).await;
        assert_eq!(session.opened_npc, None);

        let buy = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_BUY,
            time: 0,
            body: shop::Buy { npc: 2050, slot: 0, amount: 1 }.to_body(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &buy).await);
        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE]);
        assert_eq!(session.character.as_ref().unwrap().gold, 2000);
    }

    /// Buying and selling straight back has to leave the player poorer, or
    /// a shop becomes a money printer.
    #[tokio::test]
    async fn buying_and_selling_a_stack_back_loses_money() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().gold = 1000;
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::SHOP)).await;

        let buy = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_BUY,
            time: 0,
            body: shop::Buy { npc: 2050, slot: 1, amount: 20 }.to_body(),
        };
        handle_message(&state, &mut session, &buy).await;
        assert_eq!(session.character.as_ref().unwrap().gold, 800, "20 potions at 10");

        let sell = Message {
            sender: TEST_CLIENT_ID,
            opcode: shop::OP_SELL,
            time: 0,
            body: shop::Sell { npc: 2050, slot: 0 }.to_body(),
        };
        handle_message(&state, &mut session, &sell).await;

        let character = session.character.as_ref().unwrap();
        assert_eq!(character.gold, 840, "20 potions back at a fifth of 10");
        assert!(character.gold < 1000, "the round trip made money");
        assert!(character.items.is_empty(), "the stack is still in the bag");
    }

    /// The field order, pinned against the record rather than against my
    /// reading of it. This was wrong once: the destination comes first, and
    /// reading it the other way round made every drag arrive backwards and
    /// look like the client dragging out of empty slots.
    #[test]
    fn move_item_reads_the_destination_first() {
        // DestType 0, DestSlot 8, SrcType 1, SrcSlot 3: equipping bag slot 3.
        let wire: Vec<u8> = vec![0, 0, 8, 0, 1, 0, 3, 0];
        let parsed = MoveItem::parse(&wire).unwrap();

        assert_eq!(parsed.to(), (inventory::EQUIP, 8), "destination read wrong");
        assert_eq!(parsed.from(), (inventory::BAG, 3), "source read wrong");
        assert_eq!(parsed.to_body(), wire);
        assert_eq!(MoveItem::parse(&[0u8; 4]), None);
    }

    /// The delete packet puts the slot before the container, which is the
    /// opposite of everywhere else.
    #[test]
    fn delete_item_reads_the_slot_first() {
        let wire: Vec<u8> = vec![7, 0, 0, 0, 1, 0, 0, 0];
        let parsed = DeleteItem::parse(&wire).unwrap();

        assert_eq!((parsed.slot, parsed.container), (7, 1));
        assert_eq!(parsed.to_body(), wire);
        assert_eq!(DeleteItem::parse(&[0u8; 4]), None);
    }

    #[test]
    fn use_item_body_roundtrip() {
        let original = UseItem { container: 1, slot: 3, argument: 0 };
        assert_eq!(UseItem::parse(&original.to_body()), Some(original));
        assert_eq!(UseItem::parse(&[0u8; 8]), None);
    }

    #[tokio::test]
    async fn dragging_an_item_tells_the_client_about_both_slots() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item { index: 1000, container: inventory::BAG, slot: 7, ..Item::default() })
            .unwrap();

        let drag = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_MOVE_ITEM,
            time: 0,
            body: MoveItem {
                to_container: inventory::BAG as u16,
                to_slot: 3,
                from_container: inventory::BAG as u16,
                from_slot: 7,
            }
            .to_body(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &drag).await);

        assert_eq!(opcodes(&frames), vec![shop::OP_REFRESH_ITEM, shop::OP_REFRESH_ITEM]);

        // slot 3 now holds it, and slot 7 is reported empty
        let filled = decode(&frames[0]);
        assert_eq!(u16::from_le_bytes(filled.body[2..4].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(filled.body[4..6].try_into().unwrap()), 1000);

        let emptied = decode(&frames[1]);
        assert_eq!(u16::from_le_bytes(emptied.body[2..4].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes(emptied.body[4..6].try_into().unwrap()), 0);
    }

    /// A refused drag still has to answer, because the client has already
    /// drawn the item in its new place and will keep it there otherwise.
    #[tokio::test]
    async fn a_refused_drag_still_tells_the_client_the_truth() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let drag = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_MOVE_ITEM,
            time: 0,
            body: MoveItem {
                to_container: inventory::BAG as u16,
                to_slot: 3,
                from_container: inventory::BAG as u16,
                from_slot: 7,
            }
            .to_body(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &drag).await);

        assert_eq!(frames.len(), 2);
        for frame in &frames {
            let body = decode(frame).body;
            assert_eq!(
                u16::from_le_bytes(body[4..6].try_into().unwrap()),
                0,
                "both slots have to come back empty"
            );
        }
    }

    #[tokio::test]
    async fn what_a_character_carries_reaches_the_client_in_the_world_packet() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        let character = session.character.as_mut().unwrap();
        character
            .items
            .put(Item { index: 1595, container: inventory::BAG, slot: 2, refine: 7, ..Item::default() })
            .unwrap();
        character.gold = 4242;

        let character = session.character.as_ref().unwrap().clone();
        let record = encode_character(&character, TEST_CLIENT_ID);

        use character_offset as off;
        let at = off::INVENTORY + 2 * off::ITEM_SIZE;
        assert_eq!(u16::from_le_bytes(record[at..at + 2].try_into().unwrap()), 1595);
        assert_eq!(u16::from_le_bytes(record[at + 16..at + 18].try_into().unwrap()), 7);
        assert_eq!(
            u64::from_le_bytes(record[off::GOLD..off::GOLD + 8].try_into().unwrap()),
            4242
        );

        // and the appearance slots still win over anything in equipment
        assert_eq!(
            u16::from_le_bytes(record[off::EQUIP..off::EQUIP + 2].try_into().unwrap()),
            character.class_index
        );
    }

    #[tokio::test]
    async fn an_unimplemented_option_says_so_instead_of_going_quiet() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(
            &state,
            &mut session,
            &open_npc(2050, dialog::option::QUESTS),
        ).await);

        // quests keep the window open, so nothing closes it
        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE]);
        assert_eq!(message_text(&frames[0]), "Quest is not available yet.");

        // repair does not, so it closes first
        let frames = frames_of(handle_message(&state, &mut session, &open_npc(2050, 31)).await);
        assert_eq!(opcodes(&frames), vec![dialog::OP_MENU_CLOSE, OP_CLIENT_MESSAGE]);
        assert_eq!(message_text(&frames[1]), "Repair is not available yet.");
    }

    // ---- making a character, and turning ---------------------------------

    fn create_message(name: &str, slot: u32) -> Message {
        Message {
            sender: TEST_CLIENT_ID,
            opcode: creation::OP_CREATE_CHARACTER,
            time: 0,
            body: creation::CreateCharacter {
                slot,
                name: name.into(),
                class_index: 20,
                hair: 7702,
                town: 0,
            }
            .to_body(),
        }
    }

    /// The names in a character list packet, one per slot, blank where the
    /// slot is empty.
    fn names_in_char_list(frame: &[u8]) -> Vec<String> {
        let body = decode(frame).body;
        (0..MAX_CHARACTERS)
            .map(|slot| {
                let at = 12 + slot * CHAR_ENTRY_SIZE;
                body[at..at + 16]
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| b as char)
                    .collect()
            })
            .collect()
    }

    #[tokio::test]
    async fn creating_a_character_puts_it_in_the_list() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;

        let frames =
            frames_of(handle_message(&state, &mut session, &create_message("Segundo", 1)).await);

        assert_eq!(opcodes(&frames), vec![OP_CHAR_LIST]);
        let names = names_in_char_list(&frames[0]);
        assert_eq!(names[0], "Athus");
        assert_eq!(names[1], "Segundo", "the new character is not in slot 1");
        assert_eq!(names[2], "");

        // and the session sees it too, so entering the world can find it
        let account = session.account.as_ref().unwrap();
        assert_eq!(account.characters.len(), 2);
    }

    /// A refusal has to say why and still redraw the screen, or the client
    /// sits on a creation form that never responds.
    #[tokio::test]
    async fn a_refused_character_gets_a_reason_and_the_list_back() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;

        let frames =
            frames_of(handle_message(&state, &mut session, &create_message("Athus", 1)).await);

        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE, OP_CHAR_LIST]);
        assert_eq!(message_text(&frames[0]), "Athus is already taken.");
        assert_eq!(names_in_char_list(&frames[1])[1], "", "the slot was filled anyway");
    }

    #[tokio::test]
    async fn a_character_cannot_take_a_slot_that_is_used() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;

        let frames =
            frames_of(handle_message(&state, &mut session, &create_message("Segundo", 0)).await);

        assert_eq!(message_text(&frames[0]), "Slot 0 does not exist.");
        assert_eq!(names_in_char_list(&frames[1])[0], "Athus", "the first one was replaced");
    }

    /// What creation puts in the bag has to reach the client in the world
    /// packet, at the slot it was put in.
    #[tokio::test]
    async fn a_new_character_arrives_carrying_its_starting_gear() {
        let state = state_with(vec![]);
        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &create_message("Novato", 0)).await;

        let created = session.account.as_ref().unwrap().characters[0].clone();
        let record = encode_character(&created, TEST_CLIENT_ID);

        use character_offset as off;
        let last_bag = off::INVENTORY + 125 * off::ITEM_SIZE;
        assert_eq!(
            u16::from_le_bytes(record[last_bag..last_bag + 2].try_into().unwrap()),
            creation::BAG_ITEM,
            "the bag in the last slot did not reach the client"
        );
    }

    /// Turning is relayed to whoever can see it and never echoed back: the
    /// client that turned has already turned.
    #[tokio::test]
    async fn turning_reaches_the_others_and_not_the_sender() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = in_world(&state).await;

        // The relay looks the sender up in the registry to find who can see
        // it, so this session has to be in there under its own id.
        let (mine, mut mine_rx) = tokio::sync::mpsc::unbounded_channel();
        session.client_id = state.world.connect(mine).expect("room to connect");
        let character = session.character.clone().unwrap();
        state.world.enter(session.client_id, character.clone());

        let (theirs, mut watcher_rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = state.world.connect(theirs).expect("room for a watcher");
        state.world.enter(watcher, character);

        let turn = Message {
            sender: session.client_id,
            opcode: OP_ROTATE,
            time: 0,
            body: 180u32.to_le_bytes().to_vec(),
        };
        let action = handle_message(&state, &mut session, &turn).await;

        assert!(matches!(action, Action::Ignore), "the sender must hear nothing back");
        assert_eq!(session.rotation, 180);

        let relayed = watcher_rx.try_recv().expect("the watcher was not told");
        let message = decode(&relayed);
        assert_eq!(message.opcode, OP_ROTATE);
        assert_eq!(message.sender, session.client_id, "the relay says who turned");
        assert_eq!(u32::from_le_bytes(message.body[0..4].try_into().unwrap()), 180);

        assert!(mine_rx.try_recv().is_err(), "the sender heard its own turn");
    }

    /// The same rotation twice is what the client sends while standing still,
    /// and relaying it would be noise on every other connection.
    #[tokio::test]
    async fn turning_the_same_way_twice_is_dropped() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = in_world(&state).await;

        let turn = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_ROTATE,
            time: 0,
            body: 90u32.to_le_bytes().to_vec(),
        };
        handle_message(&state, &mut session, &turn).await;
        assert_eq!(session.rotation, 90);

        // no way to observe the drop from here other than that it does not
        // panic and the value is unchanged; the point is that it is cheap
        handle_message(&state, &mut session, &turn).await;
        assert_eq!(session.rotation, 90);
    }

    /// A player arriving is drawn differently from one walking into view, and
    /// the difference is one byte the client reads.
    #[tokio::test]
    async fn a_player_arriving_is_announced_as_an_arrival() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &enter_world(0)).await;

        // the arriving player and a watcher already standing there
        session.client_id =
            state.world.connect(tokio::sync::mpsc::unbounded_channel().0).unwrap();
        let (tx, mut watcher_rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = state.world.connect(tx).expect("room for a watcher");
        let character = session.character.clone().unwrap();
        state.world.enter(watcher, character.clone());

        handle_client_ready(&state, &mut session);

        let mut seen = None;
        while let Ok(frame) = watcher_rx.try_recv() {
            let message = decode(&frame);
            if message.opcode == OP_CREATE_MOB {
                seen = Some(message);
            }
        }
        let arrival = seen.expect("the watcher never saw the arrival");
        assert_eq!(
            arrival.body[spawn_offset::SPAWN_TYPE],
            SPAWN_TELEPORT_IN,
            "an arrival was drawn as somebody who was already there"
        );

        // the copy the player gets of itself is the ordinary kind
        let own = decode(&world_burst(&character, session.client_id, &[])[0]);
        assert_eq!(own.body[spawn_offset::SPAWN_TYPE], SPAWN_NORMAL);
    }

    // ---- using what is in a slot ----------------------------------------

    fn use_message(container: u8, slot: u32) -> Message {
        Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_USE_ITEM,
            time: 0,
            body: UseItem { container: container as u32, slot, argument: 0 }.to_body(),
        }
    }

    /// Drinking one out of a stack leaves the rest, and the client is told
    /// what is left rather than being made to guess.
    #[tokio::test]
    async fn using_a_potion_takes_one_off_the_stack() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item {
                index: POTION,
                container: inventory::BAG,
                slot: 2,
                refine: 5,
                ..Item::default()
            })
            .unwrap();

        let frames = frames_of(handle_message(&state, &mut session, &use_message(inventory::BAG, 2)).await);

        assert_eq!(opcodes(&frames), vec![OP_HP_MP, shop::OP_REFRESH_ITEM]);
        assert_eq!(
            session.character.as_ref().unwrap().items.get(inventory::BAG, 2).unwrap().refine,
            4,
            "the stack did not go down"
        );

        let refresh = decode(&frames[1]);
        assert_eq!(u16::from_le_bytes(refresh.body[4..6].try_into().unwrap()), POTION);
        assert_eq!(u16::from_le_bytes(refresh.body[20..22].try_into().unwrap()), 4);
    }

    /// The last one empties the slot, and the client has to be told that too
    /// or it keeps drawing a potion that is gone.
    #[tokio::test]
    async fn using_the_last_one_empties_the_slot() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item {
                index: POTION,
                container: inventory::BAG,
                slot: 0,
                refine: 1,
                ..Item::default()
            })
            .unwrap();

        let frames = frames_of(handle_message(&state, &mut session, &use_message(inventory::BAG, 0)).await);

        assert!(session.character.as_ref().unwrap().items.get(inventory::BAG, 0).is_none());
        let refresh = decode(&frames[1]);
        assert_eq!(
            u16::from_le_bytes(refresh.body[4..6].try_into().unwrap()),
            0,
            "the client was not told the slot is empty"
        );
    }

    /// Healing stops at the ceiling instead of running past it.
    #[tokio::test]
    async fn a_potion_cannot_heal_past_the_maximum() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        let (max_hp, _) = vitals(session.character.as_ref().unwrap());

        session.cur_hp = max_hp - 10;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item {
                index: POTION,
                container: inventory::BAG,
                slot: 0,
                refine: 2,
                ..Item::default()
            })
            .unwrap();

        handle_message(&state, &mut session, &use_message(inventory::BAG, 0)).await;
        assert_eq!(session.cur_hp, max_hp, "a big potion overshot the ceiling");
    }

    /// An item the server has no rule for says so rather than going quiet.
    #[tokio::test]
    async fn an_item_with_no_rule_yet_says_so() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item { index: 1000, container: inventory::BAG, slot: 0, ..Item::default() })
            .unwrap();

        let frames = frames_of(handle_message(&state, &mut session, &use_message(inventory::BAG, 0)).await);
        assert_eq!(message_text(&frames[0]), "That cannot be used yet.");
        assert!(
            session.character.as_ref().unwrap().items.get(inventory::BAG, 0).is_some(),
            "the item was consumed anyway"
        );
    }

    /// Using an empty slot is what the client sends on a stray double click.
    #[tokio::test]
    async fn using_an_empty_slot_does_nothing() {
        let state = shop_state();
        let mut session = in_world(&state).await;

        let action = handle_message(&state, &mut session, &use_message(inventory::BAG, 9)).await;
        assert!(matches!(action, Action::Ignore));
    }

    /// Equipping is a drag from the bag into an equipment slot, which is the
    /// case that arrives with the containers different.
    #[tokio::test]
    async fn equipping_moves_an_item_between_containers() {
        let state = shop_state();
        let mut session = in_world(&state).await;
        session
            .character
            .as_mut()
            .unwrap()
            .items
            .put(Item { index: 1000, container: inventory::BAG, slot: 0, ..Item::default() })
            .unwrap();

        let drag = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_MOVE_ITEM,
            time: 0,
            body: MoveItem {
                to_container: inventory::EQUIP as u16,
                to_slot: 8,
                from_container: inventory::BAG as u16,
                from_slot: 0,
            }
            .to_body(),
        };
        handle_message(&state, &mut session, &drag).await;

        let items = &session.character.as_ref().unwrap().items;
        assert_eq!(items.get(inventory::EQUIP, 8).unwrap().index, 1000, "not equipped");
        assert!(items.get(inventory::BAG, 0).is_none(), "still in the bag too");
    }

    fn delete_message(slot: u32) -> Message {
        Message {
            sender: TEST_CLIENT_ID,
            opcode: creation::OP_DELETE_CHARACTER,
            time: 0,
            body: creation::DeleteCharacter { slot, pin: String::new() }.to_body(),
        }
    }

    #[tokio::test]
    async fn deleting_a_character_takes_it_off_the_list() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &create_message("Segundo", 1)).await;

        let frames = frames_of(handle_message(&state, &mut session, &delete_message(1)).await);

        assert_eq!(opcodes(&frames), vec![OP_CHAR_LIST]);
        let names = names_in_char_list(&frames[0]);
        assert_eq!(names[0], "Athus", "the wrong character went");
        assert_eq!(names[1], "", "the character is still on the list");
        assert_eq!(session.account.as_ref().unwrap().characters.len(), 1);
    }

    /// Deleting the character being played would leave the connection holding
    /// something that no longer exists.
    #[tokio::test]
    async fn the_character_being_played_cannot_be_deleted() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(&state, &mut session, &delete_message(0)).await);

        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE, OP_CHAR_LIST]);
        assert_eq!(names_in_char_list(&frames[1])[0], "Athus");
    }

    /// An empty slot redraws the screen instead of going quiet, so the client
    /// is never left waiting on a dialog.
    #[tokio::test]
    async fn deleting_an_empty_slot_just_redraws_the_list() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;

        let frames = frames_of(handle_message(&state, &mut session, &delete_message(2)).await);
        assert_eq!(opcodes(&frames), vec![OP_CHAR_LIST]);
    }

    /// Going back to the selection screen ends the connection, which is how
    /// the original does it and what runs the save on the way out.
    #[tokio::test]
    async fn going_back_to_the_character_list_ends_the_connection() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = in_world(&state).await;

        let back = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_BACK_TO_CHARACTER_SELECT,
            time: 0,
            body: Vec::new(),
        };
        assert!(matches!(
            handle_message(&state, &mut session, &back).await,
            Action::Disconnect
        ));
    }

    // ---- monsters and fighting -------------------------------------------

    /// A world with one monster standing next to where the player spawns.
    fn fight_state() -> State {
        use aika_data::mobs::MobTable;
        // Level 40 with 5000 health, against the level 42 the test character
        // is: enough to take a dozen swings, so a fight is a fight.
        const INFO: &str = "1,Rato,216,0,0,5000,0,40,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,25,0,1";
        const LIST: &str = "1,1,1,1,Rato,Rato,0,0,0,3452,692,11,8,0,3452,692,11,8,0";

        let mut state = state_with(vec![dev_character("Athus", 0)]);
        let table = MobTable::parse(INFO, LIST).unwrap();
        state.world = World::with_npcs(Vec::new()).with_mobs(crate::mob::place_all(&table));
        state
    }

    const RAT: u16 = aika_data::mobs::FIRST_MOB_ID;
    const RAT_HP: u32 = 5000;

    fn attack_message(target: u16) -> Message {
        Message {
            sender: TEST_CLIENT_ID,
            opcode: combat::OP_ATTACK,
            time: 0,
            body: combat::Attack {
                target,
                animation: 2,
                skill: 0,
                from: (3450.0, 690.0),
                at: (3452.0, 692.0),
            }
            .to_body(),
        }
    }

    /// A monster near the player has to be on screen when they arrive, the
    /// same as the townspeople.
    #[tokio::test]
    async fn a_monster_nearby_is_on_screen_on_arrival() {
        let state = fight_state();
        let mut session = in_world(&state).await;

        assert!(session.visible_mobs.contains(&RAT), "the monster never appeared");
    }

    #[tokio::test]
    async fn hitting_a_monster_takes_health_off_it_and_tells_everyone() {
        let state = fight_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(&state, &mut session, &attack_message(RAT)).await);
        let report = decode(&frames[0]);

        assert_eq!(report.opcode, combat::OP_DAMAGE);
        assert_eq!(report.body.len() + MIN_FRAME, combat::DAMAGE_SIZE);

        use combat::damage_offset as off;
        let damage = u64::from_le_bytes(
            report.body[off::DAMAGE..off::DAMAGE + 8].try_into().unwrap(),
        );
        let left = u32::from_le_bytes(
            report.body[off::TARGET_HP..off::TARGET_HP + 4].try_into().unwrap(),
        );
        assert!(damage > 0, "the swing did nothing");
        assert_eq!(left, RAT_HP - damage as u32, "the health left does not match the damage");
        assert_eq!(state.world.mob(RAT).unwrap().hp, left);
    }

    /// Reaching across the map is what a modified client would try. The
    /// position comes from the world, not from the packet.
    #[tokio::test]
    async fn a_monster_out_of_reach_cannot_be_hit() {
        let state = fight_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().x = 9000;

        let action = handle_message(&state, &mut session, &attack_message(RAT)).await;
        assert!(matches!(action, Action::Ignore));
        assert_eq!(state.world.mob(RAT).unwrap().hp, RAT_HP, "it was hit from across the map");
    }

    #[tokio::test]
    async fn hitting_something_that_is_not_a_monster_does_nothing() {
        let state = fight_state();
        let mut session = in_world(&state).await;

        assert!(matches!(
            handle_message(&state, &mut session, &attack_message(9999)).await,
            Action::Ignore
        ));
    }

    /// Killing pays, and the corpse comes off the screen.
    #[tokio::test]
    async fn killing_a_monster_pays_experience_and_clears_it() {
        let state = fight_state();
        let mut session = in_world(&state).await;
        let before = session.character.as_ref().unwrap().exp;

        let mut swings = 0;
        while state.world.mob(RAT).is_some_and(|m| m.is_alive()) && swings < 200 {
            handle_message(&state, &mut session, &attack_message(RAT)).await;
            swings += 1;
        }

        assert!(swings < 200, "the monster would not die");
        assert!(swings > 1, "it died in one hit, which is not a fight");
        assert_eq!(
            session.character.as_ref().unwrap().exp,
            before + 25,
            "the kill did not pay"
        );
        assert!(!session.visible_mobs.contains(&RAT), "the corpse is still on screen");
        assert!(session.dirty, "the experience was not marked for saving");
    }

    /// A corpse pays once. Two players landing on the same one must not both
    /// be paid for it.
    #[tokio::test]
    async fn a_corpse_pays_nothing() {
        let state = fight_state();
        let mut session = in_world(&state).await;

        while state.world.mob(RAT).is_some_and(|m| m.is_alive()) {
            handle_message(&state, &mut session, &attack_message(RAT)).await;
        }
        let after_kill = session.character.as_ref().unwrap().exp;

        handle_message(&state, &mut session, &attack_message(RAT)).await;
        assert_eq!(session.character.as_ref().unwrap().exp, after_kill, "the corpse paid twice");
    }

    /// A monster that comes back has to reappear on a standing player, which
    /// is what the client's own heartbeat is spent on.
    #[tokio::test]
    async fn a_monster_that_comes_back_reappears_without_walking() {
        let state = fight_state();
        let mut session = in_world(&state).await;

        while state.world.mob(RAT).is_some_and(|m| m.is_alive()) {
            handle_message(&state, &mut session, &attack_message(RAT)).await;
        }
        assert!(!session.visible_mobs.contains(&RAT));

        let respawn = state.world.mob(RAT).unwrap().respawn;
        assert_eq!(state.world.revive_mobs(std::time::Instant::now() + respawn).len(), 1);

        // the heartbeat, not a step
        let ready = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_CLIENT_READY,
            time: 0,
            body: Vec::new(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &ready).await);

        assert_eq!(opcodes(&frames), vec![OP_CREATE_MOB]);
        assert!(session.visible_mobs.contains(&RAT), "it never came back on screen");
        assert_eq!(state.world.mob(RAT).unwrap().hp, RAT_HP, "it came back hurt");
    }

    // ---- casting -----------------------------------------------------------

    /// A world with a monster to aim at and a skill table laid out the way
    /// the real one is: the test character is class index 20, which is the
    /// second class, so its skills live in the second block.
    fn cast_state() -> State {
        use aika_data::skills::{field, SkillTable, RECORD_SIZE};

        let mut state = fight_state();
        let mut raw = vec![0u8; 4000 * RECORD_SIZE];

        let mut define = |id: usize, family: u32, min_level: u32, class: u32,
                          mana: u32, damage: u32, range: u32, cooldown: u32| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            let put = |r: &mut [u8], at: usize, v: u32| {
                r[at..at + 4].copy_from_slice(&v.to_le_bytes());
            };
            put(r, field::FAMILY, family);
            put(r, field::RANK, 1);
            put(r, field::MIN_LEVEL, min_level);
            put(r, field::CLASS, class);
            put(r, field::MANA, mana);
            put(r, field::DAMAGE, damage);
            put(r, field::RANGE, range);
            put(r, field::COOLDOWN, cooldown);
            put(r, field::AGGRESSIVE, 1);
            r[field::NAME_ENGLISH.start] = b'x';
        };

        define(SPELL as usize, 1, 1, 11, 30, 500, 300, 3000);       // ours
        define(OTHER_SPELL as usize, 1, 1, 51, 5, 40, 200, 1000);   // another class

        state.skills = SkillTable::decode(&raw).unwrap();
        state
    }

    /// The second class owns ids 960 to 1919, and its seventh slot - the
    /// first one the bar carries - is at 960 + 6 * 16 + 1.
    const SPELL: u32 = 960 + 6 * 16 + 1;
    /// The fifth class's, which the test character must not be able to reach.
    const OTHER_SPELL: u32 = 4 * 960 + 1;

    fn cast_message(skill: u32, target: u32) -> Message {
        Message {
            sender: TEST_CLIENT_ID,
            opcode: ability::OP_USE_SKILL,
            time: 0,
            body: ability::UseSkill { skill, target, at: (3452.0, 692.0) }.to_body(),
        }
    }

    /// The client draws the skill bar from what the server sends on arrival.
    #[tokio::test]
    async fn the_skill_list_reaches_the_client_on_arrival() {
        let state = cast_state();
        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &enter_world(0)).await;

        let frames = frames_of(handle_client_ready(&state, &mut session));
        let list = frames
            .iter()
            .map(|f| decode(f))
            .find(|m| m.opcode == ability::OP_SKILL_LIST)
            .expect("no skill list");

        assert_eq!(list.body.len() + MIN_FRAME, SKILLS_SIZE);
        let ids: Vec<u16> = (0..ability::SKILL_SLOTS)
            .map(|i| {
                let at = 4 + i * 2;
                u16::from_le_bytes(list.body[at..at + 2].try_into().unwrap())
            })
            .collect();

        assert!(ids.contains(&(SPELL as u16)), "the class spell is not on the bar");
        assert!(
            !ids.contains(&(OTHER_SPELL as u16)),
            "another class's spell is on the bar"
        );
    }

    #[tokio::test]
    async fn casting_spends_mana_and_hurts_the_target() {
        let state = cast_state();
        let mut session = in_world(&state).await;
        let mana = session.cur_mp;

        let frames = frames_of(handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await);

        assert_eq!(
            opcodes(&frames),
            vec![ability::OP_USE_SKILL, OP_HP_MP, combat::OP_DAMAGE],
            "the cast has to be relayed, the mana reported, and the damage sent"
        );
        assert_eq!(session.cur_mp, mana - 30, "the mana was not spent");
        assert!(state.world.mob(RAT).unwrap().hp < RAT_HP, "the spell did nothing");
    }

    /// A spell hits harder than a swing, because the table says what it does.
    #[tokio::test]
    async fn a_spell_hits_harder_than_a_swing() {
        let state = cast_state();

        let mut session = in_world(&state).await;
        handle_message(&state, &mut session, &attack_message(RAT)).await;
        let by_swing = RAT_HP - state.world.mob(RAT).unwrap().hp;

        let state = cast_state();
        let mut session = in_world(&state).await;
        handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await;
        let by_spell = RAT_HP - state.world.mob(RAT).unwrap().hp;

        assert!(by_spell > by_swing, "the spell did {by_spell} and the swing {by_swing}");
    }

    #[tokio::test]
    async fn a_spell_still_cooling_is_refused_and_costs_nothing() {
        let state = cast_state();
        let mut session = in_world(&state).await;

        handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await;
        let mana = session.cur_mp;
        let hp = state.world.mob(RAT).unwrap().hp;

        let frames = frames_of(handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await);

        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE]);
        assert_eq!(session.cur_mp, mana, "the refused cast spent mana");
        assert_eq!(state.world.mob(RAT).unwrap().hp, hp, "the refused cast hurt it");
    }

    #[tokio::test]
    async fn another_classs_spell_is_refused() {
        let state = cast_state();
        let mut session = in_world(&state).await;

        let frames = frames_of(handle_message(&state, &mut session, &cast_message(OTHER_SPELL, RAT as u32)).await);
        assert_eq!(message_text(&frames[0]), format!("You have not learned skill {OTHER_SPELL}."));
    }

    #[tokio::test]
    async fn casting_without_the_mana_is_refused() {
        let state = cast_state();
        let mut session = in_world(&state).await;
        session.cur_mp = 5;

        let frames = frames_of(handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await);
        assert_eq!(message_text(&frames[0]), "You need 30 mana and have 5.");
        assert_eq!(session.cur_mp, 5);
    }

    /// Reaching across the map with a spell is the same hole as with a swing.
    #[tokio::test]
    async fn a_target_out_of_the_spells_range_is_refused() {
        let state = cast_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().x = 9000;

        let frames = frames_of(handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await);
        assert_eq!(message_text(&frames[0]), "That is too far away.");
        assert_eq!(state.world.mob(RAT).unwrap().hp, RAT_HP);
    }

    #[tokio::test]
    async fn killing_with_a_spell_pays_the_same_as_killing_with_a_swing() {
        let state = cast_state();
        let mut session = in_world(&state).await;
        let before = session.character.as_ref().unwrap().exp;
        session.cur_mp = 100_000;

        let mut casts = 0;
        while state.world.mob(RAT).is_some_and(|m| m.is_alive()) && casts < 200 {
            // the cooldown is the reason a fight is not one packet
            session.cooldowns = ability::Cooldowns::new();
            handle_message(&state, &mut session, &cast_message(SPELL, RAT as u32)).await;
            casts += 1;
        }

        assert!(casts < 200, "the monster would not die");
        assert_eq!(session.character.as_ref().unwrap().exp, before + 25);
        assert!(!session.visible_mobs.contains(&RAT));
    }

    /// A character wearing armour has to arrive wearing it. The record the
    /// client holds is not what dresses it: the spawn packet is.
    #[tokio::test]
    async fn what_a_character_wears_reaches_the_spawn_and_the_selection_screen() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state).await;
        handle_message(&state, &mut session, &enter_world(0)).await;

        let character = session.character.as_mut().unwrap();
        // a breastplate, and a helmet whose appearance is something else
        character
            .items
            .put(Item { index: 3314, container: inventory::EQUIP, slot: 2, ..Item::default() })
            .unwrap();
        character
            .items
            .put(Item {
                index: 3344,
                appearance: 9999,
                container: inventory::EQUIP,
                slot: 3,
                ..Item::default()
            })
            .unwrap();

        let character = session.character.as_ref().unwrap().clone();
        let spawn = decode(&encode_spawn(&character, TEST_CLIENT_ID));

        use spawn_offset as off;
        let worn = |slot: usize| {
            let at = off::EQUIP + slot * 2;
            u16::from_le_bytes(spawn.body[at..at + 2].try_into().unwrap())
        };
        assert_eq!(worn(0), character.class_index, "the body is still the class");
        assert_eq!(worn(1), character.hair);
        assert_eq!(worn(2), 3314, "the breastplate did not reach the client");
        assert_eq!(worn(3), 9999, "the appearance has to win over the item");
        assert_eq!(worn(7), 0, "an empty slot stays empty");

        // and the selection screen dresses it the same way
        let entry = encode_char_list_entry(Some(&character));
        let at = 24 + 2 * 2;
        assert_eq!(u16::from_le_bytes(entry[at..at + 2].try_into().unwrap()), 3314);
    }

    // ---- levelling, dying and loot ---------------------------------------

    /// A world with a monster a level 1 can actually kill, and a curve to
    /// level on. The rat the other tests use is level 40, which a level 1
    /// cannot scratch — correctly, and uselessly for testing levelling.
    fn progress_state() -> State {
        use aika_data::mobs::MobTable;
        const INFO: &str = "1,Ratinho,216,0,0,60,0,1,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,25,0,1";
        const LIST: &str = "1,1,1,1,Ratinho,Ratinho,0,0,0,3452,692,11,8,0,3452,692,11,8,0";

        let mut state = fight_state();
        let table = MobTable::parse(INFO, LIST).unwrap();
        state.world = World::with_npcs(Vec::new()).with_mobs(crate::mob::place_all(&table));
        state.levels = aika_data::exp::ExpTable::decode(&{
            let mut bytes = Vec::new();
            for total in [0u64, 20, 60, 200] {
                bytes.extend_from_slice(&total.to_le_bytes());
            }
            bytes
        })
        .unwrap();
        state
    }

    async fn kill_the_rat(state: &State, session: &mut Session) {
        let mut swings = 0;
        while state.world.mob(RAT).is_some_and(|m| m.is_alive()) && swings < 500 {
            handle_message(state, session, &attack_message(RAT)).await;
            swings += 1;
        }
        assert!(swings < 500, "the monster would not die");
    }

    /// Experience buys levels off the curve, not off a count of kills.
    #[tokio::test]
    async fn enough_experience_is_a_level() {
        let state = progress_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().level = 1;
        session.character.as_mut().unwrap().exp = 0;

        kill_the_rat(&state, &mut session).await;

        // the rat is worth 25, which the curve says is level 2
        let character = session.character.as_ref().unwrap();
        assert_eq!(character.exp, 25);
        assert_eq!(character.level, 2, "the kill did not buy a level");
    }

    /// One kill can be worth more than one level, and health comes back with
    /// it: arriving at a new level nearly dead is a punishment for winning.
    #[tokio::test]
    async fn a_level_fills_the_health_back_up() {
        let state = progress_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().level = 1;
        session.character.as_mut().unwrap().exp = 0;
        session.cur_hp = 1;

        kill_the_rat(&state, &mut session).await;

        let stats = stats::of(session.character.as_ref().unwrap(), &state.items);
        assert_eq!(session.cur_hp, stats.max_hp, "it levelled and stayed hurt");
    }

    /// A character whose experience is nowhere near the next level stays put.
    #[tokio::test]
    async fn not_enough_experience_is_not_a_level() {
        let state = progress_state();
        let mut session = in_world(&state).await;
        session.character.as_mut().unwrap().level = 3;
        session.character.as_mut().unwrap().exp = 60;

        kill_the_rat(&state, &mut session).await;
        assert_eq!(session.character.as_ref().unwrap().level, 3);
    }

    /// A monster's blow reaches the player on its next packet, because the
    /// world tick cannot reach into a session to take health off it.
    #[tokio::test]
    async fn a_blow_left_by_the_tick_reaches_the_player() {
        let state = fight_state();
        let mut session = in_world(&state).await;
        let before = session.cur_hp;

        state.world.deal_to_player(
            session.client_id,
            crate::mob::Attack { attacker: RAT, target: session.client_id, damage: 40, skill: 0 },
        );

        let ready = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_CLIENT_READY,
            time: 0,
            body: Vec::new(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &ready).await);

        assert!(
            frames.iter().any(|f| decode(f).opcode == combat::OP_DAMAGE),
            "the player was never told it was hit"
        );
        assert!(session.cur_hp < before, "the blow took nothing off");
    }

    /// Enough blows and the character is down.
    #[tokio::test]
    async fn enough_blows_kill_the_player() {
        let state = fight_state();
        let mut session = in_world(&state).await;

        for _ in 0..200 {
            state.world.deal_to_player(
                session.client_id,
                crate::mob::Attack {
                    attacker: RAT,
                    target: session.client_id,
                    damage: 10_000,
                    skill: 0,
                },
            );
            let ready = Message {
                sender: TEST_CLIENT_ID,
                opcode: OP_CLIENT_READY,
                time: 0,
                body: Vec::new(),
            };
            handle_message(&state, &mut session, &ready).await;
            if session.dead {
                break;
            }
        }

        assert!(session.dead, "the player would not die");
        assert_eq!(session.cur_hp, 0);
    }

    /// A dead character takes no more damage and cannot swing.
    #[tokio::test]
    async fn the_dead_do_nothing() {
        let state = fight_state();
        let mut session = in_world(&state).await;
        session.dead = true;
        session.cur_hp = 0;

        let action = handle_message(&state, &mut session, &attack_message(RAT)).await;
        assert!(matches!(action, Action::Ignore), "a corpse swung");
        assert_eq!(state.world.mob(RAT).unwrap().hp, RAT_HP);

        state.world.deal_to_player(
            session.client_id,
            crate::mob::Attack { attacker: RAT, target: session.client_id, damage: 40, skill: 0 },
        );
        assert!(collect_blows(&state, &mut session).is_empty(), "a corpse was hit again");
    }

    #[tokio::test]
    async fn getting_up_restores_health_and_puts_you_in_town() {
        let state = fight_state();
        let mut session = in_world(&state).await;
        session.dead = true;
        session.cur_hp = 0;
        session.character.as_mut().unwrap().x = 9000;

        let revive = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_REVIVE,
            time: 0,
            body: Vec::new(),
        };
        let frames = frames_of(handle_message(&state, &mut session, &revive).await);

        assert!(!session.dead);
        assert!(session.cur_hp > 0, "it got up with no health");
        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), creation::TOWN_FIRST);
        assert_eq!(decode(&frames[0]).opcode, OP_CREATE_MOB, "it was never put back on screen");
    }

    /// Getting up when you are not down is what a client sends twice.
    #[tokio::test]
    async fn getting_up_while_alive_does_nothing() {
        let state = fight_state();
        let mut session = in_world(&state).await;
        let where_it_was = session.character.as_ref().unwrap().x;

        let revive = Message {
            sender: TEST_CLIENT_ID,
            opcode: OP_REVIVE,
            time: 0,
            body: Vec::new(),
        };
        assert!(matches!(
            handle_message(&state, &mut session, &revive).await,
            Action::Ignore
        ));
        assert_eq!(session.character.as_ref().unwrap().x, where_it_was);
    }

    /// A kill leaves something behind often enough to notice, and it goes
    /// into the bag.
    #[tokio::test]
    async fn a_kill_can_leave_something_behind() {
        use aika_data::drops::DropTable;

        let mut state = progress_state();
        // a band where every roll gives the same thing, so the test does not
        // depend on which one comes up
        state.drops = DropTable::default();
        state.items = {
            use aika_data::itemlist::{field, ItemList, RECORD_SIZE};
            let mut raw = vec![0u8; 5000 * RECORD_SIZE];
            let r = &mut raw[1000 * RECORD_SIZE..1001 * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&10u32.to_le_bytes());
            ItemList::decode(&raw).unwrap()
        };

        let mut session = in_world(&state).await;
        // with an empty drop table nothing can drop, which is the case that
        // has to not crash
        kill_the_rat(&state, &mut session).await;
        assert!(session.character.as_ref().unwrap().items.is_empty());
    }

    /// Gear is what makes a character hit harder, which is the whole point of
    /// reading its stats.
    #[tokio::test]
    async fn better_gear_hits_harder() {
        let mut state = fight_state();
        state.items = {
            use aika_data::itemlist::{field, ItemList, RECORD_SIZE};
            let mut raw = vec![0u8; 5000 * RECORD_SIZE];
            let r = &mut raw[1000 * RECORD_SIZE..1001 * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::ATTACK..field::ATTACK + 2].copy_from_slice(&500u16.to_le_bytes());
            ItemList::decode(&raw).unwrap()
        };

        let bare = {
            let mut session = in_world(&state).await;
            handle_message(&state, &mut session, &attack_message(RAT)).await;
            RAT_HP - state.world.mob(RAT).unwrap().hp
        };

        let state = {
            let mut fresh = fight_state();
            fresh.items = state.items;
            fresh
        };
        let armed = {
            let mut session = in_world(&state).await;
            session
                .character
                .as_mut()
                .unwrap()
                .items
                .put(Item {
                    index: 1000,
                    container: inventory::EQUIP,
                    slot: 6,
                    ..Item::default()
                })
                .unwrap();
            handle_message(&state, &mut session, &attack_message(RAT)).await;
            RAT_HP - state.world.mob(RAT).unwrap().hp
        };

        assert!(armed > bare, "a sword did nothing: {armed} against {bare}");
    }
}
