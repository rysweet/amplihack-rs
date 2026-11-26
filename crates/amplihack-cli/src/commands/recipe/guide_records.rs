//! Issue #2783: the records a recipe run leaves at start, read back.
//!
//! A run writes an `amplihack.recipe.log_pointer` line on stderr as each
//! attempt starts (see [`super::run`]). For a detached run that line lands
//! in `<recipe log dir>/<session>.log`, which is resolved from `HOME` or
//! `recipe.log_dir` and so does not depend on the work root. That directory
//! is therefore the index `recipe guide` searches when the caller's
//! environment says nothing about where the run put its folder.
//!
//! On the ordinary path the line is written after the runner has been
//! probed and spawned, not before: what it precedes is any output of that
//! runner, whose stderr is piped and relayed only afterwards. The one thing
//! that can come ahead of it in a log is the wrapper's own output, a warning
//! line or two at most. One record is read per log, so a run that retried is
//! indexed by its first attempt's correlation id alone; the folder that
//! record names is shared by every attempt, and `guide.rs` reads the ids of
//! the later ones out of the `run.json` there.
//!
//! Two readers, one parser:
//!
//! - [`read_named`] reads the log of a session the operator named. Its
//!   failures are the operator's to see, so they are errors.
//! - [`scan`] reads every log in the directory. Nothing it meets is an
//!   error: a log that cannot be opened, is not a regular file, holds no
//!   pointer line or holds one that will not parse is counted and skipped,
//!   and the count reaches the caller in [`Scan::skipped`]. The directory
//!   itself is treated the same way: one that is missing, cannot be listed
//!   or fails the trust rule yields an empty scan carrying the reason, and
//!   none of those is called a refusal, because the rules that need no
//!   index still get their turn.
//!
//! A record is a claim, not a fact. The folder it names is checked by
//! `guide.rs` before anything is written to it. Discovery has no log-count,
//! byte-offset, line-length or field-length cutoff; logs are read one line
//! at a time and the search stops at the first pointer line.

use crate::run_guidance::GuideError;
use crate::run_guidance::fs_safety::{DirState, dir_state, untrusted_reason};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::SystemTime;

pub(super) const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";

/// The fields of a pointer line that name a run. Every one is optional: a
/// record written before a field existed, or by a run that had no folder,
/// simply does not carry it, which is never an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Pointer {
    /// The correlation id (`AMPLIHACK_RECIPE_RUN_ID`) of the attempt that
    /// wrote the line, which for a log read here is always the first: a run
    /// that retried has further ids, and they are in `run.json` under
    /// [`Self::run_dir`] rather than here.
    pub(super) run_id: Option<String>,
    /// `<work root>/runs/<run folder>`, exactly as recorded. Never turned
    /// into a path here: `guide.rs` checks the raw string first.
    pub(super) run_dir: Option<String>,
    pub(super) issue_number: Option<String>,
    pub(super) issue_url: Option<String>,
    /// Issue #3161: the pull request in the run's context, as recorded.
    /// Untrusted text: `live_runs` validates it before matching.
    pub(super) pr_url: Option<String>,
    pub(super) pr_number: Option<String>,
    /// The run's working folder and git top level, which a number-only
    /// pull request is resolved against.
    pub(super) cwd: Option<String>,
    pub(super) worktree: Option<String>,
    /// The stem of the log the record was read from: the detached session
    /// name. Set by [`scan`] and [`read_named`], not by the pointer line.
    pub(super) session: Option<String>,
}

/// What a scan of the log directory found.
#[derive(Debug, Clone, Default)]
pub(super) struct Scan {
    /// The records read, newest log first.
    pub(super) records: Vec<Pointer>,
    /// Logs that were listed but yielded no record, with the reason folded
    /// into a count: unreadable, untrusted, not a regular file, no pointer
    /// line, or an unparseable one.
    pub(super) skipped: usize,
    /// Directory failure or explanations for skipped logs, including parser limits.
    pub(super) note: Option<String>,
}

impl Scan {
    /// The directory was not read at all. `note` says why in terms of the
    /// directory, which it does not name: [`Self::summary`] goes inside a
    /// note that names it already.
    fn unread(note: String) -> Self {
        Self {
            note: Some(note),
            ..Self::default()
        }
    }

    /// How the scan is described in the not-found message.
    pub(super) fn summary(&self) -> String {
        if self.records.is_empty()
            && self.skipped == 0
            && let Some(note) = &self.note
        {
            return note.clone();
        }
        let read = self.records.len();
        let mut summary = format!("{read} pointer record(s)");
        if self.skipped > 0 {
            summary.push_str(&format!(", {} log(s) skipped", self.skipped));
        }
        if let Some(note) = &self.note {
            summary.push_str(&format!(": {note}"));
        }
        summary
    }
}

/// One nonempty string field. Unknown fields and fields of another JSON
/// type do not prevent the other fields from being recovered.
fn field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Parse a pointer line. Parse errors retain serde_json's explanation,
/// including its recursion limit, rather than claiming no line was found.
fn parse_line(line: &[u8]) -> Result<Pointer, String> {
    let body = line
        .strip_prefix(LOG_POINTER_PREFIX.as_bytes())
        .ok_or_else(|| "not a pointer line".to_string())?;
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    Ok(Pointer {
        run_id: field(&value, "run_id"),
        run_dir: field(&value, "run_dir"),
        issue_number: field(&value, "issue_number"),
        issue_url: field(&value, "issue_url"),
        pr_url: field(&value, "pr_url"),
        pr_number: field(&value, "pr_number"),
        cwd: field(&value, "cwd"),
        worktree: field(&value, "worktree"),
        session: None,
    })
}

/// `record` with the session its log is named after.
fn with_session(mut record: Pointer, log: &Path) -> Pointer {
    record.session = log
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_string);
    record
}

/// The first pointer line of `log`, read through one handle that is opened
/// without following a link, cannot block on a FIFO, and is checked by its
/// own metadata rather than by a separate look at the path.
///
/// Reading stops at the first pointer line or EOF. Only the current line
/// is kept, so a long prefix of unrelated output does not accumulate.
///
/// Reasons name the log and the fault and stop there, with no word for what
/// happens next: the same failure ends the command when the operator named
/// the log and only adds to a skipped-log count when the sweep found it.
/// [`read_named`] is what turns an `Unsafe` reason into a refusal; see the
/// two-word rule in the `guide` module documentation (issue #2809).
fn read_pointer_line(log: &Path) -> Result<Option<Vec<u8>>, GuideError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(log)
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                GuideError::Unsafe(format!("{}: it is a symbolic link", log.display()))
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
        return Err(GuideError::Unsafe(format!("{}: {reason}", log.display())));
    }
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = match reader.read_until(b'\n', &mut line) {
            Ok(read) => read,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(GuideError::Io(format!(
                    "cannot read {}: {e}",
                    log.display()
                )));
            }
        };
        if read == 0 {
            return Ok(None);
        }
        if line.starts_with(LOG_POINTER_PREFIX.as_bytes()) {
            return Ok(Some(line));
        }
    }
}

/// The record in the log of a session the operator named. `None` when the
/// log does not exist.
///
/// # Errors
///
/// [`GuideError::Unsafe`] when the log is a symbolic link or fails the trust
/// rule, [`GuideError::NotFound`] when it holds no usable pointer line, and
/// [`GuideError::Io`] for any other read failure. These are the operator's
/// own input, so they are reported rather than passed over: the trust
/// failure is the one place a log is *refused*, and the word is added here
/// rather than in [`read_pointer_line`], which the sweep also calls and
/// whose reasons there only count a skipped log (issue #2809).
pub(super) fn read_named(log: &Path) -> Result<Option<Pointer>, GuideError> {
    let found = match read_pointer_line(log) {
        Ok(found) => found,
        Err(GuideError::NotFound(_)) => return Ok(None),
        Err(GuideError::Unsafe(why)) => {
            return Err(GuideError::Unsafe(format!("refusing {why}")));
        }
        Err(e) => return Err(e),
    };
    let missing = || {
        GuideError::NotFound(format!(
            "{} has no amplihack.recipe.log_pointer line",
            log.display()
        ))
    };
    let line = found.ok_or_else(missing)?;
    parse_line(&line)
        .map(|record| Some(with_session(record, log)))
        .map_err(|reason| {
            GuideError::NotFound(format!(
                "{} has an unusable amplihack.recipe.log_pointer line: {reason}",
                log.display()
            ))
        })
}

/// One log worth opening: a regular file named `*.log`, with the time it
/// was last written so the newest is read first.
///
/// The failure reason is about `log_dir` and does not name it: its one
/// caller is [`scan`], whose note names the directory already.
fn listed_logs(log_dir: &Path) -> Result<Vec<(SystemTime, std::path::PathBuf)>, String> {
    let rd = fs::read_dir(log_dir).map_err(|e| format!("it cannot be listed: {e}"))?;
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
///
/// A directory that fails the trust rule is passed over, not refused: the
/// search goes on to the rules that do not need the index, and the command
/// exits only if they miss too. So the reason carries no verdict word, as
/// the per-log reasons do not, and `guide.rs` prints it inside the note
/// that already names the directory (the two-word rule, issue #2809).
pub(super) fn scan(log_dir: &Path) -> Scan {
    match dir_state(log_dir) {
        Ok(DirState::Trusted) => {}
        Ok(DirState::Missing) => return Scan::unread("it does not exist".to_string()),
        Ok(DirState::Untrusted(reason)) => return Scan::unread(reason),
        Err(e) => return Scan::unread(e.to_string()),
    }
    let logs = match listed_logs(log_dir) {
        Ok(logs) => logs,
        Err(reason) => return Scan::unread(reason),
    };
    let mut skipped = 0;
    let mut reasons = BTreeMap::<String, usize>::new();
    let mut records = Vec::with_capacity(logs.len());
    for (_, log) in logs {
        let record = match read_pointer_line(&log) {
            Ok(Some(line)) => parse_line(&line).map(|record| with_session(record, &log)),
            Ok(None) => Err("no pointer line".to_string()),
            Err(e) => Err(e.to_string()),
        };
        match record {
            Ok(record) => records.push(record),
            Err(reason) => {
                skipped += 1;
                *reasons.entry(reason).or_default() += 1;
            }
        }
    }
    let note = (!reasons.is_empty()).then(|| {
        reasons
            .into_iter()
            .map(|(reason, count)| format!("{count}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ")
    });
    Scan {
        records,
        skipped,
        note,
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
    fn long_fields_are_retained_and_wrong_types_or_empty_fields_are_absent() {
        let long = "a".repeat(5000);
        let p = parse_line(&line(&format!(
            r#"{{"run_id":["r1"],"run_dir":"{long}","issue_number":""}}"#
        )))
        .unwrap();
        assert_eq!(p.run_dir.as_deref(), Some(long.as_str()));
        assert_eq!(p.run_id, None);
        assert_eq!(p.issue_number, None);
    }

    #[test]
    fn long_lines_parse_and_unparseable_lines_report_the_cause() {
        let long = "x".repeat(200_000);
        let p = parse_line(&line(&format!(r#"{{"run_dir":"{long}"}}"#))).unwrap();
        assert_eq!(p.run_dir.as_deref(), Some(long.as_str()));
        assert!(parse_line(&line("{not json")).is_err());
        let nested = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
        assert!(
            parse_line(&line(&nested))
                .unwrap_err()
                .contains("recursion limit")
        );
        assert!(parse_line(b"some other output").is_err());
    }

    /// A retried run writes one pointer per attempt.
    /// what a scan indexes for a retried run is one record, the first
    /// attempt's id with the folder every attempt of it uses.
    #[test]
    fn a_retried_run_is_indexed_by_its_first_attempt_and_the_shared_folder() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("s.log");
        std::fs::write(
            &log,
            format!(
                "starting run\n\
                 {LOG_POINTER_PREFIX}{{\"run_id\":\"attempt-1\",\"run_dir\":\"/w/runs/r\"}}\n\
                 transient failure; retrying\n\
                 {LOG_POINTER_PREFIX}{{\"run_id\":\"attempt-2\",\"run_dir\":\"/w/runs/r\"}}\n"
            ),
        )
        .unwrap();
        let record = read_named(&log).unwrap().unwrap();
        assert_eq!(record.run_id.as_deref(), Some("attempt-1"));
        assert_eq!(record.run_dir.as_deref(), Some("/w/runs/r"));
    }

    /// A log whose pointer line falls past the first read: the reader must
    /// ask for more and still return the line whole.
    #[test]
    fn a_pointer_line_beyond_the_first_read_is_found_whole() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("s.log");
        let mut body = "filler\n".repeat(8192 / 7 + 1);
        assert!(body.len() > 8192);
        body.push_str(&format!(
            "{LOG_POINTER_PREFIX}{{\"run_id\":\"late\",\"run_dir\":\"/w/runs/late\"}}\nmore\n"
        ));
        std::fs::write(&log, &body).unwrap();
        let line = read_pointer_line(&log).unwrap().unwrap();
        let p = parse_line(&line).unwrap();
        assert_eq!(p.run_id.as_deref(), Some("late"));
        assert_eq!(p.run_dir.as_deref(), Some("/w/runs/late"));
    }

    /// The last line of a log a run is still writing has no newline yet.
    #[test]
    fn a_pointer_line_the_log_has_not_yet_closed_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("s.log");
        std::fs::write(
            &log,
            format!("starting\n{LOG_POINTER_PREFIX}{{\"run_id\":\"open\"}}"),
        )
        .unwrap();
        let line = read_pointer_line(&log).unwrap().unwrap();
        assert_eq!(parse_line(&line).unwrap().run_id.as_deref(), Some("open"));
    }

    /// A pointer beyond the old 64 KiB cutoff is still found.
    #[test]
    fn a_pointer_beyond_64_kib_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("s.log");
        let mut body = "filler\n".repeat(64 * 1024 / 7 + 64);
        body.push_str(&format!("{LOG_POINTER_PREFIX}{{\"run_id\":\"deep\"}}\n"));
        std::fs::write(&log, &body).unwrap();
        assert_eq!(
            read_named(&log).unwrap().unwrap().run_id.as_deref(),
            Some("deep")
        );
    }
    #[test]
    fn more_than_ten_thousand_logs_are_searched() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..10_001 {
            let log = dir.path().join(format!("{n:05}.log"));
            fs::write(
                &log,
                format!("{LOG_POINTER_PREFIX}{{\"run_id\":\"r{n}\"}}\n"),
            )
            .unwrap();
            if n == 0 {
                fs::File::open(&log)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
                    .unwrap();
            }
        }
        let found = scan(dir.path());
        assert_eq!(found.records.len(), 10_001);
        assert_eq!(found.skipped, 0);
        assert!(
            found
                .records
                .iter()
                .any(|p| p.run_id.as_deref() == Some("r0"))
        );
    }

    #[test]
    fn parser_limits_are_explained_in_named_and_scan_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("s.log");
        let nested = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
        fs::write(&log, line(&nested)).unwrap();
        let error = read_named(&log).unwrap_err().to_string();
        assert!(error.contains("recursion limit"), "{error}");
        assert!(!error.contains("has no"), "{error}");
        let summary = scan(dir.path()).summary();
        assert!(summary.contains("1 log(s) skipped"), "{summary}");
        assert!(summary.contains("recursion limit"), "{summary}");
    }
}
