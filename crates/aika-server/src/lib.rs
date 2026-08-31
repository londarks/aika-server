//! Aika Online server.
//!
//! Three services running in one process, sharing the same state, the way the
//! original Delphi server does:
//!
//! - [`web`]: the HTTP routes the login screen calls (port 8090), which check
//!   username and password and hand back a short-lived token;
//! - [`login`]: the TCP socket (port 8831) where the client presents that
//!   token again and receives its account id;
//! - [`game`]: the game server (port 8822), which answers with the character
//!   list and from there on runs the world.
//!
//! [`world`] holds the registry of who is online, which is what lets two
//! players see each other.

pub mod config;
pub mod db;
pub mod game;
pub mod http;
pub mod login;
pub mod state;
pub mod store;
pub mod web;
pub mod world;

pub use config::Config;
pub use state::State;
