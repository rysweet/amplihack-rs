//! Issue #1710 (D3): `<session>.run.json`, the durable record a detached run
//! leaves beside its log.
//!
//! `recipe run --detach` writes it once the session name is claimed, and the
//! pane rewrites it with the outcome when the run ends. Unlike the context
//! file it is never removed, so `workstreams status` can show finished runs
//! and walk the tree of runs that started each other.
//!
//! The record holds no task text, no environment and no context value other
//! than the issue and PR numbers. The origin URL is reduced to its GitHub
//! `owner/name` before it gets here. Every read re-validates every field, so
//! a record edited by hand cannot put arbitrary text in front of a reader.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::run::{FAILURE_CLASS_MARKER_PREFIX, FailureClass, classify_failure_text};
use crate::user_config::safe::read_bounded;

/// The only schema version written and read.
const VERSION: u32 = 1;
/// Bytes of a log read to classify how it ended.
pub(crate) const TAIL_BYTES: u64 = 64 * 1024;
/// Longest session name accepted on read.
const MAX_SESSION: usize = 128;
/// Longest parent identity accepted on read.
const MAX_PARENT: usize = 256;
/// Longest recipe name accepted on read.
const MAX_RECIPE: usize = 256;
/// Mode bits a record may not have.
const LOOSE_RECORD_BITS: u32 = 0o077;

/// Every failure class, so a marker's class is accepted only when it is one
/// of their names.
const FAILURE_CLASSES: [FailureClass; 5] = [
    FailureClass::TransientTransport,
    FailureClass::Environmental,
    FailureClass::Work,
    FailureClass::UsageLimit,
    FailureClass::Indeterminate,
];
const OK: &str = "ok";
const ERROR: &str = "error";

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Outcome {
    pub exit_code: i32,
    /// `ok`, `error` or a failure class name.
    pub class: String,
    /// RFC 3339 UTC.
    pub finished_at: String,
}

/// Schema version 1 of `<session>.run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunRecord {
    pub version: u32,
    pub session: String,
    /// The launching session's identity (`run:`, `session:`, `tree:` or
    /// `claude:` and a value), or `None`.
    pub parent: Option<String>,
    /// RFC 3339 UTC.
    pub started_at: String,
    pub recipe: String,
    pub issue_number: Option<u64>,
    pub pr_number: Option<u64>,
    /// GitHub `owner/name` of the launch directory's origin.
    pub repo: Option<String>,
    pub cwd: PathBuf,
    pub outcome: Option<Outcome>,
}

/// A new record with no outcome. Only the numeric `issue_number` and
/// `pr_number` are taken from `context`.
pub(crate) fn build(
    session: &str,
    parent: Option<String>,
    started_at: DateTime<Utc>,
    recipe: &str,
    context: &Map<String, Value>,
    repo: Option<String>,
    cwd: &Path,
) -> RunRecord {
    RunRecord {
        version: VERSION,
        session: session.to_string(),
        parent,
        started_at: rfc3339(started_at),
        recipe: recipe.to_string(),
        issue_number: number(context.get("issue_number")),
        pr_number: number(context.get("pr_number")),
        repo,
        cwd: cwd.to_path_buf(),
        outcome: None,
    }
}

/// `time` as RFC 3339 UTC with whole seconds and a `Z`.
pub(crate) fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A whole number written only with ASCII digits, that fits a `u64`.
fn number(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.parse().ok()
        }
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// `<log_dir>/<session>.run.json`.
pub(crate) fn record_path(log_dir: &Path, session: &str) -> PathBuf {
    log_dir.join(format!("{session}.run.json"))
}

/// Create `path` with mode 0600 and write `record`. Never opens an existing
/// file or follows a symlink.
pub(crate) fn write_new(path: &Path, record: &RunRecord) -> Result<()> {
    let mut file = create_private(path)?;
    file.write_all(&serde_json::to_vec(record)?)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write the run record {}", path.display()))
}

/// Set the record's outcome: the checked record is read, the new one goes
/// to `<path>.tmp.<pid>` (0600), is synced, and is renamed over it. The
/// temporary file is removed on any failure.
pub(crate) fn write_outcome(path: &Path, outcome: Outcome) -> Result<()> {
    let Some(mut record) = read(path) else {
        bail!(
            "{} is not a private run record owned by you; its outcome was not written",
            path.display()
        );
    };
    record.outcome = Some(outcome);
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp_name);
    let written = create_private(&tmp).and_then(|mut file| {
        file.write_all(&serde_json::to_vec(&record)?)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
            .with_context(|| format!("cannot replace the run record {}", path.display()))
    });
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn create_private(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

/// The record at `path`, when it is a regular file owned by this user with
/// no group or other access, parses, and every field is valid.
pub(crate) fn read(path: &Path) -> Option<RunRecord> {
    if !is_owned_file(path, LOOSE_RECORD_BITS) {
        return None;
    }
    let text = read_bounded(path).ok()??;
    let record: RunRecord = serde_json::from_str(&text).ok()?;
    valid(&record).then_some(record)
}

fn valid(r: &RunRecord) -> bool {
    r.version == VERSION
        && valid_session(&r.session)
        && r.parent
            .as_deref()
            .is_none_or(|p| p.len() <= MAX_PARENT && printable_ascii(p))
        && valid_time(&r.started_at)
        && !r.recipe.is_empty()
        && r.recipe.len() <= MAX_RECIPE
        && !r.recipe.chars().any(char::is_control)
        && r.repo.as_deref().is_none_or(valid_slug)
        && r.cwd.is_absolute()
        && r.outcome
            .as_ref()
            .is_none_or(|o| known_class(&o.class).is_some() && valid_time(&o.finished_at))
}

/// A session name as `detach::session_name` makes them.
pub(crate) fn valid_session(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SESSION
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn printable_ascii(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (b' '..=b'~').contains(&b))
}

fn valid_time(s: &str) -> bool {
    DateTime::parse_from_rfc3339(s).is_ok()
}

/// `owner/name`, each `[A-Za-z0-9._-]+` and neither `.` nor `..`.
fn valid_slug(s: &str) -> bool {
    let segment = |v: &str| {
        !v.is_empty()
            && v != "."
            && v != ".."
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    matches!(s.split_once('/'), Some((owner, name)) if segment(owner) && segment(name))
}

/// The allowlisted name equal to `class`.
fn known_class(class: &str) -> Option<&'static str> {
    [OK, ERROR]
        .into_iter()
        .chain(FAILURE_CLASSES.map(FailureClass::as_str))
        .find(|known| *known == class)
}

/// Whether `path` is a regular file (not a symlink) owned by this user with
/// none of `loose_bits` set.
pub(crate) fn is_owned_file(path: &Path, loose_bits: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && owned(&m, loose_bits))
}

/// Whether `path` is a directory (not a symlink) owned by this user with
/// none of `loose_bits` set.
pub(crate) fn is_owned_dir(path: &Path, loose_bits: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && owned(&m, loose_bits))
}

fn owned(meta: &fs::Metadata, loose_bits: u32) -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    meta.uid() == unsafe { libc::geteuid() } && meta.mode() & loose_bits == 0
}

/// The outcome class of a log tail.
///
/// With an exit code: the class of the last marker line naming a known
/// failure class, whatever the code; otherwise `ok` for 0; otherwise the
/// class the failure text matches; otherwise `error`. Without one (a run
/// with no record), only a marker line gives a class.
pub(crate) fn classify_tail(tail: &str, exit_code: Option<i32>) -> Option<&'static str> {
    if let Some(class) = tail.lines().rev().find_map(marker_class) {
        return Some(class);
    }
    match exit_code? {
        0 => Some(OK),
        _ => match classify_failure_text(tail).0 {
            FailureClass::Indeterminate => Some(ERROR),
            class => Some(class.as_str()),
        },
    }
}

/// The class of a well-formed marker line naming a failure class.
fn marker_class(line: &str) -> Option<&'static str> {
    let payload = line
        .trim_start()
        .strip_prefix(FAILURE_CLASS_MARKER_PREFIX)?;
    let value: Value = serde_json::from_str(payload.trim_end()).ok()?;
    let class = value.get("class")?.as_str()?;
    FAILURE_CLASSES
        .map(FailureClass::as_str)
        .into_iter()
        .find(|known| *known == class)
}

/// At most the last [`TAIL_BYTES`] of the log at `path`, decoded lossily;
/// empty when it is missing, a symlink or not a regular file. Used only to
/// classify the run's end, never printed.
pub(crate) fn read_tail(path: &Path) -> String {
    let Ok(mut file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    else {
        return String::new();
    };
    let Ok(meta) = file.metadata() else {
        return String::new();
    };
    if !meta.is_file() {
        return String::new();
    }
    let start = meta.len().saturating_sub(TAIL_BYTES);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err()
        || file.take(TAIL_BYTES).read_to_end(&mut bytes).is_err()
    {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // A replacement character is longer than the byte it replaces; keep the
    // end within the bound.
    let excess = text.len().saturating_sub(TAIL_BYTES as usize);
    let cut = (excess..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len());
    text[cut..].to_string()
}

/// The GitHub `owner/name` of `origin_url`; never the URL itself.
pub(crate) fn repo_slug(origin_url: &str) -> Option<String> {
    amplihack_workflows::remote_repository::github_repository_from_remote_url(origin_url.trim())
        .filter(|slug| valid_slug(slug))
}

/// The `origin` URL of the checkout at `dir`, read with a bounded, hardened
/// `git` call. `None` when `dir` is not a directory owned by this user, git
/// fails or the limit passes. The URL may hold a credential: callers reduce
/// it to a slug or a state key and never print it.
pub(crate) fn origin_url(dir: &Path, timeout: Duration) -> Option<String> {
    if !is_owned_dir(dir, 0) {
        return None;
    }
    let mut cmd = amplihack_git::command();
    cmd.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.fsmonitor=",
            "-c",
            "core.hooksPath=/dev/null",
            "remote",
            "get-url",
            "origin",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    let out = crate::commands::workstreams::sources::run_bounded(cmd, timeout)?;
    if !out.success {
        return None;
    }
    let url = String::from_utf8(out.stdout).ok()?;
    let url = url.trim();
    (!url.is_empty()).then(|| url.to_string())
}

/// [`repo_slug`] of [`origin_url`].
pub(crate) fn origin_repo(dir: &Path, timeout: Duration) -> Option<String> {
    repo_slug(&origin_url(dir, timeout)?)
}
