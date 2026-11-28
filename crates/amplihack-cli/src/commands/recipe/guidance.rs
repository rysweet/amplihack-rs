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

use super::live_runs::{RunEnd, run_end, runs_for_pr_by_state};
use crate::cli_subcommands::{GuidanceCommands, PrFolderArgs};
use crate::command_error::exit_error;
use crate::run_guidance::env::{DirCheck, check_guidance_dir};
use crate::run_guidance::folders::{Folder, Folders};
use crate::run_guidance::log_state::{Drop, LogState, read_log_state, render_drop_line};
use crate::run_guidance::pr_key::{PrRef, parse_pr_url, parse_repo_slug};
use crate::run_guidance::sanitize::sanitize_line;
use crate::run_guidance::wait::{WaitOutcome, arm, wait_armed};
use crate::run_guidance::{
    Entry, GUIDANCE_DIR, GUIDANCE_DIRS_ENV, GuideError, Source, is_entry_name, is_new,
    render_block_marked,
};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
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
        GuidanceCommands::Deliver => deliver().map(|()| 0),
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

/// Issue #3435: `recipe guidance deliver`, run by the PostToolUse and
/// UserPromptSubmit hooks of an agent step inside a recipe run. Prints the
/// block of entries the step has not been given yet, or nothing, and records
/// them for the step. Outside a recipe run it prints nothing. When the
/// delivery fails it records a `hook_delivery_failed` drop where it can, so
/// `recipe guide` tells the sender, and exits 1; the hook goes on either way.
fn deliver() -> Result<(), GuideError> {
    use crate::run_guidance::step::{STEP_ID_ENV, deliver};
    let Some(step_id) = std::env::var(STEP_ID_ENV).ok().filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    let Some(value) = std::env::var_os(GUIDANCE_DIRS_ENV).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    let dirs = crate::run_guidance::env::dirs_from_env_value(&value);
    let run_id = std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_default();
    match deliver(&step_id, &run_id, &dirs) {
        Ok(Some(block)) => {
            let mut out = std::io::stdout().lock();
            writeln!(out, "{block}")
                .and_then(|()| out.flush())
                .map_err(|e| GuideError::Io(format!("cannot write the guidance: {e}")))
        }
        Ok(None) => Ok(()),
        Err(error) => {
            let label = crate::run_guidance::StepLabel {
                step_run_id: run_id,
                tool: hook_tool(),
                launch_form: "hook of a running step".to_string(),
                agent: None,
            };
            crate::run_guidance::GuidanceForLaunch::from_dirs(dirs)
                .record_drop(&label, crate::run_guidance::DropReason::HookDeliveryFailed);
            Err(error)
        }
    }
}

/// The tool a hook runs under, from the closed set the launcher writes.
fn hook_tool() -> String {
    match std::env::var("AMPLIHACK_AGENT_BINARY").as_deref() {
        Ok(tool @ ("claude" | "codex" | "copilot")) => tool.to_string(),
        _ => "other".to_string(),
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

/// Whether two folder paths name one folder: the same path, or the same
/// canonical path when both exist.
fn same_folder(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(x), Ok(y)) if x == y
        )
}

/// Adds `folder` to `list` at `at` unless the list already holds it.
fn insert_folder(list: &mut Vec<Folder>, at: usize, folder: Folder) -> usize {
    if list.iter().any(|f| same_folder(&f.path, &folder.path)) {
        return at;
    }
    list.insert(at, folder);
    at + 1
}

/// The folders `show` and `wait` read.
///
/// - A pull request (URL, or `--pr` with `--pr-repo`): the folders
///   `AMPLIHACK_RUN_GUIDANCE_DIRS` lists, then the guidance folder of each
///   live run for the pull request (issue #3161), then the pull request's
///   folder. This holds every folder `recipe guide <PR URL>` writes, so
///   the goal loop's read and the sender's write agree whatever the run's
///   work root. A live run that cannot be reached is listed as untrusted,
///   with why. An ended run's folder is not listed: the goal loop exports
///   this list to every turn, so a new drive would otherwise take an ended
///   drive's own guidance (PR #3190 review; see `live_runs`).
/// - Any other target (issue #3162): the guidance folder of the run that a
///   detached session name, a run folder name or a correlation id names,
///   found as `recipe guide` finds it, after the listed folders.
/// - No target: the listed folders.
///
/// The second value: for a pull request, the guidance folders of its ended
/// runs that still pass the checks. They are not listed and their entries
/// are not read; only their logs are, so an entry a step read before its
/// run ended is still reported as read when the copy in a listed folder
/// holds no record of it (a drive whose state folder is not the sender's).
fn resolve_folders(args: &PrFolderArgs) -> Result<(Folders, Vec<PathBuf>), GuideError> {
    if let (Some(target), None, None) = (&args.target, &args.pr, &args.pr_repo)
        && !target.contains("://")
    {
        let dir = super::guide::run_guidance_dir(target)?;
        let mut folders = Folders::resolve(None)?;
        let at = folders.list.len();
        let check = check_guidance_dir(&dir);
        insert_folder(&mut folders.list, at, Folder { path: dir, check });
        return Ok((folders, Vec::new()));
    }
    let pr = named_pr(args)?;
    let mut folders = Folders::resolve(pr.as_ref())?;
    let mut ended_logs = Vec::new();
    if let Some(pr) = &pr {
        let pr_path = folders.pr.as_ref().map(|p| p.path.clone());
        let mut at = folders
            .list
            .iter()
            .position(|f| Some(&f.path) == pr_path.as_ref())
            .unwrap_or(folders.list.len());
        let (live, ended) = runs_for_pr_by_state(pr);
        ended_logs = ended;
        for run in live {
            let folder = match run.reachable {
                Ok(dir) => Folder {
                    check: check_guidance_dir(&dir),
                    path: dir,
                },
                Err(reason) => Folder {
                    path: run.run_dir.join(GUIDANCE_DIR),
                    check: DirCheck::Untrusted(format!(
                        "live run of session {} cannot be reached: {reason}",
                        run.session
                    )),
                },
            };
            at = insert_folder(&mut folders.list, at, folder);
        }
    }
    Ok((folders, ended_logs))
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
    /// Issue #3162: launches since the last read that could not take the
    /// guidance, per folder. Added to schema 1; readers ignore it.
    drops: Vec<DropJson>,
}

#[derive(Debug, Serialize)]
struct FolderJson {
    path: String,
    source: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// Issue #3162: why this trusted folder's `guidance.log` could not be
    /// read. Its entries' `read` is then `null` unless another folder
    /// proves them read, and its drops are unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    log_error: Option<String>,
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
    /// Issue #3162: `true` when a `guidance_injected` record in a folder
    /// that holds the entry (or a copy of it) names it; `null` when none
    /// does and the log of at least one of those folders could not be read;
    /// `false` otherwise. `true` means the block was written in full to an
    /// agent step's input, not that the agent acted on it.
    read: Option<bool>,
    /// PR #3190 review: present when every run folder holding the entry is
    /// that of a run whose `run.json` records its end. Absent when a run
    /// folder holding it records none, or only a pull request's folder
    /// holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    run_ended: Option<RunEndJson>,
}

/// The end a run recorded, as `run.json` gives it (sanitised).
#[derive(Debug, Serialize)]
struct RunEndJson {
    at: String,
    outcome: String,
}

/// The recorded end of the runs holding `entry`, when every run folder that
/// holds it records one: the first such folder's. `None` when no run folder
/// holds it or one of them records no end.
fn entry_run_end(entry: &Entry) -> Option<RunEnd> {
    let mut ends = std::iter::once(entry.dir.as_path())
        .chain(entry.also_in.iter().map(|(d, _)| d.as_path()))
        .filter(|dir| Source::of(dir) == Source::Run)
        .map(run_end);
    let first = ends.next()??;
    ends.all(|end| end.is_some()).then_some(first)
}

/// One `guidance_dropped` record. Every value comes from a closed table or
/// is a run id or a sanitised agent name; none is guidance or argv text.
#[derive(Debug, Serialize)]
struct DropJson {
    folder: String,
    ts: String,
    step_run_id: String,
    tool: String,
    launch_form: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    reason: String,
    detail: String,
}

/// What each guidance folder's log says, read once per folder, and the
/// guidance folders of ended runs whose logs also count as proof of a read.
#[derive(Default)]
struct LogStates {
    logs: HashMap<PathBuf, LogState>,
    ended: Vec<PathBuf>,
}

impl LogStates {
    /// Reads `ended` folders' logs, never their entries, for read state.
    fn with_ended(ended: &[PathBuf]) -> Self {
        Self {
            logs: HashMap::new(),
            ended: ended.to_vec(),
        }
    }

    fn of(&mut self, dir: &Path) -> &LogState {
        self.logs
            .entry(dir.to_path_buf())
            .or_insert_with(|| read_log_state(dir))
    }

    /// Whether a step has read `entry`, from any folder that holds a copy:
    /// `Some(true)` when one of their logs names it, `None` when none does
    /// and one of their logs could not be read, `Some(false)` otherwise.
    fn read(&mut self, entry: &Entry) -> Option<bool> {
        let mut unknown = false;
        for (dir, name) in std::iter::once((entry.dir.as_path(), entry.name.as_str()))
            .chain(entry.also_in.iter().map(|(d, n)| (d.as_path(), n.as_str())))
        {
            let state = self.of(dir);
            if state.read.contains(name) {
                return Some(true);
            }
            unknown |= state.log_error.is_some();
        }
        // PR #3190 review: an ended run's log naming the entry proves a step
        // read a copy of it. An ended run's unreadable log proves nothing
        // and leaves the answer as it is.
        for dir in self.ended.clone() {
            if self.of(&dir).read.contains(&entry.name) {
                return Some(true);
            }
        }
        if unknown { None } else { Some(false) }
    }
}

/// The drops since the last read in each trusted folder of `list`.
fn drops_of(list: &[Folder], states: &mut LogStates) -> Vec<(PathBuf, Drop)> {
    list.iter()
        .filter(|f| f.check == DirCheck::Trusted)
        .flat_map(|f| {
            states
                .of(&f.path)
                .drops_since_injection
                .iter()
                .map(|d| (f.path.clone(), d.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The `show --json` document. Read state and drops come from each folder's
/// `guidance.log`.
fn show_json<'a>(
    list: &[Folder],
    ended: &[PathBuf],
    entries: &'a [Entry],
    since: &'a str,
) -> ShowJson<'a> {
    let mut states = LogStates::with_ended(ended);
    let folders = list
        .iter()
        .map(|f| {
            let (state, reason) = match &f.check {
                DirCheck::Trusted => ("present", None),
                DirCheck::Missing => ("missing", None),
                DirCheck::Untrusted(reason) => ("untrusted", Some(sanitize_line(reason))),
            };
            let log_error = (f.check == DirCheck::Trusted)
                .then(|| states.of(&f.path).log_error.clone())
                .flatten();
            FolderJson {
                path: path_text(&f.path),
                source: Source::of(&f.path).label(),
                state,
                reason,
                log_error,
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
            read: states.read(e),
            run_ended: entry_run_end(e).map(|end| RunEndJson {
                at: end.at,
                outcome: end.outcome,
            }),
        })
        .collect();
    let drops = drops_of(list, &mut states)
        .into_iter()
        .map(|(folder, d)| DropJson {
            folder: path_text(&folder),
            ts: d.ts,
            step_run_id: d.step_run_id,
            tool: d.tool,
            launch_form: d.launch_form,
            agent: d.agent,
            reason: d.reason,
            detail: d.detail,
        })
        .collect();
    ShowJson {
        schema_version: SCHEMA_VERSION,
        since,
        newest: entries.last().map_or("", |e| e.name.as_str()),
        folders,
        entries: entries_json,
        drops,
    }
}

/// The lines plain `show` prints after the block (issue #3162): whether each
/// entry has been read, then each drop since the last read, once. An entry
/// not yet read whose run has recorded its end says so (PR #3190 review):
/// no later step of that run started to read it.
fn read_and_drop_lines(list: &[Folder], ended: &[PathBuf], entries: &[Entry]) -> Vec<String> {
    let mut states = LogStates::with_ended(ended);
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            let state = match states.read(e) {
                Some(true) => "read".to_string(),
                Some(false) => match entry_run_end(e) {
                    Some(end) => format!(
                        "not yet read; the run ended at {} ({})",
                        end.at, end.outcome
                    ),
                    None => "not yet read".to_string(),
                },
                None => "read state unknown (log unreadable)".to_string(),
            };
            format!("entry {}: {state}", e.name)
        })
        .collect();
    for (_, drop) in drops_of(list, &mut states) {
        let line = render_drop_line(&drop);
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines
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
    let (folders, ended) = resolve_folders(args)?;
    folders.warn_untrusted();
    let mut entries = folders.read();
    if operator_only {
        entries.retain(|e| !e.is_from_step());
    }
    if json {
        print_json(&show_json(&folders.list, &ended, &entries, since))
    } else {
        print_lines(&render_block_marked(&entries, Some(since)))?;
        print_lines(&read_and_drop_lines(&folders.list, &ended, &entries).join("\n"))
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
    let (mut folders, _) = resolve_folders(args)?;
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
