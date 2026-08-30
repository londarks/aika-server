//! The full flow the way the client does it: fetch a token over HTTP, ask
//! for the character count, present the token again on the login socket and
//! enter the game server, which answers with the character selection screen.

use aika_server::config::{Config, DevAccount, DevCharacter, ServerEntry};
use aika_server::game::{self, RequestLogin, OP_CHAR_LIST, OP_REQUEST_LOGIN};
use aika_server::login::{self, CheckToken, OP_CHECK_TOKEN, OP_LOGIN_RESULT};
use aika_server::{web, State};
use aika_net::frame::{self, FrameReader, Message};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};

const CLIENT_VERSION: u16 = 124;

struct Servers {
    web: SocketAddr,
    login: SocketAddr,
    game: SocketAddr,
}

async fn spawn_servers() -> Servers {
    let cfg = Config {
        servers: vec![
            ServerEntry { name: "Teste1-PvP".into(), online: 1 },
            ServerEntry { name: "Teste2-PvP".into(), online: -1 },
        ],
        accounts: vec![DevAccount {
            username: "admin".into(),
            password: Some("admin".into()),
            password_hash: None,
            nation: 2,
            account_status: 0,
            ban_days: 0,
            characters: vec![DevCharacter {
                name: "Athus".into(),
                slot: 0,
                level: 42,
                class_index: 20,
                hair: 7702,
                nation: 2,
                gold: 999,
                exp: 12345,
                x: None,
                y: None,
                speed_move: None,
            }],
        }],
        ..Default::default()
    };

    let state = Arc::new(State::new(cfg).unwrap());

    let web_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let login_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let game_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addrs = Servers {
        web: web_listener.local_addr().unwrap(),
        login: login_listener.local_addr().unwrap(),
        game: game_listener.local_addr().unwrap(),
    };

    tokio::spawn(web::serve(Arc::clone(&state), web_listener));
    tokio::spawn(login::serve(Arc::clone(&state), login_listener));
    tokio::spawn(game::serve(state, game_listener));

    addrs
}

async fn post(addr: SocketAddr, path: &str, body: &str) -> String {
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    send_http(addr, request.as_bytes()).await
}

async fn send_http(addr: SocketAddr, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    text.split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("HTTP reply without a body: {text:?}"))
        .1
        .to_string()
}

/// Sends bytes on a protocol socket and returns the deciphered reply.
/// Reads until a frame closes instead of waiting for the connection to end:
/// the login server hangs up after replying, but the game server keeps the
/// socket open, since that same connection carries the game. `None` means
/// the server refused (closed silently) or did not answer in time.

async fn exchange(addr: SocketAddr, wire: &[u8]) -> Option<Message> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(wire).await.unwrap();

    let mut reader = FrameReader::new();
    let mut buf = [0u8; 4096];

    loop {
        let read = timeout(Duration::from_secs(5), stream.read(&mut buf)).await;
        let n = match read {
            Ok(Ok(0)) | Err(_) => return None,
            Ok(Ok(n)) => n,
            Ok(Err(e)) => panic!("error reading from the server: {e}"),
        };
        reader.push(&buf[..n]);
        if let Some(message) = reader.next_message() {
            return Some(message.expect("unreadable reply"));
        }
    }
}

fn check_token_packet(username: &str, token: &str) -> Vec<u8> {
    let body = CheckToken { username: username.into(), token: token.into() }.to_body();
    frame::encode(&Message { sender: 0, opcode: OP_CHECK_TOKEN, time: 0, body }, rand::random())
}

fn game_login_packet(account_id: u32, username: &str, token: &str, version: u16) -> Vec<u8> {
    let body = RequestLogin {
        account_id,
        username: username.into(),
        version,
        token: token.into(),
    }
    .to_body();
    frame::encode(
        &Message { sender: 0, opcode: OP_REQUEST_LOGIN, time: 0, body },
        rand::random(),
    )
}

#[tokio::test]
async fn full_login_flow_reaches_character_select() {
    let servers = spawn_servers().await;

    // 1. login screen: username and password become a token
    let token = post(servers.web, "/member/aika_get_token.asp", "id=admin&pw=admin").await;
    assert_eq!(token.len(), 32, "expected a 32 hex token, got {token:?}");

    // 2. server selection screen: characters and nation
    let chr =
        post(servers.web, "/servers/aika_get_chrcnt.asp", &format!("id=admin&pw={token}")).await;
    assert_eq!(chr, "CNT 1 0 0 0<br>2 0 0 0");

    // 3. channel population
    let status = post(servers.web, "/servers/serv00.asp", "").await;
    assert!(status.starts_with("1 -1"), "unexpected status: {status:?}");

    // 4. login server: the token comes back and the account is cleared
    let result =
        exchange(servers.login, &check_token_packet("admin", &token)).await.expect("no 0x82");
    assert_eq!(result.opcode, OP_LOGIN_RESULT);
    let account_id = u32::from_le_bytes(result.body[0..4].try_into().unwrap());
    assert_eq!(account_id, 1);

    // 5. game server: enter and receive the character selection screen
    let wire = game_login_packet(account_id, "admin", &token, CLIENT_VERSION);
    let char_list = exchange(servers.game, &wire).await.expect("no 0x901");

    assert_eq!(char_list.opcode, OP_CHAR_LIST);
    assert_eq!(u32::from_le_bytes(char_list.body[0..4].try_into().unwrap()), account_id);

    // the seeded character shows up in slot 0
    let slot0 = &char_list.body[12..12 + 104];
    let name: String = slot0[..16]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    assert_eq!(name, "Athus");
    assert_eq!(u16::from_le_bytes(slot0[64..66].try_into().unwrap()), 41, "level 42 travels as 41");
}

/// The client sends 4 bytes in front of the first packet on every socket;
/// the server has to swallow that without losing its place.
#[tokio::test]
async fn game_server_accepts_leading_prefix() {
    let servers = spawn_servers().await;

    let mut wire = vec![0x11, 0xF3, 0x11, 0x1F];
    wire.extend(game_login_packet(1, "admin", &"t".repeat(32), CLIENT_VERSION));

    let response = exchange(servers.game, &wire).await.expect("no 0x901");
    assert_eq!(response.opcode, OP_CHAR_LIST);
}

#[tokio::test]
async fn game_server_refuses_wrong_client_version() {
    let servers = spawn_servers().await;
    let wire = game_login_packet(1, "admin", &"t".repeat(32), 999);
    assert!(exchange(servers.game, &wire).await.is_none(), "a wrong version must not pass");
}

#[tokio::test]
async fn login_rejects_bad_token() {
    let servers = spawn_servers().await;
    let _ = post(servers.web, "/member/aika_get_token.asp", "id=admin&pw=admin").await;

    let response = exchange(servers.login, &check_token_packet("admin", &"0".repeat(32))).await;
    assert!(response.is_none(), "an invalid token must not receive 0x82");
}

#[tokio::test]
async fn login_accepts_client_hello_prefix() {
    let servers = spawn_servers().await;
    let token = post(servers.web, "/member/aika_get_token.asp", "id=admin&pw=admin").await;

    let mut wire = vec![0x11, 0xF3, 0x11, 0x1F];
    wire.extend(check_token_packet("admin", &token));

    let response = exchange(servers.login, &wire).await.expect("no 0x82");
    assert_eq!(response.opcode, OP_LOGIN_RESULT);
}

/// A request in the style of a 2008 client: HTTP/1.0, no `Host`, no
/// `Content-Type`, and lines terminated with a bare newline.
#[tokio::test]
async fn web_accepts_legacy_http_request() {
    let servers = spawn_servers().await;

    let body = "id=admin&pw=admin";
    let request = format!(
        "POST /member/aika_get_token.asp HTTP/1.0\nContent-Length: {}\n\n{body}",
        body.len()
    );
    let token = send_http(servers.web, request.as_bytes()).await;

    assert_eq!(token.len(), 32, "legacy client refused: {token:?}");
}

#[tokio::test]
async fn web_reports_wrong_password() {
    let servers = spawn_servers().await;
    let response = post(servers.web, "/member/aika_get_token.asp", "id=admin&pw=errada").await;
    assert_eq!(response, "-1");
}
