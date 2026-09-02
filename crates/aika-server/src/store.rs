//! Accounts and tokens.
//!
//! For now the data lives in memory, loaded from the configuration file.
//! The fields mirror the `accounts` columns in the original server's MySQL
//! dump (`id`, `password_hash`, `last_token`, `last_token_creation_time`,
//! `nation`, `account_status`, `ban_days`), so swapping in a real database
//! backend means replacing this struct's methods.

use crate::config::{DevAccount, DevCharacter};
use crate::inventory::Inventory;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Account {
    pub id: u32,
    pub username: String,
    /// Lowercase hex MD5, like the `password_hash` column.
    pub password_hash: String,
    pub nation: u8,
    pub account_status: u8,
    pub ban_days: u32,
    /// Three at most, one per slot on the selection screen.
    pub characters: Vec<Character>,
    /// The chest, which belongs to the account and not to any one character:
    /// `Account.Header.Storage`. That is what makes it useful — it is how a
    /// player hands something from one of their characters to another.
    pub storage: Inventory,
    /// Gold kept in the chest, separate from what any character carries.
    pub storage_gold: u64,
    /// The companions. They hang off the account for the same reason the
    /// chest does -- `Account.Header.Pran1` and `Pran2` -- so every character
    /// on it shares them. See [`crate::pran`].
    pub prans: Vec<crate::pran::Pran>,
    pub last_token: Option<String>,
    pub last_token_at: Option<Instant>,
}

/// One item, mirroring the 20-byte `TItem` the protocol carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    /// Which container holds it: equipment, inventory or storage.
    pub container: u8,
    pub slot: u16,
    /// Id into the item table. Zero means the slot is empty.
    pub index: u16,
    /// Appearance override, used when a look differs from the real item.
    pub appearance: u16,
    pub identific: i32,
    pub effect_index: [u8; 3],
    pub effect_value: [u8; 3],
    pub durability_min: u8,
    pub durability_max: u8,
    pub refine: u16,
    /// When the item runs out, as the record carries it: whole days since a
    /// base date for a mount, three bytes of shifted unix time for anything
    /// else. Zero for an item that does not run out. See [`crate::expiry`].
    pub expires_at: u32,
}

impl Item {
    pub fn is_empty(&self) -> bool {
        self.index == 0
    }

    /// One item, as the table defines it.
    ///
    /// The original builds every item it hands out this way:
    /// `MIN := ItemList[Index].Durabilidade` and `MAX := MIN`
    /// (`Functions/ItemFunctions.pas:414`, and again in `PutEquipament`).
    ///
    /// Skipping it stores a piece of armour as zero out of zero, which is
    /// what the client draws as broken -- and a broken item is one it will
    /// not equip. It refuses on its own, without sending anything, so there
    /// is no packet to log and nothing for a server to explain. Twenty-one
    /// items in the working database were sitting like that.
    pub fn from_table(index: u16, table: &aika_data::itemlist::ItemList) -> Self {
        let durability = table.get(index as usize).map_or(0, |def| def.durability());
        Self {
            index,
            appearance: index,
            durability_min: durability,
            durability_max: durability,
            ..Self::default()
        }
    }
}

/// A character, holding what the selection screen needs to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Character {
    /// Row id in the database, zero before it has been stored.
    pub id: i64,
    /// Slot 0 to 2.
    pub slot: usize,
    pub name: String,
    pub nation: u16,
    /// Class index in the form the client uses on creation (10..69).
    pub class_index: u16,
    pub hair: u16,
    pub level: u16,
    pub exp: u64,
    pub gold: u64,
    /// Height, torso, legs and body. The default noted in the original is
    /// `07 77 77` in hexadecimal (`Data/PlayerData.pas:96`).
    pub sizes: [u8; 4],
    /// Movement speed carried in the spawn packet.
    pub speed_move: u8,
    /// Strength, agility, intelligence, constitution, luck and free points.
    pub attributes: [u16; 6],
    /// Where the character stands. The original server spawns a new character
    /// at (3450, 690) — the starting city (`PacketHandlers.pas`, character
    /// creation with `Local = 0`).
    pub x: u32,
    pub y: u32,
    /// Everything the character carries, across every container.
    pub items: Inventory,
    /// The skills the character knows, as the client reads them from the
    /// record: sixty slots, zero for empty. Kept as laid out so the client's
    /// own packing is preserved.
    pub skill_list: [u16; 60],
    /// The hotbar: forty action-bar slots, mostly empty. What the template
    /// puts here is what the player sees on the bar the first time they log
    /// in, and it is saved so a rearranged bar survives a logout.
    pub item_bar: [u32; 40],
    /// Unspent skill points. One is earned per level; learning a skill spends
    /// what that skill costs. The client shows this in the skill window.
    pub skill_points: u16,
    /// How far along the class the character is: the units digit of
    /// `ClassInfo`, and what decides how far it may level. See
    /// [`crate::promotion`].
    pub tier: u16,
}

/// How many skill points a character of a given level has earned in total
/// (`TPlayer.CalcSkillPoints`): one per level, and below the cap of fifty that
/// is the whole story.
pub fn skill_points_for(level: u16) -> u16 {
    let mut points = 0u16;
    for l in 1..=level {
        points = points.saturating_add(1);
        if l > 50 && l % 10 == 1 {
            points = points.saturating_add(7);
        }
    }
    points
}

/// Where a brand new character appears.
pub const CITY_SPAWN: (u32, u32) = (3450, 690);
/// Default body proportions, from the `07 77 77` comment in the original.
pub const DEFAULT_SIZES: [u8; 4] = [0x07, 0x77, 0x77, 0x00];
/// First guess: the original reads it from the saved account, which we do
/// not have yet. If the character moves too fast or too slow, it is this.
pub const DEFAULT_SPEED_MOVE: u8 = 50;

impl Character {
    /// Which of the six the creation screen offers, counted from one.
    ///
    /// The screen shows them in a fixed order and sends that position times
    /// ten: Guerreiro 10-19, Templaria 20-29, Atirador 30-39, Pistoleira
    /// 40-49, Feiticeiro 50-59, Cleriga 60-69.
    pub fn class_number(&self) -> u16 {
        (self.class_index / 10).clamp(1, 6)
    }

    /// The code the client reads to name a class, which is not the position.
    ///
    /// Read out of the six templates the original ships in `Data/BaseAccs`,
    /// at offset 29 of the `TCharacter` inside each: Guerreiro 1, Templaria
    /// 11, Atirador 21, Pistoleira 31, Feiticeiro 41, Cleriga 51. So it is
    /// the position minus one, times ten, plus one — the same code the skill
    /// table uses for a class at its first tier, and advancing tiers is what
    /// the second digit is for.
    ///
    /// Sending the position instead put a 6 in here for a Cleriga, which is
    /// not a code at all, and the client fell back to naming everybody a
    /// Fighter.
    ///
    /// The tier is the second digit. It is stored rather than worked out,
    /// which is what the original does -- it is the same `classinfo` column --
    /// and it is what [`crate::promotion`] moves. It used to be a hardcoded 1.
    pub fn class_info(&self) -> u16 {
        (self.class_number() - 1) * 10 + self.tier.max(crate::promotion::FIRST_TIER)
    }

    /// The same class counted from zero, which is how the skill table groups
    /// them: an Atirador's skills carry 21, 22 and 23, so its group is 2.
    pub fn skill_class(&self) -> u16 {
        self.class_number() - 1
    }
}

impl From<&DevCharacter> for Character {
    fn from(dev: &DevCharacter) -> Self {
        Self {
            id: 0,
            slot: dev.slot,
            name: dev.name.clone(),
            nation: dev.nation,
            class_index: dev.class_index,
            hair: dev.hair,
            level: dev.level,
            exp: dev.exp,
            gold: dev.gold,
            sizes: DEFAULT_SIZES,
            speed_move: dev.speed_move.unwrap_or(DEFAULT_SPEED_MOVE),
            attributes: [10, 10, 10, 10, 10, 0],
            x: dev.x.unwrap_or(CITY_SPAWN.0),
            y: dev.y.unwrap_or(CITY_SPAWN.1),
            items: Inventory::new(),
            skill_list: [0; 60],
            item_bar: [0; 40],
            skill_points: skill_points_for(dev.level),
            tier: crate::promotion::FIRST_TIER,
        }
    }
}

/// Result of `/member/aika_get_token.asp`, using the same codes the Delphi
/// server returns to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    Ok { token: String },
    NotFound,
    WrongPassword,
    Cancelled,
    Banned,
    BanExpired,
    NotCbt,
    IpBlocked,
}

impl AuthOutcome {
    pub fn as_response(&self) -> &str {
        match self {
            AuthOutcome::Ok { token } => token,
            AuthOutcome::NotFound => "0",
            AuthOutcome::WrongPassword => "-1",
            AuthOutcome::Cancelled => "-2",
            AuthOutcome::Banned => "-8",
            AuthOutcome::NotCbt => "-10",
            AuthOutcome::IpBlocked => "-11",
            AuthOutcome::BanExpired => "-22",
        }
    }
}

/// How the received password matched. The original client may send it as
/// plaintext or already MD5-hashed, and we only find out on the first real
/// login. We accept both forms and log which one matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordForm {
    Plain,
    PreHashed,
}

/// Character slots the client displays.
pub const MAX_CHARACTERS: usize = 3;

pub fn md5_hex(data: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Debug)]
struct Attempts {
    failures: u32,
}

pub struct AccountStore {
    accounts: Mutex<HashMap<String, Account>>,
    attempts: Mutex<HashMap<String, Attempts>>,
    blocked: Mutex<HashMap<IpAddr, Instant>>,
    max_attempts: u32,
    block_duration: Duration,
}

impl Account {
    /// Builds an account from a configuration entry, validating what the
    /// client cannot cope with. Shared by the in-memory store and the
    /// database seeding, so both reject the same things.
    pub fn from_dev(entry: &DevAccount, id: u32) -> anyhow::Result<Self> {
        let password_hash = match (&entry.password, &entry.password_hash) {
            (_, Some(hash)) => hash.to_ascii_lowercase(),
            (Some(plain), None) => md5_hex(plain),
            (None, None) => anyhow::bail!(
                "account '{}' needs either 'password' or 'password_hash'",
                entry.username
            ),
        };

        if entry.characters.len() > MAX_CHARACTERS {
            anyhow::bail!(
                "account '{}' has {} characters; the client shows only {}",
                entry.username,
                entry.characters.len(),
                MAX_CHARACTERS
            );
        }
        for character in &entry.characters {
            if character.slot >= MAX_CHARACTERS {
                anyhow::bail!(
                    "character '{}' is in slot {}; slots run from 0 to {}",
                    character.name,
                    character.slot,
                    MAX_CHARACTERS - 1
                );
            }
        }

        Ok(Self {
            id,
            username: entry.username.to_ascii_lowercase(),
            password_hash,
            nation: entry.nation,
            account_status: entry.account_status,
            ban_days: entry.ban_days,
            characters: entry.characters.iter().map(Character::from).collect(),
            storage: crate::creation::starting_storage(),
            storage_gold: 0,
            // An account starts with none. One is hatched from a summon stone;
            // see [`crate::pran`].
            prans: Vec::new(),
            last_token: None,
            last_token_at: None,
        })
    }
}

impl AccountStore {
    /// Builds a store from already loaded accounts, which is how the database
    /// hands its contents over.
    pub fn from_accounts(
        accounts: Vec<Account>,
        max_attempts: u32,
        block_duration: Duration,
    ) -> anyhow::Result<Self> {
        let mut map = HashMap::new();
        for account in accounts {
            let key = account.username.to_ascii_lowercase();
            if map.insert(key, account).is_some() {
                anyhow::bail!("two accounts share a username");
            }
        }
        Ok(Self {
            accounts: Mutex::new(map),
            attempts: Mutex::new(HashMap::new()),
            blocked: Mutex::new(HashMap::new()),
            max_attempts,
            block_duration,
        })
    }

    pub fn from_dev_accounts(
        entries: &[DevAccount],
        max_attempts: u32,
        block_duration: Duration,
    ) -> anyhow::Result<Self> {
        let accounts = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| Account::from_dev(entry, i as u32 + 1))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Self::from_accounts(accounts, max_attempts, block_duration)
    }


    /// Replaces a character in the in-memory copy with what a session left.
    /// The database holds the durable version of the same fact; this keeps a
    /// second login in the same run from reading a stale one.
    ///
    /// Returns whether the character was found, which is false for the id 0 a
    /// character has before it reaches the database.
    pub fn update_character(&self, updated: &Character) -> bool {
        if updated.id == 0 {
            return false;
        }
        let mut accounts = self.accounts.lock().unwrap();
        for account in accounts.values_mut() {
            for character in &mut account.characters {
                if character.id == updated.id {
                    *character = updated.clone();
                    return true;
                }
            }
        }
        false
    }

    /// Copies what a session changed back over the stored account, so the
    /// next character to log in on the same account finds what the last one
    /// left.
    ///
    /// Everything the *account* owns has to be here, not only the chest. A
    /// login reads this copy and not the database, so anything written to the
    /// database and not to this is written and then forgotten: a companion
    /// evolved and saved came back a fairy on the next login, because the
    /// account in memory still held the fairy.
    pub fn update_account(&self, updated: &Account) -> bool {
        let mut accounts = self.accounts.lock().unwrap();
        let Some(account) = accounts.get_mut(&updated.username) else {
            return false;
        };
        account.storage = updated.storage.clone();
        account.storage_gold = updated.storage_gold;
        account.prans = updated.prans.clone();
        true
    }

    /// Whether any account already has a character by this name. Names are
    /// unique across the whole server, not per account, which is what the
    /// database enforces too.
    pub fn name_taken(&self, name: &str) -> bool {
        let accounts = self.accounts.lock().unwrap();
        accounts
            .values()
            .flat_map(|a| a.characters.iter())
            .any(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Adds a freshly created character to the in-memory copy, so the list
    /// the client is sent next shows it without a reload.
    pub fn add_character(&self, username: &str, character: Character) -> bool {
        let mut accounts = self.accounts.lock().unwrap();
        let Some(account) = accounts.get_mut(&username.to_ascii_lowercase()) else {
            return false;
        };
        account.characters.retain(|c| c.slot != character.slot);
        account.characters.push(character);
        account.characters.sort_by_key(|c| c.slot);
        true
    }

    /// Takes a character out of the in-memory copy. The database keeps the
    /// row and marks it deleted; this is only the list the client is shown.
    pub fn remove_character(&self, username: &str, slot: usize) -> Option<Character> {
        let mut accounts = self.accounts.lock().unwrap();
        let account = accounts.get_mut(&username.to_ascii_lowercase())?;
        let at = account.characters.iter().position(|c| c.slot == slot)?;
        Some(account.characters.remove(at))
    }

    pub fn get(&self, username: &str) -> Option<Account> {
        let key = username.to_ascii_lowercase();
        self.accounts.lock().unwrap().get(&key).cloned()
    }

    fn is_blocked(&self, ip: IpAddr) -> bool {
        let mut blocked = self.blocked.lock().unwrap();
        match blocked.get(&ip) {
            Some(since) if since.elapsed() < self.block_duration => true,
            Some(_) => {
                blocked.remove(&ip);
                false
            }
            None => false,
        }
    }

    fn register_failure(&self, username: &str, ip: IpAddr) {
        let mut attempts = self.attempts.lock().unwrap();
        let entry = attempts
            .entry(username.to_ascii_lowercase())
            .or_insert(Attempts { failures: 0 });
        entry.failures += 1;
        if entry.failures >= self.max_attempts {
            self.blocked.lock().unwrap().insert(ip, Instant::now());
        }
    }

    fn clear_failures(&self, username: &str) {
        self.attempts.lock().unwrap().remove(&username.to_ascii_lowercase());
    }

    /// Autentica e, em caso de sucesso, grava um token novo na conta.
    pub fn authenticate(
        &self,
        username: &str,
        password: &str,
        ip: IpAddr,
    ) -> (AuthOutcome, Option<PasswordForm>) {
        if self.is_blocked(ip) {
            return (AuthOutcome::IpBlocked, None);
        }

        let key = username.to_ascii_lowercase();
        let mut accounts = self.accounts.lock().unwrap();
        let Some(account) = accounts.get_mut(&key) else {
            return (AuthOutcome::NotFound, None);
        };

        // The Delphi server compares `password_hash` with MD5(received).
        // If the client already sends MD5, the direct comparison covers it.
        let form = if md5_hex(password) == account.password_hash {
            Some(PasswordForm::Plain)
        } else if password.eq_ignore_ascii_case(&account.password_hash) {
            Some(PasswordForm::PreHashed)
        } else {
            None
        };

        let Some(form) = form else {
            drop(accounts);
            self.register_failure(&key, ip);
            return (AuthOutcome::WrongPassword, None);
        };

        match account.account_status {
            2 => return (AuthOutcome::Cancelled, Some(form)),
            8 => {
                // An expired temporary ban returns the account to normal.
                if account.ban_days > 0 {
                    let expired = account
                        .last_token_at
                        .map(|at| at.elapsed() > Duration::from_secs(86_400 * account.ban_days as u64))
                        .unwrap_or(true);
                    if expired {
                        account.ban_days = 0;
                        account.account_status = 0;
                        return (AuthOutcome::BanExpired, Some(form));
                    }
                }
                return (AuthOutcome::Banned, Some(form));
            }
            10 => return (AuthOutcome::NotCbt, Some(form)),
            _ => {}
        }

        let token = generate_token(password);
        account.last_token = Some(token.clone());
        account.last_token_at = Some(Instant::now());

        drop(accounts);
        self.clear_failures(&key);
        (AuthOutcome::Ok { token }, Some(form))
    }

    /// Checks the token the client presents again on the TCP login.
    pub fn check_token(&self, username: &str, token: &str, ttl: Duration) -> TokenCheck {
        let key = username.to_ascii_lowercase();
        let accounts = self.accounts.lock().unwrap();
        let Some(account) = accounts.get(&key) else {
            return TokenCheck::UnknownAccount;
        };

        let (Some(stored), Some(issued_at)) = (&account.last_token, account.last_token_at) else {
            return TokenCheck::NoToken;
        };
        if !stored.eq_ignore_ascii_case(token) {
            return TokenCheck::Mismatch;
        }
        if issued_at.elapsed() > ttl {
            return TokenCheck::Expired;
        }
        if account.account_status == 8 {
            return TokenCheck::Banned;
        }
        TokenCheck::Ok(account.clone())
    }

    /// Renews the current token's lifetime (`/servers/aika_reset_flag.asp`).
    pub fn reset_token_flag(&self, username: &str, token: &str) -> bool {
        let key = username.to_ascii_lowercase();
        let mut accounts = self.accounts.lock().unwrap();
        let Some(account) = accounts.get_mut(&key) else {
            return false;
        };
        match &account.last_token {
            Some(stored) if stored.eq_ignore_ascii_case(token) => {
                account.last_token_at = Some(Instant::now());
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenCheck {
    Ok(Account),
    UnknownAccount,
    NoToken,
    Mismatch,
    Expired,
    Banned,
}

impl PartialEq for Account {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.username == other.username
    }
}
impl Eq for Account {}

/// A 32 hex character token, shaped like Delphi's `TPlayerToken.Generate`
/// (`MD5(MD5(password) + MD5(now))`). The difference is the extra entropy:
/// the original collides when the same user logs in twice within one
/// second, and the token is opaque to the client anyway.
fn generate_token(password: &str) -> String {
    let nonce: u128 = rand::random();
    md5_hex(&format!("{}{}", md5_hex(password), md5_hex(&nonce.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> AccountStore {
        let entries = vec![
            DevAccount {
                username: "Admin".into(),
                password: Some("admin".into()),
                password_hash: None,
                nation: 2,
                account_status: 0,
                ban_days: 0,
                characters: Vec::new(),
            },
            DevAccount {
                username: "banido".into(),
                password: Some("x".into()),
                password_hash: None,
                nation: 1,
                account_status: 8,
                ban_days: 0,
                characters: Vec::new(),
            },
        ];
        AccountStore::from_dev_accounts(&entries, 10, Duration::from_secs(600)).unwrap()
    }

    fn ip() -> IpAddr {
        "127.0.0.1".parse().unwrap()
    }

    #[test]
    fn known_md5_of_admin() {
        // The MD5 of "admin", which the original stores in `password_hash`.
        // Pinned because the whole login turns on this one function agreeing
        // with a column somebody else wrote years ago.
        assert_eq!(md5_hex("admin"), "21232f297a57a5a743894a0e4a801fc3");
    }

    #[test]
    fn authenticates_with_plaintext_password() {
        let store = store();
        let (outcome, form) = store.authenticate("admin", "admin", ip());
        assert_eq!(form, Some(PasswordForm::Plain));
        let AuthOutcome::Ok { token } = outcome else {
            panic!("expected success, got {outcome:?}");
        };
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn authenticates_with_prehashed_password() {
        let store = store();
        let (outcome, form) = store.authenticate("admin", &md5_hex("admin"), ip());
        assert_eq!(form, Some(PasswordForm::PreHashed));
        assert!(matches!(outcome, AuthOutcome::Ok { .. }));
    }

    #[test]
    fn username_is_case_insensitive() {
        let store = store();
        let (outcome, _) = store.authenticate("ADMIN", "admin", ip());
        assert!(matches!(outcome, AuthOutcome::Ok { .. }));
    }

    #[test]
    fn reports_delphi_response_codes() {
        let store = store();
        assert_eq!(store.authenticate("ninguem", "x", ip()).0.as_response(), "0");
        assert_eq!(store.authenticate("admin", "errada", ip()).0.as_response(), "-1");
        assert_eq!(store.authenticate("banido", "x", ip()).0.as_response(), "-8");
    }

    #[test]
    fn blocks_ip_after_repeated_failures() {
        let store = store();
        for _ in 0..10 {
            assert_eq!(store.authenticate("admin", "errada", ip()).0, AuthOutcome::WrongPassword);
        }
        // even the right password is refused while the IP stays blocked
        assert_eq!(store.authenticate("admin", "admin", ip()).0, AuthOutcome::IpBlocked);
    }

    #[test]
    fn token_roundtrip_and_expiry() {
        let store = store();
        let AuthOutcome::Ok { token } = store.authenticate("admin", "admin", ip()).0 else {
            panic!("login failed");
        };

        let ttl = Duration::from_secs(300);
        assert!(matches!(store.check_token("admin", &token, ttl), TokenCheck::Ok(_)));
        // the client may echo the token back in a different case
        assert!(matches!(
            store.check_token("admin", &token.to_uppercase(), ttl),
            TokenCheck::Ok(_)
        ));
        assert_eq!(store.check_token("admin", "outro", ttl), TokenCheck::Mismatch);
        assert_eq!(store.check_token("ninguem", &token, ttl), TokenCheck::UnknownAccount);
        // zero TTL: every token is born expired
        assert_eq!(store.check_token("admin", &token, Duration::ZERO), TokenCheck::Expired);
    }

    #[test]
    fn tokens_differ_between_logins() {
        let store = store();
        let AuthOutcome::Ok { token: a } = store.authenticate("admin", "admin", ip()).0 else {
            panic!()
        };
        let AuthOutcome::Ok { token: b } = store.authenticate("admin", "admin", ip()).0 else {
            panic!()
        };
        assert_ne!(a, b, "tokens from the same second would collide in the original algorithm");
    }
}

#[cfg(test)]
mod class_tests {
    use super::*;
    use crate::config::DevCharacter;

    fn with_index(class_index: u16) -> Character {
        Character::from(&DevCharacter {
            name: "x".into(),
            slot: 0,
            level: 1,
            class_index,
            hair: 7700,
            nation: 2,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        })
    }

    /// The six the creation screen offers, in the order it offers them.
    #[test]
    fn the_class_number_is_the_index_over_ten() {
        for (index, number) in [(10, 1), (19, 1), (20, 2), (30, 3), (45, 4), (59, 5), (69, 6)] {
            assert_eq!(with_index(index).class_number(), number, "class index {index}");
        }
    }

    /// The six codes, read out of the templates the original ships. Getting
    /// one of these wrong makes the client name the wrong class, and there is
    /// no way to tell from our side that it did.
    #[test]
    fn the_class_code_is_the_one_the_templates_carry() {
        for (index, code) in
            [(10, 1), (20, 11), (30, 21), (40, 31), (50, 41), (60, 51)]
        {
            assert_eq!(with_index(index).class_info(), code, "class index {index}");
        }
    }

    /// An Atirador is the third class, its code is 21, and its skills are the
    /// third group in the table, which counts from zero.
    #[test]
    fn the_three_numbers_of_one_class_agree() {
        let atirador = with_index(30);
        assert_eq!(atirador.class_number(), 3);
        assert_eq!(atirador.class_info(), 21);
        assert_eq!(atirador.skill_class(), 2);

        assert_eq!(with_index(10).skill_class(), 0, "the first class is group zero");
        assert_eq!(with_index(69).skill_class(), 5, "the sixth is group five");
    }

    /// A class index the client should never send must not produce a code
    /// that names some other class.
    #[test]
    fn an_impossible_class_index_is_clamped() {
        assert_eq!(with_index(0).class_number(), 1);
        assert_eq!(with_index(999).class_number(), 6);
    }
}
