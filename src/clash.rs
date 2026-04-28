#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use flate2::read::GzDecoder;
use reqwest::header::USER_AGENT;
use reqwest::Client;
use reqwest::RequestBuilder;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use tokio::time::sleep;
use tracing::{info, warn};
use zip::ZipArchive;

const CORE_DIR: &str = "clash-meta";
const UNIX_CORE_NAME: &str = "mihomo";
const WINDOWS_CORE_NAME: &str = "mihomo.exe";
const MIHOMO_RELEASE_API: &str = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
const GITHUB_USER_AGENT: &str = "autojiedian/0.1";
const GITHUB_TOKEN_ENV_KEYS: [&str; 3] = ["GITHUB_TOKEN", "GH_TOKEN", "GITHUB_AUTOJIEDIAN_PAT"];

pub struct ClashMeta {
    pub external_port: u64,
    pub mixed_port: u64,
    pub proxy_url: String,
    pub external_url: String,
    core_path: String,
    test_path: String,
    log_path: String,
    process: Option<Child>,
}

impl ClashMeta {
    pub fn new(external_port: u64, mixed_port: u64) -> Self {
        ClashMeta {
            external_port,
            mixed_port,
            external_url: format!("http://127.0.0.1:{}", external_port),
            proxy_url: format!("http://127.0.0.1:{}", mixed_port),
            process: None,
            core_path: default_core_path().to_string_lossy().to_string(),
            test_path: "subs/test".to_string(),
            log_path: "logs/clash.log".to_string(),
        }
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_core_ready().await?;

        let log_file = File::create(&self.log_path)?;
        let mut clash_process = Command::new(&self.core_path)
            .arg("-d")
            .arg(&self.test_path)
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .spawn()?;

        let version_check = self.wait_for_ready().await;
        match version_check {
            Ok(res) => {
                info!("mihomo started, version: {}", res.version);
                self.process = Some(clash_process);
                Ok(())
            }
            Err(err) => {
                let _ = clash_process.kill();
                let _ = clash_process.wait();
                Err(err)
            }
        }
    }

    pub async fn restart(&self) -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
        let response = client
            .post(format!("{}/restart", &self.external_url))
            .json(&json!({"path": self.test_path,"payload": ""}))
            .send()
            .await?;

        if response.status().is_success() {
            info!("mihomo restarted successfully");
            sleep(Duration::from_secs(2)).await;
        } else {
            info!("mihomo restart failed: {}", response.status());
        }
        Ok(())
    }

    pub fn stop(mut self) -> std::io::Result<()> {
        if let Some(mut process) = self.process.take() {
            process.kill()?;
            process.wait()?;
        }
        Ok(())
    }

    pub async fn get_group(&self, group_name: &str) -> Result<Group, Box<dyn std::error::Error>> {
        let url = format!("{}/group/{}", &self.external_url, group_name);
        let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
        let response = client.get(url).send().await?;
        let group = response.json::<Group>().await?;
        Ok(group)
    }

    pub async fn test_group(
        &self,
        group_name: &str,
        delay_test_config: &DelayTestConfig,
    ) -> Result<HashMap<String, i64>, Box<dyn std::error::Error>> {
        let url = format!("{}/group/{}/delay", &self.external_url, group_name);
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let response = client.get(&url).query(&delay_test_config).send().await?;
        if !response.status().is_success() {
            return Err(Box::new(std::io::Error::other(
                "failed to fetch delay result for proxy group",
            )));
        }
        let res: Value = response.json().await?;
        match res {
            Value::Object(map) => {
                if let Some(msg) = map.get("message") {
                    Err(Box::new(std::io::Error::other(msg.to_string())))
                } else {
                    let mut result = HashMap::new();
                    for (name, value) in map {
                        if let Some(num) = value.as_i64() {
                            result.insert(name.clone(), num);
                        }
                    }
                    Ok(result)
                }
            }
            _ => Err(Box::new(std::io::Error::other(
                "all proxies in the group failed delay test",
            ))),
        }
    }

    pub async fn test_proxy(
        &self,
        proxy_name: &str,
        delay_test_config: &DelayTestConfig,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let url = format!("{}/proxies/{}/delay", &self.external_url, proxy_name);
        let client = Client::builder().timeout(Duration::from_secs(60)).build()?;
        let response = client.get(&url).query(delay_test_config).send().await?;
        if !response.status().is_success() {
            return Err(Box::new(std::io::Error::other(
                "failed to fetch delay result for proxy",
            )));
        }
        Ok(response.json::<ProxyDelay>().await?.delay)
    }

    pub async fn test_direct_delay(&self) -> Result<u64, Box<dyn std::error::Error>> {
        self.test_proxy(
            "DIRECT",
            &DelayTestConfig {
                url: "http://www.gstatic.com/generate_204".to_string(),
                expected: Some(204),
                timeout: 200,
            },
        )
        .await
    }

    pub async fn set_group_proxy(
        &self,
        group_name: &str,
        proxy_name: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let url = format!("{}/proxies/{}", &self.external_url, group_name);
        let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
        let response = client
            .put(url)
            .json(&json!({"name": proxy_name}))
            .send()
            .await?;
        Ok(response.status().is_success())
    }

    async fn ensure_core_ready(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let expected_path = default_core_path();
        self.core_path = expected_path.to_string_lossy().to_string();
        fs::create_dir_all(CORE_DIR)?;

        if is_valid_core_binary(&expected_path)? {
            ensure_executable_permissions(&expected_path)?;
            return Ok(());
        }

        let platform = current_release_os().ok_or_else(|| {
            std::io::Error::other("unsupported platform for official mihomo download")
        })?;

        if cfg!(windows) {
            let legacy_path = Path::new(CORE_DIR).join(UNIX_CORE_NAME);
            if is_valid_core_binary(&legacy_path)? {
                warn!(
                    "found a valid Windows mihomo core at {}, copying it to {}",
                    legacy_path.display(),
                    expected_path.display()
                );
                fs::copy(&legacy_path, &expected_path)?;
                return Ok(());
            }
        }

        if expected_path.exists() {
            warn!(
                "existing mihomo core at {} is not a valid {} executable, downloading a fresh one",
                expected_path.display(),
                platform
            );
        } else {
            info!(
                "mihomo core missing at {}, downloading a {} build from the official release",
                expected_path.display(),
                platform
            );
        }

        download_core_for_current_platform(&expected_path).await?;
        ensure_executable_permissions(&expected_path)?;
        if is_valid_core_binary(&expected_path)? {
            return Ok(());
        }

        Err(Box::new(std::io::Error::other(format!(
            "downloaded mihomo core at {} is still invalid for {}",
            expected_path.display(),
            platform
        ))))
    }

    async fn wait_for_ready(&self) -> Result<ClashVersion, Box<dyn std::error::Error>> {
        let client = Client::builder().timeout(Duration::from_secs(2)).build()?;
        let version_url = format!("{}/version", &self.external_url);

        for _ in 0..10 {
            if let Ok(response) = client.get(&version_url).send().await {
                if let Ok(ok_response) = response.error_for_status() {
                    match ok_response.json::<ClashVersion>().await {
                        Ok(version) => return Ok(version),
                        Err(err) => return Err(Box::new(err)),
                    }
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        Err(Box::new(std::io::Error::other(format!(
            "mihomo did not become ready at {} within 10 seconds",
            version_url
        ))))
    }
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
struct ClashVersion {
    meta: bool,
    version: String,
}

#[derive(Deserialize, Debug)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize, Debug)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProxyDelay {
    pub delay: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct DelayTestConfig {
    pub url: String,
    pub expected: Option<u16>,
    pub timeout: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Group {
    pub all: Vec<String>,
    pub now: String,
    pub name: String,
}

fn default_core_path() -> PathBuf {
    if cfg!(windows) {
        Path::new(CORE_DIR).join(WINDOWS_CORE_NAME)
    } else {
        Path::new(CORE_DIR).join(UNIX_CORE_NAME)
    }
}

fn is_valid_core_binary(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(false);
    }

    let mut file = File::open(path)?;
    let mut magic = [0_u8; 4];
    let read_len = file.read(&mut magic)?;
    if read_len < 2 {
        return Ok(false);
    }

    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    Ok(binary_matches_platform(&magic[..read_len], os))
}

fn binary_matches_platform(header: &[u8], os: &str) -> bool {
    match os {
        "windows" => header.starts_with(b"MZ"),
        "macos" => {
            header.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
                || header.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
                || header.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
                || header.starts_with(&[0xBE, 0xBA, 0xFE, 0xCA])
        }
        _ => header.starts_with(&[0x7F, b'E', b'L', b'F']),
    }
}

fn current_release_os() -> Option<&'static str> {
    if cfg!(windows) {
        Some("windows")
    } else if cfg!(target_os = "macos") {
        Some("darwin")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else {
        None
    }
}

fn archive_extension_for_os(os: &str) -> &'static str {
    if os == "windows" {
        ".zip"
    } else {
        ".gz"
    }
}

fn ensure_executable_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

async fn download_core_for_current_platform(
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let release = with_optional_github_auth(client.get(MIHOMO_RELEASE_API))
        .header(USER_AGENT, GITHUB_USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .json::<GithubRelease>()
        .await?;

    let os = current_release_os().ok_or_else(|| {
        std::io::Error::other("unsupported platform for official mihomo download")
    })?;
    let asset =
        select_asset_for_platform(&release, os, std::env::consts::ARCH).ok_or_else(|| {
            std::io::Error::other(format!(
                "could not find a suitable {} mihomo asset in release {}",
                os, release.tag_name
            ))
        })?;

    info!("downloading official mihomo {} core: {}", os, asset.name);

    let zip_bytes = with_optional_github_auth(client.get(&asset.browser_download_url))
        .header(USER_AGENT, GITHUB_USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    if asset.name.ends_with(".zip") {
        extract_mihomo_from_zip(zip_bytes.as_ref(), target_path)?;
    } else if asset.name.ends_with(".gz") {
        extract_mihomo_from_gzip(zip_bytes.as_ref(), target_path)?;
    } else {
        return Err(Box::new(std::io::Error::other(format!(
            "unsupported mihomo asset format: {}",
            asset.name
        ))));
    }

    ensure_executable_permissions(target_path)?;
    info!("mihomo core saved to {}", target_path.display());
    Ok(())
}

fn with_optional_github_auth(request: RequestBuilder) -> RequestBuilder {
    if let Some(token) = github_api_token_from_env() {
        request.bearer_auth(token)
    } else {
        request
    }
}

fn github_api_token_from_env() -> Option<String> {
    GITHUB_TOKEN_ENV_KEYS.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn select_asset_for_platform<'a>(
    release: &'a GithubRelease,
    os: &str,
    arch: &str,
) -> Option<&'a GithubAsset> {
    let tag = release.tag_name.as_str();
    let exact_candidates = preferred_asset_names(os, arch, tag);
    for candidate in &exact_candidates {
        if let Some(asset) = release.assets.iter().find(|asset| asset.name == *candidate) {
            return Some(asset);
        }
    }

    let prefixes = preferred_asset_prefixes(os, arch);
    let extension = archive_extension_for_os(os);
    for prefix in prefixes {
        if let Some(asset) = release.assets.iter().find(|asset| {
            asset.name.starts_with(&prefix)
                && asset.name.ends_with(extension)
                && !asset.name.contains("-go")
        }) {
            return Some(asset);
        }
    }

    release.assets.iter().find(|asset| {
        asset.name.starts_with(&format!("mihomo-{}-", os))
            && asset.name.ends_with(extension)
            && !asset.name.contains("-go")
    })
}

fn preferred_asset_names(os: &str, arch: &str, tag: &str) -> Vec<String> {
    let extension = archive_extension_for_os(os);
    match (os, arch) {
        ("windows", "x86_64") | ("linux", "x86_64") | ("darwin", "x86_64") => vec![
            format!("mihomo-{}-amd64-compatible-{}{}", os, tag, extension),
            format!("mihomo-{}-amd64-{}{}", os, tag, extension),
            format!("mihomo-{}-amd64-v1-{}{}", os, tag, extension),
            format!("mihomo-{}-amd64-v2-{}{}", os, tag, extension),
            format!("mihomo-{}-amd64-v3-{}{}", os, tag, extension),
        ],
        ("windows", "aarch64") | ("linux", "aarch64") | ("darwin", "aarch64") => {
            vec![format!("mihomo-{}-arm64-{}{}", os, tag, extension)]
        }
        (_, "x86") => vec![format!("mihomo-{}-386-{}{}", os, tag, extension)],
        ("linux", "arm") => vec![
            format!("mihomo-linux-armv7-{}{}", tag, extension),
            format!("mihomo-linux-armv6-{}{}", tag, extension),
        ],
        _ => Vec::new(),
    }
}

fn preferred_asset_prefixes(os: &str, arch: &str) -> Vec<String> {
    match (os, arch) {
        ("windows", "x86_64") | ("linux", "x86_64") | ("darwin", "x86_64") => vec![
            format!("mihomo-{}-amd64-compatible-", os),
            format!("mihomo-{}-amd64-", os),
            format!("mihomo-{}-amd64-v1-", os),
            format!("mihomo-{}-amd64-v2-", os),
            format!("mihomo-{}-amd64-v3-", os),
        ],
        ("windows", "aarch64") | ("linux", "aarch64") | ("darwin", "aarch64") => {
            vec![format!("mihomo-{}-arm64-", os)]
        }
        (_, "x86") => vec![format!("mihomo-{}-386-", os)],
        ("linux", "arm") => vec![
            "mihomo-linux-armv7-".to_string(),
            "mihomo-linux-armv6-".to_string(),
        ],
        _ => Vec::new(),
    }
}

fn extract_mihomo_from_zip(
    zip_bytes: &[u8],
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let reader = Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(reader)?;
    let temp_path = target_path.with_extension("download");

    if temp_path.exists() {
        fs::remove_file(&temp_path)?;
    }

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }

        let file_name = Path::new(entry.name())
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        if is_mihomo_archive_entry(file_name) {
            let mut output = File::create(&temp_path)?;
            std::io::copy(&mut entry, &mut output)?;
            output.flush()?;

            if target_path.exists() {
                fs::remove_file(target_path)?;
            }

            fs::rename(&temp_path, target_path)?;
            return Ok(());
        }
    }

    Err(Box::new(std::io::Error::other(
        "the downloaded zip does not contain mihomo executable",
    )))
}

fn extract_mihomo_from_gzip(
    gzip_bytes: &[u8],
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_path = target_path.with_extension("download");
    if temp_path.exists() {
        fs::remove_file(&temp_path)?;
    }

    let mut decoder = GzDecoder::new(Cursor::new(gzip_bytes));
    let mut output = File::create(&temp_path)?;
    std::io::copy(&mut decoder, &mut output)?;
    output.flush()?;
    ensure_executable_permissions(&temp_path)?;

    if target_path.exists() {
        fs::remove_file(target_path)?;
    }

    fs::rename(&temp_path, target_path)?;
    Ok(())
}

fn is_mihomo_archive_entry(file_name: &str) -> bool {
    let normalized = file_name.to_ascii_lowercase();
    normalized == WINDOWS_CORE_NAME
        || normalized == UNIX_CORE_NAME
        || (normalized.starts_with("mihomo") && normalized.ends_with(".exe"))
}

#[cfg(test)]
mod tests {
    use crate::clash::binary_matches_platform;
    use crate::clash::is_mihomo_archive_entry;
    use crate::clash::select_asset_for_platform;
    use crate::clash::ClashMeta;
    use crate::clash::DelayTestConfig;
    use crate::clash::GithubAsset;
    use crate::clash::GithubRelease;

    #[test]
    fn test_binary_magic_matches_platform() {
        assert!(binary_matches_platform(b"MZ\x90\x00", "windows"));
        assert!(!binary_matches_platform(b"\x7FELF", "windows"));
        assert!(binary_matches_platform(b"\x7FELF", "linux"));
        assert!(binary_matches_platform(&[0xCF, 0xFA, 0xED, 0xFE], "macos"));
    }

    #[test]
    fn test_select_windows_asset_prefers_compatible_build() {
        let release = GithubRelease {
            tag_name: "v1.2.3".to_string(),
            assets: vec![
                GithubAsset {
                    name: "mihomo-windows-amd64-v1-v1.2.3.zip".to_string(),
                    browser_download_url: "https://example.com/v1.zip".to_string(),
                },
                GithubAsset {
                    name: "mihomo-windows-amd64-compatible-v1.2.3.zip".to_string(),
                    browser_download_url: "https://example.com/compatible.zip".to_string(),
                },
            ],
        };

        let selected = select_asset_for_platform(&release, "windows", "x86_64").unwrap();
        assert_eq!(selected.name, "mihomo-windows-amd64-compatible-v1.2.3.zip");
    }

    #[test]
    fn test_select_windows_asset_skips_go_variants_when_falling_back() {
        let release = GithubRelease {
            tag_name: "v1.2.3".to_string(),
            assets: vec![
                GithubAsset {
                    name: "mihomo-windows-amd64-v1-go125-v1.2.3.zip".to_string(),
                    browser_download_url: "https://example.com/go.zip".to_string(),
                },
                GithubAsset {
                    name: "mihomo-windows-amd64-v2-v1.2.3.zip".to_string(),
                    browser_download_url: "https://example.com/v2.zip".to_string(),
                },
            ],
        };

        let selected = select_asset_for_platform(&release, "windows", "x86_64").unwrap();
        assert_eq!(selected.name, "mihomo-windows-amd64-v2-v1.2.3.zip");
    }

    #[test]
    fn test_archive_entry_match_accepts_official_windows_binary_name() {
        assert!(is_mihomo_archive_entry(
            "mihomo-windows-amd64-compatible.exe"
        ));
        assert!(is_mihomo_archive_entry("mihomo.exe"));
        assert!(!is_mihomo_archive_entry("readme.txt"));
    }

    #[test]
    fn test_select_linux_asset_prefers_compatible_gzip_build() {
        let release = GithubRelease {
            tag_name: "v1.2.3".to_string(),
            assets: vec![
                GithubAsset {
                    name: "mihomo-linux-amd64-v2-v1.2.3.gz".to_string(),
                    browser_download_url: "https://example.com/v2.gz".to_string(),
                },
                GithubAsset {
                    name: "mihomo-linux-amd64-compatible-v1.2.3.gz".to_string(),
                    browser_download_url: "https://example.com/compatible.gz".to_string(),
                },
            ],
        };

        let selected = select_asset_for_platform(&release, "linux", "x86_64").unwrap();
        assert_eq!(selected.name, "mihomo-linux-amd64-compatible-v1.2.3.gz");
    }

    #[tokio::test]
    #[ignore]
    async fn test_proxy_delay() {
        let clash_meta = ClashMeta::new(9095, 7998);
        let delay = clash_meta
            .test_proxy(
                "DIRECT",
                &DelayTestConfig {
                    url: "http://www.gstatic.com/generate_204".to_string(),
                    expected: Some(204),
                    timeout: 500,
                },
            )
            .await
            .unwrap();
        println!("{}", delay);
    }

    #[tokio::test]
    #[ignore]
    async fn test_group_proxies() {
        let clash_meta = ClashMeta::new(9095, 7998);
        let result = clash_meta.get_group("PROXY").await;
        println!("{:?}", result);
    }

    #[tokio::test]
    #[ignore]
    async fn test_set_group_node() {
        let clash_meta = ClashMeta::new(9095, 7998);
        let result = clash_meta
            .set_group_proxy("PROXY", "None_None_vmess_044")
            .await;
        if result.is_ok() {
            println!("success")
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_group_delay() {
        let clash_meta = ClashMeta::new(9095, 7890);
        let result = clash_meta
            .test_group(
                "PROXY",
                &DelayTestConfig {
                    url: "http://www.google.com/generate_204".to_string(),
                    expected: Some(204),
                    timeout: 1000,
                },
            )
            .await;

        println!("{:?}", result);
    }
}
