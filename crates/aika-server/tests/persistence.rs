//! Logging out somewhere and logging back in there.
//!
//! The other integration test drives the protocol with everything in memory.
//! This one puts a real database file underneath, walks a character across
//! the map, drops the connection, then starts a *second* server on the same
//! file and reads the position out of the spawn packet the client receives.
//! Nothing is shared between the two servers except the file, so a position
//! that survives has genuinely been through the database.

use aika_data::itemlist::ItemList;
use aika_data::npc::Npc;
use aika_net::frame::{self, FrameReader, Message};
use aika_server::config::{Config, DatabaseConfig, DevAccount, DevCharacter};
use aika_server::db::Database;
use aika_server::game::{
    self, Movement, RequestLogin, OP_CHAR_LIST, OP_CLIENT_READY, OP_CREATE_MOB, OP_ENTER_WORLD,
    OP_MOVE, OP_REMOVE_MOB, OP_REQUEST_LOGIN,
};
use aika_server::inventory;
use aika_server::shop;
use aika_server::store::CITY_SPAWN;
use aika_server::world::World;
use aika_server::{login, web, State};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration, Instant};

const CLIENT_VERSION: u16 = 124;
const MERCHANT_ID: u16 = 2050;
const SWORD: u16 = 1000;
const SWORD_PRICE: u32 = 500;
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

/// A merchant standing on the spawn point, selling one sword.
///
/// Built rather than read: the `.npc` files and the 14 MB item table belong to
/// the original pack and are not in this repository, but the shop code has to
/// be exercised against a real socket all the same.
fn merchant() -> Npc {
    let mut shop = [0u16; aika_data::npc::SHOP_SLOTS];
    shop[0] = SWORD;
    Npc {
        id: MERCHANT_ID,
        title: "Merchant".into(),
        label: "Thomas Henrikson".into(),
        name_index: Some(43),
        options: vec![1, 5, 8],
        equip: [234, 234, 0, 0, 0, 0, 0, 0],
        sizes: [7, 119, 119, 3],
        shop,
        max_hp: 20000,
        cur_hp: 20000,
        max_mp: 20000,
        cur_mp: 0,
        x: CITY_SPAWN.0 as f32,
        y: CITY_SPAWN.1 as f32,
        rotation: 0,
        speed_move: 0,
        stale_id: None,
    }
}

fn item_table() -> ItemList {
    use aika_data::itemlist::{field, RECORD_SIZE};
    let mut raw = vec![0u8; (SWORD as usize + 1) * RECORD_SIZE];
    let r = &mut raw[SWORD as usize * RECORD_SIZE..];
    r[field::NAME.start] = b'x';
    // One field is both the asking price and the base for the resale.
    r[field::SELL_PRICE..field::SELL_PRICE + 4].copy_from_slice(&SWORD_PRICE.to_le_bytes());
    r[field::DURABILITY] = 60;
    ItemList::decode(&raw).expect("the fixture table is malformed")
}

/// Starts a server on the given database. Calling it twice on the same path
/// is what a restart looks like from the outside.
async fn spawn_servers(database_path: &str) -> Servers {
    let mut state = State::open(config(database_path)).await.unwrap();
    state.world = World::with_npcs(vec![merchant()]);
    state.items = item_table();
    let state = Arc::new(state);

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
    /// The selection screen the server sent on connecting. Kept because
    /// asking for it a second time would wait forever: the server sends one
    /// when it has something to say, not when it is asked.
    char_list: Option<Message>,
}

impl GameClient {
    async fn join(addr: SocketAddr, token: &str) -> Self {
        let mut client = Self {
            stream: TcpStream::connect(addr).await.unwrap(),
            reader: FrameReader::new(),
            char_list: None,
        };
        let body = RequestLogin {
            account_id: 1,
            username: "admin".into(),
            version: CLIENT_VERSION,
            token: token.into(),
        }
        .to_body();
        client.send(OP_REQUEST_LOGIN, body).await;
        client.char_list = Some(client.expect(OP_CHAR_LIST).await);
        client
    }

    /// The selection screen from connecting.
    fn char_list(&self) -> &Message {
        self.char_list.as_ref().expect("no character list was received")
    }

    /// Picks slot 0 and reports the scene as loaded, which is what makes the
    /// server send the spawn. Returns where the spawn puts the character.
    ///
    /// Townspeople arrive on the same opcode, so the one addressed to a
    /// player id is the one to read. Without that, a merchant standing on the
    /// spawn point answers for the character.
    async fn enter_world(&mut self) -> (f32, f32) {
        self.send(OP_ENTER_WORLD, 0u32.to_le_bytes().to_vec()).await;
        self.send(OP_CLIENT_READY, Vec::new()).await;

        let spawn = self.expect_player_spawn().await;
        (
            f32::from_le_bytes(spawn.body[SPAWN_X..SPAWN_X + 4].try_into().unwrap()),
            f32::from_le_bytes(spawn.body[SPAWN_Y..SPAWN_Y + 4].try_into().unwrap()),
        )
    }

    /// The next `0x349` that is about a player rather than a townsperson.
    async fn expect_player_spawn(&mut self) -> Message {
        loop {
            let message = self.expect(OP_CREATE_MOB).await;
            if message.sender < aika_server::world::FIRST_NPC_ID {
                return message;
            }
        }
    }

    /// Opens the shop and buys slot zero.
    async fn buy_from_shop(&mut self) {
        let open = aika_server::dialog::OpenNpc {
            npc: MERCHANT_ID as u32,
            option: aika_server::dialog::option::SHOP,
            extra: 0,
        };
        self.send(aika_server::dialog::OP_OPEN_NPC, open.to_body()).await;
        self.expect(shop::OP_SHOW_SHOP).await;

        let buy = shop::Buy { npc: MERCHANT_ID as u32, slot: 0, amount: 1 };
        self.send(shop::OP_BUY, buy.to_body()).await;
        self.expect(shop::OP_REFRESH_MONEY).await;
    }

    /// Reads the character record out of the world packet the server sends on
    /// entering, which is where the client learns what it is carrying.
    async fn world_record(&mut self) -> Vec<u8> {
        let packet = self.expect(0x925).await;
        packet.body[4..].to_vec()
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

    // Walking away puts the merchant off the screen, and waiting for that is
    // how the test knows the server has actually applied the movement.
    // Closing the socket while frames are still unread makes the client send
    // a reset, and a reset throws away whatever the server had not read yet -
    // including the movement.
    client.walk_to(WALKED_TO.0, WALKED_TO.1).await;
    client.expect(OP_REMOVE_MOB).await;
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

/// Buying is only real if the sword is still there after a restart. This is
/// the whole chain: a shop packet over a socket, a purchase, a disconnect, a
/// second server on the same file, and the item read back out of the record
/// the client receives.
#[tokio::test]
async fn a_bought_item_and_the_gold_it_cost_survive_a_restart() {
    let path = fresh_database_path("bought-item");

    let first = spawn_servers(&path).await;
    let token = token_for(first.web).await;
    let mut client = GameClient::join(first.game, &token).await;
    client.enter_world().await;

    let before = gold_in_database(&path).await;
    client.buy_from_shop().await;
    drop(client);

    await_saved_gold(&path, before - SWORD_PRICE as u64).await;

    // A different server, sharing nothing but the file.
    let second = spawn_servers(&path).await;
    let token = token_for(second.web).await;
    let mut client = GameClient::join(second.game, &token).await;

    client.send(OP_ENTER_WORLD, 0u32.to_le_bytes().to_vec()).await;
    let record = client.world_record().await;

    // The bag is at 664 inside the record, twenty bytes to an item.
    let at = 664;
    assert_eq!(
        u16::from_le_bytes(record[at..at + 2].try_into().unwrap()),
        SWORD,
        "the sword did not come back"
    );
    assert_eq!(
        u64::from_le_bytes(record[3184..3192].try_into().unwrap()),
        before - SWORD_PRICE as u64,
        "the gold came back wrong"
    );
}

async fn gold_in_database(path: &str) -> u64 {
    let db = Database::open(path).await.unwrap();
    db.load_accounts().await.unwrap()[0].characters[0].gold
}

async fn await_saved_gold(path: &str, expected: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = gold_in_database(path).await;
        if found == expected {
            return;
        }
        if Instant::now() >= deadline {
            panic!("the purchase never reached the database: {found} gold");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The bag has to survive too, not only the gold.
#[tokio::test]
async fn an_item_thrown_away_stays_thrown_away() {
    let path = fresh_database_path("thrown-away");

    let first = spawn_servers(&path).await;
    let token = token_for(first.web).await;
    let mut client = GameClient::join(first.game, &token).await;
    client.enter_world().await;
    client.buy_from_shop().await;

    let throw = aika_server::game::MoveItem {
        from_container: inventory::BAG,
        from_slot: 0,
        to_container: inventory::BAG,
        to_slot: 0,
    };
    client.send(aika_server::game::OP_DELETE_ITEM, throw.to_body()).await;
    client.expect(shop::OP_REFRESH_ITEM).await;
    drop(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let db = Database::open(&path).await.unwrap();
        let id = db.load_accounts().await.unwrap()[0].characters[0].id;
        if db.load_items(id).await.unwrap().is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("the thrown away item is still in the database");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A character made on the selection screen has to exist after a restart, or
/// the player creates it again every session.
#[tokio::test]
async fn a_created_character_survives_a_restart() {
    let path = fresh_database_path("created-character");

    let first = spawn_servers(&path).await;
    let token = token_for(first.web).await;
    let mut client = GameClient::join(first.game, &token).await;

    let request = aika_server::creation::CreateCharacter {
        slot: 1,
        name: "Segundo".into(),
        class_index: 40,
        hair: 7710,
        town: 1,
    };
    client.send(aika_server::creation::OP_CREATE_CHARACTER, request.to_body()).await;
    let list = client.expect(OP_CHAR_LIST).await;

    // slot 1 of the list the client is sent back
    let name = name_in_slot(&list.body, 1);
    assert_eq!(name, "Segundo", "the new character is not on the selection screen");
    drop(client);

    // A different server, sharing nothing but the file.
    let second = spawn_servers(&path).await;
    let token = token_for(second.web).await;
    let client = GameClient::join(second.game, &token).await;

    assert_eq!(
        name_in_slot(&client.char_list().body, 1),
        "Segundo",
        "the character did not come back"
    );

    // and it starts where the creation screen said, not at the default town
    let db = Database::open(&path).await.unwrap();
    let created = db.load_accounts().await.unwrap()[0]
        .characters
        .iter()
        .find(|c| c.name == "Segundo")
        .expect("not in the database")
        .clone();

    assert_eq!((created.x, created.y), aika_server::creation::TOWN_SECOND);
    assert_eq!(created.class_index, 40);
    assert!(created.id > 0, "the character has no database id");
    assert!(!created.items.is_empty(), "it was created with nothing to carry");
}

/// A name already in use is refused, and the refusal must not leave a half
/// made character behind.
#[tokio::test]
async fn a_duplicate_name_is_refused_and_leaves_nothing_behind() {
    let path = fresh_database_path("duplicate-name");

    let servers = spawn_servers(&path).await;
    let token = token_for(servers.web).await;
    let mut client = GameClient::join(servers.game, &token).await;

    let request = aika_server::creation::CreateCharacter {
        slot: 1,
        name: "Athus".into(),
        class_index: 20,
        hair: 7702,
        town: 0,
    };
    client.send(aika_server::creation::OP_CREATE_CHARACTER, request.to_body()).await;
    let list = client.expect(OP_CHAR_LIST).await;

    assert_eq!(name_in_slot(&list.body, 1), "", "the slot was filled with a duplicate");

    let db = Database::open(&path).await.unwrap();
    assert_eq!(
        db.load_accounts().await.unwrap()[0].characters.len(),
        1,
        "a second character reached the database"
    );
}

/// The name in one slot of a character list packet.
fn name_in_slot(body: &[u8], slot: usize) -> String {
    const ENTRY: usize = 104;
    let at = 12 + slot * ENTRY;
    body[at..at + 16].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()
}
