//! The run report (`autodrive_outcome`, schema version 2, #2575).
//!
//! The report never fails and never panics: a badly shaped agent output can
//! not fail it. `merge` and `stop_reason` come only from JSON written by the
//! auto-drive tools (`finalize_merge` from the finalize merge step,
//! `merge_loop_result` from the merge loop step, which reads the loop driver's
//! own result file). `summary` is the finalisation agent's statement,
//! verbatim and cut on a character boundary at `MAX_RECORD_BYTES`. Nothing in
//! the agent's prose decides a field. Notes name what could not be read,
//! never the contents of what was read.
//! docs/reference/autodrive-merge-ready-round.md#run-report.
use super::{
    authorization::InvocationAuthorization,
    identity::{Binding, MAX_RECORD_BYTES, validate_binding},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};

/// Merge results the finalize merge step writes. Any other result is reported
/// as a hand-off (never `completed`); a missing or unparseable one is unreadable.
const MERGE_RESULTS: [&str; 9] = [
    "merged",
    "not-authorised",
    "authorization-unreadable",
    "checks-not-green",
    "refused",
    "not-open",
    "message-unavailable",
    "checks-pending",
    "base-behind",
];

pub(crate) fn report(
    auth: &InvocationAuthorization,
    context: &Map<String, Value>,
    expected_pr: Option<&str>,
) -> Value {
    let mut notes: Vec<&'static str> = Vec::new();
    let (summary, truncated) = summary(context.get("autodrive_finalization"));
    let requested_action = if auth.allows_merge() {
        "merge"
    } else {
        "verify_readiness"
    };
    let mut out = Map::new();
    out.insert("schema_version".into(), json!(2));
    out.insert("run_id".into(), json!(auth.run_id()));
    out.insert("requested_action".into(), json!(requested_action));

    match already_merged(auth, context, expected_pr) {
        Ok(Some(pr)) => {
            out.insert(
                "merge".into(),
                json!({"result": "already-merged", "pr": pr}),
            );
            out.insert("loop_result".into(), Value::Null);
            out.insert("stop_reason".into(), json!("completed"));
            return finish(out, summary, truncated, &notes);
        }
        Ok(None) => {}
        Err(_) => notes.push("build_result: already-merged preparation was refused"),
    }

    let state_dir_ok = state_dir_matches(auth, context);
    if !state_dir_ok {
        notes.push("autodrive_state_dir: differs from the runner-assigned state directory");
    }

    let (loop_result, crusty) = match loop_result(context.get("merge_loop_result")) {
        Some(result) => (Some(result), false),
        None => (loop_result(context.get("crusty_loop_result")), true),
    };
    let (merge, unknown_merge) = if state_dir_ok {
        let read = match merge_result(context.get("finalize_merge"), expected_pr) {
            // #2575 B3: a finalisation re-run by recovery runs in another
            // process, so its result never reaches this context. Its merge
            // step wrote the same line, bound to this run, to the state folder.
            MergeRead::Missing => match state_merge_record(auth) {
                Some(record) => {
                    notes.push("finalize_merge: read from the state folder");
                    merge_result(Some(&record), expected_pr)
                }
                None => MergeRead::Missing,
            },
            read => read,
        };
        match read {
            MergeRead::Known(map) => (Some(map), false),
            MergeRead::Unknown => (None, true),
            MergeRead::Missing => (None, false),
        }
    } else {
        (None, false)
    };
    let merge_name = merge
        .as_ref()
        .and_then(|m| m.get("result"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    // #2663: the tool wrote `merged` only after GitHub read the PR MERGED at
    // the expected head, so a merge recorded with its PR and head is
    // `completed` whatever the loop line says. A step that failed after the
    // merge, or a loop result lost with it, does not undo the merge.
    let merged_at_head = merge.as_ref().is_some_and(|m| {
        m.get("result").and_then(Value::as_str) == Some("merged")
            && m.contains_key("pr")
            && m.contains_key("expected_head")
    });
    // #2663 M1: `completed` (a success) means merged. #2575 B2: a WAITING loop
    // goes to finalisation like DONE, and a pending check is a wait.
    let stop_reason = match loop_result.as_deref() {
        _ if merged_at_head => {
            if !matches!(loop_result.as_deref(), Some("DONE" | "WAITING")) || crusty {
                notes.push("finalize_merge: merged; loop_result not consulted");
            }
            "completed"
        }
        Some("TERMINAL_REFUSAL" | "TERMINAL_POLICY_REFUSAL") => "terminal_refusal",
        Some("DONE" | "WAITING") if crusty => {
            notes.push("merge_loop_result: missing or unreadable");
            "unreadable"
        }
        Some("DONE" | "WAITING") => match merge_name.as_deref() {
            Some("merged") => "completed",
            // #2575 B4: a verify-only run (no_merge or dry run) that reached
            // DONE did what it was asked; it is `completed`, as before
            // 8794c2df2. A merge run that was refused stays `not_merged`.
            Some("not-authorised") if requested_action == "verify_readiness" => "completed",
            Some("not-authorised" | "base-behind") => "not_merged",
            Some("checks-pending") => "waiting_for_checks",
            Some(_) => "handoff",
            None if unknown_merge => {
                notes.push("finalize_merge: unknown merge result");
                "handoff"
            }
            None => {
                notes.push("finalize_merge: missing or unreadable");
                "unreadable"
            }
        },
        // A crusty STUCK keeps the stop reason it had before #2575.
        Some("STUCK") if crusty => "no_progress",
        Some("HANDOFF" | "STUCK") => "handoff",
        Some("BUDGET_EXHAUSTED") => "budget_exhausted",
        Some("USAGE_LIMIT") => "usage_limit",
        Some("ORPHANED") => "orphaned",
        _ => {
            if loop_result.is_none() {
                notes.push("merge_loop_result: missing or unreadable");
            }
            "unreadable"
        }
    };
    out.insert("merge".into(), merge.map_or(Value::Null, Value::Object));
    out.insert(
        "loop_result".into(),
        loop_result.map_or(Value::Null, Value::String),
    );
    out.insert("stop_reason".into(), json!(stop_reason));
    finish(out, summary, truncated, &notes)
}

fn finish(mut out: Map<String, Value>, summary: String, truncated: bool, notes: &[&str]) -> Value {
    out.insert("summary".into(), Value::String(summary));
    if truncated {
        out.insert("summary_truncated".into(), json!(true));
    }
    if !notes.is_empty() {
        out.insert("note".into(), json!(notes.join("; ")));
    }
    Value::Object(out)
}

/// The finalisation statement verbatim, cut on a character boundary.
fn summary(raw: Option<&Value>) -> (String, bool) {
    let text = match raw {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    if text.len() <= MAX_RECORD_BYTES {
        return (text, false);
    }
    let mut end = MAX_RECORD_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

/// One JSON object from a tool-written context value: the value itself when
/// the runner already parsed it, else the last line that parses as an object.
fn tool_object(raw: Option<&Value>) -> Option<Map<String, Value>> {
    match raw? {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) if text.len() <= MAX_RECORD_BYTES => {
            text.lines()
                .rev()
                .find_map(|line| match serde_json::from_str::<Value>(line.trim()) {
                    Ok(Value::Object(map)) => Some(map),
                    _ => None,
                })
        }
        _ => None,
    }
}

/// `loop_result` from a loop line: an upper-case token, or nothing.
fn loop_result(raw: Option<&Value>) -> Option<String> {
    let map = tool_object(raw)?;
    let token = map.get("loop_result")?.as_str()?;
    (!token.is_empty()
        && token.len() <= 64
        && token.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'))
    .then(|| token.to_owned())
}

/// The file the finalize merge step writes its result to, in the runner-bound
/// state folder (#2575 B3).
pub(crate) const STATE_MERGE_RECORD: &str = "finalize-merge.json";

/// The merge step's result from the runner-bound state folder, only when it
/// names this run: a record another run left in a reused folder is ignored.
/// Read through the private evidence store, so a link, a shared file or an
/// oversized one is never read.
fn state_merge_record(auth: &InvocationAuthorization) -> Option<Value> {
    let store = super::state::EvidenceStore::create(auth.state_dir()?).ok()?;
    let bytes = store.read_private(STATE_MERGE_RECORD).ok()?;
    let record = tool_object(Some(&Value::String(String::from_utf8(bytes).ok()?)))?;
    (record.get("run_id").and_then(Value::as_str) == Some(auth.run_id()))
        .then_some(Value::Object(record))
}

/// What the finalize merge step's JSON said.
enum MergeRead {
    /// A documented result, reduced to its documented fields.
    Known(Map<String, Value>),
    /// A string `result` that is not a documented one: never `completed`.
    Unknown,
    /// No readable object, no string `result`, another PR's result, or no
    /// `pr` when one is expected.
    Missing,
}

/// The finalize merge step's JSON, reduced to its documented fields.
fn merge_result(raw: Option<&Value>, expected_pr: Option<&str>) -> MergeRead {
    let Some(map) = tool_object(raw) else {
        return MergeRead::Missing;
    };
    let Some(result) = map.get("result").and_then(Value::as_str) else {
        return MergeRead::Missing;
    };
    let pr = map.get("pr").and_then(Value::as_u64);
    // A record must name the expected PR; one with no readable `pr` is not
    // trusted when a PR is expected.
    if let Some(expected) = expected_pr.filter(|p| !p.is_empty())
        && (pr.is_none() || expected.parse::<u64>().ok() != pr)
    {
        return MergeRead::Missing;
    }
    if !MERGE_RESULTS.contains(&result) {
        return MergeRead::Unknown;
    }
    let mut out = Map::new();
    out.insert("result".into(), json!(result));
    if let Some(pr) = pr {
        out.insert("pr".into(), json!(pr));
    }
    if let Some(head) = map
        .get("expected_head")
        .and_then(Value::as_str)
        .filter(|h| [40, 64].contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        out.insert("expected_head".into(), json!(head));
    }
    if let Some(handoff) = map
        .get("handoff")
        .and_then(Value::as_str)
        .filter(|h| matches!(*h, "true" | "false"))
    {
        out.insert("handoff".into(), json!(handoff));
    }
    MergeRead::Known(out)
}

/// #2329: a state directory the child reports must equal the runner-assigned
/// one by `Path` equality, with no join, canonicalisation or symlink
/// resolution. Absent or empty counts as absent; checked only when the runner
/// bound a directory to this invocation.
fn state_dir_matches(auth: &InvocationAuthorization, context: &Map<String, Value>) -> bool {
    let Some(assigned) = auth.state_dir() else {
        return true;
    };
    let preparation = tool_object(context.get("build_result"));
    [
        context.get("autodrive_state_dir"),
        preparation.as_ref().and_then(|p| p.get("state_dir")),
    ]
    .into_iter()
    .all(|reported| require_assigned_state_dir(reported, assigned))
}

fn require_assigned_state_dir(reported: Option<&Value>, assigned: &std::path::Path) -> bool {
    match reported {
        None => true,
        Some(Value::String(text)) => text.is_empty() || std::path::Path::new(text) == assigned,
        Some(_) => false,
    }
}

/// A PR already merged at entry, confirmed by the build preflight and bound to
/// this invocation. Returns the PR number.
fn already_merged(
    auth: &InvocationAuthorization,
    context: &Map<String, Value>,
    expected_pr: Option<&str>,
) -> Result<Option<u64>> {
    let Some(raw) = context.get("build_result") else {
        return Ok(None);
    };
    let value: Value = match raw {
        Value::String(s) if s.is_empty() => return Ok(None),
        Value::String(s) => {
            ensure!(
                s.len() <= MAX_RECORD_BYTES,
                "run-entry preparation exceeds size limit"
            );
            serde_json::from_str(s).context("unreadable run-entry preparation")?
        }
        value => value.clone(),
    };
    if value["merged_before_run"] != true {
        return Ok(None);
    }
    ensure!(
        value["pr_state"] == "MERGED"
            && value["entry_evidence"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()),
        "already-merged preparation requires concrete platform confirmation"
    );
    let binding: Binding = serde_json::from_value(value["binding"].clone())
        .context("already-merged PR identity missing")?;
    validate_binding(&binding)?;
    auth.validate_selection(&binding)?;
    ensure!(
        binding.run_id == auth.run_id()
            && auth
                .policy_provenance()
                .is_none_or(|p| p == binding.policy_provenance),
        "already-merged preparation belongs to another invocation or policy"
    );
    if let Some(pr) = expected_pr.filter(|p| !p.is_empty()) {
        ensure!(
            pr.parse::<u64>()? == binding.pr_number,
            "already-merged preparation changed the explicitly selected PR"
        );
    }
    Ok(Some(binding.pr_number))
}
