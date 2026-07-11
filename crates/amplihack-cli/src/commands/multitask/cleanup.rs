use super::models::{WorkstreamConfig, sanitize_id};
use super::utils::dir_size_bytes;
use crate::util::run_output_with_timeout;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const GH_PR_VIEW_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn cleanup_merged(
    base_dir: &Path,
    state_dir: &Path,
    config_path: &str,
    dry_run: bool,
) -> Result<()> {
    let config_text = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {config_path}"))?;
    let items: Vec<WorkstreamConfig> = serde_json::from_str(&config_text)?;
    let mut deleted_count = 0u32;
    let mut freed_bytes = 0u64;

    for item in &items {
        let issue = item.issue_id();
        let safe_id = sanitize_id(&issue.to_string());
        let work_dir = base_dir.join(format!("ws-{issue}"));
        let state_file = state_dir.join(format!("ws-{safe_id}.json"));

        let mut gh_cmd = Command::new("gh");
        gh_cmd.args([
            "pr",
            "view",
            &item.branch,
            "--json",
            "state",
            "-q",
            ".state",
        ]);
        let is_merged = run_output_with_timeout(gh_cmd, GH_PR_VIEW_TIMEOUT)
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|s| s.trim() == "MERGED");
        if !is_merged || !work_dir.exists() {
            continue;
        }

        let dir_size = dir_size_bytes(&work_dir);
        if dry_run {
            println!(
                "[{issue}] Would delete work dir ({:.0}MB)",
                dir_size as f64 / (1024.0 * 1024.0)
            );
        } else {
            let _ = fs::remove_dir_all(&work_dir);
            let _ = fs::remove_file(&state_file);
            println!(
                "[{issue}] Deleted work dir ({:.0}MB freed)",
                dir_size as f64 / (1024.0 * 1024.0)
            );
        }
        freed_bytes += dir_size;
        deleted_count += 1;
    }

    let freed_gb = freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let prefix = if dry_run { "DRY RUN " } else { "" };
    let modal = if dry_run { "would be " } else { "" };
    println!(
        "\n{prefix}Summary:\n  Workstreams {modal}deleted: {deleted_count}\n  Disk space {modal}freed: {freed_gb:.2}GB"
    );

    if dry_run && deleted_count > 0 {
        println!("\nRun without --dry-run to actually delete these workstreams.");
    }
    Ok(())
}
