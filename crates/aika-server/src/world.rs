//! Who is online, where they stand, and how to reach them.
//!
//! Until now every connection only knew about itself, which is enough to put
//! one player in the world and no more. This registry is what lets two players
//! exist at the same time: it hands out the client ids the protocol identifies
//! players by, keeps their positions, and owns the channels other connections
//! push packets into.
//!
//! Visibility is a plain distance check for now. The original keeps a per
//! player list refreshed on movement, with separate thresholds for noticing
//! and forgetting someone; with a handful of players a scan costs nothing and
//! the shape of the answer is the same. A spatial grid belongs here later,
//! behind the same methods.

use crate::store::Character;
use aika_data::npc::Npc;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc;

/// Frames queued for one connection to write.
pub type Outbox = mpsc::UnboundedSender<Vec<u8>>;

/// How close a player must be to be seen, from `DISTANCE_TO_WATCH` in the
/// original server's ini.
pub const DISTANCE_TO_WATCH: f32 = 50.0;
/// How far a player must go to be dropped from view. The gap between the two
/// is deliberate: a single threshold makes someone standing right on the edge
/// flicker in and out.
pub const DISTANCE_TO_FORGET: f32 = 60.0;

/// Client ids are one space shared by players and NPCs, because the protocol
/// carries them in the same header field. The original reserves 1 to 2000 for
/// players and 2048 to 3048 for NPCs (`Connections/ServerSocket.pas:44`), and
/// only ever fills 200 player slots. Handing a player an id in the NPC range
/// would make the client draw them over a townsperson.
pub const MAX_PLAYERS: u16 = 200;
/// The first id an NPC can hold.
pub const FIRST_NPC_ID: u16 = 2048;

/// One connected player.
#[derive(Clone)]
pub struct Presence {
    pub client_id: u16,
    pub account_id: u32,
    /// Set once the player actually enters the world. Until then they are
    /// connected but invisible to everyone.
    pub character: Option<Character>,
    outbox: Outbox,
}

impl Presence {
    pub fn is_in_world(&self) -> bool {
        self.character.is_some()
    }

    pub fn position(&self) -> Option<(f32, f32)> {
        self.character.as_ref().map(|c| (c.x as f32, c.y as f32))
    }

    /// Queues a frame for this player. Fails silently when the connection is
    /// already gone, which is the right behaviour for a broadcast.
    pub fn send(&self, frame: Vec<u8>) {
        let _ = self.outbox.send(frame);
    }
}

#[derive(Default)]
pub struct World {
    players: Mutex<HashMap<u16, Presence>>,
    /// Read from `Data/NPCs` at startup and never touched again: an NPC does
    /// not move, log out or take damage, so it needs no lock.
    npcs: Vec<Npc>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_npcs(npcs: Vec<Npc>) -> Self {
        Self { npcs, ..Self::default() }
    }

    pub fn npcs(&self) -> &[Npc] {
        &self.npcs
    }

    /// The NPCs a player standing at this point should have on screen.
    pub fn npcs_near(&self, at: (f32, f32), radius: f32) -> Vec<&Npc> {
        self.npcs.iter().filter(|npc| within(at, (npc.x, npc.y), radius)).collect()
    }

    /// Registers a new connection and gives it a client id, or `None` when
    /// the server is full.
    ///
    /// Ids start at 1 and the lowest free one is reused, the way the original
    /// picks a free connection slot. The client learns its id from the packets
    /// we send it, so it must be ours to choose: echoing back whatever the
    /// client claimed would give every player the same id. The cap is what
    /// keeps a player id out of the range the NPCs occupy.
    pub fn connect(&self, outbox: Outbox) -> Option<u16> {
        let mut players = self.players.lock().unwrap();
        let client_id = (1..=MAX_PLAYERS).find(|id| !players.contains_key(id))?;
        players.insert(
            client_id,
            Presence { client_id, account_id: 0, character: None, outbox },
        );
        Some(client_id)
    }

    pub fn disconnect(&self, client_id: u16) -> Option<Presence> {
        self.players.lock().unwrap().remove(&client_id)
    }

    pub fn set_account(&self, client_id: u16, account_id: u32) {
        if let Some(presence) = self.players.lock().unwrap().get_mut(&client_id) {
            presence.account_id = account_id;
        }
    }

    /// Marks a player as present in the world, which is what makes them
    /// visible to everyone else.
    pub fn enter(&self, client_id: u16, character: Character) {
        if let Some(presence) = self.players.lock().unwrap().get_mut(&client_id) {
            presence.character = Some(character);
        }
    }

    pub fn move_to(&self, client_id: u16, x: u32, y: u32) {
        if let Some(presence) = self.players.lock().unwrap().get_mut(&client_id) {
            if let Some(character) = presence.character.as_mut() {
                character.x = x;
                character.y = y;
            }
        }
    }

    pub fn online(&self) -> usize {
        self.players.lock().unwrap().values().filter(|p| p.is_in_world()).count()
    }

    /// Everyone in the world other than this player, near enough to be seen.
    pub fn visible_to(&self, client_id: u16) -> Vec<Presence> {
        let players = self.players.lock().unwrap();
        let Some(origin) = players.get(&client_id).and_then(|p| p.position()) else {
            return Vec::new();
        };
        players
            .values()
            .filter(|other| other.client_id != client_id && other.is_in_world())
            .filter(|other| {
                other.position().is_some_and(|p| within(origin, p, DISTANCE_TO_WATCH))
            })
            .cloned()
            .collect()
    }

    /// Sends a frame to everyone who can see this player, never to the player
    /// themselves. That split matters: the client moves itself and would fight
    /// its own echo.
    pub fn send_to_visible(&self, from: u16, frame: Vec<u8>) {
        for presence in self.visible_to(from) {
            presence.send(frame.clone());
        }
    }
}

/// Whether two points are within a radius, compared squared to avoid a square
/// root on every check.
fn within(a: (f32, f32), b: (f32, f32), radius: f32) -> bool {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DevCharacter;

    fn character(name: &str, x: u32, y: u32) -> Character {
        let mut c = Character::from(&DevCharacter {
            name: name.into(),
            slot: 0,
            level: 1,
            class_index: 10,
            hair: 7700,
            nation: 2,
            gold: 0,
            exp: 0,
            x: Some(x),
            y: Some(y),
            speed_move: None,
        });
        c.name = name.to_string();
        c
    }

    fn join(world: &World, name: &str, x: u32, y: u32) -> (u16, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = world.connect(tx).expect("the world is not full");
        world.enter(id, character(name, x, y));
        (id, rx)
    }

    #[test]
    fn hands_out_the_lowest_free_id() {
        let world = World::new();
        let (a, _ra) = join(&world, "a", 0, 0);
        let (b, _rb) = join(&world, "b", 0, 0);
        assert_eq!((a, b), (1, 2));

        world.disconnect(a);
        let (c, _rc) = join(&world, "c", 0, 0);
        assert_eq!(c, 1, "a freed id is reused");
    }

    #[test]
    fn only_players_in_the_world_are_visible() {
        let world = World::new();
        let (seer, _rx) = join(&world, "seer", 100, 100);

        // connected but never entered the world
        let (tx, _lurker_rx) = mpsc::unbounded_channel();
        world.connect(tx).unwrap();

        assert_eq!(world.online(), 1);
        assert!(world.visible_to(seer).is_empty(), "a lurker is not visible");
    }

    #[test]
    fn visibility_follows_distance() {
        let world = World::new();
        let (seer, _rx) = join(&world, "seer", 100, 100);
        let (near, _rn) = join(&world, "near", 120, 100); // 20 away
        let (far, _rf) = join(&world, "far", 400, 100); // 300 away

        let visible: Vec<u16> = world.visible_to(seer).iter().map(|p| p.client_id).collect();
        assert_eq!(visible, vec![near]);
        assert!(!visible.contains(&far));

        // walking closer brings them into view
        world.move_to(far, 130, 100);
        let visible: Vec<u16> = world.visible_to(seer).iter().map(|p| p.client_id).collect();
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn a_broadcast_reaches_the_others_and_never_the_sender() {
        let world = World::new();
        let (mover, mut mover_rx) = join(&world, "mover", 0, 0);
        let (watcher, mut watcher_rx) = join(&world, "watcher", 10, 0);

        world.send_to_visible(mover, vec![1, 2, 3]);

        assert_eq!(watcher_rx.try_recv().ok(), Some(vec![1, 2, 3]));
        assert!(mover_rx.try_recv().is_err(), "the mover must not hear its own echo");
        let _ = watcher;
    }

    #[test]
    fn a_disconnected_player_stops_being_visible() {
        let world = World::new();
        let (seer, _rx) = join(&world, "seer", 0, 0);
        let (other, _ro) = join(&world, "other", 5, 0);
        assert_eq!(world.visible_to(seer).len(), 1);

        let gone = world.disconnect(other).expect("was connected");
        assert_eq!(gone.client_id, other);
        assert!(world.visible_to(seer).is_empty());
        assert_eq!(world.online(), 1);
    }

    /// A player id in the NPC range would draw over a townsperson, so the
    /// server refuses the connection instead.
    #[test]
    fn ids_stop_before_the_range_the_npcs_use() {
        let world = World::new();
        let mut held = Vec::new();
        for _ in 0..MAX_PLAYERS {
            let (tx, rx) = mpsc::unbounded_channel();
            held.push((world.connect(tx).expect("still room"), rx));
        }

        assert_eq!(held.last().unwrap().0, MAX_PLAYERS);
        assert!(MAX_PLAYERS < FIRST_NPC_ID);

        let (tx, _rx) = mpsc::unbounded_channel();
        assert_eq!(world.connect(tx), None, "the server is full and must say so");
    }

    #[test]
    fn distance_is_measured_as_a_circle_not_a_square() {
        // a point diagonally at 40,40 is 56.6 away, outside a radius of 50,
        // even though each axis alone is within it
        assert!(!within((0.0, 0.0), (40.0, 40.0), DISTANCE_TO_WATCH));
        assert!(within((0.0, 0.0), (40.0, 20.0), DISTANCE_TO_WATCH));
        assert!(within((0.0, 0.0), (40.0, 40.0), DISTANCE_TO_FORGET));
    }
}
