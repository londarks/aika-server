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
        Ok(())
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
                 (account_id, slot, name, nation, class_index, hair, level, exp, gold,
                  x, y, speed_move, height, torso, legs, body,
                  strength, agility, intellect, constitution, luck, free_points, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(now())
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("inserting character {}", character.name))?;

        Ok(row.try_get::<i64, _>("id")?)
    }

    /// Every account with its characters and their items.
    pub async fn load_accounts(&self) -> Result<Vec<Account>> {
        let rows = sqlx::query("SELECT * FROM accounts ORDER BY id")
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
                last_token: None,
                last_token_at: None,
            });
        }
        Ok(accounts)
    }

    async fn load_characters(&self, account_id: i64) -> Result<Vec<Character>> {
        let rows = sqlx::query(
            "SELECT * FROM characters
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
                    expires_at: row.try_get::<i64, _>("expires_at")? as u16,
                })
            })
            .collect()
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
             SET x = ?, y = ?, level = ?, exp = ?, gold = ?,
                 strength = ?, agility = ?, intellect = ?,
                 constitution = ?, luck = ?, free_points = ?
             WHERE id = ?",
        )
        .bind(character.x as i64)
        .bind(character.y as i64)
        .bind(character.level as i64)
        .bind(character.exp as i64)
        .bind(character.gold as i64)
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
        sqlx::query("UPDATE characters SET x = ?, y = ?, gold = ? WHERE id = ?")
            .bind(character.x as i64)
            .bind(character.y as i64)
            .bind(character.gold as i64)
            .bind(character.id)
            .execute(&self.pool)
            .await
            .context("saving the character")?;

        self.save_inventory(character.id, &character.items).await?;
        Ok(())
    }

    pub async fn soft_delete_character(&self, character_id: i64) -> Result<()> {
        sqlx::query("UPDATE characters SET deleted_at = ? WHERE id = ?")
            .bind(now())
            .bind(character_id)
            .execute(&self.pool)
            .await
            .context("deleting character")?;
        Ok(())
    }
}

/// Unix seconds. Times are integers so the column means the same thing in
/// SQLite and MySQL.
fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
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
