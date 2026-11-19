//! Issue #2535: `amplihack recipe guide <TARGET> [-] [--file <PATH>] [--from-step]`.
//!
//! Appends one operator instruction to a recipe run's guidance. Every later
//! agent step of that run receives all entries, in order, after its own
//! prompt (see [`crate::run_guidance`]).
//!
//! The text is read from stdin (no argument, or `-`) or from `--file <PATH>`,
//! never from the command line (PR #2567, issue #2582): argv is visible to
//! other processes and is copied into logs. Any other positional value is
//! refused before anything else is checked, read or written.
//!
//! A run is found from what it recorded when it started, so the command works
//! from any shell whatever `AMPLIHACK_WORK_ROOT` says (issue #2783). At start
//! a run writes an `amplihack.recipe.log_pointer` line carrying `run_dir`,
//! the absolute path of its own folder; for a detached run that line is kept
//! in `<recipe log dir>/<session>.log`, a directory that does not depend on
//! the work root. `guide_records` reads those lines back.
//!
//! The target is resolved in this order; the first match wins:
//!
//! 1. a GitHub issue URL: the recorded run that names that issue. This rule
//!    never creates the state folder;
//! 2. a pull request URL (any other target with `://` gets this rule's
//!    refusal): the per-PR state folder, created when absent;
//! 3. a detached session name: the pointer line of
//!    `<recipe log dir>/<session>.log`, through its recorded `run_dir` first
//!    and the work root in the environment second;
//! 4. a run folder name or correlation id: the recorded `run_dir` of the
//!    pointer line carrying it, then `<work root>/runs/` as a folder name
//!    and then as `attempts[].correlation_id` in each run's `run.json`;
//! 5. a bare issue number, `<n>` or `#<n>`: the recorded run that names that
//!    issue number, whatever the repository.
//!
//! Two kinds of input, treated differently (issue #2809). What the operator
//! named -- the target, the session log it points at, the `--file` path and
//! the destination folder -- is refused when it fails a check. What a record
//! claimed is never refused: a claim that fails a check becomes a note in the
//! not-found message and the next rule is still tried. Nothing here fails
//! closed, and no new refusal was added.
//!
//! Inside a recipe run (`AMPLIHACK_RECIPE_RUN_ID` set) the command is refused
//! unless `--from-step` is passed, so an agent step misled by text it read
//! does not add guidance by accident. Entries it adds with the flag carry
//! ` run=<id>` in their caller field and are shown under their own preamble.
//!
//! Exit codes: 0 appended; 1 target not found, a folder fails a check, or
//! the `--file` file cannot be read; 2 usage (text on the command line, `-`
//! together with `--file`, a terminal on stdin, empty, oversize or invalid
//! UTF-8 text, a delimiter or control character, malformed URL, called
//! inside a run without `--from-step`).
//! Every failure prints one line on stderr and writes nothing. The not-found
//! line names the target and every place that was looked at, so a miss says
//! what was searched rather than only that it failed.

use super::guide_records::{self, Pointer};
use crate::command_error::exit_error;
use crate::commands::cleanup::run_dir::{RUN_JSON, RunReport};
use crate::commands::cleanup::work_root;
use crate::commands::recipe::run_id::validate_run_id;
use crate::run_guidance::fs_safety::{check_dir, ensure_private_chain, untrusted_reason};
use crate::run_guidance::pr_key::{
    PrRef, parse_issue_url, parse_pr_url, pr_guidance_parent, pr_key, same_repo_ref, state_root,
};
use crate::run_guidance::{
    AUTO_DRIVE_DIR, GUIDANCE_DIR, GuideError, HEADER_ROOM, MAX_GUIDANCE_ENTRY_BYTES, Source,
    append_entry, caller_identity, check_text,
};
use anyhow::Result;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

/// The most raw bytes read from stdin or a file: room for surrounding white
/// space, which is trimmed before the entry limit applies.
const READ_CAP: u64 = (MAX_GUIDANCE_ENTRY_BYTES + HEADER_ROOM) as u64;

/// The refusal for text on the command line. It never includes the value.
const POSITIONAL_REFUSAL: &str = "instruction text is not accepted on the command line; pipe it on stdin (or pass -) or use --file <path>";
/// The refusal for `-` together with `--file`.
const BOTH_SOURCES: &str = "give the text on stdin (-) or with --file <path>, not both";
/// The refusal for a terminal on stdin with no `--file`.
const TERMINAL_STDIN: &str = "no text given and stdin is a terminal; pipe the text on stdin (or pass -) or use --file <path>";

/// Run `amplihack recipe guide`.
pub fn run_guide(
    target: &str,
    positional: &[OsString],
    file: Option<&Path>,
    from_step: bool,
) -> Result<()> {
    match guide(target, positional, file, from_step) {
        Ok(()) => Ok(()),
        Err(error) => {
            // One line, always: `sanitize_line` is the single outlet, so
            // nothing taken from a log or a path can add a line or a
            // terminal sequence to it (issue #2783).
            let message = sanitize_line(&error.to_string());
            match &error {
                // The documented limit message stands alone.
                GuideError::Oversize { .. } => eprintln!("{message}"),
                _ => eprintln!("recipe guide: {message}"),
            }
            Err(exit_error(error.exit_code()))
        }
    }
}

fn guide(
    target: &str,
    positional: &[OsString],
    file: Option<&Path>,
    from_step: bool,
) -> Result<(), GuideError> {
    // A pure argv check, first: a refused call reads and writes nothing,
    // including the per-PR folder `resolve_target` would create.
    let (dash, file_after_dash, from_step_after_dash) = split_positional(positional)?;
    let from_step = from_step || from_step_after_dash;
    if dash && (file.is_some() || file_after_dash) {
        return Err(GuideError::Usage(BOTH_SOURCES.to_string()));
    }
    if !from_step && std::env::var_os("AMPLIHACK_RECIPE_RUN_ID").is_some_and(|v| !v.is_empty()) {
        return Err(GuideError::Usage(
            "refusing to add guidance from inside a recipe run (AMPLIHACK_RECIPE_RUN_ID is set); an agent step that means to add an entry must pass --from-step, and the entry is then shown as coming from an agent step"
                .to_string(),
        ));
    }
    let raw = match file {
        Some(path) => read_file(path)?,
        None => read_stdin()?,
    };
    let text = check_text(&raw)?;
    let Destination {
        dir,
        source,
        warning,
    } = resolve_target(target)?;
    let appended = append_entry(&dir, &text, &caller_identity())?;
    // PR #2567 M1: a PR folder is read only by runs started with the pull
    // request in their context, so a PR entry is not reported as "appended"
    // to anything that will read it.
    let verb = match source {
        Source::Pr => "guidance saved for pull request",
        _ => "guidance appended",
    };
    println!(
        "{verb}: {} ({} bytes, {})",
        appended.path.display(),
        appended.bytes,
        source.label()
    );
    if source == Source::Pr {
        println!(
            "written to {}; it is read at the next agent step of each run started with this pull request in its context (pr_url or pr_number), such as a later auto-drive round. A run that opened this pull request itself does not read it; address that run by its run ID or session name.",
            dir.display()
        );
    }
    // After the append, so a failed append still prints exactly one line.
    if let Some(warning) = warning {
        eprintln!("recipe guide: warning: {}", sanitize_line(&warning));
    }
    if let Some(error) = appended.log_error {
        eprintln!("recipe guide: warning: the entry was added but guidance.log was not: {error}");
    }
    Ok(())
}

/// How [`read_bounded`] reports input over [`READ_CAP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overflow {
    /// Read the rest without keeping it and report the exact size (stdin).
    Drain,
    /// Report this size, taken from the open handle of a regular file. A size
    /// at or below the cap cannot be right and is reported as "more than".
    Size(u64),
    /// Stop one byte past the cap and report "more than" (a FIFO or device).
    Stop,
}

/// Checks the hidden positional values. The only one accepted is a lone `-`.
///
/// The positional allows leading hyphens, so text such as `--force` gets
/// [`POSITIONAL_REFUSAL`] rather than clap's error, which would repeat it.
/// clap then also gives this argument the options that follow its first
/// value, so `--from-step` and `--file` after `-` are read here. Returns
/// whether `-` was given, whether `--file` followed it, and whether
/// `--from-step` followed it.
fn split_positional(positional: &[OsString]) -> Result<(bool, bool, bool), GuideError> {
    let refuse = || GuideError::Usage(POSITIONAL_REFUSAL.to_string());
    let Some((first, rest)) = positional.split_first() else {
        return Ok((false, false, false));
    };
    if first != "-" {
        return Err(refuse());
    }
    let (mut file, mut from_step) = (false, false);
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        if arg == "--from-step" {
            from_step = true;
        } else if arg == "--file" {
            rest.next().ok_or_else(refuse)?;
            file = true;
        } else if arg.to_str().is_some_and(|a| a.starts_with("--file=")) {
            file = true;
        } else {
            return Err(refuse());
        }
    }
    Ok((true, file, from_step))
}

/// Reads at most [`READ_CAP`] + 1 bytes of `src` and returns them as text,
/// untrimmed. `what` names the source in I/O errors; no error ever holds any
/// of the bytes read.
fn read_bounded<R: Read>(
    src: &mut R,
    what: &str,
    overflow: Overflow,
) -> Result<String, GuideError> {
    let io_error = |e: std::io::Error| GuideError::Io(format!("cannot read {what}: {e}"));
    let mut buf = Vec::new();
    (&mut *src)
        .take(READ_CAP + 1)
        .read_to_end(&mut buf)
        .map_err(io_error)?;
    if buf.len() as u64 > READ_CAP {
        let (size, exact) = match overflow {
            Overflow::Drain => {
                let rest = std::io::copy(src, &mut std::io::sink()).map_err(io_error)?;
                (buf.len() as u64 + rest, true)
            }
            // More than READ_CAP bytes were read, so a smaller size is wrong
            // (procfs, sysfs, or a file that grew during the read).
            Overflow::Size(size) if size > READ_CAP => (size, true),
            Overflow::Size(_) | Overflow::Stop => (buf.len() as u64, false),
        };
        return Err(GuideError::Oversize { size, exact });
    }
    String::from_utf8(buf)
        .map_err(|_| GuideError::Usage("guidance text is not valid UTF-8".to_string()))
}

/// The text on stdin, refusing a terminal.
fn read_stdin() -> Result<String, GuideError> {
    // SAFETY: isatty has no preconditions.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Err(GuideError::Usage(TERMINAL_STDIN.to_string()));
    }
    read_bounded(&mut std::io::stdin().lock(), "stdin", Overflow::Drain)
}

/// The text in `path`, opened once and read only through that handle. A
/// symbolic link is followed with the caller's own permissions.
fn read_file(path: &Path) -> Result<String, GuideError> {
    let what = path.display().to_string();
    let io_error = |e: std::io::Error| GuideError::Io(format!("cannot read {what}: {e}"));
    let mut file = fs::File::open(path).map_err(io_error)?;
    let meta = file.metadata().map_err(io_error)?;
    let overflow = if meta.is_file() {
        Overflow::Size(meta.len())
    } else {
        Overflow::Stop
    };
    read_bounded(&mut file, &what, overflow)
}

/// Where one entry goes.
struct Destination {
    /// The guidance folder.
    dir: PathBuf,
    /// Which kind of folder it is, which the printed line names.
    source: Source,
    /// Printed on stderr after the append, when more than one recorded run
    /// matched an issue and the newest was taken.
    warning: Option<String>,
}

impl Destination {
    fn run(run_dir: &Path, warning: Option<String>) -> Self {
        Self {
            dir: run_dir.join(GUIDANCE_DIR),
            source: Source::Run,
            warning,
        }
    }
}

/// A value shown on stderr: in quotes, with every character that could make
/// the line read differently from what it holds escaped as `\u{..}`.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if needs_escape(c) => out.push_str(&escaped(c)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Characters that never reach stderr as themselves: control characters,
/// the bidirectional and zero-width marks, and the line and paragraph
/// separators, which would end the one line a failure prints.
fn needs_escape(c: char) -> bool {
    crate::commands::agent::validate::unsafe_char(c) || matches!(c, '\u{2028}' | '\u{2029}')
}

fn escaped(c: char) -> String {
    format!("\\u{{{:04x}}}", c as u32)
}

/// The last pass over a message before it is printed. Paths reach it from
/// `check_dir`, the work root validator and `run.json`, which build their
/// own text, so the escaping is applied once here to all of it.
fn sanitize_line(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for c in message.chars() {
        if needs_escape(c) {
            out.push_str(&escaped(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// An error and its causes on one line.
fn one_line(e: &anyhow::Error) -> String {
    format!("{e:#}").replace(['\n', '\r'], " ")
}

/// The issue number a bare target names: `<n>` or `#<n>`, ASCII digits only
/// so neither a sign nor white space is read as a number.
fn bare_issue_number(target: &str) -> Option<u64> {
    let digits = target.strip_prefix('#').unwrap_or(target);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether a recorded value is an issue number equal to `number`.
fn records_issue_number(recorded: Option<&str>, number: u64) -> bool {
    recorded.is_some_and(|value| {
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
            && value.parse::<u64>().is_ok_and(|n| n == number)
    })
}

/// Whether a folder a record named may be written to.
///
/// Every reason it returns is a note, never a refusal: a record is a claim,
/// and a claim that does not hold must not stop a later rule from matching
/// (issue #2809). `run_id` is the correlation id the record carried, which
/// the folder's own `run.json` must agree with, so a log planted beside the
/// real ones cannot point the entry at somebody else's run.
fn accept_recorded(recorded: &str, run_id: Option<&str>) -> Result<PathBuf, String> {
    let shown = quoted(recorded);
    // A lossy conversion stands for bytes that are not UTF-8, so the string
    // cannot name the folder it came from.
    if recorded.contains('\u{fffd}') {
        return Err(format!(
            "the recorded run folder {shown} holds U+FFFD and cannot name a real folder"
        ));
    }
    // The raw string, not `Path::components`, which drops empty and `.`
    // segments before they can be seen.
    let Some(rest) = recorded.strip_prefix('/') else {
        return Err(format!(
            "the recorded run folder {shown} is not an absolute path"
        ));
    };
    if rest.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err(format!(
            "the recorded run folder {shown} has an empty, `.` or `..` segment"
        ));
    }
    let dir = PathBuf::from(recorded);
    let name = dir.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if !validate_run_id(name) {
        return Err(format!(
            "the recorded run folder {shown} is not named like a run"
        ));
    }
    let Some(runs) = dir.parent() else {
        return Err(format!("the recorded run folder {shown} has no parent"));
    };
    if runs.file_name() != Some(OsStr::new("runs")) {
        return Err(format!(
            "the recorded run folder {shown} is not inside a `runs` folder"
        ));
    }
    let Some(root) = runs.parent() else {
        return Err(format!("the recorded run folder {shown} has no work root"));
    };
    // An unset HOME skips the comparison with the home folder rather than
    // refusing; the not-found line still names what could not be resolved.
    let home = crate::user_config::home_dir().unwrap_or_default();
    if let Err(e) = work_root::validate(root, &home) {
        return Err(format!(
            "the work root of the recorded run folder {shown} is not usable: {}",
            one_line(&e)
        ));
    }
    // `lstat` at each level from the work root down, so a link swapped in
    // above the run folder is seen rather than followed.
    for level in [root, runs, dir.as_path()] {
        match check_dir(level) {
            Ok(true) => {}
            Ok(false) if level == dir.as_path() => {
                return Err(format!("the recorded run folder {shown} no longer exists"));
            }
            Ok(false) => {
                return Err(format!(
                    "the recorded run folder {shown} is gone: {} does not exist",
                    quoted(&level.display().to_string())
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let report_path = dir.join(RUN_JSON);
    match fs::symlink_metadata(&report_path) {
        Ok(meta) => {
            if let Some(reason) = untrusted_reason(&meta, false) {
                return Err(format!("refusing {}: {reason}", report_path.display()));
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(format!("the recorded run folder {shown} has no {RUN_JSON}"));
        }
        Err(e) => return Err(format!("cannot check {}: {e}", report_path.display())),
    }
    let report = RunReport::read(&dir)
        .map_err(|e| format!("cannot read {}: {}", report_path.display(), one_line(&e)))?;
    if report.run_id != name {
        return Err(format!(
            "{} names run {}, not {}",
            report_path.display(),
            quoted(&report.run_id),
            quoted(name)
        ));
    }
    if let Some(id) = run_id
        && id != name
        && !report.attempts.iter().any(|a| a.correlation_id == id)
    {
        return Err(format!(
            "{} does not record run {}",
            report_path.display(),
            quoted(id)
        ));
    }
    Ok(dir)
}

/// One target being resolved: the places it can be looked for, and what
/// each of them yielded.
struct Resolver<'a> {
    target: &'a str,
    /// The recipe log directory, or why it could not be resolved.
    log_dir: Result<PathBuf, String>,
    /// `<work root>/runs`, or why the work root could not be resolved. Kept
    /// as a `Result`, so a work root that cannot be resolved never turns
    /// into a misleading "not found".
    runs: Result<PathBuf, String>,
    /// The records, read at most once and only when a cheaper rule missed.
    records: Vec<Pointer>,
    scanned: bool,
    /// Where the command looked, in the order it looked.
    notes: Vec<String>,
    /// Why a folder a record named was not used. Kept apart from the places
    /// so the line stays bounded when many records are rejected.
    rejected: Vec<String>,
}

/// The most rejection reasons shown; the rest are counted.
const MAX_SHOWN_REJECTIONS: usize = 3;

impl<'a> Resolver<'a> {
    fn new(target: &'a str) -> Self {
        Self {
            target,
            log_dir: super::detach::log_dir().map_err(|e| one_line(&e)),
            runs: work_root::resolve_from_env()
                .map(|w| w.runs_dir())
                .map_err(|e| one_line(&e)),
            records: Vec::new(),
            scanned: false,
            notes: Vec::new(),
            rejected: Vec::new(),
        }
    }

    /// A place that was looked at and what it held.
    fn note(&mut self, note: String) {
        self.notes.push(sanitize_line(&note));
    }

    /// A folder a record named that was not used, and why.
    fn reject(&mut self, reason: String) {
        self.rejected.push(sanitize_line(&reason));
    }

    /// Every record in the log directory, newest log first. Read once, and
    /// only after the lookups that need no scan have missed.
    fn records(&mut self) -> Vec<Pointer> {
        if !self.scanned {
            self.scanned = true;
            match self.log_dir.clone() {
                Ok(dir) => {
                    let scan = guide_records::scan(&dir);
                    self.note(format!(
                        "pointer lines in {} ({})",
                        dir.display(),
                        scan.summary()
                    ));
                    self.records = scan.records;
                }
                Err(e) => self.note(format!("the recipe log directory cannot be resolved: {e}")),
            }
        }
        self.records.clone()
    }

    /// The recorded runs that name this issue, in the order their logs were
    /// last written, newest first. A record that fails a check becomes a
    /// note here and the search goes on.
    fn issue_destination(&mut self, url: Option<&PrRef>, number: u64) -> Option<Destination> {
        let mut matched: Vec<PathBuf> = Vec::new();
        for record in self.records() {
            let names_it = match url {
                Some(wanted) => record
                    .issue_url
                    .as_deref()
                    .and_then(parse_issue_url)
                    .is_some_and(|found| same_repo_ref(&found, wanted)),
                None => records_issue_number(record.issue_number.as_deref(), number),
            };
            if !names_it {
                continue;
            }
            let Some(recorded) = record.run_dir.as_deref() else {
                self.reject(format!(
                    "a record names issue {number} but recorded no run folder"
                ));
                continue;
            };
            match accept_recorded(recorded, record.run_id.as_deref()) {
                Ok(dir) if !matched.contains(&dir) => matched.push(dir),
                Ok(_) => {}
                Err(reason) => self.reject(reason),
            }
        }
        let chosen = matched.first()?;
        let warning = (matched.len() > 1).then(|| {
            format!(
                "{} recorded runs name issue {number}; the newest, {}, was used",
                matched.len(),
                quoted(&chosen.display().to_string())
            )
        });
        Some(Destination::run(chosen, warning))
    }

    /// Rule 3: a detached session name. The session log is something the
    /// operator named, so a log that is a symbolic link, is not trusted or
    /// holds no pointer line is refused rather than passed over.
    fn session_destination(&mut self) -> Result<Option<Destination>, GuideError> {
        let target = self.target;
        if crate::commands::agent::validate::session_name(target).is_err() || target.contains('/') {
            return Ok(None);
        }
        let log_dir = match self.log_dir.clone() {
            Ok(dir) => dir,
            // The record scan reports the same cause; one note is enough.
            Err(_) => return Ok(None),
        };
        let log = log_dir.join(format!("{target}.log"));
        let Some(record) = guide_records::read_named(&log)? else {
            self.note(format!("session log {} (not found)", log.display()));
            return Ok(None);
        };
        if let Some(recorded) = record.run_dir.as_deref() {
            match accept_recorded(recorded, record.run_id.as_deref()) {
                Ok(dir) => return Ok(Some(Destination::run(&dir, None))),
                Err(reason) => self.reject(reason),
            }
        }
        // A record written before `run_dir` existed, or by a run with no
        // folder: the work root in the environment still resolves it.
        match record.run_id.as_deref() {
            Some(id) if validate_run_id(id) => match self.runs.clone() {
                Ok(runs) => match find_run(&runs, id)? {
                    Some(dir) => return Ok(Some(Destination::run(&dir, None))),
                    None => self.note(format!(
                        "session {} names run {}, which has no run folder under {}",
                        quoted(target),
                        quoted(id),
                        runs.display()
                    )),
                },
                Err(e) => self.note(format!(
                    "session {} names run {}, but the work root cannot be resolved: {e}",
                    quoted(target),
                    quoted(id)
                )),
            },
            Some(_) => self.note(format!(
                "the amplihack.recipe.log_pointer line of {} names an invalid run id",
                log.display()
            )),
            None => self.note(format!(
                "the amplihack.recipe.log_pointer line of {} names no run",
                log.display()
            )),
        }
        Ok(None)
    }

    /// Rule 4: a run folder name or a correlation id, through the record
    /// that carries it first and the work root in the environment second.
    fn run_destination(&mut self) -> Result<Option<Destination>, GuideError> {
        let target = self.target;
        if !validate_run_id(target) {
            return Ok(None);
        }
        for record in self.records() {
            let Some(recorded) = record.run_dir.as_deref() else {
                continue;
            };
            let names_it = record.run_id.as_deref() == Some(target)
                || Path::new(recorded).file_name() == Some(OsStr::new(target));
            if !names_it {
                continue;
            }
            match accept_recorded(recorded, record.run_id.as_deref()) {
                Ok(dir) => return Ok(Some(Destination::run(&dir, None))),
                Err(reason) => self.reject(reason),
            }
        }
        if let Ok(runs) = self.runs.clone() {
            match find_run(&runs, target)? {
                Some(dir) => return Ok(Some(Destination::run(&dir, None))),
                None => self.note(format!(
                    "{} (no run folder or run.json with that ID)",
                    runs.display()
                )),
            }
        }
        Ok(None)
    }

    /// One line naming the target, the forms tried and each place looked at.
    /// Nothing new is refused here: a miss simply says what was searched.
    fn not_found(self) -> GuideError {
        let target = quoted(self.target);
        // The target could have been a run, which was never looked for.
        let head = match &self.runs {
            Err(e) if validate_run_id(self.target) => {
                format!("cannot look for run {target}: {e}")
            }
            _ => format!("no recipe run or detached session named {target}"),
        };
        let mut message = head;
        if !self.notes.is_empty() {
            message.push_str(&format!("; looked at {}", self.notes.join(", ")));
        }
        if !self.rejected.is_empty() {
            let shown = self.rejected.len().min(MAX_SHOWN_REJECTIONS);
            message.push_str(&format!(
                "; not used: {}",
                self.rejected[..shown].join("; ")
            ));
            let left = self.rejected.len() - shown;
            if left > 0 {
                message.push_str(&format!(", and {left} more"));
            }
        }
        GuideError::NotFound(sanitize_line(&message))
    }
}

/// The guidance folder `target` names, and its kind.
fn resolve_target(target: &str) -> Result<Destination, GuideError> {
    let mut resolver = Resolver::new(target);
    // Rules 1 and 2. A target with `://` is only ever read as a GitHub URL,
    // so a mistyped one never reaches the filesystem as a run name.
    if target.contains("://") {
        if let Some(issue) = parse_issue_url(target) {
            // Never `state_root` or `ensure_private_chain` here: an issue URL
            // that matches nothing must create nothing.
            return match resolver.issue_destination(Some(&issue), issue.number) {
                Some(found) => Ok(found),
                None => Err(resolver.not_found()),
            };
        }
        let pr = parse_pr_url(target)?;
        let root = state_root().map_err(|reason| {
            GuideError::NotFound(format!("cannot find the state folder: {reason}"))
        })?;
        let key = pr_key(&pr.owner, &pr.repo, pr.number);
        ensure_private_chain(&root, &[AUTO_DRIVE_DIR, &key])?;
        return Ok(Destination {
            dir: pr_guidance_parent(&root, &key).join(GUIDANCE_DIR),
            source: Source::Pr,
            warning: None,
        });
    }
    if let Some(found) = resolver.session_destination()? {
        return Ok(found);
    }
    if let Some(found) = resolver.run_destination()? {
        return Ok(found);
    }
    if let Some(number) = bare_issue_number(target)
        && let Some(found) = resolver.issue_destination(None, number)
    {
        return Ok(found);
    }
    Err(resolver.not_found())
}

/// The run folder for `id`: a folder of that name under `runs`, or the
/// folder whose `run.json` lists `id` as an attempt's correlation id.
fn find_run(runs: &Path, id: &str) -> Result<Option<PathBuf>, GuideError> {
    if !validate_run_id(id) || !check_dir(runs)? {
        return Ok(None);
    }
    let direct = runs.join(id);
    if check_dir(&direct)? && direct.join(RUN_JSON).is_file() {
        return Ok(Some(direct));
    }
    let rd = fs::read_dir(runs)
        .map_err(|e| GuideError::Io(format!("cannot list {}: {e}", runs.display())))?;
    for item in rd.flatten() {
        let path = item.path();
        let Ok(kind) = item.file_type() else { continue };
        if !kind.is_dir() {
            continue;
        }
        if let Ok(report) = RunReport::read(&path)
            && report.attempts.iter().any(|a| a.correlation_id == id)
        {
            check_dir(&path)?;
            return Ok(Some(path));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    const CAP: usize = MAX_GUIDANCE_ENTRY_BYTES + HEADER_ROOM;

    #[test]
    fn text_within_the_cap_is_returned_untrimmed() {
        let mut src = io::Cursor::new(b"  keep the API\n".to_vec());
        assert_eq!(
            read_bounded(&mut src, "stdin", Overflow::Drain).unwrap(),
            "  keep the API\n"
        );
        let exact = "x".repeat(CAP);
        let mut src = io::Cursor::new(exact.clone().into_bytes());
        assert_eq!(
            read_bounded(&mut src, "stdin", Overflow::Stop).unwrap(),
            exact
        );
    }

    #[test]
    fn stop_mode_ends_an_endless_source_with_the_at_least_message() {
        // io::repeat never ends: only a read that stops at cap + 1 returns.
        let e = read_bounded(&mut io::repeat(b'z'), "/dev/zero", Overflow::Stop).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(
            e.to_string(),
            "guidance entry is more than 51200 bytes; the limit is 51200 bytes"
        );
    }

    #[test]
    fn drain_mode_reports_the_exact_size_of_oversize_stdin() {
        let mut src = io::Cursor::new(vec![b'z'; MAX_GUIDANCE_ENTRY_BYTES * 2]);
        let e = read_bounded(&mut src, "stdin", Overflow::Drain).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(
            e.to_string(),
            "guidance entry is 102400 bytes; the limit is 51200 bytes"
        );
    }

    #[test]
    fn size_mode_reports_the_size_it_was_given_without_reading_the_rest() {
        let mut src = io::Cursor::new(vec![b'z'; CAP + 10]);
        let e = read_bounded(&mut src, "notes.md", Overflow::Size(204_800)).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(
            e.to_string(),
            "guidance entry is 204800 bytes; the limit is 51200 bytes"
        );
        assert_eq!(
            src.position(),
            (CAP + 1) as u64,
            "nothing past cap + 1 is read"
        );
    }

    #[test]
    fn size_mode_with_a_size_below_the_cap_reports_more_than() {
        let mut src = io::Cursor::new(vec![b'z'; CAP + 10]);
        let e = read_bounded(&mut src, "notes.md", Overflow::Size(0)).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(
            e.to_string(),
            "guidance entry is more than 51200 bytes; the limit is 51200 bytes"
        );
    }

    #[test]
    fn invalid_utf8_is_a_fixed_message_that_never_echoes_the_text() {
        let mut src = io::Cursor::new([b"MARKER-2567-utf8 ".as_slice(), b"\xff\xfe"].concat());
        let e = read_bounded(&mut src, "notes.md", Overflow::Stop).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(e.to_string(), "guidance text is not valid UTF-8");
    }

    #[test]
    fn a_read_error_is_exit_1_and_names_the_source() {
        struct Failing;
        impl io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        let e = read_bounded(&mut Failing, "/srv/notes.md", Overflow::Stop).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(
            e.to_string().starts_with("cannot read /srv/notes.md: "),
            "{e}"
        );
    }

    /// Issue #2783: nothing taken from a log reaches stderr as itself.
    #[test]
    fn quoting_escapes_every_character_that_could_rewrite_the_line() {
        assert_eq!(quoted("/w/runs/a-1"), "\"/w/runs/a-1\"");
        assert_eq!(
            quoted("a\u{202e}b\u{2028}c\u{1b}d\u{9b}e\u{2069}f"),
            "\"a\\u{202e}b\\u{2028}c\\u{001b}d\\u{009b}e\\u{2069}f\""
        );
        assert_eq!(quoted("say \"x\"\\y"), "\"say \\\"x\\\"\\\\y\"");
        // A replacement character is printable and is shown as itself; it is
        // `accept_recorded` that refuses a path holding one.
        assert_eq!(quoted("a\u{fffd}b"), "\"a\u{fffd}b\"");
    }

    #[test]
    fn a_message_is_one_line_however_it_was_built() {
        let built = "refusing /w/a\nb\r\u{2029}c: it is a symbolic link";
        let shown = sanitize_line(built);
        assert!(!shown.contains('\n') && !shown.contains('\r'), "{shown}");
        assert!(!shown.contains('\u{2029}'), "{shown}");
        assert!(
            shown.contains("\\u{000a}") && shown.contains("\\u{2029}"),
            "{shown}"
        );
    }

    #[test]
    fn a_bare_issue_number_is_ascii_digits_with_an_optional_hash() {
        assert_eq!(bare_issue_number("2783"), Some(2783));
        assert_eq!(bare_issue_number("#2783"), Some(2783));
        for value in ["+2783", "27 83", "", "#", "2783x", "٢٧٨٣", "-1"] {
            assert_eq!(bare_issue_number(value), None, "{value:?}");
        }
    }

    #[test]
    fn a_recorded_issue_number_matches_only_as_digits() {
        assert!(records_issue_number(Some("2783"), 2783));
        assert!(records_issue_number(Some("02783"), 2783));
        for value in ["+2783", " 2783", "2783 ", "", "2784"] {
            assert!(!records_issue_number(Some(value), 2783), "{value:?}");
        }
        assert!(!records_issue_number(None, 2783));
    }
}
