use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceRegistry {
    pub updated_at_epoch_secs: u64,
    pub records: BTreeMap<String, SourceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRecord {
    pub url: String,
    pub source_kind: String,
    pub discovered_from_feeds: Vec<String>,
    pub seen_count: u64,
    pub discover_hits: u64,
    pub feed_scan_count: u64,
    pub fetch_attempts: u64,
    pub fetch_successes: u64,
    pub fetch_empty_results: u64,
    pub total_proxy_count: u64,
    pub max_proxy_count: usize,
    pub last_proxy_count: usize,
    pub total_validated_proxy_count: u64,
    pub last_validated_proxy_count: usize,
    pub total_released_proxy_count: u64,
    pub last_released_proxy_count: usize,
    pub last_error: Option<String>,
    pub first_seen_epoch_secs: u64,
    pub last_seen_epoch_secs: u64,
    pub score: f64,
}

impl Default for SourceRecord {
    fn default() -> Self {
        let now = now_epoch_secs();
        Self {
            url: String::new(),
            source_kind: "unknown".to_string(),
            discovered_from_feeds: Vec::new(),
            seen_count: 0,
            discover_hits: 0,
            feed_scan_count: 0,
            fetch_attempts: 0,
            fetch_successes: 0,
            fetch_empty_results: 0,
            total_proxy_count: 0,
            max_proxy_count: 0,
            last_proxy_count: 0,
            total_validated_proxy_count: 0,
            last_validated_proxy_count: 0,
            total_released_proxy_count: 0,
            last_released_proxy_count: 0,
            last_error: None,
            first_seen_epoch_secs: now,
            last_seen_epoch_secs: now,
            score: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceRegistrySnapshot {
    pub updated_at_epoch_secs: u64,
    pub records: Vec<SourceRecord>,
}

impl SourceRegistry {
    pub fn load_or_default(path: &str) -> Self {
        let registry_path = Path::new(path);
        if !registry_path.exists() {
            return Self::default();
        }

        fs::read_to_string(registry_path)
            .ok()
            .and_then(|content| serde_json::from_str::<SourceRegistry>(&content).ok())
            .unwrap_or_default()
    }

    pub fn persist(&mut self, path: &str) -> io::Result<()> {
        self.updated_at_epoch_secs = now_epoch_secs();
        let output = serde_json::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, output)
    }

    pub fn snapshot(&self) -> SourceRegistrySnapshot {
        let mut records = self.records.values().cloned().collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        SourceRegistrySnapshot {
            updated_at_epoch_secs: self.updated_at_epoch_secs,
            records,
        }
    }

    pub fn mark_seed_source(&mut self, url: &str, source_kind: &str) {
        let record = self.record_mut(url, source_kind);
        record.seen_count += 1;
        record.last_seen_epoch_secs = now_epoch_secs();
        if record.source_kind == "unknown" {
            record.source_kind = source_kind.to_string();
        }
        update_score(record);
    }

    pub fn mark_feed_scan(&mut self, feed: &str, discovered_urls: &[String], error: Option<&str>) {
        let record = self.record_mut(feed, "discovery_feed");
        record.feed_scan_count += 1;
        record.last_seen_epoch_secs = now_epoch_secs();
        record.last_error = error.map(str::to_string);
        if error.is_none() {
            record.discover_hits += discovered_urls.len() as u64;
        }
        update_score(record);

        for url in discovered_urls {
            let child = self.record_mut(url, "discovered_subscription");
            child.seen_count += 1;
            child.last_seen_epoch_secs = now_epoch_secs();
            if !child.discovered_from_feeds.iter().any(|item| item == feed) {
                child.discovered_from_feeds.push(feed.to_string());
            }
            update_score(child);
        }
    }

    pub fn mark_fetch_result(&mut self, url: &str, proxy_count: usize) {
        let record = self.record_mut(url, "subscription");
        record.fetch_attempts += 1;
        record.last_proxy_count = proxy_count;
        record.last_seen_epoch_secs = now_epoch_secs();
        if proxy_count > 0 {
            record.fetch_successes += 1;
            record.total_proxy_count += proxy_count as u64;
            record.max_proxy_count = record.max_proxy_count.max(proxy_count);
            record.last_error = None;
        } else {
            record.fetch_empty_results += 1;
            record.last_error = Some("empty_result".to_string());
        }
        update_score(record);
    }

    pub fn apply_last_validated_counts(&mut self, counts: &BTreeMap<String, usize>) {
        for record in self.records.values_mut() {
            record.last_validated_proxy_count = 0;
        }
        for (url, count) in counts {
            let record = self.record_mut(url, "subscription");
            record.last_validated_proxy_count = *count;
            record.total_validated_proxy_count += *count as u64;
            update_score(record);
        }
    }

    pub fn apply_last_released_counts(&mut self, counts: &BTreeMap<String, usize>) {
        for record in self.records.values_mut() {
            record.last_released_proxy_count = 0;
        }
        for (url, count) in counts {
            let record = self.record_mut(url, "subscription");
            record.last_released_proxy_count = *count;
            record.total_released_proxy_count += *count as u64;
            update_score(record);
        }
    }

    fn record_mut(&mut self, url: &str, source_kind: &str) -> &mut SourceRecord {
        let now = now_epoch_secs();
        self.records
            .entry(url.to_string())
            .or_insert_with(|| SourceRecord {
                url: url.to_string(),
                source_kind: source_kind.to_string(),
                first_seen_epoch_secs: now,
                last_seen_epoch_secs: now,
                ..SourceRecord::default()
            })
    }
}

fn update_score(record: &mut SourceRecord) {
    let fetch_attempts = record.fetch_attempts.max(1) as f64;
    let success_rate = record.fetch_successes as f64 / fetch_attempts;
    let average_proxy_count = if record.fetch_successes == 0 {
        0.0
    } else {
        record.total_proxy_count as f64 / record.fetch_successes as f64
    };
    let yield_score = (average_proxy_count / 50.0).min(1.0);
    let validation_rate = if record.total_proxy_count == 0 {
        0.0
    } else {
        (record.total_validated_proxy_count as f64 / record.total_proxy_count as f64).min(1.0)
    };
    let release_rate = if record.total_validated_proxy_count == 0 {
        0.0
    } else {
        (record.total_released_proxy_count as f64 / record.total_validated_proxy_count as f64)
            .min(1.0)
    };
    let discovery_bonus = (record.discover_hits as f64 / 20.0).min(1.0);

    record.score = ((success_rate * 0.45)
        + (yield_score * 0.2)
        + (validation_rate * 0.2)
        + (release_rate * 0.1)
        + (discovery_bonus * 0.05))
        * 100.0;
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_tracks_discovery_and_fetch() {
        let mut registry = SourceRegistry::default();
        let discovered = vec!["https://example.com/sub.yaml".to_string()];

        registry.mark_seed_source("https://example.com/feed", "discovery_feed");
        registry.mark_feed_scan("https://example.com/feed", &discovered, None);
        registry.mark_fetch_result("https://example.com/sub.yaml", 12);

        let feed = registry.records.get("https://example.com/feed").unwrap();
        let sub = registry
            .records
            .get("https://example.com/sub.yaml")
            .unwrap();
        assert_eq!(feed.feed_scan_count, 1);
        assert_eq!(sub.fetch_successes, 1);
        assert_eq!(sub.last_proxy_count, 12);
    }

    #[test]
    fn test_registry_applies_stage_counts() {
        let mut registry = SourceRegistry::default();
        registry.mark_fetch_result("https://example.com/sub.yaml", 10);

        let mut validated = BTreeMap::new();
        validated.insert("https://example.com/sub.yaml".to_string(), 4);
        registry.apply_last_validated_counts(&validated);

        let mut released = BTreeMap::new();
        released.insert("https://example.com/sub.yaml".to_string(), 2);
        registry.apply_last_released_counts(&released);

        let record = registry
            .records
            .get("https://example.com/sub.yaml")
            .unwrap();
        assert_eq!(record.last_validated_proxy_count, 4);
        assert_eq!(record.last_released_proxy_count, 2);
        assert!(record.score > 0.0);
    }
}
