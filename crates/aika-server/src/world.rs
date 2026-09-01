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

use crate::mob::{Attack, Mob, Reaction, Turn};
use crate::store::Character;
use aika_data::npc::Npc;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
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
    /// Which way the player is facing. It lives here rather than on the
    /// character because it is worth nothing after a logout: the client sends
    /// it again as soon as the mouse moves.
    pub rotation: u32,
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
    /// Read from `assets/npcs` at startup and never touched again: an NPC
    /// does not move, log out or take damage, so it needs no lock.
    npcs: Vec<Npc>,
    /// Monsters. Unlike the NPCs these change — they take damage, die and
    /// come back — so they sit behind a lock and the world owns them rather
    /// than any one connection.
    mobs: Mutex<Vec<Mob>>,
    /// Blows monsters have landed that the players have not been told about
    /// yet, by player.
    ///
    /// The world tick runs outside every connection and cannot reach into a
    /// session to take health off it. So it leaves the damage here and the
    /// session picks it up on its next packet, which the client's own
    /// twice-a-second heartbeat guarantees.
    incoming: Mutex<HashMap<u16, Vec<Attack>>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_npcs(npcs: Vec<Npc>) -> Self {
        Self { npcs, ..Self::default() }
    }

    pub fn with_mobs(mut self, mobs: Vec<Mob>) -> Self {
        self.mobs = Mutex::new(mobs);
        self
    }

    pub fn mob_count(&self) -> usize {
        self.mobs.lock().unwrap().len()
    }

    /// The living monsters a player standing at this point should see.
    pub fn mobs_near(&self, at: (f32, f32), radius: f32) -> Vec<Mob> {
        self.mobs
            .lock()
            .unwrap()
            .iter()
            .filter(|mob| mob.is_alive())
            .filter(|mob| within(at, mob.position(), radius))
            .cloned()
            .collect()
    }

    pub fn mob(&self, id: u16) -> Option<Mob> {
        self.mobs.lock().unwrap().iter().find(|m| m.id == id).cloned()
    }

    /// Hurts a monster and says what became of it: `None` when there is no
    /// such monster or it is already down, otherwise the monster as it now
    /// stands and whether this was the killing blow.
    ///
    /// The whole thing happens under one lock, which is what stops two
    /// players being paid for the same corpse.
    pub fn wound_mob(
        &self,
        id: u16,
        damage: u32,
        by: u16,
        now: Instant,
    ) -> Option<(Mob, bool)> {
        let mut mobs = self.mobs.lock().unwrap();
        let mob = mobs.iter_mut().find(|m| m.id == id)?;
        if !mob.is_alive() {
            return None;
        }
        // Hitting a monster is what makes a passive one fight back.
        mob.provoked_by(by);
        let killed = mob.wound(damage, now);
        Some((mob.clone(), killed))
    }

    /// The movement thread: `TMobMovimentThread1`, every three seconds.
    ///
    /// A monster that is fighting drifts towards whoever it is fighting; one
    /// that is not ambles between its two points. Nothing here starts a fight
    /// — that is the other thread's job, which is why a monster walking past
    /// you does not turn round.
    ///
    /// The positions of the players are passed in rather than read under the
    /// same lock: taking both locks in one place is how two of them end up
    /// taken in the other order somewhere else.
    pub fn move_mobs(&self, players: &[(u16, (f32, f32))], now: Instant) -> Vec<(Mob, Turn)> {
        let mut turns = Vec::new();
        let mut mobs = self.mobs.lock().unwrap();

        for mob in mobs.iter_mut().filter(|m| m.is_alive()) {
            let turn = mob.move_turn(players, now);
            if turn != Turn::default() {
                turns.push((mob.clone(), turn));
            }
        }
        turns
    }

    /// The combat thread: `TMobHandlerThread1`, every second.
    ///
    /// It swings, or it closes the distance by putting the monster beside
    /// whoever it is after, or it gives up and goes home. This is also where
    /// a fight starts: the original checks it from the player's side —
    /// `LureMobsInRange` walks what the *player* can see and annoys anything
    /// within eight of them — and runs it from inside this same handler
    /// (`Mob/MOB.pas:1124`), so it belongs on this clock rather than on every
    /// movement packet.
    ///
    /// Returns the blows to deal and, separately, the monsters that moved, so
    /// the caller can tell everyone who can see them.
    pub fn fight_mobs(
        &self,
        players: &[(u16, (f32, f32))],
        now: Instant,
    ) -> (Vec<Attack>, Vec<(Mob, Turn)>) {
        let mut blows = Vec::new();
        let mut moved = Vec::new();
        let mut mobs = self.mobs.lock().unwrap();

        for mob in mobs.iter_mut().filter(|m| m.is_alive()) {
            if !mob.is_fighting() {
                if let Some((who, _)) = players.iter().find(|(_, at)| mob.is_lured_by(*at)) {
                    mob.provoked_by(*who);
                }
            }
            match mob.combat_turn(players, now) {
                Some(Reaction::Swing(attack)) => blows.push(attack),
                Some(Reaction::Closed { to, speed } | Reaction::WentHome { to, speed }) => {
                    moved.push((mob.clone(), Turn { walk: Some(to), speed }));
                }
                None => {}
            }
        }
        (blows, moved)
    }

    /// Leaves a blow for a player to find on its next packet.
    pub fn deal_to_player(&self, player: u16, attack: Attack) {
        self.incoming.lock().unwrap().entry(player).or_default().push(attack);
    }

    /// Takes everything left for a player, emptying the list.
    pub fn take_incoming(&self, player: u16) -> Vec<Attack> {
        self.incoming.lock().unwrap().remove(&player).unwrap_or_default()
    }

    /// Everyone in the world, with where they stand.
    pub fn positions(&self) -> Vec<(u16, (f32, f32))> {
        self.players
            .lock()
            .unwrap()
            .values()
            .filter_map(|p| Some((p.client_id, p.position()?)))
            .collect()
    }

    /// Every monster near enough to any of these points to matter, which is
    /// what the tick has to move.
    pub fn mobs_moved(&self, since: &[(u16, (f32, f32))]) -> Vec<Mob> {
        let mobs = self.mobs.lock().unwrap();
        mobs.iter()
            .filter(|m| m.is_alive())
            .filter(|m| {
                since.iter().any(|(_, at)| within(*at, m.position(), DISTANCE_TO_FORGET))
            })
            .cloned()
            .collect()
    }

    /// Brings back every monster whose time is up, and says which.
    pub fn revive_mobs(&self, now: Instant) -> Vec<Mob> {
        let mut mobs = self.mobs.lock().unwrap();
        mobs.iter_mut().filter_map(|m| m.revive(now).then(|| m.clone())).collect()
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
            Presence { client_id, account_id: 0, character: None, rotation: 0, outbox },
        );
        Some(client_id)
    }

    pub fn disconnect(&self, client_id: u16) -> Option<Presence> {
        self.incoming.lock().unwrap().remove(&client_id);
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

    pub fn turn(&self, client_id: u16, rotation: u32) {
        if let Some(presence) = self.players.lock().unwrap().get_mut(&client_id) {
            presence.rotation = rotation;
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

    /// Sends a frame to one player, if they are still connected.
    pub fn send_to(&self, client_id: u16, frame: Vec<u8>) {
        if let Some(presence) = self.players.lock().unwrap().get(&client_id) {
            presence.send(frame);
        }
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
