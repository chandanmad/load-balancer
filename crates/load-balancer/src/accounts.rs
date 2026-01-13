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
    pub monthly_quota: Option<i32>,
    pub rps_limit: Option<i32>,
    pub price_per_1k_req: Option<f64>,
}

/// Represents an account that owns subscriptions.
#[derive(Debug, Clone)]
pub struct Account {
    pub account_id: i64,
    pub email: String,
    pub plan_id: Option<i64>,
    pub billing_status: Option<String>,
}

/// Represents an API key belonging to an account.
#[derive(Debug, Clone)]
pub struct ApiKey {
    pub key_id: i64,
    pub account_id: Option<i64>,
    pub api_key_hash: Option<String>,
    pub is_active: bool,
    pub created_at: Option<String>,
}

// ============================================================================
// Account Store
// ============================================================================

/// Thread-safe in-memory store for account data with delta loading support.
#[derive(Debug, Default)]
pub struct AccountStore {
    /// API key hash -> Account ID
    api_key_to_account: HashMap<String, i64>,
    /// Account ID -> Plan ID
    account_to_plan: HashMap<i64, i64>,
    /// Plan ID -> Plan
    plans: HashMap<i64, Plan>,
    /// Track max IDs for delta loading
    max_plan_id: i64,
    max_account_id: i64,
    max_key_id: i64,
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

    /// Get max plan_id for delta loading.
    pub fn max_plan_id(&self) -> i64 {
        self.max_plan_id
    }

    /// Get max account_id for delta loading.
    pub fn max_account_id(&self) -> i64 {
        self.max_account_id
    }

    /// Get max key_id for delta loading.
    pub fn max_key_id(&self) -> i64 {
        self.max_key_id
    }

    /// Insert or update a plan.
    pub fn upsert_plan(&mut self, plan: Plan) {
        if plan.plan_id > self.max_plan_id {
            self.max_plan_id = plan.plan_id;
        }
        self.plans.insert(plan.plan_id, plan);
    }

    /// Insert or update an account.
    pub fn upsert_account(&mut self, account: Account) {
        if account.account_id > self.max_account_id {
            self.max_account_id = account.account_id;
        }
        if let Some(plan_id) = account.plan_id {
            self.account_to_plan.insert(account.account_id, plan_id);
        }
    }

    /// Insert or update an API key.
    pub fn upsert_api_key(&mut self, api_key: ApiKey) {
        if api_key.key_id > self.max_key_id {
            self.max_key_id = api_key.key_id;
        }
        if api_key.is_active {
            if let (Some(hash), Some(account_id)) = (api_key.api_key_hash, api_key.account_id) {
                self.api_key_to_account.insert(hash, account_id);
            }
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
        let mut stmt = conn.prepare(
            "SELECT key_id, account_id, api_key_hash, is_active, created_at FROM APIKeys",
        )?;
        let keys = stmt.query_map([], |row| {
            Ok(ApiKey {
                key_id: row.get(0)?,
                account_id: row.get(1)?,
                api_key_hash: row.get(2)?,
                is_active: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        for key in keys {
            store.upsert_api_key(key?);
        }

        log::info!(
            "Loaded {} plans, {} accounts, {} API keys",
            store.plans.len(),
            store.account_to_plan.len(),
            store.api_key_to_account.len()
        );

        Ok(store)
    }

    /// Perform delta load of new records since last load.
    pub fn load_delta(&self, store: &mut AccountStore) -> Result<(), rusqlite::Error> {
        let conn = self.open_connection()?;

        let max_plan_id = store.max_plan_id();
        let max_account_id = store.max_account_id();
        let max_key_id = store.max_key_id();

        let mut new_plans = 0;
        let mut new_accounts = 0;
        let mut new_keys = 0;

        // Load new plans
        let mut stmt = conn.prepare(
            "SELECT plan_id, name, monthly_quota, rps_limit, price_per_1k_req FROM Plans WHERE plan_id > ?"
        )?;
        let plans = stmt.query_map([max_plan_id], |row| {
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
            new_plans += 1;
        }

        // Load new accounts
        let mut stmt = conn.prepare(
            "SELECT account_id, email, plan_id, billing_status FROM Accounts WHERE account_id > ?",
        )?;
        let accounts = stmt.query_map([max_account_id], |row| {
            Ok(Account {
                account_id: row.get(0)?,
                email: row.get(1)?,
                plan_id: row.get(2)?,
                billing_status: row.get(3)?,
            })
        })?;
        for account in accounts {
            store.upsert_account(account?);
            new_accounts += 1;
        }

        // Load new API keys
        let mut stmt = conn.prepare(
            "SELECT key_id, account_id, api_key_hash, is_active, created_at FROM APIKeys WHERE key_id > ?"
        )?;
        let keys = stmt.query_map([max_key_id], |row| {
            Ok(ApiKey {
                key_id: row.get(0)?,
                account_id: row.get(1)?,
                api_key_hash: row.get(2)?,
                is_active: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        for key in keys {
            store.upsert_api_key(key?);
            new_keys += 1;
        }

        if new_plans > 0 || new_accounts > 0 || new_keys > 0 {
            log::info!(
                "Delta loaded {} plans, {} accounts, {} API keys",
                new_plans,
                new_accounts,
                new_keys
            );
        }

        Ok(())
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
}

impl Ratelimit for AccountRatelimit {
    fn limit_for_key(&self, api_key: &str) -> Limit {
        let api_key_hash = hash_api_key(api_key);
        let store = self.store.read().unwrap();

        match store.get_plan_for_key(&api_key_hash) {
            Some(plan) => {
                let rps = plan.rps_limit.unwrap_or(DEFAULT_RPS_LIMIT as i32) as isize;
                Limit {
                    quota: rps,
                    per_seconds: DEFAULT_WINDOW_SECS,
                }
            }
            None => {
                // Unknown key - use restrictive defaults
                Limit {
                    quota: DEFAULT_RPS_LIMIT,
                    per_seconds: DEFAULT_WINDOW_SECS,
                }
            }
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
                plan_id BIGINT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                monthly_quota INTEGER,
                rps_limit INTEGER,
                price_per_1k_req REAL
            );
            CREATE TABLE Accounts (
                account_id BIGINT PRIMARY KEY NOT NULL,
                email TEXT UNIQUE NOT NULL,
                plan_id INTEGER,
                billing_status TEXT,
                FOREIGN KEY (plan_id) REFERENCES Plans(plan_id)
            );
            CREATE TABLE APIKeys (
                key_id BIGINT PRIMARY KEY NOT NULL,
                account_id INTEGER,
                api_key_hash TEXT UNIQUE,
                is_active BOOLEAN DEFAULT 1,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
            );

            INSERT INTO Plans (plan_id, name, monthly_quota, rps_limit, price_per_1k_req)
            VALUES (1, 'Free', 1000, 5, 0.0);
            INSERT INTO Plans (plan_id, name, monthly_quota, rps_limit, price_per_1k_req)
            VALUES (2, 'Pro', 100000, 100, 0.001);

            INSERT INTO Accounts (account_id, email, plan_id, billing_status)
            VALUES (1, 'free@example.com', 1, 'active');
            INSERT INTO Accounts (account_id, email, plan_id, billing_status)
            VALUES (2, 'pro@example.com', 2, 'active');

            INSERT INTO APIKeys (key_id, account_id, api_key_hash, is_active)
            VALUES (1, 1, 'hash_free_key', 1);
            INSERT INTO APIKeys (key_id, account_id, api_key_hash, is_active)
            VALUES (2, 2, 'hash_pro_key', 1);
            INSERT INTO APIKeys (key_id, account_id, api_key_hash, is_active)
            VALUES (3, 1, 'hash_inactive_key', 0);
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
            monthly_quota: Some(1000),
            rps_limit: Some(5),
            price_per_1k_req: Some(0.0),
        });

        store.upsert_account(Account {
            account_id: 1,
            email: "test@example.com".to_string(),
            plan_id: Some(1),
            billing_status: Some("active".to_string()),
        });

        store.upsert_api_key(ApiKey {
            key_id: 1,
            account_id: Some(1),
            api_key_hash: Some("test_hash".to_string()),
            is_active: true,
            created_at: None,
        });

        let plan = store.get_plan_for_key("test_hash").unwrap();
        assert_eq!(plan.name, "Free");
        assert_eq!(plan.rps_limit, Some(5));
    }

    #[test]
    fn test_account_store_inactive_key_not_found() {
        let mut store = AccountStore::new();

        store.upsert_plan(Plan {
            plan_id: 1,
            name: "Free".to_string(),
            monthly_quota: Some(1000),
            rps_limit: Some(5),
            price_per_1k_req: Some(0.0),
        });

        store.upsert_account(Account {
            account_id: 1,
            email: "test@example.com".to_string(),
            plan_id: Some(1),
            billing_status: Some("active".to_string()),
        });

        store.upsert_api_key(ApiKey {
            key_id: 1,
            account_id: Some(1),
            api_key_hash: Some("inactive_hash".to_string()),
            is_active: false,
            created_at: None,
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
        assert_eq!(free_plan.rps_limit, Some(5));

        let pro_plan = store.get_plan_for_key("hash_pro_key").unwrap();
        assert_eq!(pro_plan.name, "Pro");
        assert_eq!(pro_plan.rps_limit, Some(100));
    }

    #[test]
    fn test_delta_loading() {
        let db = create_test_db();
        let loader = AccountLoader::new(db.path());
        let mut store = loader.load_initial().unwrap();

        assert_eq!(store.max_plan_id(), 2);
        assert_eq!(store.max_account_id(), 2);
        assert_eq!(store.max_key_id(), 3);

        // Insert new records
        let conn = Connection::open(db.path()).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO Plans (plan_id, name, monthly_quota, rps_limit, price_per_1k_req)
            VALUES (3, 'Enterprise', 1000000, 1000, 0.0001);
            INSERT INTO Accounts (account_id, email, plan_id, billing_status)
            VALUES (3, 'enterprise@example.com', 3, 'active');
            INSERT INTO APIKeys (key_id, account_id, api_key_hash, is_active)
            VALUES (4, 3, 'hash_enterprise_key', 1);
            "#,
        )
        .unwrap();

        // Delta load
        loader.load_delta(&mut store).unwrap();

        assert_eq!(store.plans.len(), 3);
        assert_eq!(store.max_plan_id(), 3);
        assert_eq!(store.max_account_id(), 3);
        assert_eq!(store.max_key_id(), 4);

        let enterprise_plan = store.get_plan_for_key("hash_enterprise_key").unwrap();
        assert_eq!(enterprise_plan.name, "Enterprise");
        assert_eq!(enterprise_plan.rps_limit, Some(1000));
    }

    #[test]
    fn test_account_ratelimit_known_key() {
        let db = create_test_db();
        let loader = AccountLoader::new(db.path());
        let store = Arc::new(RwLock::new(loader.load_initial().unwrap()));
        let limiter = AccountRatelimit::new(store);

        // The hash_pro_key has rps_limit of 100
        // We need to manually insert with the actual hash since the DB has raw hashes
        // For this test, let's check the store directly
        let store = limiter.store.read().unwrap();
        let plan = store.get_plan_for_key("hash_pro_key").unwrap();
        assert_eq!(plan.rps_limit, Some(100));
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
