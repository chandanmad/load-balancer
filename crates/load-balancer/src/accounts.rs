//! Account-based rate limiting using SQLite database.
//!
//! Loads Plans, Accounts, and API Keys from SQLite and provides rate limiting
//! based on the account's plan settings.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use pingora::services::background::BackgroundService;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

// ============================================================================
// Rate Limit Trait and Structs
// ============================================================================

/// Basic rate limit description.
pub struct Limit {
    pub quota: isize,
    pub per_seconds: u64,
}

/// Provide rate limit settings for a given API key.
pub trait Ratelimit {
    fn limit_for_key(&self, api_key: &str) -> Limit;
}

// ============================================================================
// Data Structs
// ============================================================================

/// Represents a pricing tier with rate limits and quotas.
#[derive(Debug, Clone)]
pub struct Plan {
    pub plan_id: i64,
    pub name: String,
    pub monthly_quota: i32,
    pub rps_limit: i32,
    pub price_per_1k_req: f64,
}

/// Represents an account that owns subscriptions.
#[derive(Debug, Clone)]
pub struct Account {
    pub account_id: i64,
    pub email: String,
    pub plan_id: i64,
    pub billing_status: String,
}

/// Represents an API key belonging to an account.
#[derive(Debug, Clone)]
pub struct ApiKey {
    pub key_id: i64,
    pub account_id: i64,
    pub api_key_hash: String,
    pub is_active: bool,
}

/// Represents a change log entry from the database.
#[derive(Debug)]
pub struct ChangeLogEntry {
    pub change_id: i64,
    pub table_name: String,
    pub record_id: i64,
    pub operation: String,
}

// ============================================================================
// Account Store
// ============================================================================

/// Thread-safe in-memory store for account data with delta loading support.
#[derive(Debug, Default)]
pub struct AccountStore {
    /// API key hash -> Account ID
    api_key_to_account: HashMap<String, i64>,
    /// API key hash -> Key ID (for usage tracking)
    api_key_to_key_id: HashMap<String, i64>,
    /// Key ID -> API key hash (for reverse lookup during deletes)
    key_id_to_hash: HashMap<i64, String>,
    /// Account ID -> Plan ID
    account_to_plan: HashMap<i64, i64>,
    /// Plan ID -> Plan
    plans: HashMap<i64, Plan>,
    /// Track max change_id for ChangeLog-based delta loading
    max_change_id: i64,
}

impl AccountStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lookup the plan for a given API key hash.
    pub fn get_plan_for_key(&self, api_key_hash: &str) -> Option<&Plan> {
        let account_id = self.api_key_to_account.get(api_key_hash)?;
        let plan_id = self.account_to_plan.get(account_id)?;
        self.plans.get(plan_id)
    }

    /// Get full context for a key: (account_id, key_id, plan_id).
    /// Used for usage tracking.
    pub fn get_key_context(&self, api_key_hash: &str) -> Option<(i64, i64, i64)> {
        let account_id = *self.api_key_to_account.get(api_key_hash)?;
        let key_id = *self.api_key_to_key_id.get(api_key_hash)?;
        let plan_id = *self.account_to_plan.get(&account_id)?;
        Some((account_id, key_id, plan_id))
    }

    /// Get max change_id for ChangeLog-based delta loading.
    pub fn max_change_id(&self) -> i64 {
        self.max_change_id
    }

    /// Set max change_id after processing ChangeLog entries.
    pub fn set_max_change_id(&mut self, change_id: i64) {
        self.max_change_id = change_id;
    }

    /// Insert or update a plan.
    pub fn upsert_plan(&mut self, plan: Plan) {
        self.plans.insert(plan.plan_id, plan);
    }

    /// Delete a plan by ID.
    pub fn delete_plan(&mut self, plan_id: i64) {
        self.plans.remove(&plan_id);
    }

    /// Insert or update an account.
    pub fn upsert_account(&mut self, account: Account) {
        self.account_to_plan
            .insert(account.account_id, account.plan_id);
    }

    /// Delete an account by ID.
    pub fn delete_account(&mut self, account_id: i64) {
        self.account_to_plan.remove(&account_id);
    }

    /// Insert or update an API key.
    pub fn upsert_api_key(&mut self, api_key: ApiKey) {
        // Remove old hash mapping if key already exists
        if let Some(old_hash) = self.key_id_to_hash.get(&api_key.key_id) {
            self.api_key_to_account.remove(old_hash);
            self.api_key_to_key_id.remove(old_hash);
        }

        if api_key.is_active {
            self.api_key_to_account
                .insert(api_key.api_key_hash.clone(), api_key.account_id);
            self.api_key_to_key_id
                .insert(api_key.api_key_hash.clone(), api_key.key_id);
            self.key_id_to_hash
                .insert(api_key.key_id, api_key.api_key_hash);
        } else {
            // Inactive key: remove from lookup maps but keep reverse lookup
            self.key_id_to_hash.remove(&api_key.key_id);
        }
    }

    /// Delete an API key by ID.
    pub fn delete_api_key(&mut self, key_id: i64) {
        if let Some(hash) = self.key_id_to_hash.remove(&key_id) {
            self.api_key_to_account.remove(&hash);
            self.api_key_to_key_id.remove(&hash);
        }
    }
}

// ============================================================================
// Account Loader
// ============================================================================

/// Loads account data from SQLite database.
pub struct AccountLoader {
    db_path: String,
}

impl AccountLoader {
    /// Create a new loader for the given database path.
    pub fn new<P: AsRef<Path>>(db_path: P) -> Self {
        Self {
            db_path: db_path.as_ref().to_string_lossy().into_owned(),
        }
    }

    /// Open a read-only connection to the database.
    fn open_connection(&self) -> Result<Connection, rusqlite::Error> {
        Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    }

    /// Perform initial full load of all data.
    pub fn load_initial(&self) -> Result<AccountStore, rusqlite::Error> {
        let conn = self.open_connection()?;
        let mut store = AccountStore::new();

        // Load all plans
        let mut stmt = conn.prepare(
            "SELECT plan_id, name, monthly_quota, rps_limit, price_per_1k_req FROM Plans",
        )?;
        let plans = stmt.query_map([], |row| {
            Ok(Plan {
                plan_id: row.get(0)?,
                name: row.get(1)?,
                monthly_quota: row.get(2)?,
                rps_limit: row.get(3)?,
                price_per_1k_req: row.get(4)?,
            })
        })?;
        for plan in plans {
            store.upsert_plan(plan?);
        }

        // Load all accounts
        let mut stmt =
            conn.prepare("SELECT account_id, email, plan_id, billing_status FROM Accounts")?;
        let accounts = stmt.query_map([], |row| {
            Ok(Account {
                account_id: row.get(0)?,
                email: row.get(1)?,
                plan_id: row.get(2)?,
                billing_status: row.get(3)?,
            })
        })?;
        for account in accounts {
            store.upsert_account(account?);
        }

        // Load all API keys
        let mut stmt =
            conn.prepare("SELECT key_id, account_id, api_key_hash, is_active FROM APIKeys")?;
        let keys = stmt.query_map([], |row| {
            Ok(ApiKey {
                key_id: row.get(0)?,
                account_id: row.get(1)?,
                api_key_hash: row.get(2)?,
                is_active: row.get(3)?,
            })
        })?;
        for key in keys {
            store.upsert_api_key(key?);
        }

        // Get the max change_id for delta loading
        let max_change_id: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(change_id), 0) FROM ChangeLog",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        store.set_max_change_id(max_change_id);

        log::info!(
            "Loaded {} plans, {} accounts, {} API keys",
            store.plans.len(),
            store.account_to_plan.len(),
            store.api_key_to_account.len()
        );

        Ok(store)
    }

    /// Perform delta load of changes since last load using ChangeLog table.
    pub fn load_delta(&self, store: &mut AccountStore) -> Result<(), rusqlite::Error> {
        let conn = self.open_connection()?;

        let last_change_id = store.max_change_id();

        // Query ChangeLog for new entries
        let mut stmt = conn.prepare(
            "SELECT change_id, table_name, record_id, operation FROM ChangeLog WHERE change_id > ? ORDER BY change_id"
        )?;
        let entries = stmt.query_map([last_change_id], |row| {
            Ok(ChangeLogEntry {
                change_id: row.get(0)?,
                table_name: row.get(1)?,
                record_id: row.get(2)?,
                operation: row.get(3)?,
            })
        })?;

        let mut inserts = 0;
        let mut updates = 0;
        let mut deletes = 0;
        let mut max_processed_id = last_change_id;

        for entry_result in entries {
            let entry = entry_result?;
            max_processed_id = entry.change_id;

            match (entry.table_name.as_str(), entry.operation.as_str()) {
                ("Plans", "DELETE") => {
                    store.delete_plan(entry.record_id);
                    deletes += 1;
                }
                ("Plans", _) => {
                    // INSERT or UPDATE: fetch and upsert
                    if let Some(plan) = self.fetch_plan(&conn, entry.record_id)? {
                        store.upsert_plan(plan);
                        if entry.operation == "INSERT" {
                            inserts += 1;
                        } else {
                            updates += 1;
                        }
                    }
                }
                ("Accounts", "DELETE") => {
                    store.delete_account(entry.record_id);
                    deletes += 1;
                }
                ("Accounts", _) => {
                    if let Some(account) = self.fetch_account(&conn, entry.record_id)? {
                        store.upsert_account(account);
                        if entry.operation == "INSERT" {
                            inserts += 1;
                        } else {
                            updates += 1;
                        }
                    }
                }
                ("APIKeys", "DELETE") => {
                    store.delete_api_key(entry.record_id);
                    deletes += 1;
                }
                ("APIKeys", _) => {
                    if let Some(api_key) = self.fetch_api_key(&conn, entry.record_id)? {
                        store.upsert_api_key(api_key);
                        if entry.operation == "INSERT" {
                            inserts += 1;
                        } else {
                            updates += 1;
                        }
                    }
                }
                _ => {
                    log::warn!(
                        "Unknown table in ChangeLog: {} (change_id={})",
                        entry.table_name,
                        entry.change_id
                    );
                }
            }
        }

        if max_processed_id > last_change_id {
            store.set_max_change_id(max_processed_id);
            log::info!(
                "Delta loaded {} inserts, {} updates, {} deletes (change_id: {} -> {})",
                inserts,
                updates,
                deletes,
                last_change_id,
                max_processed_id
            );
        }

        Ok(())
    }

    /// Fetch a single plan by ID.
    fn fetch_plan(&self, conn: &Connection, plan_id: i64) -> Result<Option<Plan>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT plan_id, name, monthly_quota, rps_limit, price_per_1k_req FROM Plans WHERE plan_id = ?"
        )?;
        let mut rows = stmt.query([plan_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Plan {
                plan_id: row.get(0)?,
                name: row.get(1)?,
                monthly_quota: row.get(2)?,
                rps_limit: row.get(3)?,
                price_per_1k_req: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Fetch a single account by ID.
    fn fetch_account(
        &self,
        conn: &Connection,
        account_id: i64,
    ) -> Result<Option<Account>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT account_id, email, plan_id, billing_status FROM Accounts WHERE account_id = ?",
        )?;
        let mut rows = stmt.query([account_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Account {
                account_id: row.get(0)?,
                email: row.get(1)?,
                plan_id: row.get(2)?,
                billing_status: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Fetch a single API key by ID.
    fn fetch_api_key(
        &self,
        conn: &Connection,
        key_id: i64,
    ) -> Result<Option<ApiKey>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT key_id, account_id, api_key_hash, is_active FROM APIKeys WHERE key_id = ?",
        )?;
        let mut rows = stmt.query([key_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ApiKey {
                key_id: row.get(0)?,
                account_id: row.get(1)?,
                api_key_hash: row.get(2)?,
                is_active: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }
}

// ============================================================================
// Background Data Service
// ============================================================================

/// Background service that periodically refreshes account data.
pub struct AccountDataService {
    loader: AccountLoader,
    store: Arc<RwLock<AccountStore>>,
}

impl AccountDataService {
    /// Create a new background service.
    pub fn new(loader: AccountLoader, store: Arc<RwLock<AccountStore>>) -> Self {
        Self { loader, store }
    }
}

#[async_trait]
impl BackgroundService for AccountDataService {
    async fn start(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            // Check for shutdown signal
            if *shutdown.borrow() {
                return;
            }

            // Wait for 30 seconds or shutdown
            tokio::select! {
                _ = shutdown.changed() => {
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    // Continue to reload
                }
            }

            // Perform delta load
            let mut store = self.store.write().unwrap();
            if let Err(e) = self.loader.load_delta(&mut store) {
                log::error!("Failed to load account data: {}", e);
            }
        }
    }
}

// ============================================================================
// Rate Limiter Implementation
// ============================================================================

/// Default rate limit for unknown keys (restrictive).
const DEFAULT_RPS_LIMIT: isize = 1;
const DEFAULT_WINDOW_SECS: u64 = 1;

/// Hash an API key using SHA-256.
pub fn hash_api_key(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Rate limiter that uses account data from SQLite.
pub struct AccountRatelimit {
    store: Arc<RwLock<AccountStore>>,
}

impl AccountRatelimit {
    /// Create a new rate limiter with the given store.
    pub fn new(store: Arc<RwLock<AccountStore>>) -> Self {
        Self { store }
    }

    /// Create and initialize a rate limiter from a database path.
    /// Returns the rate limiter and the background service that should be spawned.
    pub fn from_db<P: AsRef<Path>>(
        db_path: P,
    ) -> Result<(Self, AccountDataService), rusqlite::Error> {
        let loader = AccountLoader::new(&db_path);
        let store = Arc::new(RwLock::new(loader.load_initial()?));
        let service = AccountDataService::new(AccountLoader::new(&db_path), store.clone());
        Ok((Self::new(store), service))
    }

    /// Get the full context for a given API key hash: (account_id, key_id, plan_id).
    /// Used for usage tracking.
    pub fn get_key_context(&self, api_key_hash: &str) -> Option<(i64, i64, i64)> {
        let store = self.store.read().unwrap();
        store.get_key_context(api_key_hash)
    }
}

impl Ratelimit for AccountRatelimit {
    fn limit_for_key(&self, api_key: &str) -> Limit {
        let api_key_hash = hash_api_key(api_key);
        let store = self.store.read().unwrap();

        match store.get_plan_for_key(&api_key_hash) {
            Some(plan) => Limit {
                quota: plan.rps_limit as isize,
                per_seconds: DEFAULT_WINDOW_SECS,
            },
            None => Limit {
                quota: DEFAULT_RPS_LIMIT,
                per_seconds: DEFAULT_WINDOW_SECS,
            },
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> NamedTempFile {
        let file = NamedTempFile::new().unwrap();
        let conn = Connection::open(file.path()).unwrap();

        conn.execute_batch(
            r#"
            CREATE TABLE Plans (
                plan_id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                monthly_quota INTEGER NOT NULL,
                rps_limit INTEGER NOT NULL,
                price_per_1k_req REAL NOT NULL,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE Accounts (
                account_id INTEGER PRIMARY KEY AUTOINCREMENT,
                email TEXT UNIQUE NOT NULL,
                plan_id INTEGER NOT NULL,
                billing_status TEXT NOT NULL,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (plan_id) REFERENCES Plans(plan_id)
            );
            CREATE TABLE APIKeys (
                key_id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id INTEGER NOT NULL,
                api_key_hash TEXT UNIQUE NOT NULL,
                is_active BOOLEAN NOT NULL DEFAULT 1,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
            );
            CREATE TABLE ChangeLog (
                change_id INTEGER PRIMARY KEY AUTOINCREMENT,
                table_name TEXT NOT NULL,
                record_id INTEGER NOT NULL,
                operation TEXT NOT NULL,
                occurred_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            -- Plans triggers
            CREATE TRIGGER trg_plans_insert AFTER INSERT ON Plans BEGIN
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', NEW.plan_id, 'INSERT');
            END;
            CREATE TRIGGER trg_plans_update AFTER UPDATE ON Plans BEGIN
                UPDATE Plans SET updated_at = CURRENT_TIMESTAMP WHERE plan_id = NEW.plan_id;
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', NEW.plan_id, 'UPDATE');
            END;
            CREATE TRIGGER trg_plans_delete AFTER DELETE ON Plans BEGIN
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', OLD.plan_id, 'DELETE');
            END;

            -- Accounts triggers
            CREATE TRIGGER trg_accounts_insert AFTER INSERT ON Accounts BEGIN
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', NEW.account_id, 'INSERT');
            END;
            CREATE TRIGGER trg_accounts_update AFTER UPDATE ON Accounts BEGIN
                UPDATE Accounts SET updated_at = CURRENT_TIMESTAMP WHERE account_id = NEW.account_id;
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', NEW.account_id, 'UPDATE');
            END;
            CREATE TRIGGER trg_accounts_delete AFTER DELETE ON Accounts BEGIN
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', OLD.account_id, 'DELETE');
            END;

            -- APIKeys triggers
            CREATE TRIGGER trg_apikeys_insert AFTER INSERT ON APIKeys BEGIN
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', NEW.key_id, 'INSERT');
            END;
            CREATE TRIGGER trg_apikeys_update AFTER UPDATE ON APIKeys BEGIN
                UPDATE APIKeys SET updated_at = CURRENT_TIMESTAMP WHERE key_id = NEW.key_id;
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', NEW.key_id, 'UPDATE');
            END;
            CREATE TRIGGER trg_apikeys_delete AFTER DELETE ON APIKeys BEGIN
                INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', OLD.key_id, 'DELETE');
            END;

            INSERT INTO Plans (name, monthly_quota, rps_limit, price_per_1k_req)
            VALUES ('Free', 1000, 5, 0.0);
            INSERT INTO Plans (name, monthly_quota, rps_limit, price_per_1k_req)
            VALUES ('Pro', 100000, 100, 0.001);

            INSERT INTO Accounts (email, plan_id, billing_status)
            VALUES ('free@example.com', 1, 'active');
            INSERT INTO Accounts (email, plan_id, billing_status)
            VALUES ('pro@example.com', 2, 'active');

            INSERT INTO APIKeys (account_id, api_key_hash, is_active)
            VALUES (1, 'hash_free_key', 1);
            INSERT INTO APIKeys (account_id, api_key_hash, is_active)
            VALUES (2, 'hash_pro_key', 1);
            INSERT INTO APIKeys (account_id, api_key_hash, is_active)
            VALUES (1, 'hash_inactive_key', 0);
            "#,
        )
        .unwrap();

        file
    }

    #[test]
    fn test_account_store_lookup() {
        let mut store = AccountStore::new();

        store.upsert_plan(Plan {
            plan_id: 1,
            name: "Free".to_string(),
            monthly_quota: 1000,
            rps_limit: 5,
            price_per_1k_req: 0.0,
        });

        store.upsert_account(Account {
            account_id: 1,
            email: "test@example.com".to_string(),
            plan_id: 1,
            billing_status: "active".to_string(),
        });

        store.upsert_api_key(ApiKey {
            key_id: 1,
            account_id: 1,
            api_key_hash: "test_hash".to_string(),
            is_active: true,
        });

        let plan = store.get_plan_for_key("test_hash").unwrap();
        assert_eq!(plan.name, "Free");
        assert_eq!(plan.rps_limit, 5);
    }

    #[test]
    fn test_account_store_inactive_key_not_found() {
        let mut store = AccountStore::new();

        store.upsert_plan(Plan {
            plan_id: 1,
            name: "Free".to_string(),
            monthly_quota: 1000,
            rps_limit: 5,
            price_per_1k_req: 0.0,
        });

        store.upsert_account(Account {
            account_id: 1,
            email: "test@example.com".to_string(),
            plan_id: 1,
            billing_status: "active".to_string(),
        });

        store.upsert_api_key(ApiKey {
            key_id: 1,
            account_id: 1,
            api_key_hash: "inactive_hash".to_string(),
            is_active: false,
        });

        assert!(store.get_plan_for_key("inactive_hash").is_none());
    }

    #[test]
    fn test_load_initial() {
        let db = create_test_db();
        let loader = AccountLoader::new(db.path());
        let store = loader.load_initial().unwrap();

        assert_eq!(store.plans.len(), 2);
        assert_eq!(store.account_to_plan.len(), 2);
        // Only 2 active keys
        assert_eq!(store.api_key_to_account.len(), 2);

        let free_plan = store.get_plan_for_key("hash_free_key").unwrap();
        assert_eq!(free_plan.name, "Free");
        assert_eq!(free_plan.rps_limit, 5);

        let pro_plan = store.get_plan_for_key("hash_pro_key").unwrap();
        assert_eq!(pro_plan.name, "Pro");
        assert_eq!(pro_plan.rps_limit, 100);
    }

    #[test]
    fn test_delta_loading() {
        let db = create_test_db();
        let loader = AccountLoader::new(db.path());
        let mut store = loader.load_initial().unwrap();

        // After initial load, max_change_id should reflect all initial inserts
        // 2 plans + 2 accounts + 3 keys = 7 change log entries
        assert_eq!(store.max_change_id(), 7);

        // Insert new records (triggers will create ChangeLog entries)
        let conn = Connection::open(db.path()).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO Plans (name, monthly_quota, rps_limit, price_per_1k_req)
            VALUES ('Enterprise', 1000000, 1000, 0.0001);
            INSERT INTO Accounts (email, plan_id, billing_status)
            VALUES ('enterprise@example.com', 3, 'active');
            INSERT INTO APIKeys (account_id, api_key_hash, is_active)
            VALUES (3, 'hash_enterprise_key', 1);
            "#,
        )
        .unwrap();

        // Delta load
        loader.load_delta(&mut store).unwrap();

        assert_eq!(store.plans.len(), 3);
        // 7 + 3 new inserts = 10
        assert_eq!(store.max_change_id(), 10);

        let enterprise_plan = store.get_plan_for_key("hash_enterprise_key").unwrap();
        assert_eq!(enterprise_plan.name, "Enterprise");
        assert_eq!(enterprise_plan.rps_limit, 1000);
    }

    #[test]
    fn test_account_ratelimit_known_key() {
        let db = create_test_db();
        let loader = AccountLoader::new(db.path());
        let store = Arc::new(RwLock::new(loader.load_initial().unwrap()));
        let limiter = AccountRatelimit::new(store);

        // The hash_pro_key has rps_limit of 100
        let store = limiter.store.read().unwrap();
        let plan = store.get_plan_for_key("hash_pro_key").unwrap();
        assert_eq!(plan.rps_limit, 100);
    }

    #[test]
    fn test_account_ratelimit_unknown_key() {
        let db = create_test_db();
        let loader = AccountLoader::new(db.path());
        let store = Arc::new(RwLock::new(loader.load_initial().unwrap()));
        let limiter = AccountRatelimit::new(store);

        // Unknown key should return restrictive defaults
        let limit = limiter.limit_for_key("unknown_api_key_12345");
        assert_eq!(limit.quota, DEFAULT_RPS_LIMIT);
        assert_eq!(limit.per_seconds, DEFAULT_WINDOW_SECS);
    }

    #[test]
    fn test_hash_api_key() {
        let hash1 = hash_api_key("test-key-123");
        let hash2 = hash_api_key("test-key-123");
        let hash3 = hash_api_key("different-key");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex characters
    }
}
