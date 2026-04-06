use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;

use crate::discovery;

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

        let registry = fs::read_to_string(registry_path)
            .ok()
            .and_then(|content| serde_json::from_str::<SourceRegistry>(&content).ok())
            .unwrap_or_default();
        normalize_registry(registry)
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
        upgrade_source_kind(&mut record.source_kind, source_kind);
        update_score(record);
    }

    pub fn mark_feed_scan(&mut self, feed: &str, discovered_urls: &[String], error: Option<&str>) {
        let normalized_feed = discovery::canonicalize_registry_url(feed);
        let record = self.record_mut(&normalized_feed, "discovery_feed");
        record.feed_scan_count += 1;
        record.last_seen_epoch_secs = now_epoch_secs();
        record.last_error = error.map(str::to_string);
        upgrade_source_kind(&mut record.source_kind, "discovery_feed");
        if error.is_none() {
            record.discover_hits += discovered_urls.len() as u64;
        }
        update_score(record);

        for url in discovered_urls {
            let child = self.record_mut(url, "discovered_subscription");
            child.seen_count += 1;
            child.last_seen_epoch_secs = now_epoch_secs();
            upgrade_source_kind(&mut child.source_kind, "discovered_subscription");
            if !child
                .discovered_from_feeds
                .iter()
                .any(|item| item == &normalized_feed)
            {
                child.discovered_from_feeds.push(normalized_feed.clone());
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
        let normalized_url = discovery::canonicalize_registry_url(url);
        self.records
            .entry(normalized_url.clone())
            .or_insert_with(|| SourceRecord {
                url: normalized_url,
                source_kind: source_kind.to_string(),
                first_seen_epoch_secs: now,
                last_seen_epoch_secs: now,
                ..SourceRecord::default()
            })
    }
}

fn normalize_registry(registry: SourceRegistry) -> SourceRegistry {
    let mut normalized = SourceRegistry {
        updated_at_epoch_secs: registry.updated_at_epoch_secs,
        records: BTreeMap::new(),
    };
    for (_, mut record) in registry.records {
        let key = discovery::canonicalize_registry_url(&record.url);
        record.url = key.clone();
        if let Some(existing) = normalized.records.get_mut(&key) {
            merge_record(existing, record);
        } else {
            normalized.records.insert(key, record);
        }
    }
    normalized
}

fn merge_record(target: &mut SourceRecord, source: SourceRecord) {
    upgrade_source_kind(&mut target.source_kind, &source.source_kind);
    target.seen_count += source.seen_count;
    target.discover_hits += source.discover_hits;
    target.feed_scan_count += source.feed_scan_count;
    target.fetch_attempts += source.fetch_attempts;
    target.fetch_successes += source.fetch_successes;
    target.fetch_empty_results += source.fetch_empty_results;
    target.total_proxy_count += source.total_proxy_count;
    target.max_proxy_count = target.max_proxy_count.max(source.max_proxy_count);
    target.total_validated_proxy_count += source.total_validated_proxy_count;
    target.total_released_proxy_count += source.total_released_proxy_count;
    target.first_seen_epoch_secs = target
        .first_seen_epoch_secs
        .min(source.first_seen_epoch_secs);
    if source.last_seen_epoch_secs >= target.last_seen_epoch_secs {
        target.last_seen_epoch_secs = source.last_seen_epoch_secs;
        target.last_proxy_count = source.last_proxy_count;
        target.last_validated_proxy_count = source.last_validated_proxy_count;
        target.last_released_proxy_count = source.last_released_proxy_count;
        target.last_error = source.last_error.clone();
    }
    for feed in source.discovered_from_feeds {
        if !target
            .discovered_from_feeds
            .iter()
            .any(|item| item == &feed)
        {
            target.discovered_from_feeds.push(feed);
        }
    }
    update_score(target);
}

fn upgrade_source_kind(current: &mut String, candidate: &str) {
    if source_kind_priority(candidate) > source_kind_priority(current.as_str()) {
        *current = candidate.to_string();
    }
}

fn source_kind_priority(source_kind: &str) -> u8 {
    match source_kind {
        "direct_subscription" => 50,
        "pool_subscription" => 40,
        "subscription" => 30,
        "discovered_subscription" => 20,
        "discovery_feed" => 10,
        _ => 0,
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

    #[test]
    fn test_load_normalizes_duplicate_raw_variants() {
        let registry = normalize_registry(SourceRegistry {
            updated_at_epoch_secs: 1,
            records: BTreeMap::from([
                (
                    "https://raw.githubusercontent.com/example/repo/refs/heads/main/subs/test.yaml"
                        .to_string(),
                    SourceRecord {
                        url: "https://raw.githubusercontent.com/example/repo/refs/heads/main/subs/test.yaml"
                            .to_string(),
                        fetch_attempts: 1,
                        ..SourceRecord::default()
                    },
                ),
                (
                    "https://raw.githubusercontent.com/example/repo/main/subs/test.yaml".to_string(),
                    SourceRecord {
                        url: "https://raw.githubusercontent.com/example/repo/main/subs/test.yaml"
                            .to_string(),
                        fetch_successes: 1,
                        ..SourceRecord::default()
                    },
                ),
            ]),
        });

        assert_eq!(registry.records.len(), 1);
        let record = registry
            .records
            .get("https://raw.githubusercontent.com/example/repo/main/subs/test.yaml")
            .unwrap();
        assert_eq!(record.fetch_attempts, 1);
        assert_eq!(record.fetch_successes, 1);
    }

    #[test]
    fn test_normalize_registry_keeps_dirty_embedded_multi_url_keys_separate() {
        let dirty_key = "https://example.com/a.yaml/nhttps://example.com/b.yaml".to_string();
        let registry = normalize_registry(SourceRegistry {
            updated_at_epoch_secs: 1,
            records: BTreeMap::from([
                (
                    dirty_key.clone(),
                    SourceRecord {
                        url: dirty_key.clone(),
                        fetch_attempts: 3,
                        ..SourceRecord::default()
                    },
                ),
                (
                    "https://example.com/a.yaml".to_string(),
                    SourceRecord {
                        url: "https://example.com/a.yaml".to_string(),
                        fetch_successes: 1,
                        ..SourceRecord::default()
                    },
                ),
            ]),
        });

        assert_eq!(registry.records.len(), 2);
        assert_eq!(
            registry
                .records
                .get("https://example.com/a.yaml")
                .unwrap()
                .fetch_successes,
            1
        );
        assert_eq!(registry.records.get(&dirty_key).unwrap().fetch_attempts, 3);
    }

    #[test]
    fn test_mark_seed_source_upgrades_source_kind_priority() {
        let mut registry = SourceRegistry::default();
        let discovered = vec!["https://example.com/sub.yaml".to_string()];

        registry.mark_feed_scan("https://example.com/feed", &discovered, None);
        registry.mark_seed_source("https://example.com/sub.yaml", "direct_subscription");

        let record = registry
            .records
            .get("https://example.com/sub.yaml")
            .unwrap();
        assert_eq!(record.source_kind, "direct_subscription");
    }
}
