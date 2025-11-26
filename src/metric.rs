use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// In-memory per-minute status counts keyed by API key.
#[derive(Default)]
pub struct Metrics {
    counts: std::sync::Mutex<HashMap<String, HashMap<u64, HashMap<u16, u64>>>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a status code occurrence using the current wall-clock time.
    pub fn record(&self, api_key: &str, status: u16) {
        self.record_at(api_key, status, SystemTime::now());
    }

    /// Record a status code occurrence at a provided time (useful for tests).
    pub fn record_at(&self, api_key: &str, status: u16, at: SystemTime) {
        let minute = Self::minute_bucket(at);
        let mut guard = self.counts.lock().expect("metrics store poisoned");
        let per_key = guard.entry(api_key.to_string()).or_default();
        let per_minute = per_key.entry(minute).or_default();
        *per_minute.entry(status).or_insert(0) += 1;
    }

    /// Snapshot counts for a given API key. Returns an empty map when the key is unknown.
    pub fn snapshot(&self, api_key: &str) -> HashMap<u64, HashMap<u16, u64>> {
        self.counts
            .lock()
            .expect("metrics store poisoned")
            .get(api_key)
            .cloned()
            .unwrap_or_default()
    }

    fn minute_bucket(at: SystemTime) -> u64 {
        at.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
            / 60
    }
}
