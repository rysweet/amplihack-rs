//! Settings.json configuration, hook contract validation, and framework verification.

use super::hooks::{ensure_array, ensure_object, update_hook_paths};
use super::paths::{global_settings_path, xpia_hooks_dir};
use super::types::*;
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Configure ~/.claude/settings.json with amplihack hook registrations.
///
/// Returns `(success, registered_event_names)` where `registered_event_names`
/// is a deduplicated list of event names that were configured.
pub(super) fn ensure_settings_json(
    staging_dir: &Path,
    timestamp: u64,
    hooks_bin: &Path,
) -> Result<(bool, Vec<String>)> {
    let settings_path = global_settings_path()?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let backup_path = settings_path
        .parent()
        .context("global settings path missing parent")?
        .join(format!("settings.json.backup.{timestamp}"));
    if settings_path.exists() {
        fs::copy(&settings_path, &backup_path).with_context(|| {
            format!(
                "failed to copy {} to {}",
                settings_path.display(),
                backup_path.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600));
        }
        let backup_dir = staging_dir.join("runtime").join("sessions");
        fs::create_dir_all(&backup_dir)
            .with_context(|| format!("failed to create {}", backup_dir.display()))?;
        // Use serde_json::json! to guarantee valid JSON even with special chars in paths
        let metadata = json!({
            "settings_path": settings_path.to_string_lossy().as_ref(),
            "backup_path": backup_path.to_string_lossy().as_ref(),
        });
        fs::write(
            backup_dir.join(format!("install_{timestamp}_backup.json")),
            serde_json::to_string_pretty(&metadata)?,
        )
        .context("failed to write install backup metadata")?;
        println!("  💾 Backup created at {}", backup_path.display());
        println!("  📋 Found existing settings.json");
    } else {
        fs::write(&settings_path, "{}\n")
            .with_context(|| format!("failed to write {}", settings_path.display()))?;
    }

    let mut settings = read_settings_json(&settings_path)?;
    ensure_permissions(&mut settings);
    update_hook_paths(&mut settings, "amplihack", AMPLIHACK_HOOK_SPECS, hooks_bin);

    let xpia_dir = xpia_hooks_dir()?;
    if xpia_dir.exists() {
        update_hook_paths(&mut settings, "xpia", XPIA_HOOK_SPECS, hooks_bin);
    }

    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )
    .with_context(|| format!("failed to write {}", settings_path.display()))?;
    println!("  ✅ settings.json configured");

    let hook_contract_drift = validate_amplihack_native_hook_contract(&settings);
    if hook_contract_drift.is_empty() {
        println!("  ✅ Native amplihack hook contract matches hooks.json");
    } else {
        // `update_hook_paths` rewrites amplihack-native hooks in canonical
        // order on every install, so surviving drift here indicates a real
        // spec mismatch (new hook added upstream, third-party wrapper that
        // masquerades as ours, etc.) rather than harmless reordering.
        println!("  ⚠️  Native amplihack hook contract drift detected after reorder:");
        for issue in hook_contract_drift {
            println!("     • {issue}");
        }
    }

    // Collect deduplicated event names that were registered
    let registered_events: Vec<String> = {
        let mut seen = BTreeSet::new();
        for spec in AMPLIHACK_HOOK_SPECS {
            seen.insert(spec.event.to_string());
        }
        seen.into_iter().collect()
    };

    Ok((true, registered_events))
}

pub(super) fn verify_framework_assets(claude_dir: &Path) -> Result<bool> {
    let missing = missing_framework_paths(claude_dir)?;
    if missing.is_empty() {
        println!("  ✅ Required framework assets found");
    } else {
        println!("  ❌ Missing required framework assets:");
        for path in missing {
            println!("     • {path}");
        }
    }

    let xpia_dir = xpia_hooks_dir()?;
    if !xpia_dir.exists() {
        println!("  ℹ️  XPIA security hooks not installed (optional feature)");
    } else {
        for file in XPIA_HOOK_FILES {
            let hook_path = xpia_dir.join(file);
            if hook_path.exists() {
                println!("    ✅ {file} found");
            } else {
                println!("    ❌ {file} missing");
            }
        }
    }

    Ok(missing_framework_paths(claude_dir)?.is_empty())
}

pub(super) fn read_settings_json(settings_path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(settings_path)
        .with_context(|| format!("failed to read {}", settings_path.display()))?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(Value::Object(map)),
        Ok(_) => Ok(Value::Object(Map::new())),
        Err(_) => {
            tracing::warn!(
                "Settings file {} contains invalid JSON, using empty defaults",
                settings_path.display()
            );
            Ok(Value::Object(Map::new()))
        }
    }
}

pub(super) fn ensure_permissions(settings: &mut Value) {
    let root = ensure_object(settings);
    let permissions = root
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()));
    let permissions = ensure_object(permissions);

    permissions
        .entry("allow")
        .or_insert_with(|| json!(["Bash", "TodoWrite", "WebSearch", "WebFetch"]));
    permissions
        .entry("deny")
        .or_insert_with(|| Value::Array(Vec::new()));
    permissions
        .entry("defaultMode")
        .or_insert_with(|| Value::String("bypassPermissions".to_string()));

    let additional = permissions
        .entry("additionalDirectories")
        .or_insert_with(|| json!([".claude", "Specs"]));
    let additional = ensure_array(additional);
    for dir in [".claude", "Specs"] {
        if !additional.iter().any(|value| value.as_str() == Some(dir)) {
            additional.push(Value::String(dir.to_string()));
        }
    }
}

pub(super) fn validate_amplihack_native_hook_contract(settings: &Value) -> Vec<String> {
    let mut drift = Vec::new();
    let mut seen_events = Vec::new();

    for entry in CANONICAL_AMPLIHACK_HOOK_CONTRACT {
        if !seen_events.contains(&entry.event) {
            seen_events.push(entry.event);
        }
    }

    for event in seen_events {
        let expected = canonical_native_hooks_for_event(event);
        let actual = observed_native_hooks_for_event(settings, event);

        for (index, expected_entry) in expected.iter().enumerate() {
            match actual.get(index) {
                Some(observed)
                    if observed.subcmd == expected_entry.native_subcmd.unwrap_or_default()
                        && observed.timeout == expected_entry.timeout
                        && observed.matcher.as_deref() == expected_entry.matcher => {}
                Some(observed) => drift.push(format!(
                    "{event}[{index}] expected {} but found {}",
                    describe_expected_native_hook(expected_entry),
                    describe_observed_native_hook(observed)
                )),
                None => drift.push(format!(
                    "{event}[{index}] missing {}; expected native hook order from hooks.json/HOOK_CONFIGS",
                    describe_expected_native_hook(expected_entry)
                )),
            }
        }

        for extra in actual.iter().skip(expected.len()) {
            drift.push(format!(
                "{event} has unexpected native hook {}",
                describe_observed_native_hook(extra)
            ));
        }
    }

    drift
}

fn canonical_native_hooks_for_event(event: &str) -> Vec<&'static CanonicalHookContractEntry> {
    CANONICAL_AMPLIHACK_HOOK_CONTRACT
        .iter()
        .filter(|entry| entry.event == event && entry.native_subcmd.is_some())
        .collect()
}

fn observed_native_hooks_for_event(
    settings: &Value,
    event: &str,
) -> Vec<ObservedNativeHookContractEntry> {
    settings
        .as_object()
        .and_then(|root| root.get("hooks"))
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|wrappers| wrappers.iter())
        .flat_map(|wrapper| {
            let matcher = wrapper
                .get("matcher")
                .and_then(Value::as_str)
                .map(str::to_string);
            wrapper
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flat_map(move |hooks| {
                    let matcher = matcher.clone();
                    hooks.iter().filter_map(move |hook| {
                        let command = hook.get("command").and_then(Value::as_str)?;
                        let subcmd = extract_native_hook_subcommand(command)?;
                        Some(ObservedNativeHookContractEntry {
                            subcmd: subcmd.to_string(),
                            timeout: hook.get("timeout").and_then(Value::as_u64),
                            matcher: matcher.clone(),
                        })
                    })
                })
        })
        .collect()
}

fn extract_native_hook_subcommand(command: &str) -> Option<&str> {
    command
        .contains("amplihack-hooks")
        .then(|| command.split_whitespace().last())
        .flatten()
}

fn describe_expected_native_hook(entry: &CanonicalHookContractEntry) -> String {
    format!(
        "subcmd=`{}`, timeout={}, matcher={}, source={}",
        entry.native_subcmd.unwrap_or_default(),
        format_optional_timeout(entry.timeout),
        format_optional_matcher(entry.matcher),
        entry.hook_file
    )
}

fn describe_observed_native_hook(entry: &ObservedNativeHookContractEntry) -> String {
    format!(
        "subcmd=`{}`, timeout={}, matcher={}",
        entry.subcmd,
        format_optional_timeout(entry.timeout),
        format_optional_matcher(entry.matcher.as_deref())
    )
}

fn format_optional_timeout(timeout: Option<u64>) -> String {
    timeout
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn format_optional_matcher(matcher: Option<&str>) -> String {
    matcher
        .map(str::to_string)
        .unwrap_or_else(|| "none".to_string())
}

pub(super) fn missing_framework_paths(claude_dir: &Path) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for dir in ESSENTIAL_DIRS {
        let path = claude_dir.join(dir);
        if !path.exists() {
            missing.push(format!("{dir} (expected at {})", path.display()));
        }
    }
    for file in ESSENTIAL_FILES {
        let path = claude_dir.join(file);
        if !path.exists() {
            missing.push(format!("{file} (expected at {})", path.display()));
        }
    }
    let claude_md = claude_dir
        .parent()
        .context("staging .claude dir missing parent")?
        .join("CLAUDE.md");
    if !claude_md.exists() {
        missing.push(format!("CLAUDE.md (expected at {})", claude_md.display()));
    }
    Ok(missing)
}
