//! State shared by the token, login and game services.

use crate::config::Config;
use crate::store::AccountStore;
use std::time::{Duration, Instant};

pub struct State {
    pub cfg: Config,
    pub store: AccountStore,
    started: Instant,
}

impl State {
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let store = AccountStore::from_dev_accounts(
            &cfg.accounts,
            cfg.login.max_attempts,
            Duration::from_secs(cfg.login.block_minutes * 60),
        )?;
        Ok(Self { cfg, store, started: Instant::now() })
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
