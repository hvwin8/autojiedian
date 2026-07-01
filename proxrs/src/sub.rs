use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io;
use std::io::Read;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::time::Duration;

use regex::Regex;
use reqwest::Client;
use serde_json::Value as JsonValue;
use serde_yaml::Mapping;
use serde_yaml::Value;
use tokio::time::sleep;
use tracing::debug;
use tracing::info;

use crate::base64::base64decode;
use crate::base64::base64encode;
use crate::protocol::Proxy;
use crate::protocol::ProxyType;

#[derive(Debug)]
pub struct SubManager {}

impl SubManager {
    /// 从链接中获取代理信息支持以下四种结构
    /// 1. http://订阅链接，传入代理地址
    /// 2. C:\\文件地址 /home/yaml，传入文件地址
    /// 3. ss://xxxx，传入单个节点链接
    /// 4. edhxxx, 传入 base64 的节点信息
    pub async fn get_proxies_from_url(url: String) -> Vec<Proxy> {
        let mut proxies: Vec<Proxy> = Vec::new();
        let mut pending_sources = vec![url];
        let mut visited_sources: HashSet<String> = HashSet::new();

        while let Some(source) = pending_sources.pop() {
            if !visited_sources.insert(source.clone()) {
                continue;
            }

            if source.starts_with("http") {
                if let Ok(content) = Self::get_content_from_sub_url(&source).await {
                    if let Ok(parsed) = Self::parse_content(content.clone()) {
                        if !parsed.is_empty() {
                            info!("{} parsed proxies: {}", &source, &parsed.len());
                            proxies.extend(parsed);
                            continue;
                        }
                    }
                    let provider_urls = Self::extract_proxy_provider_urls(&content);
                    info!(
                        "{} parsed proxies: 0, nested providers: {}",
                        &source,
                        &provider_urls.len()
                    );
                    pending_sources.extend(provider_urls);
                }
            } else if Path::new(&source).is_file() {
                if let Ok(content) = fs::read_to_string(&source) {
                    if let Ok(parsed) = Self::parse_content(content.clone()) {
                        if !parsed.is_empty() {
                            info!("{} parsed proxies: {}", &source, &parsed.len());
                            proxies.extend(parsed);
                            continue;
                        }
                    }
                    let provider_urls = Self::extract_proxy_provider_urls(&content);
                    info!(
                        "{} parsed proxies: 0, nested providers: {}",
                        &source,
                        &provider_urls.len()
                    );
                    pending_sources.extend(provider_urls);
                }
            } else if let Ok(parsed) = Self::parse_content(source.clone()) {
                info!("{} parsed proxies: {}", &source, &parsed.len());
                proxies.extend(parsed);
            } else {
                info!("{} parsed proxies: 0", &source);
            }
        }
        proxies
    }

    /// 传入 urls 列表解析代理
    pub async fn get_proxies_from_urls(subs: &Vec<String>) -> Vec<Proxy> {
        let mut proxies: Vec<Proxy> = Vec::new();
        for url in subs {
            proxies.extend(Self::get_proxies_from_url(url.to_string()).await)
        }

        if !proxies.is_empty() {
            proxies = Self::exclude_dup_proxies(proxies);
            Self::rename_dup_proxies_name(&mut proxies);
        }

        proxies
    }

    async fn get_content_from_sub_url(sub_url: &str) -> Result<String, Box<dyn std::error::Error>> {
        let client = Client::new();
        let mut attempts = 0;
        let retries = 3;

        loop {
            let result = client
                .get(sub_url)
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            match result {
                Ok(resp) => {
                    let status = resp.status();
                    return if status.is_success() {
                        // 获取 UUID 作为文件名
                        // let re = Regex::new(r"files/(.*?)/raw").unwrap();
                        // let uuid = re.captures(sub_url)
                        //     .and_then(|caps| caps.get(1))
                        //     .map_or_else(|| {
                        //         format!("{:x}", md5::compute(sub_url))
                        //     }, |m| m.as_str().to_string());

                        // let file_path = PathBuf::from_iter(vec!["subs", &uuid.to_string()]);
                        // let mut file = File::create(&file_path).unwrap();

                        let content_result = resp.text().await;
                        match content_result {
                            Ok(content) => {
                                // file.write_all(content.as_bytes()).unwrap();
                                // Ok(env::current_dir().unwrap().join(file_path).to_string_lossy().
                                // to_string())
                                Ok(content)
                            }
                            Err(e) => {
                                if e.is_timeout() {
                                    continue;
                                }
                                return Err(Box::new(e));
                            }
                        }
                    } else {
                        Err(format!("获取订阅连失败 {} 响应码 {}", sub_url, status.as_str()).into())
                    };
                }
                Err(e) => {
                    if !e.is_timeout() {
                        return Err(Box::new(e));
                    }
                }
            }

            if attempts < retries {
                attempts += 1;
                sleep(Duration::from_secs(1)).await;
            } else {
                return Err(format!(
                    "当前链接 {} 无法访问，已跳过，或请确保当前网络通顺",
                    sub_url
                )
                .into());
            }
        }
    }

    /// 从本地文件中解析代理
    pub fn parse_from_path<P: AsRef<Path>>(
        file_path: P,
    ) -> Result<Vec<Proxy>, Box<dyn std::error::Error>> {
        match fs::read_to_string(file_path) {
            Ok(contents) => Ok(Self::parse_content(contents)?),
            Err(e) => Err(format!("Error reading file: {}", e).into()),
        }
    }

    /// 从字符串中解析代理
    /// 1. 先尝试使用 yaml 格式解析
    /// 2. 尝试解析 base64 格式
    /// 3. 尝试使用纯链接格式解析
    pub fn parse_content(content: String) -> Result<Vec<Proxy>, Box<dyn std::error::Error>> {
        let conf_proxies: Vec<Proxy> = Vec::new();
        match Self::parse_yaml_content(&content) {
            Ok(proxies) => return Ok(proxies),
            Err(_) => match Self::parse_base64_content(&content) {
                Ok(proxies) => return Ok(proxies),
                Err(_) => {
                    if let Ok(proxies) = Self::parse_links_content(&content) {
                        return Ok(proxies);
                    }
                }
            },
        }
        Ok(conf_proxies)
    }

    fn parse_yaml_content(content: &str) -> Result<Vec<Proxy>, Box<dyn std::error::Error>> {
        let mut conf_proxies: Vec<Proxy> = Vec::new();
        let yaml = serde_yaml::from_str::<JsonValue>(content)?;
        Self::collect_proxies_from_json_value(&yaml, &mut conf_proxies);
        if conf_proxies.is_empty() {
            return Err(format!("Proxy not found: {}", content).into());
        }
        Ok(conf_proxies)
    }

    fn parse_base64_content(content: &str) -> Result<Vec<Proxy>, Box<dyn std::error::Error>> {
        let compact = content.lines().map(str::trim).collect::<String>();
        let decoded = base64decode(compact.trim());
        if decoded.trim().is_empty() || decoded.trim() == compact.trim() {
            return Err("Base64 content decode failed".into());
        }
        if let Ok(proxies) = Self::parse_yaml_content(&decoded) {
            return Ok(proxies);
        }
        Self::parse_links_content(&decoded)
    }

    fn parse_links_content(content: &str) -> Result<Vec<Proxy>, Box<dyn std::error::Error>> {
        let mut conf_proxies: Vec<Proxy> = Vec::new();
        let link_regex =
            Regex::new(r#"(?i)(?:ssr?|vmess|vless|trojan|hysteria2|hy2)://[^\s"'<>]+"#).unwrap();
        for matched in link_regex.find_iter(content) {
            let link = Self::normalize_link_candidate(matched.as_str());
            if let Some(proxy) = Self::parse_proxy_link_safely(link.as_str()) {
                conf_proxies.push(proxy)
            }
        }
        if conf_proxies.is_empty() {
            return Err("No supported proxy links found".into());
        }
        Ok(conf_proxies)
    }

    fn collect_proxies_from_json_value(value: &JsonValue, proxies: &mut Vec<Proxy>) {
        match value {
            JsonValue::Object(map) => {
                for key in ["proxies", "Proxies", "payload"] {
                    if let Some(items) = map.get(key).and_then(JsonValue::as_array) {
                        Self::collect_proxy_array(items, proxies);
                    }
                }
                for nested in map.values() {
                    Self::collect_proxies_from_json_value(nested, proxies);
                }
            }
            JsonValue::Array(items) => {
                for item in items {
                    Self::collect_proxies_from_json_value(item, proxies);
                }
            }
            _ => {}
        }
    }

    fn collect_proxy_array(items: &[JsonValue], proxies: &mut Vec<Proxy>) {
        for item in items {
            let is_proxy_like = item
                .as_object()
                .is_some_and(|value| value.contains_key("type") && value.contains_key("server"));
            if !is_proxy_like {
                continue;
            }
            if let Some(proxy) = Self::parse_proxy_json_safely(item) {
                proxies.push(proxy);
            }
        }
    }

    fn normalize_link_candidate(link: &str) -> String {
        let trimmed = link.trim().trim_matches(|c| {
            matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}' | '>' | '.'
            )
        });
        if let Some(rest) = trimmed.strip_prefix("hy2://") {
            return format!("hysteria2://{}", rest);
        }
        trimmed.to_string()
    }

    fn extract_proxy_provider_urls(content: &str) -> Vec<String> {
        let mut urls = Vec::new();
        let Ok(yaml) = serde_yaml::from_str::<JsonValue>(content) else {
            return urls;
        };
        let Some(root) = yaml.as_object() else {
            return urls;
        };
        let Some(provider_root) = root.get("proxy-providers").and_then(JsonValue::as_object) else {
            return urls;
        };
        for provider in provider_root.values() {
            if let Some(url) = provider
                .as_object()
                .and_then(|item| item.get("url"))
                .and_then(JsonValue::as_str)
            {
                let trimmed = url.trim();
                if trimmed.starts_with("http") && !urls.iter().any(|item| item == trimmed) {
                    urls.push(trimmed.to_string());
                }
            }
        }
        urls
    }

    fn parse_proxy_link_safely(link: &str) -> Option<Proxy> {
        match catch_unwind(AssertUnwindSafe(|| Proxy::from_link(link.to_string()))) {
            Ok(Ok(proxy)) => Some(proxy),
            Ok(Err(err)) => {
                debug!("skip unsupported proxy link: {} {}", err, link);
                None
            }
            Err(_) => {
                debug!("skip panicking proxy link: {}", link);
                None
            }
        }
    }

    fn parse_proxy_json_safely(item: &JsonValue) -> Option<Proxy> {
        let raw = item.to_string();
        match catch_unwind(AssertUnwindSafe(|| Proxy::from_json(&raw))) {
            Ok(Ok(proxy)) => Some(proxy),
            Ok(Err(err)) => {
                debug!("skip unsupported proxy item: {} {}", err, raw);
                None
            }
            Err(_) => {
                debug!("skip panicking proxy item: {}", raw);
                None
            }
        }
    }

    /// 移除重复节点
    pub fn exclude_dup_proxies(proxies: Vec<Proxy>) -> Vec<Proxy> {
        let mut new_proxies = Vec::new();
        if !proxies.is_empty() {
            let set: HashSet<Proxy> = HashSet::from_iter(proxies);
            new_proxies = set.into_iter().collect();
            new_proxies.sort_by(|a, b| a.proxy_type.cmp(&b.proxy_type));
        }
        new_proxies
    }

    /// 重置节点名称
    #[allow(dead_code)]
    pub fn unset_proxies_name(proxies: &mut Vec<Proxy>) {
        for proxy in proxies {
            let server = proxy.get_server().to_string();
            let hash = &mut DefaultHasher::new();
            proxy.to_json().unwrap().hash(hash);
            let h = hash.finish();
            proxy.set_name(&(server + "_" + &h.to_string()[..5]));
        }
    }

    /// 重命名相同名称的节点，在末尾加序号
    pub fn rename_dup_proxies_name(proxies: &mut Vec<Proxy>) {
        let mut name_counts: HashMap<String, usize> = HashMap::new();
        let number_suffix = Regex::new(r"\d+$").unwrap();

        // 打点，并删除其中原有的数字后缀
        for proxy in proxies.iter_mut() {
            let mut name = proxy.get_name().to_string();
            name = number_suffix.replace(&name, "").to_string();
            proxy.set_name(&name);
            *name_counts.entry(name).or_insert(0) += 1;
        }

        for proxy in &mut *proxies {
            let name = proxy.get_name().to_string();
            if let Some(count) = name_counts.get(&name) {
                if count > &1 {
                    let mut counter = 1;
                    let mut new_name = format!("{}{}", &name, counter);
                    while name_counts.contains_key(&new_name) {
                        counter += 1;
                        new_name = format!("{}{}", &name, counter);
                    }

                    proxy.set_name(&new_name);
                    name_counts.insert(new_name, 1);
                }
            }
        }

        // 以名称重新排序
        proxies.sort_by(|a, b| a.get_name().cmp(b.get_name()));
    }

    // 通过配置格式，获取 clash 配置文件内容
    pub fn get_clash_config_content(
        config_path: String,
        new_proxies: &Vec<Proxy>,
    ) -> io::Result<String> {
        let mut file = File::open(config_path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let mut yaml: Value = serde_yaml::from_str(&contents).expect("Failed to parse YAML");

        // 插入 proxies
        if let Some(proxies) = yaml.get_mut("proxies").and_then(Value::as_sequence_mut) {
            for proxy in new_proxies {
                proxies.push(Value::Mapping(
                    serde_yaml::from_str::<Mapping>(&proxy.to_json()?).unwrap(),
                ));
            }
        } else {
            println!("Failed to find 'proxies' in the YAML file");
        }

        // 处理 proxy-groups 逻辑
        if let Some(groups) = yaml
            .get_mut("proxy-groups")
            .and_then(Value::as_sequence_mut)
        {
            for group in groups.iter_mut() {
                if let Some(group_map) = group.as_mapping_mut() {
                    if let Some(Value::String(filter)) =
                        group_map.get(Value::String("filter".to_string()))
                    {
                        let regex = Regex::new(filter).expect("Invalid regex");
                        if let Some(proxies) = group_map
                            .get_mut(Value::String("proxies".to_string()))
                            .and_then(Value::as_sequence_mut)
                        {
                            let mut removed_default = false;
                            for proxy in new_proxies {
                                if regex.is_match(proxy.get_name()) {
                                    if !removed_default
                                        && proxies
                                            .first()
                                            .is_some_and(|p| p.as_str().unwrap().eq("PROXY"))
                                    {
                                        proxies.remove(0);
                                        removed_default = true;
                                    }
                                    proxies.push(Value::String(proxy.get_name().to_string()));
                                }
                            }
                            if proxies.is_empty() {
                                proxies.push(Value::String("DIRECT".to_string()));
                            }
                        }
                    }
                }
            }
        }
        Ok(serde_yaml::to_string(&yaml).expect("Failed to serialize YAML"))
    }

    pub fn save_proxies_into_clash_file(
        proxies: &Vec<Proxy>,
        config_path: String,
        save_path: String,
    ) {
        let content = SubManager::get_clash_config_content(config_path, proxies).unwrap();
        let mut file = File::create(&save_path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    pub fn get_links_content(proxies: &[Proxy]) -> String {
        Self::get_filtered_links_content(proxies, Self::is_v2rayn_supported_proxy)
    }

    pub fn get_basic_links_content(proxies: &[Proxy]) -> String {
        Self::get_filtered_links_content(proxies, Self::is_v2rayn_basic_proxy)
    }

    fn get_filtered_links_content(proxies: &[Proxy], predicate: fn(&Proxy) -> bool) -> String {
        proxies
            .iter()
            .filter(|proxy| predicate(proxy))
            .filter_map(|proxy| {
                catch_unwind(AssertUnwindSafe(|| proxy.adapter.to_link()))
                    .map_err(|_| {
                        info!(
                            "skip proxy {} during link export because to_link is not fully supported",
                            proxy.get_name()
                        );
                    })
                    .ok()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn is_v2rayn_supported_proxy(proxy: &Proxy) -> bool {
        matches!(
            proxy.proxy_type,
            ProxyType::SS
                | ProxyType::Vmess
                | ProxyType::Vless
                | ProxyType::Trojan
                | ProxyType::Hysteria2
                | ProxyType::Socks5
                | ProxyType::WireGuard
        )
    }

    fn is_v2rayn_basic_proxy(proxy: &Proxy) -> bool {
        matches!(
            proxy.proxy_type,
            ProxyType::SS | ProxyType::Vmess | ProxyType::Vless | ProxyType::Trojan
        )
    }

    pub fn save_proxies_into_links_file(proxies: &[Proxy], save_path: String) {
        let mut file = File::create(&save_path).unwrap();
        let content = Self::get_links_content(proxies);
        file.write_all(content.as_bytes()).unwrap();
    }

    pub fn save_proxies_into_base64_file(proxies: &[Proxy], save_path: String) {
        let mut file = File::create(&save_path).unwrap();
        let content = base64encode(Self::get_links_content(proxies));
        file.write_all(content.as_bytes()).unwrap();
    }

    pub fn save_basic_proxies_into_links_file(proxies: &[Proxy], save_path: String) {
        let mut file = File::create(&save_path).unwrap();
        let content = Self::get_basic_links_content(proxies);
        file.write_all(content.as_bytes()).unwrap();
    }

    pub fn save_basic_proxies_into_base64_file(proxies: &[Proxy], save_path: String) {
        let mut file = File::create(&save_path).unwrap();
        let content = base64encode(Self::get_basic_links_content(proxies));
        file.write_all(content.as_bytes()).unwrap();
    }
}

#[cfg(test)]
mod test {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::protocol;
    use crate::protocol::ProxyType::Hysteria2;
    use crate::protocol::ProxyType::Vless;
    use crate::protocol::ProxyType::Vmess;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf()
    }

    fn repo_path(relative: &str) -> String {
        repo_root().join(relative).to_string_lossy().into_owned()
    }

    fn test_output_path(name: &str) -> String {
        let dir = repo_root().join("target").join("test-artifacts");
        fs::create_dir_all(&dir).expect("create test artifact dir");
        dir.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn test_get_clash_config_content() {
        let path = repo_path("conf/clash_release.yaml");
        let mut proxies =
            SubManager::parse_from_path(repo_path("tests/res/base64_proxies")).unwrap();
        SubManager::unset_proxies_name(&mut proxies);
        let content = SubManager::get_clash_config_content(path, &proxies).unwrap();
        println!("{}", content);
    }

    #[test]
    fn test_urls_type() {
        let link = "ss://YWVzLTEyOC1nY206ZDljNTc3MzI4ZmIzNDlmZQ==@120.232.73.68:40676#%F0%9F%87%AD%F0%9F%87%B0HK";
        assert!(!Path::new(link).is_file());

        let path = repo_root().join("tests").join("res").join("base64_proxies");
        assert!(path.is_file());
    }

    #[tokio::test]
    #[ignore]
    async fn test_parse_conf() {
        let url = "https://github.com/ripaojiedian/freenode/raw/refs/heads/main/sub".to_string();
        let proxies = SubManager::get_proxies_from_url(url).await;
        for proxy in &proxies {
            println!("{:?}", proxy);
        }
    }

    #[test]
    fn test_regex_filter() {
        let filter = "台湾|TW|Tw|Taiwan|新北|彰化|CHT|HINET";
        let name = "JP_Tokyo_Shenzhen lesuyun Network Technology";
        let is_match = Regex::new(filter).unwrap().is_match(name);
        assert!(!is_match);
    }

    #[test]
    fn test_rename_dup_proxies_name() {
        let content = String::from(
            "ss://cmM0LW1kNToydnpobzU=@120.241.144.101:2410#name\n\
        ss://cmM0LW1kNToydnpobzU=@120.241.144.101:2410#name1\n\
        ss://cmM0LW1kNToydnpobzU=@120.241.144.101:2410#name1\n\
        ss://cmM0LW1kNToydnpobzU=@120.241.144.101:2410#name\n\
        ss://cmM0LW1kNToydnpobzU=@120.241.144.101:2410#xixi",
        );

        let mut proxies = SubManager::parse_content(content).unwrap();
        assert_eq!(proxies.len(), 5);
        assert_eq!(proxies.first().unwrap().get_name(), "name");
        assert_eq!(proxies.get(1).unwrap().get_name(), "name1");
        assert_eq!(proxies.get(2).unwrap().get_name(), "name1");
        assert_eq!(proxies.get(3).unwrap().get_name(), "name");
        assert_eq!(proxies.get(4).unwrap().get_name(), "xixi");
        SubManager::rename_dup_proxies_name(&mut proxies);
        assert_eq!(proxies.len(), 5);
        assert_eq!(proxies.first().unwrap().get_name(), "name1");
        assert_eq!(proxies.get(1).unwrap().get_name(), "name2");
        assert_eq!(proxies.get(2).unwrap().get_name(), "name3");
        assert_eq!(proxies.get(3).unwrap().get_name(), "name4");
        assert_eq!(proxies.get(4).unwrap().get_name(), "xixi");
    }

    #[test]
    fn test_parse_payload_yaml() {
        let content = r#"
payload:
  - name: payload-ss
    type: ss
    server: 1.1.1.1
    port: 443
    cipher: aes-128-gcm
    password: pass
"#
        .to_string();
        let proxies = SubManager::parse_content(content).unwrap();
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].get_name(), "payload-ss");
    }

    #[test]
    fn test_parse_base64_yaml_content() {
        let yaml = r#"
proxies:
  - name: base64-yaml-ss
    type: ss
    server: 2.2.2.2
    port: 443
    cipher: aes-128-gcm
    password: pass
"#;
        let content = crate::base64::base64encode(yaml.to_string());
        let proxies = SubManager::parse_content(content).unwrap();
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].get_name(), "base64-yaml-ss");
    }

    #[test]
    fn test_parse_mixed_links_and_hy2_alias() {
        let content = r#"
订阅说明文本
hy2://pass@127.0.0.1:8443/?insecure=1&sni=example.com#hy2-node
末尾还有一个 ss://YWVzLTEyOC1nY206cGFzcw==@3.3.3.3:443#ss-node
"#
        .to_string();
        let proxies = SubManager::parse_content(content).unwrap();
        assert_eq!(proxies.len(), 2);
        assert!(proxies.iter().any(|proxy| proxy.get_name() == "hy2-node"));
        assert!(proxies.iter().any(|proxy| proxy.get_name() == "ss-node"));
    }

    #[test]
    fn test_parse_links_skips_invalid_entries_without_panicking() {
        let content = r#"
ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:not-a-port#broken
ss://YWVzLTEyOC1nY206ZDljNTc3MzI4ZmIzNDlmZQ==@120.232.73.68:40676#good
"#
        .to_string();
        let proxies = SubManager::parse_content(content).unwrap();
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].get_name(), "good");
    }

    #[tokio::test]
    async fn test_merge_config() {
        let urls = vec![
            "hysteria2://bc97f674-c578-4940-9234-0a1da46041b9@188.68.234.53:36604/?sni=www.bing.com&alpn=h3&insecure=1#s14panel".to_string(),
            "hysteria2://bc97f674-c578-4940-9234-0a1da46041b9@188.68.248.8:8220?peer=www.bing.com&insecure=1&alpn=h3#s15panel".to_string(),
            "ss://Y2hhY2hhMjA6c0pNZGNJN05QakAxNC4xOC4yNTMuMTc4OjkwMDU#175.29.122.147_OpenAI".to_string(),
            "ss://Y2hhY2hhMjA6djVhVVV0bWUzanhzQDE0LjE4LjI1My4xNzg6OTAwMw#175.29.122.149_OpenAI_Claude".to_string(),
            "vmess://YXV0bzo5MjA0YWZjZC0wMjNlLTc4MWYtMWFiYy1jMTJlZmNjZDEzNDRAMTgzLjIzMi4xOTcuMjIzOjMzODAz?remarks=Tokyo-Akamai-H&path=/ray&obfs=websocket&tls=1&alterId=0".to_string(),
            "vmess://YXV0bzo5MjA0YWZjZC0wMjNlLTc4MWYtMWFiYy1jMTJlZmNjZDEzNDRAMTEzLjU2LjIxOC4xMzozMzgwMA?remarks=Tokyo-Akamai-H&path=/ray&obfs=websocket&tls=1&alterId=0".to_string(),
            "vmess://YXV0bzo5MjA0YWZjZC0wMjNlLTc4MWYtMWFiYy1jMTJlZmNjZDEzNDRAMTIyLjE5NS4xODkuMTI0OjMzODAw?remarks=Tokyo-Akamai-H&path=/ray&obfs=websocket&tls=1&alterId=0".to_string(),
            "vmess://YXV0bzo5MjA0YWZjZC0wMjNlLTc4MWYtMWFiYy1jMTJlZmNjZDEzNDRANDMuMjQ4LjExOS4xNDU6MzM0MDc?remarks=%E9%A6%99%E6%B8%AF%E9%98%BF%E9%87%8C%E4%BA%91-H&path=/ray&obfs=websocket&tls=1&alterId=0".to_string(),
        ];
        let proxies = SubManager::get_proxies_from_urls(&urls).await;
        let release_clash_template_path = repo_path("conf/clash_release.yaml");
        let save_path = test_output_path("proxy-merge.yaml");
        SubManager::save_proxies_into_clash_file(&proxies, release_clash_template_path, save_path);
    }

    #[test]
    fn test_extract_proxy_provider_urls() {
        let content = r#"
proxy-providers:
  one:
    type: http
    url: https://example.com/one.yaml
  two:
    type: http
    url: https://example.com/two.yaml
"#;
        let urls = SubManager::extract_proxy_provider_urls(content);
        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .any(|item| item == "https://example.com/one.yaml"));
        assert!(urls
            .iter()
            .any(|item| item == "https://example.com/two.yaml"));
    }

    #[tokio::test]
    async fn test_rename() {
        let urls = vec![repo_path("clash.yaml")];
        let mut proxies = SubManager::get_proxies_from_urls(&urls).await;
        SubManager::rename_dup_proxies_name(&mut proxies);
        let release_clash_template_path = repo_path("conf/clash_release.yaml");
        let save_path = test_output_path("clash1.yaml");
        SubManager::save_proxies_into_clash_file(&proxies, release_clash_template_path, save_path)
    }

    #[tokio::test]
    #[ignore = "manual exploratory fixture test"]
    async fn test_merge_uuids() {
        let mut proxies = SubManager::get_proxies_from_url(repo_path("clash.yaml")).await;

        let mut result = vec![];
        let uuids = vec![
            "f425df23-6ab6-449a-87bd-3ba74fdc1777",
            "742104bc-2d31-4139-9db1-848e36713207",
            "839ebb68-8a8b-4cf3-aa78-bf8d8721cd04",
        ];

        for uuid in uuids {
            for proxy in &mut proxies {
                println!("{:?}", proxy);
                if proxy.proxy_type.eq(&Vless) {
                    if let Some(vless) = proxy
                        .adapter
                        .as_any()
                        .downcast_ref::<protocol::vless::Vless>()
                    {
                        let mut p = vless.clone();
                        p.uuid = uuid.to_string();
                        proxy.adapter = Box::new(p);
                        result.push(proxy.clone());
                    }
                } else if proxy.proxy_type.eq(&Vmess) {
                    if let Some(vmess) = proxy
                        .adapter
                        .as_any()
                        .downcast_ref::<protocol::vmess::Vmess>()
                    {
                        let mut p = vmess.clone();
                        p.uuid = uuid.to_string();
                        proxy.adapter = Box::new(p);
                        result.push(proxy.clone());
                    }
                } else if proxy.proxy_type.eq(&Hysteria2) {
                    if let Some(hysteria2) = proxy
                        .adapter
                        .as_any()
                        .downcast_ref::<protocol::hysteria2::Hysteria2>()
                    {
                        let mut p = hysteria2.clone();
                        p.password = uuid.to_string();
                        proxy.adapter = Box::new(p);
                        result.push(proxy.clone());
                    }
                }
            }
        }

        SubManager::rename_dup_proxies_name(&mut result);

        SubManager::save_proxies_into_clash_file(
            &result,
            repo_path("conf/clash_release.yaml"),
            test_output_path("2025.02.17.yaml"),
        );

        println!("{:?}", result.len());
    }
}
