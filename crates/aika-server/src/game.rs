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
use aika_net::frame::{self, FrameError, FrameReader, Message, MIN_FRAME};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
    /// The character already spawned. The client resends `0xF0B` whenever it
    /// thinks something is missing, and spawning again teleports the player to the
    /// starting point, which is the original's `if IsInstantiated then Exit`
    /// (`Mob/Player.pas:4967`).
    spawned: bool,
}

async fn handle_connection(state: Arc<State>, mut stream: TcpStream) -> anyhow::Result<()> {
    let mut reader = FrameReader::new();
    let mut prefix = LeadingPrefix::default();
    let mut session = Session::default();
    let mut buf = [0u8; 8192];

    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        if let Some(data) = prefix.feed(&buf[..n]) {
            reader.push(data);
        }

        while let Some(message) = reader.next_message() {
            match message {
                Ok(message) => match handle_message(&state, &mut session, &message) {
                    Action::Reply(frames) => {
                        for frame in frames {
                            stream.write_all(&frame).await?;
                        }
                        stream.flush().await?;
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
        OP_CLIENT_READY => handle_client_ready(session),
        OP_MOVE => handle_move(session, message),
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

    let frame = encode_char_list(&account, message.sender, state.uptime_ms());
    session.client_id = message.sender;
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
fn handle_client_ready(session: &mut Session) -> Action {
    let Some(character) = session.character.clone() else {
        warn!("0xF0B before a character was chosen");
        return Action::Ignore;
    };

    // Spawning twice throws the player back to the starting point, and the
    // client resends this packet whenever it thinks something is missing,
    // to walk. Without this guard it gets stuck in a respawn loop.
    if session.spawned {
        debug!(character = %character.name, "repeated 0xF0B, ignoring");
        return Action::Ignore;
    }

    info!(
        character = %character.name,
        x = character.x,
        y = character.y,
        "nascendo no mapa"
    );
    session.spawned = true;
    Action::Reply(vec![encode_spawn(&character, session.client_id)])
}

/// `0x301`: the player walked.
///
/// The original server **returns nothing to the mover**: the client moves
/// on its own and the server only relays the same packet to whoever can see
/// the player (`SendToVisible(..., false)` in `PacketHandlers.pas:892`).
/// While there is no registry of online players, storing the position does.
fn handle_move(session: &mut Session, message: &Message) -> Action {
    let Some(movement) = Movement::parse(&message.body) else {
        warn!(size = message.body.len(), "0x301 packet too short");
        return Action::Ignore;
    };

    if message.sender != session.client_id {
        warn!(from = message.sender, expected = session.client_id, "0x301 from another client");
        return Action::Ignore;
    }

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

    if let Some(character) = session.character.as_mut() {
        character.x = movement.x as u32;
        character.y = movement.y as u32;
    }

    // Nothing goes back to the mover: the client moves on its own. Relaying to
    // the players who can see them lands with the online player registry.
    Action::Ignore
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

    /// Faz o login e devolve os bytes da resposta.
    fn reply(state: &State, version: u16) -> Vec<u8> {
        match handle_message(state, &mut Session::default(), &login_message("admin", version)) {
            Action::Reply(frames) => frames.into_iter().next().expect("sem frames"),
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    /// A session that already went through `0x685`, as a real connection would.
    fn logged_in(state: &State) -> Session {
        let mut session = Session::default();
        let action = handle_message(state, &mut session, &login_message("admin", 124));
        assert!(matches!(action, Action::Reply(_)), "o login precisa passar");
        session
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
        assert_eq!(message.sender, 7, "the client expects its own id back");
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
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), CREATE_MOB_SIZE, "the client expects 508 bytes");

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
        session.client_id = 7;

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
    #[test]
    fn movement_tracks_move_types_other_than_walking() {
        let state = state_with(vec![dev_character("Athus", 0)]);
        let mut session = logged_in(&state);
        let _ = handle_message(&state, &mut session, &enter_world(0));
        session.client_id = 7;

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
        session.client_id = 7;
        let start = (session.character.as_ref().unwrap().x, session.character.as_ref().unwrap().y);

        // teleport must never come from the client: that is the map-jump exploit
        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&9999.0f32.to_le_bytes());
        body[Movement::MOVE_TYPE] = 1;
        let teleport = Message { sender: 7, opcode: OP_MOVE, time: 0, body: body.clone() };
        let _ = handle_message(&state, &mut session, &teleport);

        // and moving another client is not allowed either
        body[Movement::MOVE_TYPE] = MOVE_NORMAL;
        let outro = Message { sender: 99, opcode: OP_MOVE, time: 0, body };
        let _ = handle_message(&state, &mut session, &outro);

        let character = session.character.as_ref().unwrap();
        assert_eq!((character.x, character.y), start, "the position must not have changed");
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
}
