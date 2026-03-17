use config::Config;
use config::ConfigError;
use config::File;
use serde::Deserialize;

use crate::clash::DelayTestConfig;
use crate::speedtest::SpeedTestConfig;

#[derive(Deserialize, Debug, Clone)]
pub struct ArtifactSettings {
    pub enabled: bool,
    pub dir: String,
}

impl Default for ArtifactSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: "artifacts".to_string(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct SourceRegistrySettings {
    pub enabled: bool,
    pub path: String,
}

impl Default for SourceRegistrySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "artifacts/source_registry.json".to_string(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct Settings {
    pub fast_mode: bool,
    pub subs: Vec<String>,
    #[serde(default)]
    pub discover_enabled: bool,
    #[serde(default)]
    pub discover_feeds: Vec<String>,
    pub rename_node: bool,
    pub rename_pattern: String,
    pub need_add_pool: bool,
    pub test_group_size: usize,
    pub pools: Vec<String>,
    #[serde(default)]
    pub artifacts: ArtifactSettings,
    #[serde(default)]
    pub source_registry: SourceRegistrySettings,
    pub connect_test: DelayTestConfig,
    pub speed_test: SpeedTestConfig,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let settings = Config::builder()
            .add_source(File::with_name("conf/config.toml"))
            .build()?;
        settings.try_deserialize::<Settings>()
    }
}
