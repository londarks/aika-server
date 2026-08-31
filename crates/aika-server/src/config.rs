//! Server configuration.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub login: LoginConfig,
    #[serde(default)]
    pub game: GameConfig,
    #[serde(default)]
    pub patch: PatchConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    /// Channels reported by `/servers/servXX.asp`, in order.
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
    /// Development accounts loaded into memory at startup.
    #[serde(default)]
    pub accounts: Vec<DevAccount>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    /// The client builds its URLs from the base configured in `Setting.txt`.
    pub bind: SocketAddr,
    /// Pads the status response with `-1` up to this many channels.
    #[serde(default)]
    pub pad_status_to: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginConfig {
    pub bind: SocketAddr,
    /// How long a token stays valid between the HTTP call and the TCP login.
    /// The original Delphi server uses 300s (`SecondsBetween(...) < 300`).
    #[serde(default = "default_token_ttl")]
    pub token_ttl_secs: u64,
    /// Failed attempts per username before the IP gets blocked.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// How long an IP stays blocked, in minutes.
    #[serde(default = "default_block_minutes")]
    pub block_minutes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameConfig {
    /// One address per channel. In the original server every channel is its
    /// own socket on the **same port 8822**, told apart by IP
    /// (`ServerSocket.pas:287` hardcodes `htons(8822)`), and the client picks
    /// which IP to reach from the channel the player clicked.
    pub binds: Vec<SocketAddr>,
    /// Client version the server demands. The original refuses anything else
    /// (`Version=124` in `AikaServer.ini`).
    #[serde(default = "default_client_version")]
    pub client_version: u16,
    /// Directory of `.npc` files to place in the world. Empty means no
    /// townspeople, which is a playable but very lonely city.
    #[serde(default)]
    pub npc_dir: String,
    /// The server's `ItemList.bin`, which prices everything. Empty means the
    /// shops open and refuse every purchase.
    #[serde(default)]
    pub item_list: String,
    /// Directory holding `AllMobsInfo.csv` and `MonsterListCSV.csv`. Empty
    /// means a world with nothing in it to fight.
    #[serde(default)]
    pub mob_dir: String,
    /// The server's `SkillData.bin`. Empty means nobody can cast anything.
    #[serde(default)]
    pub skill_data: String,
}

/// What the launcher receives when it checks the client version.
///
/// It downloads this, writes it to `update.dat` in the client folder and
/// compares it with what it already had; on a match the START button unlocks.
/// The format is the original file's: `[AIKA] ` CRLF `<version> <patch file>`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchConfig {
    #[serde(default = "default_patch_version")]
    pub version: u32,
    #[serde(default = "default_patch_file")]
    pub file: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEntry {
    pub name: String,
    /// Population reported to the client. `-1` marks the channel offline.
    #[serde(default = "default_online")]
    pub online: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevAccount {
    pub username: String,
    /// Plaintext password; the MD5 hash is derived at startup.
    pub password: Option<String>,
    /// Alternative to the field above, in `password_hash` column format.
    pub password_hash: Option<String>,
    #[serde(default)]
    pub nation: u8,
    #[serde(default)]
    pub account_status: u8,
    #[serde(default)]
    pub ban_days: u32,
    /// The account's characters. The client shows three slots.
    #[serde(default)]
    pub characters: Vec<DevCharacter>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevCharacter {
    pub name: String,
    /// Slot on the selection screen, 0 to 2.
    #[serde(default)]
    pub slot: usize,
    #[serde(default = "default_level")]
    pub level: u16,
    /// Class index in the form the client sends on creation: 10-19 warrior,
    /// 20-29 templar, 30-39 ranger, 40-49 dual, 50-59 mage, 60-69 cleric.
    #[serde(default = "default_class_index")]
    pub class_index: u16,
    /// The client only accepts 7700..7731.
    #[serde(default = "default_hair")]
    pub hair: u16,
    #[serde(default)]
    pub nation: u16,
    #[serde(default)]
    pub gold: u64,
    #[serde(default)]
    pub exp: u64,
    /// Position on the map. Without it, spawns in the starting city.
    #[serde(default)]
    pub x: Option<u32>,
    #[serde(default)]
    pub y: Option<u32>,
    /// Movement speed; falls back to the default.
    #[serde(default)]
    pub speed_move: Option<u8>,
}

/// Where the world is stored.
///
/// SQLite while developing, so a checkout runs with no service to install.
/// The queries are written for both, so production only changes this path
/// into a MySQL URL.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// A file path, or `:memory:` for a database that dies with the process.
    #[serde(default = "default_database_path")]
    pub path: String,
    /// How long a change may sit in memory before it is written, in seconds.
    ///
    /// Saving only when a player disconnects loses the whole session to a
    /// crash or a kill, which to the player is indistinguishable from a
    /// database that does not work. Zero writes on every change.
    #[serde(default = "default_autosave_secs")]
    pub autosave_secs: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self { path: default_database_path(), autosave_secs: default_autosave_secs() }
    }
}

fn default_database_path() -> String {
    "aika.db".to_string()
}

fn default_autosave_secs() -> u64 {
    5
}

fn default_token_ttl() -> u64 {
    300
}
fn default_max_attempts() -> u32 {
    10
}
fn default_block_minutes() -> u64 {
    10
}
fn default_online() -> i32 {
    1
}
fn default_level() -> u16 {
    1
}
fn default_class_index() -> u16 {
    10
}
fn default_hair() -> u16 {
    7700
}
fn default_client_version() -> u16 {
    124
}
fn default_patch_version() -> u32 {
    301
}
fn default_patch_file() -> String {
    "valhalla301.zip".to_string()
}

impl Default for PatchConfig {
    fn default() -> Self {
        Self { version: default_patch_version(), file: default_patch_file() }
    }
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            binds: vec!["127.0.0.1:8822".parse().unwrap()],
            client_version: default_client_version(),
            npc_dir: String::new(),
            item_list: String::new(),
            mob_dir: String::new(),
            skill_data: String::new(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self { bind: "0.0.0.0:8090".parse().unwrap(), pad_status_to: 0 }
    }
}

impl Default for LoginConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8831".parse().unwrap(),
            token_ttl_secs: default_token_ttl(),
            max_attempts: default_max_attempts(),
            block_minutes: default_block_minutes(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading configuration at {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }
}
