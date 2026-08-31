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

use aika_data::mobs::{MobKind, MobSpawn, MobTable};
use std::time::{Duration, Instant};

/// How close somebody has to come before a monster minds them.
///
/// Eight, and the original checks it from the *player's* side rather than
/// the monster's (`TBaseMob.LureMobsInRange`). Eight is close: you have to
/// walk into a monster to annoy it, which is why the world is not a corridor
/// of things chasing you.
pub const AGGRO_RANGE: f32 = 8.0;

/// How far from where it started a monster will let itself be led before it
/// gives up and walks back (`Mob/MOB.pas:932`).
pub const LEASH_RANGE: f32 = 25.0;

/// How close it has to be to swing (`Mob/MOB.pas:865`). Three: it has to be
/// on top of you.
pub const REACH: f32 = 3.0;

/// How often it swings (`Mob/MOB.pas:872`).
pub const ATTACK_EVERY: Duration = Duration::from_secs(3);

/// How far a patrolling monster shifts per turn, per axis
/// (`Mob/MOB.pas:1155`). One and a half units: a monster ambles.
pub const PATROL_STEP: f32 = 1.5;

/// How many turns of ambling go by between movement packets
/// (`Mob/MOB.pas:1166`). The client is told every second step, not every one.
pub const STEPS_PER_PACKET: u8 = 2;

/// How close it has to get to its destination to count as arrived.
pub const ARRIVED: f32 = 2.0;

/// Speeds the client is told to move at: ambling, and going somewhere
/// (`Mob/MOB.pas:1169` and `958`).
pub const PATROL_SPEED: u8 = 25;
pub const CHASE_SPEED: u8 = 40;

/// How far it moves per turn while chasing. Faster than an amble, or nothing
/// could ever be caught.
pub const CHASE_STEP: f32 = 4.0;

/// What a monster is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    /// Standing at one end of its patrol, waiting out the file's own timer.
    Waiting,
    /// Ambling to the other end.
    Patrolling,
    /// After somebody, because they hit it or walked into it.
    Chasing(u16),
    /// Walking back to where it started, ignoring everything.
    GoingHome,
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
    /// How long it stands at each end before ambling back, from the spawn
    /// line rather than from a number I picked.
    pub home_wait: Duration,
    pub away_wait: Duration,
    pub x: f32,
    pub y: f32,
    /// How long it takes to come back after dying.
    pub respawn: Duration,
    /// When it may come back, or `None` while it is alive.
    pub dead_until: Option<Instant>,
    /// What it is doing.
    pub doing: Doing,
    /// Which end of the patrol it is walking towards.
    pub heading_away: bool,
    /// When it may move or swing again.
    pub ready_at: Option<Instant>,
    /// Steps taken since the last movement packet went out.
    pub steps: u8,
    /// How hard it hits, from its level. Monsters have no gear to read.
    pub attack: u32,
    /// What it has taken off players, so a kill can be credited.
    pub last_hurt_by: Option<u16>,
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
            home_wait: Duration::from_secs(spawn.start_wait.max(1) as u64),
            away_wait: Duration::from_secs(spawn.end_wait.max(1) as u64),
            x: spawn.start.0,
            y: spawn.start.1,
            respawn: Duration::from_secs(kind.respawn_seconds.max(1) as u64),
            dead_until: None,
            doing: Doing::Waiting,
            heading_away: true,
            ready_at: None,
            steps: 0,
            // Monsters wear nothing, so their attack is their level. The
            // number is chosen so a level-appropriate one takes a few hits
            // off a player rather than none or all of them.
            attack: 5 + kind.level as u32 * 3,
            last_hurt_by: None,
            skills: kind.skills,
            drop_index: kind.drop_index,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.dead_until.is_none()
    }

    /// Whether it walks at all. The kinds that do not have both ends of
    /// their patrol in the same place.
    pub fn is_rooted(&self) -> bool {
        self.home == self.away
    }

    /// Where it is walking to right now.
    pub fn destination(&self) -> (f32, f32) {
        if self.heading_away {
            self.away
        } else {
            self.home
        }
    }

    /// The skill it swings with, or zero when it has none.
    pub fn attack_skill(&self) -> u16 {
        self.skills.iter().copied().find(|&s| s != 0).unwrap_or(0)
    }

    /// Shifts towards a point by at most `step` on each axis.
    ///
    /// Per axis rather than along the line between them, which is what the
    /// original does and is why a monster's amble is a little square.
    fn shift_towards(&mut self, to: (f32, f32), step: f32) {
        if (to.0 - self.x).abs() >= ARRIVED {
            self.x += if to.0 > self.x { step } else { -step };
        }
        if (to.1 - self.y).abs() >= ARRIVED {
            self.y += if to.1 > self.y { step } else { -step };
        }
    }

    /// Whether it is close enough to a point to have arrived.
    fn arrived_at(&self, to: (f32, f32)) -> bool {
        (to.0 - self.x).abs() < ARRIVED && (to.1 - self.y).abs() < ARRIVED
    }

    /// How far it is from a point.
    pub fn distance_to(&self, to: (f32, f32)) -> f32 {
        let (dx, dy) = (to.0 - self.x, to.1 - self.y);
        (dx * dx + dy * dy).sqrt()
    }

    pub fn position(&self) -> (f32, f32) {
        (self.x, self.y)
    }

    /// Takes damage and says whether that was the killing blow.
    ///
    /// Returns false for a monster that is already down, so two players
    /// landing on the same corpse cannot both be paid for it.
    pub fn wound(&mut self, damage: u32, now: Instant) -> bool {
        if !self.is_alive() {
            return false;
        }
        self.hp = self.hp.saturating_sub(damage);
        if self.hp > 0 {
            return false;
        }
        self.doing = Doing::Waiting;
        self.dead_until = Some(now + self.respawn);
        true
    }

    /// Brings it back at full health, in the place it was first put, if its
    /// time is up. Says whether it came back.
    pub fn revive(&mut self, now: Instant) -> bool {
        match self.dead_until {
            Some(at) if now >= at => {
                self.hp = self.max_hp;
                self.x = self.home.0;
                self.y = self.home.1;
                self.dead_until = None;
                self.doing = Doing::Waiting;
                self.heading_away = true;
                self.ready_at = None;
                self.steps = 0;
                self.last_hurt_by = None;
                true
            }
            _ => false,
        }
    }

    /// One turn of thinking, given where the nearest player is.
    ///
    /// `nearest` is the closest player and where they stand, or `None` when
    /// there is nobody about.
    ///
    /// The shape is the original's, and the thing to know about it is that a
    /// monster is **passive**. It does not pick fights across the field: it
    /// minds you if you hit it, or if you walk within eight of it. An earlier
    /// version of this had them noticing at twenty and sprinting, which is
    /// how a player ends up being hit by something they never saw.
    pub fn think(&mut self, nearest: Option<(u16, (f32, f32))>, now: Instant) -> Turn {
        if !self.is_alive() {
            return Turn::default();
        }

        // Walking into one is enough to annoy it, but only just.
        if let Some((who, at)) = nearest {
            let already = matches!(self.doing, Doing::Chasing(_) | Doing::GoingHome);
            if !already && self.distance_to(at) <= AGGRO_RANGE {
                self.doing = Doing::Chasing(who);
            }
        }

        match self.doing {
            Doing::Chasing(who) => self.chase(who, nearest, now),
            Doing::GoingHome => self.go_home(now),
            Doing::Waiting | Doing::Patrolling => self.patrol(now),
        }
    }

    /// Marks it as having been hit, which is what makes a passive monster
    /// fight back.
    pub fn provoked_by(&mut self, who: u16) {
        if self.is_alive() && !matches!(self.doing, Doing::Chasing(id) if id == who) {
            self.doing = Doing::Chasing(who);
            self.last_hurt_by = Some(who);
        }
    }

    fn chase(
        &mut self,
        who: u16,
        nearest: Option<(u16, (f32, f32))>,
        now: Instant,
    ) -> Turn {
        let Some((_, at)) = nearest.filter(|(id, _)| *id == who) else {
            self.doing = Doing::GoingHome;
            return Turn::default();
        };

        // Led too far from where it started. A monster that follows forever
        // is one players learn to drag into the next zone.
        let (dx, dy) = (self.home.0 - at.0, self.home.1 - at.1);
        if (dx * dx + dy * dy).sqrt() > LEASH_RANGE {
            self.doing = Doing::GoingHome;
            return Turn::default();
        }

        if self.ready_at.is_some_and(|ready| now < ready) {
            return Turn::default();
        }

        if self.distance_to(at) > REACH {
            self.shift_towards(at, CHASE_STEP);
            self.ready_at = Some(now + TICK);
            return Turn::walking(self.position(), CHASE_SPEED);
        }

        // On top of them. Swinging runs on its own, much slower clock.
        self.ready_at = Some(now + ATTACK_EVERY);
        Turn {
            walk: None,
            speed: 0,
            attack: Some(Attack {
                attacker: self.id,
                target: who,
                damage: self.attack,
                skill: self.attack_skill(),
            }),
        }
    }

    fn go_home(&mut self, now: Instant) -> Turn {
        if self.ready_at.is_some_and(|ready| now < ready) {
            return Turn::default();
        }
        if self.arrived_at(self.home) {
            self.x = self.home.0;
            self.y = self.home.1;
            self.doing = Doing::Waiting;
            self.heading_away = true;
            self.ready_at = Some(now + self.home_wait);
            return Turn::walking(self.position(), CHASE_SPEED);
        }
        self.shift_towards(self.home, CHASE_STEP);
        self.ready_at = Some(now + TICK);
        Turn::walking(self.position(), CHASE_SPEED)
    }

    fn patrol(&mut self, now: Instant) -> Turn {
        if self.is_rooted() || self.ready_at.is_some_and(|ready| now < ready) {
            return Turn::default();
        }
        self.doing = Doing::Patrolling;

        let destination = self.destination();
        self.shift_towards(destination, PATROL_STEP);
        self.ready_at = Some(now + TICK);

        if self.arrived_at(destination) {
            self.x = destination.0;
            self.y = destination.1;
            self.steps = 0;
            self.heading_away = !self.heading_away;
            self.doing = Doing::Waiting;
            // The file says how long it stands there.
            self.ready_at = Some(now + if self.heading_away { self.home_wait } else { self.away_wait });
            return Turn::walking(self.position(), PATROL_SPEED);
        }

        // The client hears about every second step, not every one.
        self.steps += 1;
        if self.steps < STEPS_PER_PACKET {
            return Turn::default();
        }
        self.steps = 0;
        Turn::walking(self.position(), PATROL_SPEED)
    }
}

/// What a monster decided to do this turn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Turn {
    /// Where it now is, when the client should be told. The original sends
    /// where the monster *is*, having already shifted it, rather than a
    /// destination further off.
    pub walk: Option<(f32, f32)>,
    /// How fast to draw it getting there.
    pub speed: u8,
    pub attack: Option<Attack>,
}

impl Turn {
    fn walking(to: (f32, f32), speed: u8) -> Self {
        Self { walk: Some(to), speed, attack: None }
    }
}

/// How often the world thinks, which is what a step and a wait are measured
/// against.
pub const TICK: Duration = Duration::from_millis(500);

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

    // ---- thinking ---------------------------------------------------------

    /// A monster that ambles between two points twenty apart, standing at
    /// each end for a second.
    fn walker() -> Mob {
        const INFO: &str = "\
1,Andarilho,216,0,0,200,0,10,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,15,0,1";
        const LIST: &str = "\
1,1,1,1,Andarilho,Andarilho,0,0,0,100,100,11,1,0,120,100,11,1,0";
        let table = MobTable::parse(INFO, LIST).unwrap();
        place_all(&table).into_iter().next().unwrap()
    }

    /// Runs turns until something happens or the patience runs out, moving
    /// the clock on by a tick each time.
    fn run(mob: &mut Mob, nearest: Option<(u16, (f32, f32))>, turns: usize) -> Vec<Turn> {
        let mut now = Instant::now();
        let mut out = Vec::new();
        for _ in 0..turns {
            let turn = mob.think(nearest, now);
            if turn != Turn::default() {
                out.push(turn);
            }
            now += TICK;
        }
        out
    }

    /// The thing that makes the world liveable: a monster does not pick a
    /// fight with somebody walking past. It minds you at eight, which means
    /// walking into it, and the original checks even that from the player's
    /// side.
    #[test]
    fn a_monster_is_passive_until_you_come_close_or_hit_it() {
        // standing well clear of the patrol, so it never ambles into them
        let mut mob = walker();
        run(&mut mob, Some((7, (110.0, 140.0))), 40);
        assert!(
            !matches!(mob.doing, Doing::Chasing(_)),
            "it went after somebody forty away"
        );

        let mut mob = walker();
        mob.provoked_by(7);
        assert_eq!(mob.doing, Doing::Chasing(7), "hitting it did not annoy it");
    }

    #[test]
    fn somebody_who_walks_into_it_gets_chased() {
        let mut mob = walker();
        mob.think(Some((7, (105.0, 100.0))), Instant::now());
        assert_eq!(mob.doing, Doing::Chasing(7));
    }

    #[test]
    fn a_monster_with_nobody_about_ambles_between_its_two_points() {
        let mut mob = walker();
        let mut now = Instant::now();

        let mut seen_home = false;
        let mut seen_away = false;
        let mut biggest_step = 0.0f32;

        for _ in 0..400 {
            let was = mob.position();
            mob.think(None, now);
            now += TICK;

            biggest_step = biggest_step.max(mob.distance_to(was));
            assert!(
                (99.0..=121.0).contains(&mob.x),
                "it wandered off its patrol to {}",
                mob.x
            );
            seen_home |= (mob.x - 100.0).abs() < 0.01;
            seen_away |= (mob.x - 120.0).abs() < 0.01;
        }

        assert!(seen_away, "it never reached the far end");
        assert!(seen_home, "it never came back");
        assert!(
            biggest_step <= PATROL_STEP * 1.5,
            "it ambled {biggest_step} in one turn, which is a sprint"
        );
    }

    /// The kinds that stand still stand still.
    #[test]
    fn a_rooted_monster_never_moves() {
        let mut mob = walker();
        mob.away = mob.home;
        run(&mut mob, None, 20);
        assert_eq!(mob.position(), mob.home);
    }

    /// It stands at each end for as long as the spawn line says, rather than
    /// turning straight round.
    #[test]
    fn it_waits_at_each_end_for_as_long_as_the_file_says() {
        let mut mob = walker();
        assert_eq!(mob.home_wait, Duration::from_secs(1));

        let mut now = Instant::now();
        while !mob.arrived_at(mob.away) {
            mob.think(None, now);
            now += TICK;
        }

        // it has arrived, and now it stands there
        let standing = mob.position();
        mob.think(None, now);
        assert_eq!(mob.position(), standing, "it turned round without waiting");
    }

    #[test]
    fn a_chase_closes_the_distance_and_then_swings() {
        let mut mob = walker();
        let player = (106.0, 100.0);
        let mut now = Instant::now();

        let mut swung = None;
        for _ in 0..40 {
            if let Some(attack) = mob.think(Some((7, player)), now).attack {
                swung = Some(attack);
                break;
            }
            now += TICK;
        }

        let attack = swung.expect("it never swung");
        assert_eq!(attack.target, 7);
        assert_eq!(attack.attacker, mob.id);
        assert!(attack.damage > 0);
        assert!(
            mob.distance_to(player) <= REACH,
            "it swung from {} away", mob.distance_to(player)
        );
    }

    /// Dragging a monster across the map is the oldest trick there is. It
    /// lets go once *you* are twenty-five from where it started.
    #[test]
    fn a_monster_led_too_far_from_home_goes_back() {
        let mut mob = walker();
        mob.provoked_by(7);
        let mut now = Instant::now();

        let mut player = (105.0, 100.0);
        for _ in 0..60 {
            mob.think(Some((7, player)), now);
            player.0 += 3.0;
            now += TICK;
            if mob.doing == Doing::GoingHome {
                break;
            }
        }
        assert_eq!(mob.doing, Doing::GoingHome, "it followed forever");

        let mut got_home = false;
        for _ in 0..60 {
            mob.think(Some((7, player)), now);
            now += TICK;
            got_home |= mob.arrived_at(mob.home);
            if got_home {
                break;
            }
        }
        assert!(got_home, "it never got home");
        assert!(!matches!(mob.doing, Doing::Chasing(_)), "it went back to chasing");
    }

    #[test]
    fn a_dead_monster_does_nothing() {
        let mut mob = walker();
        let now = Instant::now();
        mob.wound(9999, now);

        assert_eq!(mob.think(Some((7, (101.0, 100.0))), now), Turn::default());
        assert_eq!(mob.position(), mob.home, "a corpse moved");
        mob.provoked_by(7);
        assert!(!matches!(mob.doing, Doing::Chasing(_)), "a corpse was annoyed");
    }

    /// Coming back has to clear what it was doing, or it carries on chasing
    /// somebody who left.
    #[test]
    fn coming_back_forgets_what_it_was_doing() {
        let mut mob = walker();
        let now = Instant::now();

        mob.provoked_by(7);
        assert_eq!(mob.doing, Doing::Chasing(7));

        mob.wound(9999, now);
        mob.revive(now + mob.respawn);
        assert_eq!(mob.doing, Doing::Waiting);
        assert_eq!(mob.last_hurt_by, None);
    }

    /// A swing is on a much slower clock than a step, so a monster does not
    /// empty somebody while they walk past.
    #[test]
    fn swings_are_slower_than_steps() {
        let mut mob = walker();
        let now = Instant::now();
        let player = (101.0, 100.0);

        assert!(mob.think(Some((7, player)), now).attack.is_some(), "the first swing");
        assert!(
            mob.think(Some((7, player)), now + TICK).attack.is_none(),
            "it swung again a tick later"
        );
        assert!(
            mob.think(Some((7, player)), now + ATTACK_EVERY).attack.is_some(),
            "it never swung again"
        );
    }

    /// The client hears about every second step, not every one, which is what
    /// keeps a patrol from being a packet storm.
    #[test]
    fn the_client_is_not_told_about_every_step() {
        let mut mob = walker();
        let turns = run(&mut mob, None, 8);
        assert!(turns.len() < 8, "every single step went out as a packet");
        assert!(!turns.is_empty(), "nothing went out at all");
        assert!(
            turns.iter().all(|t| t.speed == PATROL_SPEED),
            "an amble went out at the wrong speed"
        );
    }

    /// A monster swings with one of its own skills, which is where the
    /// animation the client plays comes from.
    #[test]
    fn a_blow_carries_the_skill_it_was_made_with() {
        let mut mob = walker();
        mob.skills = [0, 8216, 0, 0, 0];
        let now = Instant::now();

        let attack = mob
            .think(Some((7, (101.0, 100.0))), now)
            .attack
            .expect("it did not swing");
        assert_eq!(attack.skill, 8216, "the blow forgot which skill it was");

        mob.skills = [0; 5];
        mob.ready_at = None;
        let attack = mob.think(Some((7, (101.0, 100.0))), now).attack.unwrap();
        assert_eq!(attack.skill, 0, "a kind with no skills swings with none");
    }
}
