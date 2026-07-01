use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use proxrs::protocol::Proxy;
use proxrs::sub::SubManager;
use serde::Deserialize;
use serde::Serialize;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

use crate::clash::ClashMeta;
use crate::clash::DelayTestConfig;
use crate::settings::Settings;

mod artifacts;
mod cgi_trace;
mod clash;
mod discovery;
mod ip;
mod node_label;
mod pipeline;
mod risk;
mod routes;
mod server;
mod settings;
mod source_registry;
mod speedtest;
mod website;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(long)]
    server: bool,

    #[arg(long)]
    export_v2rayn_from_jsonl: Option<String>,
}

const TEST_PROXY_GROUP_NAME: &str = "PROXY";
const V2RAYN_SUB_PATH: &str = "v2rayn.txt";
const V2RAYN_LINKS_PATH: &str = "v2rayn-links.txt";
const V2RAYN_BASIC_SUB_PATH: &str = "v2rayn-basic.txt";
const V2RAYN_BASIC_LINKS_PATH: &str = "v2rayn-basic-links.txt";
const VALIDATED_POOL_MIHOMO_ARTIFACT: &str = "12_validated_pool_mihomo.json";

#[derive(Debug, Clone)]
struct CachedValidatedProxy {
    name: String,
    metadata: pipeline::ValidatedPoolMetadata,
}

#[derive(Debug)]
struct LiveProbeResult {
    renamed_name: String,
    metadata: pipeline::ValidatedPoolMetadata,
}

#[tokio::main]
async fn main() {
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .finish(),
    )
    .expect("setting default subscriber failed");

    let args = Cli::parse();
    if let Some(jsonl_path) = args.export_v2rayn_from_jsonl.as_deref() {
        create_folder();
        match export_v2rayn_from_jsonl(jsonl_path) {
            Ok(exported_count) => info!(
                "exported {} proxies into {} and {}",
                exported_count, V2RAYN_SUB_PATH, V2RAYN_LINKS_PATH
            ),
            Err(err) => error!("export v2rayN outputs failed: {}", err),
        }
        return;
    }

    let config = Settings::new();
    match config {
        Ok(config) => {
            create_folder();
            if args.server {
                // server::start_server(config).await
            } else {
                run(config).await
            }
        }
        Err(err) => error!("read config failed: {}", err),
    }
}

async fn run(config: Settings) {
    let direct_subs = canonicalize_subscription_sources(&config.subs);
    let discovery_feeds = dedupe_strings(
        config
            .discover_feeds
            .iter()
            .map(|url| discovery::canonicalize_registry_url(url))
            .collect(),
    );
    let pool_sources = canonicalize_subscription_sources(&config.pools);
    let artifact_store = artifacts::ArtifactStore::new(&config.artifacts);
    let mut source_registry =
        source_registry::SourceRegistry::load_or_default(&config.source_registry.path);

    let test_yaml_path = "subs/test/config.yaml";
    let test_all_yaml_path = "subs/test/all.yaml";
    let release_yaml_path = env::current_dir().unwrap().join("clash.yaml");
    let v2rayn_sub_path = env::current_dir().unwrap().join(V2RAYN_SUB_PATH);
    let v2rayn_links_path = env::current_dir().unwrap().join(V2RAYN_LINKS_PATH);
    let v2rayn_basic_sub_path = env::current_dir().unwrap().join(V2RAYN_BASIC_SUB_PATH);
    let v2rayn_basic_links_path = env::current_dir().unwrap().join(V2RAYN_BASIC_LINKS_PATH);
    let test_clash_template_path = "conf/clash_test.yaml";
    let release_clash_template_path = "conf/clash_release.yaml";

    let source_inputs = pipeline::SourceInputsArtifact {
        direct_subs: direct_subs.clone(),
        discovery_enabled: config.discover_enabled,
        discovery_feeds: discovery_feeds.clone(),
        pool_enabled: config.need_add_pool,
        pool_sources: pool_sources.clone(),
    };
    write_artifact_json(&artifact_store, "01_source_inputs.json", &source_inputs);

    for source_url in &direct_subs {
        source_registry.mark_seed_source(source_url, "direct_subscription");
    }
    if config.need_add_pool {
        for pool_url in &pool_sources {
            source_registry.mark_seed_source(pool_url, "pool_subscription");
        }
    }
    if config.discover_enabled {
        for feed_url in &discovery_feeds {
            source_registry.mark_seed_source(feed_url, "discovery_feed");
        }
    }

    let discovery_report = if config.discover_enabled {
        discovery::discover_sub_urls_with_report(&discovery_feeds).await
    } else {
        discovery::DiscoveryReport::default()
    };
    for feed_result in &discovery_report.feeds {
        source_registry.mark_feed_scan(
            &feed_result.feed,
            &feed_result.discovered_urls,
            feed_result.error.as_deref(),
        );
    }
    write_artifact_json(
        &artifact_store,
        "02_discovery_report.json",
        &discovery_report,
    );

    let mut urls = direct_subs.clone();
    if !discovery_report.unique_discovered_urls.is_empty() {
        info!(
            "found {} extra subscription sources from {} discovery feeds",
            discovery_report.unique_discovered_urls.len(),
            discovery_feeds.len()
        );
        urls.extend(discovery_report.unique_discovered_urls.clone());
    }
    if config.need_add_pool {
        urls.extend(pool_sources.clone());
    }
    urls = canonicalize_subscription_sources(&urls);

    let candidate_sources = pipeline::CandidateSourcesArtifact {
        sources: urls.clone(),
        total_count: urls.len(),
    };
    write_artifact_json(
        &artifact_store,
        "03_candidate_sources.json",
        &candidate_sources,
    );

    let mut source_fetch_results = Vec::new();
    let mut raw_test_proxies = Vec::new();
    let mut fingerprint_sources: HashMap<String, BTreeSet<String>> = HashMap::new();
    for source_url in &urls {
        let fetched_proxies = SubManager::get_proxies_from_url(source_url.clone()).await;
        source_registry.mark_fetch_result(source_url, fetched_proxies.len());
        source_fetch_results.push(pipeline::SourceFetchArtifact {
            source_url: source_url.clone(),
            proxy_count: fetched_proxies.len(),
            status: if fetched_proxies.is_empty() {
                "empty".to_string()
            } else {
                "ok".to_string()
            },
        });

        for proxy in fetched_proxies {
            if let Some(fingerprint) = pipeline::proxy_fingerprint(&proxy) {
                fingerprint_sources
                    .entry(fingerprint)
                    .or_default()
                    .insert(source_url.clone());
            }
            raw_test_proxies.push(proxy);
        }
    }
    write_artifact_json(
        &artifact_store,
        "04_source_fetch_results.json",
        &source_fetch_results,
    );

    let raw_proxy_count = raw_test_proxies.len();
    let mut test_proxies = raw_test_proxies;
    if !test_proxies.is_empty() {
        test_proxies = SubManager::exclude_dup_proxies(test_proxies);
        SubManager::rename_dup_proxies_name(&mut test_proxies);
    }
    let unique_proxy_count = test_proxies.len();
    let candidate_proxy_artifacts =
        pipeline::build_proxy_artifacts(&test_proxies, &fingerprint_sources);
    write_artifact_json_lines(
        &artifact_store,
        "05_candidate_proxies.jsonl",
        &candidate_proxy_artifacts,
    );

    info!("pending test proxies: {}", &test_proxies.len());
    if test_proxies.is_empty() {
        error!("no usable proxy candidates found");
        source_registry.apply_last_validated_counts(&BTreeMap::new());
        source_registry.apply_last_released_counts(&BTreeMap::new());
        let summary = pipeline::PipelineSummaryArtifact {
            candidate_source_count: urls.len(),
            raw_proxy_count,
            unique_proxy_count,
            useful_proxy_count: 0,
            final_release_proxy_count: 0,
        };
        write_artifact_json(&artifact_store, "09_pipeline_summary.json", &summary);
        persist_source_registry(
            &artifact_store,
            &mut source_registry,
            config.source_registry.enabled,
            &config.source_registry.path,
        );
        return;
    }

    SubManager::save_proxies_into_clash_file(
        &test_proxies,
        test_clash_template_path.to_string(),
        test_all_yaml_path.to_string(),
    );

    let chunk_size = config.test_group_size;
    let proxies_group: Vec<_> = test_proxies
        .chunks(chunk_size)
        .map(|items| items.to_vec())
        .collect();
    let group_size = proxies_group.len();
    if group_size > 1 {
        info!(
            "split proxies into {} groups with chunk size {}",
            proxies_group.len(),
            chunk_size
        );
    }

    let external_port = 9095;
    let mixed_port = 7998;
    let mut useful_proxies = Vec::new();
    let mut delay_group_artifacts = Vec::new();
    for (index, proxies) in proxies_group.iter().enumerate() {
        if group_size > 1 {
            info!("testing proxy group {}", index + 1);
        }

        SubManager::save_proxies_into_clash_file(
            proxies,
            test_clash_template_path.to_string(),
            test_yaml_path.to_string(),
        );

        let mut clash_meta = ClashMeta::new(external_port, mixed_port);
        if let Err(err) = clash_meta.start().await {
            error!("start clash meta failed: {}", err);
            clash_meta.stop().unwrap();
            continue;
        }

        match clash_meta.get_group(TEST_PROXY_GROUP_NAME).await {
            Ok(nodes) => info!("group nodes count: {}", nodes.all.len()),
            Err(err) => {
                error!("get group nodes failed: {}", err);
                clash_meta.stop().unwrap();
                continue;
            }
        }

        let delay_results = test_node_with_delay_config(&clash_meta, &config.connect_test).await;
        let nodes = get_all_tested_nodes(&delay_results);
        delay_group_artifacts.push(pipeline::DelayGroupArtifact {
            group_index: index + 1,
            input_proxy_count: proxies.len(),
            delay_rounds: delay_results.clone(),
            surviving_node_names: nodes.clone(),
        });
        info!("usable nodes in group {}: {}", index + 1, nodes.len());
        if !nodes.is_empty() {
            let current_useful = proxies
                .iter()
                .filter(|proxy| nodes.contains(&proxy.get_name().to_string()))
                .cloned()
                .collect::<Vec<Proxy>>();
            useful_proxies.extend(current_useful);
        }
        clash_meta.stop().unwrap();
    }
    write_artifact_json(
        &artifact_store,
        "06_delay_groups.json",
        &delay_group_artifacts,
    );

    if useful_proxies.is_empty() {
        error!("no proxies survived connectivity tests");
        source_registry.apply_last_validated_counts(&BTreeMap::new());
        source_registry.apply_last_released_counts(&BTreeMap::new());
        let summary = pipeline::PipelineSummaryArtifact {
            candidate_source_count: urls.len(),
            raw_proxy_count,
            unique_proxy_count,
            useful_proxy_count: 0,
            final_release_proxy_count: 0,
        };
        write_artifact_json(&artifact_store, "09_pipeline_summary.json", &summary);
        persist_source_registry(
            &artifact_store,
            &mut source_registry,
            config.source_registry.enabled,
            &config.source_registry.path,
        );
        return;
    }

    info!(
        "useful proxies after connectivity test: {}",
        useful_proxies.len()
    );
    let useful_source_counts =
        pipeline::count_sources_for_proxies(&useful_proxies, &fingerprint_sources);
    source_registry.apply_last_validated_counts(&useful_source_counts);
    let useful_proxy_artifacts =
        pipeline::build_proxy_artifacts(&useful_proxies, &fingerprint_sources);
    write_artifact_json_lines(
        &artifact_store,
        "07_useful_proxies.jsonl",
        &useful_proxy_artifacts,
    );

    let timeout: Duration = Duration::from_millis(config.connect_test.timeout + 2000);
    let useful_proxy_fingerprints = useful_proxies
        .iter()
        .filter_map(|proxy| {
            pipeline::proxy_fingerprint(proxy)
                .map(|fingerprint| (proxy.get_name().to_string(), fingerprint))
        })
        .collect::<HashMap<String, String>>();
    let mut validated_pool_metadata: HashMap<String, pipeline::ValidatedPoolMetadata> =
        HashMap::new();
    let cached_validated_proxies =
        load_cached_validated_proxy_map(artifact_store.path(VALIDATED_POOL_MIHOMO_ARTIFACT));
    let (mut release_proxies, proxies_needing_live_probe) = reuse_cached_validated_proxies(
        useful_proxies,
        &cached_validated_proxies,
        &mut validated_pool_metadata,
    );
    if !cached_validated_proxies.is_empty() {
        info!(
            "reused {} previously validated proxies, probing {} remaining proxies live",
            release_proxies.len(),
            proxies_needing_live_probe.len()
        );
    }
    if config.fast_mode {
        if !proxies_needing_live_probe.is_empty() {
            warn!(
                "fast mode skips {} unprobed proxies; only cached supports_google=true proxies can enter release pool",
                proxies_needing_live_probe.len()
            );
        }
        if release_proxies.is_empty() {
            error!("no proxies survived Google-specific release gate");
        } else {
            SubManager::rename_dup_proxies_name(&mut release_proxies);
            SubManager::save_proxies_into_clash_file(
                &release_proxies,
                release_clash_template_path.to_string(),
                release_yaml_path.to_string_lossy().to_string(),
            );
            write_v2rayn_outputs(
                &release_proxies,
                v2rayn_sub_path.to_string_lossy().to_string(),
                v2rayn_links_path.to_string_lossy().to_string(),
                v2rayn_basic_sub_path.to_string_lossy().to_string(),
                v2rayn_basic_links_path.to_string_lossy().to_string(),
            );
            info!("release file: {}", release_yaml_path.to_string_lossy());
        }
    } else {
        if !proxies_needing_live_probe.is_empty() {
            let mut clash_meta = ClashMeta::new(external_port, mixed_port);
            SubManager::save_proxies_into_clash_file(
                &proxies_needing_live_probe,
                test_clash_template_path.to_string(),
                test_yaml_path.to_string(),
            );

            match clash_meta.start().await {
                Ok(_) => {
                    let nodes = &mut proxies_needing_live_probe
                        .iter()
                        .map(|proxy| proxy.get_name().to_string())
                        .collect::<Vec<String>>();
                    let mut node_rename_map: HashMap<String, String> = HashMap::new();
                    let mut index = 0;
                    while index < nodes.len() {
                        let node = nodes[index].clone();
                        if let Some(probe_result) =
                            probe_release_proxy(&clash_meta, &config, &node, timeout).await
                        {
                            if let Some(fingerprint) = useful_proxy_fingerprints.get(&node) {
                                validated_pool_metadata
                                    .insert(fingerprint.clone(), probe_result.metadata);
                            }
                            node_rename_map.insert(node.clone(), probe_result.renamed_name);
                            index += 1;
                        } else {
                            nodes.remove(index);
                        }
                    }

                    let mut live_release_proxies = proxies_needing_live_probe
                        .into_iter()
                        .filter(|proxy| nodes.contains(&proxy.get_name().to_string()))
                        .collect::<Vec<Proxy>>();
                    if !node_rename_map.is_empty() {
                        for proxy in &mut live_release_proxies {
                            let name = if let Some(new_name) = node_rename_map.get(proxy.get_name())
                            {
                                new_name.clone()
                            } else {
                                proxy.get_name().to_string()
                            };
                            proxy.set_name(&name);
                        }
                    }
                    release_proxies.extend(live_release_proxies);
                }
                Err(err) => {
                    error!("restart clash meta for rename stage failed: {}", err);
                    if !release_proxies.is_empty() {
                        warn!(
                            "falling back to {} cached validated proxies because live probe stage could not start",
                            release_proxies.len()
                        );
                    }
                }
            }
            clash_meta.stop().unwrap();
        }

        if !release_proxies.is_empty() {
            SubManager::rename_dup_proxies_name(&mut release_proxies);
        }
        SubManager::save_proxies_into_clash_file(
            &release_proxies,
            release_clash_template_path.to_string(),
            release_yaml_path.to_string_lossy().to_string(),
        );
        write_v2rayn_outputs(
            &release_proxies,
            v2rayn_sub_path.to_string_lossy().to_string(),
            v2rayn_links_path.to_string_lossy().to_string(),
            v2rayn_basic_sub_path.to_string_lossy().to_string(),
            v2rayn_basic_links_path.to_string_lossy().to_string(),
        );
        info!("release file: {}", release_yaml_path.to_string_lossy());
    }

    let release_source_counts =
        pipeline::count_sources_for_proxies(&release_proxies, &fingerprint_sources);
    source_registry.apply_last_released_counts(&release_source_counts);
    let release_proxy_artifacts =
        pipeline::build_proxy_artifacts(&release_proxies, &fingerprint_sources);
    write_artifact_json_lines(
        &artifact_store,
        "08_final_release_proxies.jsonl",
        &release_proxy_artifacts,
    );
    let validated_pool = pipeline::build_validated_pool(&release_proxies, &fingerprint_sources);
    write_artifact_json(&artifact_store, "11_validated_pool.json", &validated_pool);
    let validated_pool_mihomo = pipeline::build_validated_pool_mihomo(
        &release_proxies,
        &fingerprint_sources,
        &validated_pool_metadata,
    );
    write_artifact_json(
        &artifact_store,
        "12_validated_pool_mihomo.json",
        &validated_pool_mihomo,
    );
    let summary = pipeline::PipelineSummaryArtifact {
        candidate_source_count: urls.len(),
        raw_proxy_count,
        unique_proxy_count,
        useful_proxy_count: useful_proxy_artifacts.len(),
        final_release_proxy_count: release_proxy_artifacts.len(),
    };
    write_artifact_json(&artifact_store, "09_pipeline_summary.json", &summary);
    persist_source_registry(
        &artifact_store,
        &mut source_registry,
        config.source_registry.enabled,
        &config.source_registry.path,
    );
}

#[allow(dead_code)]
fn get_top_node(test_results: &Vec<HashMap<String, i64>>) -> (String, i64) {
    let mut combined_data: HashMap<String, Vec<i64>> = HashMap::new();
    for test in test_results {
        for (node, latency) in test {
            combined_data
                .entry(node.clone())
                .or_default()
                .push(*latency);
        }
    }
    let node_stats: Vec<(String, i64)> = combined_data
        .clone()
        .into_iter()
        .map(|(node, latencies)| {
            let sum: i64 = latencies.iter().sum();
            let count = latencies.len() as i64;
            let mean = sum / count;
            (node, mean)
        })
        .collect();
    node_stats
        .into_iter()
        .min_by_key(|(_, mean)| *mean)
        .unwrap()
}

async fn test_node_with_delay_config(
    clash_meta: &ClashMeta,
    delay_test_config: &DelayTestConfig,
) -> Vec<HashMap<String, i64>> {
    const ROUND: i32 = 5;
    info!("delay test config: {:?}", delay_test_config);
    let mut delay_results = vec![];

    for _ in 0..2 {
        let _ = clash_meta
            .test_group(TEST_PROXY_GROUP_NAME, delay_test_config)
            .await;
    }

    for round in 0..ROUND {
        info!("delay round {}", round + 1);
        let result = clash_meta
            .test_group(TEST_PROXY_GROUP_NAME, delay_test_config)
            .await;

        match result {
            Ok(delay) => {
                delay_results.push(delay.clone());
                info!("delay result node count {}", delay.len())
            }
            Err(err) => info!("delay round failed: {}", err),
        }
    }
    delay_results
}

fn get_all_tested_nodes(test_results: &Vec<HashMap<String, i64>>) -> Vec<String> {
    let mut keys_set = HashSet::new();
    for result in test_results {
        for key in result.keys() {
            keys_set.insert(key.clone());
        }
    }
    keys_set.into_iter().collect()
}

#[allow(dead_code)]
fn get_stable_tested_nodes(test_results: &Vec<HashMap<String, i64>>) -> Vec<String> {
    let mut combined_data: HashMap<String, Vec<i64>> = HashMap::new();
    for test in test_results {
        for (node, latency) in test {
            combined_data
                .entry(node.clone())
                .or_default()
                .push(*latency);
        }
    }

    let mut node_stats: Vec<(String, f64)> = combined_data
        .clone()
        .into_iter()
        .filter_map(|(node, latencies)| {
            let sum: i64 = latencies.iter().sum();
            let count = latencies.len();
            if count <= combined_data.len() / 2 {
                None
            } else {
                let mean = sum as f64 / count as f64;
                Some((node, mean))
            }
        })
        .collect();

    node_stats.sort_by(|left, right| left.1.partial_cmp(&right.1).unwrap());
    node_stats.into_iter().map(|(node, _)| node).collect()
}

fn create_folder() {
    let logs_path = "logs";
    if !Path::new(logs_path).exists() {
        fs::create_dir(logs_path).unwrap()
    }

    let subs_path = "subs";
    if !Path::new(subs_path).exists() {
        fs::create_dir(subs_path).unwrap();
    }

    let test_path = "subs/test";
    if !Path::new(test_path).exists() {
        fs::create_dir(test_path).unwrap();
    }

    let release_path = "subs/release";
    if !Path::new(release_path).exists() {
        fs::create_dir(release_path).unwrap();
    }
}

#[derive(Deserialize)]
struct ReleaseProxyJsonLine {
    json: String,
}

fn export_v2rayn_from_jsonl(path: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut proxies = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let item: ReleaseProxyJsonLine = serde_json::from_str(&line).map_err(|err| {
            std::io::Error::other(format!(
                "parse release proxy jsonl line {} failed: {}",
                line_no + 1,
                err
            ))
        })?;
        let proxy = Proxy::from_json(&item.json).map_err(|err| {
            std::io::Error::other(format!(
                "rebuild proxy from line {} failed: {}",
                line_no + 1,
                err
            ))
        })?;
        proxies.push(proxy);
    }

    write_v2rayn_outputs(
        &proxies,
        V2RAYN_SUB_PATH.to_string(),
        V2RAYN_LINKS_PATH.to_string(),
        V2RAYN_BASIC_SUB_PATH.to_string(),
        V2RAYN_BASIC_LINKS_PATH.to_string(),
    );
    Ok(proxies.len())
}

fn write_v2rayn_outputs(
    proxies: &[Proxy],
    base64_path: String,
    links_path: String,
    basic_base64_path: String,
    basic_links_path: String,
) {
    SubManager::save_proxies_into_base64_file(proxies, base64_path);
    SubManager::save_proxies_into_links_file(proxies, links_path);
    SubManager::save_basic_proxies_into_base64_file(proxies, basic_base64_path);
    SubManager::save_basic_proxies_into_links_file(proxies, basic_links_path);
}

fn load_cached_validated_proxy_map(path: PathBuf) -> HashMap<String, CachedValidatedProxy> {
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return HashMap::new(),
    };
    let items = match serde_json::from_str::<Vec<pipeline::ValidatedPoolMihomoItem>>(&content) {
        Ok(items) => items,
        Err(err) => {
            warn!(
                "failed to parse cached validated pool at {}: {}",
                path.display(),
                err
            );
            return HashMap::new();
        }
    };
    build_cached_validated_proxy_map(items)
}

fn build_cached_validated_proxy_map(
    items: Vec<pipeline::ValidatedPoolMihomoItem>,
) -> HashMap<String, CachedValidatedProxy> {
    let mut cache = HashMap::new();
    for item in items {
        if item.name.trim().is_empty() {
            continue;
        }
        if !item.supports_gemini && !item.supports_claude {
            continue;
        }
        if !item.supports_google {
            continue;
        }
        cache.insert(
            item.fingerprint,
            CachedValidatedProxy {
                name: item.name,
                metadata: pipeline::ValidatedPoolMetadata {
                    exit_ip: item.exit_ip,
                    country: item.country,
                    country_code: item.country_code,
                    region: item.region,
                    city: item.city,
                    isp: item.isp,
                    supports_google: item.supports_google,
                    supports_gemini: item.supports_gemini,
                    supports_claude: item.supports_claude,
                },
            },
        );
    }
    cache
}

fn reuse_cached_validated_proxies(
    useful_proxies: Vec<Proxy>,
    cache: &HashMap<String, CachedValidatedProxy>,
    validated_pool_metadata: &mut HashMap<String, pipeline::ValidatedPoolMetadata>,
) -> (Vec<Proxy>, Vec<Proxy>) {
    let mut reused = Vec::new();
    let mut pending = Vec::new();

    for mut proxy in useful_proxies {
        let Some(fingerprint) = pipeline::proxy_fingerprint(&proxy) else {
            pending.push(proxy);
            continue;
        };
        let Some(cached) = cache.get(&fingerprint) else {
            pending.push(proxy);
            continue;
        };

        proxy.set_name(&cached.name);
        validated_pool_metadata.insert(fingerprint, cached.metadata.clone());
        reused.push(proxy);
    }

    (reused, pending)
}

async fn probe_release_proxy(
    clash_meta: &ClashMeta,
    config: &Settings,
    node: &str,
    timeout: Duration,
) -> Option<LiveProbeResult> {
    match clash_meta
        .set_group_proxy(TEST_PROXY_GROUP_NAME, node)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            error!(
                "set group proxy {} failed: controller returned non-success",
                node
            );
            return None;
        }
        Err(err) => {
            error!("set group proxy {} failed: {}", node, err);
            return None;
        }
    }

    let (proxy_ip, from) = match cgi_trace::get_ip(&clash_meta.proxy_url, timeout).await {
        Ok(result) => result,
        Err(err) => {
            error!("get ip for node {} failed: {}", node, err);
            return None;
        }
    };
    info!("node {} ip {} from {}", node, proxy_ip, from);

    let google_is_ok = if config.google_test.enabled {
        let google_timeout = Duration::from_millis(config.google_test.timeout);
        match website::google_is_ok(
            &clash_meta.proxy_url,
            &config.google_test.url,
            config.google_test.expected,
            google_timeout,
        )
        .await
        {
            Ok(_) => {
                info!("node {} google ok", node);
                true
            }
            Err(err) => {
                error!("node {} google failed: {:#}", node, err);
                return None;
            }
        }
    } else {
        true
    };

    let (gemini_result, claude_result, ip_detail_result) = tokio::join!(
        website::gemini_is_ok(&clash_meta.proxy_url, timeout),
        website::claude_is_ok(&clash_meta.proxy_url, timeout),
        ip::get_ip_detail(&proxy_ip, &clash_meta.proxy_url)
    );

    let mut gemini_is_ok = false;
    match gemini_result {
        Ok(_) => {
            info!("node {} gemini ok", node);
            gemini_is_ok = true;
        }
        Err(err) => error!("node {} gemini failed: {:#}", node, err),
    }

    let mut claude_is_ok = false;
    match claude_result {
        Ok(_) => {
            info!("node {} claude ok", node);
            claude_is_ok = true;
        }
        Err(err) => error!("node {} claude failed: {:#}", node, err),
    }

    if !gemini_is_ok && !claude_is_ok {
        return None;
    }

    let metadata = pipeline::ValidatedPoolMetadata::from_probe_result(
        &proxy_ip.to_string(),
        ip_detail_result.as_ref().ok(),
        google_is_ok,
        gemini_is_ok,
        claude_is_ok,
    );

    let mut new_name = proxy_ip.to_string();
    match ip_detail_result {
        Ok(ip_detail) => {
            info!("{:?}", ip_detail);
            if config.rename_node {
                new_name =
                    node_label::render_node_name(&config.rename_pattern, &proxy_ip, &ip_detail);
            }
        }
        Err(err) => error!("get ip detail for {} failed: {}", node, err),
    }

    if gemini_is_ok {
        new_name += "_Gemini";
    }
    if claude_is_ok {
        new_name += "_Claude";
    }

    Some(LiveProbeResult {
        renamed_name: new_name,
        metadata,
    })
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

fn canonicalize_subscription_sources(items: &[String]) -> Vec<String> {
    dedupe_strings(
        items
            .iter()
            .flat_map(|item| discovery::canonicalize_trusted_subscription_urls(item))
            .collect(),
    )
}

fn write_artifact_json<T: Serialize>(
    artifact_store: &artifacts::ArtifactStore,
    relative_path: &str,
    value: &T,
) {
    if let Err(err) = artifact_store.write_json(relative_path, value) {
        error!("write artifact {} failed: {}", relative_path, err);
    }
}

fn write_artifact_json_lines<T: Serialize>(
    artifact_store: &artifacts::ArtifactStore,
    relative_path: &str,
    values: &[T],
) {
    if let Err(err) = artifact_store.write_json_lines(relative_path, values) {
        error!("write artifact {} failed: {}", relative_path, err);
    }
}

fn persist_source_registry(
    artifact_store: &artifacts::ArtifactStore,
    source_registry: &mut source_registry::SourceRegistry,
    persist_registry_file: bool,
    registry_path: &str,
) {
    source_registry.updated_at_epoch_secs = current_epoch_secs();
    write_artifact_json(
        artifact_store,
        "10_source_registry.json",
        &source_registry.snapshot(),
    );
    if persist_registry_file && source_registry.persist(registry_path).is_err() {
        error!("persist source registry failed: {}", registry_path);
    }
}

fn current_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cached_validated_proxy_map_skips_unusable_entries() {
        let cache = build_cached_validated_proxy_map(vec![
            pipeline::ValidatedPoolMihomoItem {
                fingerprint: "fp-ok".to_string(),
                proxy_type: "Vless".to_string(),
                name: "cached-node".to_string(),
                server: "example.com".to_string(),
                source_urls: Vec::new(),
                source_count: 0,
                json: "{}".to_string(),
                exit_ip: "1.1.1.1".to_string(),
                country: "United States".to_string(),
                country_code: "US".to_string(),
                region: "California".to_string(),
                city: "Los Angeles".to_string(),
                isp: "Example ISP".to_string(),
                region_hint: "US".to_string(),
                supports_google: true,
                supports_gemini: true,
                supports_claude: false,
            },
            pipeline::ValidatedPoolMihomoItem {
                fingerprint: "fp-skip".to_string(),
                proxy_type: "Vless".to_string(),
                name: "skip-node".to_string(),
                server: "example.org".to_string(),
                source_urls: Vec::new(),
                source_count: 0,
                json: "{}".to_string(),
                exit_ip: "2.2.2.2".to_string(),
                country: String::new(),
                country_code: String::new(),
                region: String::new(),
                city: String::new(),
                isp: String::new(),
                region_hint: "OTHER".to_string(),
                supports_google: false,
                supports_gemini: false,
                supports_claude: false,
            },
        ]);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("fp-ok").unwrap().name, "cached-node");
        assert!(!cache.contains_key("fp-skip"));
    }

    #[test]
    fn test_build_cached_validated_proxy_map_requires_google_gate() {
        let cache = build_cached_validated_proxy_map(vec![pipeline::ValidatedPoolMihomoItem {
            fingerprint: "fp-google-missing".to_string(),
            proxy_type: "Vless".to_string(),
            name: "old-cache-node".to_string(),
            server: "example.com".to_string(),
            source_urls: Vec::new(),
            source_count: 0,
            json: "{}".to_string(),
            exit_ip: "1.1.1.1".to_string(),
            country: "United States".to_string(),
            country_code: "US".to_string(),
            region: "California".to_string(),
            city: "Los Angeles".to_string(),
            isp: "Example ISP".to_string(),
            region_hint: "US".to_string(),
            supports_google: false,
            supports_gemini: true,
            supports_claude: true,
        }]);

        assert!(cache.is_empty());
    }

    #[test]
    fn test_get_stable_nodes() {
        let test_data = vec![
            HashMap::from([
                ("node1".to_string(), 100),
                ("node2".to_string(), 200),
                ("node3".to_string(), 150),
            ]),
            HashMap::from([
                ("node1".to_string(), 110),
                ("node2".to_string(), 190),
                ("node3".to_string(), 160),
            ]),
            HashMap::from([("node1".to_string(), 120), ("node3".to_string(), 10000)]),
        ];

        println!("{:?}", get_top_node(&test_data));
    }

    #[test]
    fn test_rename_pattern() {
        let count = "${COUNTRYCODE}_${CITY}_${ISP}".matches('_').count();
        println!("{count}");
        let count = "HongKong_Jordan_VertexConnectivityLLC62"
            .matches('_')
            .count();
        println!("{count}");
    }
}
