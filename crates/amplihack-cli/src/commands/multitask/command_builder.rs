//! Deterministic command and launcher-script construction for multitask workstreams.

use super::models::{Workstream, WorkstreamScope};
use super::process_scope::normalize_path;
use super::utils::rand_u32;
use amplihack_types::workflow;
use anyhow::{Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::ffi::OsString;
use tracing::warn;

/// Valid delegate commands for subprocess execution.
pub(super) const VALID_DELEGATES: &[&str] = &[
    "amplihack claude",
    "amplihack copilot",
    "amplihack amplifier",
];

/// Detect which delegate command to use from the environment.
pub(super) fn detect_delegate() -> String {
    if let Ok(delegate) = std::env::var("AMPLIHACK_DELEGATE") {
        if VALID_DELEGATES.contains(&delegate.as_str()) {
            return delegate;
        }
        warn!("AMPLIHACK_DELEGATE={delegate:?} is not valid. Using default.");
    }
    "amplihack claude".to_string()
}

pub(super) fn populate_workstream_scope(ws: &mut Workstream, repo_url: &str, base_ref: &str) {
    let repository = parse_github_repo_identity(repo_url)
        .map_or_else(|| repository_fallback(repo_url), str::to_string);
    let repo_root = normalize_path(&ws.work_dir);
    let tree_id =
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()));
    let recipe_run_id =
        std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_else(|_| tree_id.clone());
    let issue_id = ws.issue.to_string();
    ws.workstream_scope = WorkstreamScope {
        repository,
        repo_root: repo_root.clone(),
        workdir: repo_root,
        branch: ws.branch.clone(),
        base_ref: base_ref.to_string(),
        issue_id: issue_id.clone(),
        work_item_id: issue_id,
        recipe: ws.recipe.clone(),
        recipe_run_id,
        tree_id,
        workstream_id: format!("ws-{}", ws.issue),
        expected_title_prefix: ws.description.clone(),
        started_at: Utc::now().to_rfc3339(),
    };
}

/// Parse a github.com origin or PR URL into `owner/repo`.
///
/// Same contract as `parse_github_repo_identity` in
/// `amplifier-bundle/tools/workflow_github_repo.sh` (issue #1642): exact,
/// case-sensitive prefixes; the userinfo is the text before the FIRST
/// `@github.com/` and must be non-empty RFC 3986 userinfo characters without
/// `@`; cut `?` then `#`, strip one trailing `/`; the path is exactly
/// `owner/repo` (one `.git` stripped) or `owner/repo/pull/<n>` with `<n>`
/// matching `^[1-9][0-9]{0,9}$` and no `.git` on the repo; each segment is
/// `[A-Za-z0-9._-]+`, not `.` or `..`, and the owner does not start with
/// `-`. Anything else would let git contact another host
/// (`https://evil.example/x@github.com/o/r`) or another repository
/// (`o/r/../../x/y`), so it is `None`.
fn parse_github_repo_identity(url: &str) -> Option<&str> {
    let path = if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else {
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let (userinfo, path) = rest.split_once("@github.com/")?;
        if userinfo.is_empty() || !userinfo.bytes().all(is_userinfo_byte) {
            return None;
        }
        path
    };
    let path = path.split_once('?').map_or(path, |(before, _)| before);
    let path = path.split_once('#').map_or(path, |(before, _)| before);
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, rest) = path.split_once('/')?;
    let repo = match rest.split_once('/') {
        Some((repo, pr)) => {
            let number = pr.strip_prefix("pull/")?;
            if !is_pr_number(number) || repo.ends_with(".git") {
                return None;
            }
            repo
        }
        None => rest.strip_suffix(".git").unwrap_or(rest),
    };
    if owner.starts_with('-') || !is_identity_segment(owner) || !is_identity_segment(repo) {
        return None;
    }
    Some(&path[..owner.len() + 1 + repo.len()])
}

/// `^[1-9][0-9]{0,9}$`
fn is_pr_number(number: &str) -> bool {
    (1..=10).contains(&number.len())
        && !number.starts_with('0')
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// RFC 3986 userinfo characters (unreserved, pct-encoded, sub-delims, ':')
/// without '@'.
fn is_userinfo_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"._~%!$&'()*+,;=:-".contains(&b)
}

fn is_identity_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The scope's repository when the URL is not a github.com repository: a URL
/// loses its userinfo (it may be a token) before it is stored; anything else,
/// such as a local path, is kept as is.
fn repository_fallback(repo_url: &str) -> String {
    if repo_url.contains("://") {
        crate::runner_ancestry::redact_url(repo_url)
    } else {
        repo_url.to_string()
    }
}

/// Build context map for recipe-based resume.
pub(super) fn build_resume_context(ws: &Workstream) -> HashMap<String, serde_json::Value> {
    let mut ctx = HashMap::new();
    ctx.insert(
        "task_description".to_string(),
        serde_json::Value::String(ws.task.clone()),
    );
    ctx.insert(
        "repo_path".to_string(),
        serde_json::Value::String(".".to_string()),
    );
    ctx.insert("issue_number".to_string(), serde_json::json!(ws.issue));
    ctx.insert(
        "workstream_state_file".to_string(),
        serde_json::Value::String(ws.state_file.to_string_lossy().to_string()),
    );
    ctx.insert(
        "workstream_progress_file".to_string(),
        serde_json::Value::String(ws.progress_file.to_string_lossy().to_string()),
    );
    if !ws.resume_checkpoint.is_empty() {
        ctx.insert(
            "resume_checkpoint".to_string(),
            serde_json::Value::String(ws.resume_checkpoint.clone()),
        );
    }

    if !ws.worktree_path.is_empty() {
        ctx.insert(
            "worktree_setup".to_string(),
            serde_json::json!({
                "worktree_path": ws.worktree_path,
                "branch_name": ws.branch,
                "created": false,
            }),
        );
    }
    ctx
}

/// `true` when `recipe` is a recipe name or path that is safe to single-quote
/// into the launcher: non-empty, not option-like, and made only of ASCII
/// letters, digits and `_ . - / + @ :`.
fn is_valid_launcher_recipe(recipe: &str) -> bool {
    !recipe.is_empty()
        && !recipe.starts_with('-')
        && recipe
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-/+@:".contains(&b))
}

/// Issue #1570: the launcher hands the workstream context to
/// `amplihack recipe run` as a file, so no context value ever becomes an
/// argument and no value is split by the shell.
pub(super) fn recipe_launcher_script(recipe: &str) -> Result<String> {
    if !is_valid_launcher_recipe(recipe) {
        bail!(
            "invalid recipe name '{}': only ASCII letters, digits and `_ . - / + @ :` \
             are allowed, and it must not start with '-'",
            recipe.escape_debug()
        );
    }
    Ok(format!(
        r#"#!/bin/bash
# Workstream launcher - Rust recipe runner execution.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
CONTEXT_JSON="$REPO_ROOT/context.json"
RECIPE={recipe}

echo "Starting recipe: $RECIPE"
echo "Work dir: $REPO_ROOT"

exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose
"#,
        recipe = sh_single_quote(recipe),
    ))
}

pub(super) fn recipe_run_script(ws: &Workstream, delegate: &str) -> Result<String> {
    let profile_exports = profile_exports(
        env_var_os,
        crate::commands::profile::substitute_inherited_profile,
    )?;
    let (tree_id, depth, max_depth, max_sessions) = session_tree_env(ws);
    Ok(format!(
        r#"#!/bin/bash
cd '{work_dir}'
export AMPLIHACK_TREE_ID='{tree_id}'
export AMPLIHACK_SESSION_DEPTH='{depth}'
export AMPLIHACK_MAX_DEPTH='{max_depth}'
export AMPLIHACK_MAX_SESSIONS='{max_sessions}'
export AMPLIHACK_DELEGATE='{delegate}'
export AMPLIHACK_WORKSTREAM_ISSUE='{issue}'
export AMPLIHACK_WORKSTREAM_PROGRESS_FILE='{progress_file}'
export AMPLIHACK_WORKSTREAM_STATE_FILE='{state_file}'
export AMPLIHACK_WORKTREE_PATH='{worktree_path}'
{profile_exports}exec bash launcher.sh
"#,
        work_dir = ws.work_dir.display(),
        tree_id = tree_id,
        depth = depth + 1,
        max_depth = max_depth,
        max_sessions = max_sessions,
        delegate = delegate,
        issue = ws.issue,
        progress_file = ws.progress_file.display(),
        state_file = ws.state_file.display(),
        worktree_path = ws.worktree_path,
        profile_exports = profile_exports,
    ))
}

pub(super) fn classic_task_markdown(ws: &Workstream) -> String {
    format!(
        "# Issue #{}\n\n{}\n\nUse the canonical {} autonomously via {} and {}. \
         NO QUESTIONS. Work through all required workflow steps. Create PR when complete.",
        ws.issue,
        ws.task,
        workflow::DEFAULT_WORKFLOW_SELECTION,
        workflow::DEV_ORCHESTRATOR_SKILL,
        workflow::SMART_ORCHESTRATOR_RECIPE_COMMAND
    )
}

pub(super) fn classic_run_script(ws: &Workstream, delegate: &str) -> Result<String> {
    let profile_exports = profile_exports(
        env_var_os,
        crate::commands::profile::substitute_inherited_profile,
    )?;
    let (tree_id, depth, max_depth, max_sessions) = session_tree_env(ws);
    Ok(format!(
        r#"#!/bin/bash
cd '{work_dir}'
export AMPLIHACK_TREE_ID='{tree_id}'
export AMPLIHACK_SESSION_DEPTH='{depth}'
export AMPLIHACK_MAX_DEPTH='{max_depth}'
export AMPLIHACK_MAX_SESSIONS='{max_sessions}'
{profile_exports}{delegate} --subprocess-safe -- -p "@TASK.md Execute task autonomously using the canonical {workflow_selection} via {dev_orchestrator} and {smart_orchestrator}. NO QUESTIONS. Work through all required workflow steps. Create PR when complete."
"#,
        work_dir = ws.work_dir.display(),
        tree_id = tree_id,
        depth = depth + 1,
        max_depth = max_depth,
        max_sessions = max_sessions,
        delegate = delegate,
        workflow_selection = workflow::DEFAULT_WORKFLOW_SELECTION,
        dev_orchestrator = workflow::DEV_ORCHESTRATOR_SKILL,
        smart_orchestrator = workflow::SMART_ORCHESTRATOR_RECIPE_COMMAND,
        profile_exports = profile_exports,
    ))
}

fn env_var_os(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Issue #1567: `export` lines that pass the pinned Claude login profile to
/// the nested `amplihack` launch. tmux sessions do not inherit the client's
/// environment, so the name (and the root, when one was set) must be written
/// into the script. `CLAUDE_CONFIG_DIR` is not exported: the nested launch
/// rebuilds and verifies the dir from the name. Values are not validated
/// here; the nested launch does that and fails closed.
///
/// A value that is set but not valid UTF-8 is an error, not a skipped line:
/// dropping it would make the nested launch fall back to the default root and
/// possibly to a different account. The error names the variable only.
///
/// Issue #1569: `substitute` may replace the inherited profile name with
/// another profile chosen by auto-switch; it is only asked about a set,
/// non-empty profile name.
pub(super) fn profile_exports(
    get: impl Fn(&str) -> Option<OsString>,
    substitute: impl Fn(&str) -> Option<String>,
) -> Result<String> {
    use crate::commands::profile::{PROFILE_ENV, ROOT_ENV};
    let mut out = String::new();
    for key in [PROFILE_ENV, ROOT_ENV] {
        let Some(raw) = get(key).filter(|v| !v.is_empty()) else {
            continue;
        };
        let Some(value) = raw.to_str() else {
            bail!(
                "{key} is set but is not valid UTF-8; refusing to start the \
                 workstream because the nested launch could not keep the parent's \
                 Claude profile. Unset it or set it to a UTF-8 value"
            );
        };
        let value = match key {
            PROFILE_ENV => substitute(value).unwrap_or_else(|| value.to_string()),
            _ => value.to_string(),
        };
        out.push_str(&format!("export {key}={}\n", sh_single_quote(&value)));
    }
    Ok(out)
}

/// Quotes `value` for POSIX sh: `'...'`, with each `'` written as `'\''`.
pub(super) fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn session_tree_env(ws: &Workstream) -> (String, u32, String, String) {
    let depth: u32 = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let tree_id = if ws.workstream_scope.tree_id.is_empty() {
        std::env::var("AMPLIHACK_TREE_ID").unwrap_or_else(|_| format!("{:08x}", rand_u32()))
    } else {
        ws.workstream_scope.tree_id.clone()
    };
    let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string());
    let max_sessions = std::env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string());
    (tree_id, depth, max_depth, max_sessions)
}
