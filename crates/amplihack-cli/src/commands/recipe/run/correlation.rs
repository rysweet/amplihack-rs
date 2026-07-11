use super::super::{RecipeLogPointerSummary, RecipeRunResult};
use crate::util::run_output_with_timeout;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use uuid::Uuid;

pub(super) const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";

const SCHEMA_VERSION: u8 = 1;
const MAX_POINTER_VALUE_BYTES: usize = 1024;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(super) struct RecipeRunCorrelation {
    run_id: String,
    recipe_name: String,
    cwd: String,
    worktree: Option<String>,
    branch: Option<String>,
    task_description: Option<String>,
    issue_number: Option<String>,
    issue_url: Option<String>,
    pr_number: Option<String>,
    pr_url: Option<String>,
    work_item_id: Option<String>,
    work_item_url: Option<String>,
    runner_path: String,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RecipeRunFinalStatus {
    Success,
    Failure,
    SpawnFailure,
    ParseFailure,
}

impl RecipeRunFinalStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::SpawnFailure => "spawn_failure",
            Self::ParseFailure => "parse_failure",
        }
    }
}

impl RecipeRunCorrelation {
    pub(super) fn new(
        recipe_name: String,
        working_dir: &Path,
        context: &BTreeMap<String, String>,
        runner_path: &Path,
    ) -> Self {
        let cwd = path_to_string(working_dir);
        let worktree = git_output(working_dir, &["rev-parse", "--show-toplevel"])
            .filter(|value| !value.is_empty())
            .or_else(|| Some(cwd.clone()));
        let branch = git_output(working_dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .filter(|value| !value.is_empty());

        Self {
            run_id: Uuid::new_v4().to_string(),
            recipe_name,
            cwd,
            worktree,
            branch,
            task_description: first_context_value(context, &["task_description"]),
            issue_number: first_context_value(
                context,
                &["issue_number", "issue", "issue_id", "github_issue_number"],
            ),
            issue_url: first_context_value(context, &["issue_url", "github_issue_url"]),
            pr_number: first_context_value(
                context,
                &["pr_number", "pull_request_number", "github_pr_number"],
            ),
            pr_url: first_context_value(context, &["pr_url", "pull_request_url", "github_pr_url"]),
            work_item_id: first_context_value(
                context,
                &["work_item_id", "work_item", "ado_work_item_id"],
            ),
            work_item_url: first_context_value(context, &["work_item_url", "ado_work_item_url"]),
            runner_path: path_to_string(runner_path),
        }
    }

    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(super) fn cwd(&self) -> &str {
        &self.cwd
    }

    pub(super) fn emit_early(&self) {
        self.emit(LogPointerEvent {
            schema_version: SCHEMA_VERSION,
            event: "early",
            run_id: &self.run_id,
            recipe_name: &self.recipe_name,
            cwd: &self.cwd,
            worktree: self.worktree.as_deref(),
            branch: self.branch.as_deref(),
            task_description: self.task_description.as_deref(),
            issue_number: self.issue_number.as_deref(),
            issue_url: self.issue_url.as_deref(),
            pr_number: self.pr_number.as_deref(),
            pr_url: self.pr_url.as_deref(),
            work_item_id: self.work_item_id.as_deref(),
            work_item_url: self.work_item_url.as_deref(),
            runner_path: &self.runner_path,
            child_pid: None,
            exit_code: None,
            status: None,
            log_paths: None,
            timestamp: timestamp(),
        })
    }

    pub(super) fn emit_final(
        &self,
        status: RecipeRunFinalStatus,
        child_pid: Option<u32>,
        exit_code: Option<i32>,
        log_paths: JsonMap<String, JsonValue>,
    ) -> RecipeLogPointerSummary {
        let log_paths_ref = if log_paths.is_empty() {
            None
        } else {
            Some(&log_paths)
        };
        self.emit(LogPointerEvent {
            schema_version: SCHEMA_VERSION,
            event: "final",
            run_id: &self.run_id,
            recipe_name: &self.recipe_name,
            cwd: &self.cwd,
            worktree: self.worktree.as_deref(),
            branch: self.branch.as_deref(),
            task_description: self.task_description.as_deref(),
            issue_number: self.issue_number.as_deref(),
            issue_url: self.issue_url.as_deref(),
            pr_number: self.pr_number.as_deref(),
            pr_url: self.pr_url.as_deref(),
            work_item_id: self.work_item_id.as_deref(),
            work_item_url: self.work_item_url.as_deref(),
            runner_path: &self.runner_path,
            child_pid,
            exit_code,
            status: Some(status.as_str()),
            log_paths: log_paths_ref,
            timestamp: timestamp(),
        });

        self.summary(status, child_pid, exit_code, log_paths)
    }

    fn summary(
        &self,
        status: RecipeRunFinalStatus,
        child_pid: Option<u32>,
        exit_code: Option<i32>,
        log_paths: JsonMap<String, JsonValue>,
    ) -> RecipeLogPointerSummary {
        RecipeLogPointerSummary {
            run_id: self.run_id.clone(),
            recipe_name: self.recipe_name.clone(),
            status: status.as_str().to_string(),
            worktree: self.worktree.clone(),
            branch: self.branch.clone(),
            child_pid,
            exit_code,
            runner_path: Some(self.runner_path.clone()),
            log_paths,
        }
    }

    fn emit(&self, event: LogPointerEvent<'_>) {
        match serde_json::to_string(&event) {
            Ok(payload) => {
                let _ = writeln!(io::stderr(), "{LOG_POINTER_PREFIX}{payload}");
            }
            Err(error) => {
                tracing::warn!(%error, "failed to serialize recipe log pointer");
            }
        }
    }
}

pub(super) fn known_log_paths(result: Option<&RecipeRunResult>) -> JsonMap<String, JsonValue> {
    let mut log_paths = JsonMap::new();
    if let Ok(path) = std::env::var("AMPLIHACK_RECIPE_LOG_JSONL")
        && !path.trim().is_empty()
    {
        log_paths.insert("jsonl".to_string(), JsonValue::String(path));
    }

    if let Some(result) = result {
        if let Some(path) = result
            .progress_summary
            .as_ref()
            .and_then(JsonValue::as_object)
            .and_then(|value| value.get("log_path"))
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            log_paths
                .entry("jsonl".to_string())
                .or_insert_with(|| JsonValue::String(path.to_string()));
        }

        if let Some(path) = result
            .extra
            .get("log_path")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            log_paths
                .entry("jsonl".to_string())
                .or_insert_with(|| JsonValue::String(path.to_string()));
        }

        if let Some(extra_paths) = result.extra.get("log_paths").and_then(JsonValue::as_object) {
            for (key, value) in extra_paths {
                if !key.trim().is_empty() {
                    log_paths.insert(key.clone(), value.clone());
                }
            }
        }
    }

    log_paths
}

#[derive(Serialize)]
struct LogPointerEvent<'a> {
    schema_version: u8,
    event: &'a str,
    run_id: &'a str,
    recipe_name: &'a str,
    cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_number: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pr_number: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pr_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_item_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_item_url: Option<&'a str>,
    runner_path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_paths: Option<&'a JsonMap<String, JsonValue>>,
    timestamp: String,
}

fn first_context_value(context: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| context.get(*key))
        .and_then(|value| bounded_value(value))
}

fn bounded_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_utf8_bytes(trimmed, MAX_POINTER_VALUE_BYTES))
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    let suffix = "...";
    let target = max_bytes.saturating_sub(suffix.len());
    let mut truncated = String::new();
    for ch in value.chars() {
        if truncated.len() + ch.len_utf8() > target {
            break;
        }
        truncated.push(ch);
    }
    truncated.push_str(suffix);
    truncated
}

fn path_to_string(path: &Path) -> String {
    path.display().to_string()
}

fn git_output(working_dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(working_dir).args(args);
    let output = run_output_with_timeout(command, GIT_COMMAND_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}
