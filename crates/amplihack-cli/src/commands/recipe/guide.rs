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
//! Two kinds of input, treated differently (issue #2809), and one word for
//! each failure, used here, in the function docs below and on the reference
//! page.
//!
//! What the operator named -- the target, the session log it points at, the
//! `--file` path and the destination folder once it is chosen -- is
//! **refused**: the command prints why and stops, having written nothing.
//!
//! What a record claimed -- a `run_dir`, a session or an issue read out of a
//! pointer line or a `run.json` -- is **not used**: the reason becomes a
//! line in the not-found message, under `not used:`, and the next rule still
//! gets its turn. A record is a claim, so a claim that does not hold must
//! not be written to; not using one ends nothing, because the search goes
//! on and the command exits only if every rule misses. Both words name the
//! same care taken with the same check -- they differ in what happens next,
//! which is why a failed claim is never called a refusal here.
//!
//! A claim that named the target and was not used is reported even when the
//! search goes on to succeed: one `warning:` line on stderr names it and
//! why. The issue rules take the newest of every record that matches, so
//! the claim left out can be a *newer* run than the one that was written
//! to; without the line the command would exit 0, print the older folder on
//! stdout, and leave the operator guiding a run that is over while the live
//! one's folder sat there failing a check (issue #2818). The folders rule
//! 4's `run.json` sweep passes over are not reported on success: they never
//! named the target, so they say nothing about where the entry went.
//!
//! Inside a recipe run (`AMPLIHACK_RECIPE_RUN_ID` set) the command is refused
//! unless `--from-step` is passed, so an agent step misled by text it read
//! does not add guidance by accident. Entries it adds with the flag carry
//! ` run=<id>` in their caller field and are shown under their own preamble.
//!
//! After the entry is stored and logged, every waiter armed beside its folder
//! (`recipe guidance wait`, issue #3129) is woken through
//! `run_guidance::wake`. Something unexpected there is one `warning:` line
//! on stderr; the entry is stored and the exit code stays 0.
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
use super::live_runs::{live_runs_for_pr, run_end};
use crate::command_error::exit_error;
use crate::commands::cleanup::run_dir::{RUN_JSON, RunReport};
use crate::commands::cleanup::work_root;
use crate::commands::recipe::run_id::validate_run_id;
use crate::run_guidance::fs_safety::{check_dir, ensure_private_chain, untrusted_reason};
use crate::run_guidance::pr_key::{
    PrRef, parse_issue_url, parse_pr_url, pr_guidance_parent, pr_key, same_repo_ref, state_root,
};
// `sanitize_line` is the one escaping pass, shared with `recipe guidance`
// and the launcher's warnings. Paths reach it from `check_dir`, the work
// root validator and `run.json`, which build their own text, so it is
// applied once, to the whole line, just before printing.
use crate::run_guidance::log_state::{read_log_state, render_drop_line};
use crate::run_guidance::sanitize::{escaped, needs_escape, sanitize_line};
use crate::run_guidance::{
    AUTO_DRIVE_DIR, GUIDANCE_DIR, GuideError, HEADER_ROOM, MAX_GUIDANCE_ENTRY_BYTES, Source,
    append_entry_at, caller_identity, check_text,
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
        live_runs,
        no_live_run,
    } = resolve_target(target)?;
    // PR #3190 review: a run named by session or id may already have ended;
    // the sender is told what its `run.json` records rather than left to
    // wait for a step that will not start.
    let ended = match source {
        Source::Run if live_runs.is_empty() => run_end(&dir),
        _ => None,
    };
    // Issue #3161: each live run's own folder first, then the destination.
    // One time for every copy, so a reader of several of them sees one entry.
    let mut folders: Vec<(PathBuf, Source)> =
        live_runs.into_iter().map(|d| (d, Source::Run)).collect();
    folders.push((dir, source));
    let now = chrono::Utc::now();
    let caller = caller_identity();
    let total = folders.len();
    let mut appended_all = Vec::with_capacity(total);
    for (written, (folder, source)) in folders.iter().enumerate() {
        let appended = match append_entry_at(folder, &text, &caller, now) {
            Ok(appended) => appended,
            Err(error) => {
                if written > 0 {
                    // Not rolled back: a run may already have read an entry,
                    // and removing it would misreport what happened.
                    eprintln!(
                        "recipe guide: {written} of {total} folders were written before the failure"
                    );
                }
                return Err(error);
            }
        };
        // PR #2567 M1: a PR folder is read only by runs started with the pull
        // request in their context, so a PR entry is not reported as
        // "appended" to anything that will read it.
        let verb = match source {
            Source::Pr => "guidance saved for pull request",
            _ => "guidance appended",
        };
        // Success goes through `sanitize_line` as failure does. The
        // destination is no longer only the caller's own environment: a
        // log's `run_dir` can steer it (issue #2783), and the threat model of
        // that lookup is a same-uid process planting a log. `accept_recorded`
        // passes over a recorded folder whose path holds one of these
        // characters, so a log cannot reach this line; the escaping here also
        // covers a work root or a state folder the operator named (#2818).
        println!(
            "{}",
            sanitize_line(&format!(
                "{verb}: {} ({} bytes, {})",
                appended.path.display(),
                appended.bytes,
                source.label()
            ))
        );
        appended_all.push(appended);
    }
    // Issue #3162: the sender is told the truth. No step has read an entry
    // that was just written; a step that reads it records `entry_names`,
    // which `recipe guidance show` reports as `read`. "Read" means the block
    // was written in full to a step's input, not that the agent acted on it.
    // With no detached run found, say only what was checked (PR #3190
    // review): a run started in the foreground writes no start record to
    // the log folder, and it reads the pull request folder at its next
    // agent step when its state folder is the sender's.
    let status = if no_live_run {
        "appended; not yet read; no detached run for this pull request was found, and a run with the pull request in its context reads the pull request folder at its next agent step".to_string()
    } else if let Some(end) = ended {
        format!(
            "appended; not yet read; the run ended at {} ({})",
            end.at, end.outcome
        )
    } else {
        "appended; not yet read".to_string()
    };
    println!("{}", sanitize_line(&format!("status: {status}")));
    // Launches since the last read that could not take the guidance, in
    // each folder written. One fan-out launch logs one drop in every folder
    // it read, so a line is printed once.
    let mut shown: Vec<String> = Vec::new();
    for (folder, _) in &folders {
        let state = read_log_state(folder);
        if let Some(reason) = &state.log_error {
            // An unreadable log says nothing about drops; say that rather
            // than print nothing, which would read as "none".
            println!(
                "{}",
                sanitize_line(&format!(
                    "drop state unknown for {}: guidance.log cannot be read: {reason}",
                    folder.display()
                ))
            );
        }
        for drop in state.drops_since_injection {
            let line = render_drop_line(&drop);
            if !shown.contains(&line) {
                println!("{line}");
                shown.push(line);
            }
        }
    }
    // After the append, so a failed append still prints exactly one line.
    if let Some(warning) = warning {
        eprintln!("recipe guide: warning: {}", sanitize_line(&warning));
    }
    for appended in appended_all {
        if let Some(error) = appended.log_error {
            // The message holds the `guidance.log` path, so it carries
            // whatever the destination's ancestors are named.
            eprintln!(
                "recipe guide: warning: the entry was added but guidance.log was not: {}",
                sanitize_line(&error)
            );
        }
        // Issue #3129: the entry is stored whether or not a waiter was
        // woken, so this is a warning and the exit code stays 0.
        if let Some(warning) = appended.wake_warning {
            eprintln!("recipe guide: warning: {}", sanitize_line(&warning));
        }
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
    /// Printed on stderr after the append, on one line. It carries what a
    /// rule raised -- more than one recorded run matched an issue and the
    /// newest was taken -- and what [`Resolver::with_passed_over`] adds:
    /// the records that named this target and were not used.
    warning: Option<String>,
    /// Issue #3161: the guidance folders of the live runs for a pull
    /// request, written before [`Self::dir`] with the same entry.
    live_runs: Vec<PathBuf>,
    /// A pull request target for which no live run was found.
    no_live_run: bool,
}

impl Destination {
    fn run(run_dir: &Path, warning: Option<String>) -> Self {
        Self {
            dir: run_dir.join(GUIDANCE_DIR),
            source: Source::Run,
            warning,
            live_runs: Vec::new(),
            no_live_run: false,
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

/// Check a recorded path against the one trust rule, without following
/// links: not a symbolic link, the expected type, owned by this process's
/// effective user and not writable by group or others.
///
/// Every failure means the path is not used, the mode included. The folder
/// is accepted here and written to later, so a folder another user can
/// write to can be renamed aside and replaced in between, which would land
/// the entry in a folder that user controls (CWE-367). `append_entry`
/// rechecks only the run folder and the guidance folder, never `runs` or
/// the work root, so nothing downstream makes up for accepting a writable
/// ancestor. Not using a path costs nothing: the reason goes into the
/// not-found line, under `not used:`, and the next rule still gets its turn
/// (issue #2809). Every caller is a recorded claim, so nothing here is a
/// refusal in the module doc's sense -- the command does not stop.
fn check_recorded_path(path: &Path, want_dir: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            format!("{} no longer exists", path.display())
        } else {
            format!("cannot check {}: {e}", path.display())
        }
    })?;
    match untrusted_reason(&meta, want_dir) {
        Some(reason) => Err(format!("{}: {reason}", path.display())),
        None => Ok(()),
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
/// The answer is remembered for one `recipe guide`
/// process, which lives for milliseconds, and re-running an `lstat` inside
/// it buys nothing: a link swapped in between two checks of the same path
/// defeats both. What stands between a swapped link and the entry is the
/// check on the destination at write time, which is unchanged.
#[derive(Default)]
pub(super) struct RootChecks(HashMap<PathBuf, Result<(), String>>);

impl RootChecks {
    /// The work root and `runs` folder of `runs` are usable. Computed once
    /// per distinct `runs` folder.
    fn check(&mut self, runs: &Path, root: &Path) -> Result<(), String> {
        if let Some(seen) = self.0.get(runs) {
            return seen.clone();
        }
        let answer = Self::compute(runs, root);
        self.0.insert(runs.to_path_buf(), answer.clone());
        answer
    }

    fn compute(runs: &Path, root: &Path) -> Result<(), String> {
        // An unset HOME skips the comparison with the home folder rather
        // than making the folder unusable; the not-found line still names
        // what could not be resolved.
        let home = crate::user_config::home_dir().unwrap_or_default();
        if let Err(e) = work_root::validate(root, &home) {
            return Err(format!(
                "has a work root that is not usable: {}",
                one_line(&e)
            ));
        }
        // `lstat` at each level above the run folder, so a link swapped in
        // there is seen rather than followed, and a level another user can
        // write to is not used rather than written under.
        for level in [root, runs] {
            check_recorded_path(level, true)
                .map_err(|reason| format!("cannot be used: {reason}"))?;
        }
        Ok(())
    }
}

/// Whether a folder a record named may be written to.
///
/// Every reason it returns means the folder is not used: a record is a
/// claim, and a claim that does not hold must not be written to. That is
/// not the end of the search -- the reason becomes a line in the not-found
/// message, under `not used:`, and a later rule still gets its turn
/// (issue #2809). Nothing it returns stops the command on its own.
///
/// `run_id` is the correlation id the record carried, which the folder's own
/// `run.json` must agree with, so a log planted beside the real ones cannot
/// point the entry at somebody else's run.
///
/// The folder's `run.json` is returned with it, so rule 4's second stage can
/// match the target against the attempts the report lists without asking
/// this function to conflate "the record is trustworthy" with "this folder
/// holds the target" (issue #2818).
pub(super) fn accept_recorded(
    recorded: &str,
    run_id: Option<&str>,
    roots: &mut RootChecks,
) -> Result<(PathBuf, RunReport), String> {
    let shown = quoted(recorded);
    // A lossy conversion stands for bytes that are not UTF-8, so the string
    // cannot name the folder it came from.
    if recorded.contains('\u{fffd}') {
        return Err(format!(
            "the recorded run folder {shown} holds U+FFFD and cannot name a real folder"
        ));
    }
    // The whole string, so an ancestor is checked as well as the run folder
    // itself. The later checks look only at the last two components and the
    // work root, none of which reads the characters of a name, so a folder
    // such as `/x/<ESC>]0;title<BEL>/runs/<uuid>` would otherwise be accepted
    // and then printed on the success path, where the operator never typed it
    // (issue #2818, CWE-150). No run folder `recipe run` creates holds one of
    // these characters.
    if let Some(c) = recorded.chars().find(|c| needs_escape(*c)) {
        return Err(format!(
            "the recorded run folder {shown} has {} in its path",
            escaped(c)
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
    // The work root and the `runs` folder above it, checked once per
    // distinct `runs` folder however many recorded folders share it.
    roots
        .check(runs, root)
        .map_err(|reason| format!("the recorded run folder {shown} {reason}"))?;
    // `lstat` on the run folder itself and on `run.json`, so a link swapped
    // in for either is seen rather than followed.
    check_recorded_path(&dir, true)?;
    let report_path = dir.join(RUN_JSON);
    check_recorded_path(&report_path, false)?;
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
    Ok((dir, report))
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
    /// The subset of `rejected` whose records named this target: a run the
    /// operator asked for by name, or a run that named the issue asked
    /// about. These are reported on stderr when a rule nevertheless
    /// succeeds, because the entry then went somewhere other than the run
    /// the record pointed at (issue #2818). A record passed over during the
    /// `run.json` sweep of rule 4 is not here: those folders did not name
    /// the target, so reporting them on a success would say nothing about
    /// where the entry went.
    passed_over: Vec<String>,
    /// The work-root checks, shared by every rule, so a sweep over the
    /// folders of one work root pays them once.
    roots: RootChecks,
}

/// The most reasons shown, in the not-found line and in the warning a
/// success prints; the rest are counted. One bound, so neither line grows
/// with the number of records a sweep met.
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
            passed_over: Vec::new(),
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

    /// [`Self::reject`] for a record that named this target: the issue the
    /// target asks about, or the run or session it names. The reason is
    /// kept a second time, in [`Self::passed_over`], so that a rule which
    /// goes on to succeed through some other record or folder still says on
    /// stderr which candidate it left out (issue #2818).
    fn reject_named(&mut self, reason: String) {
        let line = sanitize_line(&reason);
        if !self.passed_over.contains(&line) {
            self.passed_over.push(line);
        }
        self.reject(reason);
    }

    /// A chosen destination, with a warning naming the records that named
    /// this target and were passed over.
    ///
    /// Without this the silence would be wrong rather than merely quiet.
    /// The issue rules take the newest of every record that matches, so a
    /// record rejected here can be a *newer* run than the one chosen: the
    /// command would exit 0, print the older folder on stdout and say
    /// nothing about the live run whose folder had drifted to a mode, an
    /// owner or a `run.json` that failed a check. The operator would then
    /// be reading a run that cannot see the entry. The list is bounded like
    /// the not-found line's, and any warning the rule itself raised is kept.
    fn with_passed_over(&self, mut dest: Destination) -> Destination {
        if self.passed_over.is_empty() {
            return dest;
        }
        let shown = self.passed_over.len().min(MAX_SHOWN_REJECTIONS);
        let mut part = format!(
            "{} record(s) that name {} were not used: {}",
            self.passed_over.len(),
            quoted(self.target),
            self.passed_over[..shown].join("; ")
        );
        let left = self.passed_over.len() - shown;
        if left > 0 {
            part.push_str(&format!(", and {left} more"));
        }
        dest.warning = Some(match dest.warning.take() {
            Some(raised) => format!("{raised}; {part}"),
            None => part,
        });
        dest
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
    /// last written, newest first. A record that fails a check is passed
    /// over and the search goes on: its reason joins the not-found line,
    /// and, because the newest match wins, it also joins the warning this
    /// destination carries, so an operator who is given an older run is
    /// told which newer one was left out (issue #2818).
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
                self.reject_named(format!(
                    "a record names issue {number} but recorded no run folder"
                ));
                continue;
            };
            match accept_recorded(recorded, record.run_id.as_deref(), &mut self.roots) {
                Ok((dir, _)) if !matched.contains(&dir) => matched.push(dir),
                Ok(_) => {}
                Err(why) => self.reject_named(why),
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
                Ok((dir, _)) => return Ok(Some(Destination::run(&dir, None))),
                // The log the operator named recorded this folder, so a
                // fallback to the work root below is a different folder
                // from the one the session actually uses: say so.
                Err(why) => self.reject_named(why),
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
                Ok((dir, _)) => return Ok(Some(Destination::run(&dir, None))),
                // This record carried the target's own ID or folder name,
                // so a later stage that finds the run somewhere else found
                // a different folder: say which one was left out.
                Err(why) => self.reject_named(why),
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
    /// correlation id, and the target is matched against that list after
    /// [`accept_recorded`] has accepted the record on its own terms. The two
    /// questions stay apart, so a folder that was never opened is not
    /// counted as one whose `run.json` was read.
    ///
    /// Only on a miss, and each distinct folder once. The checks above the
    /// run folder are shared through [`Resolver::roots`], so a sweep over
    /// one work root pays them once and each folder costs two `lstat` calls
    /// and one small `run.json`.
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
            match accept_recorded(&recorded, record.run_id.as_deref(), &mut self.roots) {
                Ok((dir, report)) => {
                    if report.run_id == target
                        || report.attempts.iter().any(|a| a.correlation_id == target)
                    {
                        return Some(Destination::run(&dir, None));
                    }
                    read += 1;
                }
                Err(why) => self.reject(why),
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
///
/// One place turns the rules' answer into the result, so that every success
/// carries the warning about the records that named this target and were
/// passed over, and every miss carries the not-found line
/// ([`Resolver::with_passed_over`], issue #2818).
fn resolve_target(target: &str) -> Result<Destination, GuideError> {
    let mut resolver = Resolver::new(target);
    match first_match(&mut resolver, target)? {
        Some(found) => Ok(resolver.with_passed_over(found)),
        None => Err(resolver.not_found()),
    }
}

/// Issue #3162: the guidance folder of the run that `target` names as a
/// detached session, a run folder name or a correlation id (rules 3 and 4),
/// for `recipe guidance show|wait`. Reads and creates nothing.
pub(super) fn run_guidance_dir(target: &str) -> Result<PathBuf, GuideError> {
    let mut resolver = Resolver::new(target);
    let found = match resolver.session_destination()? {
        Some(found) => Some(found),
        None => resolver.run_destination()?,
    };
    match found {
        Some(found) => Ok(found.dir),
        None => Err(resolver.not_found()),
    }
}

/// The first rule that matches `target`, or `None` when every rule misses.
fn first_match(
    resolver: &mut Resolver<'_>,
    target: &str,
) -> Result<Option<Destination>, GuideError> {
    // Rules 1 and 2. A target with `://` is only ever read as a GitHub URL,
    // so a mistyped one never reaches the filesystem as a run name.
    if target.contains("://") {
        if let Some(issue) = parse_issue_url(target) {
            // Never `state_root` or `ensure_private_chain` here: an issue URL
            // that matches nothing must create nothing. The miss also ends
            // the search, because no later rule may read a URL as a run or
            // session name.
            return Ok(resolver.issue_destination(Some(&issue), issue.number));
        }
        let pr = parse_pr_url(target)?;
        // Issue #3161: every live run for the pull request reads its own
        // run folder, whatever its work root and state folder, so the entry
        // goes there as well. Found before anything is created: a live run
        // that cannot be reached is a refusal, and a refusal writes nothing.
        let live = live_runs_for_pr(&pr);
        let unreachable: Vec<String> = live
            .iter()
            .filter_map(|run| {
                run.reachable
                    .as_ref()
                    .err()
                    .map(|reason| format!("session {} ({reason})", quoted(&run.session)))
            })
            .collect();
        if !unreachable.is_empty() {
            return Err(GuideError::Unreachable(format!(
                "refusing to add guidance for {target}: {} live run(s) for this pull request cannot be reached: {}; nothing was written. Repair the run folder, or guide the run by its session name once it can be read",
                unreachable.len(),
                unreachable.join("; ")
            )));
        }
        let root = state_root().map_err(|reason| {
            GuideError::NotFound(format!("cannot find the state folder: {reason}"))
        })?;
        let key = pr_key(&pr.owner, &pr.repo, pr.number);
        ensure_private_chain(&root, &[AUTO_DRIVE_DIR, &key])?;
        let pr_dir = pr_guidance_parent(&root, &key).join(GUIDANCE_DIR);
        let mut live_runs: Vec<PathBuf> = Vec::new();
        for dir in live.into_iter().filter_map(|run| run.reachable.ok()) {
            if dir != pr_dir && !live_runs.contains(&dir) {
                live_runs.push(dir);
            }
        }
        return Ok(Some(Destination {
            dir: pr_dir,
            source: Source::Pr,
            warning: None,
            no_live_run: live_runs.is_empty(),
            live_runs,
        }));
    }
    if let Some(found) = resolver.session_destination()? {
        return Ok(Some(found));
    }
    if let Some(found) = resolver.run_destination()? {
        return Ok(Some(found));
    }
    if let Some(number) = bare_issue_number(target) {
        return Ok(resolver.issue_destination(None, number));
    }
    Ok(None)
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
        // `accept_recorded` that declines to use a path holding one.
        assert_eq!(quoted("a\u{fffd}b"), "\"a\u{fffd}b\"");
    }

    #[test]
    fn a_message_is_one_line_however_it_was_built() {
        let built = "/w/a\nb\r\u{2029}c: it is a symbolic link";
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
                .map(|(d, _)| d.display().to_string())
                .unwrap_err();
            assert!(reason.contains(shown), "{path:?}: {reason}");
            assert!(reason.contains("in its path"), "{path:?}: {reason}");
            // The reason itself is printable: no character it rejected
            // survives into the line that reports it.
            assert!(
                !reason.chars().any(needs_escape),
                "the reason is not printable: {reason:?}"
            );
        }
    }

    /// The memo of the work-root checks answers for the second folder under
    /// a `runs` directory exactly as it did for the first, a root that
    /// cannot be used included.
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
                    .map(|(d, _)| d.display().to_string())
                    .unwrap_err(),
            );
        }
        for reason in &reasons {
            assert!(reason.contains("no longer exists"), "{reason}");
        }
        // One entry, whatever the number of folders under it.
        assert_eq!(roots.0.len(), 1, "one memo entry per `runs` folder");
    }

    /// A work root that cannot be used keeps every folder under it from
    /// being used, from the memo as well as on the first call.
    #[test]
    fn an_unusable_work_root_is_unusable_again_from_the_memo() {
        let run = "3f1b7c2a-5d4e-4a90-8b61-7e2c9d0a4f18";
        let mut roots = RootChecks::default();
        // `/` is not a usable work root, so `/runs/<id>` has nowhere to live.
        for _ in 0..2 {
            let reason = accept_recorded(&format!("/runs/{run}"), None, &mut roots)
                .map(|(d, _)| d.display().to_string())
                .unwrap_err();
            assert!(
                reason.contains("work root") || reason.contains("is gone"),
                "{reason}"
            );
        }
        assert_eq!(roots.0.len(), 1);
    }
    /// A recorded path fails the one trust rule in every one of its four
    /// ways, and each one means the path is not used. The mode case is the
    /// one a writable ancestor reaches: the folder is accepted here and
    /// written to later, so accepting it would leave room for a rename in
    /// between (CWE-367, issue #2818).
    #[test]
    fn a_recorded_path_is_not_used_for_type_owner_and_mode() {
        use crate::run_guidance::fs_safety::{euid, with_euid_override};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run.json");
        fs::write(&file, "{}").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(check_recorded_path(dir.path(), true).is_ok());
        assert!(check_recorded_path(&file, false).is_ok());
        assert!(
            check_recorded_path(&file, true)
                .unwrap_err()
                .contains("not a folder")
        );
        assert!(
            check_recorded_path(dir.path(), false)
                .unwrap_err()
                .contains("not a regular file")
        );
        let other = euid().wrapping_add(1);
        with_euid_override(other, || {
            for (path, want_dir) in [(dir.path(), true), (file.as_path(), false)] {
                let reason = check_recorded_path(path, want_dir).unwrap_err();
                assert!(reason.contains("owned by uid"), "{reason}");
            }
        });
        // Group write and other write, on the folder and on the file.
        for (path, want_dir, mode) in [
            (dir.path(), true, 0o770),
            (dir.path(), true, 0o707),
            (file.as_path(), false, 0o660),
            (file.as_path(), false, 0o606),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            let reason = check_recorded_path(path, want_dir).unwrap_err();
            assert!(reason.contains("writable by group or others"), "{reason}");
            // The reason opens with the path it is about, so the
            // not-found line reads `not used: <path>: <reason>` rather
            // than calling a record's failed claim a refusal (issue #2809).
            assert!(
                reason.starts_with(&format!("{}: ", path.display())),
                "{reason}"
            );
        }
        // Nothing here changed the permissions it read.
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o606
        );
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
}
