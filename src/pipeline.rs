use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use proxrs::protocol::Proxy;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct SourceInputsArtifact {
    pub direct_subs: Vec<String>,
    pub discovery_enabled: bool,
    pub discovery_feeds: Vec<String>,
    pub pool_enabled: bool,
    pub pool_sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateSourcesArtifact {
    pub sources: Vec<String>,
    pub total_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceFetchArtifact {
    pub source_url: String,
    pub proxy_count: usize,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyArtifact {
    pub fingerprint: String,
    pub proxy_type: String,
    pub name: String,
    pub server: String,
    pub source_urls: Vec<String>,
    pub json: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DelayGroupArtifact {
    pub group_index: usize,
    pub input_proxy_count: usize,
    pub delay_rounds: Vec<HashMap<String, i64>>,
    pub surviving_node_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineSummaryArtifact {
    pub candidate_source_count: usize,
    pub raw_proxy_count: usize,
    pub unique_proxy_count: usize,
    pub useful_proxy_count: usize,
    pub final_release_proxy_count: usize,
}

pub fn proxy_fingerprint(proxy: &Proxy) -> Option<String> {
    let json = proxy.to_json().ok()?;
    let mut value = serde_json::from_str::<Value>(&json).ok()?;
    if let Value::Object(ref mut map) = value {
        map.remove("name");
    }
    serde_json::to_string(&value).ok()
}

pub fn build_proxy_artifacts(
    proxies: &[Proxy],
    fingerprint_sources: &HashMap<String, BTreeSet<String>>,
) -> Vec<ProxyArtifact> {
    let mut artifacts = Vec::new();
    for proxy in proxies {
        let Some(fingerprint) = proxy_fingerprint(proxy) else {
            continue;
        };
        let json = match proxy.to_json() {
            Ok(json) => json,
            Err(_) => continue,
        };
        let source_urls = fingerprint_sources
            .get(&fingerprint)
            .map(|items| items.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        artifacts.push(ProxyArtifact {
            fingerprint,
            proxy_type: format!("{:?}", proxy.proxy_type),
            name: proxy.get_name().to_string(),
            server: proxy.get_server().to_string(),
            source_urls,
            json,
        });
    }
    artifacts
}

pub fn count_sources_for_proxies(
    proxies: &[Proxy],
    fingerprint_sources: &HashMap<String, BTreeSet<String>>,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for proxy in proxies {
        let Some(fingerprint) = proxy_fingerprint(proxy) else {
            continue;
        };
        let Some(source_urls) = fingerprint_sources.get(&fingerprint) else {
            continue;
        };
        for source_url in source_urls {
            *counts.entry(source_url.clone()).or_insert(0) += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_sources_for_proxies_empty_is_safe() {
        let proxies = Vec::new();
        let counts = count_sources_for_proxies(&proxies, &HashMap::new());
        assert!(counts.is_empty());
    }
}
