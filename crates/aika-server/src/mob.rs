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
        }
    }

    pub fn is_alive(&self) -> bool {
        self.dead_until.is_none()
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
                true
            }
            _ => false,
        }
    }

    pub fn position(&self) -> (f32, f32) {
        (self.x, self.y)
    }
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
}
