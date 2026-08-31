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

/// How close something has to come before a monster minds.
pub const AGGRO_RANGE: f32 = 20.0;
/// How far it will chase before giving up and walking home.
pub const CHASE_RANGE: f32 = 60.0;
/// How close it has to be to swing.
pub const REACH: f32 = 12.0;
/// How often it swings.
pub const ATTACK_EVERY: Duration = Duration::from_millis(1500);
/// The speed the client is told to walk a monster at.
///
/// `TBaseMob.WalkTo` defaults to 70 (`Mob/BaseMob.pas:331`), and the number
/// is not decoration: the client walks the monster from where it is to where
/// it is told at this speed, and the server sends the *destination* rather
/// than a step. Sending a short step every second instead makes the monster
/// arrive early and stand still until the next one, which reads as
/// teleporting.
pub const WALK_SPEED: u8 = 70;

/// How far a monster will commit to in one go.
///
/// `WalkAvanced` refuses to send a walk longer than 18 (`BaseMob.pas:1985`).
/// Chasing keeps to that so the monster keeps up with a player who turns,
/// rather than committing to where they were.
pub const MAX_WALK: f32 = 18.0;

/// How long a walk of a given distance takes, which is how long the monster
/// has nothing to decide.
pub fn walk_time(distance: f32) -> Duration {
    Duration::from_secs_f32((distance / WALK_SPEED as f32).clamp(0.05, 4.0))
}

/// What a monster is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    /// Standing at one end of its patrol, waiting to walk back.
    Waiting,
    /// Walking to the other end.
    Patrolling,
    /// Chasing somebody.
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
            x: spawn.start.0,
            y: spawn.start.1,
            respawn: Duration::from_secs(kind.respawn_seconds.max(1) as u64),
            dead_until: None,
            doing: Doing::Waiting,
            heading_away: true,
            ready_at: None,
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

    /// Commits to walking somewhere, at most `MAX_WALK` of the way.
    ///
    /// The position moves to the destination straight away and the client is
    /// told to walk there, which is what the original does: the server keeps
    /// where the monster *will be* and the client animates getting there.
    /// Returns where it committed to, and how long the walk takes.
    fn walk_towards(&mut self, to: (f32, f32), now: Instant) -> ((f32, f32), Duration) {
        let (dx, dy) = (to.0 - self.x, to.1 - self.y);
        let distance = (dx * dx + dy * dy).sqrt();

        let target = if distance <= MAX_WALK || distance == 0.0 {
            to
        } else {
            (self.x + dx / distance * MAX_WALK, self.y + dy / distance * MAX_WALK)
        };

        let travelled = self.distance_to(target);
        self.x = target.0;
        self.y = target.1;

        let time = walk_time(travelled);
        self.ready_at = Some(now + time);
        (target, time)
    }

    /// How far it is from a point.
    pub fn distance_to(&self, to: (f32, f32)) -> f32 {
        let (dx, dy) = (to.0 - self.x, to.1 - self.y);
        (dx * dx + dy * dy).sqrt()
    }

    /// Whether it may act, and marks it as having acted.
    fn take_turn(&mut self, now: Instant, wait: Duration) -> bool {
        if self.ready_at.is_some_and(|at| now < at) {
            return false;
        }
        self.ready_at = Some(now + wait);
        true
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
                self.last_hurt_by = None;
                true
            }
            _ => false,
        }
    }

    pub fn position(&self) -> (f32, f32) {
        (self.x, self.y)
    }

    /// One turn of thinking, given where the nearest player is.
    ///
    /// `nearest` is the closest player and how far away it is, or `None` when
    /// there is nobody about. Returns what to do to that player, if anything.
    ///
    /// The rules are the small ones every monster in every game of this shape
    /// has: mind somebody who comes close, chase them, swing when in reach,
    /// give up when they get too far from where you started, walk home, then
    /// go back to pacing. Keeping it in one function keeps the whole
    /// behaviour readable in one place.
    pub fn think(&mut self, nearest: Option<(u16, (f32, f32))>, now: Instant) -> Turn {
        if !self.is_alive() {
            return Turn::default();
        }

        // Somebody close enough is worth minding, unless we are already on
        // our way home, which is when a monster ignores the world.
        if let Some((who, at)) = nearest {
            let close = self.distance_to(at) <= AGGRO_RANGE;
            let chasing_them = matches!(self.doing, Doing::Chasing(id) if id == who);
            if close && !matches!(self.doing, Doing::GoingHome) && !chasing_them {
                self.doing = Doing::Chasing(who);
            }
        }

        match self.doing {
            Doing::Chasing(who) => self.chase(who, nearest, now),
            Doing::GoingHome => {
                if self.ready_at.is_some_and(|at| now < at) {
                    return Turn::default();
                }
                let (to, _) = self.walk_towards(self.home, now);
                if to == self.home {
                    self.doing = Doing::Waiting;
                    self.heading_away = true;
                }
                Turn { walk: Some(to), attack: None }
            }
            Doing::Waiting | Doing::Patrolling => self.patrol(now),
        }
    }

    fn chase(
        &mut self,
        who: u16,
        nearest: Option<(u16, (f32, f32))>,
        now: Instant,
    ) -> Turn {
        // Whoever it was chasing is gone, or somebody else is now the nearest.
        let Some((_, at)) = nearest.filter(|(id, _)| *id == who) else {
            self.doing = Doing::GoingHome;
            return Turn::default();
        };

        // Chased too far from home. A monster that follows forever ends up
        // in the next zone, and players learn to drag them there.
        let from_home = {
            let (dx, dy) = (self.home.0 - self.x, self.home.1 - self.y);
            (dx * dx + dy * dy).sqrt()
        };
        if from_home > CHASE_RANGE {
            self.doing = Doing::GoingHome;
            return Turn::default();
        }

        if self.ready_at.is_some_and(|now_at| now < now_at) {
            return Turn::default();
        }

        if self.distance_to(at) > REACH {
            let (to, _) = self.walk_towards(at, now);
            return Turn { walk: Some(to), attack: None };
        }

        // In reach. Swinging is on its own, slower clock than walking.
        self.ready_at = Some(now + ATTACK_EVERY);
        Turn {
            walk: None,
            attack: Some(Attack {
                attacker: self.id,
                target: who,
                damage: self.attack,
                skill: self.attack_skill(),
            }),
        }
    }

    fn patrol(&mut self, now: Instant) -> Turn {
        if self.is_rooted() || self.ready_at.is_some_and(|at| now < at) {
            return Turn::default();
        }
        self.doing = Doing::Patrolling;

        let destination = self.destination();
        let (to, _) = self.walk_towards(destination, now);
        if to == destination {
            self.heading_away = !self.heading_away;
            self.doing = Doing::Waiting;
        }
        Turn { walk: Some(to), attack: None }
    }
}

/// What a monster decided to do this turn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Turn {
    /// Where it set off for, if it moved. The client is told this and walks
    /// it there itself.
    pub walk: Option<(f32, f32)>,
    pub attack: Option<Attack>,
}

/// How often the world thinks, which is what a step and a wait are measured
/// against.
pub const TICK: Duration = Duration::from_secs(1);

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

    /// A monster that walks between two points twenty apart.
    fn walker() -> Mob {
        const INFO: &str = "\
1,Andarilho,216,0,0,200,0,10,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,15,0,1";
        const LIST: &str = "\
1,1,1,1,Andarilho,Andarilho,0,0,0,100,100,11,8,0,120,100,11,8,0";
        let table = MobTable::parse(INFO, LIST).unwrap();
        place_all(&table).into_iter().next().unwrap()
    }

    #[test]
    fn a_monster_with_nobody_about_paces_between_its_two_points() {
        let mut mob = walker();
        let mut now = Instant::now();

        assert_eq!(mob.position(), (100.0, 100.0));

        // over enough ticks it should stand at both ends and never outside
        // them, which is what pacing is
        let mut seen_home = false;
        let mut seen_away = false;
        for _ in 0..12 {
            mob.think(None, now);
            now += TICK;
            assert!(
                (100.0..=120.0).contains(&mob.x),
                "it walked past the end of its patrol to {}",
                mob.x
            );
            seen_home |= mob.x == 100.0;
            seen_away |= mob.x == 120.0;
        }
        assert!(seen_away, "it never reached the far end");
        assert!(seen_home, "it never came back");
    }

    /// The kinds that stand still stand still.
    #[test]
    fn a_rooted_monster_never_moves() {
        let mut mob = walker();
        mob.away = mob.home;
        let mut now = Instant::now();

        for _ in 0..10 {
            mob.think(None, now);
            now += TICK;
        }
        assert_eq!(mob.position(), mob.home);
    }

    #[test]
    fn somebody_who_comes_close_gets_chased() {
        let mut mob = walker();
        let now = Instant::now();

        mob.think(Some((7, (110.0, 100.0))), now);
        assert_eq!(mob.doing, Doing::Chasing(7));
    }

    /// Standing far enough away is safe, which is what makes the aggro range
    /// mean anything.
    #[test]
    fn somebody_far_off_is_ignored() {
        let mut mob = walker();
        mob.think(Some((7, (500.0, 500.0))), Instant::now());
        assert!(!matches!(mob.doing, Doing::Chasing(_)));
    }

    #[test]
    fn a_chase_closes_the_distance_and_then_swings() {
        let mut mob = walker();
        let mut now = Instant::now();
        let player = (130.0, 100.0);

        let mut swung = None;
        for _ in 0..20 {
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
        assert!(mob.distance_to(player) <= REACH, "it swung from out of reach");
    }

    /// Dragging a monster across the map is the oldest trick there is. It
    /// gives up and walks home.
    #[test]
    fn a_monster_chased_too_far_from_home_goes_back() {
        let mut mob = walker();
        let mut now = Instant::now();

        // a player walking steadily away, faster than the monster
        let mut player = (110.0, 100.0);
        for _ in 0..40 {
            mob.think(Some((7, player)), now);
            player.0 += 30.0;
            now += TICK;
            if mob.doing == Doing::GoingHome {
                break;
            }
        }
        assert_eq!(mob.doing, Doing::GoingHome, "it followed forever");

        // and it gets there, ignoring the player on the way, after which it
        // goes back to pacing
        let mut got_home = false;
        for _ in 0..20 {
            mob.think(Some((7, player)), now);
            now += TICK;
            got_home |= mob.position() == mob.home;
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

        assert_eq!(mob.think(Some((7, (100.0, 100.0))), now), Turn::default());
        assert_eq!(mob.position(), mob.home, "a corpse moved");
    }

    /// Coming back has to clear what it was doing, or it carries on chasing
    /// somebody who left.
    #[test]
    fn coming_back_forgets_what_it_was_doing() {
        let mut mob = walker();
        let now = Instant::now();

        mob.think(Some((7, (110.0, 100.0))), now);
        assert_eq!(mob.doing, Doing::Chasing(7));

        mob.wound(9999, now);
        mob.revive(now + mob.respawn);
        assert_eq!(mob.doing, Doing::Waiting);
        assert_eq!(mob.last_hurt_by, None);
    }

    /// A swing is on a slower clock than a step, so a monster does not empty
    /// somebody in one tick.
    #[test]
    fn swings_are_slower_than_steps() {
        let mut mob = walker();
        let now = Instant::now();
        let player = (105.0, 100.0);

        assert!(mob.think(Some((7, player)), now).attack.is_some(), "the first swing");
        assert!(
            mob.think(Some((7, player)), now).attack.is_none(),
            "it swung twice in the same instant"
        );
        assert!(
            mob.think(Some((7, player)), now + ATTACK_EVERY).attack.is_some(),
            "it never swung again"
        );
    }

    /// A walk goes out as a destination, not as a step, and the server puts
    /// the monster there straight away. Sending short steps every tick is
    /// what made monsters look like they were teleporting: the client walked
    /// them there, then they stood still until the next packet.
    #[test]
    fn a_walk_commits_to_a_destination_and_takes_time() {
        let mut mob = walker();
        let now = Instant::now();

        let turn = mob.think(None, now);
        let to = turn.walk.expect("it did not set off");

        assert_eq!(mob.position(), to, "the server did not move it to where it is going");
        assert!(mob.ready_at.is_some_and(|at| at > now), "it can decide again immediately");

        // and it decides nothing more until it would have arrived
        assert_eq!(mob.think(None, now), Turn::default(), "it decided again mid-walk");
    }

    /// A chase never commits further than the original allows, so a monster
    /// keeps up with somebody who turns rather than running to where they
    /// used to be.
    #[test]
    fn a_chase_never_commits_further_than_the_original_allows() {
        let mut mob = walker();
        let now = Instant::now();

        let turn = mob.think(Some((7, (400.0, 100.0))), now);
        let to = turn.walk.expect("it did not give chase");

        let committed = ((to.0 - 100.0f32).powi(2) + (to.1 - 100.0f32).powi(2)).sqrt();
        assert!(committed <= MAX_WALK + 0.01, "it committed {committed} in one go");
    }

    /// A longer walk takes longer, which is what stops a monster deciding
    /// again before it has got anywhere.
    #[test]
    fn a_walk_takes_as_long_as_it_is_far() {
        assert!(walk_time(70.0) > walk_time(10.0));
        assert!(walk_time(0.0) >= Duration::from_millis(50), "a walk is never instant");
        assert!(walk_time(100_000.0) <= Duration::from_secs(4), "and never forever");
    }

    /// A monster swings with one of its own skills, which is where the
    /// animation the client plays comes from.
    #[test]
    fn a_blow_carries_the_skill_it_was_made_with() {
        let mut mob = walker();
        mob.skills = [0, 8216, 0, 0, 0];
        let now = Instant::now();

        let attack = mob
            .think(Some((7, (105.0, 100.0))), now)
            .attack
            .expect("it did not swing");
        assert_eq!(attack.skill, 8216, "the blow forgot which skill it was");

        mob.skills = [0; 5];
        mob.ready_at = None;
        let attack = mob.think(Some((7, (105.0, 100.0))), now).attack.unwrap();
        assert_eq!(attack.skill, 0, "a kind with no skills swings with none");
    }
}
