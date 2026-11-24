//! Issue #3129: `amplihack recipe guidance show|wait`.
//!
//! `show` prints the guidance a run has received; `wait` blocks until more
//! arrives or a command finishes. Bash and agent steps of the auto-drive goal
//! loop call both, so both are allowed inside recipe runs and their output is
//! a contract: docs/reference/recipe-cli-reference.md#amplihack-recipe-guidance.
//!
//! Folders read ([`Folders`]): those `AMPLIHACK_RUN_GUIDANCE_DIRS` lists,
//! then the folder of the pull request named by `<TARGET>` (a pull request
//! URL) or by `--pr` with `--pr-repo`. `show` never creates anything. `wait`
//! arms a waiter beside every folder whose parent exists and passes the
//! trust rule ([`crate::run_guidance::wait`]), and for the pull request's
//! folder may create `<state root>/auto-drive/<key>/`, never the state root.
//!
//! Guidance text is printed only by plain `show`. It is never in JSON, argv
//! or a log. Every line printed, on either stream, goes through
//! `sanitize_line`, as `recipe guide`'s lines do.
//!
//! Exit codes: 0 done (for `wait`, guidance arrived or the command finished,
//! whatever the command's own status); 1 error; 2 usage; 3 `wait` could not
//! arm a waiter and did not start the command. An error or usage failure
//! prints one stderr line and nothing on stdout.

use crate::cli_subcommands::{GuidanceCommands, PrFolderArgs};
use crate::command_error::exit_error;
use crate::run_guidance::env::DirCheck;
use crate::run_guidance::folders::{Folder, Folders};
use crate::run_guidance::pr_key::{PrRef, parse_pr_url, parse_repo_slug};
use crate::run_guidance::sanitize::sanitize_line;
use crate::run_guidance::wait::{WaitOutcome, arm, wait_armed};
use crate::run_guidance::{
    Entry, GUIDANCE_DIRS_ENV, GuideError, Source, is_entry_name, is_new, render_block_marked,
};
use anyhow::Result;
use serde::Serialize;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;

/// The exit code of a `wait` that could not arm a waiter.
const UNARMED: i32 = 3;
/// The `schema_version` of `show --json`.
const SCHEMA_VERSION: u32 = 1;

/// Run `amplihack recipe guidance`.
pub fn run_guidance(command: GuidanceCommands) -> Result<()> {
    let result = match command {
        GuidanceCommands::Show {
            folder,
            since,
            operator_only,
            json,
        } => show(&folder, since.as_deref(), operator_only, json).map(|()| 0),
        GuidanceCommands::Wait {
            folder,
            since,
            command,
        } => wait(&folder, &since, &command),
    };
    match result {
        Ok(0) => Ok(()),
        Ok(code) => Err(exit_error(code)),
        Err(error) => {
            eprintln!("{}", sanitize_line(&format!("recipe guidance: {error}")));
            Err(exit_error(error.exit_code()))
        }
    }
}

/// `--pr`: `^[1-9][0-9]*$`, fitting in 64 bits.
fn pr_number(value: &str) -> Result<u64, GuideError> {
    let digits = !value.starts_with('0') && value.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| value.parse::<u64>().ok())
        .flatten()
        .ok_or_else(|| {
            GuideError::Usage(
                "--pr must be a pull request number: digits only, not starting with 0, at most 18446744073709551615"
                    .to_string(),
            )
        })
}

/// The pull request `args` name, if any, checked but not yet resolved.
fn named_pr(args: &PrFolderArgs) -> Result<Option<PrRef>, GuideError> {
    match (&args.target, &args.pr, &args.pr_repo) {
        (None, None, None) => Ok(None),
        (Some(url), None, None) => parse_pr_url(url).map(Some),
        (None, Some(number), Some(slug)) => {
            let number = pr_number(number)?;
            let (owner, repo) = parse_repo_slug(slug).ok_or_else(|| {
                GuideError::Usage(
                    "--pr-repo must be <owner>/<repo>: a GitHub account name and a repository name"
                        .to_string(),
                )
            })?;
            Ok(Some(PrRef {
                owner,
                repo,
                number,
            }))
        }
        (Some(_), _, _) => Err(GuideError::Usage(
            "name the pull request once: as a URL or with --pr and --pr-repo, not both".to_string(),
        )),
        _ => Err(GuideError::Usage(
            "--pr and --pr-repo must be given together".to_string(),
        )),
    }
}

/// `--since`: empty, or an entry file name.
fn check_since(since: &str) -> Result<(), GuideError> {
    if since.is_empty() || is_entry_name(since) {
        Ok(())
    } else {
        Err(GuideError::Usage(
            "--since must be an entry file name, such as 20261123T184002.000000000Z-4242-0.md, or empty"
                .to_string(),
        ))
    }
}

/// Prints each line of `text` on stdout, after `sanitize_line`.
fn print_lines(text: &str) -> Result<(), GuideError> {
    let mut out = std::io::stdout().lock();
    text.lines()
        .try_for_each(|line| writeln!(out, "{}", sanitize_line(line)))
        .and_then(|()| out.flush())
        .map_err(|e| GuideError::Io(format!("cannot write to stdout: {e}")))
}

/// Prints `value` as one JSON line.
fn print_json(value: &impl Serialize) -> Result<(), GuideError> {
    let line = serde_json::to_string(value)
        .map_err(|e| GuideError::Io(format!("cannot build the JSON line: {e}")))?;
    print_lines(&line)
}

/// A path as JSON text: lossy UTF-8, escaped as every printed line is, so
/// the JSON line stays valid and holds nothing a terminal would act on.
fn path_text(path: &Path) -> String {
    sanitize_line(&path.to_string_lossy())
}

#[derive(Debug, Serialize)]
struct ShowJson<'a> {
    schema_version: u32,
    since: &'a str,
    newest: &'a str,
    folders: Vec<FolderJson>,
    entries: Vec<EntryJson<'a>>,
}

#[derive(Debug, Serialize)]
struct FolderJson {
    path: String,
    source: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// One entry's metadata. There is deliberately no field for its text.
#[derive(Debug, Serialize)]
struct EntryJson<'a> {
    name: &'a str,
    folder: String,
    timestamp: String,
    caller: String,
    source: &'static str,
    from_step: bool,
    new: bool,
    bytes: usize,
}

/// The `show --json` document.
fn show_json<'a>(list: &[Folder], entries: &'a [Entry], since: &'a str) -> ShowJson<'a> {
    let folders = list
        .iter()
        .map(|f| {
            let (state, reason) = match &f.check {
                DirCheck::Trusted => ("present", None),
                DirCheck::Missing => ("missing", None),
                DirCheck::Untrusted(reason) => ("untrusted", Some(sanitize_line(reason))),
            };
            FolderJson {
                path: path_text(&f.path),
                source: Source::of(&f.path).label(),
                state,
                reason,
            }
        })
        .collect();
    let entries_json = entries
        .iter()
        .map(|e| EntryJson {
            name: &e.name,
            folder: path_text(&e.dir),
            timestamp: sanitize_line(&e.timestamp),
            caller: sanitize_line(&e.caller),
            source: e.source.label(),
            from_step: e.is_from_step(),
            new: is_new(&e.name, since),
            bytes: e.text.len(),
        })
        .collect();
    ShowJson {
        schema_version: SCHEMA_VERSION,
        since,
        newest: entries.last().map_or("", |e| e.name.as_str()),
        folders,
        entries: entries_json,
    }
}

/// `recipe guidance show`. With `operator_only`, entries an agent step added
/// are removed before either output is built, so neither can hold one. This
/// guards a run against widening its own allowed PR text through ordinary use;
/// it is not a trust boundary, since the `from_step` label is the CLI's own.
fn show(
    args: &PrFolderArgs,
    since: Option<&str>,
    operator_only: bool,
    json: bool,
) -> Result<(), GuideError> {
    let since = since.unwrap_or("");
    check_since(since)?;
    let folders = Folders::resolve(named_pr(args)?.as_ref())?;
    folders.warn_untrusted();
    let mut entries = folders.read();
    if operator_only {
        entries.retain(|e| !e.is_from_step());
    }
    if json {
        print_json(&show_json(&folders.list, &entries, since))
    } else {
        print_lines(&render_block_marked(&entries, Some(since)))
    }
}

/// One `wait` result line.
#[derive(Debug, Default, Serialize)]
struct Event<'a> {
    event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    entry: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<i32>,
}

fn guidance_event(entry: &str) -> Event<'_> {
    Event {
        event: "guidance",
        entry: Some(entry),
        ..Event::default()
    }
}

/// The line for a command that finished: its exit code, or `128 + N` and
/// the signal `N` that ended it. Its code is reported, never passed on as
/// this command's own status.
fn command_event(status: ExitStatus) -> Result<Event<'static>, GuideError> {
    let (exit, signal) = match (status.code(), status.signal()) {
        (Some(code), _) => (code, None),
        (None, Some(signal)) => (128 + signal, Some(signal)),
        (None, None) => {
            return Err(GuideError::Io(format!(
                "the command ended with a status that is neither an exit nor a signal: {status}"
            )));
        }
    };
    Ok(Event {
        event: "command",
        exit: Some(exit),
        signal,
        ..Event::default()
    })
}

/// `recipe guidance wait`. Returns the exit code after printing its line.
fn wait(args: &PrFolderArgs, since: &str, command: &[OsString]) -> Result<i32, GuideError> {
    check_since(since)?;
    let mut folders = Folders::resolve(named_pr(args)?.as_ref())?;
    folders.warn_untrusted();
    let (mut waiters, problems) = arm(&folders);
    if waiters.is_empty() {
        let why = if problems.is_empty() {
            format!(
                "no guidance folder to wait on: {GUIDANCE_DIRS_ENV} lists none and no pull request was named"
            )
        } else {
            problems.join("; ")
        };
        print_json(&Event {
            event: "unarmed",
            ..Event::default()
        })?;
        eprintln!(
            "{}",
            sanitize_line(&format!(
                "recipe guidance: cannot arm a guidance wake: {why}"
            ))
        );
        return Ok(UNARMED);
    }
    for problem in &problems {
        eprintln!(
            "{}",
            sanitize_line(&format!("recipe guidance: warning: {problem}"))
        );
    }
    match wait_armed(&mut folders, &mut waiters, since, command)? {
        WaitOutcome::Guidance(name) => print_json(&guidance_event(&name))?,
        WaitOutcome::Command(status) => print_json(&command_event(status)?)?,
    }
    Ok(0)
}

#[cfg(test)]
#[path = "guidance_tests.rs"]
mod tests;
