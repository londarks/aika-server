//! Logging out somewhere and logging back in there.
//!
//! The other integration test drives the protocol with everything in memory.
//! This one puts a real database file underneath, walks a character across
//! the map, drops the connection, then starts a *second* server on the same
//! file and reads the position out of the spawn packet the client receives.
//! Nothing is shared between the two servers except the file, so a position
//! that survives has genuinely been through the database.

use aika_net::frame::{self, FrameReader, Message};
use aika_server::config::{Config, DatabaseConfig, DevAccount, DevCharacter};
use aika_server::db::Database;
use aika_server::game::{
    self, Movement, RequestLogin, OP_CHAR_LIST, OP_CLIENT_READY, OP_CREATE_MOB, OP_ENTER_WORLD,
    OP_MOVE, OP_REQUEST_LOGIN,
};
use aika_server::store::CITY_SPAWN;
use aika_server::{login, web, State};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration, Instant};

const CLIENT_VERSION: u16 = 124;
/// Body offsets in `0x349`, the spawn packet.
const SPAWN_X: usize = 44;
const SPAWN_Y: usize = 48;
/// Somewhere the character has never been, so a stale read cannot pass.
const WALKED_TO: (f32, f32) = (4200.0, 815.0);

struct Servers {
    web: SocketAddr,
    game: SocketAddr,
}

/// A database file of its own for each test, removed when the test starts so
/// a previous run cannot leak into this one.
fn fresh_database_path(name: &str) -> String {
    let path = std::env::temp_dir().join(format!("aika-test-{name}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path.display().to_string()
}

fn config(database_path: &str) -> Config {
    Config {
        database: DatabaseConfig { path: database_path.to_string() },
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
    }
}

/// Starts a server on the given database. Calling it twice on the same path
/// is what a restart looks like from the outside.
async fn spawn_servers(database_path: &str) -> Servers {
    let state = Arc::new(State::open(config(database_path)).await.unwrap());

    let web_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let login_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let game_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addrs = Servers {
        web: web_listener.local_addr().unwrap(),
        game: game_listener.local_addr().unwrap(),
    };

    tokio::spawn(web::serve(Arc::clone(&state), web_listener));
    tokio::spawn(login::serve(Arc::clone(&state), login_listener));
    tokio::spawn(game::serve(state, game_listener));

    addrs
}

async fn token_for(web: SocketAddr) -> String {
    let body = "id=admin&pw=admin";
    let request = format!(
        "POST /member/aika_get_token.asp HTTP/1.1\r\nHost: {web}\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(web).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    text.split_once("\r\n\r\n").expect("HTTP reply without a body").1.to_string()
}

struct GameClient {
    stream: TcpStream,
    reader: FrameReader,
}

impl GameClient {
    async fn join(addr: SocketAddr, token: &str) -> Self {
        let mut client =
            Self { stream: TcpStream::connect(addr).await.unwrap(), reader: FrameReader::new() };
        let body = RequestLogin {
            account_id: 1,
            username: "admin".into(),
            version: CLIENT_VERSION,
            token: token.into(),
        }
        .to_body();
        client.send(OP_REQUEST_LOGIN, body).await;
        client.expect(OP_CHAR_LIST).await;
        client
    }

    /// Picks slot 0 and reports the scene as loaded, which is what makes the
    /// server send the spawn. Returns where the spawn puts the character.
    async fn enter_world(&mut self) -> (f32, f32) {
        self.send(OP_ENTER_WORLD, 0u32.to_le_bytes().to_vec()).await;
        self.send(OP_CLIENT_READY, Vec::new()).await;

        let spawn = self.expect(OP_CREATE_MOB).await;
        (
            f32::from_le_bytes(spawn.body[SPAWN_X..SPAWN_X + 4].try_into().unwrap()),
            f32::from_le_bytes(spawn.body[SPAWN_Y..SPAWN_Y + 4].try_into().unwrap()),
        )
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

    async fn expect(&mut self, opcode: u16) -> Message {
        let mut buf = [0u8; 8192];
        loop {
            while let Some(message) = self.reader.next_message() {
                let message = message.expect("unreadable frame");
                if message.opcode == opcode {
                    return message;
                }
            }
            let read = timeout(Duration::from_secs(5), self.stream.read(&mut buf)).await;
            match read {
                Ok(Ok(0)) | Err(_) => panic!("connection closed waiting for 0x{opcode:X}"),
                Ok(Ok(n)) => self.reader.push(&buf[..n]),
                Ok(Err(e)) => panic!("error reading: {e}"),
            }
        }
    }
}

/// Waits for the saved position to reach the file rather than guessing at a
/// sleep: the write happens as the server notices the socket close.
async fn await_saved_position(path: &str, expected: (u32, u32)) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let db = Database::open(path).await.unwrap();
        let accounts = db.load_accounts().await.unwrap();
        let found = {
            let character = &accounts[0].characters[0];
            (character.x, character.y)
        };
        if found == expected {
            return;
        }
        if Instant::now() >= deadline {
            panic!("the position never reached the database: {found:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_character_comes_back_where_it_logged_out() {
    let path = fresh_database_path("logout-position");

    // First run: the character starts in the city, walks somewhere else, and
    // the connection drops.
    let first = spawn_servers(&path).await;
    let token = token_for(first.web).await;
    let mut client = GameClient::join(first.game, &token).await;

    let spawned_at = client.enter_world().await;
    assert_eq!(
        (spawned_at.0 as u32, spawned_at.1 as u32),
        CITY_SPAWN,
        "a fresh character starts in the city"
    );

    client.walk_to(WALKED_TO.0, WALKED_TO.1).await;
    drop(client);

    await_saved_position(&path, (WALKED_TO.0 as u32, WALKED_TO.1 as u32)).await;

    // Second run: a different server, sharing nothing but the file.
    let second = spawn_servers(&path).await;
    let token = token_for(second.web).await;
    let mut client = GameClient::join(second.game, &token).await;

    assert_eq!(
        client.enter_world().await,
        WALKED_TO,
        "logging back in must land where the last session ended"
    );
}

/// The configuration only fills an empty database. Editing it afterwards must
/// not overwrite what people have played.
#[tokio::test]
async fn the_configuration_does_not_overwrite_a_played_character() {
    let path = fresh_database_path("no-reseed");

    let state = State::open(config(&path)).await.unwrap();
    let id = state.store.get("admin").unwrap().characters[0].id;
    drop(state);

    let db = Database::open(&path).await.unwrap();
    db.save_position(id, 4200, 815).await.unwrap();
    drop(db);

    // Opening again would seed a second time if the count were not checked.
    let state = State::open(config(&path)).await.unwrap();
    let account = state.store.get("admin").unwrap();
    assert_eq!(account.characters.len(), 1, "the character was seeded twice");
    assert_eq!((account.characters[0].x, account.characters[0].y), (4200, 815));
}
