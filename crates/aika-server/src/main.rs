use aika_server::{game, login, web, Config, State};
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "aika_server=debug,info".into()),
        )
        .init();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "config.toml".to_string());
    let cfg = Config::load(&config_path)?;

    let web_addr = cfg.web.bind;
    let login_addr = cfg.login.bind;
    let game_addrs = cfg.game.binds.clone();
    let accounts = cfg.accounts.len();
    let channels = cfg.servers.len();

    if game_addrs.is_empty() {
        anyhow::bail!("no address in [game].binds; the client would have nowhere to connect");
    }

    let state = Arc::new(State::open(cfg).await?);

    // Monsters come back on their own, whether or not anybody is connected.
    game::spawn_world_tick(Arc::clone(&state));

    let web_listener = bind(web_addr, "token server").await?;
    let login_listener = bind(login_addr, "login server").await?;

    // One socket per channel, all sharing the same state.
    for addr in game_addrs {
        let listener = bind(addr, "game server").await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = game::serve(state, listener).await {
                error!(%addr, error = %e, "game server stopped");
            }
        });
    }

    info!(
        config = %config_path,
        accounts,
        channels,
        npcs = state.world.npcs().len(),
        monsters = state.world.mob_count(),
        "aika-server started"
    );

    tokio::select! {
        result = web::serve(Arc::clone(&state), web_listener) => result?,
        result = login::serve(Arc::clone(&state), login_listener) => result?,
        _ = tokio::signal::ctrl_c() => info!("shutting down"),
    }

    Ok(())
}

async fn bind(addr: std::net::SocketAddr, what: &str) -> Result<TcpListener> {
    TcpListener::bind(addr).await.with_context(|| format!("binding the {what} on {addr}"))
}
