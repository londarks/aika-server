//! State shared by the token, login and game services.

use crate::config::Config;
use crate::db::Database;
use crate::store::{AccountStore, Character};
use crate::world::World;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub struct State {
    pub cfg: Config,
    pub store: AccountStore,
    /// Who is online and where, shared by every game connection.
    pub world: World,
    /// Where the world is kept between runs. Unit tests that only build
    /// packets leave it out; every server that people log into has one.
    db: Option<Database>,
    started: Instant,
}

impl State {
    /// Opens the database and takes the accounts from it. The configuration
    /// seeds the very first run, so a fresh checkout still has somewhere to
    /// log in, and is ignored from then on: the database is the truth.
    pub async fn open(cfg: Config) -> anyhow::Result<Self> {
        let db = Database::open(&cfg.database.path).await?;

        if db.account_count().await? == 0 {
            let seeded = db.seed(&cfg.accounts).await?;
            info!(path = %cfg.database.path, accounts = seeded, "seeded a new database");
        }

        let accounts = db.load_accounts().await?;
        info!(path = %cfg.database.path, accounts = accounts.len(), "database ready");

        let store = AccountStore::from_accounts(
            accounts,
            cfg.login.max_attempts,
            Duration::from_secs(cfg.login.block_minutes * 60),
        )?;
        Ok(Self { cfg, store, world: World::new(), db: Some(db), started: Instant::now() })
    }

    /// State with no database behind it, for tests about packets rather than
    /// persistence. Accounts come straight from the configuration.
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let store = AccountStore::from_dev_accounts(
            &cfg.accounts,
            cfg.login.max_attempts,
            Duration::from_secs(cfg.login.block_minutes * 60),
        )?;
        Ok(Self { cfg, store, world: World::new(), db: None, started: Instant::now() })
    }

    pub fn db(&self) -> Option<&Database> {
        self.db.as_ref()
    }

    /// Remembers where a character stopped, in the database and in the copy
    /// the login screen reads, so the next login starts there either way.
    ///
    /// A failure is logged rather than returned: this runs while a connection
    /// is being torn down, and there is nobody left to tell.
    pub async fn save_position(&self, character: &Character) {
        self.store.update_position(character.id, character.x, character.y);

        let Some(db) = &self.db else { return };
        if let Err(e) = db.save_position(character.id, character.x, character.y).await {
            warn!(character = %character.name, error = %e, "could not save the position");
        }
    }

    /// Equivalent of the `timeGetTime` the Delphi server stamps on packets:
    /// milliseconds since the process started, truncated to 32 bits.
    pub fn uptime_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    pub fn token_ttl(&self) -> Duration {
        Duration::from_secs(self.cfg.login.token_ttl_secs)
    }
}
