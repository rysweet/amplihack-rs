//! Memory system configuration for Node.js processes.
//!
//! Matches Python `amplihack/launcher/memory_config.py`:
//! - Detect system RAM across Linux / macOS / Windows
//! - Calculate recommended `--max-old-space-size` limit
//! - Parse and merge `NODE_OPTIONS` strings
//! - Persist user consent in `~/.amplihack/config`

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

const MIN_MEMORY_MB: u64 = 8192;
const MAX_MEMORY_MB: u64 = 32768;

/// Memory configuration result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub system_ram_gb: Option<u64>,
    pub recommended_limit_mb: u64,
    pub current_limit_mb: Option<u64>,
    pub node_options: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_consent: Option<bool>,
    pub returning_user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Saved user preference for memory configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPreference {
    pub consent: bool,
    pub limit_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_options: Option<String>,
}

/// Detect system RAM in gigabytes.
pub fn detect_system_ram_gb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        detect_ram_linux()
    }
    #[cfg(target_os = "macos")]
    {
        detect_ram_macos()
    }
    #[cfg(target_os = "windows")]
    {
        detect_ram_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Formula: `clamp(ram_mb / 4, MIN, MAX)`.
pub fn calculate_recommended_limit(ram_gb: u64) -> u64 {
    let quarter = ram_gb.saturating_mul(1024) / 4;
    quarter.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB)
}

pub fn should_warn_about_limit(limit_mb: u64) -> bool {
    limit_mb < MIN_MEMORY_MB
}

/// Parse a `NODE_OPTIONS` string into key-value pairs.
pub fn parse_node_options(options_str: &str) -> Vec<(String, String)> {
    options_str
        .split_whitespace()
        .map(|token| {
            let stripped = token.trim_start_matches('-');
            if let Some((k, v)) = stripped.split_once('=') {
                (k.to_string(), v.to_string())
            } else {
                (stripped.to_string(), String::new())
            }
        })
        .collect()
}

/// Merge a new memory limit into existing `NODE_OPTIONS`.
pub fn merge_node_options(existing: &str, new_limit_mb: u64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut replaced = false;
    for token in existing.split_whitespace() {
        if token.starts_with("--max-old-space-size") || token.starts_with("--max_old_space_size") {
            if !replaced {
                parts.push(format!("--max-old-space-size={new_limit_mb}"));
                replaced = true;
            }
        } else {
            parts.push(token.to_string());
        }
    }
    if !replaced {
        parts.push(format!("--max-old-space-size={new_limit_mb}"));
    }
    parts.join(" ")
}

pub fn get_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".amplihack").join("config"))
}

pub fn load_user_preference() -> Option<MemoryPreference> {
    load_preference_from(&get_config_path()?).ok()
}

pub fn save_user_preference(consent: bool, limit_mb: u64) -> Result<()> {
    save_preference_to(
        &get_config_path().ok_or_else(|| anyhow::anyhow!("no config path"))?,
        consent,
        limit_mb,
    )
}

/// Main entry point: detect RAM, calculate limits, check saved preferences.
pub fn get_memory_config(existing_node_options: Option<&str>) -> MemoryConfig {
    let Some(ram_gb) = detect_system_ram_gb() else {
        debug!("Could not detect system RAM");
        return MemoryConfig {
            system_ram_gb: None,
            recommended_limit_mb: MIN_MEMORY_MB,
            current_limit_mb: None,
            node_options: existing_node_options.unwrap_or_default().into(),
            warning: None,
            user_consent: None,
            returning_user: false,
            error: Some("Could not detect system RAM".into()),
        };
    };
    let recommended = calculate_recommended_limit(ram_gb);
    info!(ram_gb, recommended_mb = recommended, "Memory configuration");
    let existing = existing_node_options.unwrap_or_default();
    let current_limit = extract_max_old_space(existing);
    let saved = load_user_preference();
    let returning_user = saved.is_some();
    let node_options = match &saved {
        Some(p) if p.consent => merge_node_options(existing, p.limit_mb),
        Some(_) => existing.to_string(),
        None => merge_node_options(existing, recommended),
    };
    let warning = should_warn_about_limit(current_limit.unwrap_or(recommended))
        .then(|| format!("Memory limit is below recommended {MIN_MEMORY_MB}MB for this system"));
    MemoryConfig {
        system_ram_gb: Some(ram_gb),
        recommended_limit_mb: recommended,
        current_limit_mb: current_limit,
        node_options,
        warning,
        user_consent: saved.map(|p| p.consent),
        returning_user,
        error: None,
    }
}

pub fn display_memory_config(config: &MemoryConfig) -> String {
    let mut lines = Vec::new();
    match config.system_ram_gb {
        Some(ram) => lines.push(format!("✓ System RAM: {ram} GB")),
        None => lines.push("✗ System RAM: unknown".into()),
    }
    lines.push(format!(
        "ℹ Recommended limit: {} MB",
        config.recommended_limit_mb
    ));
    if let Some(c) = config.current_limit_mb {
        lines.push(format!("ℹ Current limit: {c} MB"));
    }
    if let Some(ref w) = config.warning {
        lines.push(format!("⚠ {w}"));
    }
    lines.join("\n")
}

fn extract_max_old_space(options: &str) -> Option<u64> {
    options.split_whitespace().find_map(|t| {
        t.strip_prefix("--max-old-space-size=")
            .or_else(|| t.strip_prefix("--max_old_space_size="))
            .and_then(|v| v.parse().ok())
    })
}

fn load_preference_from(path: &Path) -> Result<MemoryPreference> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content).context("parse config")?;
    let section = value
        .get("memory_config")
        .ok_or_else(|| anyhow::anyhow!("no memory_config"))?;
    serde_json::from_value(section.clone()).context("parse preference")
}

fn save_preference_to(path: &Path, consent: bool, limit_mb: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut config: serde_json::Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(path)?)
            .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    config["memory_config"] = serde_json::json!({ "consent": consent, "limit_mb": limit_mb });
    std::fs::write(path, serde_json::to_string_pretty(&config)?)
        .with_context(|| format!("write {}", path.display()))?;
    info!(consent, limit_mb, "Saved memory preference");
    Ok(())
}

fn round_to_power_of_2(gb_float: f64) -> u64 {
    if gb_float <= 0.0 {
        return 0;
    }
    let nearest_pow2 = 2_u64.pow(gb_float.log2().round() as u32);
    let ratio = gb_float / nearest_pow2 as f64;
    if (0.75..=1.25).contains(&ratio) {
        nearest_pow2
    } else {
        gb_float.round() as u64
    }
}

#[cfg(target_os = "linux")]
fn detect_ram_linux() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
            return Some(round_to_power_of_2(kb as f64 / 1_048_576.0));
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn detect_ram_macos() -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(round_to_power_of_2(bytes as f64 / 1_073_741_824.0))
}

#[cfg(target_os = "windows")]
fn detect_ram_windows() -> Option<u64> {
    let out = std::process::Command::new("wmic")
        .args(["ComputerSystem", "get", "TotalPhysicalMemory"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
        if let Ok(bytes) = line.trim().parse::<u64>() {
            return Some(round_to_power_of_2(bytes as f64 / 1_073_741_824.0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_limit_16gb() {
        assert_eq!(calculate_recommended_limit(16), MIN_MEMORY_MB);
    }
    #[test]
    fn calculate_limit_64gb() {
        assert_eq!(calculate_recommended_limit(64), 16384);
    }
    #[test]
    fn calculate_limit_256gb() {
        assert_eq!(calculate_recommended_limit(256), MAX_MEMORY_MB);
    }
    #[test]
    fn warn_below_min() {
        assert!(should_warn_about_limit(4096));
        assert!(!should_warn_about_limit(8192));
    }
    #[test]
    fn parse_node_options_basic() {
        let opts = parse_node_options("--max-old-space-size=4096 --no-warnings");
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0], ("max-old-space-size".into(), "4096".into()));
    }
    #[test]
    fn parse_empty() {
        assert!(parse_node_options("").is_empty());
    }
    #[test]
    fn merge_replaces() {
        assert_eq!(
            merge_node_options("--max-old-space-size=4096 --no-warnings", 8192),
            "--max-old-space-size=8192 --no-warnings"
        );
    }
    #[test]
    fn merge_adds() {
        assert_eq!(
            merge_node_options("--no-warnings", 8192),
            "--no-warnings --max-old-space-size=8192"
        );
    }
    #[test]
    fn merge_empty() {
        assert_eq!(merge_node_options("", 16384), "--max-old-space-size=16384");
    }
    #[test]
    fn extract_found() {
        assert_eq!(
            extract_max_old_space("--max-old-space-size=4096"),
            Some(4096)
        );
    }
    #[test]
    fn extract_missing() {
        assert_eq!(extract_max_old_space("--no-warnings"), None);
    }
    #[test]
    fn round_pow2_exact() {
        assert_eq!(round_to_power_of_2(16.0), 16);
        assert_eq!(round_to_power_of_2(64.0), 64);
    }
    #[test]
    fn round_pow2_close() {
        assert_eq!(round_to_power_of_2(15.8), 16);
        assert_eq!(round_to_power_of_2(31.5), 32);
    }
    #[test]
    fn config_serializes() {
        let c = MemoryConfig {
            system_ram_gb: Some(64),
            recommended_limit_mb: 16384,
            current_limit_mb: Some(4096),
            node_options: "--max-old-space-size=16384".into(),
            warning: None,
            user_consent: Some(true),
            returning_user: true,
            error: None,
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["system_ram_gb"], 64);
        assert!(json.get("warning").is_none() || json["warning"].is_null());
    }
    #[test]
    fn preference_round_trips() {
        let p = MemoryPreference {
            consent: true,
            limit_mb: 16384,
            node_options: None,
        };
        let parsed: MemoryPreference =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(parsed.consent);
        assert_eq!(parsed.limit_mb, 16384);
    }
    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        save_preference_to(&path, true, 16384).unwrap();
        let loaded = load_preference_from(&path).unwrap();
        assert!(loaded.consent);
        assert_eq!(loaded.limit_mb, 16384);
    }
    #[test]
    fn save_merges_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        std::fs::write(&path, r#"{"other_key": "value"}"#).unwrap();
        save_preference_to(&path, false, 8192).unwrap();
        let c: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(c["other_key"], "value");
        assert_eq!(c["memory_config"]["consent"], false);
    }
    #[test]
    fn get_config_returns_result() {
        let c = get_memory_config(Some("--no-warnings"));
        assert!(!c.node_options.is_empty());
        assert!(c.recommended_limit_mb >= MIN_MEMORY_MB);
    }
    #[test]
    fn display_format() {
        let c = MemoryConfig {
            system_ram_gb: Some(32),
            recommended_limit_mb: 8192,
            current_limit_mb: Some(4096),
            node_options: String::new(),
            warning: Some("low memory".into()),
            user_consent: None,
            returning_user: false,
            error: None,
        };
        let out = display_memory_config(&c);
        assert!(out.contains("32 GB"));
        assert!(out.contains("8192 MB"));
        assert!(out.contains("low memory"));
    }
}
