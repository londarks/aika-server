//! The full flow the way the client does it: fetch a token over HTTP, ask
//! for the character count, present the token again on the login socket and
//! enter the game server, which answers with the character selection screen.

use aika_server::config::{Config, DevAccount, DevCharacter, ServerEntry};
use aika_server::game::{
    self, Movement, RequestLogin, OP_CHAR_LIST, OP_CLIENT_READY, OP_CREATE_MOB,
    OP_ENTER_WORLD, OP_MOVE, OP_REMOVE_MOB, OP_REQUEST_LOGIN,
};
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


/// A connection to the game server that stays open, the way a real client's
/// does, so packets pushed by other players can be observed arriving.
struct GameClient {
    stream: TcpStream,
    reader: FrameReader,
}

impl GameClient {
    async fn join(addr: SocketAddr, username: &str, token: &str) -> Self {
        let mut client = Self {
            stream: TcpStream::connect(addr).await.unwrap(),
            reader: FrameReader::new(),
        };
        client
            .send(OP_REQUEST_LOGIN, RequestLogin {
                account_id: 1,
                username: username.into(),
                version: CLIENT_VERSION,
                token: token.into(),
            }
            .to_body())
            .await;
        client.expect(OP_CHAR_LIST).await;
        client
    }

    /// Picks the character and reports the scene as loaded, which is what
    /// makes the player visible to everyone else.
    async fn enter_world(&mut self) {
        self.send(OP_ENTER_WORLD, 0u32.to_le_bytes().to_vec()).await;
        self.send(OP_CLIENT_READY, Vec::new()).await;
    }

    async fn walk_to(&mut self, x: f32, y: f32) {
        let mut body = vec![0u8; Movement::BODY_SIZE];
        body[0..4].copy_from_slice(&x.to_le_bytes());
        body[4..8].copy_from_slice(&y.to_le_bytes());
        self.send(OP_MOVE, body).await;
    }

    async fn send(&mut self, opcode: u16, body: Vec<u8>) {
        let wire = frame::encode(&Message { sender: 0, opcode, time: 0, body }, rand::random());
        self.stream.write_all(&wire).await.unwrap();
    }

    /// Next packet, waiting for more bytes only when a frame is incomplete.
    async fn next(&mut self) -> Option<Message> {
        let mut buf = [0u8; 8192];
        loop {
            if let Some(message) = self.reader.next_message() {
                return Some(message.expect("unreadable frame"));
            }
            let read = timeout(Duration::from_secs(2), self.stream.read(&mut buf)).await;
            match read {
                Ok(Ok(0)) | Err(_) => return None,
                Ok(Ok(n)) => self.reader.push(&buf[..n]),
                Ok(Err(e)) => panic!("error reading: {e}"),
            }
        }
    }

    /// Reads until the given opcode shows up, ignoring the rest.
    async fn expect(&mut self, opcode: u16) -> Message {
        while let Some(message) = self.next().await {
            if message.opcode == opcode {
                return message;
            }
        }
        panic!("connection closed while waiting for 0x{opcode:X}");
    }

    /// Whether the opcode arrives before the connection goes quiet.
    async fn sees(&mut self, opcode: u16) -> bool {
        while let Some(message) = self.next().await {
            if message.opcode == opcode {
                return true;
            }
        }
        false
    }
}

async fn token_for(web: SocketAddr) -> String {
    post(web, "/member/aika_get_token.asp", "id=admin&pw=admin").await
}

/// The point of the whole registry: two players in the world at once, each
/// appearing on the other's screen.
#[tokio::test]
async fn two_players_see_each_other() {
    let servers = spawn_servers().await;

    let token = token_for(servers.web).await;
    let mut first = GameClient::join(servers.game, "admin", &token).await;
    first.enter_world().await;
    first.expect(OP_CREATE_MOB).await;

    // the second player arrives next to the first
    let mut second = GameClient::join(servers.game, "admin", &token).await;
    second.enter_world().await;

    // the newcomer is told about who was already there
    let spawns = collect_spawns(&mut second).await;
    assert!(spawns >= 2, "the newcomer sees itself and the player already there, got {spawns}");

    // and the player already there is told about the newcomer
    assert!(first.sees(OP_CREATE_MOB).await, "the first player never saw the second arrive");
}

/// Counts spawn packets until the connection goes quiet.
async fn collect_spawns(client: &mut GameClient) -> usize {
    let mut spawns = 0;
    while let Some(message) = client.next().await {
        if message.opcode == OP_CREATE_MOB {
            spawns += 1;
        }
    }
    spawns
}

/// Walking away from someone takes both players off each other's screen.
#[tokio::test]
async fn walking_out_of_range_removes_the_other_player() {
    let servers = spawn_servers().await;

    let token = token_for(servers.web).await;
    let mut first = GameClient::join(servers.game, "admin", &token).await;
    first.enter_world().await;
    first.expect(OP_CREATE_MOB).await;

    let mut second = GameClient::join(servers.game, "admin", &token).await;
    second.enter_world().await;
    second.expect(OP_CREATE_MOB).await;

    // far beyond the watch radius
    second.walk_to(9000.0, 9000.0).await;
    assert!(
        second.sees(OP_REMOVE_MOB).await,
        "walking away must take the other player off the screen"
    );
}

/// A player leaving is taken off the screens of everyone who could see them.
#[tokio::test]
async fn disconnecting_removes_the_player_from_the_others() {
    let servers = spawn_servers().await;

    let token = token_for(servers.web).await;
    let mut watcher = GameClient::join(servers.game, "admin", &token).await;
    watcher.enter_world().await;
    watcher.expect(OP_CREATE_MOB).await;

    let mut leaver = GameClient::join(servers.game, "admin", &token).await;
    leaver.enter_world().await;
    leaver.expect(OP_CREATE_MOB).await;

    // the watcher has to have seen the arrival before it can see the exit
    assert!(watcher.sees(OP_CREATE_MOB).await, "arrival was not seen");

    drop(leaver);
    assert!(watcher.sees(OP_REMOVE_MOB).await, "the leaver was never removed");
}
