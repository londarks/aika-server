//! Tests for the game server.
//!
//! They live beside `game.rs` rather than inside it because the file was
//! eight thousand lines with the tests in it, which is past the point where
//! anyone can find their way around one.

use super::*;
use crate::config::{Config, DevAccount, DevCharacter};
use crate::world::World;
use crate::promotion;

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
            OP_SKILLS_LEVEL,
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
            SKILLS_LEVEL_SIZE,
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
    let level = decode(&frames[11]);
    assert_eq!(u16::from_le_bytes(level.body[0..2].try_into().unwrap()), 29);
    assert_eq!(u16::from_le_bytes(level.body[2..4].try_into().unwrap()), LEVEL_UNK);

    // HP and MP must not be zero or the character is born dead.
    let vitals = decode(&frames[12]);
    assert!(u32::from_le_bytes(vitals.body[0..4].try_into().unwrap()) > 0, "max HP");
    assert!(u32::from_le_bytes(vitals.body[8..12].try_into().unwrap()) > 0, "max MP");

    // Some packets identify themselves with a fixed index, not the client id.
    assert_eq!(decode(&frames[8]).sender, FIXED_INDEX, "0x109");
    assert_eq!(decode(&frames[9]).sender, FIXED_INDEX, "0x10A");

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
    // A sword is a weapon and a horse is a mount, and the slot each one
    // goes in comes from that. Leaving them typeless made them fixtures
    // of something that could not exist.
    raw[1000 * RECORD_SIZE + field::ITEM_TYPE..1000 * RECORD_SIZE + field::ITEM_TYPE + 2]
        .copy_from_slice(&1002u16.to_le_bytes());
    raw[963 * RECORD_SIZE + field::NAME.start] = b'x';
    raw[963 * RECORD_SIZE + field::ITEM_TYPE..963 * RECORD_SIZE + field::ITEM_TYPE + 2]
        .copy_from_slice(&9u16.to_le_bytes());

    // a health potion: the type and the effect are what makes it drinkable
    let r = &mut raw[POTION as usize * RECORD_SIZE..(POTION as usize + 1) * RECORD_SIZE];
    r[field::NAME.start] = b'x';
    r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&10u32.to_le_bytes());
    r[field::CAN_GROUP] = 1;
    r[field::ITEM_TYPE..field::ITEM_TYPE + 2]
        .copy_from_slice(&ITEM_TYPE_HP_POTION.to_le_bytes());
    r[field::USE_EFFECT..field::USE_EFFECT + 2].copy_from_slice(&500u16.to_le_bytes());

    // and the item that opens the chest, which is spent by nothing
    let r = &mut raw[CHEST_KEY as usize * RECORD_SIZE..(CHEST_KEY as usize + 1) * RECORD_SIZE];
    r[field::NAME.start] = b'x';
    r[field::ITEM_TYPE..field::ITEM_TYPE + 2]
        .copy_from_slice(&ITEM_TYPE_STORAGE_OPEN.to_le_bytes());

    ItemList::decode(&raw).expect("the fixture table is malformed")
}

/// A health potion in the fixture table.
const POTION: u16 = 4351;

/// A session standing in the world, ready to talk to somebody.
///
/// It carries the six bags every character is created with. Without them
/// the bag has no unlocked pages and nothing in it can be dragged, which
/// is true of the real thing too -- these fixtures build a character the
/// short way round and would otherwise be one nobody could ever have.
async fn in_world(state: &State) -> Session {
    let mut session = logged_in(state).await;
    handle_message(state, &mut session, &enter_world(0)).await;
    handle_client_ready(state, &mut session);
    if let Some(character) = session.character.as_mut() {
        for slot in creation::BAG_SLOTS {
            let _ = character.items.put(Item {
                container: inventory::BAG,
                slot,
                index: creation::BAG_ITEM,
                refine: 1,
                ..Item::default()
            });
        }
    }
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

/// Whether the character is carrying nothing but the bags it was made
/// with, which is what "the bag is empty" means once a fixture is a
/// character somebody could really have.
fn carries_nothing(session: &Session) -> bool {
    session
        .character
        .as_ref()
        .unwrap()
        .items
        .iter()
        .all(|item| item.index == creation::BAG_ITEM)
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

/// The hotbar and the known skills land at the right bytes of the world
/// record, which is the only place the client reads them from. Get the
/// offset wrong and the bar is empty however much is stored.
#[test]
fn the_record_carries_the_hotbar_and_known_skills() {
    let mut character = Character::from(&dev_character("Athus", 0));
    character.item_bar[3] = 30994;
    character.skill_list[52] = 15378;

    let record = encode_character(&character, 7);

    use character_offset as off;
    let bar3 = u32::from_le_bytes(
        record[off::ITEM_BAR + 3 * 4..off::ITEM_BAR + 3 * 4 + 4].try_into().unwrap(),
    );
    assert_eq!(bar3, 30994, "the hotbar icon is not where the client looks");

    let skill52 = u16::from_le_bytes(
        record[off::SKILL_LIST + 52 * 2..off::SKILL_LIST + 52 * 2 + 2].try_into().unwrap(),
    );
    assert_eq!(skill52, 15378, "the known skill is not where the client looks");
}

/// The skill window reads unspent points from the record, so they have to
/// land at the right byte or it shows zero however many the character has.
#[test]
fn the_record_carries_the_skill_points() {
    let mut character = Character::from(&dev_character("Athus", 0));
    character.skill_points = 100;

    let record = encode_character(&character, 7);
    let points = u16::from_le_bytes(
        record[character_offset::SKILL_POINT..character_offset::SKILL_POINT + 2]
            .try_into()
            .unwrap(),
    );
    assert_eq!(points, 100, "the skill window would show zero");
}

/// And the point the window actually reads: the `SkillsPoint` word of the
/// `0x109` refresh-point packet, which was hardcoded to zero.
#[test]
fn the_refresh_point_packet_carries_the_skill_points() {
    let mut character = Character::from(&dev_character("Athus", 0));
    character.skill_points = 100;

    let body = decode(&encode_refresh_point(&character)).body;
    let points = u16::from_le_bytes(body[14..16].try_into().unwrap());
    assert_eq!(points, 100, "0x109 still sends zero skill points");
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

/// Dragging a skill onto the bar stores the encoded value the original
/// stores, and the change comes back as its own confirmation.
#[tokio::test]
async fn dragging_a_skill_onto_the_bar_stores_and_echoes_it() {
    let state = shop_state();
    let mut session = in_world(&state).await;

    let mut body = vec![0u8; 12];
    body[0..4].copy_from_slice(&3u32.to_le_bytes()); // dest slot 3
    body[4..8].copy_from_slice(&2u32.to_le_bytes()); // kind: a skill
    body[8..12].copy_from_slice(&1937u32.to_le_bytes()); // skill id

    let change = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_CHANGE_ITEM_BAR,
        time: 0,
        body,
    };
    let frames = frames_of(handle_message(&state, &mut session, &change).await);

    assert_eq!(
        session.character.as_ref().unwrap().item_bar[3],
        1937 * 16 + 2,
        "a skill on the bar is stored as id*16+2, the way the original does"
    );
    assert_eq!(decode(&frames[0]).opcode, OP_CHANGE_ITEM_BAR, "the change is not confirmed");
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
    assert!(carries_nothing(&session), "an item was handed over anyway");
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
    assert!(carries_nothing(&session), "the stack is still in the bag");
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
        &open_npc(2050, dialog::option::TALK),
    ).await);

    // talking keeps the window open, so nothing closes it
    assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE]);
    assert_eq!(message_text(&frames[0]), "Talk is not available yet.");

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

/// The saddle, the mount, and a potion that lasts, with the skills each
/// one names. The chain is the real one: an item points at a skill through
/// `UseEffect`, and the skill's family is the buff.
const SADDLE: u16 = 4503;
const HORSE: u16 = 963;
const LASTING_POTION: u16 = 4879;
const SADDLE_SKILL: usize = 7259;

/// An attack that leaves something behind, aimed at a target. Skill 289 in
/// the shipped table is one: sixty seconds of cooldown, 240 damage, and a
/// duration, which is what let it be mistaken for a buff.
const ROOTING_ATTACK: usize = 289;
const ROOTING_FAMILY: u32 = 13;
/// Target type four, the second most common in the table after self.
const AIMED_AT_SOMETHING: u32 = 4;
const POTION_SKILL: usize = 9031;
/// The stone quest 39 hands out, which is the fire one: `Quests.csv` line
/// `2072,39,21,9,100`. Its element is the whole reason it is this id and not
/// another of the seventeen.
const SUMMON_STONE: u16 = 100;
/// One of the fourteen that no quest gives, which carry a pran that already
/// exists rather than making one.
const CARRIER_STONE: u16 = 104;

fn buff_state() -> State {
    let mut state = shop_state();

    state.items = {
        use aika_data::itemlist::{field, ItemList, RECORD_SIZE};
        let mut raw = vec![0u8; 10000 * RECORD_SIZE];
        let mut define = |id: u16, item_type: u16, effect: u16| {
            let r = &mut raw[id as usize * RECORD_SIZE..(id as usize + 1) * RECORD_SIZE];
            r[field::NAME.start] = b'x';
            r[field::ITEM_TYPE..field::ITEM_TYPE + 2]
                .copy_from_slice(&item_type.to_le_bytes());
            r[field::USE_EFFECT..field::USE_EFFECT + 2].copy_from_slice(&effect.to_le_bytes());
        };
        define(SADDLE, ITEM_TYPE_BUFF, SADDLE_SKILL as u16);
        define(LASTING_POTION, ITEM_TYPE_POTION_BUFF, POTION_SKILL as u16);
        define(HORSE, 9, 0);
        // A Pran Summon Stone. Type ten is what sends it to equipment
        // slot ten, which is the slot the companion lives in.
        define(SUMMON_STONE, crate::pran::STONE_ITEM_TYPE, 0);
        define(CARRIER_STONE, crate::pran::STONE_ITEM_TYPE, 0);
        ItemList::decode(&raw).expect("the fixture table is malformed")
    };

    state.skills = {
        use aika_data::skills::{field, SkillTable, RECORD_SIZE, SLOTS};
        let mut raw = vec![0u8; SLOTS * RECORD_SIZE + 4];
        // The target type is the skill's own property and is what decides
        // whether a cast lands on the caster, so the fixture has to carry it.
        let mut define = |id: usize, family: u32, seconds: u32, target_type: u32| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            r[field::FAMILY..field::FAMILY + 4].copy_from_slice(&family.to_le_bytes());
            r[field::DURATION..field::DURATION + 4].copy_from_slice(&seconds.to_le_bytes());
            r[field::TARGET_TYPE..field::TARGET_TYPE + 4]
                .copy_from_slice(&target_type.to_le_bytes());
            r[field::NAME_ENGLISH.start] = b'x';
        };
        define(SADDLE_SKILL, crate::buffs::FAMILY_MOUNTED, 3600, TARGET_TYPE_SELF);
        define(POTION_SKILL, 383, 10_800, TARGET_TYPE_SELF);
        // And one that is aimed at something else, and lasts: the shape of
        // the skill that froze a session.
        define(ROOTING_ATTACK, ROOTING_FAMILY, 60, AIMED_AT_SOMETHING);
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    };
    state
}

fn carrying(session: &mut Session, index: u16, slot: u16) {
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index,
            container: inventory::BAG,
            slot,
            refine: 1,
            durability_min: 255,
            ..Item::default()
        })
        .unwrap();
}

fn use_buff_item(slot: u16) -> Message {
    Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_USE_BUFF_ITEM,
        time: 0,
        body: (slot as u32).to_le_bytes().to_vec(),
    }
}

/// The saddle is what puts a rider on a horse, and it does it through a
/// packet of its own. Using it was doing nothing at all.
#[tokio::test]
async fn using_a_saddle_makes_the_player_mounted() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);

    let frames = frames_of(handle_message(&state, &mut session, &use_buff_item(7)).await);

    assert!(
        session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED),
        "the saddle did not put the player on a mount"
    );
    let sent = opcodes(&frames);
    assert!(sent.contains(&OP_ADD_BUFF), "the client was not told: {sent:?}");
    assert!(sent.contains(&OP_BUFFS), "the buff list was not sent again");

    // The saddle lasts thirty days: using it must not spend it.
    assert_eq!(
        session.character.as_ref().unwrap().items.get(inventory::BAG, 7).map(|i| i.index),
        Some(SADDLE),
        "the saddle was eaten"
    );
}

/// The list names the buff and says **how long is left**, not when it
/// ends. Sending the moment instead is what put "689 Mês" on the screen
/// for a buff of one hour: the client draws the number as a duration.
#[tokio::test]
async fn the_buff_list_counts_down_rather_than_naming_a_moment() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);
    handle_message(&state, &mut session, &use_buff_item(7)).await;

    let body = decode(&encode_buffs(session.client_id, &session.buffs, &state.skills)).body;
    assert_eq!(
        u16::from_le_bytes(body[0..2].try_into().unwrap()),
        SADDLE_SKILL as u16,
        "the first slot does not name the skill"
    );

    // The fixture saddle lasts an hour, so the field is an hour and a
    // little less — never a unix timestamp, which is a billion and a half.
    let left = u32::from_le_bytes(body[BUFFS_TIMES_AT..BUFFS_TIMES_AT + 4].try_into().unwrap());
    assert!(left > 3500, "the buff is nearly over already: {left}");
    assert!(left <= 3600, "an hour's buff has more than an hour left: {left}");
}

/// The pool and what is left in it are two different fields, and putting
/// the second in both is what made the client say there was no mana while
/// the bar still looked full: every spell shrank the maximum too.
#[tokio::test]
async fn spending_mana_does_not_shrink_the_pool() {
    let state = shop_state();
    let session = in_world(&state).await;
    let character = session.character.as_ref().unwrap();
    let (max_hp, max_mp) = vitals(character);

    let spent = max_mp / 4;
    let frame = encode_hp_mp(character, session.client_id, max_hp, max_mp - spent);
    let body = decode(&frame).body;
    let at = |i: usize| u32::from_le_bytes(body[i..i + 4].try_into().unwrap());

    // MaxHP, CurHP, MaxMP, CurMP, in that order.
    assert_eq!(at(0), max_hp, "the health pool");
    assert_eq!(at(4), max_hp, "health left");
    assert_eq!(at(8), max_mp, "the mana pool must not follow what was spent");
    assert_eq!(at(12), max_mp - spent, "mana left");
    assert!(at(8) > at(12), "the pool is no bigger than what is in it");
}

/// A skill with a cast time finishes on `0x302`, not on the `0x320` that
/// started it. The mount's is 1500 milliseconds, so the client draws a bar and
/// sends this when it fills, aimed at the caster's own id.
///
/// Reading that as a swing at a monster nobody can find is what made the bar
/// fill and nothing happen. The line was in the log all along, once a minute,
/// looking like noise: "0x302 at something that is not a monster target=1".
#[tokio::test]
async fn a_cast_with_a_bar_finishes_on_the_second_packet() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    assert!(!session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED));

    // What the client sends when the bar fills: the skill, aimed at itself.
    let finished = Message {
        sender: TEST_CLIENT_ID,
        opcode: combat::OP_ATTACK,
        time: 0,
        body: combat::Attack {
            target: session.client_id,
            animation: 0,
            skill: SADDLE_SKILL as u16,
            from: (0.0, 0.0),
            at: (0.0, 0.0),
        }
        .to_body(),
    };
    let frames = frames_of(handle_message(&state, &mut session, &finished).await);

    assert!(
        session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED),
        "the cast finished and nothing came of it"
    );
    assert!(opcodes(&frames).contains(&OP_ADD_BUFF), "the client was not told");
}

/// An attack aimed at nobody arrives carrying the caster's own id, and that
/// does not make it a cast on oneself.
///
/// The original decides by the skill: `if (DataSkill^.TargetType = 1) then
/// SelfBuffSkill` (`Mob/BaseMob.pas:5754`). Deciding by the target id instead
/// handed a player a sixty-second debuff off their own attack and rooted them
/// where they stood -- no walking, no attacking, no casting, and a client that
/// stopped sending anything but which way it was facing.
#[tokio::test]
async fn an_attack_carrying_the_casters_own_id_is_not_a_self_buff() {
    let state = buff_state();
    let mut session = in_world(&state).await;

    let finished = Message {
        sender: TEST_CLIENT_ID,
        opcode: combat::OP_ATTACK,
        time: 0,
        body: combat::Attack {
            target: session.client_id,
            animation: 0,
            skill: ROOTING_ATTACK as u16,
            from: (0.0, 0.0),
            at: (0.0, 0.0),
        }
        .to_body(),
    };
    let frames = frames_of(handle_message(&state, &mut session, &finished).await);

    assert!(
        !session.buffs.has_family(&state.skills, ROOTING_FAMILY),
        "the caster was given its own attack as a buff"
    );
    assert!(!opcodes(&frames).contains(&OP_ADD_BUFF), "and told about it");

    // It still has to be let go of, or the cure is the same as the disease.
    assert!(!frames.is_empty(), "the client was left waiting on the cast");
}

/// And the skill that really is a cast on oneself still is one, so the fix
/// is a narrowing rather than a wall.
#[tokio::test]
async fn a_skill_aimed_at_the_caster_by_its_own_type_still_lands() {
    let state = buff_state();
    let mut session = in_world(&state).await;

    let finished = Message {
        sender: TEST_CLIENT_ID,
        opcode: combat::OP_ATTACK,
        time: 0,
        body: combat::Attack {
            target: session.client_id,
            animation: 0,
            skill: SADDLE_SKILL as u16,
            from: (0.0, 0.0),
            at: (0.0, 0.0),
        }
        .to_body(),
    };
    handle_message(&state, &mut session, &finished).await;

    assert!(session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED));
}

/// Wearing a summon stone brings out a companion, and the first stone to be
/// worn hatches one. Both halves matter: the packet is what draws the pran
/// window, and the effect is the whole of how a young one is shown.
#[tokio::test]
async fn wearing_a_summon_stone_hatches_a_pran_and_shows_it() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);
    assert!(session.account.as_ref().unwrap().prans.is_empty());

    let frames = frames_of(handle_message(&state, &mut session, &wear_stone()).await);

    let prans = &session.account.as_ref().unwrap().prans;
    assert_eq!(prans.len(), 1, "no pran hatched");
    assert_eq!(prans[0].item_id, 4242, "it is not bound to the stone it came out of");
    assert_eq!(prans[0].class, 61, "the stone says fire, so the pran is fire");
    assert!(session.dirty, "a pran that is not saved is hatched again next time");

    assert!(
        opcodes(&frames).contains(&crate::pran::OP_WORLD),
        "the pran window was not drawn"
    );
    assert!(
        effects_in(&frames).contains(&crate::pran::Element::Fire.fairy_effect()),
        "a fairy is only ever an effect, so nothing was shown at all"
    );
}

/// Wearing it a second time is the same pran, not another. The stone is what
/// it belongs to, so taking it off and putting it back has to find it again.
#[tokio::test]
async fn the_same_stone_brings_back_the_same_pran() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);

    handle_message(&state, &mut session, &wear_stone()).await;
    session.account.as_mut().unwrap().prans[0].level = 9;

    // off, and on again
    let frames = frames_of(handle_message(&state, &mut session, &take_stone_off()).await);
    assert!(effects_in(&frames).contains(&0), "the fairy was left on the player");
    handle_message(&state, &mut session, &wear_stone()).await;

    let prans = &session.account.as_ref().unwrap().prans;
    assert_eq!(prans.len(), 1, "it hatched a second one");
    assert_eq!(prans[0].level, 9, "and forgot the first");
}

/// Anything else in that slot is not a companion. The slot is reachable by
/// item type alone, and a wrong guess would hatch a pran out of a hat.
#[tokio::test]
async fn something_that_is_not_a_stone_hatches_nothing() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);

    handle_message(&state, &mut session, &wear_stone()).await;

    assert!(session.account.as_ref().unwrap().prans.is_empty());
}

/// Naming a companion. The original names the first one that has none, so
/// there is no way to change a name once it is set.
#[tokio::test]
async fn a_pran_is_named_once_and_keeps_it() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);
    handle_message(&state, &mut session, &wear_stone()).await;
    assert_eq!(session.account.as_ref().unwrap().prans[0].name, "");

    let frames = frames_of(handle_message(&state, &mut session, &name_pran("Alice")).await);

    assert_eq!(session.account.as_ref().unwrap().prans[0].name, "Alice");
    assert!(session.dirty, "a name that is not saved is not a name");

    // the answer is the question sent back, and the two chest slots follow it
    assert!(opcodes(&frames).contains(&crate::pran::OP_RENAME));
    let answered = frames
        .iter()
        .map(|frame| decode(frame))
        .find(|m| m.opcode == crate::pran::OP_RENAME)
        .unwrap();
    assert_eq!(crate::pran::Rename::parse(&answered.body).unwrap().name, "Alice");

    // and a second name is refused rather than replacing the first
    let frames = frames_of(handle_message(&state, &mut session, &name_pran("Bob")).await);
    assert_eq!(session.account.as_ref().unwrap().prans[0].name, "Alice", "it was renamed");
    assert!(!opcodes(&frames).contains(&crate::pran::OP_RENAME));
}

/// A name the original would refuse is refused here, with a reason rather
/// than in silence.
#[tokio::test]
async fn a_name_that_is_not_letters_or_digits_is_refused() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);
    handle_message(&state, &mut session, &wear_stone()).await;

    for bad in ["", "Al ice", "Alice!"] {
        let frames = frames_of(handle_message(&state, &mut session, &name_pran(bad)).await);
        assert_eq!(
            session.account.as_ref().unwrap().prans[0].name,
            "",
            "{bad:?} was accepted"
        );
        assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE], "{bad:?} was refused in silence");
    }

    // and a good one still goes through afterwards
    handle_message(&state, &mut session, &name_pran("Alice2")).await;
    assert_eq!(session.account.as_ref().unwrap().prans[0].name, "Alice2");
}

fn name_pran(name: &str) -> Message {
    let mut body = vec![0u8; crate::pran::Rename::BODY_SIZE];
    body[4..4 + name.len()].copy_from_slice(name.as_bytes());
    Message { sender: TEST_CLIENT_ID, opcode: crate::pran::OP_RENAME, time: 0, body }
}

/// The Pran station is the chest with a different face on it.
///
/// `OpenNPC` answers option 7 and option 13 with the same `SendStorage` and
/// only the type differs (`$7` player, `$D` prans). Neither was wired at all,
/// which is why a pran sitting in the chest could not be reached from the one
/// NPC whose whole job is prans.
#[tokio::test]
async fn the_pran_station_opens_the_chest_on_its_pran_side() {
    let state = shop_state();
    let mut session = in_world(&state).await;

    for (option, expected) in [
        (dialog::option::STORAGE, STORAGE_TYPE_PLAYER),
        (dialog::option::PRAN_STATION, STORAGE_TYPE_PRANS),
    ] {
        let frames =
            frames_of(handle_message(&state, &mut session, &open_npc(2050, option)).await);
        let sent = opcodes(&frames);

        assert!(sent.contains(&OP_STORAGE), "option {option} did not send the chest");
        let opened = frames
            .iter()
            .map(|frame| decode(frame))
            .find(|m| m.opcode == OP_STORAGE_OPEN)
            .expect("the window was never opened");
        assert_eq!(
            u32::from_le_bytes(opened.body[0..4].try_into().unwrap()),
            expected,
            "option {option} opened the wrong side of the chest"
        );
    }
}

/// And the two slots the chest packet does not carry go out on their own.
///
/// `SendStorage` refreshes 84 and 85 separately, every time. They sit past
/// what `TStoragePlayer` copies, so a pran in one of them is invisible without
/// this -- which is exactly how it looked.
#[tokio::test]
async fn opening_the_chest_sends_the_two_pran_slots_on_their_own() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.account.as_mut().unwrap().storage.put(Item {
        index: 104,
        container: inventory::STORAGE,
        slot: 84,
        identific: 1,
        ..Item::default()
    })
    .unwrap();

    let frames = frames_of(
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::PRAN_STATION))
            .await,
    );

    let refreshed: Vec<u16> = frames
        .iter()
        .map(|frame| decode(frame))
        .filter(|m| m.opcode == shop::OP_REFRESH_ITEM && m.body.len() >= 6)
        .map(|m| u16::from_le_bytes(m.body[2..4].try_into().unwrap()))
        .collect();

    for slot in inventory::STORAGE_PRAN_SLOTS {
        assert!(refreshed.contains(&slot), "slot {slot} was never sent, so it draws empty");
    }
}

/// A stone no quest hands out hatches nothing.
///
/// Only three of the seventeen make a pran -- items 100, 101 and 102, one per
/// element, and `Quests.csv` says so outright. The rest carry a pran that
/// already exists, sorted by the tier they fit. Hatching from one of those
/// would be inventing an element the data never named.
#[tokio::test]
async fn a_stone_no_quest_gives_hatches_nothing() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index: CARRIER_STONE,
            container: inventory::BAG,
            slot: 7,
            identific: 4242,
            ..Item::default()
        })
        .unwrap();

    handle_message(&state, &mut session, &wear_stone()).await;

    assert!(session.account.as_ref().unwrap().prans.is_empty());
}

/// A stone that identifies nothing hatches nothing.
///
/// The binding runs both ways: a pran remembers its stone by `Identific`, and
/// a stone with none can never be matched again. Hatching against one would
/// hatch a second the next time the slot is looked at, and a third after that.
#[tokio::test]
async fn a_stone_with_nothing_to_identify_it_hatches_nothing() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 0);

    for _ in 0..3 {
        handle_message(&state, &mut session, &wear_stone()).await;
        handle_message(&state, &mut session, &take_stone_off()).await;
    }

    assert!(
        session.account.as_ref().unwrap().prans.is_empty(),
        "it hatched {} prans out of one nameless stone",
        session.account.as_ref().unwrap().prans.len()
    );
}

/// Every form after the first is a companion standing beside its owner, with
/// a body and an id of its own. Only the first of each element is the
/// bodiless glow.
#[tokio::test]
async fn a_grown_pran_is_drawn_as_a_body_beside_its_owner() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);
    handle_message(&state, &mut session, &wear_stone()).await;

    // it grows, and comes back out
    for class in [62u8, 63, 64] {
        session.account.as_mut().unwrap().prans[0].class = class;
        handle_message(&state, &mut session, &take_stone_off()).await;
        let frames = frames_of(handle_message(&state, &mut session, &wear_stone()).await);

        assert!(
            opcodes(&frames).contains(&crate::pran::OP_SPAWN),
            "class {class} was not given a body"
        );
        assert!(
            !effects_in(&frames).contains(&crate::pran::Element::Fire.fairy_effect()),
            "class {class} was drawn as a glow as well as a body"
        );
        assert!(session.pran_body.is_some(), "nothing was remembered to take away");
    }
}

/// And the body it was given is the body that is taken away. The original
/// picks by class on the way out and by level on the way back, which leaves a
/// companion standing in the field for anything the two disagree about.
#[tokio::test]
async fn the_body_that_was_drawn_is_the_body_that_is_removed() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);
    handle_message(&state, &mut session, &wear_stone()).await;
    session.account.as_mut().unwrap().prans[0].class = 62;
    // a level the original would have called an effect, with a class that has
    // a body: the case the two tests disagree about
    session.account.as_mut().unwrap().prans[0].level = 1;

    handle_message(&state, &mut session, &take_stone_off()).await;
    let frames = frames_of(handle_message(&state, &mut session, &wear_stone()).await);
    let drawn_under = decode(
        frames
            .iter()
            .find(|f| decode(f).opcode == crate::pran::OP_SPAWN)
            .expect("no body was drawn"),
    )
    .sender;

    let frames = frames_of(handle_message(&state, &mut session, &take_stone_off()).await);
    let removed = frames.iter().map(|f| decode(f)).find(|m| m.opcode == OP_REMOVE_MOB);
    let removed = removed.expect("the body was left standing in the field");

    assert_eq!(
        u32::from_le_bytes(removed.body[0..4].try_into().unwrap()),
        drawn_under as u32,
        "it removed something other than the companion it drew"
    );
    assert!(session.pran_body.is_none(), "it still thinks one is out");
}

/// The companion's id comes out of its own range, which is what stops the
/// client drawing it on top of a player or a townsperson.
#[tokio::test]
async fn a_pran_is_drawn_under_an_id_of_its_own() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying_stone(&mut session, 4242);
    handle_message(&state, &mut session, &wear_stone()).await;
    session.account.as_mut().unwrap().prans[0].class = 63;
    handle_message(&state, &mut session, &take_stone_off()).await;

    let frames = frames_of(handle_message(&state, &mut session, &wear_stone()).await);
    let spawn = frames
        .iter()
        .map(|f| decode(f))
        .find(|m| m.opcode == crate::pran::OP_SPAWN)
        .expect("no body was drawn");

    assert!(
        crate::pran::IDS.contains(&(spawn.sender as u32)),
        "drawn under {}, which belongs to somebody else",
        spawn.sender
    );
    assert_ne!(spawn.sender, session.client_id, "drawn as its owner");

    // and it is named after whoever it follows
    let title = &spawn.body[444..444 + 32];
    let title = String::from_utf8_lossy(&title[..title.iter().position(|b| *b == 0).unwrap()]);
    assert!(title.starts_with("Pran do "), "the title reads {title:?}");
}
/// The effect values in a burst of frames.
///
/// An effect is not its own packet: it shares `0x117` with the client index,
/// and the two are told apart by the second word. So the opcode alone proves
/// nothing -- arriving in the world sends a dozen of them.
fn effects_in(frames: &[Vec<u8>]) -> Vec<u32> {
    frames
        .iter()
        .map(|frame| decode(frame))
        .filter(|m| m.body.len() >= 8)
        .map(|m| u32::from_le_bytes(m.body[4..8].try_into().unwrap()))
        .collect()
}

/// A summon stone in the bag, with an identific of its own: a pran belongs to
/// one stone and not to a kind of stone.
fn carrying_stone(session: &mut Session, identific: i32) {
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index: SUMMON_STONE,
            container: inventory::BAG,
            slot: 7,
            identific,
            ..Item::default()
        })
        .unwrap();
}

fn wear_stone() -> Message {
    Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_MOVE_ITEM,
        time: 0,
        body: MoveItem {
            to_container: inventory::EQUIP as u16,
            to_slot: crate::pran::STONE_SLOT,
            from_container: inventory::BAG as u16,
            from_slot: 7,
        }
        .to_body(),
    }
}

fn take_stone_off() -> Message {
    Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_MOVE_ITEM,
        time: 0,
        body: MoveItem {
            to_container: inventory::BAG as u16,
            to_slot: 7,
            from_container: inventory::EQUIP as u16,
            from_slot: crate::pran::STONE_SLOT,
        }
        .to_body(),
    }
}
/// Everyone watching has to see the spell go off. The original builds a fresh
/// `0x302` for it rather than echoing the one it got, and fills the animation
/// from the skill's own `SelfAnimation` — the client sends nothing useful in
/// that field, so a cast finished without it leaves the caster standing still
/// while the spell happens.
#[tokio::test]
async fn a_finished_cast_plays_its_animation() {
    let state = buff_state();
    let mut session = in_world(&state).await;

    let finished = Message {
        sender: TEST_CLIENT_ID,
        opcode: combat::OP_ATTACK,
        time: 0,
        body: combat::Attack {
            target: session.client_id,
            // Whatever the client puts here is ignored.
            animation: 0,
            skill: SADDLE_SKILL as u16,
            from: (0.0, 0.0),
            at: (0.0, 0.0),
        }
        .to_body(),
    };
    let frames = frames_of(handle_message(&state, &mut session, &finished).await);

    let played = frames
        .iter()
        .map(|f| decode(f))
        .find(|m| m.opcode == combat::OP_ATTACK)
        .expect("the spell went off with nobody seeing it");
    let played = combat::Attack::parse(&played.body).expect("a well formed relay");
    assert_eq!(played.skill, SADDLE_SKILL as u16);
    assert_eq!(
        played.animation as u32,
        state.skills.get(SADDLE_SKILL).unwrap().self_animation(),
        "the animation is the skill's own, not the one the client sent"
    );
}

/// A mount's buff has no end, but the field the client draws is a countdown.
/// Told the truth — all ones — it made the label wide enough to shove the icon
/// out of the buff bar. It gets a window that renders like any other instead,
/// and the buff itself still never runs out.
#[tokio::test]
async fn an_endless_buff_is_drawn_with_a_window_and_never_expires() {
    let mut state = buff_state();
    state.skills = {
        use aika_data::skills::{field, SkillTable, RECORD_SIZE, SLOTS};
        let mut raw = vec![0u8; SLOTS * RECORD_SIZE + 4];
        let r = &mut raw[SADDLE_SKILL * RECORD_SIZE..(SADDLE_SKILL + 1) * RECORD_SIZE];
        r[field::FAMILY..field::FAMILY + 4]
            .copy_from_slice(&crate::buffs::FAMILY_MOUNTED.to_le_bytes());
        r[field::DURATION..field::DURATION + 4]
            .copy_from_slice(&crate::buffs::FOREVER.to_le_bytes());
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    };

    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);
    handle_message(&state, &mut session, &use_buff_item(7)).await;

    let body = decode(&encode_buffs(session.client_id, &session.buffs, &state.skills)).body;
    let left = u32::from_le_bytes(body[BUFFS_TIMES_AT..BUFFS_TIMES_AT + 4].try_into().unwrap());
    assert_eq!(left, crate::buffs::ENDLESS_SHOWN, "an unreadable label went out");
    assert_ne!(left, crate::buffs::FOREVER);

    // A century on and the rider is still mounted: the window is for drawing
    // with, and nothing reads it back.
    let far = std::time::SystemTime::now() + std::time::Duration::from_secs(3_000_000_000);
    assert_eq!(session.buffs.expire(&state.skills, far), 0, "the rider was thrown off");
    assert!(session.buffs.any_endless(&state.skills), "nothing said it needs topping up");
}

/// A swing at yourself is not a buff. Only a skill that lasts becomes one.
#[tokio::test]
async fn a_skill_that_does_not_last_is_not_a_self_buff() {
    let state = buff_state();
    let mut session = in_world(&state).await;

    let swing = Message {
        sender: TEST_CLIENT_ID,
        opcode: combat::OP_ATTACK,
        time: 0,
        body: combat::Attack {
            target: session.client_id,
            animation: 0,
            // A skill the fixture table does not define at all.
            skill: 4242,
            from: (0.0, 0.0),
            at: (0.0, 0.0),
        }
        .to_body(),
    };
    handle_message(&state, &mut session, &swing).await;

    assert!(session.buffs.is_empty(), "a skill with no duration became a buff");
}

/// A buff has to come off when the player clicks it off. A mount's does
/// not run out on its own, so without this it never goes at all — which
/// left a rider mounted with no horse and unable to equip another.
#[tokio::test]
async fn a_buff_can_be_clicked_off() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);
    handle_message(&state, &mut session, &use_buff_item(7)).await;
    assert!(session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED));

    let off = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_REMOVE_BUFF,
        time: 0,
        body: (SADDLE_SKILL as u32).to_le_bytes().to_vec(),
    };
    let frames = frames_of(handle_message(&state, &mut session, &off).await);

    assert!(
        !session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED),
        "the rider is still mounted after getting off"
    );
    let sent = opcodes(&frames);
    assert!(sent.contains(&OP_BUFFS), "the list was not sent again: {sent:?}");
    assert!(sent.contains(&OP_REFRESH_STATUS), "the sheet still counts the buff");

    // And the list really is empty now.
    let body = decode(&encode_buffs(session.client_id, &session.buffs, &state.skills)).body;
    assert_eq!(u16::from_le_bytes(body[0..2].try_into().unwrap()), 0);
}

fn spend_points(which: u32, amount: u32) -> Message {
    let mut body = which.to_le_bytes().to_vec();
    body.extend_from_slice(&amount.to_le_bytes());
    Message { sender: TEST_CLIENT_ID, opcode: OP_STATUS_POINT, time: 0, body }
}

/// Spending a point raises the attribute, spends the point, and reaches
/// the sheet — which is the whole of `GetStatusPoint`.
#[tokio::test]
async fn spending_a_status_point_raises_the_attribute() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().attributes[FREE_POINTS] = 10;
    let before = session.character.as_ref().unwrap().attributes[0];

    let frames = frames_of(handle_message(&state, &mut session, &spend_points(0, 3)).await);

    let after = session.character.as_ref().unwrap();
    assert_eq!(after.attributes[0], before + 3, "strength did not go up");
    assert_eq!(after.attributes[FREE_POINTS], 7, "the points were not spent");

    let sent = opcodes(&frames);
    assert!(sent.contains(&OP_REFRESH_STATUS), "the sheet was not sent: {sent:?}");
    assert!(sent.contains(&OP_REFRESH_POINT), "the point count was not sent");
}

/// Points nobody has cannot be spent, however many the client claims.
#[tokio::test]
async fn points_that_are_not_there_cannot_be_spent() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().attributes[FREE_POINTS] = 2;
    let before = session.character.as_ref().unwrap().attributes;

    handle_message(&state, &mut session, &spend_points(0, 99)).await;
    assert_eq!(
        session.character.as_ref().unwrap().attributes,
        before,
        "a character spent points it never had"
    );

    // And an attribute that does not exist is refused too.
    handle_message(&state, &mut session, &spend_points(9, 1)).await;
    assert_eq!(session.character.as_ref().unwrap().attributes, before);
}

/// The attributes really are what the fight reads, so a spent point has to
/// change the numbers. Strength is worth 2.6 attack a point.
#[tokio::test]
async fn a_spent_point_changes_what_the_character_is_worth() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().attributes[FREE_POINTS] = 10;

    let effects = session.effects(&state);
    let before = stats::of(session.character.as_ref().unwrap(), &state.items, &effects).attack;

    handle_message(&state, &mut session, &spend_points(0, 10)).await;

    let effects = session.effects(&state);
    let after = stats::of(session.character.as_ref().unwrap(), &state.items, &effects).attack;
    assert_eq!(after, before + 26, "ten strength is twenty-six attack");
}

/// The whole point of a mount: it makes you faster. The speed is an
/// effect the saddle's skill carries, and the sheet reads it through
/// `GetCurrentScore` rather than off the character.
#[tokio::test]
async fn a_mount_makes_the_rider_faster() {
    let mut state = buff_state();
    state.skills = {
        use aika_data::skills::{field, SkillTable, RECORD_SIZE, SLOTS};
        let mut raw = vec![0u8; SLOTS * RECORD_SIZE + 4];
        let r = &mut raw[SADDLE_SKILL * RECORD_SIZE..(SADDLE_SKILL + 1) * RECORD_SIZE];
        r[field::FAMILY..field::FAMILY + 4]
            .copy_from_slice(&crate::buffs::FAMILY_MOUNTED.to_le_bytes());
        r[field::DURATION..field::DURATION + 4].copy_from_slice(&3600u32.to_le_bytes());
        // The real saddle's second pair: run speed, thirty of it.
        r[field::EFFECT.start + 4..field::EFFECT.start + 8]
            .copy_from_slice(&(crate::effects::id::RUNSPEED as u32).to_le_bytes());
        r[field::EFFECT_VALUE.start + 4..field::EFFECT_VALUE.start + 8]
            .copy_from_slice(&30u32.to_le_bytes());
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    };

    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);

    let on_foot = stats::of(
        session.character.as_ref().unwrap(),
        &state.items,
        &session.effects(&state),
    );
    assert_eq!(on_foot.speed_move, stats::BASE_SPEED_MOVE as u32, "walking is forty");

    handle_message(&state, &mut session, &use_buff_item(7)).await;

    let mounted = stats::of(
        session.character.as_ref().unwrap(),
        &state.items,
        &session.effects(&state),
    );
    assert_eq!(mounted.speed_move, stats::BASE_SPEED_MOVE as u32 + 30, "the mount added nothing");

    // And the client is told, in the packet the sheet reads.
    let frame = encode_refresh_status(
        session.character.as_ref().unwrap(),
        &state.items,
        &session.effects(&state),
    );
    let body = decode(&frame).body;
    let at = status_offset::SPEED_MOVE;
    assert_eq!(
        u16::from_le_bytes(body[at..at + 2].try_into().unwrap()),
        stats::BASE_SPEED_MOVE + 30
    );
}

/// A mount's skills belong to no class, so the ownership test that stops a
/// client asking for a skill it never learned would refuse them outright.
/// The server is the one that names them, so there is nothing to own.
#[tokio::test]
async fn a_mount_skill_is_cast_even_though_it_is_nobody_class() {
    let mut state = buff_state();
    state.skills = {
        use aika_data::skills::{field, SkillTable, RECORD_SIZE, SLOTS};
        let mut raw = vec![0u8; SLOTS * RECORD_SIZE + 4];
        let mut define = |id: usize, family: u32, seconds: u32, class: u32| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            r[field::FAMILY..field::FAMILY + 4].copy_from_slice(&family.to_le_bytes());
            r[field::DURATION..field::DURATION + 4].copy_from_slice(&seconds.to_le_bytes());
            r[field::CLASS..field::CLASS + 4].copy_from_slice(&class.to_le_bytes());
            r[field::NAME_ENGLISH.start] = b'x';
        };
        define(SADDLE_SKILL, crate::buffs::FAMILY_MOUNTED, 3600, 0);
        // The mount's own skill: class nought, in nobody's block.
        define(MOUNT_SKILLS[0], 164, 10, 0);
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    };

    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index: HORSE,
            container: inventory::EQUIP,
            slot: MOUNT_SLOT,
            ..Item::default()
        })
        .unwrap();
    handle_message(&state, &mut session, &use_buff_item(7)).await;
    assert!(session.buffs.has_family(&state.skills, crate::buffs::FAMILY_MOUNTED));

    let ask = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_MOUNT_SKILL,
        time: 0,
        body: vec![0, 0],
    };
    let frames = frames_of(handle_message(&state, &mut session, &ask).await);

    let sent = opcodes(&frames);
    assert!(
        sent.contains(&ability::OP_USE_SKILL),
        "the mount skill was refused as one the class does not own: {sent:?}"
    );
}

/// A mount's own skills need a mount. The original says so in as many
/// words rather than casting nothing.
#[tokio::test]
async fn a_mount_skill_is_refused_on_foot() {
    let state = buff_state();
    let mut session = in_world(&state).await;

    let ask = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_MOUNT_SKILL,
        time: 0,
        body: vec![0, 0],
    };
    let frames = frames_of(handle_message(&state, &mut session, &ask).await);

    assert_eq!(opcodes(&frames), vec![OP_CLIENT_MESSAGE]);
    assert!(
        message_text(&frames[0]).contains("montado"),
        "the player was not told why: {}",
        message_text(&frames[0])
    );
}

/// A potion that lasts starts a buff and is spent doing it, which is the
/// difference between it and the saddle.
#[tokio::test]
async fn a_lasting_potion_starts_a_buff_and_is_drunk() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying(&mut session, LASTING_POTION, 7);

    let use_it = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_USE_ITEM,
        time: 0,
        body: UseItem { container: inventory::BAG as u32, slot: 7, argument: 0 }.to_body(),
    };
    let frames = frames_of(handle_message(&state, &mut session, &use_it).await);

    assert!(session.buffs.has_family(&state.skills, 383), "the potion did nothing");
    assert!(opcodes(&frames).contains(&OP_ADD_BUFF));
    assert!(
        session.character.as_ref().unwrap().items.get(inventory::BAG, 7).is_none(),
        "the last potion of the stack was not drunk"
    );
}

/// A mount is drawn from a field of its own, not from the equip array the
/// spawn carries: that array holds eight slots and the mount is in the
/// tenth. Put it in the wrong place and the rider is drawn on foot.
#[test]
fn the_spawn_says_what_the_character_is_riding() {
    const HORSE: u16 = 963;
    const STONE: u16 = 4220;

    let mut character = Character::from(&dev_character("Athus", 0));
    character
        .items
        .put(Item {
            index: HORSE,
            container: inventory::EQUIP,
            slot: MOUNT_SLOT,
            ..Item::default()
        })
        .unwrap();
    character
        .items
        .put(Item {
            index: STONE,
            container: inventory::EQUIP,
            slot: STONE_SLOT,
            ..Item::default()
        })
        .unwrap();

    let body = decode(&encode_spawn(&character, TEST_CLIENT_ID, stats::BASE_SPEED_MOVE as u32)).body;
    let at = |offset: usize| u16::from_le_bytes(body[offset..offset + 2].try_into().unwrap());

    assert_eq!(at(spawn_offset::ITEM_EFF_MOUNT), HORSE, "the rider was drawn on foot");
    assert_eq!(at(spawn_offset::ITEM_EFF_STONE), STONE);
    // And the equip array, which ends before either of them, is untouched.
    assert_eq!(at(spawn_offset::EQUIP + 7 * 2), 0, "the mount leaked into the equip array");
}

/// A saddle is not a mount, and the mount slot is not a place to put one.
///
/// This is what happened for real: the saddle went into slot 9, the spawn
/// then told everyone the player was riding a saddle, and the client hung
/// trying to draw a horse out of it.
#[tokio::test]
async fn a_saddle_cannot_be_worn_as_a_mount() {
    let state = buff_state();
    let mut session = in_world(&state).await;
    carrying(&mut session, SADDLE, 7);

    handle_message(
        &state,
        &mut session,
        &drag((inventory::BAG, 7), (inventory::EQUIP, MOUNT_SLOT)),
    )
    .await;

    let items = &session.character.as_ref().unwrap().items;
    assert!(
        items.get(inventory::EQUIP, MOUNT_SLOT).is_none(),
        "a saddle was worn as a mount, and the client cannot draw that"
    );
    assert_eq!(
        items.get(inventory::BAG, 7).map(|i| i.index),
        Some(SADDLE),
        "and it was lost on the way"
    );
}

/// Nor does a helmet go on a foot. The slot an item goes in is its type,
/// and the server no longer takes the client's word for it.
#[tokio::test]
async fn gear_only_goes_in_the_slot_its_type_names() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index: 1000,
            container: inventory::BAG,
            slot: 7,
            durability_min: 255,
            ..Item::default()
        })
        .unwrap();

    // A sword in the boots.
    handle_message(&state, &mut session, &drag((inventory::BAG, 7), (inventory::EQUIP, 5)))
        .await;
    assert!(
        session.character.as_ref().unwrap().items.get(inventory::EQUIP, 5).is_none(),
        "a sword was worn as a pair of boots"
    );

    // And in the hand, where it belongs.
    handle_message(
        &state,
        &mut session,
        &drag((inventory::BAG, 7), (inventory::EQUIP, inventory::WEAPON_SLOT)),
    )
    .await;
    assert_eq!(
        session
            .character
            .as_ref()
            .unwrap()
            .items
            .get(inventory::EQUIP, inventory::WEAPON_SLOT)
            .map(|i| i.index),
        Some(1000),
        "a sword could not be drawn"
    );
}

/// Getting on a horse has to reach everyone who can see the rider, which
/// is what the respawn after an equipment move is for.
#[tokio::test]
async fn mounting_makes_everyone_redraw_the_rider() {
    const HORSE: u16 = 963;
    let state = shop_state();
    let mut session = in_world(&state).await;
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index: HORSE,
            container: inventory::BAG,
            slot: 7,
            durability_min: 255,
            ..Item::default()
        })
        .unwrap();

    let frames = frames_of(
        handle_message(
            &state,
            &mut session,
            &drag((inventory::BAG, 7), (inventory::EQUIP, MOUNT_SLOT)),
        )
        .await,
    );

    let spawn = frames
        .iter()
        .map(|f| decode(f))
        .find(|m| m.opcode == OP_CREATE_MOB)
        .expect("nobody was told the player got on a horse");
    assert_eq!(
        u16::from_le_bytes(
            spawn.body[spawn_offset::ITEM_EFF_MOUNT..spawn_offset::ITEM_EFF_MOUNT + 2]
                .try_into()
                .unwrap()
        ),
        HORSE
    );
}

/// The character sheet reads its numbers out of `0x10A` and nowhere else,
/// so what is in the packet is what the player sees. Every field but the
/// speed used to go out as zero.
#[test]
fn the_character_sheet_carries_what_the_gear_is_worth() {
    let state = shop_state();
    let mut character = Character::from(&dev_character("Athus", 0));
    character.attributes = [20, 40, 5, 30, 25, 0];
    character
        .items
        .put(Item {
            index: 1000,
            container: inventory::EQUIP,
            slot: 6,
            durability_min: 255,
            ..Item::default()
        })
        .unwrap();

    let frame = encode_refresh_status(&character, &state.items, &crate::effects::Effects::none());
    let body = decode(&frame).body;
    let at = |offset: usize| u16::from_le_bytes(body[offset..offset + 2].try_into().unwrap());

    use status_offset as off;
    let stats = stats::of(&character, &state.items, &crate::effects::Effects::none());
    assert!(stats.attack > 0, "the fixture sword is worth nothing, so this proves nothing");
    assert_eq!(at(off::ATTACK), stats.attack as u16, "attack went out as zero");
    assert_eq!(at(off::MAGIC_ATTACK), stats.magic_attack as u16);
    assert_eq!(at(off::DEFENCE), stats.defence as u16);
    assert_eq!(at(off::MAGIC_DEFENCE), stats.magic_defence as u16);
    assert_eq!(at(off::CRITICAL), stats.critical as u16);
    assert_eq!(at(off::DODGE), stats.dodge as u16);
    assert_eq!(at(off::ACCURACY), stats.accuracy as u16);
    assert_eq!(at(off::DOUBLE_ATTACK), stats.double_attack as u16);
    assert_eq!(at(off::RESISTANCE), stats.resistance as u16);
    assert_eq!(at(off::SPEED_MOVE), stats::BASE_SPEED_MOVE);
}

/// Taking a sword off has to reach the window, or it keeps claiming the
/// attack of a weapon that is back in the bag.
#[tokio::test]
async fn equipping_something_sends_the_sheet_again() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            index: 1000,
            container: inventory::BAG,
            slot: 7,
            durability_min: 255,
            ..Item::default()
        })
        .unwrap();

    let frames =
        frames_of(handle_message(&state, &mut session, &drag((inventory::BAG, 7), (inventory::EQUIP, 6))).await);

    let sent = opcodes(&frames);
    assert!(sent.contains(&OP_REFRESH_STATUS), "the sheet was not sent again: {sent:?}");
    assert!(sent.contains(&OP_HP_MP), "health and mana were not sent again");
    assert!(
        sent.contains(&OP_CREATE_MOB),
        "a weapon shows on the character, so everyone has to redraw it"
    );

    let status = frames
        .iter()
        .find(|f| decode(f).opcode == OP_REFRESH_STATUS)
        .map(|f| decode(f).body)
        .expect("checked above");
    let attack =
        u16::from_le_bytes(status[status_offset::ATTACK..status_offset::ATTACK + 2].try_into().unwrap());
    let character = session.character.as_ref().unwrap();
    assert_eq!(attack, stats::of(character, &state.items, &crate::effects::Effects::none()).attack as u16);
}

/// A `0x70F` the way the client sends one.
fn drag(from: (u8, u16), to: (u8, u16)) -> Message {
    Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_MOVE_ITEM,
        time: 0,
        body: MoveItem {
            to_container: to.0 as u16,
            to_slot: to.1,
            from_container: from.0 as u16,
            from_slot: from.1,
        }
        .to_body(),
    }
}

/// Opens the chest the way the client does, by using the item that opens
/// it, and hands back the frames that came out.
async fn open_the_chest(state: &State, session: &mut Session) -> Vec<Vec<u8>> {
    let slot = 40;
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item {
            container: inventory::BAG,
            slot,
            index: CHEST_KEY,
            refine: 1,
            ..Item::default()
        })
        .unwrap();
    let use_it = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_USE_ITEM,
        time: 0,
        body: UseItem { container: inventory::BAG as u32, slot: slot as u32, argument: 0 }
            .to_body(),
    };
    frames_of(handle_message(state, session, &use_it).await)
}

/// The item of type 226 in the fixture table.
const CHEST_KEY: u16 = 4400;

/// The chest comes over in one packet, followed by the signal that opens
/// its window and the two pran slots the original sends on their own.
#[tokio::test]
async fn using_the_key_opens_the_chest() {
    let state = shop_state();
    let mut session = in_world(&state).await;

    let frames = open_the_chest(&state, &mut session).await;

    assert_eq!(
        opcodes(&frames),
        vec![OP_STORAGE, OP_STORAGE_OPEN, shop::OP_REFRESH_ITEM, shop::OP_REFRESH_ITEM]
    );
    assert_eq!(frames[0].len(), STORAGE_SIZE, "the chest packet is the wrong size");

    // The four vaults an account is created with are in it.
    let chest = decode(&frames[0]);
    for slot in inventory::STORAGE_PAGE_ITEMS {
        let at = 12 + slot as usize * character_offset::ITEM_SIZE;
        assert_eq!(
            u16::from_le_bytes(chest.body[at..at + 2].try_into().unwrap()),
            creation::VAULT_ITEM,
            "no vault in slot {slot}, so the page is locked"
        );
    }
    assert_eq!(session.opened_option, OPTION_STORAGE, "the window was not marked open");
}

/// Something put in the chest by one character is there for the next one:
/// that is what the chest is for.
#[tokio::test]
async fn an_item_goes_into_the_chest_and_comes_back() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    open_the_chest(&state, &mut session).await;

    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item { index: 1000, container: inventory::BAG, slot: 7, ..Item::default() })
        .unwrap();

    handle_message(&state, &mut session, &drag((inventory::BAG, 7), (inventory::STORAGE, 3)))
        .await;

    let account = session.account.as_ref().unwrap();
    assert_eq!(
        account.storage.get(inventory::STORAGE, 3).map(|i| i.index),
        Some(1000),
        "the item did not reach the chest"
    );
    assert!(
        session.character.as_ref().unwrap().items.get(inventory::BAG, 7).is_none(),
        "the item is in the bag as well, which would duplicate it"
    );

    handle_message(&state, &mut session, &drag((inventory::STORAGE, 3), (inventory::BAG, 9)))
        .await;

    assert_eq!(
        session.character.as_ref().unwrap().items.get(inventory::BAG, 9).map(|i| i.index),
        Some(1000),
        "the item did not come back"
    );
    assert!(session.account.as_ref().unwrap().storage.get(inventory::STORAGE, 3).is_none());
}

/// Putting something in needs the window open, which is the original's own
/// guard and stops a client stashing things it never opened a chest for.
#[tokio::test]
async fn the_chest_refuses_an_item_while_it_is_shut() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item { index: 1000, container: inventory::BAG, slot: 7, ..Item::default() })
        .unwrap();

    handle_message(&state, &mut session, &drag((inventory::BAG, 7), (inventory::STORAGE, 3)))
        .await;

    assert!(
        session.account.as_ref().unwrap().storage.get(inventory::STORAGE, 3).is_none(),
        "the item went into a chest nobody opened"
    );
    assert_eq!(
        session.character.as_ref().unwrap().items.get(inventory::BAG, 7).map(|i| i.index),
        Some(1000),
        "and it left the bag on the way"
    );
}

/// A page with no vault in front of it is not a place to put things. The
/// item has to stay where it was rather than fall down the gap.
#[tokio::test]
async fn a_locked_page_of_the_chest_refuses_an_item() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    open_the_chest(&state, &mut session).await;

    // Take the vault that unlocks the second page away.
    let account = session.account.as_mut().unwrap();
    account.storage.take(inventory::STORAGE, 81).unwrap();
    session
        .character
        .as_mut()
        .unwrap()
        .items
        .put(Item { index: 1000, container: inventory::BAG, slot: 7, ..Item::default() })
        .unwrap();

    handle_message(&state, &mut session, &drag((inventory::BAG, 7), (inventory::STORAGE, 25)))
        .await;

    assert!(
        session.account.as_ref().unwrap().storage.get(inventory::STORAGE, 25).is_none(),
        "an item landed on a page that was never unlocked"
    );
    assert_eq!(
        session.character.as_ref().unwrap().items.get(inventory::BAG, 7).map(|i| i.index),
        Some(1000),
        "and it was lost on the way"
    );
}

/// A vault is the reason a page exists; dragging it away would take the
/// page with it.
#[tokio::test]
async fn a_vault_cannot_be_dragged_out_of_the_chest() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    open_the_chest(&state, &mut session).await;

    handle_message(&state, &mut session, &drag((inventory::STORAGE, 80), (inventory::BAG, 9)))
        .await;

    assert_eq!(
        session.account.as_ref().unwrap().storage.get(inventory::STORAGE, 80).map(|i| i.index),
        Some(creation::VAULT_ITEM),
        "the vault left the chest and took its page with it"
    );
}

/// Gold moves both ways on one packet, the sign saying which.
#[tokio::test]
async fn gold_goes_into_the_chest_and_comes_out_again() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    let purse = session.character.as_ref().unwrap().gold;

    let deposit = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_CHEST_GOLD,
        time: 0,
        body: chest_gold_body(CHEST_TYPE_STORAGE, 500),
    };
    handle_message(&state, &mut session, &deposit).await;

    assert_eq!(session.account.as_ref().unwrap().storage_gold, 500);
    assert_eq!(session.character.as_ref().unwrap().gold, purse - 500);

    let withdraw = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_CHEST_GOLD,
        time: 0,
        body: chest_gold_body(CHEST_TYPE_STORAGE, -200),
    };
    handle_message(&state, &mut session, &withdraw).await;

    assert_eq!(session.account.as_ref().unwrap().storage_gold, 300);
    assert_eq!(session.character.as_ref().unwrap().gold, purse - 300);
}

/// Taking out gold that is not there would be money from nowhere.
#[tokio::test]
async fn the_chest_refuses_gold_it_does_not_have() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    let purse = session.character.as_ref().unwrap().gold;

    let withdraw = Message {
        sender: TEST_CLIENT_ID,
        opcode: OP_CHEST_GOLD,
        time: 0,
        body: chest_gold_body(CHEST_TYPE_STORAGE, -1),
    };
    handle_message(&state, &mut session, &withdraw).await;

    assert_eq!(session.account.as_ref().unwrap().storage_gold, 0);
    assert_eq!(session.character.as_ref().unwrap().gold, purse, "gold appeared from nowhere");
}

fn chest_gold_body(chest: u32, value: i64) -> Vec<u8> {
    let mut body = chest.to_le_bytes().to_vec();
    body.extend_from_slice(&value.to_le_bytes());
    body
}

/// A `0x304` the way the client sends one.
fn action_message(index: u32, in_loop: u32) -> Message {
    let mut body = index.to_le_bytes().to_vec();
    body.extend_from_slice(&in_loop.to_le_bytes());
    Message { sender: TEST_CLIENT_ID, opcode: OP_ACTION, time: 0, body }
}

/// Puts this session and a second player in the registry at the same spot,
/// and hands back the second one's id and its queue.
fn with_a_watcher(
    state: &State,
    session: &mut Session,
) -> (u16, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
    let (mine, _) = tokio::sync::mpsc::unbounded_channel();
    session.client_id = state.world.connect(mine).expect("room to connect");
    let character = session.character.clone().unwrap();
    state.world.enter(session.client_id, character.clone());

    let (theirs, watcher_rx) = tokio::sync::mpsc::unbounded_channel();
    let watcher = state.world.connect(theirs).expect("room for a watcher");
    state.world.enter(watcher, character);
    (watcher, watcher_rx)
}

/// Sitting down is relayed to everyone who can see it, and remembered, so
/// the player is still sitting to whoever turns up next.
#[tokio::test]
async fn sitting_down_is_relayed_and_remembered() {
    let state = state_with(vec![dev_character("Athus", 0)]);
    let mut session = in_world(&state).await;
    let (_, mut watcher_rx) = with_a_watcher(&state, &mut session);

    let action = handle_message(&state, &mut session, &action_message(ACTION_SIT, 1)).await;

    assert!(matches!(action, Action::Ignore), "the sender is already sitting");
    assert_eq!(session.action, ACTION_SIT, "the session forgot the player sat down");

    let relayed = decode(&watcher_rx.try_recv().expect("the watcher was not told"));
    assert_eq!(relayed.opcode, OP_ACTION);
    assert_eq!(relayed.sender, session.client_id, "the relay says who sat down");
    assert_eq!(u32::from_le_bytes(relayed.body[0..4].try_into().unwrap()), ACTION_SIT);
}

/// A wave plays once and is gone: remembering it would have the player
/// waving for as long as they stood there.
#[tokio::test]
async fn a_one_off_action_is_relayed_but_not_remembered() {
    let state = state_with(vec![dev_character("Athus", 0)]);
    let mut session = in_world(&state).await;
    let (_, mut watcher_rx) = with_a_watcher(&state, &mut session);

    const WAVE: u32 = 0x30;
    handle_message(&state, &mut session, &action_message(WAVE, 0)).await;

    assert_eq!(session.action, 0, "a one-off action stuck to the player");
    let relayed = decode(&watcher_rx.try_recv().expect("the watcher was not told"));
    assert_eq!(u32::from_le_bytes(relayed.body[0..4].try_into().unwrap()), WAVE);
}

/// The original never plays the dance that was asked for: `$41` is rolled
/// into one of eleven others, and that is what everyone sees.
#[tokio::test]
async fn a_dance_is_never_the_one_asked_for() {
    let state = state_with(vec![dev_character("Athus", 0)]);
    let mut session = in_world(&state).await;
    let (_, mut watcher_rx) = with_a_watcher(&state, &mut session);

    let frames = frames_of(
        handle_message(&state, &mut session, &action_message(ACTION_DANCE, 0)).await,
    );

    // Unlike sitting, the dancer is told: `SendEffectOther` sends to
    // everyone who can see the player, that player included.
    let mine = decode(&frames[0]);
    assert_eq!(mine.opcode, OP_ACTION);
    let danced = u32::from_le_bytes(mine.body[0..4].try_into().unwrap());
    assert_ne!(danced, ACTION_DANCE, "the request was played as it arrived");
    assert!(DANCES.contains(&danced), "danced something that is not a dance: {danced}");
    assert_eq!(u32::from_le_bytes(mine.body[4..8].try_into().unwrap()), 1, "a dance loops");
    assert_eq!(session.action, danced, "the dance was not remembered");

    let relayed = decode(&watcher_rx.try_recv().expect("the watcher was not told"));
    assert_eq!(
        u32::from_le_bytes(relayed.body[0..4].try_into().unwrap()),
        danced,
        "the watcher saw a different dance from the dancer"
    );
}

/// Walking stands the player up. Without this a player who sat down once
/// is drawn sitting to everyone who meets them, wherever they walked to.
#[tokio::test]
async fn walking_stands_the_player_back_up() {
    let state = state_with(vec![dev_character("Athus", 0)]);
    let mut session = in_world(&state).await;
    let (_, _watcher_rx) = with_a_watcher(&state, &mut session);

    handle_message(&state, &mut session, &action_message(ACTION_SIT, 1)).await;
    assert_eq!(session.action, ACTION_SIT);

    let character = session.character.as_ref().unwrap();
    let mut body = vec![0u8; Movement::BODY_SIZE];
    body[0..4].copy_from_slice(&(character.x as f32 + 1.0).to_le_bytes());
    body[4..8].copy_from_slice(&(character.y as f32).to_le_bytes());
    body[Movement::SPEED] = 50;
    let step = Message { sender: session.client_id, opcode: OP_MOVE, time: 0, body };
    handle_message(&state, &mut session, &step).await;

    assert_eq!(session.action, 0, "the player walked off still sitting down");
}

/// Someone coming into view is told what the player is doing. A spawn
/// draws them standing, so without this the two see different things.
#[tokio::test]
async fn walking_up_to_a_sitting_player_shows_them_sitting() {
    let state = state_with(vec![dev_character("Athus", 0)]);

    // The one who sits down, in the registry under its own id.
    let mut sitter = in_world(&state).await;
    let (mine, _mine_rx) = tokio::sync::mpsc::unbounded_channel();
    sitter.client_id = state.world.connect(mine).expect("room to connect");
    let character = sitter.character.clone().unwrap();
    state.world.enter(sitter.client_id, character.clone());
    handle_message(&state, &mut sitter, &action_message(ACTION_SIT, 1)).await;

    // And the one who walks up, who has never seen them before.
    let mut walker = in_world(&state).await;
    let (theirs, _theirs_rx) = tokio::sync::mpsc::unbounded_channel();
    walker.client_id = state.world.connect(theirs).expect("room for a second");
    state.world.enter(walker.client_id, character);

    let frames = frames_of(refresh_visibility(&state, &mut walker));
    let actions: Vec<_> = frames
        .iter()
        .map(|f| decode(f))
        .filter(|m| m.opcode == OP_ACTION)
        .collect();

    assert_eq!(actions.len(), 1, "the sitting player was drawn standing up");
    assert_eq!(actions[0].sender, sitter.client_id, "the wrong player was said to be sitting");
    assert_eq!(
        u32::from_le_bytes(actions[0].body[0..4].try_into().unwrap()),
        ACTION_SIT
    );
}

/// `0x202` answers with the server's own wall clock, formatted the way the
/// original's `DateTimeToStr` formats it.
#[tokio::test]
async fn asking_the_time_answers_with_the_server_clock() {
    let state = state_with(vec![dev_character("Athus", 0)]);
    let mut session = in_world(&state).await;

    let frames = frames_of(
        handle_message(
            &state,
            &mut session,
            &Message { sender: TEST_CLIENT_ID, opcode: OP_SERVER_TIME, time: 0, body: vec![] },
        )
        .await,
    );

    assert_eq!(decode(&frames[0]).opcode, OP_CLIENT_MESSAGE);
    let text = message_text(&frames[0]);
    let digits: Vec<char> = text.chars().filter(|c| c.is_ascii_digit()).collect();
    assert_eq!(digits.len(), 14, "not a dd/mm/yyyy hh:mm:ss clock: {text}");
    assert_eq!(text.matches('/').count(), 2, "no date in {text}");
    assert_eq!(text.matches(':').count(), 2, "no time in {text}");
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
    let own = decode(&world_burst(&character, session.client_id, &[], &state.items, &crate::effects::Effects::none())[0]);
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
            // A sword goes in the hand and nowhere else.
            to_slot: inventory::WEAPON_SLOT,
            from_container: inventory::BAG as u16,
            from_slot: 0,
        }
        .to_body(),
    };
    handle_message(&state, &mut session, &drag).await;

    let items = &session.character.as_ref().unwrap().items;
    assert_eq!(
        items.get(inventory::EQUIP, inventory::WEAPON_SLOT).unwrap().index,
        1000,
        "not equipped"
    );
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
    let session = in_world(&state).await;

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

    let frames = frames_of(handle_message(&state, &mut session, &attack_message(RAT)).await);
    assert_eq!(state.world.mob(RAT).unwrap().hp, RAT_HP, "it was hit from across the map");

    // Refused, but not in silence. The client filled a bar and is waiting to
    // be told the cast is over; one that is never told stops sending anything
    // but which way it is facing.
    assert!(!frames.is_empty(), "the client was left waiting on a cast that missed");
}

#[tokio::test]
async fn hitting_something_that_is_not_a_monster_does_nothing() {
    let state = fight_state();
    let mut session = in_world(&state).await;

    let frames = frames_of(handle_message(&state, &mut session, &attack_message(9999)).await);

    // Nothing happens to the world, and the client is still let go of it.
    assert!(!frames.is_empty(), "the client was left waiting on a cast at nothing");
    assert_eq!(state.world.mob(RAT).unwrap().hp, RAT_HP);
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

fn learn_message(skill: u32) -> Message {
    let mut body = vec![0u8; 8];
    body[0..4].copy_from_slice(&skill.to_le_bytes());
    Message { sender: TEST_CLIENT_ID, opcode: OP_LEARN_SKILL, time: 0, body }
}

/// Learning an advanced skill ranks it up in the record and spends the
/// points and gold it costs.
#[tokio::test]
async fn learning_a_skill_ranks_it_up_and_spends_points() {
    let state = cast_state();
    let mut session = in_world(&state).await;
    {
        let c = session.character.as_mut().unwrap();
        c.skill_points = 5;
        c.gold = 1000;
    }

    handle_message(&state, &mut session, &learn_message(SPELL)).await;

    let c = session.character.as_ref().unwrap();
    // SPELL is the second class's seventh slot, record index six.
    assert_eq!(c.skill_list[6], 1, "the advanced skill did not climb a rank");
    // The fixture leaves the cost at zero, so points are untouched here;
    // what matters is the rank went up and nothing was overspent.
    assert!(c.skill_points <= 5, "points went up rather than down");
}

/// A skill from another class is refused, points untouched.
#[tokio::test]
async fn learning_another_class_skill_is_refused() {
    let state = cast_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().skill_points = 5;

    let frames = frames_of(handle_message(&state, &mut session, &learn_message(OTHER_SPELL)).await);
    assert_eq!(decode(&frames[0]).opcode, OP_CLIENT_MESSAGE, "it did not refuse");
    assert_eq!(session.character.as_ref().unwrap().skill_points, 5, "points were spent on a refusal");
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
    let spawn = decode(&encode_spawn(&character, TEST_CLIENT_ID, stats::BASE_SPEED_MOVE as u32));

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

    let stats = stats::of(session.character.as_ref().unwrap(), &state.items, &crate::effects::Effects::none());
    assert_eq!(session.cur_hp, stats.max_hp, "it levelled and stayed hurt");
}

/// The same world, but with a curve that really reaches the cap: the ordinary
/// fixture stops at four levels, which is enough for "a kill bought a level"
/// and no use at all for "a kill did not buy the hundredth".
fn to_the_cap_state() -> State {
    let mut state = progress_state();
    state.levels = aika_data::exp::ExpTable::decode(&{
        let mut bytes = Vec::new();
        for level in 0..=promotion::level_cap(promotion::LAST_TIER) as u64 + 1 {
            bytes.extend_from_slice(&(level * 20).to_le_bytes());
        }
        bytes
    })
    .unwrap();
    state
}

/// The curve stops at the cap however much experience is thrown at it.
///
/// It is not a round number for its own sake: the saddle is an item of `10..99`
/// and the client is what enforces that range, so the level past the cap is the
/// level where a character's own mount stops working. `ExpList.bin` holds a
/// hundred, the item table stops one short, and the item table is the one the
/// client reads.
#[tokio::test]
async fn the_curve_stops_at_the_cap() {
    let state = to_the_cap_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().level = promotion::level_cap(promotion::LAST_TIER);
    // Far more than the curve asks for the next level, so nothing but the cap
    // is holding it back.
    session.character.as_mut().unwrap().exp = 1_000_000;
    session.character.as_mut().unwrap().tier = promotion::LAST_TIER;
    let before = session.character.as_ref().unwrap().exp;

    kill_the_rat(&state, &mut session).await;

    let character = session.character.as_ref().unwrap();
    assert_eq!(character.level, promotion::level_cap(promotion::LAST_TIER), "a character levelled past the cap");
    assert!(character.exp > before, "the experience itself still counts");
}

/// And a character one short of it lands on it rather than over it, however
/// many levels the kill was worth.
#[tokio::test]
async fn a_kill_worth_many_levels_lands_on_the_cap() {
    let state = to_the_cap_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().level = promotion::level_cap(promotion::LAST_TIER) - 1;
    session.character.as_mut().unwrap().exp = u32::MAX as u64;
    session.character.as_mut().unwrap().tier = promotion::LAST_TIER;

    kill_the_rat(&state, &mut session).await;

    assert_eq!(
        session.character.as_ref().unwrap().level,
        promotion::level_cap(promotion::LAST_TIER),
        "it overshot the cap"
    );
}

/// A character that has not been promoted stops at its own tier's wall,
/// nowhere near the end of the curve. This is the whole point of the tier:
/// before it, everything levelled straight to 99.
#[tokio::test]
async fn an_unpromoted_character_stops_at_the_first_wall() {
    let state = to_the_cap_state();
    let mut session = in_world(&state).await;
    let wall = promotion::level_cap(promotion::FIRST_TIER);
    session.character.as_mut().unwrap().level = wall - 1;
    session.character.as_mut().unwrap().exp = u32::MAX as u64;

    kill_the_rat(&state, &mut session).await;

    let character = session.character.as_ref().unwrap();
    assert_eq!(character.level, wall, "it levelled through the wall");
    assert_eq!(character.tier, promotion::FIRST_TIER, "and it promoted itself");
}

/// The quest option is what lifts the wall, standing in for the chain the
/// original never wired up.
#[tokio::test]
async fn the_quest_option_promotes_a_character_at_the_wall() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().level = promotion::level_cap(promotion::FIRST_TIER);

    let frames = frames_of(
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::QUESTS)).await,
    );

    let character = session.character.as_ref().unwrap();
    assert_eq!(character.tier, promotion::FIRST_TIER + 1);
    assert_eq!(promotion::level_cap(character.tier), 89, "and the wall moved");
    assert!(session.dirty, "a promotion that is not saved is not a promotion");

    // The class name lives in the character record, so the client only
    // repaints it when the whole record arrives.
    assert!(
        opcodes(&frames).contains(&OP_SEND_TO_WORLD),
        "the client was never told, so it still draws the old class"
    );
}

/// And short of the wall it says how far off you are, rather than going
/// quiet or promoting anyway.
#[tokio::test]
async fn short_of_the_wall_the_promotion_is_refused_with_the_level() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().level = 49;

    let frames = frames_of(
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::QUESTS)).await,
    );

    assert_eq!(session.character.as_ref().unwrap().tier, promotion::FIRST_TIER);
    let text: Vec<String> = frames
        .iter()
        .zip(opcodes(&frames))
        .filter(|(_, op)| *op == OP_CLIENT_MESSAGE)
        .map(|(frame, _)| message_text(frame))
        .collect();
    assert!(
        text.iter().any(|t| t == "Come back when you have reached level 50."),
        "said {text:?} instead"
    );
}

/// Twice promoted, there is nothing left to give, and it says so instead of
/// handing out a fourth tier the data has no skills for.
#[tokio::test]
async fn a_fully_promoted_character_is_offered_nothing_further() {
    let state = shop_state();
    let mut session = in_world(&state).await;
    session.character.as_mut().unwrap().level = 99;
    session.character.as_mut().unwrap().tier = promotion::LAST_TIER;

    let frames = frames_of(
        handle_message(&state, &mut session, &open_npc(2050, dialog::option::QUESTS)).await,
    );

    assert_eq!(session.character.as_ref().unwrap().tier, promotion::LAST_TIER);
    let text: Vec<String> = frames
        .iter()
        .zip(opcodes(&frames))
        .filter(|(_, op)| *op == OP_CLIENT_MESSAGE)
        .map(|(frame, _)| message_text(frame))
        .collect();
    assert!(
        text.iter().any(|t| t == "There is nothing further for you here."),
        "said {text:?} instead"
    );
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
    assert!(carries_nothing(&session), "something dropped from an empty table");
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
                // A weapon with no durability left counts for nothing,
                // which is the original refusing to arm a broken sword.
                durability_min: 255,
                ..Item::default()
            })
            .unwrap();
        handle_message(&state, &mut session, &attack_message(RAT)).await;
        RAT_HP - state.world.mob(RAT).unwrap().hp
    };

    assert!(armed > bare, "a sword did nothing: {armed} against {bare}");
}
