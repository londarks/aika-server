//! Persistence.
//!
//! SQLite while developing, MySQL in production, which is why the schema and
//! the queries stay inside the subset both understand: only `INTEGER`, `TEXT`
//! and `BLOB`, timestamps as integer unix seconds rather than a date type, no
//! `INSERT OR REPLACE` (SQLite only) and no `REPLACE INTO` (MySQL only). Every
//! query lives in this module so the day the driver changes there is one file
//! to read.
//!
//! The original server's MySQL dump is in `sql/schema.sql` as documentation of
//! which fields the game needs. This is not a copy of it: it holds what our
//! server actually uses, and grows as features land.

use crate::config::DevAccount;
use crate::inventory::Inventory;
use crate::store::{Account, Character, Item, DEFAULT_SIZES, DEFAULT_SPEED_MOVE};
use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Item containers, matching the `TypeSlot` the protocol uses.
pub const CONTAINER_EQUIP: i64 = 0;
pub const CONTAINER_INVENTORY: i64 = 1;
pub const CONTAINER_STORAGE: i64 = 2;

/// The whole schema. Written for SQLite; the only lines that need a MySQL
/// spelling are the primary keys, since `AUTOINCREMENT` there is
/// `AUTO_INCREMENT` on an `INT`.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    username       TEXT    NOT NULL UNIQUE,
    password_hash  TEXT    NOT NULL,
    nation         INTEGER NOT NULL DEFAULT 0,
    account_status INTEGER NOT NULL DEFAULT 0,
    ban_days       INTEGER NOT NULL DEFAULT 0,
    storage_gold   INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS characters (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id   INTEGER NOT NULL,
    slot         INTEGER NOT NULL,
    name         TEXT    NOT NULL UNIQUE,
    nation       INTEGER NOT NULL DEFAULT 0,
    class_index  INTEGER NOT NULL,
    hair         INTEGER NOT NULL,
    level        INTEGER NOT NULL DEFAULT 1,
    exp          INTEGER NOT NULL DEFAULT 0,
    gold         INTEGER NOT NULL DEFAULT 0,
    x            INTEGER NOT NULL,
    y            INTEGER NOT NULL,
    speed_move   INTEGER NOT NULL,
    height       INTEGER NOT NULL,
    torso        INTEGER NOT NULL,
    legs         INTEGER NOT NULL,
    body         INTEGER NOT NULL,
    strength     INTEGER NOT NULL DEFAULT 0,
    agility      INTEGER NOT NULL DEFAULT 0,
    intellect    INTEGER NOT NULL DEFAULT 0,
    constitution INTEGER NOT NULL DEFAULT 0,
    luck         INTEGER NOT NULL DEFAULT 0,
    free_points  INTEGER NOT NULL DEFAULT 0,
    skill_list   BLOB,
    item_bar     BLOB,
    skill_points INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    deleted_at   INTEGER,
    UNIQUE (account_id, slot)
);

CREATE INDEX IF NOT EXISTS characters_by_account ON characters (account_id);

CREATE TABLE IF NOT EXISTS items (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id   INTEGER NOT NULL,
    container      INTEGER NOT NULL,
    slot           INTEGER NOT NULL,
    item_index     INTEGER NOT NULL,
    appearance     INTEGER NOT NULL DEFAULT 0,
    identific      INTEGER NOT NULL DEFAULT 0,
    effect1_index  INTEGER NOT NULL DEFAULT 0,
    effect2_index  INTEGER NOT NULL DEFAULT 0,
    effect3_index  INTEGER NOT NULL DEFAULT 0,
    effect1_value  INTEGER NOT NULL DEFAULT 0,
    effect2_value  INTEGER NOT NULL DEFAULT 0,
    effect3_value  INTEGER NOT NULL DEFAULT 0,
    durability_min INTEGER NOT NULL DEFAULT 0,
    durability_max INTEGER NOT NULL DEFAULT 0,
    refine         INTEGER NOT NULL DEFAULT 0,
    expires_at     INTEGER NOT NULL DEFAULT 0,
    UNIQUE (character_id, container, slot)
);

CREATE INDEX IF NOT EXISTS items_by_character ON items (character_id);

-- The chest. Same columns as `items`, but owned by the account rather than by
-- a character, because that is what it is for: handing something from one of
-- your characters to another.
CREATE TABLE IF NOT EXISTS storage_items (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id     INTEGER NOT NULL,
    slot           INTEGER NOT NULL,
    item_index     INTEGER NOT NULL,
    appearance     INTEGER NOT NULL DEFAULT 0,
    identific      INTEGER NOT NULL DEFAULT 0,
    effect1_index  INTEGER NOT NULL DEFAULT 0,
    effect2_index  INTEGER NOT NULL DEFAULT 0,
    effect3_index  INTEGER NOT NULL DEFAULT 0,
    effect1_value  INTEGER NOT NULL DEFAULT 0,
    effect2_value  INTEGER NOT NULL DEFAULT 0,
    effect3_value  INTEGER NOT NULL DEFAULT 0,
    durability_min INTEGER NOT NULL DEFAULT 0,
    durability_max INTEGER NOT NULL DEFAULT 0,
    refine         INTEGER NOT NULL DEFAULT 0,
    expires_at     INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, slot)
);

CREATE INDEX IF NOT EXISTS storage_by_account ON storage_items (account_id);
"#;

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Opens the database file, creating it and the schema if needed.
    pub async fn open(path: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(path)
            .with_context(|| format!("reading the database path {path}"))?
            .create_if_missing(true)
            // Characters are saved as players disconnect, so a crash should
            // cost at most the last write rather than the file.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .with_context(|| format!("opening the database at {path}"))?;

        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        for statement in SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .with_context(|| format!("applying schema: {}", statement.trim()))?;
        }

        // Columns added after the first release. A database made before them
        // has the rest of the schema already, so `CREATE TABLE IF NOT EXISTS`
        // leaves it untouched and these fill the gap. Re-running them on a
        // database that already has the column is a "duplicate column" error,
        // which is the one error we swallow — both dialects raise it, and it
        // means the work is already done.
        // The class tier is the one column that cannot simply default. A
        // character made before it levelled with nothing stopping it, so its
        // level is the only record of how far it got, and defaulting it to 1
        // would hand a level 99 a cap of 50. Seeded only on the run that adds
        // the column: doing it every start would demote anybody promoted at
        // the wall itself, whose level still reads as the tier below.
        let tier_column_is_new = self.add_column("characters", &format!(
            "class_tier INTEGER NOT NULL DEFAULT {}",
            crate::promotion::FIRST_TIER
        )).await?;
        if tier_column_is_new {
            for tier in crate::promotion::FIRST_TIER..=crate::promotion::LAST_TIER {
                sqlx::query("UPDATE characters SET class_tier = ? WHERE level > ? AND level <= ?")
                    .bind(tier as i64)
                    .bind(if tier == crate::promotion::FIRST_TIER {
                        0
                    } else {
                        crate::promotion::level_cap(tier - 1) as i64
                    })
                    .bind(crate::promotion::level_cap(tier) as i64)
                    .execute(&self.pool)
                    .await
                    .context("seeding the class tier from the level")?;
            }
        }

        for (table, column) in [
            ("characters", "skill_list BLOB"),
            ("characters", "item_bar BLOB"),
            ("characters", "skill_points INTEGER NOT NULL DEFAULT 0"),
            ("accounts", "storage_gold INTEGER NOT NULL DEFAULT 0"),
        ] {
            self.add_column(table, column).await?;
        }
        Ok(())
    }

    /// Adds one column, saying whether it was actually added.
    ///
    /// Re-running this on a database that already has it raises a "duplicate
    /// column" error, which is the one error swallowed -- both dialects raise
    /// it and it means the work is already done. The answer matters for a
    /// column that has to be filled in from the rows that are already there.
    async fn add_column(&self, table: &str, column: &str) -> Result<bool> {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column}");
        match sqlx::query(&sql).execute(&self.pool).await {
            Ok(_) => Ok(true),
            Err(e) if e.to_string().to_ascii_lowercase().contains("duplicate column") => Ok(false),
            Err(e) => Err(anyhow::Error::new(e).context(format!("adding {column}"))),
        }
    }

    /// Gives a durability to items stored without one.
    ///
    /// An item is created with the table's durability in both halves
    /// (`Item::from_table`). Paths that predate that stored zero out of zero,
    /// which the client reads as broken and silently refuses to equip -- no
    /// packet, no refusal, nothing in a log to find it by.
    ///
    /// `durability_max = 0` is what makes those safe to tell apart from a
    /// worn one: wear only ever lowers the first half, so an item that has
    /// been used has a ceiling and an item that was never given one has not.
    /// Runs on every start and is its own no-op once there is nothing left
    /// to fix, so a database that has been through it does not need marking.
    pub async fn repair_durability(&self, items: &aika_data::itemlist::ItemList) -> Result<usize> {
        let rows = sqlx::query("SELECT id, item_index FROM items WHERE durability_max = 0")
            .fetch_all(&self.pool)
            .await
            .context("looking for items with no durability")?;

        let mut fixed = 0;
        for row in rows {
            let id = row.try_get::<i64, _>("id")?;
            let index = row.try_get::<i64, _>("item_index")? as usize;
            let Some(durability) = items.get(index).map(|def| def.durability()).filter(|d| *d > 0)
            else {
                continue;
            };
            sqlx::query("UPDATE items SET durability_min = ?, durability_max = ? WHERE id = ?")
                .bind(durability as i64)
                .bind(durability as i64)
                .bind(id)
                .execute(&self.pool)
                .await
                .context("giving an item its durability")?;
            fixed += 1;
        }
        Ok(fixed)
    }
    pub async fn account_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM accounts").fetch_one(&self.pool).await?;
        Ok(row.try_get::<i64, _>("n")?)
    }

    /// Writes the accounts from the configuration file, for a database that
    /// has none. It is how a fresh checkout gets a usable login without a
    /// separate setup step; once accounts exist the config is ignored.
    pub async fn seed(&self, accounts: &[DevAccount]) -> Result<usize> {
        if accounts.is_empty() {
            return Ok(0);
        }
        let mut written = 0;

        for entry in accounts {
            let account = Account::from_dev(entry, 0)?;
            let account_id = self.insert_account(&account).await?;
            // The chest belongs to the account, so it is written here rather
            // than once per character, and it is written even though it holds
            // only the vaults: those are what its pages are unlocked by.
            self.save_storage(account_id, account.storage_gold, &account.storage).await?;
            for character in &account.characters {
                self.insert_character(account_id, character).await?;
            }
            written += 1;
        }
        Ok(written)
    }

    async fn insert_account(&self, account: &Account) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO accounts
                 (username, password_hash, nation, account_status, ban_days, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(&account.username)
        .bind(&account.password_hash)
        .bind(account.nation as i64)
        .bind(account.account_status as i64)
        .bind(account.ban_days as i64)
        .bind(now())
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("inserting account {}", account.username))?;

        Ok(row.try_get::<i64, _>("id")?)
    }

    pub async fn insert_character(&self, account_id: i64, character: &Character) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO characters
                 (account_id, slot, name, nation, class_index, hair, level, exp, gold, class_tier,
                  x, y, speed_move, height, torso, legs, body,
                  strength, agility, intellect, constitution, luck, free_points,
                  skill_list, item_bar, skill_points, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(account_id)
        .bind(character.slot as i64)
        .bind(&character.name)
        .bind(character.nation as i64)
        .bind(character.class_index as i64)
        .bind(character.hair as i64)
        .bind(character.level as i64)
        .bind(character.exp as i64)
        .bind(character.gold as i64)
        .bind(character.tier as i64)
        .bind(character.x as i64)
        .bind(character.y as i64)
        .bind(character.speed_move as i64)
        .bind(character.sizes[0] as i64)
        .bind(character.sizes[1] as i64)
        .bind(character.sizes[2] as i64)
        .bind(character.sizes[3] as i64)
        .bind(character.attributes[0] as i64)
        .bind(character.attributes[1] as i64)
        .bind(character.attributes[2] as i64)
        .bind(character.attributes[3] as i64)
        .bind(character.attributes[4] as i64)
        .bind(character.attributes[5] as i64)
        .bind(pack_u16(&character.skill_list))
        .bind(pack_u32(&character.item_bar))
        .bind(character.skill_points as i64)
        .bind(now())
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("inserting character {}", character.name))?;

        // A new character is handed starting gear, and it has to go in with
        // it: writing the row alone leaves a character that arrives empty
        // after the first restart.
        let id: i64 = row.try_get("id")?;
        self.save_inventory(id, &character.items).await?;
        Ok(id)
    }

    /// Every account with its characters and their items.
    pub async fn load_accounts(&self) -> Result<Vec<Account>> {
        // The columns are named rather than starred. A `SELECT *` prepared in
        // the same process that just added a column through ALTER TABLE hands
        // back a row with the old width while claiming the new one, and reading
        // the added column then panics inside the driver rather than failing.
        let rows = sqlx::query(
            "SELECT id, username, password_hash, nation, account_status, ban_days,
                    storage_gold
             FROM accounts ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .context("loading accounts")?;

        let mut accounts = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = row.try_get("id")?;
            accounts.push(Account {
                id: id as u32,
                username: row.try_get::<String, _>("username")?,
                password_hash: row.try_get::<String, _>("password_hash")?,
                nation: row.try_get::<i64, _>("nation")? as u8,
                account_status: row.try_get::<i64, _>("account_status")? as u8,
                ban_days: row.try_get::<i64, _>("ban_days")? as u32,
                characters: self.load_characters(id).await?,
                storage: self.load_storage(id).await?,
                storage_gold: row.try_get::<i64, _>("storage_gold").unwrap_or(0) as u64,
                last_token: None,
                last_token_at: None,
            });
        }
        Ok(accounts)
    }

    async fn load_characters(&self, account_id: i64) -> Result<Vec<Character>> {
        // Named for the same reason the account columns are.
        let rows = sqlx::query(
            "SELECT id, slot, name, nation, class_index, hair, level, exp, gold, class_tier,
                    x, y, speed_move, height, torso, legs, body,
                    strength, agility, intellect, constitution, luck, free_points,
                    skill_list, item_bar, skill_points
             FROM characters
             WHERE account_id = ? AND deleted_at IS NULL
             ORDER BY slot",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .context("loading characters")?;

        let mut characters = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = row.try_get("id")?;
            characters.push(Character {
                id,
                slot: row.try_get::<i64, _>("slot")? as usize,
                name: row.try_get::<String, _>("name")?,
                nation: row.try_get::<i64, _>("nation")? as u16,
                class_index: row.try_get::<i64, _>("class_index")? as u16,
                hair: row.try_get::<i64, _>("hair")? as u16,
                level: row.try_get::<i64, _>("level")? as u16,
                exp: row.try_get::<i64, _>("exp")? as u64,
                gold: row.try_get::<i64, _>("gold")? as u64,
                tier: row.try_get::<i64, _>("class_tier")? as u16,
                x: row.try_get::<i64, _>("x")? as u32,
                y: row.try_get::<i64, _>("y")? as u32,
                speed_move: row.try_get::<i64, _>("speed_move")? as u8,
                sizes: [
                    row.try_get::<i64, _>("height")? as u8,
                    row.try_get::<i64, _>("torso")? as u8,
                    row.try_get::<i64, _>("legs")? as u8,
                    row.try_get::<i64, _>("body")? as u8,
                ],
                attributes: [
                    row.try_get::<i64, _>("strength")? as u16,
                    row.try_get::<i64, _>("agility")? as u16,
                    row.try_get::<i64, _>("intellect")? as u16,
                    row.try_get::<i64, _>("constitution")? as u16,
                    row.try_get::<i64, _>("luck")? as u16,
                    row.try_get::<i64, _>("free_points")? as u16,
                ],
                skill_list: unpack_u16(row.try_get::<Option<Vec<u8>>, _>("skill_list")?),
                item_bar: unpack_u32(row.try_get::<Option<Vec<u8>>, _>("item_bar")?),
                skill_points: match row.try_get::<i64, _>("skill_points") {
                    Ok(points) => points as u16,
                    Err(e) => {
                        tracing::warn!(error = %e, "skill_points column not read, defaulting to 0");
                        0
                    }
                },
                items: self.load_items(id).await?.into(),
            });
        }
        Ok(characters)
    }

    pub async fn load_items(&self, character_id: i64) -> Result<Vec<Item>> {
        let rows = sqlx::query(
            "SELECT * FROM items WHERE character_id = ? ORDER BY container, slot",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await
        .context("loading items")?;

        rows.into_iter()
            .map(|row| {
                Ok(Item {
                    container: row.try_get::<i64, _>("container")? as u8,
                    slot: row.try_get::<i64, _>("slot")? as u16,
                    index: row.try_get::<i64, _>("item_index")? as u16,
                    appearance: row.try_get::<i64, _>("appearance")? as u16,
                    identific: row.try_get::<i64, _>("identific")? as i32,
                    effect_index: [
                        row.try_get::<i64, _>("effect1_index")? as u8,
                        row.try_get::<i64, _>("effect2_index")? as u8,
                        row.try_get::<i64, _>("effect3_index")? as u8,
                    ],
                    effect_value: [
                        row.try_get::<i64, _>("effect1_value")? as u8,
                        row.try_get::<i64, _>("effect2_value")? as u8,
                        row.try_get::<i64, _>("effect3_value")? as u8,
                    ],
                    durability_min: row.try_get::<i64, _>("durability_min")? as u8,
                    durability_max: row.try_get::<i64, _>("durability_max")? as u8,
                    refine: row.try_get::<i64, _>("refine")? as u16,
                    expires_at: row.try_get::<i64, _>("expires_at")? as u32,
                })
            })
            .collect()
    }

    /// The account's chest. Its rows carry no container: every one of them is
    /// in the storage, which is the whole point of the separate table.
    ///
    /// Nothing at all means a chest that was never written — an account made
    /// before there was a table to write it to. It comes back with the vaults
    /// a new one is given, because a chest with no vaults has no unlocked
    /// pages and is a chest nobody could ever put anything in. A chest that
    /// has been used cannot look like this: the vaults are the one thing in it
    /// that cannot be taken out.
    pub async fn load_storage(&self, account_id: i64) -> Result<Inventory> {
        let rows = sqlx::query("SELECT * FROM storage_items WHERE account_id = ? ORDER BY slot")
            .bind(account_id)
            .fetch_all(&self.pool)
            .await
            .context("loading the storage")?;

        if rows.is_empty() {
            return Ok(crate::creation::starting_storage());
        }

        rows.into_iter()
            .map(|row| {
                Ok(Item {
                    container: crate::inventory::STORAGE,
                    slot: row.try_get::<i64, _>("slot")? as u16,
                    index: row.try_get::<i64, _>("item_index")? as u16,
                    appearance: row.try_get::<i64, _>("appearance")? as u16,
                    identific: row.try_get::<i64, _>("identific")? as i32,
                    effect_index: [
                        row.try_get::<i64, _>("effect1_index")? as u8,
                        row.try_get::<i64, _>("effect2_index")? as u8,
                        row.try_get::<i64, _>("effect3_index")? as u8,
                    ],
                    effect_value: [
                        row.try_get::<i64, _>("effect1_value")? as u8,
                        row.try_get::<i64, _>("effect2_value")? as u8,
                        row.try_get::<i64, _>("effect3_value")? as u8,
                    ],
                    durability_min: row.try_get::<i64, _>("durability_min")? as u8,
                    durability_max: row.try_get::<i64, _>("durability_max")? as u8,
                    refine: row.try_get::<i64, _>("refine")? as u16,
                    expires_at: row.try_get::<i64, _>("expires_at")? as u32,
                })
            })
            .collect()
    }

    /// Writes the chest and the gold in it. One call and one transaction: a
    /// crash between the two would move an item without moving what paid for
    /// the space, or worse, lose it.
    pub async fn save_storage(
        &self,
        account_id: i64,
        gold: u64,
        storage: &Inventory,
    ) -> Result<usize> {
        let mut tx = self.pool.begin().await.context("saving the storage")?;

        sqlx::query("UPDATE accounts SET storage_gold = ? WHERE id = ?")
            .bind(gold as i64)
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .context("saving the storage gold")?;

        sqlx::query("DELETE FROM storage_items WHERE account_id = ?")
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .context("clearing the old storage")?;

        let mut written = 0;
        for item in storage.iter().filter(|i| !i.is_empty()) {
            sqlx::query(
                "INSERT INTO storage_items
                   (account_id, slot, item_index, appearance, identific,
                    effect1_index, effect2_index, effect3_index,
                    effect1_value, effect2_value, effect3_value,
                    durability_min, durability_max, refine, expires_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(account_id)
            .bind(item.slot as i64)
            .bind(item.index as i64)
            .bind(item.appearance as i64)
            .bind(item.identific as i64)
            .bind(item.effect_index[0] as i64)
            .bind(item.effect_index[1] as i64)
            .bind(item.effect_index[2] as i64)
            .bind(item.effect_value[0] as i64)
            .bind(item.effect_value[1] as i64)
            .bind(item.effect_value[2] as i64)
            .bind(item.durability_min as i64)
            .bind(item.durability_max as i64)
            .bind(item.refine as i64)
            .bind(item.expires_at as i64)
            .execute(&mut *tx)
            .await
            .context("writing a storage item")?;
            written += 1;
        }

        tx.commit().await.context("committing the storage")?;
        Ok(written)
    }

    /// Saves where a character stands, which is what makes a player log back
    /// in where they logged out.
    pub async fn save_position(&self, character_id: i64, x: u32, y: u32) -> Result<()> {
        sqlx::query("UPDATE characters SET x = ?, y = ? WHERE id = ?")
            .bind(x as i64)
            .bind(y as i64)
            .bind(character_id)
            .execute(&self.pool)
            .await
            .context("saving position")?;
        Ok(())
    }

    /// Saves everything that changes while playing.
    pub async fn save_character(&self, character: &Character) -> Result<()> {
        sqlx::query(
            "UPDATE characters
             SET x = ?, y = ?, level = ?, exp = ?, gold = ?, class_tier = ?,
                 strength = ?, agility = ?, intellect = ?,
                 constitution = ?, luck = ?, free_points = ?
             WHERE id = ?",
        )
        .bind(character.x as i64)
        .bind(character.y as i64)
        .bind(character.level as i64)
        .bind(character.exp as i64)
        .bind(character.gold as i64)
        .bind(character.tier as i64)
        .bind(character.attributes[0] as i64)
        .bind(character.attributes[1] as i64)
        .bind(character.attributes[2] as i64)
        .bind(character.attributes[3] as i64)
        .bind(character.attributes[4] as i64)
        .bind(character.attributes[5] as i64)
        .bind(character.id)
        .execute(&self.pool)
        .await
        .context("saving character")?;
        Ok(())
    }

    /// Writes one item into a slot, replacing whatever was there.
    ///
    /// Spelled as a delete followed by an insert rather than an upsert,
    /// because the upsert syntaxes differ between SQLite and MySQL and this
    /// one works on both.
    pub async fn put_item(&self, character_id: i64, item: &Item) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM items WHERE character_id = ? AND container = ? AND slot = ?")
            .bind(character_id)
            .bind(item.container as i64)
            .bind(item.slot as i64)
            .execute(&mut *tx)
            .await?;

        if item.index != 0 {
            sqlx::query(
                "INSERT INTO items
                     (character_id, container, slot, item_index, appearance, identific,
                      effect1_index, effect2_index, effect3_index,
                      effect1_value, effect2_value, effect3_value,
                      durability_min, durability_max, refine, expires_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(character_id)
            .bind(item.container as i64)
            .bind(item.slot as i64)
            .bind(item.index as i64)
            .bind(item.appearance as i64)
            .bind(item.identific as i64)
            .bind(item.effect_index[0] as i64)
            .bind(item.effect_index[1] as i64)
            .bind(item.effect_index[2] as i64)
            .bind(item.effect_value[0] as i64)
            .bind(item.effect_value[1] as i64)
            .bind(item.effect_value[2] as i64)
            .bind(item.durability_min as i64)
            .bind(item.durability_max as i64)
            .bind(item.refine as i64)
            .bind(item.expires_at as i64)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await.context("writing item")?;
        Ok(())
    }

    /// Marks a character deleted without dropping the row, the way the
    /// original schedules a deletion instead of performing it.
    /// Replaces everything a character carries.
    ///
    /// Written as a delete and a set of inserts inside one transaction rather
    /// than as an upsert, because the two dialects spell upserts differently
    /// and this file has to work on both. It is also the only shape that gets
    /// an emptied slot right: an update alone would leave the old row behind.
    pub async fn save_inventory(
        &self,
        character_id: i64,
        items: &Inventory,
    ) -> Result<usize> {
        let mut tx = self.pool.begin().await.context("saving the inventory")?;

        sqlx::query("DELETE FROM items WHERE character_id = ?")
            .bind(character_id)
            .execute(&mut *tx)
            .await
            .context("clearing the old inventory")?;

        let mut written = 0;
        for item in items.iter().filter(|i| !i.is_empty()) {
            sqlx::query(
                "INSERT INTO items
                   (character_id, container, slot, item_index, appearance, identific,
                    effect1_index, effect2_index, effect3_index,
                    effect1_value, effect2_value, effect3_value,
                    durability_min, durability_max, refine, expires_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(character_id)
            .bind(item.container as i64)
            .bind(item.slot as i64)
            .bind(item.index as i64)
            .bind(item.appearance as i64)
            .bind(item.identific as i64)
            .bind(item.effect_index[0] as i64)
            .bind(item.effect_index[1] as i64)
            .bind(item.effect_index[2] as i64)
            .bind(item.effect_value[0] as i64)
            .bind(item.effect_value[1] as i64)
            .bind(item.effect_value[2] as i64)
            .bind(item.durability_min as i64)
            .bind(item.durability_max as i64)
            .bind(item.refine as i64)
            .bind(item.expires_at as i64)
            .execute(&mut *tx)
            .await
            .context("writing an item")?;
            written += 1;
        }

        tx.commit().await.context("committing the inventory")?;
        Ok(written)
    }

    /// What a session leaves behind: where the character stood, what it holds
    /// and how much it has. One call so a disconnect cannot save half of it.
    pub async fn save_session(&self, character: &Character) -> Result<()> {
        sqlx::query(
            "UPDATE characters SET x = ?, y = ?, gold = ?, skill_list = ?, item_bar = ?,
                 skill_points = ?
             WHERE id = ?",
        )
        .bind(character.x as i64)
        .bind(character.y as i64)
        .bind(character.gold as i64)
        .bind(pack_u16(&character.skill_list))
        .bind(pack_u32(&character.item_bar))
        .bind(character.skill_points as i64)
        .bind(character.id)
        .execute(&self.pool)
        .await
        .context("saving the character")?;

        self.save_inventory(character.id, &character.items).await?;
        Ok(())
    }

    /// The character in a slot, if the account has one there.
    pub async fn character_in_slot(
        &self,
        account_id: i64,
        slot: u32,
    ) -> Result<Option<i64>> {
        let row = sqlx::query(
            "SELECT id FROM characters
             WHERE account_id = ? AND slot = ? AND deleted_at IS NULL",
        )
        .bind(account_id)
        .bind(slot as i64)
        .fetch_optional(&self.pool)
        .await
        .context("looking up a character by slot")?;

        Ok(match row {
            Some(row) => Some(row.try_get::<i64, _>("id")?),
            None => None,
        })
    }

    /// Marks a character deleted and moves it out of its slot.
    ///
    /// Both halves matter. Keeping the row is what makes a mistaken deletion
    /// recoverable and what stops the name being claimed a minute later. But
    /// `UNIQUE (account_id, slot)` does not care whether a row is deleted, so
    /// a row left sitting in slot 1 blocks the next character from being made
    /// there — which is exactly what happened to somebody.
    ///
    /// The slot becomes the negative of the row id: unique by construction,
    /// never in the 0 to 2 a live character uses, and it still says where the
    /// character was. A partial index would be the tidier fix and MySQL does
    /// not have them, so this is the portable one.
    pub async fn soft_delete_character(&self, character_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE characters SET deleted_at = ?, slot = -id
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(now())
        .bind(character_id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("deleting character {character_id}"))?;
        Ok(())
    }
}

/// Unix seconds. Times are integers so the column means the same thing in
/// SQLite and MySQL.
fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The skill list and the hotbar are fixed-size arrays of little numbers.
/// Rather than a column each — sixty and forty of them — they are stored as
/// one blob of little-endian bytes, which is portable between SQLite and
/// MySQL and reads back into the same array.
fn pack_u16<const N: usize>(values: &[u16; N]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn pack_u32<const N: usize>(values: &[u32; N]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Reads a blob back into a fixed array. A missing or short blob — an older
/// row, a NULL — fills what it can and leaves the rest zero, so a character
/// saved before these columns existed simply comes back with an empty bar.
fn unpack_u16<const N: usize>(blob: Option<Vec<u8>>) -> [u16; N] {
    let bytes = blob.unwrap_or_default();
    std::array::from_fn(|i| {
        bytes
            .get(i * 2..i * 2 + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .unwrap_or(0)
    })
}

fn unpack_u32<const N: usize>(blob: Option<Vec<u8>>) -> [u32; N] {
    let bytes = blob.unwrap_or_default();
    std::array::from_fn(|i| {
        bytes
            .get(i * 4..i * 4 + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0)
    })
}

/// Defaults used when seeding a character that the configuration left blank.
pub fn default_sizes() -> [u8; 4] {
    DEFAULT_SIZES
}

pub fn default_speed() -> u8 {
    DEFAULT_SPEED_MOVE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DevCharacter;

    async fn memory_db() -> Database {
        Database::open("sqlite::memory:").await.expect("in-memory database")
    }

    fn dev_account(username: &str, character: &str) -> DevAccount {
        DevAccount {
            username: username.into(),
            password: Some("admin".into()),
            password_hash: None,
            nation: 2,
            account_status: 0,
            ban_days: 0,
            characters: vec![DevCharacter {
                name: character.into(),
                slot: 0,
                level: 42,
                class_index: 20,
                hair: 7702,
                nation: 2,
                gold: 999,
                exp: 12345,
                x: Some(3450),
                y: Some(690),
                speed_move: None,
            }],
        }
    }

    #[tokio::test]
    async fn seeds_and_loads_back_what_it_wrote() {
        let db = memory_db().await;
        assert_eq!(db.account_count().await.unwrap(), 0);

        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();
        assert_eq!(db.account_count().await.unwrap(), 1);

        let accounts = db.load_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);

        let account = &accounts[0];
        assert_eq!(account.username, "admin");
        assert_eq!(account.nation, 2);

        let character = &account.characters[0];
        assert_eq!(character.name, "Athus");
        assert_eq!(character.level, 42);
        assert_eq!(character.gold, 999);
        assert_eq!((character.x, character.y), (3450, 690));
        assert!(character.id > 0, "a loaded character carries its row id");
    }

    /// A session ends with three things worth keeping, and losing any one of
    /// them is the bug players notice: gold that resets, a bought item that
    /// vanishes, a walk that never happened.
    #[tokio::test]
    async fn a_session_saves_position_gold_and_what_was_carried() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let mut character = db.load_accounts().await.unwrap()[0].characters[0].clone();
        character.x = 4200;
        character.y = 815;
        character.gold = 12345;
        character
            .items
            .put(Item { index: 1595, container: 1, slot: 3, refine: 7, ..Item::default() })
            .unwrap();

        db.save_session(&character).await.unwrap();

        let reloaded = db.load_accounts().await.unwrap()[0].characters[0].clone();
        assert_eq!((reloaded.x, reloaded.y), (4200, 815));
        assert_eq!(reloaded.gold, 12345);
        assert_eq!(reloaded.items.get(1, 3).unwrap().index, 1595);
        assert_eq!(reloaded.items.get(1, 3).unwrap().refine, 7);
    }

    /// Selling everything has to leave the bag empty, not leave the old rows
    /// behind. This is what an update alone would get wrong.
    #[tokio::test]
    async fn saving_an_emptied_bag_removes_the_old_rows() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let mut character = db.load_accounts().await.unwrap()[0].characters[0].clone();
        character.items.put(Item { index: 1000, container: 1, slot: 0, ..Item::default() }).unwrap();
        character.items.put(Item { index: 2000, container: 1, slot: 1, ..Item::default() }).unwrap();
        db.save_session(&character).await.unwrap();
        assert_eq!(db.load_items(character.id).await.unwrap().len(), 2);

        character.items.take(1, 0).unwrap();
        character.items.take(1, 1).unwrap();
        db.save_session(&character).await.unwrap();

        assert!(db.load_items(character.id).await.unwrap().is_empty(), "sold items came back");
    }

    /// Deleting frees the slot. A row left sitting in it blocks the next
    /// character from being made there, because the unique index does not
    /// care whether a row is deleted.
    #[tokio::test]
    async fn deleting_frees_the_slot_for_a_new_character() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let mut second = Character::from(&crate::config::DevCharacter {
            name: "Segundo".into(),
            slot: 1,
            level: 1,
            class_index: 20,
            hair: 7702,
            nation: 0,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        });
        let id = db.insert_character(1, &second).await.unwrap();
        db.soft_delete_character(id).await.unwrap();

        // the same slot, again
        second.name = "Terceiro".into();
        db.insert_character(1, &second)
            .await
            .expect("the deleted character is still holding the slot");

        let account = &db.load_accounts().await.unwrap()[0];
        let names: Vec<&str> = account.characters.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Athus", "Terceiro"], "the deleted one came back");
    }

    /// The name stays taken, which is the point of keeping the row.
    #[tokio::test]
    async fn a_deleted_characters_name_stays_taken() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let id = db.load_accounts().await.unwrap()[0].characters[0].id;
        db.soft_delete_character(id).await.unwrap();

        let mut clone = Character::from(&crate::config::DevCharacter {
            name: "Athus".into(),
            slot: 1,
            level: 1,
            class_index: 20,
            hair: 7702,
            nation: 0,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        });
        clone.slot = 1;
        assert!(
            db.insert_character(1, &clone).await.is_err(),
            "the name of a deleted character was handed out again"
        );
    }

    /// A character is inserted with whatever it is holding, not as a bare row.
    #[tokio::test]
    async fn inserting_a_character_stores_what_it_starts_with() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let mut fresh = Character::from(&crate::config::DevCharacter {
            name: "Novato".into(),
            slot: 1,
            level: 1,
            class_index: 20,
            hair: 7702,
            nation: 0,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        });
        fresh
            .items
            .put(Item { index: 5300, container: 1, slot: 0, refine: 1, ..Item::default() })
            .unwrap();

        let id = db.insert_character(1, &fresh).await.unwrap();
        let stored = db.load_items(id).await.unwrap();

        assert_eq!(stored.len(), 1, "the starting gear was not stored");
        assert_eq!(stored[0].index, 5300);
    }

    /// The whole point of persistence: log out somewhere, log back in there.
    #[tokio::test]
    async fn a_character_logs_back_in_where_it_logged_out() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let id = db.load_accounts().await.unwrap()[0].characters[0].id;
        db.save_position(id, 4200, 815).await.unwrap();

        let reloaded = db.load_accounts().await.unwrap();
        let character = &reloaded[0].characters[0];
        assert_eq!((character.x, character.y), (4200, 815));
    }

    #[tokio::test]
    async fn saving_a_character_keeps_level_experience_and_gold() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();

        let mut character = db.load_accounts().await.unwrap()[0].characters[0].clone();
        character.level = 50;
        character.exp = 999_999;
        character.gold = 1_234_567;
        character.x = 100;
        character.y = 200;
        db.save_character(&character).await.unwrap();

        let reloaded = db.load_accounts().await.unwrap()[0].characters[0].clone();
        assert_eq!(reloaded.level, 50);
        assert_eq!(reloaded.exp, 999_999);
        assert_eq!(reloaded.gold, 1_234_567);
        assert_eq!((reloaded.x, reloaded.y), (100, 200));
    }

    #[tokio::test]
    async fn items_survive_a_reload_and_a_slot_holds_one_thing() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();
        let id = db.load_accounts().await.unwrap()[0].characters[0].id;

        let sword = Item {
            container: CONTAINER_INVENTORY as u8,
            slot: 3,
            index: 1595,
            refine: 7,
            durability_min: 40,
            durability_max: 40,
            ..Item::default()
        };
        db.put_item(id, &sword).await.unwrap();

        let items = db.load_items(id).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].index, 1595);
        assert_eq!(items[0].refine, 7);

        // writing the same slot replaces rather than duplicates
        let potion = Item { index: 4314, ..sword.clone() };
        db.put_item(id, &potion).await.unwrap();
        let items = db.load_items(id).await.unwrap();
        assert_eq!(items.len(), 1, "a slot holds one item");
        assert_eq!(items[0].index, 4314);

        // an empty item clears the slot
        db.put_item(id, &Item { index: 0, ..sword }).await.unwrap();
        assert!(db.load_items(id).await.unwrap().is_empty());
    }

    /// The chest is the account's, so it has to come back for whichever
    /// character logs in next — with its gold, which is not the purse.
    #[tokio::test]
    async fn the_chest_and_its_gold_survive_a_reload() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();
        let account_id = db.load_accounts().await.unwrap()[0].id as i64;

        // A seeded account starts with the four vaults and nothing else.
        let mut storage = db.load_storage(account_id).await.unwrap();
        assert_eq!(
            storage.in_container(crate::inventory::STORAGE).count(),
            crate::inventory::STORAGE_PAGE_ITEMS.count(),
            "a fresh chest has no vaults, so none of its pages open"
        );

        storage
            .put(Item {
                container: crate::inventory::STORAGE,
                slot: 3,
                index: 4314,
                refine: 2,
                ..Item::default()
            })
            .unwrap();
        db.save_storage(account_id, 12_345, &storage).await.unwrap();

        let account = &db.load_accounts().await.unwrap()[0];
        assert_eq!(account.storage_gold, 12_345, "the chest gold did not come back");
        let kept = account.storage.get(crate::inventory::STORAGE, 3).expect("slot 3 came back empty");
        assert_eq!((kept.index, kept.refine), (4314, 2));
    }

    #[tokio::test]
    async fn a_deleted_character_stops_loading_but_keeps_its_row() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();
        let id = db.load_accounts().await.unwrap()[0].characters[0].id;

        db.soft_delete_character(id).await.unwrap();

        let accounts = db.load_accounts().await.unwrap();
        assert!(accounts[0].characters.is_empty(), "deleted characters do not load");

        let row = sqlx::query("SELECT deleted_at FROM characters WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert!(row.try_get::<i64, _>("deleted_at").unwrap() > 0, "the row is still there");
    }

    /// An item stored with no durability at all is a piece of armour the
    /// client will not equip, and it says nothing when it refuses. A worn
    /// one is left alone: it keeps its ceiling, and only the ceiling being
    /// zero says the item was never given one.
    #[tokio::test]
    async fn items_stored_without_a_durability_are_given_one() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();
        let character_id = db.load_accounts().await.unwrap()[0].characters[0].id;

        // A table where item 10 wears out and item 11 does not.
        let table = {
            use aika_data::itemlist::{field, RECORD_SIZE};
            let mut raw = vec![0u8; 20 * RECORD_SIZE];
            for index in [10usize, 11] {
                raw[index * RECORD_SIZE + field::NAME.start] = b'x';
            }
            raw[10 * RECORD_SIZE + field::DURABILITY] = 80;
            aika_data::itemlist::ItemList::decode(&raw).expect("the fixture table is malformed")
        };
        // never given one, and the table has a value for it
        db.put_item(character_id, &Item { index: 10, container: 1, slot: 0, ..Item::default() })
            .await
            .unwrap();
        // worn down to nothing, which is not the same thing at all
        db.put_item(
            character_id,
            &Item {
                index: 10,
                container: 1,
                slot: 1,
                durability_min: 0,
                durability_max: 80,
                ..Item::default()
            },
        )
        .await
        .unwrap();
        // and one the table itself says has no durability
        db.put_item(character_id, &Item { index: 11, container: 1, slot: 2, ..Item::default() })
            .await
            .unwrap();

        assert_eq!(db.repair_durability(&table).await.unwrap(), 1, "it fixed the wrong number");

        let items = db.load_items(character_id).await.unwrap();
        let at = |slot: u16| items.iter().find(|i| i.slot == slot).cloned().unwrap();
        assert_eq!((at(0).durability_min, at(0).durability_max), (80, 80), "never given one");
        assert_eq!((at(1).durability_min, at(1).durability_max), (0, 80), "worn, not broken");
        assert_eq!((at(2).durability_min, at(2).durability_max), (0, 0), "none in the table");

        // and running it again finds nothing, so a start-up cost is paid once
        assert_eq!(db.repair_durability(&table).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn a_name_cannot_be_taken_twice() {
        let db = memory_db().await;
        db.seed(&[dev_account("admin", "Athus")]).await.unwrap();
        assert!(
            db.seed(&[dev_account("outro", "Athus")]).await.is_err(),
            "two characters must not share a name"
        );
    }
}
