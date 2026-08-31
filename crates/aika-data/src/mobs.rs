//! Monsters: what kinds exist, and where each one stands.
//!
//! Two comma-separated files, both under `Data/Mobs` in the original:
//!
//! - `AllMobsInfo.csv` — one line per *kind* of monster: its model, health,
//!   level, experience and how long it takes to come back.
//! - `MonsterListCSV.csv` — one line per *spawn point*: which kind, where it
//!   starts, and where it walks to. A kind appears once here for every copy
//!   of it in the world, which is why there are five thousand lines and only
//!   three hundred kinds.
//!
//! The two are joined by name, and the name is spelled differently in each:
//! `AllMobsInfo` writes `Max Filhote` and `MonsterListCSV` writes
//! `Max_Filhote`. The original replaces spaces with underscores before
//! comparing (`ServerSocket.pas:742`), and so does this.
//!
//! Column numbers are the ones the original reads, not a reading of the
//! header — there is no header. `AllMobsInfo` is parsed at
//! `ServerSocket.pas:750` and `MonsterListCSV` at `ServerSocket.pas:614`.

use std::collections::HashMap;
use std::path::Path;

/// Monsters live above the NPCs in the shared client id space: players get
/// 1 to 2000, NPCs 2048 to 3047, and every monster in the world is numbered
/// from here (`Count + 3048` in `ServerSocket.pas:620`).
pub const FIRST_MOB_ID: u16 = 3048;

/// What the original falls back to when a kind has no health in the file.
pub const DEFAULT_HP: u32 = 3500;

/// Columns of `AllMobsInfo.csv`.
mod info {
    pub const NAME_INDEX: usize = 0;
    pub const NAME: usize = 1;
    pub const MODEL: usize = 2;
    pub const MODEL_2: usize = 3;
    pub const MODEL_3: usize = 4;
    pub const HP: usize = 5;
    pub const ROTATION: usize = 6;
    pub const LEVEL: usize = 7;
    pub const HEIGHT: usize = 8;
    pub const HEAD: usize = 9;
    pub const LEG: usize = 10;
    pub const MOB_TYPE: usize = 11;
    pub const SPAWN_TYPE: usize = 12;
    pub const IS_SERVICE: usize = 13;
    pub const RESPAWN_SECONDS: usize = 18;
    pub const SKILLS: [usize; 5] = [19, 20, 21, 22, 23];
    pub const EXPERIENCE: usize = 24;
    pub const DROP_INDEX: usize = 25;
    pub const ACTIVE: usize = 26;
    pub const COLUMNS: usize = 27;
}

/// Columns of `MonsterListCSV.csv`.
mod spawn {
    pub const NAME: usize = 4;
    pub const START_X: usize = 9;
    pub const START_Y: usize = 10;
    pub const START_RANGE: usize = 11;
    pub const START_WAIT: usize = 12;
    pub const END_X: usize = 14;
    pub const END_Y: usize = 15;
    pub const END_RANGE: usize = 16;
    pub const END_WAIT: usize = 17;
    pub const COLUMNS: usize = 18;
}

/// Two kinds of monster stand still wherever they are put. The original
/// decides by looking for these words in the name (`ServerSocket.pas:620`),
/// which is as fragile as it sounds, but it is what the data was built for.
const ROOTED_NAMES: [&str; 2] = ["Mutante", "Crenon"];

/// A kind of monster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobKind {
    /// Index into the client's string table, where the displayed name lives.
    pub name_index: u16,
    /// The name as `AllMobsInfo.csv` spells it, spaces and all.
    pub name: String,
    /// What the client draws. Only the first is set in the shipped file.
    pub model: [u16; 3],
    pub hp: u32,
    pub level: u16,
    /// Height, head and leg, the same three the character record carries.
    pub sizes: [u8; 3],
    pub rotation: u16,
    pub mob_type: u16,
    pub spawn_type: u8,
    pub is_service: bool,
    pub respawn_seconds: u32,
    pub skills: [u16; 5],
    pub experience: u32,
    pub drop_index: u16,
    /// Kinds marked inactive are in the file but not in the world.
    pub active: bool,
}

impl MobKind {
    /// The name with spaces turned into underscores, which is how the spawn
    /// file spells it and therefore how the two are matched.
    pub fn key(&self) -> String {
        underscored(&self.name)
    }

    /// Whether this kind stays where it is put.
    pub fn is_rooted(&self) -> bool {
        ROOTED_NAMES.iter().any(|word| self.name.contains(word))
    }
}

/// One monster in the world: a kind, and the two points it walks between.
#[derive(Debug, Clone, PartialEq)]
pub struct MobSpawn {
    /// The kind's name, underscored, as the spawn file spells it.
    pub kind: String,
    pub start: (f32, f32),
    pub end: (f32, f32),
    /// How close something has to come before the monster reacts, at each end.
    pub start_range: u16,
    pub end_range: u16,
    /// How long it waits at each end before walking back, in seconds.
    pub start_wait: u16,
    pub end_wait: u16,
}

impl MobSpawn {
    /// A monster that does not walk has both ends in the same place.
    pub fn is_stationary(&self) -> bool {
        self.start == self.end
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MobError {
    /// A line with fewer columns than the file is supposed to have.
    ShortLine { line: usize, columns: usize, wanted: usize },
    /// A column that should hold a number and does not.
    NotANumber { line: usize, column: usize, text: String },
}

impl std::fmt::Display for MobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MobError::ShortLine { line, columns, wanted } => {
                write!(f, "line {line} has {columns} columns, not {wanted}")
            }
            MobError::NotANumber { line, column, text } => {
                write!(f, "line {line} column {column} is {text:?}, not a number")
            }
        }
    }
}

impl std::error::Error for MobError {}

/// Everything about monsters, with the two files already joined.
#[derive(Debug, Default)]
pub struct MobTable {
    kinds: HashMap<String, MobKind>,
    spawns: Vec<MobSpawn>,
    /// Spawn lines naming a kind that `AllMobsInfo.csv` does not describe.
    /// The original skips these silently; we count them, because a monster
    /// missing from the world is not something to find out from a player.
    pub orphans: Vec<String>,
}

impl MobTable {
    /// Reads both files out of one directory.
    ///
    /// They are latin-1, like every other file the pack ships: the monster
    /// names carry Portuguese accents and reading them as UTF-8 fails on the
    /// first one.
    pub fn load_dir(dir: impl AsRef<Path>) -> std::io::Result<Result<Self, MobError>> {
        let dir = dir.as_ref();
        let info = latin1(&std::fs::read(dir.join("AllMobsInfo.csv"))?);
        let list = latin1(&std::fs::read(dir.join("MonsterListCSV.csv"))?);
        Ok(Self::parse(&info, &list))
    }

    pub fn parse(info: &str, list: &str) -> Result<Self, MobError> {
        let kinds = parse_kinds(info)?;
        let spawns = parse_spawns(list)?;

        let mut orphans: Vec<String> = spawns
            .iter()
            .filter(|s| !kinds.contains_key(&s.kind))
            .map(|s| s.kind.clone())
            .collect();
        orphans.sort();
        orphans.dedup();

        Ok(Self { kinds, spawns, orphans })
    }

    pub fn kinds(&self) -> impl Iterator<Item = &MobKind> {
        self.kinds.values()
    }

    pub fn kind(&self, key: &str) -> Option<&MobKind> {
        self.kinds.get(key)
    }

    /// Every spawn point, in file order. The index is what the client id is
    /// counted from.
    pub fn spawns(&self) -> &[MobSpawn] {
        &self.spawns
    }

    /// Spawn points whose kind is known and marked active, paired with it,
    /// and numbered the way the original numbers them.
    ///
    /// The id counts every line in the file, not every *placed* monster, so
    /// skipping an inactive kind leaves a gap rather than shifting everything
    /// after it. That matters: the ids have to match what the client is told
    /// about, and a shift would rename every monster in the world.
    pub fn placed(&self) -> impl Iterator<Item = (u16, &MobKind, &MobSpawn)> {
        self.spawns.iter().enumerate().filter_map(move |(i, spawn)| {
            let kind = self.kinds.get(&spawn.kind)?;
            if !kind.active {
                return None;
            }
            let id = FIRST_MOB_ID.checked_add(u16::try_from(i).ok()?)?;
            Some((id, kind, spawn))
        })
    }

    pub fn len(&self) -> usize {
        self.spawns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty()
    }
}

/// The pack's text files are latin-1 throughout.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// The original replaces spaces with underscores before matching a name.
fn underscored(name: &str) -> String {
    name.replace(' ', "_")
}

fn parse_kinds(text: &str) -> Result<HashMap<String, MobKind>, MobError> {
    let mut kinds = HashMap::new();

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < info::COLUMNS {
            return Err(MobError::ShortLine {
                line: line_number + 1,
                columns: f.len(),
                wanted: info::COLUMNS,
            });
        }

        let n = |column: usize| number(&f, column, line_number + 1);
        let hp = n(info::HP)?;

        let kind = MobKind {
            name_index: n(info::NAME_INDEX)? as u16,
            name: f[info::NAME].to_string(),
            model: [
                n(info::MODEL)? as u16,
                n(info::MODEL_2)? as u16,
                n(info::MODEL_3)? as u16,
            ],
            // A kind with no health in the file gets the same fallback the
            // original gives it, rather than being born dead.
            hp: if hp == 0 { DEFAULT_HP } else { hp },
            level: n(info::LEVEL)? as u16,
            sizes: [n(info::HEIGHT)? as u8, n(info::HEAD)? as u8, n(info::LEG)? as u8],
            rotation: n(info::ROTATION)? as u16,
            mob_type: n(info::MOB_TYPE)? as u16,
            spawn_type: n(info::SPAWN_TYPE)? as u8,
            is_service: n(info::IS_SERVICE)? != 0,
            respawn_seconds: n(info::RESPAWN_SECONDS)?,
            skills: [
                n(info::SKILLS[0])? as u16,
                n(info::SKILLS[1])? as u16,
                n(info::SKILLS[2])? as u16,
                n(info::SKILLS[3])? as u16,
                n(info::SKILLS[4])? as u16,
            ],
            experience: n(info::EXPERIENCE)?,
            drop_index: n(info::DROP_INDEX)? as u16,
            active: n(info::ACTIVE)? != 0,
        };
        kinds.insert(kind.key(), kind);
    }
    Ok(kinds)
}

fn parse_spawns(text: &str) -> Result<Vec<MobSpawn>, MobError> {
    let mut spawns = Vec::new();

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < spawn::COLUMNS {
            return Err(MobError::ShortLine {
                line: line_number + 1,
                columns: f.len(),
                wanted: spawn::COLUMNS,
            });
        }

        let n = |column: usize| number(&f, column, line_number + 1);
        let kind = underscored(f[spawn::NAME]);

        // The two that stand still are given the same point at both ends, so
        // the walking code needs no special case for them.
        let start = (n(spawn::START_X)? as f32, n(spawn::START_Y)? as f32);
        let rooted = ROOTED_NAMES.iter().any(|word| kind.contains(word));
        let end = if rooted {
            start
        } else {
            (n(spawn::END_X)? as f32, n(spawn::END_Y)? as f32)
        };

        spawns.push(MobSpawn {
            kind,
            start,
            end,
            start_range: n(spawn::START_RANGE)? as u16,
            end_range: n(spawn::END_RANGE)? as u16,
            start_wait: n(spawn::START_WAIT)? as u16,
            end_wait: n(spawn::END_WAIT)? as u16,
        });
    }
    Ok(spawns)
}

/// Reads one column as a number. The files hold whole numbers written as
/// text, and a few carry a decimal point, so a float is parsed and truncated
/// rather than refusing the line.
fn number(fields: &[&str], column: usize, line: usize) -> Result<u32, MobError> {
    let text = fields.get(column).copied().unwrap_or("");
    if text.is_empty() {
        return Ok(0);
    }
    text.parse::<f64>()
        .map(|v| v.max(0.0) as u32)
        .map_err(|_| MobError::NotANumber { line, column, text: text.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two lines taken from the shipped files, unchanged.
    const INFO: &str = "\
1,Max Filhote,216,0,0,207,134,1,7,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,15,0,1
2,Max Domesticado,216,0,0,228,179,2,8,119,119,1025,0,0,0,0,0,0,45,0,0,0,0,0,25,0,1
9,Mutante Teste,300,0,0,0,0,9,7,119,119,1025,0,0,0,0,0,0,60,0,0,0,0,0,90,0,1
7,Dorminhoco,111,0,0,50,0,3,7,119,119,1025,0,0,0,0,0,0,30,0,0,0,0,0,10,0,0";

    const LIST: &str = "\
1,1,1,1,Max_Filhote,Max_Filhote,0,0,0,3496,844,11,8,0,3474,831,11,8,0
1,1,1,1,Max_Domesticado,Max_Domesticado,0,0,0,3502,862,11,8,0,3494,852,11,8,0
1,1,1,1,Mutante_Teste,Mutante_Teste,0,0,0,4000,900,11,8,0,4100,950,11,8,0
1,1,1,1,Dorminhoco,Dorminhoco,0,0,0,100,100,11,8,0,110,110,11,8,0
1,1,1,1,Fantasma,Fantasma,0,0,0,200,200,11,8,0,210,210,11,8,0";

    fn table() -> MobTable {
        MobTable::parse(INFO, LIST).expect("the sample files do not parse")
    }

    #[test]
    fn reads_a_kind_the_way_the_original_reads_it() {
        let t = table();
        let cub = t.kind("Max_Filhote").expect("no Max Filhote");

        assert_eq!(cub.name, "Max Filhote");
        assert_eq!(cub.name_index, 1);
        assert_eq!(cub.model[0], 216, "the model the client draws");
        assert_eq!(cub.hp, 207);
        assert_eq!(cub.level, 1);
        assert_eq!(cub.sizes, [7, 119, 119]);
        assert_eq!(cub.experience, 15);
        assert_eq!(cub.respawn_seconds, 45);
        assert!(cub.active);
    }

    /// The two files spell the same monster differently, and joining them is
    /// the whole point of this module.
    #[test]
    fn the_two_files_are_joined_on_the_underscored_name() {
        let t = table();
        assert_eq!(t.kind("Max_Filhote").unwrap().name, "Max Filhote");
        assert!(t.kind("Max Filhote").is_none(), "the key is the underscored form");
    }

    /// A kind with no health in the file would otherwise be born dead.
    #[test]
    fn a_kind_with_no_health_gets_the_fallback() {
        let t = table();
        assert_eq!(t.kind("Mutante_Teste").unwrap().hp, DEFAULT_HP);
    }

    #[test]
    fn spawn_points_carry_both_ends_of_the_walk() {
        let t = table();
        let first = &t.spawns()[0];

        assert_eq!(first.kind, "Max_Filhote");
        assert_eq!(first.start, (3496.0, 844.0));
        assert_eq!(first.end, (3474.0, 831.0));
        assert_eq!((first.start_range, first.start_wait), (11, 8));
        assert!(!first.is_stationary());
    }

    /// The mutants do not walk. The original decides that by looking for the
    /// word in the name, and pins both ends of the walk to the same point.
    #[test]
    fn a_mutant_stays_where_it_is_put() {
        let t = table();
        let mutant = t.spawns().iter().find(|s| s.kind.contains("Mutante")).unwrap();

        assert!(mutant.is_stationary(), "a mutant was given somewhere to walk to");
        assert_eq!(mutant.start, (4000.0, 900.0));
        assert_eq!(mutant.end, mutant.start, "the file's second point is ignored");
    }

    /// Ids are counted from the file, not from what ends up in the world, so
    /// an inactive kind leaves a gap. Renumbering would rename every monster
    /// after it.
    #[test]
    fn ids_count_lines_and_a_skipped_one_leaves_a_gap() {
        let t = table();
        let placed: Vec<u16> = t.placed().map(|(id, _, _)| id).collect();

        assert_eq!(
            placed,
            vec![FIRST_MOB_ID, FIRST_MOB_ID + 1, FIRST_MOB_ID + 2],
            "the sleeper is inactive and the ghost has no kind, and neither \
             may shift the others"
        );
    }

    #[test]
    fn a_spawn_naming_a_kind_that_does_not_exist_is_reported() {
        let t = table();
        assert_eq!(t.orphans, vec!["Fantasma".to_string()]);
    }

    #[test]
    fn a_short_line_says_which_one_and_how_short() {
        let err = MobTable::parse("1,Max Filhote,216", LIST).unwrap_err();
        assert_eq!(
            err,
            MobError::ShortLine { line: 1, columns: 3, wanted: info::COLUMNS }
        );
    }

    #[test]
    fn a_column_that_should_be_a_number_and_is_not_says_so() {
        let bad = INFO.replace("216,0,0,207", "216,0,0,muito");
        let err = MobTable::parse(&bad, LIST).unwrap_err();
        assert!(matches!(err, MobError::NotANumber { line: 1, .. }), "got {err}");
    }

    /// The real files are not in this repository. When they are present the
    /// parser is held to what they contain.
    #[test]
    fn reads_the_original_files_when_they_are_available() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/mobs");
        if !dir.join("AllMobsInfo.csv").is_file() {
            return;
        }

        let table = MobTable::load_dir(&dir).unwrap().unwrap();
        assert!(table.kinds().count() > 300, "only {} kinds", table.kinds().count());
        assert!(table.len() > 5000, "only {} spawn points", table.len());

        for (_, kind, spawn) in table.placed() {
            assert!(kind.hp > 0, "{} has no health", kind.name);
            assert!(spawn.start.0 > 0.0 && spawn.start.1 > 0.0, "{} is at the origin", kind.name);
        }
    }
}
