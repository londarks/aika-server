//! What is currently working on somebody.
//!
//! A buff is a skill that keeps running after it is cast. Drinking a potion,
//! sitting on a mount and casting a blessing all end up in the same place:
//! `AddBuff(skill)` (`Mob/BaseMob.pas:3779`), which remembers when it started
//! and lets the skill's own duration decide when it stops.
//!
//! # Two numbers, easily confused
//!
//! The original keeps `_buffs` keyed by the **skill id**, and asks about it by
//! the skill's **family** — the field it calls `Index`, the first word of the
//! skill record. Several skills share a family: every rank of one blessing,
//! and every saddle that puts a rider on a horse, carry the same one. So
//! "is this player mounted" is `BuffExistsByIndex(163)`, a question about
//! families, while "add this buff" names one exact skill.
//!
//! Keeping that split is what makes the mount work: the saddle names skill
//! 7259, whose family is 163, and the mount's own skills refuse to fire unless
//! a buff of family 163 is running.
//!
//! # Nothing expires on a timer
//!
//! The original never runs a clock over these. `RefreshBuffs` walks the list
//! and drops what has run out, and it is called when the list is about to be
//! looked at or sent. This does the same: ask, and what has expired is gone.

use aika_data::skills::SkillTable;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// How many the packet can carry (`TSendBuffsPacket`, forty of them).
pub const MAX_BUFFS: usize = 40;

/// The family of the buff that means "this player is on a mount"
/// (`BuffExistsByIndex(163)` in `UseMountSkill`).
pub const FAMILY_MOUNTED: u32 = 163;

/// A duration the table writes as all ones, which is how it says "this does
/// not run out on its own". The mount buff is the one that matters: a rider
/// stays on the horse until they get off, not until a clock runs down.
///
/// The original adds it to the start time all the same, lands a hundred and
/// thirty-six years out, and truncates that into the packet's four bytes --
/// which wraps, and tells the client the buff ended some time in the past.
/// We saturate instead. Sending a time that has already gone is the one
/// answer that is certainly wrong.
pub const FOREVER: u32 = u32::MAX;

/// The buffs on one character: which skill, and when it started.
#[derive(Debug, Clone, Default)]
pub struct Buffs {
    started: HashMap<usize, SystemTime>,
}

impl Buffs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a buff, or restarts one already running.
    ///
    /// A skill of a family that is already on replaces it rather than stacking
    /// beside it, which is the original removing the old one before adding the
    /// new. Refuses once the list is full, as `AddBuff` does at sixty — we
    /// stop at forty, because that is all the packet can say.
    pub fn add(&mut self, skills: &SkillTable, skill: usize, now: SystemTime) -> bool {
        let Some(def) = skills.get(skill) else {
            return false;
        };
        // A skill with no duration is not a buff at all; remembering it would
        // leave something that never runs out.
        if def.duration_secs() == 0 {
            return false;
        }

        let family = def.family();
        self.started.retain(|&id, _| skills.get(id).is_none_or(|s| s.family() != family));

        if self.started.len() >= MAX_BUFFS {
            return false;
        }
        self.started.insert(skill, now);
        true
    }

    /// Whether a buff of this family is running.
    pub fn has_family(&self, skills: &SkillTable, family: u32) -> bool {
        self.started
            .keys()
            .any(|&id| skills.get(id).is_some_and(|s| s.family() == family))
    }

    /// Drops whatever has run out, and says how many went. The original sends
    /// fresh health, status and points whenever this is more than nothing.
    pub fn expire(&mut self, skills: &SkillTable, now: SystemTime) -> usize {
        let before = self.started.len();
        self.started.retain(|&id, &mut started| match skills.get(id) {
            Some(def) => match ends_at(started, def.duration_secs()) {
                // One that does not run out stays until it is taken off.
                None => true,
                Some(end) => end > now,
            },
            // A skill the table does not have cannot be timed, so it goes.
            None => false,
        });
        before - self.started.len()
    }

    /// Every buff still running, as the skill and the moment it ends, which
    /// is `None` for one that does not. Sorted, so the packet is the same
    /// twice in a row for the same state.
    pub fn running(&self, skills: &SkillTable) -> Vec<(usize, Option<SystemTime>)> {
        let mut out: Vec<_> = self
            .started
            .iter()
            .filter_map(|(&id, &started)| {
                let def = skills.get(id)?;
                Some((id, ends_at(started, def.duration_secs())))
            })
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out.truncate(MAX_BUFFS);
        out
    }

    pub fn is_empty(&self) -> bool {
        self.started.is_empty()
    }

    /// Takes one off by family, which is how the original ends a mount: the
    /// player is dismounted, not the saddle unlearned.
    pub fn remove_family(&mut self, skills: &SkillTable, family: u32) -> bool {
        let before = self.started.len();
        self.started.retain(|&id, _| skills.get(id).is_none_or(|s| s.family() != family));
        before != self.started.len()
    }
}

/// When a buff started now would end, or `None` for one that does not.
fn ends_at(started: SystemTime, duration_secs: u32) -> Option<SystemTime> {
    if duration_secs == FOREVER {
        return None;
    }
    Some(started + Duration::from_secs(duration_secs as u64))
}

/// Unix seconds, which is what the packet carries (`DateTimeToUnix`), with
/// all ones for a buff that does not run out.
pub fn unix(at: Option<SystemTime>) -> u32 {
    let Some(at) = at else { return FOREVER };
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs().min(FOREVER as u64) as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aika_data::skills::{field, RECORD_SIZE, SLOTS};

    const MOUNT_SKILL: usize = 7259;
    const OTHER_MOUNT_SKILL: usize = 7260;
    const POTION_SKILL: usize = 9031;
    const INSTANT: usize = 100;

    /// Two saddles of the same family, a potion of another, and one skill with
    /// no duration at all.
    fn skills() -> SkillTable {
        let mut raw = vec![0u8; SLOTS * RECORD_SIZE + 4];
        let mut define = |id: usize, family: u32, seconds: u32| {
            let r = &mut raw[id * RECORD_SIZE..(id + 1) * RECORD_SIZE];
            r[field::FAMILY..field::FAMILY + 4].copy_from_slice(&family.to_le_bytes());
            r[field::DURATION..field::DURATION + 4].copy_from_slice(&seconds.to_le_bytes());
        };
        define(MOUNT_SKILL, FAMILY_MOUNTED, 600);
        define(OTHER_MOUNT_SKILL, FAMILY_MOUNTED, 900);
        define(POTION_SKILL, 383, 10_800);
        define(INSTANT, 42, 0);
        SkillTable::decode(&raw).expect("the fixture table is malformed")
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
    }

    /// The mount is asked about by family, not by which saddle was used.
    #[test]
    fn a_saddle_makes_the_player_mounted() {
        let (skills, mut buffs) = (skills(), Buffs::new());
        assert!(!buffs.has_family(&skills, FAMILY_MOUNTED));

        assert!(buffs.add(&skills, MOUNT_SKILL, at(0)));
        assert!(buffs.has_family(&skills, FAMILY_MOUNTED), "the rider is not mounted");
    }

    /// A second saddle replaces the first rather than sitting beside it: the
    /// original removes a buff of the same family before adding one.
    #[test]
    fn a_second_saddle_replaces_the_first() {
        let (skills, mut buffs) = (skills(), Buffs::new());
        buffs.add(&skills, MOUNT_SKILL, at(0));
        buffs.add(&skills, OTHER_MOUNT_SKILL, at(10));

        let running = buffs.running(&skills);
        assert_eq!(running.len(), 1, "the player is mounted twice over");
        assert_eq!(running[0].0, OTHER_MOUNT_SKILL);
    }

    /// A skill that does not last is not a buff, and remembering one would
    /// leave something running for ever.
    #[test]
    fn a_skill_with_no_duration_is_not_a_buff() {
        let (skills, mut buffs) = (skills(), Buffs::new());
        assert!(!buffs.add(&skills, INSTANT, at(0)));
        assert!(buffs.is_empty());
    }

    /// Nothing runs a clock: the list is cleaned when it is asked about.
    #[test]
    fn a_buff_runs_out_after_its_duration() {
        let (skills, mut buffs) = (skills(), Buffs::new());
        buffs.add(&skills, MOUNT_SKILL, at(0));

        assert_eq!(buffs.expire(&skills, at(599)), 0, "it went early");
        assert!(buffs.has_family(&skills, FAMILY_MOUNTED));

        assert_eq!(buffs.expire(&skills, at(601)), 1, "it outlasted its ten minutes");
        assert!(!buffs.has_family(&skills, FAMILY_MOUNTED));
    }

    /// The end time the packet carries is the start plus the duration.
    #[test]
    fn the_end_time_is_the_start_plus_the_duration() {
        let (skills, mut buffs) = (skills(), Buffs::new());
        buffs.add(&skills, POTION_SKILL, at(0));

        let running = buffs.running(&skills);
        assert_eq!(running[0].0, POTION_SKILL);
        assert_eq!(unix(running[0].1), unix(Some(at(10_800))), "three hours, as the potion says");
    }

    /// The real mount buff is written as lasting for ever, and this is the
    /// case that matters: a rider who is thrown off after a moment, or told
    /// their buff ended in the past, is the bug the wrapping would cause.
    #[test]
    fn a_buff_written_as_lasting_for_ever_never_runs_out() {
        let mut raw = vec![0u8; SLOTS * RECORD_SIZE + 4];
        let r = &mut raw[MOUNT_SKILL * RECORD_SIZE..(MOUNT_SKILL + 1) * RECORD_SIZE];
        r[field::FAMILY..field::FAMILY + 4].copy_from_slice(&FAMILY_MOUNTED.to_le_bytes());
        r[field::DURATION..field::DURATION + 4].copy_from_slice(&FOREVER.to_le_bytes());
        let skills = SkillTable::decode(&raw).unwrap();

        let mut buffs = Buffs::new();
        assert!(buffs.add(&skills, MOUNT_SKILL, at(0)), "a mount buff was refused");

        // A century later it is still on.
        assert_eq!(buffs.expire(&skills, at(3_000_000_000)), 0, "the rider fell off");
        assert!(buffs.has_family(&skills, FAMILY_MOUNTED));

        let running = buffs.running(&skills);
        assert_eq!(running[0].1, None, "it was given an end after all");
        assert_eq!(unix(running[0].1), FOREVER, "the client would read a time in the past");
    }

    /// Getting off is by family too.
    #[test]
    fn dismounting_takes_the_buff_off() {
        let (skills, mut buffs) = (skills(), Buffs::new());
        buffs.add(&skills, MOUNT_SKILL, at(0));

        assert!(buffs.remove_family(&skills, FAMILY_MOUNTED));
        assert!(buffs.is_empty());
        assert!(!buffs.remove_family(&skills, FAMILY_MOUNTED), "it went twice");
    }
}
