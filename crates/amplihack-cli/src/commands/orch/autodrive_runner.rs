//! Runner-owned execution authority. Child output and persisted files are diagnostics only.
use super::autodrive_preflight::{self, PreflightRequest, RefusalOutcome};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(crate) fn known(outcome: &str) -> bool {
    serde_json::from_value::<RefusalOutcome>(Value::String(outcome.into())).is_ok()
}

pub(crate) fn inspect(
    recipe: &str,
    context: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<Value>> {
    if !matches!(recipe, "autodrive-crusty-loop" | "autodrive-merge-loop") {
        return Ok(None);
    }
    let pr = context
        .get("pr_number")
        .filter(|p| p.parse::<u64>().is_ok())
        .or_else(|| context.get("autodrive_pr_input"));
    let Some(pr) = pr.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let pr = pr.parse::<u64>().context("invalid auto-drive PR number")?;
    let cwd = context
        .get("repo_path")
        .filter(|p| !p.is_empty())
        .map(|p| cwd.join(p))
        .unwrap_or_else(|| cwd.to_path_buf());
    let platform = autodrive_preflight::output(
        &cwd,
        "gh",
        &["pr", "view", &pr.to_string(), "--json", "state,mergedAt"],
    )
    .ok()
    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if platform.as_ref().is_some_and(|p| {
        p["state"] == "MERGED" || p["mergedAt"].as_str().is_some_and(|m| !m.is_empty())
    }) {
        return Ok(None);
    }
    let slug = autodrive_preflight::git(&cwd, &["remote", "get-url", "origin"])
        .unwrap_or_else(|_| "local".into());
    let key: String = format!("{}/{pr}", slug.trim_end_matches(".git"))
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let root = std::env::var_os("AMPLIHACK_STATE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
                .join(".amplihack/state")
        });
    let state_dir = context
        .get("autodrive_state_dir")
        .filter(|p| !p.is_empty())
        .map(|p| cwd.join(p))
        .unwrap_or_else(|| root.join("auto-drive").join(key));
    if recipe == "autodrive-crusty-loop"
        && state_dir.exists()
        && autodrive_preflight::validate_state_directory(&state_dir).is_ok()
    {
        // Resume markers can skip a completed phase, but never assert a refusal.
        let marker = state_dir.join("phases.tsv");
        let safe = std::fs::symlink_metadata(&marker).is_ok_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                m.is_file()
                    && !m.file_type().is_symlink()
                    && m.mode() & 0o077 == 0
                    && m.uid() == unsafe { libc::geteuid() }
            }
            #[cfg(not(unix))]
            {
                m.is_file() && !m.file_type().is_symlink()
            }
        });
        if safe
            && std::fs::read_to_string(marker)
                .is_ok_and(|s| s.lines().any(|l| l.starts_with("crusty-loop\t")))
        {
            return Ok(None);
        }
    }
    let result = autodrive_preflight::inspect(
        PreflightRequest {
            pr,
            repo: ".".into(),
            branch: context.get("existing_branch").cloned().unwrap_or_default(),
            state_dir,
        },
        &cwd,
    )?;
    Ok((result["terminal_refusal"] == true).then_some(result))
}
