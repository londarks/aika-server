//! What players say and do at each other.
//!
//! Chat and the actions -- sitting, waving, dancing -- are one subject: both
//! are a packet the client sends about itself that the server echoes to
//! everyone who can see them, and neither changes anything but what is drawn.
//! They came out of `game.rs` when it reached eight thousand lines.

use super::*;
/// Offsets inside the `0xF86` body, once the 12-byte header is gone
/// (`TChatPacket`, `Data/Packets.pas:188`).
pub(crate) mod chat_offset {
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
pub(crate) const CHAT_NORMAL: u16 = 0;
pub(crate) const CHAT_WHISPER: u16 = 1;

/// `0xF86`: a player speaks (`TPacketHandlers.SendClientSay`).
///
/// Only ordinary say and whisper are here. Say is the one that makes the world
/// feel inhabited: the original stamps the speaker's name into the packet and
/// echoes it, unchanged, to everyone who can see them — themselves included,
/// which is why you see your own bubble (`SendToVisible` defaults
/// `sendToSelf` to true). Party, guild and nation chat wait on those systems
/// and are logged, not dropped silently, so a tester can see the client tried.
pub(crate) fn handle_chat(state: &State, session: &mut Session, message: &Message) -> Action {
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

/// The two actions the original remembers past the packet that started them
/// (`UpdateAction`): sitting down, and asking to dance. Everything else — a
/// wave, a bow — plays once and is forgotten, so nobody arriving later is told
/// about it.
pub(crate) const ACTION_SIT: u32 = 40;
pub(crate) const ACTION_DANCE: u32 = 0x41;

/// What a request to dance actually becomes. The original never plays `$41`:
/// it rolls one of these eleven and plays that instead, so the same key gives
/// a different dance each time.
pub(crate) const DANCES: [u32; 11] =
    [0x43, 0x44, 0x45, 0x46, 0x4A, 0x4B, 0x47, 0x48, 0x49, 0x4C, 0x4D];

/// `0x304`: the player sat down, waved or danced (`UpdateAction`).
///
/// Mostly a relay — the client that sent it is already playing the animation,
/// and everyone who can see the player needs to play the same. Two of them
/// stick, though: sitting and dancing outlast the packet, so they are kept on
/// the presence and sent to anyone who walks up afterwards. Walking or casting
/// clears it, which is the original setting `CurrentAction := 0` at the end of
/// `MovementCommand` and `UseSkill`.
pub(crate) fn handle_action(state: &State, session: &mut Session, message: &Message) -> Action {
    if message.body.len() < ACTION_SIZE - MIN_FRAME {
        warn!(size = message.body.len(), "0x304 packet too short");
        return Action::Ignore;
    }
    if session.character.is_none() {
        return Action::Ignore;
    }
    let client_id = session.client_id;
    let mut index = u32::from_le_bytes(message.body[0..4].try_into().unwrap());
    let in_loop = u32::from_le_bytes(message.body[4..8].try_into().unwrap());

    let mut frames = Vec::new();
    if index == ACTION_DANCE {
        index = DANCES[rand::random::<usize>() % DANCES.len()];
        session.action = index;
        state.world.act(client_id, index);
        // `SendEffectOther`: to everyone who can see the player, this time
        // including the player, and always looping.
        let effect = encode_action(client_id, index, 1);
        state.world.send_to_visible(client_id, effect.clone());
        frames.push(effect);
        debug!(dance = index, "danced");
    } else if index == ACTION_SIT {
        session.action = index;
        state.world.act(client_id, index);
    }

    // And the packet itself, to everyone but the sender, carrying whichever
    // index we ended up with. The original relays this even after the dance
    // above has gone out, so a dance is sent twice; copying that is cheaper
    // than guessing which of the two the client actually draws from.
    state.world.send_to_visible(client_id, encode_action(client_id, index, in_loop));

    if frames.is_empty() {
        Action::Ignore
    } else {
        Action::Reply(frames)
    }
}

/// `TSendActionPacket`: which animation, and whether it repeats.
pub(crate) fn encode_action(client_id: u16, index: u32, in_loop: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(ACTION_SIZE - MIN_FRAME);
    body.extend_from_slice(&index.to_le_bytes());
    body.extend_from_slice(&in_loop.to_le_bytes());

    debug_assert_eq!(body.len() + MIN_FRAME, ACTION_SIZE);
    frame::encode(
        &Message { sender: client_id, opcode: OP_ACTION, time: 0, body },
        rand::random(),
    )
}
