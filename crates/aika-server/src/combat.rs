//! Hitting things.
//!
//! ```text
//! client -> 0x302  { target, _, animation, skill, my position, target position }
//! server -> 0x102  { skill, attacker position, attacker, animation, attacker
//!                    health, target, damage kind, damage, target health,
//!                    where it fell }
//! ```
//!
//! One packet in, one packet out, and the one going out is sent to everyone
//! who can see the fight rather than only to whoever swung: a fight nobody
//! else can see is a fight that looks like it never happened.
//!
//! The damage is provisional and says so. The original's `GetDamage`
//! (`Mob/BaseMob.pas:4820`) reads attack and defence off the character, the
//! weapon and the skill, then runs them through critical, block, immune and
//! miss tables — thirty-three outcomes in `TDamageType` alone. None of those
//! inputs exist here yet: no stats from equipment, no `SkillData.bin`, no
//! resistances. What is here is a formula that makes a level-appropriate
//! monster take a sensible number of hits, so the rest of the loop — dying,
//! paying out, coming back — can be built and tested against something.

use rand::Rng;

/// `TSendAtkPacket`.
pub const OP_ATTACK: u16 = 0x302;
/// `TRecvDamagePacket`.
pub const OP_DAMAGE: u16 = 0x102;

/// Header, then 72 bytes of body.
pub const DAMAGE_SIZE: usize = 12 + 72;

/// How close you have to be to hit something. The original checks the
/// distance against the skill's range; a plain swing gets this.
pub const MELEE_RANGE: f32 = 15.0;

/// `TDamageType` is an enum of thirty-three outcomes. Only the first two are
/// produced here; the rest need resistances, blocking and immunity.
pub const DAMAGE_NORMAL: u8 = 0;
pub const DAMAGE_CRITICAL: u8 = 1;

/// How often a swing lands for extra. A placeholder for the real critical
/// chance, which comes off the character's luck and its weapon.
const CRITICAL_CHANCE: f64 = 0.15;
const CRITICAL_MULTIPLIER: u32 = 2;

/// `0x302`: what the client says it is hitting.
///
/// The record declares fourteen spare bytes between the target and the
/// animation, and unlike the login and creation packets they really are on
/// the wire — the body is 36 bytes and the fields land where the record says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attack {
    pub target: u16,
    pub animation: u16,
    pub skill: u16,
    pub from: (f32, f32),
    pub at: (f32, f32),
}

impl Attack {
    pub const BODY_SIZE: usize = 36;

    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < Self::BODY_SIZE {
            return None;
        }
        let f32_at = |i: usize| f32::from_le_bytes(body[i..i + 4].try_into().unwrap());
        Some(Self {
            target: u16::from_le_bytes(body[0..2].try_into().ok()?),
            animation: u16::from_le_bytes(body[16..18].try_into().ok()?),
            skill: u16::from_le_bytes(body[18..20].try_into().ok()?),
            from: (f32_at(20), f32_at(24)),
            at: (f32_at(28), f32_at(32)),
        })
    }

    pub fn to_body(self) -> Vec<u8> {
        let mut body = vec![0u8; Self::BODY_SIZE];
        body[0..2].copy_from_slice(&self.target.to_le_bytes());
        body[16..18].copy_from_slice(&self.animation.to_le_bytes());
        body[18..20].copy_from_slice(&self.skill.to_le_bytes());
        body[20..24].copy_from_slice(&self.from.0.to_le_bytes());
        body[24..28].copy_from_slice(&self.from.1.to_le_bytes());
        body[28..32].copy_from_slice(&self.at.0.to_le_bytes());
        body[32..36].copy_from_slice(&self.at.1.to_le_bytes());
        body
    }
}

/// What one swing did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blow {
    pub damage: u32,
    pub kind: u8,
}

impl Blow {
    pub fn is_critical(&self) -> bool {
        self.kind == DAMAGE_CRITICAL
    }
}

/// What a swing takes off, before the target's health is touched.
///
/// **Provisional.** See the module note: the real formula needs stats this
/// server does not collect yet. This one grows with the attacker's level,
/// shrinks against a tougher target, and never falls to nothing — a hit that
/// does no damage reads as a broken server rather than a hard fight.
pub fn swing(attacker_level: u16, target_level: u16, rng: &mut impl Rng) -> Blow {
    swing_with(attacker_level, target_level, 0, rng)
}

/// The same, with a skill's own damage as the floor.
///
/// A spell that the table says hits for 260 has to hit for at least about
/// that, or ranks stop meaning anything; the level still matters on top.
pub fn swing_with(
    attacker_level: u16,
    target_level: u16,
    skill_damage: u32,
    rng: &mut impl Rng,
) -> Blow {
    let base = 10 + attacker_level as i32 * 4 + skill_damage as i32;
    // A monster ten levels above you is hard; one ten below is not free.
    let gap = (attacker_level as i32 - target_level as i32).clamp(-20, 20);
    let scaled = (base + gap * 3).max(1) as u32;

    // A spread either side, so two hits on the same thing differ.
    let spread = (scaled / 5).max(1);
    let rolled = rng.gen_range(scaled.saturating_sub(spread)..=scaled + spread);

    if rng.gen_bool(CRITICAL_CHANCE) {
        Blow { damage: rolled * CRITICAL_MULTIPLIER, kind: DAMAGE_CRITICAL }
    } else {
        Blow { damage: rolled.max(1), kind: DAMAGE_NORMAL }
    }
}

/// Offsets inside the `0x102` body.
pub mod damage_offset {
    pub const SKILL: usize = 0;
    pub const ATTACKER_X: usize = 4;
    pub const ATTACKER_Y: usize = 8;
    pub const ATTACKER: usize = 16;
    pub const ANIMATION: usize = 19;
    pub const ATTACKER_HP: usize = 32;
    pub const TARGET: usize = 44;
    pub const DAMAGE_KIND: usize = 46;
    pub const TARGET_ANIMATION: usize = 47;
    pub const DAMAGE: usize = 48;
    pub const TARGET_HP: usize = 60;
    pub const DEATH_X: usize = 64;
    pub const DEATH_Y: usize = 68;
    pub const BODY_SIZE: usize = 72;
}

/// Everything one `0x102` has to say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Damage {
    pub skill: u16,
    pub attacker: u16,
    pub attacker_at: (f32, f32),
    pub attacker_hp: u32,
    /// What the attacker plays, from the skill's `Anim`.
    pub animation: u16,
    /// What the target plays — the flinch — from the skill's
    /// `TargetAnimation`. Leaving this at zero is why a blow used to land in
    /// silence with nobody reacting.
    pub target_animation: u8,
    pub target: u16,
    pub target_hp: u32,
    pub blow: Blow,
    /// Where the target fell, or where it stands if it is still up.
    pub at: (f32, f32),
}

impl Damage {
    pub fn to_body(&self) -> Vec<u8> {
        use damage_offset as off;
        let mut body = vec![0u8; off::BODY_SIZE];

        let put32 = |b: &mut Vec<u8>, at: usize, v: u32| {
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put_f32 = |b: &mut Vec<u8>, at: usize, v: f32| {
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };

        put32(&mut body, off::SKILL, self.skill as u32);
        put_f32(&mut body, off::ATTACKER_X, self.attacker_at.0);
        put_f32(&mut body, off::ATTACKER_Y, self.attacker_at.1);
        body[off::ATTACKER..off::ATTACKER + 2].copy_from_slice(&self.attacker.to_le_bytes());
        body[off::ANIMATION] = self.animation as u8;
        put32(&mut body, off::ATTACKER_HP, self.attacker_hp);
        body[off::TARGET..off::TARGET + 2].copy_from_slice(&self.target.to_le_bytes());
        body[off::DAMAGE_KIND] = self.blow.kind;
        body[off::TARGET_ANIMATION] = self.target_animation;

        // The damage field is 64 bits wide in the record.
        body[off::DAMAGE..off::DAMAGE + 8].copy_from_slice(&(self.blow.damage as u64).to_le_bytes());
        put32(&mut body, off::TARGET_HP, self.target_hp);
        put_f32(&mut body, off::DEATH_X, self.at.0);
        put_f32(&mut body, off::DEATH_Y, self.at.1);
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(7)
    }

    #[test]
    fn attack_body_roundtrip() {
        let original = Attack {
            target: 3048,
            animation: 2,
            skill: 0,
            from: (3450.0, 690.0),
            at: (3460.0, 700.0),
        };
        assert_eq!(Attack::parse(&original.to_body()), Some(original));
        assert_eq!(Attack::parse(&[0u8; 20]), None);
    }

    /// The size the record declares, and what the client sends.
    #[test]
    fn the_damage_packet_is_the_size_the_record_says() {
        let damage = Damage {
            skill: 0,
            attacker: 1,
            attacker_at: (0.0, 0.0),
            attacker_hp: 100,
            animation: 2,
            target_animation: 26,
            target: 3048,
            target_hp: 50,
            blow: Blow { damage: 42, kind: DAMAGE_NORMAL },
            at: (10.0, 20.0),
        };
        assert_eq!(damage.to_body().len() + 12, DAMAGE_SIZE);
    }

    #[test]
    fn the_damage_packet_carries_who_hit_what_for_how_much() {
        use damage_offset as off;
        let damage = Damage {
            skill: 0,
            attacker: 7,
            attacker_at: (3450.0, 690.0),
            attacker_hp: 520,
            animation: 2,
            target_animation: 26,
            target: 3048,
            target_hp: 158,
            blow: Blow { damage: 42, kind: DAMAGE_CRITICAL },
            at: (3460.0, 700.0),
        };
        let body = damage.to_body();

        assert_eq!(u16::from_le_bytes(body[off::ATTACKER..off::ATTACKER + 2].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes(body[off::TARGET..off::TARGET + 2].try_into().unwrap()), 3048);
        assert_eq!(
            u64::from_le_bytes(body[off::DAMAGE..off::DAMAGE + 8].try_into().unwrap()),
            42
        );
        assert_eq!(
            u32::from_le_bytes(body[off::TARGET_HP..off::TARGET_HP + 4].try_into().unwrap()),
            158
        );
        assert_eq!(body[off::DAMAGE_KIND], DAMAGE_CRITICAL);
        assert_eq!(
            body[off::TARGET_ANIMATION],
            26,
            "the flinch is what makes a blow look like it landed"
        );
        assert_eq!(
            f32::from_le_bytes(body[off::ATTACKER_X..off::ATTACKER_X + 4].try_into().unwrap()),
            3450.0
        );
    }

    /// A hit that takes nothing off reads as a broken server, not a hard
    /// fight, however far above you the target is.
    #[test]
    fn a_swing_always_takes_something_off() {
        let mut rng = rng();
        for attacker in [1u16, 10, 50] {
            for target in [1u16, 50, 200] {
                let blow = swing(attacker, target, &mut rng);
                assert!(blow.damage >= 1, "level {attacker} against {target} did nothing");
            }
        }
    }

    #[test]
    fn a_higher_level_hits_harder() {
        let mut rng = rng();
        let low: u32 = (0..200).map(|_| swing(5, 20, &mut rng).damage).sum();
        let high: u32 = (0..200).map(|_| swing(40, 20, &mut rng).damage).sum();
        assert!(high > low, "level 40 did not out-hit level 5: {high} against {low}");
    }

    /// The same fight against a tougher monster has to take longer.
    #[test]
    fn a_tougher_target_takes_less_damage() {
        let mut rng = rng();
        let weak: u32 = (0..200).map(|_| swing(20, 5, &mut rng).damage).sum();
        let strong: u32 = (0..200).map(|_| swing(20, 60, &mut rng).damage).sum();
        assert!(strong < weak, "the tougher one took more: {strong} against {weak}");
    }

    /// Two swings must not be identical, or a fight looks like a loop.
    #[test]
    fn swings_vary() {
        let mut rng = rng();
        let rolls: std::collections::HashSet<u32> =
            (0..40).map(|_| swing(30, 30, &mut rng).damage).collect();
        assert!(rolls.len() > 5, "only {} distinct rolls in forty swings", rolls.len());
    }

    #[test]
    fn a_critical_hits_harder_than_a_normal_one() {
        let mut rng = rng();
        let mut normal = 0u32;
        let mut critical = 0u32;
        for _ in 0..500 {
            let blow = swing(30, 30, &mut rng);
            if blow.is_critical() {
                critical = critical.max(blow.damage);
            } else {
                normal = normal.max(blow.damage);
            }
        }
        assert!(critical > normal, "a critical was not worth more");
    }
}
