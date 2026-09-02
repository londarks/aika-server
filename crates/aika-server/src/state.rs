//! State shared by the token, login and game services.

use crate::config::Config;
use crate::db::Database;
use crate::store::{Account, AccountStore, Character};
use crate::world::World;
use aika_data::itemlist::ItemList;
use aika_data::mobs::MobTable;
use aika_data::npc::NpcSet;
use aika_data::skills::SkillTable;
use aika_data::drops::DropTable;
use aika_data::exp::ExpTable;
use aika_data::template::Template;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub struct State {
    pub cfg: Config,
    pub store: AccountStore,
    /// Who is online and where, shared by every game connection.
    pub world: World,
    /// What everything costs and what it does. Empty when no table was
    /// configured, which makes every lookup miss rather than panic.
    pub items: ItemList,
    /// What a companion's level costs. See [`crate::pran::ExpCurve`].
    pub pran_levels: crate::pran::ExpCurve,
    /// Every skill at every rank. Empty when none was configured, which
    /// refuses every cast rather than panicking.
    pub skills: SkillTable,
    /// What a character of each class is born as, in class order. A missing
    /// one leaves that class playable but naked.
    pub templates: [Option<Template>; 6],
    /// What each level costs. Empty means nobody gains one.
    pub levels: ExpTable,
    /// What monsters leave behind. Empty means they leave nothing.
    pub drops: DropTable,
    /// Where the world is kept between runs. Unit tests that only build
    /// packets leave it out; every server that people log into has one.
    db: Option<Database>,
    started: Instant,
}

impl State {
    /// Opens the database and takes the accounts from it. The configuration
    /// seeds the very first run, so a fresh checkout still has somewhere to
    /// log in, and is ignored from then on: the database is the truth.
    pub async fn open(cfg: Config) -> anyhow::Result<Self> {
        let connection = cfg.database.connection();
        let db = Database::open(&connection).await?;
        // Never the raw string: it can carry a password, and a log line is
        // the first thing pasted into a chat when something goes wrong.
        let where_it_is = crate::db::redacted(&connection);

        if db.account_count().await? == 0 {
            let seeded = db.seed(&cfg.accounts).await?;
            info!(database = %where_it_is, accounts = seeded, "seeded a new database");
        }

        let accounts = db.load_accounts().await?;
        info!(database = %where_it_is, accounts = accounts.len(), "database ready");

        let store = AccountStore::from_accounts(
            accounts,
            cfg.login.max_attempts,
            Duration::from_secs(cfg.login.block_minutes * 60),
        )?;
        let world = World::with_npcs(load_npcs(&cfg.game.npc_dir))
            .with_mobs(crate::mob::place_all(&load_mobs(&cfg.game.mob_dir)));
        let items = load_items(&cfg.game.item_list);
        let skills = load_skills(&cfg.game.skill_data);
        let templates = load_templates(&cfg.game.template_dir);
        let levels = load_levels(&cfg.game.exp_list);
        let pran_levels = load_pran_levels(&cfg.game.pran_exp_list);
        let drops = load_drops(&cfg.game.drop_dir);

        // Needs both halves, which is why it is here and not in the
        // migration: the fix for one row is in the item table, and the
        // database does not have one.
        match db.repair_durability(&items).await {
            Ok(0) => {}
            Ok(fixed) => info!(items = fixed, "items given the durability they were stored without"),
            Err(e) => warn!(error = %e, "could not repair item durability"),
        }

        Ok(Self {
            cfg, store, world, items, skills, templates, levels, drops, pran_levels,
            db: Some(db), started: Instant::now(),
        })
    }

    /// State with no database behind it, for tests about packets rather than
    /// persistence. Accounts come straight from the configuration.
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let store = AccountStore::from_dev_accounts(
            &cfg.accounts,
            cfg.login.max_attempts,
            Duration::from_secs(cfg.login.block_minutes * 60),
        )?;
        let world = World::with_npcs(load_npcs(&cfg.game.npc_dir))
            .with_mobs(crate::mob::place_all(&load_mobs(&cfg.game.mob_dir)));
        let items = load_items(&cfg.game.item_list);
        let skills = load_skills(&cfg.game.skill_data);
        let templates = load_templates(&cfg.game.template_dir);
        let levels = load_levels(&cfg.game.exp_list);
        let pran_levels = load_pran_levels(&cfg.game.pran_exp_list);
        let drops = load_drops(&cfg.game.drop_dir);
        Ok(Self {
            cfg, store, world, items, skills, templates, levels, drops, pran_levels,
            db: None, started: Instant::now(),
        })
    }

    /// The template for a class, counted from one the way the client counts.
    pub fn template(&self, class_number: u16) -> Option<&Template> {
        self.templates.get(class_number.checked_sub(1)? as usize)?.as_ref()
    }

    pub fn db(&self) -> Option<&Database> {
        self.db.as_ref()
    }

    /// Writes back everything a session changed: where the character stopped,
    /// what it is carrying and how much gold it has. Goes to the database and
    /// to the copy the login screen reads, so a second login in the same run
    /// sees the same thing a restart would.
    ///
    /// A failure is logged rather than returned: this runs while a connection
    /// is being torn down, and there is nobody left to tell.
    pub async fn save_session(&self, character: &Character) {
        self.store.update_character(character);

        let Some(db) = &self.db else { return };
        if let Err(e) = db.save_session(character).await {
            warn!(character = %character.name, error = %e, "could not save the session");
        }
    }

    /// Writes the chest, which belongs to the account rather than to the
    /// character and so is saved beside the session rather than inside it.
    /// Whether a companion of this name already exists.
    ///
    /// With no database behind it nothing is taken, which suits the tests:
    /// they are about the rule and not about the storage.
    pub async fn pran_name_taken(&self, name: &str) -> bool {
        let Some(db) = &self.db else { return false };
        match db.pran_name_taken(name).await {
            Ok(taken) => taken,
            Err(e) => {
                warn!(name, error = %e, "could not check the pran name, allowing it");
                false
            }
        }
    }

    pub async fn save_storage(&self, account: &Account) {
        self.store.update_account(account);

        let Some(db) = &self.db else { return };
        if let Err(e) =
            db.save_storage(account.id as i64, account.storage_gold, &account.storage).await
        {
            warn!(account = %account.username, error = %e, "could not save the chest");
        }

        // The companions hang off the account the same way the chest does, so
        // they are written where it is written.
        for pran in &account.prans {
            if let Err(e) = db.save_pran(account.id as i64, pran).await {
                warn!(account = %account.username, error = %e, "could not save a pran");
            }
        }
    }

    /// Equivalent of the `timeGetTime` the Delphi server stamps on packets:
    /// milliseconds since the process started, truncated to 32 bits.
    pub fn uptime_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    /// How long a change may sit in memory before it is written.
    pub fn autosave_every(&self) -> Duration {
        Duration::from_secs(self.cfg.database.autosave_secs)
    }

    pub fn token_ttl(&self) -> Duration {
        Duration::from_secs(self.cfg.login.token_ttl_secs)
    }
}

/// Reads the townspeople, if a directory was configured.
///
/// A missing or unreadable directory is a warning, not a failure: a server
/// with no NPCs still lets people log in and walk around, and refusing to
/// start would be a worse trade.
fn load_npcs(dir: &str) -> Vec<aika_data::npc::Npc> {
    if dir.is_empty() {
        return Vec::new();
    }

    let set = match NpcSet::load_dir(dir) {
        Ok(set) => set,
        Err(e) => {
            warn!(dir, error = %e, "could not read the npc directory; the world will be empty");
            return Vec::new();
        }
    };

    for (file, why) in &set.rejected {
        warn!(file, reason = %why, "npc not loaded");
    }
    info!(dir, npcs = set.len(), "npcs loaded");

    set.iter().cloned().collect()
}

/// Reads the item table, if one was configured.
///
/// Like the NPCs, a missing table is a warning rather than a refusal to
/// start: everything except buying and selling works without it.
fn load_items(path: &str) -> ItemList {
    if path.is_empty() {
        return ItemList::default();
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(path, error = %e, "could not read the item table; shops will be empty");
            return ItemList::default();
        }
    };

    match ItemList::decode(&bytes) {
        Ok(list) => {
            info!(path, ids = list.len(), defined = list.defined().count(), "item table loaded");
            list
        }
        Err(e) => {
            warn!(path, error = %e, "the item table is malformed; shops will be empty");
            ItemList::default()
        }
    }
}

/// Reads the monster tables, if a directory was configured.
///
/// A world with nothing to fight still lets people log in and walk around, so
/// a missing directory is a warning rather than a refusal to start.
fn load_mobs(dir: &str) -> MobTable {
    if dir.is_empty() {
        return MobTable::default();
    }

    let table = match MobTable::load_dir(dir) {
        Ok(Ok(table)) => table,
        Ok(Err(e)) => {
            warn!(dir, error = %e, "the monster tables are malformed; the world will be empty");
            return MobTable::default();
        }
        Err(e) => {
            warn!(dir, error = %e, "could not read the monster tables; the world will be empty");
            return MobTable::default();
        }
    };

    if !table.orphans.is_empty() {
        warn!(
            dir,
            kinds = table.orphans.len(),
            first = %table.orphans.first().map(String::as_str).unwrap_or(""),
            "spawn points name monsters that have no entry in AllMobsInfo.csv"
        );
    }
    info!(
        dir,
        kinds = table.kinds().count(),
        points = table.len(),
        placed = table.placed().count(),
        "monsters loaded"
    );
    table
}

/// Reads the skill table, if one was configured.
///
/// Like everything else the pack ships, a missing file is a warning: a server
/// where nobody can cast is still a server people can walk around.
fn load_skills(path: &str) -> SkillTable {
    if path.is_empty() {
        return SkillTable::default();
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(path, error = %e, "could not read the skill table; nobody will cast");
            return SkillTable::default();
        }
    };

    match SkillTable::decode(&bytes) {
        Ok(table) => {
            info!(path, slots = table.len(), skills = table.defined().count(), "skills loaded");
            table
        }
        Err(e) => {
            warn!(path, error = %e, "the skill table is malformed; nobody will cast");
            SkillTable::default()
        }
    }
}

/// Reads the six character templates, if a directory was configured.
fn load_templates(dir: &str) -> [Option<Template>; 6] {
    if dir.is_empty() {
        return Default::default();
    }

    let loaded = aika_data::template::load_all(dir);
    for (i, template) in loaded.iter().enumerate() {
        if template.is_none() {
            warn!(
                dir,
                class = aika_data::template::CLASS_FILES[i],
                "no template; that class will start naked"
            );
        }
    }
    info!(dir, classes = loaded.iter().filter(|t| t.is_some()).count(), "templates loaded");
    loaded
}

/// Reads the experience curve, if one was configured.
/// The companion's curve: plain little-endian dwords, one per level.
///
/// Nothing levels a pran without it, which is a quiet failure rather than
/// a loud one -- so it says so.
fn load_pran_levels(path: &str) -> crate::pran::ExpCurve {
    if path.is_empty() {
        return crate::pran::ExpCurve::default();
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            let curve = crate::pran::ExpCurve::decode(&bytes);
            info!(path, levels = curve.levels(), "pran experience curve loaded");
            curve
        }
        Err(e) => {
            warn!(path, error = %e, "no pran curve, so no companion will ever grow");
            crate::pran::ExpCurve::default()
        }
    }
}

fn load_levels(path: &str) -> ExpTable {
    if path.is_empty() {
        return ExpTable::default();
    }
    match std::fs::read(path).map(|b| ExpTable::decode(&b)) {
        Ok(Ok(table)) => {
            info!(path, levels = table.max_level(), "experience curve loaded");
            table
        }
        Ok(Err(e)) => {
            warn!(path, error = %e, "the experience curve is malformed; nobody will level");
            ExpTable::default()
        }
        Err(e) => {
            warn!(path, error = %e, "could not read the experience curve");
            ExpTable::default()
        }
    }
}

/// Reads the drop tables, if a directory was configured.
fn load_drops(dir: &str) -> DropTable {
    if dir.is_empty() {
        return DropTable::default();
    }
    let table = DropTable::load_dir(dir);
    if table.is_empty() {
        warn!(dir, "no drop tables were read; monsters will leave nothing");
    } else {
        info!(dir, drops = table.len(), "drop tables loaded");
    }
    table
}
