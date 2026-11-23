//! The run report (`autodrive_outcome`, schema version 2) for the goal loop
//! (#3129, D8 and D-9).
//!
//! The report never fails, never panics and makes no network call. It reads
//! two records, only from the state folder the runner bound to this
//! invocation (`auth.state_dir()`), and only through the private evidence
//! store:
//!
//! - `loop-end.json`, which only the loop driver and its end actions write.
//!   Its `end` word alone decides `stop_reason`.
//! - `merge-result.json`, the loop's merge action's own line. It fills only
//!   the `merge` field and never makes a run `completed` by itself.
//!
//! Each must be a regular file, not a link and not shared, of at most
//! `MAX_RECORD_BYTES`, holding one JSON object whose `run_id` is this run's.
//! When the context's `autodrive_state_dir` differs from the bound folder,
//! neither is read. The report creates no folder and never reads a turn
//! record (`turns/NNN/record.md`): those are for people.
//!
//! `completed` comes only from a PR already merged at entry, a `merged` end,
//! or a `ready` end when the captured invocation does not allow merging. The
//! record's own `verify_readiness` claim grants nothing. `loop_result` is
//! always null; the loop lines it came from are retired.
//!
//! `summary` is the `autodrive_finalization` context value, verbatim and cut
//! on a character boundary at `MAX_RECORD_BYTES`. Nothing in an agent's prose
//! decides a field. Notes name what could not be read, never the contents of
//! what was read.
//!
//! docs/reference/autodrive-goal-loop.md#reported-outcomes.
use super::{
    authorization::InvocationAuthorization,
    identity::{Binding, MAX_RECORD_BYTES, validate_binding},
    state::EvidenceStore,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};

/// The record the loop's merge action writes in the runner-bound state folder.
pub(crate) const STATE_MERGE_RECORD: &str = "merge-result.json";

/// The record only the loop driver and its end actions write, in the
/// runner-bound state folder.
pub(crate) const LOOP_END_RECORD: &str = "loop-end.json";

/// One record's JSON object.
type Record = Map<String, Value>;

/// Results the loop's merge action writes. Any other result, including the
/// retired `checks-pending` and `base-behind`, is not echoed.
const MERGE_RESULTS: [&str; 7] = [
    "merged",
    "not-authorised",
    "authorization-unreadable",
    "checks-not-green",
    "refused",
    "not-open",
    "message-unavailable",
];

/// Each `end` word the driver writes and the `stop_reason` it gives. `ready`
/// is not here: the captured invocation decides it. Any other word is
/// `failed`.
const END_REASONS: [(&str, &str); 8] = [
    ("merged", "completed"),
    ("handoff", "handoff"),
    ("closed", "closed"),
    ("stopped", "stopped"),
    ("refused", "terminal_refusal"),
    ("failed", "failed"),
    ("usage_limit", "usage_limit"),
    ("orphaned", "orphaned"),
];

/// The most steps a `merged` end can list for after the merge, and the most
/// bytes they can hold together.
const MAX_AFTER_MERGE_STEPS: usize = 32;
const MAX_AFTER_MERGE_BYTES: usize = 8 * 1024;

/// The longest `end` word the report echoes.
const MAX_END_WORD: usize = 64;

pub(crate) fn report(
    auth: &InvocationAuthorization,
    context: &Map<String, Value>,
    expected_pr: Option<&str>,
) -> Value {
    let mut notes: Vec<String> = Vec::new();
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
    out.insert("loop_result".into(), Value::Null);

    match already_merged(auth, context, expected_pr) {
        Ok(Some(pr)) => {
            out.insert(
                "merge".into(),
                json!({"result": "already-merged", "pr": pr}),
            );
            out.insert("loop_end".into(), Value::Null);
            out.insert("after_merge_steps".into(), json!([]));
            out.insert("stop_reason".into(), json!("completed"));
            return finish(out, summary, truncated, &notes);
        }
        Ok(None) => {}
        Err(_) => notes.push("build_result: already-merged preparation was refused".into()),
    }

    let (loop_end, merge) = read_records(auth, context, expected_pr, &mut notes);
    let end = loop_end.as_ref().and_then(|record| {
        let word = end_word(record);
        if word.is_none() {
            notes.push(format!("{LOOP_END_RECORD}: end is not a word"));
        }
        word
    });
    let merge_result = merge
        .as_ref()
        .and_then(|m| m.get("result"))
        .and_then(Value::as_str);
    // The merge action writes `pr` and `expected_head` with every result.
    let merged_by_action = merge.as_ref().is_some_and(|m| {
        merge_result == Some("merged") && m.contains_key("pr") && m.contains_key("expected_head")
    });

    let stop_reason = match end {
        // The note for a missing or unread record is already in `notes`.
        None => "failed",
        Some("ready") if auth.allows_merge() => {
            notes.push(format!(
                "{LOOP_END_RECORD}: ready while merging is on is a hand-off"
            ));
            "handoff"
        }
        Some("ready") => "completed",
        Some(word) => match END_REASONS.iter().find(|(known, _)| *known == word) {
            Some(&(_, reason)) => reason,
            None => {
                notes.push(format!("{LOOP_END_RECORD}: unknown end word"));
                "failed"
            }
        },
    };
    if end == Some("merged") && !merged_by_action {
        notes.push(format!(
            "{LOOP_END_RECORD}: merged; the merge was not by this run's merge action"
        ));
    }
    if end != Some("merged") && merge_result == Some("merged") {
        notes.push(format!(
            "{STATE_MERGE_RECORD}: merged, but the loop did not end merged"
        ));
    }
    let after_merge_steps = match (end, &loop_end) {
        (Some("merged"), Some(record)) => after_merge_steps(record, &mut notes),
        _ => Vec::new(),
    };

    out.insert("merge".into(), merge.map_or(Value::Null, Value::Object));
    out.insert("loop_end".into(), end.map_or(Value::Null, |w| json!(w)));
    out.insert("after_merge_steps".into(), Value::Array(after_merge_steps));
    out.insert("stop_reason".into(), json!(stop_reason));
    finish(out, summary, truncated, &notes)
}

fn finish(
    mut out: Map<String, Value>,
    summary: String,
    truncated: bool,
    notes: &[String],
) -> Value {
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

/// `loop-end.json` and the reduced `merge-result.json`, each only when it is
/// this run's and readable. Each reason a record is not used goes into
/// `notes`, except a merge record that is simply absent: a run whose agent
/// never chose `merge` has none.
fn read_records(
    auth: &InvocationAuthorization,
    context: &Map<String, Value>,
    expected_pr: Option<&str>,
    notes: &mut Vec<String>,
) -> (Option<Record>, Option<Record>) {
    if !state_dir_matches(auth, context) {
        notes.push("autodrive_state_dir: differs from the runner-assigned state directory".into());
        return (None, None);
    }
    let Some(dir) = auth.state_dir() else {
        notes.push("state folder: no runner-assigned state directory".into());
        return (None, None);
    };
    // The report creates nothing: a folder that is missing, or is a link, is
    // not handed to the store, whose `create` would make it.
    let store = match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => EvidenceStore::create(dir).ok(),
        _ => None,
    };
    let Some(store) = store else {
        notes.push("state folder: missing or refused".into());
        return (None, None);
    };
    let loop_end = match read_record(&store, LOOP_END_RECORD, auth.run_id()) {
        Ok(record) => Some(record),
        Err(why) => {
            notes.push(format!("{LOOP_END_RECORD}: {}", why.describe()));
            None
        }
    };
    let merge = match read_record(&store, STATE_MERGE_RECORD, auth.run_id()) {
        Ok(record) => match merge_field(&record, expected_pr) {
            Ok(merge) => Some(merge),
            Err(why) => {
                notes.push(format!("{STATE_MERGE_RECORD}: {why}"));
                None
            }
        },
        Err(Unread::Missing) => None,
        Err(why) => {
            notes.push(format!("{STATE_MERGE_RECORD}: {}", why.describe()));
            None
        }
    };
    (loop_end, merge)
}

/// Why a record in the state folder was not used.
enum Unread {
    /// There is no such entry.
    Missing,
    /// The store refused it: a link, a shared or special file, one with more
    /// than one name, or one over `MAX_RECORD_BYTES`.
    Refused,
    /// The file is not exactly one JSON object.
    NotAnObject,
    /// Its `run_id` is not this run's, or it has none.
    OtherRun,
}

impl Unread {
    fn describe(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Refused => "refused (a link, a shared or special file, or over the size limit)",
            Self::NotAnObject => "not one JSON object",
            Self::OtherRun => "not this run's",
        }
    }
}

/// One record from the private store: exactly one JSON object, compact or
/// pretty-printed, whose `run_id` is `run_id`.
fn read_record(
    store: &EvidenceStore,
    name: &str,
    run_id: &str,
) -> std::result::Result<Record, Unread> {
    let bytes = store.read_private(name).map_err(|error| {
        let missing = error
            .root_cause()
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        if missing {
            Unread::Missing
        } else {
            Unread::Refused
        }
    })?;
    let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(&bytes) else {
        return Err(Unread::NotAnObject);
    };
    if record.get("run_id").and_then(Value::as_str) != Some(run_id) {
        return Err(Unread::OtherRun);
    }
    Ok(record)
}

/// The `end` word of a loop-end record: lower-case ASCII letters, digits,
/// `_` and `-`, starting with a letter, at most `MAX_END_WORD` bytes. Anything
/// else is not echoed.
fn end_word(record: &Record) -> Option<&str> {
    record.get("end").and_then(Value::as_str).filter(|word| {
        word.len() <= MAX_END_WORD
            && word.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && word
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    })
}

/// The steps a `merged` end lists for after the merge: text values only, in
/// order, at most `MAX_AFTER_MERGE_STEPS` of them and `MAX_AFTER_MERGE_BYTES`
/// together. The list stops at the first step that would pass either cap.
fn after_merge_steps(record: &Record, notes: &mut Vec<String>) -> Vec<Value> {
    let Some(raw) = record.get("after_merge") else {
        return Vec::new();
    };
    let Some(items) = raw.as_array() else {
        notes.push(format!("{LOOP_END_RECORD}: after_merge is not a list"));
        return Vec::new();
    };
    let mut steps = Vec::new();
    let mut bytes = 0;
    let mut skipped = false;
    for item in items {
        let Some(step) = item.as_str() else {
            skipped = true;
            continue;
        };
        if steps.len() == MAX_AFTER_MERGE_STEPS || bytes + step.len() > MAX_AFTER_MERGE_BYTES {
            notes.push(format!(
                "{LOOP_END_RECORD}: after_merge cut to {MAX_AFTER_MERGE_STEPS} steps and {} KiB",
                MAX_AFTER_MERGE_BYTES / 1024
            ));
            break;
        }
        bytes += step.len();
        steps.push(Value::String(step.to_owned()));
    }
    if skipped {
        notes.push(format!(
            "{LOOP_END_RECORD}: after_merge values that are not text were left out"
        ));
    }
    steps
}

/// The merge action's record reduced to its documented fields, or why it is
/// not echoed.
fn merge_field(
    record: &Record,
    expected_pr: Option<&str>,
) -> std::result::Result<Record, &'static str> {
    let Some(result) = record.get("result").and_then(Value::as_str) else {
        return Err("no result");
    };
    let pr = record.get("pr").and_then(Value::as_u64);
    // A record must name the expected PR; one with no readable `pr` is not
    // trusted when a PR is expected.
    if let Some(expected) = expected_pr.filter(|p| !p.is_empty())
        && (pr.is_none() || expected.parse::<u64>().ok() != pr)
    {
        return Err("names another PR, or none");
    }
    if !MERGE_RESULTS.contains(&result) {
        return Err("not a merge result the merge action writes");
    }
    let mut out = Map::new();
    out.insert("result".into(), json!(result));
    if let Some(pr) = pr {
        out.insert("pr".into(), json!(pr));
    }
    if let Some(head) = record
        .get("expected_head")
        .and_then(Value::as_str)
        .filter(|h| {
            [40, 64].contains(&h.len())
                && h.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        out.insert("expected_head".into(), json!(head));
    }
    if let Some(handoff) = record
        .get("handoff")
        .and_then(Value::as_str)
        .filter(|h| matches!(*h, "true" | "false"))
    {
        out.insert("handoff".into(), json!(handoff));
    }
    Ok(out)
}

/// #2329: the run's `autodrive_state_dir` must equal the runner-assigned one
/// by `Path` equality, with no join, canonicalisation or symlink resolution.
/// Absent or empty counts as absent; checked only when the runner bound a
/// directory to this invocation.
///
/// #3053: a folder an agent step reports, such as `build_result.state_dir`, is
/// information only and is not compared. Every record this report reads is
/// opened from `auth.state_dir()`, never from a reported folder.
fn state_dir_matches(auth: &InvocationAuthorization, context: &Map<String, Value>) -> bool {
    let Some(assigned) = auth.state_dir() else {
        return true;
    };
    require_assigned_state_dir(context.get("autodrive_state_dir"), assigned)
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
