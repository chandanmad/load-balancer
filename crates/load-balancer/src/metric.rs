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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minute_bucket_groups_by_60_seconds() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(59);
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
        assert_eq!(Metrics::minute_bucket(t0), 0);
        assert_eq!(Metrics::minute_bucket(t1), 1);
    }

    #[test]
    fn record_and_snapshot_counts() {
        let metrics = Metrics::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(65);

        metrics.record_at("k", 200, t0);
        metrics.record_at("k", 429, t0);
        metrics.record_at("k", 200, t1);

        let snap = metrics.snapshot("k");
        let first_min = snap.get(&0).unwrap();
        let second_min = snap.get(&1).unwrap();

        assert_eq!(first_min.get(&200), Some(&1));
        assert_eq!(first_min.get(&429), Some(&1));
        assert_eq!(second_min.get(&200), Some(&1));
    }

    #[test]
    fn snapshot_unknown_key_is_empty() {
        let metrics = Metrics::new();
        assert!(metrics.snapshot("missing").is_empty());
    }
}
