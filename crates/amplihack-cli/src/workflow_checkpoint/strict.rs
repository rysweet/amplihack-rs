//! `stage-strict LIST`: strict callers supply a complete NUL-terminated list
//! of literal paths. Validate all of it before the first index write, then
//! pass the same bytes to `git add`.

use super::repo::{GitEnv, canonical, git_command, split_nul, strip_newlines};
use super::{Fail, Res};
use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn refusal(reason: &str) -> Fail {
    Fail::Refusal(reason.to_owned())
}

fn os_error(error: std::io::Error) -> Fail {
    Fail::Refusal(error.to_string())
}

/// An `OSError` on the path list as `str(exc)` printed it in 918c:
/// `[Errno N] <strerror>: '<filename>'`.
fn list_error(list: &Path) -> impl Fn(std::io::Error) -> Fail + '_ {
    move |error| match error.raw_os_error() {
        Some(errno) => {
            let text = error.to_string();
            let suffix = format!(" (os error {errno})");
            let strerror = text.strip_suffix(&suffix).unwrap_or(&text);
            Fail::Refusal(format!(
                "[Errno {errno}] {strerror}: {}",
                super::json::py_repr_os(list.as_os_str().as_bytes())
            ))
        }
        None => os_error(error),
    }
}

/// One argument of a STRICT_PATHS_PY Git call, kept with its Python type so
/// a failure can quote the argv list exactly as `CalledProcessError` did:
/// the literal options were `str`, the toplevel from `os.path.realpath` of
/// Git's bytes output was `bytes`.
#[derive(Clone, Copy)]
enum Arg<'a> {
    Str(&'a str),
    Bytes(&'a OsStr),
}

impl Arg<'_> {
    fn os(&self) -> &OsStr {
        match self {
            Arg::Str(s) => OsStr::new(s),
            Arg::Bytes(b) => b,
        }
    }

    fn repr(&self) -> String {
        match self {
            Arg::Str(s) => super::json::py_repr_os(s.as_bytes()),
            Arg::Bytes(b) => super::json::py_bytes_repr(b.as_bytes()),
        }
    }
}

/// `str(subprocess.CalledProcessError)` for `['git', '--literal-pathspecs',
/// *args]`: `Command '[...]' returned non-zero exit status N.`, or, for a
/// signal, `died with <Signals.NAME: N>.` (`unknown signal N.` when Python's
/// `signal.Signals` has no name for it).
fn called_process_error(args: &[Arg], output: &std::process::Output) -> String {
    use std::os::unix::process::ExitStatusExt;
    let mut argv = vec![Arg::Str("git"), Arg::Str("--literal-pathspecs")];
    argv.extend_from_slice(args);
    let list: Vec<String> = argv.iter().map(Arg::repr).collect();
    let cmd = format!("[{}]", list.join(", "));
    match (output.status.code(), output.status.signal()) {
        (Some(code), _) => format!("Command '{cmd}' returned non-zero exit status {code}."),
        (None, Some(signal)) => match signal_name(signal) {
            Some(name) => format!("Command '{cmd}' died with <Signals.{name}: {signal}>."),
            None => format!("Command '{cmd}' died with unknown signal {signal}."),
        },
        (None, None) => format!("Command '{cmd}' returned non-zero exit status -1."),
    }
}

/// Linux names of Python's `signal.Signals` members.
fn signal_name(signal: i32) -> Option<&'static str> {
    const NAMES: [&str; 31] = [
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
    ];
    usize::try_from(signal)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| NAMES.get(i).copied())
}

/// Run Git for `stage-strict`. 918c STRICT_PATHS_PY ran git with
/// `os.environ` unchanged, so the builder is the row's `GitEnv` (Inherit):
/// GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE and the rest select the repository
/// and index exactly as they did for the Python.
///
/// Input is written on its own thread while the output is read. A write
/// error is ignored, as `subprocess.run(input=...)` ignored the
/// `BrokenPipeError` of a Git that exited before reading all of it; Git's
/// own status then decides.
///
/// A non-zero status is the `CalledProcessError` 918c caught: Git's stderr,
/// then a strict refusal (status 2) quoting the command. The one exception
/// is a real 79, which the 17.126 stage-strict addendum (ApprovedRow
/// `17.126-stage-strict-79`) passes through as 79 after Git's stderr, with
/// no refusal line, so a terminal policy status is never reported as 2.
fn git(git_env: GitEnv, args: &[Arg], data: Option<&[u8]>) -> Res<Vec<u8>> {
    let mut child = git_command(git_env)?
        .arg("--literal-pathspecs")
        .args(args.iter().map(Arg::os))
        .stdin(if data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(os_error)?;
    let stdin = child.stdin.take();
    let output = std::thread::scope(|scope| {
        if let (Some(data), Some(mut stdin)) = (data, stdin) {
            scope.spawn(move || {
                let _ = stdin.write_all(data);
            });
        }
        child.wait_with_output()
    })
    .map_err(os_error)?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let _ = std::io::stderr().write_all(&output.stderr);
    if output.status.code() == Some(79) {
        return Err(Fail::Terminal79);
    }
    Err(Fail::Refusal(called_process_error(args, &output)))
}

fn validate(git_env: GitEnv, list: &Path) -> Res<(PathBuf, Vec<u8>, usize)> {
    let fail = |reason: &str| Err(refusal(reason));
    let info = std::fs::symlink_metadata(list).map_err(list_error(list))?;
    if !info.file_type().is_file() {
        return fail("path list must be a regular file");
    }
    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(list)
            .map_err(list_error(list))?,
        &mut raw,
    )
    .map_err(os_error)?;
    if !raw.is_empty() && !raw.ends_with(&[0]) {
        return fail("unterminated path list");
    }
    let paths: Vec<&[u8]> = if raw.is_empty() {
        Vec::new()
    } else {
        raw[..raw.len() - 1].split(|&b| b == 0).collect()
    };
    if paths.iter().collect::<BTreeSet<_>>().len() != paths.len() {
        return fail("duplicate path");
    }
    // First call: no -C, so Git's own discovery (cwd and the inherited
    // GIT_DIR / GIT_WORK_TREE) decides the top, as STRICT_PATHS_PY did.
    let top = canonical(Path::new(OsStr::from_bytes(&strip_newlines(git(
        git_env,
        &[Arg::Str("rev-parse"), Arg::Str("--show-toplevel")],
        None,
    )?))));
    // Later calls: -C top. Under GIT_DIR without GIT_WORK_TREE, -C changes
    // only the cwd; GIT_DIR still selects the repository, as in 918c.
    let tracked = split_nul(&git(
        git_env,
        &[
            Arg::Str("-C"),
            Arg::Bytes(top.as_os_str()),
            Arg::Str("ls-files"),
            Arg::Str("-z"),
        ],
        None,
    )?);
    let mut directory_entries: HashMap<PathBuf, BTreeSet<Vec<u8>>> = HashMap::new();
    for path in &paths {
        if path.is_empty()
            || path.starts_with(b"/")
            || path
                .split(|&b| b == b'/')
                .any(|p| p.is_empty() || p == b"." || p == b"..")
        {
            return fail("path must be an exact worktree-relative identity");
        }
        let components: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
        let mut parent = top.clone();
        for (number, component) in components.iter().enumerate() {
            // A case-insensitive filesystem can resolve a different spelling.
            // Compare directory-entry bytes without normalisation or folding.
            if parent.is_dir() {
                if !directory_entries.contains_key(&parent) {
                    let names = std::fs::read_dir(&parent)
                        .map_err(os_error)?
                        .flatten()
                        .map(|e| e.file_name().as_bytes().to_vec())
                        .collect();
                    directory_entries.insert(parent.clone(), names);
                }
                let child = parent.join(OsStr::from_bytes(component));
                if !directory_entries[&parent].contains(*component) && super::repo::lexists(&child)
                {
                    return fail("path spelling differs from filesystem identity");
                }
            }
            if number == components.len() - 1 {
                break;
            }
            parent.push(OsStr::from_bytes(component));
            if super::repo::is_link(&parent) {
                return fail("symlink parent is unsupported");
            }
            if super::repo::lexists(&parent) && !parent.is_dir() {
                return fail("non-directory parent");
            }
        }
        let full = top.join(OsStr::from_bytes(path));
        match std::fs::symlink_metadata(&full) {
            Ok(meta) => {
                let kind = meta.file_type();
                if !(kind.is_file() || kind.is_symlink()) {
                    return fail("directory or special-file selector");
                }
            }
            Err(_) if !tracked.contains(*path) => return fail("missing untracked path"),
            Err(_) => {}
        }
    }
    let count = paths.len();
    Ok((top, raw, count))
}

/// 918c STRICT_PATHS_PY read `sys.argv[1]` with no length check: a missing
/// list is IndexError (status 1, `Arity::Indexed(1)`); extra arguments are
/// ignored. `git_env` is the row's builder, `Inherit` for this
/// `Selectors::Ignored` row.
pub(super) fn stage_strict(git_env: GitEnv, args: &[OsString]) -> Res<Vec<u8>> {
    let list = super::indexed(args, 0)?;
    let (top, raw, count) = validate(git_env, Path::new(list))?;
    if count > 0 {
        git(
            git_env,
            &[
                Arg::Str("-C"),
                Arg::Bytes(top.as_os_str()),
                Arg::Str("add"),
                Arg::Str("--pathspec-from-file=-"),
                Arg::Str("--pathspec-file-nul"),
            ],
            Some(&raw),
        )?;
    }
    Ok(Vec::new())
}
