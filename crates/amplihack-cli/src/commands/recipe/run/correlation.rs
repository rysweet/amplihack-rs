use super::super::{RecipeLogPointerSummary, RecipeRunResult};
use crate::util::run_output_with_timeout;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

pub(super) const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";

const SCHEMA_VERSION: u8 = 1;
const MAX_POINTER_VALUE_BYTES: usize = 1024;
/// Issue #2783: the longest `run_dir` a pointer line carries. A path is
/// recorded whole or left out: a truncated path would name another folder,
/// and `recipe guide` resolves a run by this value, so [`bounded_value`],
/// which truncates at [`MAX_POINTER_VALUE_BYTES`], is never used for it.
const MAX_POINTER_PATH_BYTES: usize = 4096;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(super) struct RecipeRunCorrelation {
    run_id: String,
    recipe_name: String,
    cwd: String,
    worktree: Option<String>,
    /// Issue #2783: the absolute path of this run's folder
    /// (`<work root>/runs/<run folder>`), so `amplihack recipe guide` finds
    /// the run from what it recorded at start, whatever the caller's
    /// environment. Absent for a run that has no folder yet.
    run_dir: Option<String>,
    branch: Option<String>,
    task_description: Option<String>,
    issue_number: Option<String>,
    issue_url: Option<String>,
    pr_number: Option<String>,
    pr_url: Option<String>,
    work_item_id: Option<String>,
    work_item_url: Option<String>,
    runner_path: String,
    /// Issue #1606: what this run resolved, recorded so a run can be audited
    /// for the agent CLI and home it actually used.
    settings: RunSettings,
}

/// The resolved values the log pointer records (issue #1606).
#[derive(Debug, Clone, Default)]
pub(super) struct RunSettings {
    /// The agent CLI every step runs under.
    pub(super) agent_binary: Option<String>,
    /// The resolver layer that chose it (`ResolutionSource::label`).
    pub(super) agent_binary_source: Option<&'static str>,
    /// The `AMPLIHACK_HOME` handed to the runner.
    pub(super) amplihack_home: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RecipeRunFinalStatus {
    Success,
    Failure,
    SpawnFailure,
    ParseFailure,
    /// Issue #1976: the parent `recipe run` exited and this nested run
    /// stopped itself.
    Orphaned,
}

/// Issue #1976: the reason recorded for an orphan stop.
pub(super) const ORPHAN_REASON: &str = "parent recipe run exited";

/// Issue #1976: what an orphaned final record adds.
#[derive(Debug, Clone, Copy)]
pub(super) struct OrphanFields<'a> {
    pub(super) parent_pid: i32,
    pub(super) tree_id: &'a str,
}

impl RecipeRunFinalStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::SpawnFailure => "spawn_failure",
            Self::ParseFailure => "parse_failure",
            Self::Orphaned => "orphaned",
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
            run_dir: None,
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
            settings: RunSettings::default(),
        }
    }

    /// Record the run's resolved settings in every pointer (issue #1606).
    pub(super) fn with_settings(mut self, settings: RunSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Record the run's own folder in every pointer (issue #2783). A path
    /// over [`MAX_POINTER_PATH_BYTES`] bytes, or one that is not valid
    /// UTF-8, is left out rather than shortened.
    pub(super) fn with_run_location(mut self, dir: Option<&Path>) -> Self {
        self.run_dir = dir
            .and_then(Path::to_str)
            .filter(|value| !value.is_empty() && value.len() <= MAX_POINTER_PATH_BYTES)
            .map(str::to_string);
        self
    }

    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(super) fn cwd(&self) -> &str {
        &self.cwd
    }

    pub(super) fn emit_early(&self) {
        self.emit(self.early_event())
    }

    /// The line [`Self::emit_early`] writes, without its newline. Factored
    /// out so the record can be read without capturing stderr.
    #[cfg(test)]
    fn early_pointer_line(&self) -> String {
        event_line(&self.early_event()).expect("the early pointer event serializes")
    }

    fn early_event(&self) -> LogPointerEvent<'_> {
        LogPointerEvent {
            schema_version: SCHEMA_VERSION,
            event: "early",
            run_id: &self.run_id,
            recipe_name: &self.recipe_name,
            cwd: &self.cwd,
            worktree: self.worktree.as_deref(),
            run_dir: self.run_dir.as_deref(),
            branch: self.branch.as_deref(),
            task_description: self.task_description.as_deref(),
            issue_number: self.issue_number.as_deref(),
            issue_url: self.issue_url.as_deref(),
            pr_number: self.pr_number.as_deref(),
            pr_url: self.pr_url.as_deref(),
            work_item_id: self.work_item_id.as_deref(),
            work_item_url: self.work_item_url.as_deref(),
            runner_path: &self.runner_path,
            agent_binary: self.settings.agent_binary.as_deref(),
            agent_binary_source: self.settings.agent_binary_source,
            amplihack_home: self.settings.amplihack_home.as_deref(),
            child_pid: None,
            exit_code: None,
            status: None,
            log_paths: None,
            parent_pid: None,
            tree_id: None,
            reason: None,
            timestamp: timestamp(),
        }
    }

    pub(super) fn emit_final(
        &self,
        status: RecipeRunFinalStatus,
        child_pid: Option<u32>,
        exit_code: Option<i32>,
        log_paths: JsonMap<String, JsonValue>,
    ) -> RecipeLogPointerSummary {
        self.emit_final_with(status, child_pid, exit_code, log_paths, None)
    }

    /// Issue #1976: the final record of an orphaned nested run. It has no
    /// `exit_code`: the runner child was torn down, and the 75 is the exit
    /// status of this `recipe run` process, not of the child.
    pub(super) fn emit_orphaned(
        &self,
        child_pid: Option<u32>,
        orphan: OrphanFields<'_>,
    ) -> RecipeLogPointerSummary {
        self.emit_final_with(
            RecipeRunFinalStatus::Orphaned,
            child_pid,
            None,
            known_log_paths(None),
            Some(orphan),
        )
    }

    fn emit_final_with(
        &self,
        status: RecipeRunFinalStatus,
        child_pid: Option<u32>,
        exit_code: Option<i32>,
        log_paths: JsonMap<String, JsonValue>,
        orphan: Option<OrphanFields<'_>>,
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
            run_dir: self.run_dir.as_deref(),
            branch: self.branch.as_deref(),
            task_description: self.task_description.as_deref(),
            issue_number: self.issue_number.as_deref(),
            issue_url: self.issue_url.as_deref(),
            pr_number: self.pr_number.as_deref(),
            pr_url: self.pr_url.as_deref(),
            work_item_id: self.work_item_id.as_deref(),
            work_item_url: self.work_item_url.as_deref(),
            runner_path: &self.runner_path,
            agent_binary: self.settings.agent_binary.as_deref(),
            agent_binary_source: self.settings.agent_binary_source,
            amplihack_home: self.settings.amplihack_home.as_deref(),
            child_pid,
            exit_code,
            status: Some(status.as_str()),
            log_paths: log_paths_ref,
            parent_pid: orphan.map(|o| o.parent_pid),
            tree_id: orphan.map(|o| o.tree_id),
            reason: orphan.map(|_| ORPHAN_REASON),
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
        // `event_line` reports a failure to serialize; nothing is written.
        if let Some(line) = event_line(&event) {
            let _ = writeln!(io::stderr(), "{line}");
        }
    }
}

/// One `amplihack.recipe.log_pointer` line, without its newline. `None` when
/// the event cannot be serialized, which it reports on the tracing log.
fn event_line(event: &LogPointerEvent<'_>) -> Option<String> {
    match serde_json::to_string(event) {
        Ok(payload) => Some(format!("{LOG_POINTER_PREFIX}{payload}")),
        Err(error) => {
            tracing::warn!(%error, "failed to serialize recipe log pointer");
            None
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
    /// Issue #2783: `<work root>/runs/<run folder>` of this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    run_dir: Option<&'a str>,
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
    agent_binary: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_binary_source: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    amplihack_home: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_paths: Option<&'a JsonMap<String, JsonValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_pid: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tree_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
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
    let mut command = amplihack_git::command();
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

#[cfg(test)]
mod tests_issue_1976 {
    use super::RecipeRunFinalStatus;

    #[test]
    fn orphaned_final_status_is_spelled_orphaned() {
        assert_eq!(RecipeRunFinalStatus::Orphaned.as_str(), "orphaned");
    }

    #[test]
    fn existing_final_statuses_are_unchanged() {
        assert_eq!(RecipeRunFinalStatus::Success.as_str(), "success");
        assert_eq!(RecipeRunFinalStatus::Failure.as_str(), "failure");
        assert_eq!(RecipeRunFinalStatus::SpawnFailure.as_str(), "spawn_failure");
        assert_eq!(RecipeRunFinalStatus::ParseFailure.as_str(), "parse_failure");
    }
}

#[cfg(test)]
mod tests_issue_2783 {
    use super::*;
    use std::path::PathBuf;

    fn correlation() -> RecipeRunCorrelation {
        RecipeRunCorrelation::new(
            "r".to_string(),
            Path::new("/"),
            &BTreeMap::new(),
            Path::new("/bin/runner"),
        )
    }

    fn pointer_json(c: &RecipeRunCorrelation) -> JsonValue {
        // Contract: `emit_early` builds its line with `early_pointer_line`
        // (the `LOG_POINTER_PREFIX` line without the newline), which the
        // implementation factors out of `emit` so it can be read here.
        let line = c.early_pointer_line();
        serde_json::from_str(line.trim().trim_start_matches(LOG_POINTER_PREFIX)).unwrap()
    }

    /// The pointer line is a format other programs parse -- `recipe guide`
    /// reads it back through `guide_records` -- so the field name and its
    /// value are the contract, not the constant. Asserting the constant
    /// against its own literal could not fail; this fails if the field is
    /// renamed, dropped or emitted as something other than the number 1.
    #[test]
    fn the_pointer_line_declares_schema_version_1() {
        let v = pointer_json(&correlation());
        assert_eq!(v["schema_version"], JsonValue::from(1));
        assert_eq!(v["event"], "early");
    }

    #[test]
    fn run_dir_is_recorded_in_full_after_worktree() {
        let dir = PathBuf::from("/w/runs/3f1b7c2a-5d4e-4a90-8b61-7e2c9d0a4f18");
        let c = correlation().with_run_location(Some(&dir));
        let v = pointer_json(&c);
        assert_eq!(v["run_dir"], dir.display().to_string());
    }

    #[test]
    fn no_run_location_omits_the_field() {
        let v = pointer_json(&correlation().with_run_location(None));
        assert!(v.get("run_dir").is_none());
    }

    #[test]
    fn an_oversize_run_dir_is_omitted_never_truncated() {
        let long = PathBuf::from(format!("/w/{}/runs/x", "a".repeat(MAX_POINTER_PATH_BYTES)));
        let v = pointer_json(&correlation().with_run_location(Some(&long)));
        assert!(v.get("run_dir").is_none(), "{v}");
    }

    #[test]
    fn a_run_dir_longer_than_the_value_bound_but_within_the_path_bound_is_whole() {
        let long = PathBuf::from(format!(
            "/w/{}/runs/x",
            "a".repeat(MAX_POINTER_VALUE_BYTES + 100)
        ));
        let v = pointer_json(&correlation().with_run_location(Some(&long)));
        assert_eq!(v["run_dir"], long.display().to_string());
    }
}
