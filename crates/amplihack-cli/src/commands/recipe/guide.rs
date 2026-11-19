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
//! the work root. `guide_records` reads those lines back. A run that retries
//! writes one line per attempt, each with a correlation id of its own and
//! all naming the one folder, so the folder is what identifies a run across
//! its attempts and `run.json` there is what lists their ids (issue #2818).
//!
//! The target is resolved in this order; the first match wins:
//!
//! 1. a GitHub issue URL: the recorded run that names that issue. This rule
//!    never creates the state folder;
//! 2. a pull request URL (any other target with `://` gets this rule's
//!    refusal): the per-PR state folder, created when absent;
//! 3. a detached session name: the pointer line of
//!    `<recipe log dir>/<session>.log`, through its recorded `run_dir` first
//!    and the work root in the environment second. The name is bounded by
//!    `detach::MAX_DETACHED_SESSION_CHARS`, the longest name
//!    `recipe run --detach` can create, not by the shorter rule for a name
//!    tmux is asked to create, and a name this rule cannot read is named in
//!    the not-found line rather than passed over in silence (issue #2818);
//! 4. a run folder name or correlation id: the recorded `run_dir` of the
//!    pointer line carrying it; then the `run.json` of each recorded folder,
//!    whose `attempts[]` holds the correlation id of every attempt of that
//!    run and not only the first one a log records (issue #2818); then
//!    `<work root>/runs/` as a folder name and then as
//!    `attempts[].correlation_id` there;
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
//!
//! Every line printed, on either stream and on success as well as failure,
//! goes through `sanitize_line` first. The destination no longer comes only
//! from the caller's own environment, so a path printed back can carry a name
//! chosen by whoever wrote the log, and a success line is as much a terminal
//! sequence's way in as a failure line (issue #2818, CWE-150).

use super::detach::MAX_DETACHED_SESSION_CHARS;
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
use std::collections::{HashMap, HashSet};
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
    // Success goes through `sanitize_line` as failure does. The destination
    // is no longer only the caller's own environment: a log's `run_dir` can
    // steer it (issue #2783), and the threat model of that lookup is a
    // same-uid process planting a log. `accept_recorded` passes over a
    // recorded folder whose path holds one of these characters, so a log
    // cannot reach this line; the escaping here also covers a work root or a
    // state folder the operator named (issue #2818).
    println!(
        "{}",
        sanitize_line(&format!(
            "{verb}: {} ({} bytes, {})",
            appended.path.display(),
            appended.bytes,
            source.label()
        ))
    );
    if source == Source::Pr {
        println!(
            "{}",
            sanitize_line(&format!(
                "written to {}; it is read at the next agent step of each run started with this pull request in its context (pr_url or pr_number), such as a later auto-drive round. A run that opened this pull request itself does not read it; address that run by its run ID or session name.",
                dir.display()
            ))
        );
    }
    // After the append, so a failed append still prints exactly one line.
    if let Some(warning) = warning {
        eprintln!("recipe guide: warning: {}", sanitize_line(&warning));
    }
    if let Some(error) = appended.log_error {
        // The message holds the `guidance.log` path, so it carries whatever
        // the destination's ancestors are named.
        eprintln!(
            "recipe guide: warning: the entry was added but guidance.log was not: {}",
            sanitize_line(&error)
        );
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

/// The last pass over a line before it is printed, on stdout as well as
/// stderr. Paths reach it from `check_dir`, the work root validator and
/// `run.json`, which build their own text, so the escaping is applied once
/// here to all of it.
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

/// Whether `name` can be the stem of a detached session's log file.
///
/// Accepts every name `recipe run --detach` can create, and refuses only
/// what would make `<log dir>/<name>.log` name something else or carry an
/// unprintable character into a message.
///
/// The length bound is [`MAX_DETACHED_SESSION_CHARS`], imported from the
/// module that creates the names rather than restated here, so one
/// identifier has one bound. It is deliberately not the 64 of
/// [`crate::commands::agent::validate::session_name`]: that rule bounds a
/// name tmux is about to be asked to create, this one a name tmux already
/// created. Holding the lookup to the creation rule skipped every session
/// detach named past 64 characters -- among them the auto-drive sessions,
/// whose recipe, repository and timestamp run past it together -- and
/// skipped it in silence, so the miss looked like an absent log (#2818).
///
/// The returned reason goes in the brackets of the note
/// `no session log (<reason>)`, so it is phrased to sit beside the other
/// places the not-found line lists.
fn lookup_session_name(name: &str) -> Result<(), String> {
    let n = name.chars().count();
    if n == 0 {
        return Err("the name is empty".to_string());
    }
    if n > MAX_DETACHED_SESSION_CHARS {
        return Err(format!(
            "the name is {n} characters; a detached session name is at most {MAX_DETACHED_SESSION_CHARS}"
        ));
    }
    if name.chars().any(needs_escape) {
        return Err("the name has a control character".to_string());
    }
    // `/` first, and on its own: it is the one that would leave the log
    // directory, and it is what a path mistyped as a target carries.
    if name.contains('/') {
        return Err("the name has a \"/\"".to_string());
    }
    // tmux rewrites `:` and `.` and expands `#`, so a live session was
    // never named with one of them whatever the log directory holds.
    if let Some(c) = name.chars().find(|c| matches!(c, ':' | '.' | '#')) {
        return Err(format!("the name has a {}", quoted(&c.to_string())));
    }
    Ok(())
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

/// Why a folder a record named was not used, and how far the checks got.
///
/// The distinction exists because [`Resolver::recorded_attempt_destination`]
/// sweeps every recorded folder looking for one id. A sound folder that
/// belongs to another run is the expected outcome of such a sweep and would
/// bury the line in noise; a folder that could not be used at all is a
/// fault worth naming wherever it is found. Both are notes, never refusals
/// (issue #2809).
#[derive(Debug)]
enum NotUsed {
    /// The folder cannot be written to: it is not shaped like a run folder,
    /// it is gone, its work root is unusable, an `lstat` refused it, or its
    /// `run.json` is absent, unreadable or names a different run. Its
    /// `attempts[]` was never consulted. Reported with its reason by every
    /// caller.
    Unusable(String),
    /// `run.json` was read and is sound; its `attempts[]` does not list the
    /// wanted id, so the folder belongs to another run.
    NotListed(String),
}

impl NotUsed {
    /// The one-line reason, for the not-found line.
    fn reason(self) -> String {
        match self {
            Self::Unusable(reason) | Self::NotListed(reason) => reason,
        }
    }
}

/// The checks that depend only on a recorded folder's work root, remembered
/// across one resolution.
///
/// [`work_root::validate`] canonicalises three paths and the two `lstat`
/// calls above the run folder repeat per folder, yet every folder under one
/// work root gets the same answer. A sweep over the folders of a log
/// directory is almost always a sweep over one or two work roots, so this
/// turns an O(folders) cost into an O(work roots) one (issue #2818).
///
/// It loosens no check. The answer is remembered for one `recipe guide`
/// process, which lives for milliseconds, and re-running an `lstat` inside
/// it buys nothing: a link swapped in between two checks of the same path
/// defeats both. What stands between a swapped link and the entry is the
/// check on the destination at write time, which is unchanged.
#[derive(Default)]
struct RootChecks(HashMap<PathBuf, Result<(), RootProblem>>);

/// What is wrong with a recorded folder's work root, and how to say it.
#[derive(Debug, Clone)]
enum RootProblem {
    /// A fragment that completes "the recorded run folder <path> ...".
    About(String),
    /// A message that already names what it is about.
    Whole(String),
}

impl RootProblem {
    fn message(self, shown: &str) -> String {
        match self {
            Self::About(tail) => format!("the recorded run folder {shown} {tail}"),
            Self::Whole(whole) => whole,
        }
    }
}

impl RootChecks {
    /// The work root and `runs` folder of `runs` are usable. Computed once
    /// per distinct `runs` folder.
    fn check(&mut self, runs: &Path, root: &Path) -> Result<(), RootProblem> {
        if let Some(seen) = self.0.get(runs) {
            return seen.clone();
        }
        let answer = Self::compute(runs, root);
        self.0.insert(runs.to_path_buf(), answer.clone());
        answer
    }

    fn compute(runs: &Path, root: &Path) -> Result<(), RootProblem> {
        // An unset HOME skips the comparison with the home folder rather
        // than refusing; the not-found line still names what could not be
        // resolved.
        let home = crate::user_config::home_dir().unwrap_or_default();
        if let Err(e) = work_root::validate(root, &home) {
            return Err(RootProblem::About(format!(
                "has a work root that is not usable: {}",
                one_line(&e)
            )));
        }
        // `lstat` at each level from the work root down, so a link swapped
        // in above the run folder is seen rather than followed.
        for level in [root, runs] {
            match check_dir(level) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(RootProblem::About(format!(
                        "is gone: {} does not exist",
                        quoted(&level.display().to_string())
                    )));
                }
                Err(e) => return Err(RootProblem::Whole(e.to_string())),
            }
        }
        Ok(())
    }
}

/// Whether a folder a record named may be written to.
///
/// Every reason it returns is a note, never a refusal: a record is a claim,
/// and a claim that does not hold must not stop a later rule from matching
/// (issue #2809). `run_id` is the correlation id the record carried, which
/// the folder's own `run.json` must agree with, so a log planted beside the
/// real ones cannot point the entry at somebody else's run.
///
/// `roots` memoises the checks above the run folder; pass a fresh one when
/// there is only a single folder to check.
fn accept_recorded(
    recorded: &str,
    run_id: Option<&str>,
    roots: &mut RootChecks,
) -> Result<PathBuf, NotUsed> {
    let shown = quoted(recorded);
    // Everything but the last check leaves the folder unusable: its
    // `attempts[]` was never consulted, so nothing was learned about which
    // ids it holds.
    let unusable = |reason: String| Err(NotUsed::Unusable(reason));
    // A lossy conversion stands for bytes that are not UTF-8, so the string
    // cannot name the folder it came from.
    if recorded.contains('\u{fffd}') {
        return unusable(format!(
            "the recorded run folder {shown} holds U+FFFD and cannot name a real folder"
        ));
    }
    // The whole string, so an ancestor is checked as well as the run folder
    // itself. The later checks look only at the last two components and the
    // work root, none of which reads the characters of a name, so a folder
    // such as `/x/<ESC>]0;title<BEL>/runs/<uuid>` would otherwise be accepted
    // and then printed on the success path, where the operator never typed it
    // (issue #2818, CWE-150). This is a note, not a refusal: the record is a
    // claim, and a later rule still gets its turn (issue #2809). No run
    // folder `recipe run` creates holds one of these characters.
    if let Some(c) = recorded.chars().find(|c| needs_escape(*c)) {
        return unusable(format!(
            "the recorded run folder {shown} has {} in its path",
            escaped(c)
        ));
    }
    // The raw string, not `Path::components`, which drops empty and `.`
    // segments before they can be seen.
    let Some(rest) = recorded.strip_prefix('/') else {
        return unusable(format!(
            "the recorded run folder {shown} is not an absolute path"
        ));
    };
    if rest.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return unusable(format!(
            "the recorded run folder {shown} has an empty, `.` or `..` segment"
        ));
    }
    let dir = PathBuf::from(recorded);
    let name = dir.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if !validate_run_id(name) {
        return unusable(format!(
            "the recorded run folder {shown} is not named like a run"
        ));
    }
    let Some(runs) = dir.parent() else {
        return unusable(format!("the recorded run folder {shown} has no parent"));
    };
    if runs.file_name() != Some(OsStr::new("runs")) {
        return unusable(format!(
            "the recorded run folder {shown} is not inside a `runs` folder"
        ));
    }
    let Some(root) = runs.parent() else {
        return unusable(format!("the recorded run folder {shown} has no work root"));
    };
    // The work root and the `runs` folder above it, checked once per
    // distinct `runs` folder however many recorded folders share it.
    if let Err(problem) = roots.check(runs, root) {
        return unusable(problem.message(&shown));
    }
    // `lstat` on the run folder itself, so a link swapped in for it is seen
    // rather than followed.
    match check_dir(&dir) {
        Ok(true) => {}
        Ok(false) => {
            return unusable(format!("the recorded run folder {shown} no longer exists"));
        }
        Err(e) => return unusable(e.to_string()),
    }
    let report_path = dir.join(RUN_JSON);
    match fs::symlink_metadata(&report_path) {
        Ok(meta) => {
            if let Some(reason) = untrusted_reason(&meta, false) {
                return unusable(format!("refusing {}: {reason}", report_path.display()));
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return unusable(format!("the recorded run folder {shown} has no {RUN_JSON}"));
        }
        Err(e) => return unusable(format!("cannot check {}: {e}", report_path.display())),
    }
    let report = RunReport::read(&dir).map_err(|e| {
        NotUsed::Unusable(format!(
            "cannot read {}: {}",
            report_path.display(),
            one_line(&e)
        ))
    })?;
    if report.run_id != name {
        return unusable(format!(
            "{} names run {}, not {}",
            report_path.display(),
            quoted(&report.run_id),
            quoted(name)
        ));
    }
    // The one outcome that says the folder is sound and simply belongs to
    // another run: `run.json` was read and does not list this id.
    if let Some(id) = run_id
        && id != name
        && !report.attempts.iter().any(|a| a.correlation_id == id)
    {
        return Err(NotUsed::NotListed(format!(
            "{} does not record run {}",
            report_path.display(),
            quoted(id)
        )));
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
    /// The reasons already in `rejected`. A folder can be reached by more
    /// than one rule -- rule 4's two stages both check the folder a record
    /// names -- and every reason names the folder it is about, so repeating
    /// one would say the same thing twice and push a different folder's
    /// reason past [`MAX_SHOWN_REJECTIONS`].
    seen_rejections: HashSet<String>,
    /// The work-root checks, shared by every rule, so a sweep over the
    /// folders of one work root pays them once.
    roots: RootChecks,
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
            seen_rejections: HashSet::new(),
            roots: RootChecks::default(),
        }
    }

    /// A place that was looked at and what it held.
    fn note(&mut self, note: String) {
        self.notes.push(sanitize_line(&note));
    }

    /// A folder a record named that was not used, and why. A reason already
    /// recorded is dropped: it names the same folder and the same fault.
    fn reject(&mut self, reason: String) {
        let line = sanitize_line(&reason);
        if self.seen_rejections.insert(line.clone()) {
            self.rejected.push(line);
        }
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
            match accept_recorded(recorded, record.run_id.as_deref(), &mut self.roots) {
                Ok(dir) if !matched.contains(&dir) => matched.push(dir),
                Ok(_) => {}
                Err(why) => self.reject(why.reason()),
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
        // A name this rule cannot read is a place that was not looked at, so
        // it says so (issue #2818). Silence here is what made the original
        // report look like an absent log rather than a skipped rule.
        if let Err(reason) = lookup_session_name(target) {
            self.note(format!("no session log ({reason})"));
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
            match accept_recorded(recorded, record.run_id.as_deref(), &mut self.roots) {
                Ok(dir) => return Ok(Some(Destination::run(&dir, None))),
                Err(why) => self.reject(why.reason()),
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
    /// that carries it first, the `run.json` of a recorded folder second
    /// (the id of an attempt after the first, which no record carries) and
    /// the work root in the environment third.
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
            match accept_recorded(recorded, record.run_id.as_deref(), &mut self.roots) {
                Ok(dir) => return Ok(Some(Destination::run(&dir, None))),
                Err(why) => self.reject(why.reason()),
            }
        }
        if let Some(found) = self.recorded_attempt_destination() {
            return Ok(Some(found));
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

    /// Rule 4, second stage: the recorded folder whose `run.json` lists the
    /// target among its attempts.
    ///
    /// A run has one folder but several correlation ids: each transient
    /// retry builds a fresh `RecipeRunCorrelation` and writes its own
    /// pointer line, all naming the one folder the run started with. A log
    /// is read only as far as its first pointer line, so the index holds the
    /// first attempt's id and no other, while the id `recipe stop` takes and
    /// the `AMPLIHACK_RECIPE_RUN_ID` the agent steps carry are the running
    /// attempt's (issue #2818). The folder is what the attempts share, so it
    /// is the folder that is indexed here: `run.json` lists every attempt's
    /// correlation id, and that list is what [`accept_recorded`] compares an
    /// id against.
    ///
    /// Only on a miss, and each distinct folder once. The checks above the
    /// run folder are shared through [`Resolver::roots`], so a sweep over
    /// one work root pays them once and each folder costs two `lstat` calls
    /// and one small `run.json`.
    ///
    /// What the note says is what was done. A folder whose `run.json` was
    /// read and belongs to another run is the ordinary outcome of a sweep,
    /// so those are counted rather than listed; a folder whose `attempts[]`
    /// was never consulted at all -- [`NotUsed::Unusable`] -- was not a read,
    /// and its reason goes to [`Resolver::reject`] exactly as the first
    /// stage's would, so `recipe guide` says why whichever id of a run is
    /// named (issue #2818). When the two counts differ the note gives both.
    fn recorded_attempt_destination(&mut self) -> Option<Destination> {
        let target = self.target;
        let mut tried: HashSet<String> = HashSet::new();
        let mut read = 0usize;
        for record in self.records() {
            let Some(recorded) = record.run_dir else {
                continue;
            };
            if !tried.insert(recorded.clone()) {
                continue;
            }
            match accept_recorded(&recorded, Some(target), &mut self.roots) {
                Ok(dir) => return Some(Destination::run(&dir, None)),
                // Read, and it names another run. Counted, not named.
                Err(NotUsed::NotListed(_)) => read += 1,
                Err(why) => self.reject(why.reason()),
            }
        }
        match tried.len() {
            0 => {}
            total if total == read => self.note(format!(
                "the attempts in {RUN_JSON} of {read} recorded run folder(s)"
            )),
            total => self.note(format!(
                "the attempts in {RUN_JSON} of {read} of {total} recorded run folder(s)"
            )),
        }
        None
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

    /// Issue #2818: the lookup must accept every name `recipe run --detach`
    /// can create. It takes detach's own bound, and this checks the bound
    /// against the names detach actually builds -- including the one from
    /// the report -- so a change at either end fails a test, not a drive.
    #[test]
    fn every_name_detach_can_produce_is_accepted_by_the_lookup() {
        use super::super::detach::{
            MAX_CLAIM_SUFFIX_CHARS, MAX_CLAIM_TRIES, nth_claim_name, session_name as detached,
        };

        // The reported case: the default pattern, an auto-drive recipe and
        // a per-PR worktree. 65 characters, so the 64 of the creation rule
        // refused it and the session rule was never tried.
        let reported = detached(
            "amp-{recipe}-{repo}-{ts}",
            "auto-drive-to-merge",
            "amplihack-rs-drive-pr2814",
            "20261119-000213",
        );
        assert_eq!(
            reported,
            "amp-auto-drive-to-merge-amplihack-rs-drive-pr2814-20261119-000213"
        );
        assert_eq!(reported.chars().count(), 65);
        assert!(lookup_session_name(&reported).is_ok(), "{reported}");
        assert!(
            crate::commands::agent::validate::session_name(&reported).is_err(),
            "the creation rule is what used to skip this name"
        );

        // The longest name detach can emit: a pattern at the truncation
        // bound, plus the longest suffix `claim_session` adds when the name
        // is already taken. That suffix is the part a bound of 80 alone
        // would still have cut off. Both come from detach's own builders,
        // never from a literal spelled out here.
        let truncated = detached("{repo}", "r", &"a".repeat(500), "t");
        let longest = nth_claim_name(&truncated, MAX_CLAIM_TRIES);
        assert_eq!(longest.chars().count(), MAX_DETACHED_SESSION_CHARS);
        assert_eq!(
            longest.chars().count() - truncated.chars().count(),
            MAX_CLAIM_SUFFIX_CHARS
        );
        assert!(lookup_session_name(&longest).is_ok(), "{longest}");

        // One past it is refused, and the reason names both numbers.
        let over = format!("{longest}x");
        let reason = lookup_session_name(&over).unwrap_err();
        assert!(
            reason.contains(&format!("{} characters", over.chars().count())),
            "{reason}"
        );
        assert!(
            reason.contains(&format!("at most {MAX_DETACHED_SESSION_CHARS}")),
            "{reason}"
        );
    }

    #[test]
    fn a_name_that_could_not_be_a_session_log_stem_says_why() {
        for (name, needle) in [
            ("", "empty"),
            ("../../etc/passwd", "\"/\""),
            ("amp:one", "\":\""),
            ("amp.one", "\".\""),
            ("amp#one", "\"#\""),
            ("amp\u{1b}one", "control character"),
        ] {
            let reason = lookup_session_name(name).unwrap_err();
            assert!(reason.contains(needle), "{name:?}: {reason}");
            assert!(!reason.contains('\u{1b}'), "{reason:?}");
        }
        assert!(lookup_session_name("amp-smart-orchestrator-myapp-1").is_ok());
    }

    /// Issue #2818: the checks that follow read only the last two path
    /// components and the work root, so a control character or a
    /// bidirectional mark in an ancestor passed them all and was then
    /// printed on the success path. It is stopped before any of that, at
    /// the string, and reported as a note (CWE-150).
    ///
    /// No filesystem is needed: the check runs before the first `lstat`, so
    /// each of these fails on its characters and not on the folder being
    /// absent -- the reason text is what says which.
    #[test]
    fn a_recorded_run_folder_with_a_control_character_in_an_ancestor_is_not_used() {
        let run = "3f1b7c2a-5d4e-4a90-8b61-7e2c9d0a4f18";
        for (path, shown) in [
            // The reported shape: a terminal title sequence in an ancestor.
            (format!("/x/\u{1b}]0;pwned\u{7}/runs/{run}"), "\\u{001b}"),
            // The run folder's own name, and the `runs` folder's.
            (format!("/x/runs/{run}\u{9b}"), "\\u{009b}"),
            (format!("/x/ru\u{85}ns/{run}"), "\\u{0085}"),
            // A bidirectional override, which reorders what is shown
            // without any control character at all.
            (format!("/x/\u{202e}gnp.eslaf/runs/{run}"), "\\u{202e}"),
            // A line separator, which would end the one line printed.
            (format!("/x/a\u{2028}b/runs/{run}"), "\\u{2028}"),
        ] {
            let reason = accept_recorded(&path, None, &mut RootChecks::default())
                .map(|d| d.display().to_string())
                .unwrap_err()
                .reason();
            assert!(reason.contains(shown), "{path:?}: {reason}");
            assert!(reason.contains("in its path"), "{path:?}: {reason}");
            // The reason itself is printable: no character it refused
            // survives into the line that reports it.
            assert!(
                !reason.chars().any(needs_escape),
                "the reason is not printable: {reason:?}"
            );
        }
    }

    /// The memo of the work-root checks answers for the second folder under
    /// a `runs` directory exactly as it did for the first, refusal included.
    /// A sweep over a log directory is a sweep over a handful of work roots,
    /// and this is what lets it cost a handful of `work_root::validate`
    /// calls rather than one per folder (issue #2818).
    #[test]
    fn the_work_root_memo_answers_the_same_for_every_folder_under_one_root() {
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        let mut roots = RootChecks::default();
        let mut reasons = Vec::new();
        for run in [
            "3f1b7c2a-5d4e-4a90-8b61-7e2c9d0a4f18",
            "c3e2a1b0-9f8e-4d7c-8b6a-5f4e3d2c1b0a",
        ] {
            let recorded = runs.join(run).display().to_string();
            // Both fail, and both fail the same way: the folder is absent
            // under a work root that is itself fine.
            reasons.push(
                accept_recorded(&recorded, None, &mut roots)
                    .map(|d| d.display().to_string())
                    .unwrap_err()
                    .reason(),
            );
        }
        for reason in &reasons {
            assert!(reason.contains("no longer exists"), "{reason}");
        }
        // One entry, whatever the number of folders under it.
        assert_eq!(roots.0.len(), 1, "one memo entry per `runs` folder");
    }

    /// A work root that cannot be used is refused for every folder under it,
    /// from the memo as well as on the first call.
    #[test]
    fn a_refused_work_root_is_refused_again_from_the_memo() {
        let run = "3f1b7c2a-5d4e-4a90-8b61-7e2c9d0a4f18";
        let mut roots = RootChecks::default();
        // `/` is refused as a work root, so `/runs/<id>` has nowhere to live.
        for _ in 0..2 {
            let reason = accept_recorded(&format!("/runs/{run}"), None, &mut roots)
                .map(|d| d.display().to_string())
                .unwrap_err()
                .reason();
            assert!(
                reason.contains("work root") || reason.contains("is gone"),
                "{reason}"
            );
        }
        assert_eq!(roots.0.len(), 1);
    }
}
