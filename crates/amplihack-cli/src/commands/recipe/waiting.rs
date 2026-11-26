//! Issue #3364: `amplihack recipe waiting [--json]`, the runs on this host
//! that wait for their owner, and the runs that ended with open decisions.
//!
//! Two records, both written into a run's own folder (`AMPLIHACK_RUN_DIR`):
//!
//! - `waiting.json`, written by the goal loop's turn tool
//!   (`autodrive_goal_turn.sh wait-owner`) before it starts
//!   `recipe guidance wait`, and removed when that wait returns. It holds
//!   `run_id`, `target`, `owner`, `question`, `turn` and `started_at`. It is
//!   listed only for a live run, found from the session pointers as
//!   `recipe guide <PR URL>` finds live runs ([`scan_runs`]), so a record left
//!   behind by a run that has ended is never listed.
//! - `open-decisions.json`, written by the review workflow's agent step when
//!   disagreements or design questions stay open. It holds `run_id`,
//!   `target` (the pull request), `questions`, `where` and `created_at`. It
//!   is read from the run folders the session pointers name and from the run
//!   folders directly under the work root's `runs/` folder (a nested run has
//!   no pointer of its own). It is listed until a guidance entry newer than
//!   `created_at` is in the run's guidance folder or the pull request's, or
//!   until `gh pr view` reports the pull request merged or closed.
//!
//! Nothing here writes, waits or retries. A record that exists but cannot be
//! read is skipped with a warning on stderr that names its path. Exit 0
//! whenever the listing ran.

use super::guide::{RootChecks, accept_recorded};
use super::live_runs::{FoundRun, read_json_capped, scan_runs};
use crate::commands::cleanup::work_root;
use crate::run_guidance::fs_safety::{DirState, dir_state};
use crate::run_guidance::pr_key::{parse_pr_url, pr_guidance_dir, pr_key, state_root};
use crate::run_guidance::sanitize::sanitize_line;
use crate::run_guidance::{GUIDANCE_DIR, is_entry_name};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The record a run waiting for its owner keeps in its folder.
pub(crate) const WAITING_FILE: &str = "waiting.json";
/// The record a run that ended with open decisions keeps in its folder.
pub(crate) const OPEN_DECISIONS_FILE: &str = "open-decisions.json";
/// The largest record read.
const MAX_RECORD_BYTES: u64 = 256 * 1024;

/// `amplihack recipe waiting`.
pub fn run_waiting(json_out: bool) -> Result<()> {
    let now = Utc::now();
    let found = scan_runs("recipe waiting", |record, recorded, roots| {
        Some(accept_recorded(recorded, record.run_id.as_deref(), roots).map(|(dir, _)| dir))
    });
    let mut items = waiting_items(&found, now);
    items.extend(open_decision_items(&found, now, &mut GhPrState::default()));
    if json_out {
        println!("{}", serde_json::to_string(&items)?);
    } else {
        for item in &items {
            println!("{}", text_line(item));
        }
    }
    Ok(())
}

/// A string field of `record`, or `""`.
fn field<'a>(record: &'a Value, key: &str) -> &'a str {
    record.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The run id a record names, or the run folder's name when it names none.
fn run_id_of(record: &Value, run_dir: &Path) -> String {
    match field(record, "run_id") {
        "" => run_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        id => id.to_string(),
    }
}

/// Seconds from `at` (RFC 3339) to `now`, or `None` when `at` does not parse.
fn age_seconds(at: &str, now: DateTime<Utc>) -> Option<i64> {
    let at = DateTime::parse_from_rfc3339(at).ok()?;
    Some((now - at.with_timezone(&Utc)).num_seconds().max(0))
}

/// The record named `file` in `run_dir`. `None`, silently, when there is
/// none; `None`, with a warning naming the path, when it cannot be read.
fn read_record(run_dir: &Path, file: &str) -> Option<Value> {
    let path = run_dir.join(file);
    if std::fs::symlink_metadata(&path).is_err() {
        return None;
    }
    let record = read_json_capped(&path, MAX_RECORD_BYTES).filter(Value::is_object);
    if record.is_none() {
        eprintln!(
            "recipe waiting: warning: skipped {}: not a readable JSON object of this user",
            sanitize_line(&path.display().to_string())
        );
    }
    record
}

/// One entry for each live run that holds a waiting record.
fn waiting_items(found: &[FoundRun], now: DateTime<Utc>) -> Vec<Value> {
    found
        .iter()
        .filter(|run| run.live)
        .filter_map(|run| run.accepted.as_ref().ok())
        .filter_map(|dir| {
            let record = read_record(dir, WAITING_FILE)?;
            let started = field(&record, "started_at");
            Some(json!({
                "kind": "waiting",
                "run_id": run_id_of(&record, dir),
                "run_dir": dir.display().to_string(),
                "target": field(&record, "target"),
                "owner": field(&record, "owner"),
                "question": field(&record, "question"),
                "turn": field(&record, "turn"),
                "started_at": started,
                "age_seconds": age_seconds(started, now),
            }))
        })
        .collect()
}

/// Every run folder that may hold an open-decisions record: those the
/// session pointers name, then those directly under the work root's `runs/`
/// folder, each once and only when it passes the folder checks.
fn candidate_run_dirs(found: &[FoundRun]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = found
        .iter()
        .filter_map(|run| run.accepted.clone().ok())
        .collect();
    if let Ok(root) = work_root::resolve_from_env() {
        let runs = root.runs_dir();
        if matches!(dir_state(&runs), Ok(DirState::Trusted))
            && let Ok(listing) = std::fs::read_dir(&runs)
        {
            let mut roots = RootChecks::default();
            for entry in listing.flatten() {
                let path = entry.path();
                if let Some(text) = path.to_str()
                    && let Ok((dir, _)) = accept_recorded(text, None, &mut roots)
                {
                    dirs.push(dir);
                }
            }
        }
    }
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.canonicalize().unwrap_or_else(|_| d.clone())));
    dirs
}

/// The state of each pull request, read once per listing with `gh pr view`.
#[derive(Default)]
struct GhPrState(HashMap<String, Option<String>>);

impl GhPrState {
    /// `OPEN`, `MERGED` or `CLOSED`, or `None` when `gh` could not say.
    fn of(&mut self, url: &str) -> Option<String> {
        if let Some(known) = self.0.get(url) {
            return known.clone();
        }
        let state = std::process::Command::new("gh")
            .args(["pr", "view", url, "--json", "state"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| serde_json::from_slice::<Value>(&out.stdout).ok())
            .and_then(|v| v.get("state")?.as_str().map(str::to_uppercase));
        if state.is_none() {
            eprintln!(
                "recipe waiting: warning: could not read the state of {} with gh; it stays listed",
                sanitize_line(url)
            );
        }
        self.0.insert(url.to_string(), state.clone());
        state
    }
}

/// Whether a guidance entry newer than `created` is in `dirs`. Entry names
/// begin with their UTC time, `YYYYMMDDTHHMMSS.nnnnnnnnnZ`, so they compare
/// as text with `created` in the same form.
fn answered_after(created: DateTime<Utc>, dirs: &[PathBuf]) -> bool {
    let key = created.format("%Y%m%dT%H%M%S%.9fZ").to_string();
    dirs.iter()
        .filter(|d| matches!(dir_state(d), Ok(DirState::Trusted)))
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|listing| listing.flatten())
        .filter_map(|e| e.file_name().into_string().ok())
        .any(|name| is_entry_name(&name) && name[..key.len()] > *key)
}

/// One entry for each run whose open-decisions record is not yet answered
/// and whose pull request is not merged or closed.
fn open_decision_items(found: &[FoundRun], now: DateTime<Utc>, gh: &mut GhPrState) -> Vec<Value> {
    let mut items = Vec::new();
    for dir in candidate_run_dirs(found) {
        let Some(record) = read_record(&dir, OPEN_DECISIONS_FILE) else {
            continue;
        };
        let target = field(&record, "target");
        let created = field(&record, "created_at");
        let pr = parse_pr_url(target).ok();
        let mut guidance = vec![dir.join(GUIDANCE_DIR)];
        if let (Some(pr), Ok(root)) = (&pr, state_root()) {
            guidance.push(pr_guidance_dir(
                &root,
                &pr_key(&pr.owner, &pr.repo, pr.number),
            ));
        }
        if let Ok(at) = DateTime::parse_from_rfc3339(created)
            && answered_after(at.with_timezone(&Utc), &guidance)
        {
            continue;
        }
        let pr_state = pr
            .as_ref()
            .map(|_| gh.of(target).unwrap_or_else(|| "unknown".to_string()));
        if matches!(pr_state.as_deref(), Some("MERGED" | "CLOSED")) {
            continue;
        }
        let strings = |key: &str| -> Vec<String> {
            record
                .get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        items.push(json!({
            "kind": "open-decisions",
            "run_id": run_id_of(&record, &dir),
            "run_dir": dir.display().to_string(),
            "target": target,
            "questions": strings("questions"),
            "where": strings("where"),
            "created_at": created,
            "age_seconds": age_seconds(created, now),
            "pr_state": pr_state,
        }));
    }
    items
}

/// `2h13m`, `5m`, `3d4h`, or `-` when the age is not known.
fn compact_age(seconds: Option<i64>) -> String {
    let Some(s) = seconds else {
        return "-".to_string();
    };
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3_600, (s % 3_600) / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h{m}m"),
        _ => format!("{d}d{h}h"),
    }
}

/// One tab-separated line: kind, age, owner, question, run id. Every field
/// is sanitised and holds no tab; an empty one is `-`.
fn text_line(item: &Value) -> String {
    let cell = |s: &str| {
        let s = sanitize_line(s).replace('\t', " ");
        if s.trim().is_empty() {
            "-".to_string()
        } else {
            s
        }
    };
    let question = if item["kind"] == "open-decisions" {
        let questions = item["questions"].as_array().cloned().unwrap_or_default();
        match questions.first().and_then(Value::as_str) {
            Some(first) => format!("{first} ({} open)", questions.len()),
            None => String::new(),
        }
    } else {
        field(item, "question").to_string()
    };
    [
        cell(field(item, "kind")),
        compact_age(item["age_seconds"].as_i64()),
        cell(field(item, "owner")),
        cell(&question),
        cell(field(item, "run_id")),
    ]
    .join("\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_are_compact_and_unknown_is_a_dash() {
        assert_eq!(compact_age(None), "-");
        assert_eq!(compact_age(Some(59)), "0m");
        assert_eq!(compact_age(Some(7_980)), "2h13m");
        assert_eq!(compact_age(Some(3 * 86_400 + 4 * 3_600)), "3d4h");
    }

    #[test]
    fn an_entry_newer_than_the_record_answers_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("guidance");
        std::fs::create_dir(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let created: DateTime<Utc> = "2026-11-26T10:00:00Z".parse().unwrap();
        std::fs::write(dir.join("20261126T095959.000000000Z-1-0.md"), "x").unwrap();
        assert!(!answered_after(created, std::slice::from_ref(&dir)));
        std::fs::write(dir.join("20261126T100001.000000000Z-1-0.md"), "x").unwrap();
        assert!(answered_after(created, &[dir]));
    }

    #[test]
    fn a_text_line_has_five_tab_separated_fields() {
        let item = json!({"kind": "waiting", "age_seconds": 60, "owner": "o\tx",
            "question": "q", "run_id": "r"});
        assert_eq!(text_line(&item).split('\t').count(), 5);
        let open = json!({"kind": "open-decisions", "questions": ["a", "b"], "run_id": "r"});
        let line = text_line(&open);
        assert_eq!(line.split('\t').nth(1), Some("-"));
        assert_eq!(line.split('\t').nth(2), Some("-"));
    }
}
