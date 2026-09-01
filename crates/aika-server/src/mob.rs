//! Monsters as they stand in the world.
//!
//! `aika_data::mobs` reads the two files and says what kinds exist and where
//! each copy is placed. This is the other half: the copies themselves, with
//! the health they have left and the clock that brings them back.
//!
//! A monster is not an NPC. An NPC is a fact about the map — it never moves,
//! never dies, and can be shared by every connection without a lock. A
//! monster has health, dies, and comes back, so it lives behind one, and the
//! world owns it rather than each session.
//!
//! The behaviour is a port of `Mob/MOB.pas`, not a paraphrase of it. Two of
//! its routines run on two threads at two rates, and both rates are part of
//! the behaviour: `TMobMovimentThread1` every three seconds decides where a
//! monster stands, and `TMobHandlerThread1` every second decides whether it
//! swings (`Connections/ServerSocket.pas:876`). Running the pair of them on
//! one fast clock is what made monsters sprint.

use aika_data::mobs::{MobKind, MobSpawn, MobTable};
use std::time::{Duration, Instant};

/// How often a monster decides where to stand (`ServerSocket.pas:880`).
pub const MOVE_TICK: Duration = Duration::from_secs(3);
/// How often it decides whether to swing (`ServerSocket.pas:877`).
pub const COMBAT_TICK: Duration = Duration::from_secs(1);

/// How close somebody has to come before a monster minds them
/// (`TBaseMob.LureMobsInRange`). Eight is close: you walk into it.
pub const AGGRO_RANGE: f32 = 8.0;

/// How far from where it started a monster will go before losing interest
/// (`Mob/MOB.pas:1256`). Measured from the monster to its own starting point.
pub const LEASH_RANGE: f32 = 40.0;

/// And the range the combat routine gives up at (`Mob/MOB.pas:932`).
pub const COMBAT_LEASH: f32 = 25.0;

/// How close it has to be to swing (`Mob/MOB.pas:865`).
pub const REACH: f32 = 3.0;

/// How often it swings (`Mob/MOB.pas:872`).
pub const ATTACK_EVERY: Duration = Duration::from_secs(3);

/// Ambling: it shifts this far on an axis whose difference is at least
/// `PATROL_THRESHOLD` (`Mob/MOB.pas:1155`).
pub const PATROL_STEP: f32 = 1.5;
pub const PATROL_THRESHOLD: i32 = 2;

/// Chasing: further per turn, and it only bothers with an axis that is at
/// least this far out (`Mob/MOB.pas:1234`).
pub const CHASE_STEP: f32 = 2.0;
pub const CHASE_THRESHOLD: i32 = 3;

/// An amble is reported every second turn; a chase every turn
/// (`Mob/MOB.pas:1166` against `1251`).
pub const STEPS_PER_PACKET: u8 = 2;

/// How many cells a player has around them for a monster to arrive in
/// (`TPlayer.SetCurrentNeighbors`).
pub const NEIGHBOURS: usize = 9;

/// The speeds those two go out at (`Mob/MOB.pas:1169` and `1251`).
pub const PATROL_SPEED: u8 = 25;
pub const CHASE_SPEED: u8 = 40;

/// What a monster is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    /// At its starting point, or on the way there.
    AtStart,
    /// At the far end of its patrol, or on the way there.
    AtDestination,
}

/// One monster in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Mob {
    /// Its id in the space shared with players and NPCs.
    pub id: u16,
    /// Index into the client's string table, for the name over its head.
    pub name_index: u16,
    /// The name from the file, for logs. The client never sees it.
    pub name: String,
    pub model: [u16; 3],
    /// Height, head and leg. The fourth byte of the spawn packet's sizes is
    /// the body, which monsters do not use.
    pub sizes: [u8; 3],
    pub level: u16,
    pub max_hp: u32,
    pub hp: u32,
    /// What killing it is worth.
    pub experience: u32,
    pub rotation: u16,
    pub spawn_type: u8,
    /// The two ends of its patrol. Equal for the kinds that stand still.
    pub home: (f32, f32),
    pub away: (f32, f32),
    /// How long it stands at an end before ambling back, from the spawn
    /// line rather than from a number I picked.
    pub wait: Duration,
    pub x: f32,
    pub y: f32,
    /// How long it takes to come back after dying.
    pub respawn: Duration,
    /// When it may come back, or `None` while it is alive.
    pub dead_until: Option<Instant>,
    /// Which end of its patrol it is at, or heading for.
    pub doing: Doing,
    /// Shifts taken since the last movement packet went out.
    pub steps: u8,
    /// When it last stood still at an end of its patrol.
    pub rested_at: Option<Instant>,
    /// When it last swung, which is its own clock: the original runs the
    /// swinging on a different thread from the walking.
    pub swung_at: Option<Instant>,
    /// Who it is fighting. `None` is a monster minding its own business,
    /// which is most of them most of the time.
    pub attacker: Option<u16>,
    /// How hard it hits, from its level. Monsters have no gear to read.
    pub attack: u32,
    /// Whether walking past it starts a fight. False for guards, mutants and
    /// the tame `Max` kinds, which have to be hit first.
    pub lurable: bool,
    /// The skills the kind knows, for the animation a blow plays. Most kinds
    /// have none, and swing with an empty one.
    pub skills: [u16; 5],
    /// Which band its drops come from.
    pub drop_index: u16,
}

impl Mob {
    /// Builds one copy from its kind and the point it was placed at.
    pub fn place(id: u16, kind: &MobKind, spawn: &MobSpawn) -> Self {
        Self {
            id,
            name_index: kind.name_index,
            name: kind.name.clone(),
            model: kind.model,
            sizes: kind.sizes,
            level: kind.level,
            max_hp: kind.hp,
            hp: kind.hp,
            experience: kind.experience,
            rotation: kind.rotation,
            spawn_type: kind.spawn_type,
            home: spawn.start,
            away: spawn.end,
            wait: Duration::from_secs(spawn.start_wait.max(1) as u64),
            x: spawn.start.0,
            y: spawn.start.1,
            respawn: Duration::from_secs(kind.respawn_seconds.max(1) as u64),
            dead_until: None,
            doing: Doing::AtStart,
            steps: 0,
            rested_at: None,
            swung_at: None,
            attacker: None,
            // Monsters wear nothing, so their attack is their level. The
            // number is chosen so a level-appropriate one takes a few hits
            // off a player rather than none or all of them.
            attack: 5 + kind.level as u32 * 3,
            lurable: kind.is_lurable(),
            skills: kind.skills,
            drop_index: kind.drop_index,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.dead_until.is_none()
    }

    /// How far it is from a point.
    pub fn distance_to(&self, to: (f32, f32)) -> f32 {
        let (dx, dy) = (to.0 - self.x, to.1 - self.y);
        (dx * dx + dy * dy).sqrt()
    }

    pub fn position(&self) -> (f32, f32) {
        (self.x, self.y)
    }

    /// The skill it swings with, or zero when its kind lists none.
    pub fn attack_skill(&self) -> u16 {
        self.skills.iter().copied().find(|&s| s != 0).unwrap_or(0)
    }

    /// Whether it walks at all.
    pub fn is_rooted(&self) -> bool {
        self.home == self.away
    }

    /// Where it is ambling to.
    pub fn destination(&self) -> (f32, f32) {
        match self.doing {
            Doing::AtStart => self.away,
            Doing::AtDestination => self.home,
        }
    }

    /// Takes damage and says whether that was the killing blow.
    pub fn wound(&mut self, damage: u32, now: Instant) -> bool {
        if !self.is_alive() {
            return false;
        }
        self.hp = self.hp.saturating_sub(damage);
        if self.hp > 0 {
            return false;
        }
        self.attacker = None;
        self.dead_until = Some(now + self.respawn);
        true
    }

    /// Brings it back at full health where it started, if its time is up.
    pub fn revive(&mut self, now: Instant) -> bool {
        match self.dead_until {
            Some(at) if now >= at => {
                self.hp = self.max_hp;
                self.x = self.home.0;
                self.y = self.home.1;
                self.dead_until = None;
                self.doing = Doing::AtStart;
                self.attacker = None;
                self.steps = 0;
                self.swung_at = None;
                true
            }
            _ => false,
        }
    }

    /// Marks it as fighting somebody, which is what a passive monster needs
    /// before it does anything at all.
    pub fn provoked_by(&mut self, who: u16) {
        if self.is_alive() {
            self.attacker = Some(who);
        }
    }

    pub fn is_fighting(&self) -> bool {
        self.attacker.is_some()
    }

    /// Whether somebody this close should be minded, which the original
    /// decides from the player's side rather than the monster's
    /// (`TBaseMob.LureMobsInRange`).
    pub fn is_lured_by(&self, at: (f32, f32)) -> bool {
        self.lurable
            && self.is_alive()
            && !self.is_fighting()
            && self.distance_to(at) <= AGGRO_RANGE
    }

    /// Shifts on each axis whose difference rounds to at least `threshold`,
    /// by `step`. Per axis and not along the line between the two points,
    /// which is what the original does and why an amble looks a little
    /// square.
    fn shift_towards(&mut self, to: (f32, f32), step: f32, threshold: i32) {
        if delphi_round(to.0 - self.x).abs() >= threshold {
            self.x += if to.0 > self.x { step } else { -step };
        }
        if delphi_round(to.1 - self.y).abs() >= threshold {
            self.y += if to.1 > self.y { step } else { -step };
        }
    }

    /// Whether it has arrived somewhere, which the original asks as
    /// `CurrentPos.Distance(DestPos) <= 0` — and `Distance` rounds to a whole
    /// number, so "arrived" means within half a unit.
    fn has_arrived_at(&self, to: (f32, f32)) -> bool {
        delphi_round(self.distance_to(to)) <= 0
    }

    // ---- the movement thread ----------------------------------------------

    /// `TMobSPosition.MobMoviment`, which runs every three seconds.
    ///
    /// A monster that is not fighting ambles between its two points, waiting
    /// at each end for as long as its spawn line says. One that is fighting
    /// drifts towards whoever it is fighting — the *closing* is the other
    /// thread's job, not this one's — and snaps back to where it started if
    /// it finds itself more than forty from it with nobody left near home.
    pub fn move_turn(&mut self, players: &[(u16, (f32, f32))], now: Instant) -> Turn {
        if !self.is_alive() {
            return Turn::default();
        }

        // Not fighting: amble. The original runs this branch only when
        // `IsAttacked` is false, so a monster in a fight never patrols.
        if !self.is_fighting() {
            return self.amble(now);
        }

        let mut turn = Turn::default();
        if let Some(at) = self.attacker.and_then(|who| position_of(players, who)) {
            self.shift_towards(at, CHASE_STEP, CHASE_THRESHOLD);
            // A chase reports every turn, whether or not it actually shifted.
            turn = Turn::walking(self.position(), CHASE_SPEED);
        }

        // And then, in the same turn, the leash. Measured from the monster to
        // its own starting point, not from the player.
        if self.distance_to(self.home) > LEASH_RANGE {
            match nearest_within(players, self.home, LEASH_RANGE) {
                // Somebody else is still near home: it turns on them instead.
                Some((who, _)) => self.attacker = Some(who),
                // Nobody is. It is standing where it started again, at once:
                // `Self.CurrentPos := Self.InitPos`, not a walk home.
                None => {
                    self.go_back();
                    turn = Turn::walking(self.position(), CHASE_SPEED);
                }
            }
        }
        turn
    }

    fn amble(&mut self, now: Instant) -> Turn {
        if self.is_rooted() {
            return Turn::default();
        }
        // The wait at each end, from the spawn line. The original uses the
        // starting one at both ends, and copying that is the point.
        if self.rested_at.is_some_and(|at| now.duration_since(at) <= self.wait) {
            return Turn::default();
        }

        let destination = self.destination();
        self.shift_towards(destination, PATROL_STEP, PATROL_THRESHOLD);
        self.steps += 1;

        // Every second shift goes out, not every one.
        let mut turn = Turn::default();
        if self.steps >= STEPS_PER_PACKET {
            self.steps = 0;
            turn = Turn::walking(self.position(), PATROL_SPEED);
        }

        if self.has_arrived_at(destination) {
            self.x = destination.0;
            self.y = destination.1;
            self.doing = match self.doing {
                Doing::AtStart => Doing::AtDestination,
                Doing::AtDestination => Doing::AtStart,
            };
            self.steps = 0;
            self.rested_at = Some(now);
        }
        turn
    }

    /// Back to the starting point, at once, and back to ambling from there.
    fn go_back(&mut self) {
        self.x = self.home.0;
        self.y = self.home.1;
        self.doing = Doing::AtStart;
        self.attacker = None;
        self.steps = 0;
        self.rested_at = None;
    }

    // ---- the combat thread ------------------------------------------------

    /// `TMobSPosition.MobHandler`, which runs every second.
    ///
    /// Three things can come of a turn, and the order is the original's:
    ///
    /// 1. Whoever it is fighting is within three: it swings, once every three
    ///    seconds.
    /// 2. They are not, but somebody is within twenty-five of where it
    ///    started: it puts itself *beside* that person and tells the client,
    ///    which walks it the whole way there at speed forty. This is the
    ///    closing, and it is why a monster in the original sticks to you —
    ///    the ambling thread's two-unit drift would never keep up.
    /// 3. Nobody is: it is standing where it started again, whole, and has
    ///    forgotten the fight.
    pub fn combat_turn(&mut self, players: &[(u16, (f32, f32))], now: Instant) -> Option<Reaction> {
        if !self.is_alive() || !self.is_fighting() {
            return None;
        }

        let who = self.attacker?;
        if let Some(at) = position_of(players, who) {
            if self.distance_to(at) <= REACH {
                if self.swung_at.is_some_and(|last| now.duration_since(last) < ATTACK_EVERY) {
                    return None;
                }
                self.swung_at = Some(now);
                return Some(Reaction::Swing(Attack {
                    attacker: self.id,
                    target: who,
                    damage: self.attack,
                    skill: self.attack_skill(),
                }));
            }
        }

        // Out of reach, or they logged out. Either way the original looks for
        // somebody — anybody — within twenty-five of where the monster
        // started, and takes the first it finds.
        match nearest_within(players, self.home, COMBAT_LEASH) {
            Some((who, at)) => {
                self.attacker = Some(who);
                let beside = self.beside(at);
                self.x = beside.0;
                self.y = beside.1;
                Some(Reaction::Closed { to: beside, speed: CHASE_SPEED })
            }
            None => {
                self.go_back();
                // Going home makes it whole again, which is what stops a
                // player whittling one down over several pulls.
                self.hp = self.max_hp;
                Some(Reaction::WentHome { to: self.home, speed: CHASE_SPEED })
            }
        }
    }

    /// One of the nine cells around somebody (`TPlayer.SetCurrentNeighbors`).
    ///
    /// They are barely a unit apart — half a unit plus a tenth per pair — so
    /// this is "on top of them", not "somewhere nearby". Which of the nine is
    /// random in the original; here it turns with the monster's own count, so
    /// two monsters do not stack and a test can predict it.
    fn beside(&mut self, at: (f32, f32)) -> (f32, f32) {
        let i = (self.steps as usize).wrapping_add(self.id as usize) % NEIGHBOURS;
        self.steps = self.steps.wrapping_add(1);
        let offset = 0.5 + (i / 2) as f32 * 0.1;
        if i % 2 == 0 {
            (at.0 - offset, at.1 - offset)
        } else {
            (at.0 + offset, at.1 + offset)
        }
    }
}

/// Delphi's `Round`, which sends a half to the nearest *even* whole number
/// rather than away from zero.
///
/// It is not a detail here. A monster stops shifting an axis once the
/// difference rounds below two, so it very often ends up exactly half a unit
/// from where it was going, and whether `Round` calls that 0 or 1 is what
/// decides whether it ever arrives.
fn delphi_round(value: f32) -> i32 {
    let floor = value.floor();
    if (value - floor - 0.5).abs() < 1e-6 {
        let below = floor as i32;
        return if below % 2 == 0 { below } else { below + 1 };
    }
    value.round() as i32
}

/// Where one player stands, or `None` when they have left.
fn position_of(players: &[(u16, (f32, f32))], who: u16) -> Option<(f32, f32)> {
    players.iter().find(|(id, _)| *id == who).map(|(_, at)| *at)
}

/// The first player within `range` of a point.
///
/// First, not nearest: the original breaks out of its loop on the first one
/// it finds, in connection order.
fn nearest_within(
    players: &[(u16, (f32, f32))],
    of: (f32, f32),
    range: f32,
) -> Option<(u16, (f32, f32))> {
    players
        .iter()
        .find(|(_, at)| {
            let (dx, dy) = (at.0 - of.0, at.1 - of.1);
            delphi_round((dx * dx + dy * dy).sqrt()) as f32 <= range
        })
        .copied()
}

/// What the movement thread decided.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Turn {
    /// Where it now stands, when the client should be told. The original
    /// sends where the monster *is*, having already moved it there, and lets
    /// the client walk it the whole way.
    pub walk: Option<(f32, f32)>,
    /// How fast the client should walk it there.
    pub speed: u8,
}

impl Turn {
    fn walking(to: (f32, f32), speed: u8) -> Self {
        Self { walk: Some(to), speed }
    }
}

/// What the combat thread decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reaction {
    /// It swung at somebody.
    Swing(Attack),
    /// It put itself beside somebody. The client is told, and walks it there.
    Closed { to: (f32, f32), speed: u8 },
    /// It gave up and is standing where it started again, whole.
    WentHome { to: (f32, f32), speed: u8 },
}

/// A monster swinging at somebody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attack {
    /// The monster that swung.
    pub attacker: u16,
    pub target: u16,
    pub damage: u32,
    /// Which of its skills it swung with, which is where the animation the
    /// client plays comes from. Zero for a kind that has none.
    pub skill: u16,
}

/// Every monster the table places, in id order.
pub fn place_all(table: &MobTable) -> Vec<Mob> {
    table.placed().map(|(id, kind, spawn)| Mob::place(id, kind, spawn)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
1,Max Filhote,216,0,0,200,134,1,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,15,0,1
7,Dorminhoco,111,0,0,50,0,3,7,119,119,1025,0,0,0,0,0,0,30,0,0,0,0,0,10,0,0";

    const LIST: &str = "\
1,1,1,1,Max_Filhote,Max_Filhote,0,0,0,3496,844,11,8,0,3474,831,11,8,0
1,1,1,1,Dorminhoco,Dorminhoco,0,0,0,100,100,11,8,0,110,110,11,8,0";

    fn one() -> Mob {
        let table = MobTable::parse(INFO, LIST).unwrap();
        place_all(&table).into_iter().next().expect("nothing was placed")
    }

    #[test]
    fn a_placed_monster_starts_whole_and_at_its_first_point() {
        let mob = one();

        assert_eq!(mob.id, aika_data::mobs::FIRST_MOB_ID);
        assert_eq!(mob.name, "Max Filhote");
        assert_eq!(mob.model[0], 216);
        assert_eq!((mob.hp, mob.max_hp), (200, 200));
        assert_eq!(mob.position(), (3496.0, 844.0));
        assert_eq!(mob.experience, 15);
        assert!(mob.is_alive());
    }

    /// An inactive kind is in the file and not in the world.
    #[test]
    fn an_inactive_kind_is_not_placed() {
        let table = MobTable::parse(INFO, LIST).unwrap();
        let placed = place_all(&table);

        assert_eq!(placed.len(), 1, "the sleeper was placed");
        assert!(placed.iter().all(|m| m.name != "Dorminhoco"));
    }

    #[test]
    fn wounding_takes_health_and_the_last_blow_kills() {
        let now = Instant::now();
        let mut mob = one();

        assert!(!mob.wound(50, now), "it died too early");
        assert_eq!(mob.hp, 150);
        assert!(mob.is_alive());

        assert!(mob.wound(150, now), "it survived a killing blow");
        assert_eq!(mob.hp, 0);
        assert!(!mob.is_alive());
    }

    /// Two players landing on the same corpse must not both be paid for it.
    #[test]
    fn a_corpse_cannot_be_killed_twice() {
        let now = Instant::now();
        let mut mob = one();

        assert!(mob.wound(9999, now));
        assert!(!mob.wound(9999, now), "the corpse was killed again");
    }

    /// Overkill must not wrap the health round to a huge number.
    #[test]
    fn damage_past_the_last_point_of_health_does_not_wrap() {
        let now = Instant::now();
        let mut mob = one();

        mob.wound(u32::MAX, now);
        assert_eq!(mob.hp, 0);
    }

    #[test]
    fn it_comes_back_whole_and_where_it_started() {
        let now = Instant::now();
        let mut mob = one();
        mob.x = 4000.0;
        mob.wound(9999, now);

        assert!(!mob.revive(now), "it came back before its time");
        assert!(mob.revive(now + mob.respawn), "it never came back");

        assert_eq!(mob.hp, mob.max_hp);
        assert_eq!(mob.position(), mob.home);
        assert!(mob.is_alive());
    }

    /// A kind with no respawn time in the file would otherwise come back the
    /// instant it dies, which reads as a monster that cannot be killed.
    #[test]
    fn respawn_is_never_instant() {
        let mob = one();
        assert!(mob.respawn >= Duration::from_secs(1));
    }

    // ---- the two threads --------------------------------------------------

    const WALKER: &str =
        "1,Andarilho,216,0,0,200,0,10,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,15,0,1";

    /// A monster whose two points are eighteen and six apart — both a whole
    /// number of its step — so it walks the round trip. Not a `Max`, so
    /// walking into it is enough to start something.
    fn walker() -> Mob {
        const LIST: &str = "1,1,1,1,Andarilho,Andarilho,0,0,0,100,100,11,8,0,113,101,11,8,0";
        let table = MobTable::parse(WALKER, LIST).unwrap();
        place_all(&table).into_iter().next().unwrap()
    }

    /// And one whose points are not, which is most of them.
    fn stopper() -> Mob {
        const LIST: &str = "1,1,1,1,Andarilho,Andarilho,0,0,0,100,100,11,8,0,112,100,11,8,0";
        let table = MobTable::parse(WALKER, LIST).unwrap();
        place_all(&table).into_iter().next().unwrap()
    }

    /// Nobody in the world.
    const ALONE: &[(u16, (f32, f32))] = &[];

    /// Turns of the movement thread, three seconds apart.
    fn amble_for(mob: &mut Mob, turns: usize) -> Vec<Turn> {
        let mut now = Instant::now();
        let mut out = Vec::new();
        for _ in 0..turns {
            let turn = mob.move_turn(ALONE, now);
            if turn != Turn::default() {
                out.push(turn);
            }
            now += MOVE_TICK;
        }
        out
    }

    /// Delphi's rounding is not Rust's, and the difference decides whether a
    /// monster ever finishes its walk.
    #[test]
    fn a_half_rounds_to_the_nearest_even_number() {
        assert_eq!(delphi_round(0.5), 0, "Rust would say 1 here");
        assert_eq!(delphi_round(1.5), 2);
        assert_eq!(delphi_round(2.5), 2);
        assert_eq!(delphi_round(0.7071), 1);
        assert_eq!(delphi_round(-1.5), -2);
    }

    /// The thing that makes the world liveable: a monster does not pick a
    /// fight with somebody walking past, and the movement thread never starts
    /// one at all. Being hit is what does.
    #[test]
    fn a_monster_left_alone_never_starts_a_fight() {
        let mut mob = walker();
        let players = [(7u16, (101.0f32, 100.0f32))];
        let mut now = Instant::now();
        for _ in 0..20 {
            mob.move_turn(&players, now);
            now += MOVE_TICK;
        }
        assert!(!mob.is_fighting(), "the walking thread picked a fight");

        mob.provoked_by(7);
        assert_eq!(mob.attacker, Some(7), "hitting it did not annoy it");
    }

    /// The fighting thread is the one that notices somebody standing on top
    /// of it, and it notices at eight, which means walking into it.
    #[test]
    fn walking_into_one_is_what_annoys_it() {
        let mob = walker();
        assert!(mob.is_lured_by((104.0, 100.0)), "eight away is close enough");
        assert!(!mob.is_lured_by((120.0, 100.0)), "twenty away is not");
    }

    /// Guards, mutants and the tame `Max` kinds have to be hit first, which
    /// is why the starting area — nothing but `Max` — is walkable.
    #[test]
    fn the_tame_kinds_have_to_be_hit_first() {
        let mut tame = one();
        assert!(tame.name.starts_with("Max"));
        assert!(!tame.is_lured_by(tame.position()), "a Max was lured");

        tame.provoked_by(7);
        assert_eq!(tame.attacker, Some(7), "a Max would not fight back");
    }

    #[test]
    fn a_monster_with_nobody_about_ambles_between_its_two_points() {
        let mut mob = walker();
        let (from, to) = (mob.home, mob.away);
        let mut now = Instant::now();

        let mut seen_home = false;
        let mut seen_away = false;

        for _ in 0..400 {
            mob.move_turn(ALONE, now);
            now += MOVE_TICK;

            assert!(
                (from.0 - 1.0..=to.0 + 1.0).contains(&mob.x),
                "it wandered off its patrol to {}",
                mob.x
            );
            seen_home |= mob.position() == from;
            seen_away |= mob.position() == to;
        }

        assert!(seen_away, "it never reached the far end");
        assert!(seen_home, "it never came back");
    }

    /// Three seconds a turn and a step and a half an axis is the whole reason
    /// monsters look like they are strolling rather than sprinting.
    #[test]
    fn an_amble_is_slower_than_a_person_walks() {
        let mut mob = walker();
        let mut now = Instant::now();
        let from = mob.position();

        for _ in 0..4 {
            mob.move_turn(ALONE, now);
            now += MOVE_TICK;
        }

        let covered = mob.distance_to(from);
        let seconds = 4.0 * MOVE_TICK.as_secs_f32();
        assert!(
            covered / seconds < 1.0,
            "it covered {covered} in {seconds}s, which is a run"
        );
    }

    /// The kinds that stand still stand still.
    #[test]
    fn a_rooted_monster_never_moves() {
        let mut mob = walker();
        mob.away = mob.home;
        amble_for(&mut mob, 20);
        assert_eq!(mob.position(), mob.home);
    }

    /// It stands at each end for as long as the spawn line says, rather than
    /// turning straight round.
    #[test]
    fn it_waits_at_each_end_for_as_long_as_the_file_says() {
        let mut mob = walker();
        assert_eq!(mob.wait, Duration::from_secs(8));

        let mut now = Instant::now();
        for _ in 0..40 {
            mob.move_turn(ALONE, now);
            now += MOVE_TICK;
            if mob.position() == mob.away {
                break;
            }
        }
        assert_eq!(mob.position(), mob.away, "it never got to the far end");

        // It has arrived and turned round; the next turn is inside the wait.
        mob.move_turn(ALONE, now);
        assert_eq!(mob.position(), mob.away, "it turned round without waiting");
    }

    /// The client hears about every second shift of an amble, not every one,
    /// which is what keeps a patrol from being a packet storm.
    #[test]
    fn the_client_is_not_told_about_every_shift() {
        let mut mob = walker();
        let turns = amble_for(&mut mob, 8);
        assert!(turns.len() < 8, "every single shift went out as a packet");
        assert!(!turns.is_empty(), "nothing went out at all");
        assert!(
            turns.iter().all(|t| t.speed == PATROL_SPEED),
            "an amble went out at the wrong speed"
        );
    }

    /// A monster in a fight stops patrolling. It would otherwise be walking
    /// its beat and fighting at the same time.
    #[test]
    fn a_monster_in_a_fight_stops_patrolling() {
        let mut mob = walker();
        mob.provoked_by(7);
        let mut now = Instant::now();
        for _ in 0..10 {
            assert_eq!(mob.move_turn(ALONE, now), Turn::default());
            now += MOVE_TICK;
        }
        assert_eq!(mob.position(), mob.home, "it patrolled mid-fight");
    }

    /// Most monsters in the shipped data never finish their walk.
    ///
    /// An axis stops shifting once its difference rounds below two, so unless
    /// the two ends are a whole number of steps apart the monster ends up
    /// half a unit short on each axis — and `Round(0.707)` is 1, not 0, so it
    /// never counts as arrived, never turns round, and stands there for good.
    /// Three thousand seven hundred of the five thousand four hundred walking
    /// monsters in `MonsterListCSV.csv` are like this. It is the original's
    /// behaviour, not a rounding slip of ours, and it is most of why an Aika
    /// field is full of things standing about.
    #[test]
    fn a_patrol_that_does_not_divide_by_the_step_parks_short_of_the_end() {
        let mut mob = stopper();
        let mut now = Instant::now();
        for _ in 0..200 {
            mob.move_turn(ALONE, now);
            now += MOVE_TICK;
        }

        assert_ne!(mob.position(), mob.away, "this one was supposed to fall short");
        assert!(
            mob.distance_to(mob.away) < 1.0,
            "it stopped {} from the end, which is not short",
            mob.distance_to(mob.away)
        );

        let parked = mob.position();
        mob.move_turn(ALONE, now);
        assert_eq!(mob.position(), parked, "it moved again after settling");
    }

    // ---- closing in -------------------------------------------------------

    /// The closing is the combat thread's, not the movement thread's: it puts
    /// the monster in one of the nine cells around the player and lets the
    /// client walk it there. The drift the movement thread does would never
    /// keep up on its own.
    #[test]
    fn it_closes_by_putting_itself_beside_you() {
        let mut mob = walker();
        mob.provoked_by(7);
        let players = [(7u16, (108.0f32, 104.0f32))];

        let Some(Reaction::Closed { to, speed }) = mob.combat_turn(&players, Instant::now())
        else {
            panic!("it did not close in");
        };
        assert_eq!(speed, CHASE_SPEED);
        assert_eq!(mob.position(), to, "it told the client somewhere it is not");
        assert!(
            mob.distance_to(players[0].1) <= 1.5,
            "it arrived {} away, which is not beside anybody",
            mob.distance_to(players[0].1)
        );
    }

    /// And then, being beside them, it swings.
    #[test]
    fn closing_in_is_followed_by_a_swing() {
        let mut mob = walker();
        mob.provoked_by(7);
        let players = [(7u16, (108.0f32, 104.0f32))];
        let now = Instant::now();

        assert!(matches!(
            mob.combat_turn(&players, now),
            Some(Reaction::Closed { .. })
        ));
        let Some(Reaction::Swing(attack)) = mob.combat_turn(&players, now + COMBAT_TICK) else {
            panic!("it stood next to somebody and did nothing");
        };
        assert_eq!(attack.target, 7);
        assert_eq!(attack.attacker, mob.id);
        assert!(attack.damage > 0);
    }

    /// A swing is on its own clock, three seconds, so a monster does not
    /// empty somebody in the second they walk past.
    #[test]
    fn it_swings_once_every_three_seconds() {
        let mut mob = walker();
        mob.provoked_by(7);
        let players = [(7u16, (101.0f32, 100.0f32))];
        let now = Instant::now();

        assert!(
            matches!(mob.combat_turn(&players, now), Some(Reaction::Swing(_))),
            "the first swing"
        );
        assert!(
            mob.combat_turn(&players, now + COMBAT_TICK).is_none(),
            "it swung again a second later"
        );
        assert!(
            matches!(
                mob.combat_turn(&players, now + ATTACK_EVERY),
                Some(Reaction::Swing(_))
            ),
            "it never swung again"
        );
    }

    // ---- letting go -------------------------------------------------------

    /// Dragging a monster across the map is the oldest trick there is. Once
    /// nobody is left within twenty-five of where it started it goes home,
    /// and the original does not walk it there: `CurrentPos := InitPos`.
    #[test]
    fn a_monster_led_too_far_snaps_back_whole() {
        let mut mob = walker();
        mob.provoked_by(7);
        mob.hp = 1;
        mob.x = 130.0;

        let players = [(7u16, (mob.home.0 + 200.0, mob.home.1))];
        let Some(Reaction::WentHome { to, .. }) = mob.combat_turn(&players, Instant::now()) else {
            panic!("it followed forever");
        };

        assert_eq!(to, mob.home);
        assert_eq!(mob.position(), mob.home, "it did not go back");
        assert!(!mob.is_fighting(), "it went home still cross");
        assert_eq!(mob.hp, mob.max_hp, "it went home hurt, so a pull whittles it down");
        assert_eq!(mob.doing, Doing::AtStart, "it went home facing the wrong way");
    }

    /// But somebody else standing near its spawn is a new fight, not a walk
    /// home. It is the reason a monster you pulled off a friend turns on you.
    #[test]
    fn somebody_else_near_its_spawn_becomes_the_new_target() {
        let mut mob = walker();
        mob.provoked_by(7);
        mob.x = 130.0;

        let players = [
            (7u16, (mob.home.0 + 90.0, mob.home.1)),
            (9u16, (mob.home.0 + 2.0, mob.home.1)),
        ];
        assert!(matches!(
            mob.combat_turn(&players, Instant::now()),
            Some(Reaction::Closed { .. })
        ));
        assert_eq!(mob.attacker, Some(9), "it did not turn on the one still there");
    }

    /// Somebody logging out mid-fight is the same thing as running away.
    #[test]
    fn a_target_that_vanishes_sends_it_home() {
        let mut mob = walker();
        mob.provoked_by(7);
        mob.x = 130.0;

        assert!(matches!(
            mob.combat_turn(ALONE, Instant::now()),
            Some(Reaction::WentHome { .. })
        ));
        assert!(!mob.is_fighting());
        assert_eq!(mob.position(), mob.home);
    }

    /// The movement thread has a leash of its own, wider than the fighting
    /// one and measured from the monster rather than from the player.
    #[test]
    fn the_movement_thread_has_a_leash_of_its_own() {
        let mut mob = walker();
        mob.provoked_by(7);
        mob.x = mob.home.0 + LEASH_RANGE + 10.0;

        let far = [(7u16, (mob.home.0 + 400.0, mob.home.1))];
        mob.move_turn(&far, Instant::now());
        assert_eq!(mob.position(), mob.home, "it drifted away for good");
        assert!(!mob.is_fighting());
    }

    #[test]
    fn a_dead_monster_does_nothing() {
        let mut mob = walker();
        let now = Instant::now();
        let players = [(7u16, (101.0f32, 100.0f32))];
        mob.wound(9999, now);

        assert_eq!(mob.move_turn(&players, now), Turn::default());
        assert_eq!(mob.combat_turn(&players, now), None);
        assert_eq!(mob.position(), mob.home, "a corpse moved");
        assert!(!mob.is_lured_by((101.0, 100.0)), "a corpse was lured");
        mob.provoked_by(7);
        assert!(!mob.is_fighting(), "a corpse was annoyed");
    }

    /// Coming back has to clear what it was doing, or it carries on fighting
    /// somebody who left.
    #[test]
    fn coming_back_forgets_what_it_was_doing() {
        let mut mob = walker();
        let now = Instant::now();

        mob.provoked_by(7);
        mob.wound(9999, now);
        mob.revive(now + mob.respawn);

        assert_eq!(mob.doing, Doing::AtStart);
        assert_eq!(mob.attacker, None);
    }

    /// A monster swings with one of its own skills, which is where the
    /// animation the client plays comes from.
    #[test]
    fn a_blow_carries_the_skill_it_was_made_with() {
        let mut mob = walker();
        mob.skills = [0, 8216, 0, 0, 0];
        mob.provoked_by(7);
        let players = [(7u16, (101.0f32, 100.0f32))];
        let now = Instant::now();

        let Some(Reaction::Swing(attack)) = mob.combat_turn(&players, now) else {
            panic!("it did not swing");
        };
        assert_eq!(attack.skill, 8216, "the blow forgot which skill it was");

        mob.skills = [0; 5];
        mob.swung_at = None;
        let Some(Reaction::Swing(attack)) = mob.combat_turn(&players, now) else {
            panic!("it did not swing");
        };
        assert_eq!(attack.skill, 0, "a kind with no skills swings with none");
    }
}
