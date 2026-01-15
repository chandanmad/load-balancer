//! API usage tracking with minute-level granularity and hourly SQLite dumps.
//!
//! This module captures per-request metrics (request count, response data size) grouped by
//! (account_id, key_id, plan_id, minute). Every hour, the data is flushed to a timestamped
//! SQLite database file (`usage-<YYYYMMDDHH>.db`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use pingora::services::background::BackgroundService;
use rusqlite::Connection;

// ============================================================================
// Data Structures
// ============================================================================

/// Composite key for usage aggregation: (account_id, key_id, plan_id, minute_timestamp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageKey {
    pub account_id: i64,
    pub key_id: i64,
    pub plan_id: i64,
    /// Unix timestamp truncated to the start of the minute.
    pub minute_ts: i64,
}

/// Mutable counters for a single usage key.
#[derive(Debug, Clone, Default)]
pub struct UsageRecord {
    pub total_requests: u64,
    pub total_data_bytes: u64,
}

// ============================================================================
// Usage Tracker
// ============================================================================

/// Thread-safe in-memory aggregator for usage data.
#[derive(Debug)]
pub struct UsageTracker {
    /// Map from usage key to aggregated record.
    data: RwLock<HashMap<UsageKey, UsageRecord>>,
    /// Output directory for shutdown flush (optional).
    output_dir: RwLock<Option<PathBuf>>,
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            output_dir: RwLock::new(None),
        }
    }
}

impl UsageTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the output directory for shutdown flush.
    pub fn set_output_dir(&self, path: impl AsRef<Path>) {
        let mut dir = self.output_dir.write().unwrap();
        *dir = Some(path.as_ref().to_path_buf());
    }

    /// Record a single request's usage.
    ///
    /// - `account_id`, `key_id`, `plan_id`: identifiers from AccountStore
    /// - `response_bytes`: size of the response body in bytes
    /// - `timestamp_secs`: Unix timestamp of the request (seconds since epoch)
    pub fn record(
        &self,
        account_id: i64,
        key_id: i64,
        plan_id: i64,
        response_bytes: u64,
        timestamp_secs: i64,
    ) {
        // Truncate to minute boundary
        let minute_ts = timestamp_secs - (timestamp_secs % 60);

        let key = UsageKey {
            account_id,
            key_id,
            plan_id,
            minute_ts,
        };

        let mut data = self.data.write().unwrap();
        let record = data.entry(key).or_default();
        record.total_requests += 1;
        record.total_data_bytes += response_bytes;
    }

    /// Extract all records for a given hour and remove them from the tracker.
    ///
    /// `hour_ts` is the Unix timestamp at the start of the hour (must be aligned to hour).
    pub fn drain_hour(&self, hour_ts: i64) -> Vec<(UsageKey, UsageRecord)> {
        let hour_end = hour_ts + 3600;

        let mut data = self.data.write().unwrap();
        let mut drained = Vec::new();

        data.retain(|key, record| {
            if key.minute_ts >= hour_ts && key.minute_ts < hour_end {
                drained.push((*key, record.clone()));
                false // remove from map
            } else {
                true // keep in map
            }
        });

        drained
    }

    /// Drain all records regardless of hour. Used for shutdown flush.
    pub fn drain_all(&self) -> Vec<(UsageKey, UsageRecord)> {
        let mut data = self.data.write().unwrap();
        data.drain().collect()
    }

    /// Flush all remaining data to disk. Called on drop.
    fn flush_to_disk(&self) {
        let output_dir = {
            let dir = self.output_dir.read().unwrap();
            dir.clone()
        };

        if let Some(output_dir) = output_dir {
            let all_records = self.drain_all();
            if all_records.is_empty() {
                return;
            }

            // Group by hour
            let mut by_hour: HashMap<i64, Vec<(UsageKey, UsageRecord)>> = HashMap::new();
            for (key, record) in all_records {
                let hour_ts = key.minute_ts - (key.minute_ts % 3600);
                by_hour.entry(hour_ts).or_default().push((key, record));
            }

            for (hour_ts, records) in by_hour {
                if let Err(e) = write_records_to_db(&output_dir, hour_ts, &records) {
                    log::error!("Failed to flush usage data on drop: {}", e);
                } else {
                    log::info!("Flushed {} usage records on drop", records.len());
                }
            }
        }
    }
}

impl Drop for UsageTracker {
    fn drop(&mut self) {
        self.flush_to_disk();
    }
}

/// Write records to the SQLite database for a given hour.
fn write_records_to_db(
    output_dir: &Path,
    hour_ts: i64,
    records: &[(UsageKey, UsageRecord)],
) -> Result<(), rusqlite::Error> {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(hour_ts as u64);
    let datetime: chrono::DateTime<chrono::Utc> = datetime.into();
    let filename = format!("usage-{}.db", datetime.format("%Y%m%d%H"));
    let db_path = output_dir.join(&filename);

    // Create directory if it doesn't exist
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let conn = Connection::open(&db_path)?;

    // Create table if it doesn't exist
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS Usage (
            account_id BIGINT NOT NULL,
            key_id BIGINT NOT NULL,
            plan_id BIGINT NOT NULL,
            date_time DATETIME NOT NULL,
            total_requests INTEGER,
            total_data_mb REAL,
            PRIMARY KEY (account_id, key_id, plan_id, date_time)
        );
        "#,
    )?;

    // Insert or update records
    let mut stmt = conn.prepare(
        r#"
        INSERT INTO Usage (account_id, key_id, plan_id, date_time, total_requests, total_data_mb)
        VALUES (?1, ?2, ?3, datetime(?4, 'unixepoch'), ?5, ?6)
        ON CONFLICT(account_id, key_id, plan_id, date_time)
        DO UPDATE SET
            total_requests = total_requests + excluded.total_requests,
            total_data_mb = total_data_mb + excluded.total_data_mb
        "#,
    )?;

    for (key, record) in records {
        let data_mb = record.total_data_bytes as f64 / (1024.0 * 1024.0);
        stmt.execute(rusqlite::params![
            key.account_id,
            key.key_id,
            key.plan_id,
            key.minute_ts,
            record.total_requests as i64,
            data_mb,
        ])?;
    }

    log::info!(
        "Flushed {} usage records to {}",
        records.len(),
        db_path.display()
    );

    Ok(())
}

// ============================================================================
// Usage Writer
// ============================================================================

/// Background service that periodically flushes usage data to SQLite files.
pub struct UsageWriter {
    tracker: Arc<UsageTracker>,
    output_dir: PathBuf,
    /// Tracks the last hour we flushed (Unix timestamp at hour start).
    last_flushed_hour: RwLock<Option<i64>>,
}

impl UsageWriter {
    /// Create a new writer that flushes data from `tracker` to `output_dir`.
    pub fn new(tracker: Arc<UsageTracker>, output_dir: impl AsRef<Path>) -> Self {
        // Set the output dir on the tracker for Drop-based flush
        tracker.set_output_dir(output_dir.as_ref());

        Self {
            tracker,
            output_dir: output_dir.as_ref().to_path_buf(),
            last_flushed_hour: RwLock::new(None),
        }
    }

    /// Get the current hour timestamp (Unix timestamp at hour start).
    fn current_hour_ts() -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now - (now % 3600)
    }

    /// Generate the database filename for a given hour timestamp.
    fn db_filename(hour_ts: i64) -> String {
        use std::time::{Duration, UNIX_EPOCH};

        let datetime = UNIX_EPOCH + Duration::from_secs(hour_ts as u64);
        let datetime: chrono::DateTime<chrono::Utc> = datetime.into();
        format!("usage-{}.db", datetime.format("%Y%m%d%H"))
    }

    /// Flush records for a specific hour to a SQLite file.
    pub fn flush_hour(&self, hour_ts: i64) -> Result<usize, rusqlite::Error> {
        let records = self.tracker.drain_hour(hour_ts);
        if records.is_empty() {
            return Ok(0);
        }

        self.write_records_to_db(hour_ts, &records)?;
        Ok(records.len())
    }

    /// Flush all remaining records (for shutdown). Groups by hour and writes each.
    pub fn flush_all(&self) -> Result<usize, rusqlite::Error> {
        let all_records = self.tracker.drain_all();
        if all_records.is_empty() {
            return Ok(0);
        }

        // Group by hour
        let mut by_hour: HashMap<i64, Vec<(UsageKey, UsageRecord)>> = HashMap::new();
        for (key, record) in all_records {
            let hour_ts = key.minute_ts - (key.minute_ts % 3600);
            by_hour.entry(hour_ts).or_default().push((key, record));
        }

        let mut total = 0;
        for (hour_ts, records) in by_hour {
            self.write_records_to_db(hour_ts, &records)?;
            total += records.len();
        }

        Ok(total)
    }

    /// Write records to the SQLite database for a given hour.
    fn write_records_to_db(
        &self,
        hour_ts: i64,
        records: &[(UsageKey, UsageRecord)],
    ) -> Result<(), rusqlite::Error> {
        let filename = Self::db_filename(hour_ts);
        let db_path = self.output_dir.join(&filename);

        // Create directory if it doesn't exist
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(&db_path)?;

        // Create table if it doesn't exist
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS Usage (
                account_id BIGINT NOT NULL,
                key_id BIGINT NOT NULL,
                plan_id BIGINT NOT NULL,
                date_time DATETIME NOT NULL,
                total_requests INTEGER,
                total_data_mb REAL,
                PRIMARY KEY (account_id, key_id, plan_id, date_time)
            );
            "#,
        )?;

        // Insert or update records
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO Usage (account_id, key_id, plan_id, date_time, total_requests, total_data_mb)
            VALUES (?1, ?2, ?3, datetime(?4, 'unixepoch'), ?5, ?6)
            ON CONFLICT(account_id, key_id, plan_id, date_time)
            DO UPDATE SET
                total_requests = total_requests + excluded.total_requests,
                total_data_mb = total_data_mb + excluded.total_data_mb
            "#,
        )?;

        for (key, record) in records {
            let data_mb = record.total_data_bytes as f64 / (1024.0 * 1024.0);
            stmt.execute(rusqlite::params![
                key.account_id,
                key.key_id,
                key.plan_id,
                key.minute_ts,
                record.total_requests as i64,
                data_mb,
            ])?;
        }

        log::info!(
            "Flushed {} usage records to {}",
            records.len(),
            db_path.display()
        );

        Ok(())
    }
}

#[async_trait]
impl BackgroundService for UsageWriter {
    async fn start(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        // Initialize last flushed hour
        {
            let mut last = self.last_flushed_hour.write().unwrap();
            *last = Some(Self::current_hour_ts());
        }

        loop {
            // Check for shutdown
            if *shutdown.borrow() {
                // Flush all remaining data on shutdown
                if let Err(e) = self.flush_all() {
                    log::error!("Failed to flush usage data on shutdown: {}", e);
                } else {
                    log::info!("Flushed remaining usage data on shutdown");
                }
                return;
            }

            // Wait for 1 minute or shutdown
            tokio::select! {
                _ = shutdown.changed() => {
                    // Shutdown requested - flush all data
                    if let Err(e) = self.flush_all() {
                        log::error!("Failed to flush usage data on shutdown: {}", e);
                    } else {
                        log::info!("Flushed remaining usage data on shutdown");
                    }
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    // Check if we crossed an hour boundary
                    let current_hour = Self::current_hour_ts();
                    let last_hour = {
                        let last = self.last_flushed_hour.read().unwrap();
                        *last
                    };

                    if let Some(last) = last_hour {
                        if current_hour > last {
                            // New hour - flush the previous hour
                            if let Err(e) = self.flush_hour(last) {
                                log::error!("Failed to flush usage data for hour {}: {}", last, e);
                            }

                            // Update last flushed hour
                            let mut last_guard = self.last_flushed_hour.write().unwrap();
                            *last_guard = Some(current_hour);
                        }
                    }
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
    use tempfile::TempDir;

    #[test]
    fn test_usage_tracker_record_increments_counts() {
        let tracker = UsageTracker::new();

        // Record 3 requests
        tracker.record(1, 10, 100, 1024, 1000);
        tracker.record(1, 10, 100, 2048, 1001);
        tracker.record(1, 10, 100, 512, 1002);

        let records = tracker.drain_all();
        assert_eq!(records.len(), 1);

        let (key, record) = &records[0];
        assert_eq!(key.account_id, 1);
        assert_eq!(key.key_id, 10);
        assert_eq!(key.plan_id, 100);
        assert_eq!(key.minute_ts, 960); // 1000 truncated to minute
        assert_eq!(record.total_requests, 3);
        assert_eq!(record.total_data_bytes, 1024 + 2048 + 512);
    }

    #[test]
    fn test_usage_tracker_minute_bucketing() {
        let tracker = UsageTracker::new();

        // Record requests in different minutes
        tracker.record(1, 10, 100, 100, 60); // minute 60
        tracker.record(1, 10, 100, 100, 119); // minute 60
        tracker.record(1, 10, 100, 100, 120); // minute 120
        tracker.record(1, 10, 100, 100, 180); // minute 180

        let records = tracker.drain_all();
        assert_eq!(records.len(), 3);

        // Verify we have 3 different minute buckets
        let minute_counts: HashMap<i64, u64> = records
            .iter()
            .map(|(k, r)| (k.minute_ts, r.total_requests))
            .collect();

        assert_eq!(minute_counts.get(&60), Some(&2)); // 2 requests in minute 60
        assert_eq!(minute_counts.get(&120), Some(&1));
        assert_eq!(minute_counts.get(&180), Some(&1));
    }

    #[test]
    fn test_drain_hour() {
        let tracker = UsageTracker::new();

        // Hour 0: timestamps 0-3599
        tracker.record(1, 10, 100, 100, 0);
        tracker.record(1, 10, 100, 100, 1800);
        tracker.record(1, 10, 100, 100, 3599);

        // Hour 1: timestamps 3600-7199
        tracker.record(1, 10, 100, 100, 3600);
        tracker.record(1, 10, 100, 100, 7199);

        // Drain hour 0
        let hour0_records = tracker.drain_hour(0);
        assert_eq!(hour0_records.len(), 3);

        // Drain hour 1
        let hour1_records = tracker.drain_hour(3600);
        assert_eq!(hour1_records.len(), 2);

        // Nothing left
        let remaining = tracker.drain_all();
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_usage_writer_creates_db_with_schema() {
        let tracker = Arc::new(UsageTracker::new());
        let temp_dir = TempDir::new().unwrap();
        let writer = UsageWriter::new(tracker.clone(), temp_dir.path());

        // Record some data
        tracker.record(1, 10, 100, 1024 * 1024, 3600); // 1 MB at hour 1

        // Flush hour 1
        let count = writer.flush_hour(3600).unwrap();
        assert_eq!(count, 1);

        // Verify the database was created
        let db_path = temp_dir.path().join("usage-1970010101.db");
        assert!(db_path.exists());

        // Query the database
        let conn = Connection::open(&db_path).unwrap();
        let mut stmt = conn
            .prepare("SELECT account_id, key_id, plan_id, total_requests, total_data_mb FROM Usage")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();

        let row = rows.next().unwrap().unwrap();
        assert_eq!(row.get::<_, i64>(0).unwrap(), 1); // account_id
        assert_eq!(row.get::<_, i64>(1).unwrap(), 10); // key_id
        assert_eq!(row.get::<_, i64>(2).unwrap(), 100); // plan_id
        assert_eq!(row.get::<_, i64>(3).unwrap(), 1); // total_requests
        assert!((row.get::<_, f64>(4).unwrap() - 1.0).abs() < 0.001); // ~1 MB
    }

    #[test]
    fn test_flush_all_groups_by_hour() {
        let tracker = Arc::new(UsageTracker::new());
        let temp_dir = TempDir::new().unwrap();
        let writer = UsageWriter::new(tracker.clone(), temp_dir.path());

        // Records in hour 0 and hour 1
        tracker.record(1, 10, 100, 100, 0);
        tracker.record(1, 10, 100, 100, 3600);

        let count = writer.flush_all().unwrap();
        assert_eq!(count, 2);

        // Both DB files should exist
        assert!(temp_dir.path().join("usage-1970010100.db").exists());
        assert!(temp_dir.path().join("usage-1970010101.db").exists());
    }
}
