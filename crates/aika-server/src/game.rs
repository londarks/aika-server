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
use crate::store::{Account, Character, MAX_CHARACTERS};
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

/// Fixed values the original writes without explanation, but which the
/// client expects (`Mob/BaseMob.pas:2974-3131`).
const SPAWN_UNK0: u8 = 0x0A;
const SPAWN_EFFECT_1: u16 = 0x1D;
const SPAWN_ARMA_REFINE: u8 = 15;
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
    /// 16 items of 20 bytes; slot 0 is the class and slot 1 the hair.
    pub const EQUIP: usize = 340;
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
    // the connection, where the player stood is still worth keeping.
    if let Some(character) = session.character.as_ref() {
        state.save_position(character).await;
    }

    leave_world(&state, &session);
    writer.abort();
    result
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
                Ok(message) => match handle_message(state, session, &message) {
                    Action::Reply(frames) => {
                        for frame in frames {
                            let _ = outbox.send(frame);
                        }
                    }
                    Action::Ignore => {}
                    Action::Disconnect => return Ok(()),
                },
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

fn handle_message(state: &State, session: &mut Session, message: &Message) -> Action {
    match message.opcode {
        OP_REQUEST_LOGIN => handle_request_login(state, session, message),
        OP_ENTER_WORLD => handle_enter_world(session, message, state.uptime_ms()),
        OP_CLIENT_READY => handle_client_ready(state, session),
        OP_MOVE => handle_move(state, session, message),
        opcode => {
            // The original merely prints the code here; we do the same, adding the
            // size alongside, to help identify the packet.
            warn!(
                opcode = format!("0x{opcode:03x}"),
                size = message.body.len(),
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
fn handle_enter_world(session: &mut Session, message: &Message, time: u32) -> Action {
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
    let mut frames = Vec::with_capacity(5);

    // The order is the original's (`Mob/Player.pas:3409-3443`): one 0xCCCC
    // signal, three 0x186 and then the character.
    frames.push(encode_signal(OP_SIGNAL_READY, client_id, time, 1));
    for _ in 0..3 {
        frames.push(encode_signal(OP_SIGNAL_LOAD, client_id, time, 1));
    }
    frames.push(encode_send_to_world(&account, &character, client_id, time));

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
        debug!(character = %character.name, "repeated 0xF0B, ignoring");
        return Action::Ignore;
    }

    session.spawned = true;
    state.world.enter(session.client_id, character.clone());

    let mut frames = world_burst(&character, session.client_id);

    // The city is drawn by the client; the people in it are not. Without this
    // the player arrives in an empty town.
    frames.extend(refresh_npc_visibility(state, session));

    // Everyone already standing nearby has to appear on this player's screen,
    // and this player on theirs. Both directions use the same spawn packet.
    let neighbours = state.world.visible_to(session.client_id);
    let mine = encode_spawn(&character, session.client_id);
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
fn world_burst(character: &Character, client_id: u16) -> Vec<Vec<u8>> {
    let (hp, mp) = vitals(character);

    let mut frames = vec![
        encode_spawn(character, client_id),
        encode_skills(client_id),
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
    body[off::SPAWN_TYPE] = SPAWN_NORMAL;
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

    // Equip[0] is the class and Equip[1] the hair, in each item's `Index`.
    put16(&mut out, off::EQUIP, character.class_index);
    put16(&mut out, off::EQUIP + 20, character.hair);

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

    // Equip[0] is the class and Equip[1] the hair; the rest is appearance.
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
    fn reply(state: &State, version: u16) -> Vec<u8> {
        let mut session = Session { client_id: TEST_CLIENT_ID, ..Session::default() };
        match handle_message(state, &mut session, &login_message("admin", version)) {
            Action::Reply(frames) => frames.into_iter().next().expect("no frames"),
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    /// A session that already went through `0x685`, as a real connection would.
    ///
    /// The id is normally handed out by the world registry when the socket is
    /// accepted; tests set it directly.
    fn logged_in(state: &State) -> Session {
        let mut session = Session { client_id: TEST_CLIENT_ID, ..Session::default() };
        let action = handle_message(state, &mut session, &login_message("admin", 124));
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

    #[test]
    fn char_list_has_the_exact_size_the_client_expects() {
        let state = state_with(vec![]);
        let wire = reply(&state, 124);
        assert_eq!(wire.len(), CHAR_LIST_SIZE);

        let message = decode(&wire);
        assert_eq!(message.opcode, OP_CHAR_LIST);
        assert_eq!(message.sender, TEST_CLIENT_ID, "addressed with the id we assigned");
        assert_eq!(u32::from_le_bytes(message.body[0..4].try_into().unwrap()), 1);
    }

    #[test]
    fn empty_slots_are_all_zeros() {
        let state = state_with(vec![]);
        let message = decode(&reply(&state, 124));
        assert!(message.body[12..].iter().all(|&b| b == 0), "slots vazios devem ser zerados");
    }

    #[test]
    fn character_lands_in_its_slot_with_delphi_quirks() {
        let state = state_with(vec![dev_character("Athus", 1)]);
        let message = decode(&reply(&state, 124));

        let slot0 = &message.body[12..12 + CHAR_ENTRY_SIZE];
        let slot1 = &message.body[12 + CHAR_ENTRY_SIZE..12 + CHAR_ENTRY_SIZE * 2];
        assert!(slot0.iter().all(|&b| b == 0), "slot 0 continua vazio");

        assert_eq!(read_fixed_str(&slot1[0..16]), "Athus");
        assert_eq!(u16::from_le_bytes(slot1[16..18].try_into().unwrap()), 2, "nation");
        assert_eq!(u16::from_le_bytes(slot1[18..20].try_into().unwrap()), 1, "base class");
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

    #[test]
    fn entering_the_world_sends_the_delphi_sequence() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);

        let enter = enter_world(0);
        let Action::Reply(frames) = handle_message(&state, &mut session, &enter) else {
            panic!("expected the world entry sequence");
        };
        assert_eq!(frames.len(), 5, "0xCCCC, three 0x186 and the 0x925");

        let opcodes: Vec<u16> = frames.iter().map(|f| decode(f).opcode).collect();
        assert_eq!(
            opcodes,
            vec![OP_SIGNAL_READY, OP_SIGNAL_LOAD, OP_SIGNAL_LOAD, OP_SIGNAL_LOAD, OP_SEND_TO_WORLD]
        );

        // the signals are TSignalData: header plus one DWORD
        assert_eq!(frames[0].len(), 16);
        assert_eq!(decode(&frames[0]).body, 1u32.to_le_bytes());

        // and the character is exactly the size the client expects
        let world = &frames[4];
        assert_eq!(world.len(), SEND_TO_WORLD_SIZE, "0x925 must be 6400 bytes");

        let message = decode(world);
        assert_eq!(message.sender, SEND_TO_WORLD_INDEX, "the Index is fixed in this packet");
        assert_eq!(u32::from_le_bytes(message.body[0..4].try_into().unwrap()), 1, "serial");

        use character_offset as off;
        let ch = &message.body[4..];
        assert_eq!(ch.len(), CHARACTER_SIZE);
        assert_eq!(read_fixed_str(&ch[off::NAME..off::NAME + 16]), "Athus");
        assert_eq!(ch[off::NATION], 2);
        assert_eq!(ch[off::CLASS_INFO], 1, "base class derived from index 20");
        assert_eq!(
            u16::from_le_bytes(ch[off::LEVEL..off::LEVEL + 2].try_into().unwrap()),
            29,
            "level 30 travels as 29"
        );
        assert_eq!(
            u16::from_le_bytes(ch[off::EQUIP..off::EQUIP + 2].try_into().unwrap()),
            20,
            "Equip[0] is the class"
        );
        assert_eq!(
            u16::from_le_bytes(ch[off::EQUIP + 20..off::EQUIP + 22].try_into().unwrap()),
            7702,
            "Equip[1] is the hair"
        );
        assert_eq!(u64::from_le_bytes(ch[off::GOLD..off::GOLD + 8].try_into().unwrap()), 1234);
        assert!(
            u32::from_le_bytes(ch[off::MAX_HP..off::MAX_HP + 4].try_into().unwrap()) > 0,
            "without health the character is born dead"
        );
    }

    /// `0x349` is what pulls the character out of limbo: without it the client
    /// world but never learns where to put the body.
    #[test]
    fn client_ready_spawns_the_character_at_its_position() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));

        let ready = Message { sender: 7, opcode: OP_CLIENT_READY, time: 0, body: Vec::new() };
        let Action::Reply(frames) = handle_message(&state, &mut session, &ready) else {
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
    #[test]
    fn spawning_happens_only_once_per_session() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));

        let ready = Message { sender: 7, opcode: OP_CLIENT_READY, time: 0, body: Vec::new() };
        assert!(matches!(handle_message(&state, &mut session, &ready), Action::Reply(_)));
        assert!(
            matches!(handle_message(&state, &mut session, &ready), Action::Ignore),
            "the second 0xF0B must not respawn the player"
        );
    }

    #[test]
    fn movement_updates_the_position_without_answering() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));

        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&3500.0f32.to_le_bytes());
        body[4..8].copy_from_slice(&720.5f32.to_le_bytes());
        body[Movement::SPEED] = 50;
        let move_msg = Message { sender: 7, opcode: OP_MOVE, time: 0, body };

        // the original returns nothing to the mover
        assert!(matches!(handle_message(&state, &mut session, &move_msg), Action::Ignore));

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), (3500, 720));
    }

    /// The real client sends move types other than plain walking (16 has been
    /// seen). The original drops those; we still track the position, because a
    /// stale one would be wrong once positions are persisted.
    /// A packet claiming somebody else's id still moves only the player who
    /// sent it: the header field is never used to pick a target.
    #[test]
    fn movement_never_moves_another_player() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));

        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&4200.0f32.to_le_bytes());
        body[4..8].copy_from_slice(&700.0f32.to_le_bytes());
        let forged = Message { sender: 999, opcode: OP_MOVE, time: 0, body };

        let _ = handle_message(&state, &mut session, &forged);

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), (4200, 700), "our own player moved");
    }

    #[test]
    fn movement_tracks_move_types_other_than_walking() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));

        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&4000.0f32.to_le_bytes());
        body[4..8].copy_from_slice(&800.0f32.to_le_bytes());
        body[Movement::MOVE_TYPE] = 16;
        let message = Message { sender: 7, opcode: OP_MOVE, time: 0, body };

        assert!(matches!(handle_message(&state, &mut session, &message), Action::Ignore));
        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), (4000, 800));
    }

    #[test]
    fn movement_refuses_client_teleport_and_other_senders() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));
        let start = (session.character.as_ref().unwrap().x, session.character.as_ref().unwrap().y);

        // teleport must never come from the client: that is the map-jump exploit
        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&9999.0f32.to_le_bytes());
        body[Movement::MOVE_TYPE] = 1;
        let teleport = Message { sender: 7, opcode: OP_MOVE, time: 0, body: body.clone() };
        let _ = handle_message(&state, &mut session, &teleport);

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), start, "teleport must not have moved us");
    }

    #[test]
    fn client_ready_before_choosing_a_character_is_ignored() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let ready = Message { sender: 7, opcode: OP_CLIENT_READY, time: 0, body: Vec::new() };
        // does not drop the connection: there is simply nothing to spawn
        assert!(matches!(handle_message(&state, &mut session, &ready), Action::Ignore));
    }

    #[test]
    fn entering_an_empty_slot_is_refused() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        assert!(matches!(
            handle_message(&state, &mut session, &enter_world(2)),
            Action::Disconnect
        ));
    }

    /// Entering the world without having logged in on the same connection must
    /// not work: that is what kept two players from coexisting.
    #[test]
    fn entering_the_world_requires_logging_in_first() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        assert!(matches!(
            handle_message(&state, &mut Session::default(), &enter_world(0)),
            Action::Disconnect
        ));
    }

    #[test]
    fn refuses_wrong_client_version() {
        let state = state_with(vec![]);
        // the server drops the connection instead of leaving the client waiting
        assert!(matches!(handle_message(&state, &mut Session::default(), &login_message("admin", 123)), Action::Disconnect));
        assert!(matches!(handle_message(&state, &mut Session::default(), &login_message("admin", 0)), Action::Disconnect));
    }

    #[test]
    fn refuses_unknown_account() {
        let state = state_with(vec![]);
        assert!(matches!(
            handle_message(&state, &mut Session::default(), &login_message("ninguem", 124)),
            Action::Disconnect
        ));
    }

    #[test]
    fn unimplemented_opcode_is_ignored() {
        let state = state_with(vec![]);
        // a not-yet-implemented packet must not drop someone who is logged in
        let message = Message { sender: 0, opcode: 0x301, time: 0, body: vec![0; 32] };
        assert!(matches!(handle_message(&state, &mut Session::default(), &message), Action::Ignore));
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
    #[test]
    fn the_townspeople_appear_when_the_player_enters_the_world() {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        state.world = World::with_npcs(vec![
            npc(2050, "Merchant", 3455.0, 700.0),  // 12 away from the spawn
            npc(2051, "Skill Master", 3460.0, 695.0), // 11 away
            npc(2500, "Far Away", 9000.0, 9000.0),
        ]);

        let mut session = logged_in(&state);
        let Action::Reply(frames) = handle_message(&state, &mut session, &enter_world(0)) else {
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
    #[test]
    fn an_npc_is_placed_once_and_removed_once() {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        state.world = World::with_npcs(vec![npc(2050, "Merchant", 3455.0, 700.0)]);

        let mut session = logged_in(&state);
        handle_message(&state, &mut session, &enter_world(0));
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
    #[test]
    fn an_npc_just_outside_the_watch_radius_is_not_forgotten_yet() {
        let mut state = state_with(vec![dev_character("Athus", 0)]);
        state.world = World::with_npcs(vec![npc(2050, "Merchant", 3450.0, 690.0)]);

        let mut session = logged_in(&state);
        handle_message(&state, &mut session, &enter_world(0));
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
}
