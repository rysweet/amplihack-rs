//! Smart NODE_OPTIONS memory configuration for launcher startup.

use crate::util::{is_noninteractive, read_user_input_with_timeout};
use anyhow::Result;
use serde_json::{Map, Value};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const MIN_MEMORY_MB: u64 = 8192;
const MAX_MEMORY_MB: u64 = 32768;
const MEMORY_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConfig {
    pub system_ram_gb: Option<u64>,
    pub recommended_limit_mb: u64,
    pub current_limit_mb: Option<u64>,
    pub node_options: String,
    pub user_consent: Option<bool>,
    pub returning_user: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SavedPreference {
    consent: bool,
}

pub fn prepare_memory_config(existing_node_options: Option<&str>) -> Result<MemoryConfig> {
    let config = resolve_memory_config(existing_node_options)?;
    println!("{}", display_memory_config(&config));
    Ok(config)
}

fn resolve_memory_config(existing_node_options: Option<&str>) -> Result<MemoryConfig> {
    let existing_node_options = existing_node_options.unwrap_or_default();
    let current_limit_mb = extract_memory_limit(existing_node_options);

    let Some(system_ram_gb) = detect_system_ram_gb() else {
        return Ok(MemoryConfig {
            system_ram_gb: None,
            recommended_limit_mb: MAX_MEMORY_MB,
            current_limit_mb,
            node_options: merge_node_options(existing_node_options, MAX_MEMORY_MB),
            user_consent: None,
            returning_user: false,
            error: Some("Failed to detect system RAM".to_string()),
        });
    };

    let recommended_limit_mb = calculate_recommended_limit(system_ram_gb);
    let saved_preference = load_user_preference();
    let mut user_consent = None;
    let mut returning_user = false;

    if let Some(preference) = saved_preference {
        user_consent = Some(preference.consent);
        returning_user = true;
    } else if current_limit_mb.is_none() || current_limit_mb != Some(recommended_limit_mb) {
        let config = MemoryConfig {
            system_ram_gb: Some(system_ram_gb),
            recommended_limit_mb,
            current_limit_mb,
            node_options: merge_node_options(existing_node_options, recommended_limit_mb),
            user_consent: None,
            returning_user: false,
            error: None,
        };
        let consent = prompt_user_consent(&config)?;
        user_consent = Some(consent);
        save_user_preference(consent, recommended_limit_mb);
    }

    Ok(MemoryConfig {
        system_ram_gb: Some(system_ram_gb),
        recommended_limit_mb,
        current_limit_mb,
        node_options: merge_node_options(existing_node_options, recommended_limit_mb),
        user_consent,
        returning_user,
        error: None,
    })
}

fn detect_system_ram_gb() -> Option<u64> {
    if cfg!(target_os = "linux") {
        detect_ram_linux()
    } else if cfg!(target_os = "macos") {
        detect_ram_macos()
    } else if cfg!(target_os = "windows") {
        detect_ram_windows()
    } else {
        None
    }
}

fn detect_ram_linux() -> Option<u64> {
    let content = fs::read_to_string("/proc/meminfo").ok()?;
    let kb = content.lines().find_map(|line| {
        let value = line.strip_prefix("MemTotal:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    let gb = kb as f64 / 1024.0 / 1024.0;
    Some(round_to_power_of_two(gb))
}

fn detect_ram_macos() -> Option<u64> {
    let output = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()?;
    let gb = bytes as f64 / 1024f64.powi(3);
    Some(round_to_power_of_two(gb))
}

fn detect_ram_windows() -> Option<u64> {
    let output = Command::new("wmic")
        .args(["ComputerSystem", "get", "TotalPhysicalMemory"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .find_map(|line| line.trim().parse::<u64>().ok())?;
    let gb = bytes as f64 / 1024f64.powi(3);
    Some(round_to_power_of_two(gb))
}

fn round_to_power_of_two(gb: f64) -> u64 {
    let rounded = gb.max(1.0);
    let nearest_power = 2f64.powf(rounded.log2().round());
    if ((rounded - nearest_power).abs() / nearest_power) < 0.25 {
        nearest_power.round() as u64
    } else {
        rounded.round() as u64
    }
}

fn calculate_recommended_limit(ram_gb: u64) -> u64 {
    let quarter_ram_mb = (ram_gb * 1024) / 4;
    quarter_ram_mb.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB)
}

fn extract_memory_limit(options: &str) -> Option<u64> {
    let tokens = tokenize_node_options(options);
    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];
        if let Some(value) = token.strip_prefix("--max-old-space-size=") {
            return value.parse::<u64>().ok();
        }
        if token == "--max-old-space-size" && index + 1 < tokens.len() {
            return tokens[index + 1].parse::<u64>().ok();
        }
        index += 1;
    }
    None
}

fn merge_node_options(existing_options: &str, new_limit_mb: u64) -> String {
    let tokens = tokenize_node_options(existing_options);
    let mut merged = Vec::new();

    let mut skip_next = false;
    for token in tokens {
        if skip_next {
            skip_next = false;
            continue;
        }
        if token == "--max-old-space-size" {
            skip_next = true;
            continue;
        }
        if token.starts_with("--max-old-space-size=") {
            continue;
        }
        merged.push(token);
    }

    merged.push(format!("--max-old-space-size={new_limit_mb}"));

    merged.join(" ").trim().to_string()
}

fn tokenize_node_options(options: &str) -> Vec<String> {
    options
        .split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn prompt_user_consent(config: &MemoryConfig) -> Result<bool> {
    if is_noninteractive() {
        return Ok(true);
    }

    let system_ram = config.system_ram_gb.unwrap_or(0);
    println!(
        "Memory: {} GB RAM detected, recommend {} MB limit",
        system_ram, config.recommended_limit_mb
    );
    let response = read_user_input_with_timeout(
        "Update NODE_OPTIONS? [Y/n] (auto-yes in 30s): ",
        MEMORY_PROMPT_TIMEOUT,
    )?;
    let response = response.unwrap_or_default();
    let normalized = response.trim().to_ascii_lowercase();
    Ok(!matches!(normalized.as_str(), "n" | "no"))
}

fn display_memory_config(config: &MemoryConfig) -> String {
    if let Some(error) = &config.error {
        return format!("⚠ {error}. Set NODE_OPTIONS={}", config.node_options);
    }

    let config_path = config_path();
    if config.returning_user {
        if config.user_consent == Some(false) {
            return match config_path {
                Some(path) => format!(
                    "ℹ NODE_OPTIONS memory config: skipped (saved preference). To change: {}",
                    path.display()
                ),
                None => "ℹ NODE_OPTIONS memory config: skipped (saved preference).".to_string(),
            };
        }
        let limit =
            extract_memory_limit(&config.node_options).unwrap_or(config.recommended_limit_mb);
        return match config_path {
            Some(path) => format!(
                "ℹ NODE_OPTIONS=--max-old-space-size={} (saved preference). To change: {}",
                limit,
                path.display()
            ),
            None => format!(
                "ℹ NODE_OPTIONS=--max-old-space-size={} (saved preference)",
                limit
            ),
        };
    }

    if config.user_consent == Some(false) {
        "✗ Skipped NODE_OPTIONS update (user declined)".to_string()
    } else {
        let limit =
            extract_memory_limit(&config.node_options).unwrap_or(config.recommended_limit_mb);
        format!("✓ Set NODE_OPTIONS=--max-old-space-size={limit}")
    }
}

fn load_user_preference() -> Option<SavedPreference> {
    let config_path = config_path()?;
    let content = fs::read_to_string(config_path).ok()?;
    let value = serde_json::from_str::<Value>(&content).ok()?;
    Some(SavedPreference {
        consent: value.get("node_options_consent")?.as_bool()?,
    })
}

fn save_user_preference(consent: bool, limit_mb: u64) {
    let Some(config_path) = config_path() else {
        return;
    };
    if let Some(parent) = config_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        tracing::warn!(
            path = %parent.display(),
            "failed to create memory config directory: {error}"
        );
        return;
    }

    let mut value = fs::read_to_string(&config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !value.is_object() {
        value = Value::Object(Map::new());
    }
    let object = value
        .as_object_mut()
        .expect("memory config value should be a JSON object");
    object.insert("node_options_consent".to_string(), Value::Bool(consent));
    object.insert(
        "node_options_limit_mb".to_string(),
        Value::Number(limit_mb.into()),
    );

    if let Err(error) = fs::write(
        &config_path,
        serde_json::to_string_pretty(&value)
            .map(|json| json + "\n")
            .unwrap_or_else(|_| "{}\n".to_string()),
    ) {
        tracing::warn!(
            path = %config_path.display(),
            "failed to save memory config preference: {error}"
        );
    }
}

fn config_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".amplihack").join("config"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{home_env_lock, restore_home, set_home};

    #[test]
    fn calculate_recommended_limit_clamps_to_expected_range() {
        assert_eq!(calculate_recommended_limit(4), 8192);
        assert_eq!(calculate_recommended_limit(64), 16384);
        assert_eq!(calculate_recommended_limit(256), 32768);
    }

    #[test]
    fn merge_node_options_replaces_existing_memory_limit() {
        let merged =
            merge_node_options("--max-old-space-size=4096 --inspect --trace-warnings", 8192);
        assert_eq!(
            merged,
            "--inspect --trace-warnings --max-old-space-size=8192"
        );
    }

    #[test]
    fn merge_node_options_preserves_existing_option_order() {
        let merged = merge_node_options("--existing-option", 32768);

        assert_eq!(merged, "--existing-option --max-old-space-size=32768");
    }

    #[test]
    fn resolve_memory_config_uses_saved_preference_without_prompting() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        fs::create_dir_all(home.path().join(".amplihack")).unwrap();
        fs::write(
            home.path().join(".amplihack/config"),
            r#"{"node_options_consent":true,"node_options_limit_mb":16384}"#,
        )
        .unwrap();

        let config = resolve_memory_config(Some("--trace-warnings")).unwrap();

        restore_home(original_home);
        assert!(config.returning_user);
        assert_eq!(config.user_consent, Some(true));
        assert!(config.node_options.contains("--max-old-space-size="));
        assert!(config.node_options.contains("--trace-warnings"));
    }

    #[test]
    fn display_memory_config_formats_saved_preference_message() {
        let config = MemoryConfig {
            system_ram_gb: Some(128),
            recommended_limit_mb: 32768,
            current_limit_mb: Some(32768),
            node_options: "--max-old-space-size=32768".to_string(),
            user_consent: Some(true),
            returning_user: true,
            error: None,
        };

        let message = display_memory_config(&config);

        assert!(message.contains("saved preference"));
        assert!(message.contains("32768"));
    }
}
