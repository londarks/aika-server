//! Aika Online game data file formats.
//!
//! Client and server share several proprietary `.bin` files. Here they are
//! read and written by our own code instead of the original pack's tools.

pub mod itemlist;
pub mod jit;
pub mod mobs;
pub mod npc;
pub mod sl;
pub mod strdef;
