//! Issue #2535: `amplihack recipe guide <TARGET> [TEXT|-]`.
//!
//! Appends one operator instruction to a recipe run's guidance. Every later
//! agent step of that run receives all entries, in order, after its own
//! prompt (see [`crate::run_guidance`]).
//!
//! The target is resolved in this order; the first match wins:
//!
//! 1. a pull request URL (anything with `://` is only ever read as one):
//!    the per-PR state folder, created when absent;
//! 2. a run folder name under `<work root>/runs/`;
//! 3. a correlation id (`attempts[].correlation_id` in a run's `run.json`);
//! 4. a detached session name: the first `amplihack.recipe.log_pointer` line
//!    of `<recipe log dir>/<session>.log` names the run.
//!
//! Inside a recipe run (`AMPLIHACK_RECIPE_RUN_ID` set) the command is refused
//! unless `--from-step` is passed, so an agent step misled by text it read
//! does not add guidance by accident. Entries it adds with the flag carry
//! ` run=<id>` in their caller field and are shown under their own preamble.
//!
//! Exit codes: 0 appended; 1 target not found or a folder fails a check;
//! 2 usage (empty, oversize, invalid UTF-8 or refused text, malformed URL,
//! called inside a run without `--from-step`).
//! Every failure prints one line on stderr and writes nothing.

use crate::command_error::exit_error;
use crate::commands::cleanup::run_dir::RunReport;
use crate::commands::recipe::run_id::validate_run_id;
use crate::run_guidance::fs_safety::{check_dir, ensure_private_chain, untrusted_reason};
use crate::run_guidance::pr_key::{parse_pr_url, pr_guidance_parent, pr_key, state_root};
use crate::run_guidance::{
    AUTO_DRIVE_DIR, GUIDANCE_DIR, GuideError, HEADER_ROOM, MAX_GUIDANCE_ENTRY_BYTES, Source,
    append_entry, caller_identity, check_text,
};
use anyhow::Result;
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// How much of a session log is scanned for its pointer line.
const MAX_SESSION_LOG_SCAN: u64 = 64 * 1024;
/// The longest pointer line accepted.
const MAX_POINTER_LINE: usize = 4096;
const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";

/// Run `amplihack recipe guide`.
pub fn run_guide(target: &str, text: Option<&OsStr>, from_step: bool) -> Result<()> {
    match guide(target, text, from_step) {
        Ok(()) => Ok(()),
        Err(error) => {
            match &error {
                // The documented limit message stands alone.
                GuideError::Oversize(_) => eprintln!("{error}"),
                _ => eprintln!("recipe guide: {error}"),
            }
            Err(exit_error(error.exit_code()))
        }
    }
}

fn guide(target: &str, text: Option<&OsStr>, from_step: bool) -> Result<(), GuideError> {
    if !from_step && std::env::var_os("AMPLIHACK_RECIPE_RUN_ID").is_some_and(|v| !v.is_empty()) {
        return Err(GuideError::Usage(
            "refusing to add guidance from inside a recipe run (AMPLIHACK_RECIPE_RUN_ID is set); an agent step that means to add an entry must pass --from-step, and the entry is then shown as coming from an agent step"
                .to_string(),
        ));
    }
    let raw = match text {
        Some(t) if t != "-" => t
            .to_str()
            .ok_or_else(|| GuideError::Usage("guidance text is not valid UTF-8".to_string()))?
            .to_string(),
        _ => read_stdin()?,
    };
    let text = check_text(&raw)?;
    let (dir, source) = resolve_target(target)?;
    let appended = append_entry(&dir, &text, &caller_identity())?;
    println!(
        "guidance appended: {} ({} bytes, {})",
        appended.path.display(),
        appended.bytes,
        source.label()
    );
    if let Some(error) = appended.log_error {
        eprintln!("recipe guide: warning: the entry was added but guidance.log was not: {error}");
    }
    Ok(())
}

/// The text on stdin, refusing a terminal and anything far over the limit.
fn read_stdin() -> Result<String, GuideError> {
    // SAFETY: isatty has no preconditions.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Err(GuideError::Usage(
            "no text given and stdin is a terminal; pass TEXT or pipe the text on stdin"
                .to_string(),
        ));
    }
    // Room for surrounding white space, which is trimmed before the limit
    // applies.
    let cap = (MAX_GUIDANCE_ENTRY_BYTES + HEADER_ROOM) as u64;
    let mut stdin = std::io::stdin().lock();
    let mut buf = Vec::new();
    (&mut stdin)
        .take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| GuideError::Io(format!("cannot read stdin: {e}")))?;
    if buf.len() as u64 > cap {
        // Count the rest without keeping it, so the message gives the size.
        let rest = std::io::copy(&mut stdin, &mut std::io::sink())
            .map_err(|e| GuideError::Io(format!("cannot read stdin: {e}")))?;
        return Err(GuideError::Oversize(buf.len() + rest as usize));
    }
    String::from_utf8(buf)
        .map_err(|_| GuideError::Usage("guidance text is not valid UTF-8".to_string()))
}

/// The guidance folder `target` names, and its kind.
fn resolve_target(target: &str) -> Result<(PathBuf, Source), GuideError> {
    if target.contains("://") {
        let pr = parse_pr_url(target)?;
        let root = state_root().map_err(|reason| {
            GuideError::NotFound(format!("cannot find the state folder: {reason}"))
        })?;
        let key = pr_key(&pr.owner, &pr.repo, pr.number);
        ensure_private_chain(&root, &[AUTO_DRIVE_DIR, &key])?;
        return Ok((
            pr_guidance_parent(&root, &key).join(GUIDANCE_DIR),
            Source::Pr,
        ));
    }
    // Kept as a Result: a work root that cannot be resolved must not turn
    // into a misleading "not found".
    let runs = crate::commands::cleanup::work_root::resolve_from_env().map(|w| w.runs_dir());
    if let Ok(runs) = &runs
        && let Some(run_dir) = find_run(runs, target)?
    {
        return Ok((run_dir.join(GUIDANCE_DIR), Source::Run));
    }
    if crate::commands::agent::validate::session_name(target).is_ok() && !target.contains('/') {
        let log_dir = super::detach::log_dir().map_err(|e| GuideError::NotFound(e.to_string()))?;
        let log = log_dir.join(format!("{target}.log"));
        if let Some(run_id) = session_run_id(&log)? {
            let runs = runs.as_ref().map_err(|e| {
                GuideError::NotFound(format!(
                    "session {target:?} names run {run_id:?}, but the work root cannot be resolved: {}",
                    one_line(e)
                ))
            })?;
            return match find_run(runs, &run_id)? {
                Some(run_dir) => Ok((run_dir.join(GUIDANCE_DIR), Source::Run)),
                None => Err(GuideError::NotFound(format!(
                    "session {target:?} names run {run_id:?}, which has no run folder"
                ))),
            };
        }
    }
    match &runs {
        // The target could have been a run, which was never looked for.
        Err(e) if validate_run_id(target) => Err(GuideError::NotFound(format!(
            "cannot look for run {target:?}: {}",
            one_line(e)
        ))),
        _ => Err(GuideError::NotFound(format!(
            "no recipe run or detached session named {target:?}"
        ))),
    }
}

/// An error and its causes on one line.
fn one_line(e: &anyhow::Error) -> String {
    format!("{e:#}").replace(['\n', '\r'], " ")
}

/// The run folder for `id`: a folder of that name under `runs`, or the
/// folder whose `run.json` lists `id` as an attempt's correlation id.
fn find_run(runs: &Path, id: &str) -> Result<Option<PathBuf>, GuideError> {
    if !validate_run_id(id) || !check_dir(runs)? {
        return Ok(None);
    }
    let direct = runs.join(id);
    if check_dir(&direct)? && direct.join("run.json").is_file() {
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

/// The run id in the first pointer line of a session log. `None` when the
/// log does not exist.
fn session_run_id(log: &Path) -> Result<Option<String>, GuideError> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(log)
    {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(GuideError::Unsafe(format!(
                "refusing {}: it is a symbolic link",
                log.display()
            )));
        }
        Err(e) => {
            return Err(GuideError::Io(format!(
                "cannot open {}: {e}",
                log.display()
            )));
        }
    };
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
    file.take(MAX_SESSION_LOG_SCAN)
        .read_to_end(&mut head)
        .map_err(|e| GuideError::Io(format!("cannot read {}: {e}", log.display())))?;
    let missing = || {
        GuideError::NotFound(format!(
            "{} has no amplihack.recipe.log_pointer line",
            log.display()
        ))
    };
    let line = head
        .split(|b| *b == b'\n')
        .find(|l| l.starts_with(LOG_POINTER_PREFIX.as_bytes()))
        .ok_or_else(missing)?;
    if line.len() > MAX_POINTER_LINE {
        return Err(missing());
    }
    let pointer: serde_json::Value =
        serde_json::from_slice(&line[LOG_POINTER_PREFIX.len()..]).map_err(|_| missing())?;
    let run_id = pointer
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(missing)?;
    if !validate_run_id(run_id) {
        return Err(GuideError::NotFound(format!(
            "the amplihack.recipe.log_pointer line of {} names an invalid run id",
            log.display()
        )));
    }
    Ok(Some(run_id.to_string()))
}
