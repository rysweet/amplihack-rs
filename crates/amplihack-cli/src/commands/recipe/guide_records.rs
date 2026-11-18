//! Issue #2783: the records a recipe run leaves at start, read back.
//!
//! A run writes one `amplihack.recipe.log_pointer` line on stderr before it
//! spawns anything (see [`super::run`]). For a detached run that line lands
//! in `<recipe log dir>/<session>.log`, which is resolved from `HOME` or
//! `recipe.log_dir` and so does not depend on the work root. That directory
//! is therefore the index `recipe guide` searches when the caller's
//! environment says nothing about where the run put its folder.
//!
//! Two readers, one parser:
//!
//! - [`read_named`] reads the log of a session the operator named. Its
//!   failures are the operator's to see, so they are errors.
//! - [`scan`] reads every log in the directory. Nothing it meets is an
//!   error: a log that cannot be opened, is not a regular file, holds no
//!   pointer line or holds one that will not parse is counted and skipped,
//!   and the count reaches the caller in [`Scan::skipped`].
//!
//! A record is a claim, not a fact. Every string it yields is bounded here,
//! and the folder it names is checked by `guide.rs` before anything is
//! written to it.

use crate::commands::recipe::run_id::MAX_RUN_ID;
use crate::run_guidance::GuideError;
use crate::run_guidance::fs_safety::{check_dir, untrusted_reason};
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::SystemTime;

pub(super) const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";

/// How much of a log is read while looking for its pointer line.
const MAX_LOG_HEAD: u64 = 64 * 1024;
/// The longest pointer line accepted. A line holds every recorded value,
/// including a `task_description` and a `run_dir` of up to 4 KiB each.
const MAX_POINTER_LINE: usize = 32 * 1024;
/// Per-field bounds, so one field of a long line cannot hold the whole line
/// for every record scanned.
const MAX_RECORDED_PATH: usize = 4096;
const MAX_RECORDED_NUMBER: usize = 32;
const MAX_RECORDED_URL: usize = 512;
/// The most logs examined in one scan, newest first. The rest are counted
/// as skipped and named in the not-found message.
const MAX_SCANNED_LOGS: usize = 10_000;

/// The fields of a pointer line that name a run. Every one is optional: a
/// record written before a field existed, or by a run that had no folder,
/// simply does not carry it, which is never an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Pointer {
    /// The run's correlation id (`AMPLIHACK_RECIPE_RUN_ID`).
    pub(super) run_id: Option<String>,
    /// `<work root>/runs/<run folder>`, exactly as recorded. Never turned
    /// into a path here: `guide.rs` checks the raw string first.
    pub(super) run_dir: Option<String>,
    pub(super) issue_number: Option<String>,
    pub(super) issue_url: Option<String>,
}

/// What a scan of the log directory found.
#[derive(Debug, Clone, Default)]
pub(super) struct Scan {
    /// The records read, newest log first.
    pub(super) records: Vec<Pointer>,
    /// Logs that were listed but yielded no record, with the reason folded
    /// into a count: unreadable, untrusted, not a regular file, no pointer
    /// line, an unparseable or oversize one, or past [`MAX_SCANNED_LOGS`].
    pub(super) skipped: usize,
    /// Why the directory itself yielded nothing, when it did.
    pub(super) note: Option<String>,
}

impl Scan {
    /// The directory was not read at all.
    fn unread(note: String) -> Self {
        Self {
            note: Some(note),
            ..Self::default()
        }
    }

    /// How the scan is described in the not-found message.
    pub(super) fn summary(&self) -> String {
        match &self.note {
            Some(note) => note.clone(),
            None => {
                let read = self.records.len();
                let mut summary = format!("{read} pointer record(s)");
                if self.skipped > 0 {
                    summary.push_str(&format!(", {} log(s) skipped", self.skipped));
                }
                summary
            }
        }
    }
}

/// One bounded string field, or `None` when it is absent, of the wrong JSON
/// type, empty or longer than `max`. A wrong type never fails the read: the
/// pointer schema may gain a field of another shape, and a record is never
/// allowed to decide that a command stops.
fn field(value: &serde_json::Value, key: &str, max: usize) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= max)
        .map(str::to_string)
}

/// The record a pointer line carries, or `None` when the line is over
/// [`MAX_POINTER_LINE`] or does not parse. serde_json's default recursion
/// limit is what stops a deeply nested line; it is deliberately left in
/// place.
fn parse_line(line: &[u8]) -> Option<Pointer> {
    if line.len() > MAX_POINTER_LINE {
        return None;
    }
    let body = line.strip_prefix(LOG_POINTER_PREFIX.as_bytes())?;
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    Some(Pointer {
        run_id: field(&value, "run_id", MAX_RUN_ID),
        run_dir: field(&value, "run_dir", MAX_RECORDED_PATH),
        issue_number: field(&value, "issue_number", MAX_RECORDED_NUMBER),
        issue_url: field(&value, "issue_url", MAX_RECORDED_URL),
    })
}

/// The first pointer line in `head`.
fn first_pointer_line(head: &[u8]) -> Option<&[u8]> {
    head.split(|b| *b == b'\n')
        .find(|l| l.starts_with(LOG_POINTER_PREFIX.as_bytes()))
}

/// The head of `log`, read through one handle that is opened without
/// following a link, cannot block on a FIFO, and is checked by its own
/// metadata rather than by a separate look at the path.
fn read_head(log: &Path) -> Result<Vec<u8>, GuideError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(log)
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                GuideError::Unsafe(format!("refusing {}: it is a symbolic link", log.display()))
            } else if e.kind() == ErrorKind::NotFound {
                GuideError::NotFound(format!("{} does not exist", log.display()))
            } else {
                GuideError::Io(format!("cannot open {}: {e}", log.display()))
            }
        })?;
    let meta = file
        .metadata()
        .map_err(|e| GuideError::Io(format!("cannot check {}: {e}", log.display())))?;
    if let Some(reason) = untrusted_reason(&meta, false) {
        return Err(GuideError::Unsafe(format!(
            "refusing {}: {reason}",
            log.display()
        )));
    }
    let mut head = Vec::new();
    file.take(MAX_LOG_HEAD)
        .read_to_end(&mut head)
        .map_err(|e| GuideError::Io(format!("cannot read {}: {e}", log.display())))?;
    Ok(head)
}

/// The record in the log of a session the operator named. `None` when the
/// log does not exist.
///
/// # Errors
///
/// [`GuideError::Unsafe`] when the log is a symbolic link or fails the trust
/// rule, [`GuideError::NotFound`] when it holds no usable pointer line, and
/// [`GuideError::Io`] for any other read failure. These are the operator's
/// own input, so they are reported rather than passed over.
pub(super) fn read_named(log: &Path) -> Result<Option<Pointer>, GuideError> {
    let head = match read_head(log) {
        Ok(head) => head,
        Err(GuideError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let missing = || {
        GuideError::NotFound(format!(
            "{} has no amplihack.recipe.log_pointer line",
            log.display()
        ))
    };
    let line = first_pointer_line(&head).ok_or_else(missing)?;
    parse_line(line).map(Some).ok_or_else(missing)
}

/// One log worth opening: a regular file named `*.log`, with the time it
/// was last written so the newest is read first.
fn listed_logs(log_dir: &Path) -> Result<Vec<(SystemTime, std::path::PathBuf)>, GuideError> {
    let rd = fs::read_dir(log_dir)
        .map_err(|e| GuideError::Io(format!("cannot list {}: {e}", log_dir.display())))?;
    let mut logs = Vec::new();
    for item in rd.flatten() {
        if !item
            .file_name()
            .to_str()
            .is_some_and(|n| n.ends_with(".log"))
        {
            continue;
        }
        // `file_type` and `metadata` on a `DirEntry` are an `lstat` relative
        // to the open directory, so a symbolic link, a FIFO or a device
        // named `*.log` is left here and never opened.
        let Ok(kind) = item.file_type() else { continue };
        if !kind.is_file() {
            continue;
        }
        let modified = item
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        logs.push((modified, item.path()));
    }
    // Newest first; the path breaks a tie so the order is the same twice.
    logs.sort_by(|a, b| b.cmp(a));
    Ok(logs)
}

/// Every record in `log_dir`, newest log first. Never fails: a directory
/// that cannot be read or does not pass the trust rule yields an empty scan
/// carrying the reason, which `guide.rs` prints when nothing matched.
pub(super) fn scan(log_dir: &Path) -> Scan {
    match check_dir(log_dir) {
        Ok(true) => {}
        Ok(false) => return Scan::unread(format!("{} does not exist", log_dir.display())),
        Err(e) => return Scan::unread(e.to_string()),
    }
    let mut logs = match listed_logs(log_dir) {
        Ok(logs) => logs,
        Err(e) => return Scan::unread(e.to_string()),
    };
    let mut skipped = logs.len().saturating_sub(MAX_SCANNED_LOGS);
    logs.truncate(MAX_SCANNED_LOGS);
    let mut records = Vec::with_capacity(logs.len());
    for (_, log) in logs {
        match read_head(&log)
            .ok()
            .as_deref()
            .and_then(first_pointer_line)
            .and_then(parse_line)
        {
            Some(record) => records.push(record),
            None => skipped += 1,
        }
    }
    Scan {
        records,
        skipped,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(body: &str) -> Vec<u8> {
        format!("{LOG_POINTER_PREFIX}{body}").into_bytes()
    }

    #[test]
    fn a_record_yields_the_four_fields_it_names() {
        let p = parse_line(&line(
            r#"{"run_id":"r1","run_dir":"/w/runs/r1","issue_number":"2783","issue_url":"https://x/1"}"#,
        ))
        .unwrap();
        assert_eq!(p.run_id.as_deref(), Some("r1"));
        assert_eq!(p.run_dir.as_deref(), Some("/w/runs/r1"));
        assert_eq!(p.issue_number.as_deref(), Some("2783"));
        assert_eq!(p.issue_url.as_deref(), Some("https://x/1"));
    }

    #[test]
    fn a_field_of_the_wrong_type_or_over_its_bound_reads_as_absent() {
        let long = "a".repeat(MAX_RECORDED_PATH + 1);
        let p = parse_line(&line(&format!(
            r#"{{"run_id":["r1"],"run_dir":"{long}","issue_number":""}}"#
        )))
        .unwrap();
        assert_eq!(p, Pointer::default());
    }

    #[test]
    fn an_oversize_or_unparseable_line_yields_no_record() {
        assert!(parse_line(&line(&"x".repeat(MAX_POINTER_LINE))).is_none());
        assert!(parse_line(&line("{not json")).is_none());
        let nested = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
        assert!(parse_line(&line(&nested)).is_none());
        assert!(parse_line(b"some other output").is_none());
    }

    #[test]
    fn the_first_pointer_line_of_a_head_is_the_one_used() {
        let head = b"starting\namplihack.recipe.log_pointer {\"run_id\":\"a\"}\namplihack.recipe.log_pointer {\"run_id\":\"b\"}\n";
        let p = parse_line(first_pointer_line(head).unwrap()).unwrap();
        assert_eq!(p.run_id.as_deref(), Some("a"));
    }
}
