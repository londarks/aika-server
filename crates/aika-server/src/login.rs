//! Login server (port 8831 by default).
//!
//! Having fetched a token over HTTP, the client opens a socket here and sends
//! a single `0x81` packet with username and token. If the token checks out the
//! server answers `0x82` with the account id and nation, then closes — that is
//! what unlocks the character selection screen.

use crate::state::State;
use crate::store::TokenCheck;
use aika_net::frame::{self, FrameError, FrameReader, Message};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

/// Client to server: check username and token.
pub const OP_CHECK_TOKEN: u16 = 0x81;
/// Server to client: login result.
pub const OP_LOGIN_RESULT: u16 = 0x82;

/// Text field width in the `0x81` packet (`TCheckTokenPacket` in Delphi).
const FIELD_LEN: usize = 32;
/// Full `0x81` body: username, token and 1040 bytes of padding.
const CHECK_TOKEN_BODY: usize = 1104;

pub async fn serve(state: Arc<State>, listener: TcpListener) -> anyhow::Result<()> {
    info!(addr = %listener.local_addr()?, "login server listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            debug!(%peer, "client connected to login");
            if let Err(e) = handle_connection(state, stream).await {
                debug!(%peer, error = %e, "login connection closed");
            }
        });
    }
}

async fn handle_connection(state: Arc<State>, mut stream: TcpStream) -> anyhow::Result<()> {
    let mut reader = FrameReader::new();
    let mut buf = [0u8; 4096];

    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        reader.push(&buf[..n]);

        while let Some(message) = reader.next_message() {
            match message {
                Ok(message) => {
                    if let Some(response) = handle_message(&state, &message) {
                        stream.write_all(&response).await?;
                        stream.flush().await?;
                    }
                    // The original server drops the connection right after
                    // handling `0x81`, successful or not: the client moves on
                    // to the game server.
                    return Ok(());
                }
                Err(FrameError::BadChecksum) => {
                    warn!("packet with invalid checksum");
                    return Ok(());
                }
                Err(FrameError::BadLength(size)) => {
                    warn!(size, "packet with invalid size");
                    return Ok(());
                }
            }
        }
    }
}

/// Handles an already deciphered message. Returns bytes to send, if any.
fn handle_message(state: &State, message: &Message) -> Option<Vec<u8>> {
    if message.opcode != OP_CHECK_TOKEN {
        warn!(
            opcode = format!("0x{:02x}", message.opcode),
            size = message.body.len(),
            "unexpected opcode on login"
        );
        return None;
    }

    let Some(request) = CheckToken::parse(&message.body) else {
        warn!(size = message.body.len(), "0x81 packet too short");
        return None;
    };

    match state.store.check_token(&request.username, &request.token, state.token_ttl()) {
        TokenCheck::Ok(account) => {
            info!(user = %account.username, id = account.id, "login accepted");
            Some(encode_login_result(account.id, state.uptime_ms(), account.nation))
        }
        outcome => {
            info!(user = %request.username, ?outcome, "login refused");
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckToken {
    pub username: String,
    pub token: String,
}

impl CheckToken {
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < FIELD_LEN * 2 {
            return None;
        }
        Some(Self {
            username: read_fixed_str(&body[..FIELD_LEN]),
            token: read_fixed_str(&body[FIELD_LEN..FIELD_LEN * 2]),
        })
    }

    /// Builds the `0x81` body the way the client does — used by tests and by
    /// any tool that needs to talk to the login server.
    pub fn to_body(&self) -> Vec<u8> {
        let mut body = vec![0u8; CHECK_TOKEN_BODY];
        write_fixed_str(&mut body[..FIELD_LEN], &self.username);
        write_fixed_str(&mut body[FIELD_LEN..FIELD_LEN * 2], &self.token);
        body
    }
}

/// `0x82` body: account id, timestamp and nation.
pub fn login_result_body(account_id: u32, time: u32, nation: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(13);
    body.extend_from_slice(&account_id.to_le_bytes());
    body.extend_from_slice(&time.to_le_bytes());
    body.push(nation);
    body.extend_from_slice(&0u32.to_le_bytes());
    body
}

fn encode_login_result(account_id: u32, time: u32, nation: u8) -> Vec<u8> {
    let message = Message {
        sender: 0,
        opcode: OP_LOGIN_RESULT,
        time,
        body: login_result_body(account_id, time, nation),
    };
    frame::encode(&message, rand::random())
}

/// Reads a fixed-width text field. The client pads the remainder with `0x00`
/// or `0xCC`, and speaks latin-1.
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
    use crate::config::{Config, DevAccount};
    use crate::store::AuthOutcome;

    fn state() -> State {
        let cfg = Config {
            accounts: vec![DevAccount {
                username: "admin".into(),
                password: Some("admin".into()),
                password_hash: None,
                nation: 2,
                account_status: 0,
                ban_days: 0,
                characters: Vec::new(),
            }],
            ..Default::default()
        };
        State::new(cfg).unwrap()
    }

    #[test]
    fn check_token_body_roundtrip() {
        let original = CheckToken { username: "admin".into(), token: "a".repeat(32) };
        let body = original.to_body();
        assert_eq!(body.len(), CHECK_TOKEN_BODY);
        assert_eq!(CheckToken::parse(&body).unwrap(), original);
    }

    #[test]
    fn parses_fields_padded_with_cc() {
        let mut body = vec![0xCCu8; CHECK_TOKEN_BODY];
        body[..5].copy_from_slice(b"admin");
        body[32..34].copy_from_slice(b"ab");
        let parsed = CheckToken::parse(&body).unwrap();
        assert_eq!(parsed.username, "admin");
        assert_eq!(parsed.token, "ab");
    }

    #[test]
    fn rejects_short_body() {
        assert!(CheckToken::parse(&[0u8; 10]).is_none());
    }

    #[test]
    fn login_result_matches_delphi_layout() {
        // TResponseLoginPacket: header(12) + Index(4) + Time(4) + Nation(1) + Null(4)
        let body = login_result_body(42, 0x1122_3344, 2);
        assert_eq!(body.len(), 13);
        let wire = encode_login_result(42, 0x1122_3344, 2);
        assert_eq!(wire.len(), 25, "the client expects exactly 25 bytes");
        assert_eq!(u16::from_le_bytes([wire[0], wire[1]]), 25);
    }

    #[test]
    fn valid_token_yields_login_result() {
        let state = state();
        let AuthOutcome::Ok { token } =
            state.store.authenticate("admin", "admin", "127.0.0.1".parse().unwrap()).0
        else {
            panic!("HTTP login failed");
        };

        let request = CheckToken { username: "admin".into(), token };
        let message =
            Message { sender: 0, opcode: OP_CHECK_TOKEN, time: 0, body: request.to_body() };

        let response = handle_message(&state, &message).expect("expected a 0x82 reply");

        // decode the reply the way the client would
        let mut reader = FrameReader::new();
        reader.push(&response);
        let decoded = reader.next_message().unwrap().unwrap();
        assert_eq!(decoded.opcode, OP_LOGIN_RESULT);
        assert_eq!(u32::from_le_bytes(decoded.body[0..4].try_into().unwrap()), 1);
        assert_eq!(decoded.body[8], 2, "nation");
    }

    #[test]
    fn wrong_token_gets_no_response() {
        let state = state();
        let request = CheckToken { username: "admin".into(), token: "0".repeat(32) };
        let message =
            Message { sender: 0, opcode: OP_CHECK_TOKEN, time: 0, body: request.to_body() };
        assert!(handle_message(&state, &message).is_none());
    }

    #[test]
    fn unknown_opcode_gets_no_response() {
        let state = state();
        let message = Message { sender: 0, opcode: 0x99, time: 0, body: vec![0; 64] };
        assert!(handle_message(&state, &message).is_none());
    }
}
