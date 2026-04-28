use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use proxrs::protocol::Proxy;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::ip::IpDetail;

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
pub struct ValidatedPoolItem {
    pub fingerprint: String,
    pub proxy_type: String,
    pub name: String,
    pub server: String,
    pub source_urls: Vec<String>,
    pub json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ValidatedPoolMetadata {
    pub exit_ip: String,
    pub country: String,
    pub country_code: String,
    pub region: String,
    pub city: String,
    pub isp: String,
    pub supports_gemini: bool,
    pub supports_claude: bool,
}

impl ValidatedPoolMetadata {
    pub fn from_probe_result(
        exit_ip: &str,
        ip_detail: Option<&IpDetail>,
        supports_gemini: bool,
        supports_claude: bool,
    ) -> Self {
        let mut metadata = Self {
            exit_ip: exit_ip.to_string(),
            supports_gemini,
            supports_claude,
            ..Self::default()
        };
        if let Some(detail) = ip_detail {
            metadata.country = detail.country.clone();
            metadata.country_code = detail.country_code.trim().to_ascii_uppercase();
            metadata.region = detail.region.clone();
            metadata.city = detail.city.clone();
            metadata.isp = detail.isp.clone();
        }
        metadata
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedPoolMihomoItem {
    pub fingerprint: String,
    pub proxy_type: String,
    pub name: String,
    pub server: String,
    pub source_urls: Vec<String>,
    pub source_count: usize,
    pub json: String,
    pub exit_ip: String,
    pub country: String,
    pub country_code: String,
    pub region: String,
    pub city: String,
    pub isp: String,
    pub region_hint: String,
    pub supports_gemini: bool,
    pub supports_claude: bool,
}

pub fn build_validated_pool(
    proxies: &[Proxy],
    fingerprint_sources: &HashMap<String, BTreeSet<String>>,
) -> Vec<ValidatedPoolItem> {
    let mut items = Vec::new();
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
        items.push(ValidatedPoolItem {
            fingerprint,
            proxy_type: format!("{:?}", proxy.proxy_type),
            name: proxy.get_name().to_string(),
            server: proxy.get_server().to_string(),
            source_urls,
            json,
        });
    }
    items
}

pub fn build_validated_pool_mihomo(
    proxies: &[Proxy],
    fingerprint_sources: &HashMap<String, BTreeSet<String>>,
    fingerprint_metadata: &HashMap<String, ValidatedPoolMetadata>,
) -> Vec<ValidatedPoolMihomoItem> {
    let mut items = Vec::new();
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
        let metadata = fingerprint_metadata.get(&fingerprint);
        let name = proxy.get_name().to_string();
        items.push(ValidatedPoolMihomoItem {
            fingerprint,
            proxy_type: format!("{:?}", proxy.proxy_type),
            name: name.clone(),
            server: proxy.get_server().to_string(),
            source_count: source_urls.len(),
            source_urls,
            json,
            exit_ip: metadata
                .map(|item| item.exit_ip.clone())
                .unwrap_or_default(),
            country: metadata
                .map(|item| item.country.clone())
                .unwrap_or_default(),
            country_code: metadata
                .map(|item| item.country_code.clone())
                .unwrap_or_default(),
            region: metadata.map(|item| item.region.clone()).unwrap_or_default(),
            city: metadata.map(|item| item.city.clone()).unwrap_or_default(),
            isp: metadata.map(|item| item.isp.clone()).unwrap_or_default(),
            region_hint: infer_region_hint(&name, metadata),
            supports_gemini: metadata
                .map(|item| item.supports_gemini)
                .unwrap_or_else(|| infer_capability_from_name(&name, "gemini")),
            supports_claude: metadata
                .map(|item| item.supports_claude)
                .unwrap_or_else(|| infer_capability_from_name(&name, "claude")),
        });
    }
    items
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

fn infer_region_hint(name: &str, metadata: Option<&ValidatedPoolMetadata>) -> String {
    if let Some(metadata) = metadata {
        if let Some(region_hint) = infer_region_hint_from_geo(metadata) {
            return region_hint.to_string();
        }
    }
    infer_region_hint_from_name(name).to_string()
}

fn infer_region_hint_from_geo(metadata: &ValidatedPoolMetadata) -> Option<&'static str> {
    let country_code = metadata.country_code.trim().to_ascii_uppercase();
    if let Some(region_hint) = region_hint_from_country_code(country_code.as_str()) {
        return Some(region_hint);
    }

    let geo_text =
        format!("{} {} {}", metadata.country, metadata.region, metadata.city).to_lowercase();
    region_hint_from_text(&geo_text)
}

fn infer_region_hint_from_name(name: &str) -> &'static str {
    let lowered = name.to_lowercase();
    if let Some(region_hint) = region_hint_from_text(&lowered) {
        return region_hint;
    }
    if name.contains("香港") || name.contains("🇭🇰") {
        return "HK";
    }
    if name.contains("台湾") || name.contains("台灣") || name.contains("🇹🇼") {
        return "TW";
    }
    if name.contains("新加坡") || name.contains("狮城") || name.contains("🇸🇬") {
        return "SG";
    }
    if name.contains("日本")
        || name.contains("东京")
        || name.contains("大阪")
        || name.contains("🇯🇵")
    {
        return "JP";
    }
    if name.contains("美国") || name.contains("🇺🇸") {
        return "US";
    }
    if name.contains("韩国") || name.contains("首尔") || name.contains("🇰🇷") {
        return "KR";
    }
    "OTHER"
}

fn infer_capability_from_name(name: &str, capability: &str) -> bool {
    name.to_lowercase().contains(capability)
}

fn region_hint_from_country_code(country_code: &str) -> Option<&'static str> {
    match country_code {
        "HK" => Some("HK"),
        "TW" => Some("TW"),
        "SG" => Some("SG"),
        "JP" => Some("JP"),
        "US" => Some("US"),
        "KR" => Some("KR"),
        _ => None,
    }
}

fn region_hint_from_text(content: &str) -> Option<&'static str> {
    if contains_any(content, &["hong kong", "香港", " hk "]) {
        Some("HK")
    } else if contains_any(
        content,
        &["taiwan", "台灣", "台湾", "台北", "彰化", "桃园", " tw "],
    ) {
        Some("TW")
    } else if contains_any(content, &["singapore", "新加坡", "狮城", " sg "]) {
        Some("SG")
    } else if contains_any(content, &["japan", "日本", "东京", "大阪", " jp "]) {
        Some("JP")
    } else if contains_any(
        content,
        &[
            "united states",
            "usa",
            "美国",
            "los angeles",
            "buffalo",
            " us ",
        ],
    ) {
        Some("US")
    } else if contains_any(content, &["korea", "韩国", "首尔", " kr "]) {
        Some("KR")
    } else {
        None
    }
}

fn contains_any(content: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| content.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxrs::protocol::Proxy;

    #[test]
    fn test_count_sources_for_proxies_empty_is_safe() {
        let proxies = Vec::new();
        let counts = count_sources_for_proxies(&proxies, &HashMap::new());
        assert!(counts.is_empty());
    }

    #[test]
    fn test_build_validated_pool_mihomo_includes_probe_metadata() {
        let proxy = Proxy::from_json(
            r#"{"name":"新加坡_Singapore_TestISP_1.1.1.1_Gemini_Claude","type":"ss","server":"1.1.1.1","port":443,"cipher":"aes-128-gcm","password":"secret"}"#,
        )
        .unwrap();
        let fingerprint = proxy_fingerprint(&proxy).unwrap();
        let proxies = vec![proxy];
        let mut fingerprint_sources = HashMap::new();
        fingerprint_sources.insert(
            fingerprint.clone(),
            BTreeSet::from(["https://example.com/sub.yaml".to_string()]),
        );
        let mut fingerprint_metadata = HashMap::new();
        fingerprint_metadata.insert(
            fingerprint,
            ValidatedPoolMetadata {
                exit_ip: "1.1.1.1".to_string(),
                country: "Singapore".to_string(),
                country_code: "SG".to_string(),
                region: "Singapore".to_string(),
                city: "Singapore".to_string(),
                isp: "TestISP".to_string(),
                supports_gemini: true,
                supports_claude: true,
            },
        );

        let items =
            build_validated_pool_mihomo(&proxies, &fingerprint_sources, &fingerprint_metadata);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].region_hint, "SG");
        assert!(items[0].supports_gemini);
        assert!(items[0].supports_claude);
        assert_eq!(items[0].source_count, 1);
        assert_eq!(items[0].exit_ip, "1.1.1.1");
    }

    #[test]
    fn test_build_validated_pool_mihomo_falls_back_to_name_signals() {
        let proxy = Proxy::from_json(
            r#"{"name":"美国_Los Angeles_TestISP_Gemini","type":"ss","server":"2.2.2.2","port":443,"cipher":"aes-128-gcm","password":"secret"}"#,
        )
        .unwrap();
        let proxies = vec![proxy];
        let items = build_validated_pool_mihomo(&proxies, &HashMap::new(), &HashMap::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].region_hint, "US");
        assert!(items[0].supports_gemini);
        assert!(!items[0].supports_claude);
    }
}
