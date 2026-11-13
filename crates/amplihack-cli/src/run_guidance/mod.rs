//! Operator guidance for recipe runs (issue #2535, `amplihack recipe guide`).
//!
//! An operator appends an instruction to a running (or relaunched) recipe
//! run; every later agent step receives all entries, in order, as an
//! "Operator instructions for this run" block after its own prompt.
//!
//! Storage: one 0600 file per entry in a 0700 `guidance/` folder, either in
//! the run folder (`<work root>/runs/<id>/guidance/`) or in the per-PR state
//! folder (`<state root>/auto-drive/<key>/guidance/`, see [`pr_key`]). Beside
//! each `guidance/` folder, `guidance.log` (0600 JSONL, append-only) records
//! who appended what size when, and each launch that injected the block. No
//! record ever holds the guidance text.
//!
//! Delivery: `recipe run` lists the folders in `AMPLIHACK_RUN_GUIDANCE_DIRS`
//! ([`env`]); the launcher reads them when an agent step starts
//! ([`GuidanceForLaunch`]). There is no timer, polling or watcher: an entry
//! reaches the next agent step that starts after it was written.
//!
//! Known limit: the [`MAX_GUIDANCE_DIR_BYTES`] check in [`append_entry`] and
//! the publication of the entry are not one atomic step, and no lock is
//! taken, so two `recipe guide` commands started at the same moment can both
//! pass it and leave a folder slightly over the limit. Only the same user can
//! do that, and the read limits ([`MAX_READ_ENTRIES`] entries and
//! [`MAX_READ_TOTAL_BYTES`] bytes of text, the newest kept) still bound what
//! a prompt can receive.
//!
//! Known limit: entries an agent step adds with `recipe guide --from-step`
//! are shown under their own preamble ([`render_block`]), but the label that
//! marks them is the caller field, which a process running as the same user
//! can leave out by unsetting `AMPLIHACK_RECIPE_RUN_ID` or by writing the
//! entry file itself. The refusal in `recipe guide` stops a misled agent that
//! uses the normal command; it is not a boundary against the same user.
//!
//! Guidance never goes into recipe context, step outputs, `run.json`, logs,
//! error messages or argv, so publish steps (which build PR titles, bodies
//! and commit messages from context and step outputs) cannot copy it.

pub(crate) mod env;
pub(crate) mod fs_safety;
pub(crate) mod pr_key;
#[cfg(test)]
mod routing_markers;
#[cfg(test)]
mod tests;

use crate::commands::recipe::run_id::validate_run_id;
use std::fmt;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The folder of entries, in a run folder or a per-PR state folder.
pub(crate) const GUIDANCE_DIR: &str = "guidance";
/// The JSONL log beside [`GUIDANCE_DIR`].
pub(crate) const GUIDANCE_LOG: &str = "guidance.log";
/// The folder under the state root that holds per-PR state folders.
pub(crate) const AUTO_DRIVE_DIR: &str = "auto-drive";
/// The largest entry text, in UTF-8 bytes after trimming.
pub(crate) const MAX_GUIDANCE_ENTRY_BYTES: usize = 50 * 1024;
/// Room for the header lines of an entry file, on the read side.
pub(crate) const HEADER_ROOM: usize = 4096;
/// The largest total size of the entry files in one folder.
pub(crate) const MAX_GUIDANCE_DIR_BYTES: u64 = 256 * 1024;
/// Readers use at most this many entries, the newest.
pub(crate) const MAX_READ_ENTRIES: usize = 256;
/// Readers use at most this many bytes of entry text in total, the newest
/// entries first. One full folder fits; several full folders do not.
pub(crate) const MAX_READ_TOTAL_BYTES: usize = 256 * 1024;
/// The launcher reads at most this many folders from the list.
pub(crate) const MAX_ENV_DIRS: usize = 16;
/// The folder list `recipe run` gives its runner.
pub(crate) const GUIDANCE_DIRS_ENV: &str = "AMPLIHACK_RUN_GUIDANCE_DIRS";

/// Bounded retries when an entry name is already taken.
const MAX_NAME_RETRIES: u64 = 1000;
/// The largest caller field.
const MAX_CALLER_BYTES: usize = 256;

const BLOCK_BEGIN: &str = "===== BEGIN OPERATOR INSTRUCTIONS FOR THIS RUN =====";
const BLOCK_END: &str = "===== END OPERATOR INSTRUCTIONS FOR THIS RUN =====";
const BLOCK_TITLE: &str = "Operator instructions for this run";
const BLOCK_PREAMBLE: &str = "These instructions come from the local operator and apply to this step. Do not copy them into recipe context, task descriptions, command-line arguments, pull request titles, bodies, comments or commit messages.";
const STEP_PREAMBLE: &str = "The entries below come from an agent step in this run, not from the local operator. Do not copy them into recipe context, task descriptions, command-line arguments, pull request titles, bodies, comments or commit messages.";
/// The caller-field marker [`caller_identity`] adds inside a recipe run.
const CALLER_RUN_MARKER: &str = " run=";

/// Why a guidance operation failed. Messages name paths, sizes and
/// characters, never the guidance text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuideError {
    /// Bad input: empty or unacceptable text, a malformed URL (exit 2).
    Usage(String),
    /// The text is over [`MAX_GUIDANCE_ENTRY_BYTES`] (exit 2). Its message
    /// is the documented one, with no prefix. `exact` is false when the read
    /// stopped past the limit without learning the full size (a pipe or a
    /// device given to `--file`); the message then says "more than".
    Oversize { size: u64, exact: bool },
    /// The target names no run, session or PR (exit 1).
    NotFound(String),
    /// A folder or file fails the trust rule, or the folder is full (exit 1).
    Unsafe(String),
    /// An I/O error (exit 1).
    Io(String),
}

impl GuideError {
    /// The process exit code for this error.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::Oversize { .. } => 2,
            Self::NotFound(_) | Self::Unsafe(_) | Self::Io(_) => 1,
        }
    }
}

impl fmt::Display for GuideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversize { size, exact: true } => write!(
                f,
                "guidance entry is {size} bytes; the limit is {MAX_GUIDANCE_ENTRY_BYTES} bytes"
            ),
            Self::Oversize { exact: false, .. } => write!(
                f,
                "guidance entry is more than {MAX_GUIDANCE_ENTRY_BYTES} bytes; the limit is {MAX_GUIDANCE_ENTRY_BYTES} bytes"
            ),
            Self::Usage(m) | Self::NotFound(m) | Self::Unsafe(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for GuideError {}

/// Which kind of folder an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Run,
    Pr,
}

impl Source {
    /// `pr` when the guidance folder's grandparent is named `auto-drive`
    /// (C3), `run` otherwise.
    pub(crate) fn of(guidance_dir: &Path) -> Self {
        let grandparent = guidance_dir.parent().and_then(Path::parent);
        if grandparent.and_then(Path::file_name) == Some(std::ffi::OsStr::new(AUTO_DRIVE_DIR)) {
            Self::Pr
        } else {
            Self::Run
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Pr => "pr",
        }
    }
}

/// One stored entry, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The file name, which sorts in append order.
    pub name: String,
    /// The guidance folder that holds it.
    pub dir: PathBuf,
    pub source: Source,
    pub timestamp: String,
    pub caller: String,
    pub text: String,
}

impl Entry {
    /// Whether an agent step inside a recipe run wrote this entry: its
    /// caller field carries the ` run=<id>` label.
    pub(crate) fn is_from_step(&self) -> bool {
        self.caller.contains(CALLER_RUN_MARKER)
    }
}

/// A successful append.
#[derive(Debug, Clone)]
pub(crate) struct Appended {
    pub path: PathBuf,
    /// UTF-8 bytes of the stored (trimmed) text.
    pub bytes: usize,
    /// Set when the entry was written but `guidance.log` was not.
    pub log_error: Option<String>,
}

fn is_block_delimiter(line: &str) -> bool {
    let line = line.trim();
    line == BLOCK_BEGIN || line == BLOCK_END
}

/// Checks operator text and returns the text to store: trimmed, non-empty,
/// at most [`MAX_GUIDANCE_ENTRY_BYTES`], free of control characters other
/// than tab, newline and a carriage return that ends a line (CRLF), of
/// bidirectional and zero-width characters, and of block delimiter lines.
///
/// Wording is not filtered: the provenance preambles of [`render_block`],
/// not a phrase list, tell an agent where the text came from (PR #2567 B2).
///
/// # Errors
///
/// [`GuideError::Oversize`] or [`GuideError::Usage`] (exit 2). The message
/// names the size or the character, never the text.
pub(crate) fn check_text(raw: &str) -> Result<String, GuideError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(GuideError::Usage("guidance text is empty".to_string()));
    }
    if text.len() > MAX_GUIDANCE_ENTRY_BYTES {
        return Err(GuideError::Oversize {
            size: text.len() as u64,
            exact: true,
        });
    }
    if let Some(c) = text.chars().find(|c| {
        !matches!(c, '\t' | '\n' | '\r') && crate::commands::agent::validate::unsafe_char(*c)
    }) {
        return Err(GuideError::Usage(format!(
            "guidance text contains a control character (U+{:04X})",
            c as u32
        )));
    }
    // `lines()` splits only on `\n`, so a lone `\r` could hide a delimiter
    // line from the check below while a model still reads it as a break.
    if text.split("\r\n").any(|part| part.contains('\r')) {
        return Err(GuideError::Usage(
            "guidance text contains a control character (U+000D) not followed by a line feed"
                .to_string(),
        ));
    }
    if text.lines().any(is_block_delimiter) {
        return Err(GuideError::Usage(
            "guidance text contains a line equal to an operator-instructions block delimiter"
                .to_string(),
        ));
    }
    Ok(text.to_string())
}

/// A caller field safe to write on one header line: control, bidirectional
/// and zero-width characters and the line and paragraph separators (U+2028,
/// U+2029) dropped, at most 256 bytes, `unknown` when
/// nothing is left.
pub(crate) fn sanitize_caller(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars().filter(|c| {
        !crate::commands::agent::validate::unsafe_char(*c) && !matches!(c, '\u{2028}' | '\u{2029}')
    }) {
        if out.len() + c.len_utf8() > MAX_CALLER_BYTES {
            break;
        }
        out.push(c);
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The user name of the effective uid, from the password database.
fn passwd_name() -> Option<String> {
    let uid = fs_safety::euid();
    let mut buf = vec![0_u8; 4096];
    // SAFETY: an all-zero passwd is a valid out-parameter for getpwuid_r.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call and `buf` outlives it.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || pwd.pw_name.is_null() {
        return None;
    }
    // SAFETY: pw_name points at a NUL-terminated string inside `buf`.
    let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    name.to_str().ok().map(str::to_string)
}

/// Who is appending: the user name of the effective uid (falling back to
/// `$USER`, then `unknown`), plus ` run=<id>` when this command runs inside
/// a recipe run (`AMPLIHACK_RECIPE_RUN_ID` set and not empty). An id that
/// fails validation is written as ` run=invalid`, never raw, so the entry is
/// still marked as written by an agent step. No credential file is read.
pub(crate) fn caller_identity() -> String {
    let user = passwd_name()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_default();
    let mut caller = sanitize_caller(&user).replace(' ', "_");
    if let Some(run) = std::env::var_os("AMPLIHACK_RECIPE_RUN_ID").filter(|v| !v.is_empty()) {
        caller.push_str(CALLER_RUN_MARKER);
        match run.to_str().filter(|r| validate_run_id(r)) {
            Some(run) => caller.push_str(run),
            None => caller.push_str("invalid"),
        }
    }
    sanitize_caller(&caller)
}

/// Whether `name` is an entry file name:
/// `^\d{8}T\d{6}\.\d{9}Z-\d+-\d+\.md$`.
fn is_entry_name(name: &str) -> bool {
    let b = name.as_bytes();
    let digits = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_digit);
    if b.len() < 31 || !name.ends_with(".md") {
        return false;
    }
    let head_ok = digits(&b[0..8])
        && b[8] == b'T'
        && digits(&b[9..15])
        && b[15] == b'.'
        && digits(&b[16..25])
        && b[25] == b'Z'
        && b[26] == b'-';
    if !head_ok {
        return false;
    }
    let tail = &name[27..name.len() - 3];
    match tail.split_once('-') {
        Some((pid, seq)) => digits(pid.as_bytes()) && digits(seq.as_bytes()),
        None => false,
    }
}

/// Total size of the entry files in `dir` (names that match the pattern,
/// regular files only).
fn folder_total(dir: &Path) -> Result<u64, GuideError> {
    let rd = fs::read_dir(dir)
        .map_err(|e| GuideError::Io(format!("cannot list {}: {e}", dir.display())))?;
    let mut total = 0_u64;
    for item in rd {
        let item =
            item.map_err(|e| GuideError::Io(format!("cannot list {}: {e}", dir.display())))?;
        let name = item.file_name();
        if !name.to_str().is_some_and(is_entry_name) {
            continue;
        }
        // `DirEntry::metadata` does not follow links on Unix (an `lstat`
        // relative to the open folder), so no full path is built.
        if let Ok(meta) = item.metadata()
            && meta.file_type().is_file()
        {
            total += meta.len();
        }
    }
    Ok(total)
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Creates a private temporary file in `dir` holding `body`; returns its path.
fn write_tmp(dir: &Path, body: &[u8]) -> Result<PathBuf, GuideError> {
    let pid = std::process::id();
    for _ in 0..MAX_NAME_RETRIES {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".tmp-{pid}-{seq}"));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&path);
        let mut file = match file {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(GuideError::Io(format!(
                    "cannot create a file in {}: {e}",
                    dir.display()
                )));
            }
        };
        if let Err(e) = file.write_all(body).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(GuideError::Io(format!(
                "cannot write a file in {}: {e}",
                dir.display()
            )));
        }
        return Ok(path);
    }
    Err(GuideError::Io(format!(
        "cannot find a free temporary name in {}",
        dir.display()
    )))
}

/// Publishes `tmp` under the first free entry name for `stamp`, by hard
/// link, so a reader never sees a partly written entry and no entry is ever
/// replaced.
fn publish(dir: &Path, tmp: &Path, stamp: &str) -> Result<PathBuf, GuideError> {
    let pid = std::process::id();
    for seq in 0..MAX_NAME_RETRIES {
        let path = dir.join(format!("{stamp}-{pid}-{seq}.md"));
        match fs::hard_link(tmp, &path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(GuideError::Io(format!(
                    "cannot add an entry to {}: {e}",
                    dir.display()
                )));
            }
        }
    }
    Err(GuideError::Io(format!(
        "{MAX_NAME_RETRIES} entries in {} already share one timestamp",
        dir.display()
    )))
}

/// Appends `text` (already checked by [`check_text`]) to `guidance_dir` as a
/// new entry written by `caller`, then logs it in `guidance.log` beside the
/// folder.
///
/// The parent of `guidance_dir` must exist and pass the trust rule;
/// `guidance_dir` is created at 0700 when missing.
///
/// # Errors
///
/// [`GuideError::NotFound`] when the parent is missing,
/// [`GuideError::Unsafe`] when a folder fails the trust rule or the folder
/// would exceed [`MAX_GUIDANCE_DIR_BYTES`], [`GuideError::Io`] otherwise.
/// Nothing is written on error. A failed log write after the entry exists is
/// reported in [`Appended::log_error`], not as an error.
pub(crate) fn append_entry(
    guidance_dir: &Path,
    text: &str,
    caller: &str,
) -> Result<Appended, GuideError> {
    let parent = guidance_dir.parent().ok_or_else(|| {
        GuideError::NotFound(format!("{} has no parent folder", guidance_dir.display()))
    })?;
    if !fs_safety::check_dir(parent)? {
        return Err(GuideError::NotFound(format!(
            "{} does not exist",
            parent.display()
        )));
    }
    fs_safety::ensure_private_dir(guidance_dir)?;

    let caller = sanitize_caller(caller);
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string();
    let stamp = now.format("%Y%m%dT%H%M%S%.9fZ").to_string();
    let body = format!("timestamp: {timestamp}\ncaller: {caller}\n\n{text}");

    let total = folder_total(guidance_dir)?;
    let after = total + body.len() as u64;
    if after > MAX_GUIDANCE_DIR_BYTES {
        return Err(GuideError::Unsafe(format!(
            "guidance folder {} would hold {after} bytes of entries; the limit is {MAX_GUIDANCE_DIR_BYTES}",
            guidance_dir.display()
        )));
    }

    let tmp = write_tmp(guidance_dir, body.as_bytes())?;
    let published = publish(guidance_dir, &tmp, &stamp);
    let _ = fs::remove_file(&tmp);
    let path = published?;

    let entry = path
        .file_name()
        .and_then(|n| n.to_str())
        .expect("publish returns a path with a UTF-8 file name")
        .to_string();
    let record = serde_json::json!({
        "event": "guidance_appended",
        "ts": timestamp,
        "caller": caller,
        "bytes": text.len(),
        "entry": entry,
    });
    let log_error = log_event(&parent.join(GUIDANCE_LOG), &record)
        .err()
        .map(|e| e.to_string());
    Ok(Appended {
        path,
        bytes: text.len(),
        log_error,
    })
}

/// Appends one JSON line to `log` (created 0600; never through a symlink;
/// the opened file must pass the trust rule).
pub(crate) fn log_event(log: &Path, record: &serde_json::Value) -> Result<(), GuideError> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        // O_NONBLOCK: a FIFO planted at this path must not hang the caller;
        // the regular-file check below then refuses it.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .mode(0o600)
        .open(log)
        .map_err(|e| GuideError::Io(format!("cannot open {}: {e}", log.display())))?;
    let meta = file
        .metadata()
        .map_err(|e| GuideError::Io(format!("cannot check {}: {e}", log.display())))?;
    if let Some(reason) = fs_safety::untrusted_reason(&meta, false) {
        return Err(GuideError::Unsafe(format!(
            "refusing {}: {reason}",
            log.display()
        )));
    }
    let mut line = record.to_string();
    line.push('\n');
    file.write_all(line.as_bytes())
        .map_err(|e| GuideError::Io(format!("cannot write {}: {e}", log.display())))
}

fn warn_entry(path: &Path, reason: &str) {
    eprintln!(
        "amplihack: skipping operator guidance entry {}: {reason}",
        path.display()
    );
}

/// Parses an entry file: header lines up to the first blank line (unknown
/// ones ignored), then the text.
fn parse_entry(body: &str) -> Option<(String, String, &str)> {
    let (header, text) = body.split_once("\n\n")?;
    let mut timestamp = String::new();
    let mut caller = String::new();
    for line in header.lines() {
        if let Some(v) = line.strip_prefix("timestamp: ") {
            timestamp = v.to_string();
        } else if let Some(v) = line.strip_prefix("caller: ") {
            caller = v.to_string();
        }
    }
    Some((timestamp, caller, text))
}

/// Reads one entry; `None` (with a warning naming the path) when it fails a
/// check.
fn read_entry(dir: &Path, name: String) -> Option<Entry> {
    let path = &dir.join(&name);
    let max = (MAX_GUIDANCE_ENTRY_BYTES + HEADER_ROOM) as u64;
    let (bytes, meta) =
        match crate::commands::cleanup::run_dir::read_capped_nofollow_with_metadata(path, max) {
            Ok(read) => read,
            Err(e) => {
                warn_entry(path, &e.to_string());
                return None;
            }
        };
    if let Some(reason) = fs_safety::untrusted_reason(&meta, false) {
        warn_entry(path, &reason);
        return None;
    }
    let Ok(body) = String::from_utf8(bytes) else {
        warn_entry(path, "it is not valid UTF-8");
        return None;
    };
    let Some((timestamp, caller, text)) = parse_entry(&body) else {
        warn_entry(path, "it has no header");
        return None;
    };
    let text = match check_text(text) {
        Ok(text) => text,
        Err(e) => {
            warn_entry(path, &e.to_string());
            return None;
        }
    };
    Some(Entry {
        name,
        dir: dir.to_path_buf(),
        source: Source::of(dir),
        timestamp: sanitize_caller(&timestamp),
        caller: sanitize_caller(&caller),
        text,
    })
}

/// The newest entries in `dirs`, merged and returned in file name (append)
/// order: at most [`MAX_READ_ENTRIES`] entries and [`MAX_READ_TOTAL_BYTES`]
/// bytes of text. Entries are read newest first, and reading stops at the
/// first entry that does not fit, so what is kept is always the most recent
/// run of entries and the operator's latest correction is never the one left
/// out. Missing folders are skipped silently; folders, files and texts that
/// fail a check are skipped with a warning and do not count.
pub(crate) fn read_entries(dirs: &[PathBuf]) -> Vec<Entry> {
    let mut found: Vec<(String, usize)> = Vec::new();
    for (index, dir) in dirs.iter().enumerate() {
        match fs_safety::check_dir(dir) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                eprintln!("amplihack: ignoring operator guidance folder: {e}");
                continue;
            }
        }
        let rd = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                eprintln!(
                    "amplihack: ignoring operator guidance folder {}: {e}",
                    dir.display()
                );
                continue;
            }
        };
        for item in rd.flatten() {
            if let Some(name) = item.file_name().to_str().filter(|n| is_entry_name(n)) {
                found.push((name.to_string(), index));
            }
        }
    }
    found.sort();
    let mut kept: Vec<Entry> = Vec::new();
    let mut bytes = 0_usize;
    // The limit reached, and how many read entries it left out (0 or 1).
    let mut limit: Option<(String, usize)> = None;
    let mut candidates = found.into_iter().rev();
    for (name, index) in candidates.by_ref() {
        let Some(entry) = read_entry(&dirs[index], name) else {
            continue;
        };
        if bytes + entry.text.len() > MAX_READ_TOTAL_BYTES {
            limit = Some((format!("{MAX_READ_TOTAL_BYTES} bytes"), 1));
            break;
        }
        bytes += entry.text.len();
        kept.push(entry);
        if kept.len() == MAX_READ_ENTRIES {
            limit = Some((format!("{MAX_READ_ENTRIES} entries"), 0));
            break;
        }
    }
    if let Some((limit, unfit)) = limit {
        // The older files are counted without being read, so one that would
        // fail a check is still counted as left out.
        let left_out = unfit + candidates.count();
        if left_out > 0 {
            eprintln!(
                "amplihack: operator guidance is over the read limit of {limit}; using the newest {} entries and leaving out {left_out} older ones",
                kept.len()
            );
        }
    }
    kept.reverse();
    kept
}

/// The operator-instructions block for `entries` (empty string for none).
///
/// Entries from the operator come first, under the operator preamble.
/// Entries an agent step wrote ([`Entry::from_step`]) follow under their own
/// preamble, so a step that was misled into adding one cannot present it as
/// the operator's. Numbering runs on across both parts.
pub(crate) fn render_block(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    use std::fmt::Write as _;
    let (from_steps, from_operator): (Vec<&Entry>, Vec<&Entry>) =
        entries.iter().partition(|e| e.is_from_step());
    let mut out = format!("{BLOCK_BEGIN}\n{BLOCK_TITLE}\n");
    let mut number = 0_usize;
    for (preamble, part) in [
        (BLOCK_PREAMBLE, &from_operator),
        (STEP_PREAMBLE, &from_steps),
    ] {
        if part.is_empty() {
            continue;
        }
        if number > 0 {
            out.push('\n');
        }
        out.push_str(preamble);
        out.push('\n');
        for e in part {
            number += 1;
            // Writing to a String cannot fail.
            let _ = writeln!(
                out,
                "\n[{number}] {} from {} ({})",
                e.timestamp,
                e.caller,
                e.source.label()
            );
            out.push_str(&e.text);
            out.push('\n');
        }
    }
    out.push_str(BLOCK_END);
    out.push('\n');
    out
}

/// `prompt` with trailing line breaks removed, one blank line, then `block`.
pub(crate) fn append_block(prompt: &str, block: &str) -> String {
    format!("{}\n\n{block}", prompt.trim_end_matches(['\n', '\r']))
}

/// Whether a Codex launch takes the guidance trailer (C4): `exec` is among
/// the arguments and the last argument is exactly `-`, so the prompt is the
/// caller's stdin.
pub(crate) fn codex_trailer_applies(args: &[String]) -> bool {
    args.last().is_some_and(|a| a == "-")
        && args.len() >= 2
        && args[..args.len() - 1].iter().any(|a| a == "exec")
}

/// The guidance one agent launch adds: read once, when the step starts.
#[derive(Debug, Default)]
pub(crate) struct GuidanceForLaunch {
    /// Folders that held at least one used entry.
    sources: Vec<PathBuf>,
    entries: usize,
    bytes: usize,
    block: Option<String>,
}

impl GuidanceForLaunch {
    /// Reads the folders listed in [`GUIDANCE_DIRS_ENV`]; nothing when the
    /// variable is unset.
    pub(crate) fn load() -> Self {
        match std::env::var_os(GUIDANCE_DIRS_ENV) {
            Some(value) => Self::from_dirs(env::dirs_from_env_value(&value)),
            None => Self::default(),
        }
    }

    /// Reads `dirs` (already checked or trusted by the caller).
    pub(crate) fn from_dirs(dirs: Vec<PathBuf>) -> Self {
        let entries = read_entries(&dirs);
        if entries.is_empty() {
            return Self::default();
        }
        let mut sources: Vec<PathBuf> = Vec::new();
        for e in &entries {
            if !sources.contains(&e.dir) {
                sources.push(e.dir.clone());
            }
        }
        Self {
            sources,
            entries: entries.len(),
            bytes: entries.iter().map(|e| e.text.len()).sum(),
            block: Some(render_block(&entries)),
        }
    }

    /// How many entries the block holds.
    pub(crate) fn entries(&self) -> usize {
        self.entries
    }

    /// Total UTF-8 bytes of the entry texts.
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The block, when there is guidance.
    pub(crate) fn block(&self) -> Option<&str> {
        self.block.as_deref()
    }

    /// `prompt` followed by the block, or `prompt` unchanged.
    pub(crate) fn append_to(&self, prompt: &str) -> String {
        match &self.block {
            Some(block) => append_block(prompt, block),
            None => prompt.to_string(),
        }
    }

    /// The bytes written after a caller's stdin prompt: a blank line, then
    /// the block.
    pub(crate) fn trailer(&self) -> Option<Vec<u8>> {
        self.block.as_ref().map(|b| format!("\n\n{b}").into_bytes())
    }

    /// The one stderr line a launch prints when it added the block.
    pub(crate) fn notice(&self) -> String {
        format!(
            "amplihack: operator guidance added to this step's prompt ({} {}, {} bytes)",
            self.entries(),
            if self.entries() == 1 {
                "entry"
            } else {
                "entries"
            },
            self.bytes()
        )
    }

    /// Logs `guidance_injected` (counts only) in the `guidance.log` of each
    /// folder that supplied an entry. Call only after the agent started.
    pub(crate) fn record_injection(&self, step_run_id: &str) {
        if self.entries == 0 {
            return;
        }
        let ts = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.9fZ")
            .to_string();
        let step = if validate_run_id(step_run_id) {
            step_run_id
        } else {
            "unknown"
        };
        let record = serde_json::json!({
            "event": "guidance_injected",
            "ts": ts,
            "step_run_id": step,
            "entries": self.entries,
            "bytes": self.bytes,
        });
        for dir in &self.sources {
            let Some(parent) = dir.parent() else { continue };
            if let Err(e) = log_event(&parent.join(GUIDANCE_LOG), &record) {
                eprintln!("amplihack: warning: operator guidance was added but not logged: {e}");
            }
        }
    }
}
