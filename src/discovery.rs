use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use regex::Regex;
use reqwest::Client;
use reqwest::Url;
use scraper::Html;
use scraper::Selector;
use serde::Serialize;
use tracing::info;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const DISCOVERY_HINTS: [&str; 10] = [
    "clash",
    "sub",
    "subscribe",
    "subscription",
    "proxy",
    "node",
    "pool",
    "merge",
    "source",
    "meta",
];
const DIRECT_EXTENSIONS: [&str; 5] = ["yaml", "yml", "txt", "sub", "conf"];
const EXCLUDED_PATH_HINTS: [&str; 3] = [".github/", "/workflows/", "/actions/"];
const SHORTENER_HOSTS: [&str; 6] = ["git.io", "bit.ly", "tinyurl.com", "is.gd", "t.ly", "goo.su"];

#[derive(Debug, Clone, Serialize, Default)]
pub struct DiscoveryFeedResult {
    pub feed: String,
    pub resolved_feed: Option<String>,
    pub discovered_urls: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DiscoveryReport {
    pub feeds: Vec<DiscoveryFeedResult>,
    pub unique_discovered_urls: Vec<String>,
}

#[allow(dead_code)]
pub async fn discover_sub_urls(feeds: &[String]) -> Vec<String> {
    discover_sub_urls_with_report(feeds)
        .await
        .unique_discovered_urls
}

pub async fn discover_sub_urls_with_report(feeds: &[String]) -> DiscoveryReport {
    if feeds.is_empty() {
        return DiscoveryReport::default();
    }

    let client = match Client::builder().timeout(DISCOVERY_TIMEOUT).build() {
        Ok(client) => client,
        Err(err) => {
            info!("build discovery client failed: {}", err);
            return DiscoveryReport::default();
        }
    };

    let mut feed_reports = Vec::new();
    let mut discovered = Vec::new();
    let mut seen = HashSet::new();

    for feed in feeds {
        match discover_feed_urls(&client, feed).await {
            Ok((resolved_feed, urls)) => {
                info!("discover feed {} extracted {} candidates", feed, urls.len());
                feed_reports.push(DiscoveryFeedResult {
                    feed: feed.clone(),
                    resolved_feed,
                    discovered_urls: urls.clone(),
                    error: None,
                });
                for url in urls {
                    if seen.insert(url.clone()) {
                        discovered.push(url);
                    }
                }
            }
            Err(err) => {
                info!("discover feed {} failed: {}", feed, err);
                feed_reports.push(DiscoveryFeedResult {
                    feed: feed.clone(),
                    resolved_feed: None,
                    discovered_urls: Vec::new(),
                    error: Some(err.to_string()),
                });
            }
        }
    }

    DiscoveryReport {
        feeds: feed_reports,
        unique_discovered_urls: discovered,
    }
}

pub fn canonicalize_registry_url(raw: &str) -> String {
    canonicalize_registry_key(raw).unwrap_or_else(|| raw.trim().to_string())
}

pub fn canonicalize_subscription_url(raw: &str) -> Option<String> {
    canonicalize_candidate_url(raw, None, true)
}

pub fn canonicalize_trusted_subscription_urls(raw: &str) -> Vec<String> {
    let embedded = split_embedded_absolute_urls(raw);
    if embedded.len() > 1 {
        return dedupe_strings(
            embedded
                .into_iter()
                .filter_map(|item| canonicalize_candidate_url(&item, None, false))
                .collect(),
        );
    }

    if let Some(url) = canonicalize_candidate_url(raw, None, false) {
        return vec![url];
    }

    embedded
        .first()
        .and_then(|item| canonicalize_candidate_url(item, None, false))
        .into_iter()
        .collect()
}

async fn discover_feed_urls(
    client: &Client,
    feed: &str,
) -> Result<(Option<String>, Vec<String>), Box<dyn std::error::Error>> {
    if feed.starts_with("http://") || feed.starts_with("https://") {
        let response = client.get(feed).send().await?;
        if !response.status().is_success() {
            return Err(format!("status {}", response.status()).into());
        }

        let final_url = response.url().clone();
        let content = response.text().await?;
        let mut urls = extract_candidate_urls(&content, Some(&final_url));

        if let Some(url) = canonicalize_candidate_url(feed, Some(&final_url), true) {
            if is_candidate_subscription_url(&url) {
                urls.push(url);
            }
        }

        return Ok((Some(final_url.to_string()), dedupe_strings(urls)));
    }

    if Path::new(feed).is_file() {
        let content = fs::read_to_string(feed)?;
        let base_url = Url::from_file_path(Path::new(feed)).ok();
        let mut urls = extract_candidate_urls(&content, base_url.as_ref());
        if is_candidate_subscription_url(feed) {
            urls.push(feed.to_string());
        }
        return Ok((Some(feed.to_string()), dedupe_strings(urls)));
    }

    Ok((None, extract_candidate_urls(feed, None)))
}

fn extract_candidate_urls(content: &str, base_url: Option<&Url>) -> Vec<String> {
    let absolute_regex = Regex::new(r#"https?://[^\s"'<>`]+"#).unwrap();
    let markdown_regex = Regex::new(r#"\[[^\]]*]\(([^)]+)\)"#).unwrap();
    let selector = Selector::parse("a[href], link[href]").unwrap();
    let mut urls = Vec::new();

    for matched in absolute_regex.find_iter(content) {
        for url in normalize_candidate_urls(matched.as_str(), base_url) {
            urls.push(url);
        }
    }

    for captures in markdown_regex.captures_iter(content) {
        if let Some(target) = captures.get(1) {
            for url in normalize_candidate_urls(target.as_str(), base_url) {
                urls.push(url);
            }
        }
    }

    let document = Html::parse_document(content);
    for element in document.select(&selector) {
        if let Some(target) = element.value().attr("href") {
            for url in normalize_candidate_urls(target, base_url) {
                urls.push(url);
            }
        }
    }

    dedupe_strings(urls)
}

fn normalize_candidate_urls(candidate: &str, base_url: Option<&Url>) -> Vec<String> {
    let embedded = split_embedded_absolute_urls(candidate);
    if embedded.len() > 1 {
        return dedupe_strings(
            embedded
                .into_iter()
                .filter_map(|item| canonicalize_candidate_url(&item, None, true))
                .collect(),
        );
    }
    canonicalize_candidate_url(candidate, base_url, true)
        .into_iter()
        .collect()
}

fn canonicalize_registry_key(raw: &str) -> Option<String> {
    let embedded = split_embedded_absolute_urls(raw);
    if embedded.len() > 1 {
        return None;
    }

    if let Some(first) = embedded.first() {
        return canonicalize_candidate_url(first, None, false);
    }

    canonicalize_candidate_url(raw, None, false)
}

fn canonicalize_candidate_url(
    candidate: &str,
    base_url: Option<&Url>,
    require_subscription: bool,
) -> Option<String> {
    let trimmed = candidate.trim().trim_matches(|ch| {
        matches!(
            ch,
            '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
    });
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("javascript:")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with("data:")
    {
        return None;
    }

    let mut parsed = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Url::parse(trimmed).ok()?
    } else {
        base_url?.join(trimmed).ok()?
    };

    parsed.set_fragment(None);
    let normalized = normalize_candidate_path(normalize_github_url(parsed));
    if !require_subscription || is_candidate_subscription_url(normalized.as_str()) {
        Some(normalized.to_string())
    } else {
        None
    }
}

fn normalize_github_url(url: Url) -> Url {
    let Some(host) = url.host_str() else {
        return url;
    };

    let Some(segments) = url
        .path_segments()
        .map(|parts| parts.map(str::to_string).collect::<Vec<_>>())
    else {
        return url;
    };

    if (host == "github.com" || host == "www.github.com")
        && segments.len() >= 5
        && (segments[2] == "blob" || segments[2] == "raw")
    {
        let raw_path = format!(
            "{}/{}/{}/{}",
            segments[0],
            segments[1],
            segments[3],
            segments[4..].join("/")
        );
        if let Ok(raw_url) = Url::parse(&format!("https://raw.githubusercontent.com/{}", raw_path))
        {
            return raw_url;
        }
    }

    if host == "raw.githubusercontent.com"
        && segments.len() >= 6
        && segments[2] == "refs"
        && segments[3] == "heads"
    {
        if let Ok(raw_url) = Url::parse(&format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            segments[0],
            segments[1],
            segments[4],
            segments[5..].join("/")
        )) {
            return raw_url;
        }
    }

    url
}

fn normalize_candidate_path(mut url: Url) -> Url {
    let mut path = url.path().trim().to_string();
    if path.len() > 1 && path.ends_with('/') {
        let trimmed = path.trim_end_matches('/');
        let extension = Path::new(trimmed)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        if extension
            .as_deref()
            .is_some_and(|value| DIRECT_EXTENSIONS.contains(&value))
        {
            path = trimmed.to_string();
            url.set_path(&path);
        }
    }
    url
}

fn split_embedded_absolute_urls(candidate: &str) -> Vec<String> {
    let repaired = candidate
        .replace("\\n", "\n")
        .replace("/nhttps://", "\nhttps://")
        .replace("/nhttp://", "\nhttp://");
    let absolute_regex = Regex::new(r#"https?://[^\s"'<>`]+"#).unwrap();
    absolute_regex
        .find_iter(&repaired)
        .map(|matched| matched.as_str().to_string())
        .collect()
}

fn is_candidate_subscription_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };

    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if SHORTENER_HOSTS.contains(&host.as_str()) {
        return true;
    }

    let lowercase_url = parsed.as_str().to_ascii_lowercase();
    if EXCLUDED_PATH_HINTS
        .iter()
        .any(|hint| lowercase_url.contains(hint))
    {
        return false;
    }

    if lowercase_url.contains("target=clash")
        || lowercase_url.contains("target=clash-meta")
        || lowercase_url.contains("target=singbox")
        || lowercase_url.contains("/subscribe")
        || lowercase_url.contains("/subscription")
    {
        return true;
    }

    let extension = Path::new(parsed.path())
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let has_supported_extension = extension
        .as_deref()
        .is_some_and(|value| DIRECT_EXTENSIONS.contains(&value));

    if !has_supported_extension {
        return false;
    }

    if host == "raw.githubusercontent.com"
        || host == "gist.githubusercontent.com"
        || host == "cdn.jsdelivr.net"
    {
        return true;
    }

    DISCOVERY_HINTS
        .iter()
        .any(|hint| lowercase_url.contains(hint))
}

fn dedupe_strings(items: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        if seen.insert(item.clone()) {
            deduped.push(item);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_urls_from_markdown() {
        let base = Url::parse("https://github.com/example/repo/blob/main/README.md").unwrap();
        let content = r#"
[merged](./subs/merged/tested_within.yaml)
[direct](https://github.com/example/repo/blob/main/source/clash-meta.yaml)
"#;

        let urls = extract_candidate_urls(content, Some(&base));
        assert!(urls.iter().any(|url| {
            url == "https://raw.githubusercontent.com/example/repo/main/subs/merged/tested_within.yaml"
        }));
        assert!(urls.iter().any(|url| {
            url == "https://raw.githubusercontent.com/example/repo/main/source/clash-meta.yaml"
        }));
    }

    #[test]
    fn test_extract_urls_from_html_tree_page() {
        let base =
            Url::parse("https://github.com/dongchengjie/airport/tree/main/subs/merged").unwrap();
        let content = r#"
<html>
  <body>
    <a href="/dongchengjie/airport/blob/main/subs/merged/tested.yaml">tested</a>
    <a href="/dongchengjie/airport/blob/main/.github/workflows/build.yml">workflow</a>
  </body>
</html>
"#;

        let urls = extract_candidate_urls(content, Some(&base));
        assert_eq!(
            urls,
            vec!["https://raw.githubusercontent.com/dongchengjie/airport/main/subs/merged/tested.yaml".to_string()]
        );
    }

    #[test]
    fn test_shortener_url_is_allowed() {
        assert!(is_candidate_subscription_url("https://git.io/example"));
    }

    #[test]
    fn test_canonicalize_subscription_url_normalizes_raw_refs_heads_path() {
        let url = canonicalize_subscription_url(
            "https://raw.githubusercontent.com/example/repo/refs/heads/main/subs/test.yaml",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://raw.githubusercontent.com/example/repo/main/subs/test.yaml"
        );
    }

    #[test]
    fn test_extract_candidate_urls_splits_embedded_multiple_urls() {
        let urls = normalize_candidate_urls(
            "https://anaer.github.io/Sub/clash.yaml/nhttps://raw.githubusercontent.com/anaer/Sub/main/clash.yaml/",
            None,
        );
        assert_eq!(
            urls,
            vec![
                "https://anaer.github.io/Sub/clash.yaml".to_string(),
                "https://raw.githubusercontent.com/anaer/Sub/main/clash.yaml".to_string(),
            ]
        );
    }

    #[test]
    fn test_canonicalize_trusted_subscription_urls_keeps_unhinted_seed_url() {
        let urls = canonicalize_trusted_subscription_urls("https://example.com/api/export?id=123");
        assert_eq!(
            urls,
            vec!["https://example.com/api/export?id=123".to_string()]
        );
    }

    #[test]
    fn test_canonicalize_registry_url_keeps_dirty_embedded_multi_url_key() {
        let raw = "https://example.com/a.yaml/nhttps://example.com/b.yaml";
        assert_eq!(canonicalize_registry_url(raw), raw);
    }
}
