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
//! Exit codes: 0 appended; 1 target not found, a folder fails a check, or
//! the `--file` file cannot be read; 2 usage (text on the command line, `-`
//! together with `--file`, a terminal on stdin, empty, oversize or invalid
//! UTF-8 text, a delimiter or control character, malformed URL, called
//! inside a run without `--from-step`).
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
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// How much of a session log is scanned for its pointer line.
const MAX_SESSION_LOG_SCAN: u64 = 64 * 1024;
/// The longest pointer line accepted.
const MAX_POINTER_LINE: usize = 4096;
const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";
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
            match &error {
                // The documented limit message stands alone.
                GuideError::Oversize { .. } => eprintln!("{error}"),
                _ => eprintln!("recipe guide: {error}"),
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
    let (dir, source) = resolve_target(target)?;
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
}
