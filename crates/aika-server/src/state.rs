//! State shared by the token, login and game services.

use crate::config::Config;
use crate::store::AccountStore;
use crate::world::World;
use std::time::{Duration, Instant};

pub struct State {
    pub cfg: Config,
    pub store: AccountStore,
    /// Who is online and where, shared by every game connection.
    pub world: World,
    started: Instant,
}

impl State {
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let store = AccountStore::from_dev_accounts(
            &cfg.accounts,
            cfg.login.max_attempts,
            Duration::from_secs(cfg.login.block_minutes * 60),
        )?;
        Ok(Self { cfg, store, world: World::new(), started: Instant::now() })
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
