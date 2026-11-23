//! Issue #2820: one process of the same user whose environment cannot be
//! read (not dumpable, changed credentials, or an environment over the
//! scan's bound). It is judged from its command line and the name in its
//! `status`, never from its environment, and `/proc/<pid>/exe` is never read:
//!
//! - A Claude or amplihack executable ([`claude_executable`]) makes doubtful
//!   only the profiles it can be tied to ([`Verdict::Counted`]): those its
//!   arguments name, and those that hold Claude Code's own record of it,
//!   `<root>/<name>/sessions/<pid>.json` ([`session_file_profiles`]).
//! - One tied to no profile makes no profile live
//!   ([`Verdict::ProfileUnknown`]), and so protects none. This is the one
//!   judgement here that errs in the unsafe direction. A login it uses
//!   through `CLAUDE_CONFIG_DIR` set only in its own environment, with no
//!   session file of Claude Code's under that profile, can be moved out from
//!   under it. The log line says `profile cannot be told`. Making every
//!   profile live instead is what issue #2820 forbids. Two other sources add
//!   nothing. A readable relative that carries `CLAUDE_CONFIG_DIR` (the
//!   parent that started it, or a hook, tool or MCP server it started)
//!   already makes that profile live by the ordinary rule in `liveness.rs`.
//!   `/proc/<pid>/cwd` is behind the same access check as `environ` for a
//!   process that is not dumpable or changed its credentials. It names a
//!   working directory, not a config directory.
//! - Any other process is no evidence ([`Verdict::NotClaude`]). It is noted
//!   only when its command line mentions `claude` or `amplihack`, which is
//!   what the rule before #2820 counted, so the log names what changed.
//!
//! A profile name taken from an argument is only ever a lookup key: it is
//! never used to build a path that is opened. The session-file check only
//! looks entries up by name and opens nothing.

use super::fsx;
use crate::commands::agent::procfs;
use std::collections::BTreeSet;
use std::path::{Component, Components, Path, PathBuf};

/// The most of `/proc/<pid>/cmdline` read. A longer one reads as empty, and
/// the process is judged by its `status` name alone.
const MAX_CMDLINE: u64 = 64 * 1024;

/// An unreadable process the scan noted, and what it counted for.
#[derive(Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub pid: u32,
    /// The basename of `argv[0]`, or the `status` name when `argv` is empty
    /// or `argv[0]` is an environment assignment.
    pub exe: String,
    /// The raw arguments. Never shown by `Debug`; the rotate log masks,
    /// escapes and cuts them (`rotate/unreadable_msg.rs`).
    pub argv: Vec<String>,
    pub verdict: Verdict,
}

/// Shows the pid, the executable, the verdict and the number of arguments,
/// never their text: a derived `Debug` would put raw arguments into any
/// future `{:?}` of a snapshot.
impl std::fmt::Debug for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Unreadable")
            .field("pid", &self.pid)
            .field("exe", &self.exe)
            .field("verdict", &self.verdict)
            .field("argc", &self.argv.len())
            .finish()
    }
}

/// What an unreadable process counts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A Claude or amplihack executable tied to these profiles, by its
    /// arguments or by a session file: each is doubtful, and doubtful counts
    /// as live.
    Counted(BTreeSet<String>),
    /// Not a Claude or amplihack executable: no evidence for any profile.
    NotClaude,
    /// A Claude or amplihack executable tied to no profile: no profile is
    /// live because of it, so it protects none (see the module docs).
    ProfileUnknown,
}

/// The judgement of the process in `/proc/<pid>` (`dir`), whose `status`
/// name is `comm`, against the canonical profile root `root`. `None` for a
/// process that is not a Claude or amplihack executable and whose command
/// line does not mention either: it is neither evidence nor worth a line.
pub fn judge(dir: &Path, pid: u32, comm: &str, root: &Path) -> Option<Unreadable> {
    let argv = argv(dir);
    let verdict = if claude_executable(comm, &argv) {
        let mut names = named_profiles(&argv, root);
        names.extend(session_file_profiles(pid, root));
        if names.is_empty() {
            Verdict::ProfileUnknown
        } else {
            Verdict::Counted(names)
        }
    } else if mentions_claude(&argv) {
        Verdict::NotClaude
    } else {
        return None;
    };
    Some(Unreadable {
        pid,
        exe: exe(comm, &argv),
        argv,
        verdict,
    })
}

/// Whether the process is a Claude or amplihack executable: by the `status`
/// name or the basename of `argv[0]` (`claude`, `amplihack`, `amplihack-*`),
/// or `node` running the Claude Code entry point (`procfs::is_claude`). Text
/// elsewhere in the command line does not count.
pub fn claude_executable(comm: &str, argv: &[String]) -> bool {
    let amplihack = |n: &str| n == "amplihack" || n.starts_with("amplihack-");
    procfs::is_claude(comm, argv)
        || amplihack(comm)
        || argv.first().is_some_and(|a| amplihack(base(a)))
}

/// The arguments in `/proc/<pid>/cmdline`, lossily decoded. Empty when it
/// cannot be read or is over [`MAX_CMDLINE`]: the process is then judged by
/// its `status` name alone.
pub fn argv(dir: &Path) -> Vec<String> {
    match fsx::read_bounded_nofollow(&dir.join("cmdline"), MAX_CMDLINE) {
        Ok(Some((raw, _))) => procfs::split_nul(&raw),
        Ok(None) | Err(_) => Vec::new(),
    }
}

/// The `NAME` of an argument shaped `NAME=value`, where `NAME` is a valid
/// environment variable name.
pub fn assignment_name(arg: &str) -> Option<&str> {
    let (name, _) = arg.split_once('=')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some(name)
}

fn base(arg: &str) -> &str {
    arg.rsplit('/').next().unwrap_or(arg)
}

fn exe(comm: &str, argv: &[String]) -> String {
    match argv.first() {
        Some(first) if assignment_name(first).is_none() && !base(first).is_empty() => {
            base(first).to_string()
        }
        _ => comm.to_string(),
    }
}

fn mentions_claude(argv: &[String]) -> bool {
    argv.iter().any(|a| {
        let a = a.to_ascii_lowercase();
        a.contains("claude") || a.contains("amplihack")
    })
}

/// The profiles under `root` that the arguments after `argv[0]` name: each
/// argument, and the text after each `=` in it (`KEY=<path>`,
/// `--env=CLAUDE_CONFIG_DIR=<path>`), that is an absolute path to a profile
/// directory or to something under one.
fn named_profiles(argv: &[String], root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for arg in argv.iter().skip(1) {
        let after_eq = arg.match_indices('=').map(|(i, _)| &arg[i + 1..]);
        for part in std::iter::once(arg.as_str()).chain(after_eq) {
            if let Some(name) = profile_of(Path::new(part), root) {
                names.insert(name);
            }
        }
    }
    names
}

/// The profiles under `root` that hold Claude Code's own record of process
/// `pid`: an entry named `<root>/<name>/sessions/<pid>.json`. Claude Code
/// writes that file in the config directory it runs under, for `-p` runs as
/// well (checked with Claude Code 2.1.330), so it ties a process whose
/// environment cannot be read to its profile. The entries are looked up by
/// name: nothing is opened or read.
///
/// Every guess here errs toward live. A file whatever its age counts: one
/// left by an earlier process with the same pid can only make one more
/// profile doubtful, which delays a swap. A profile directory, or a
/// `sessions` directory, that is a symlink is not that profile's own record
/// and ties nothing. Otherwise a `sessions` directory shared by several
/// profiles would make each of them live for one process, the failure of
/// issue #2820. A root that cannot be listed ties nothing.
fn session_file_profiles(pid: u32, root: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return BTreeSet::new();
    };
    let file = format!("{pid}.json");
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let sessions = e.path().join("sessions");
            std::fs::symlink_metadata(&sessions).is_ok_and(|m| m.is_dir())
                && std::fs::symlink_metadata(sessions.join(&file)).is_ok()
        })
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect()
}

/// The profile `path` is, or is under: the first component under `root`
/// once `path` is resolved, its existing prefix canonicalised ([`resolve`])
/// and the rest joined on. Only an absolute path counts. `root` itself, and
/// anything outside it, names nothing. The rest may hold only plain names:
/// a `..` in a part that does not exist cannot be resolved, so the path
/// names nothing.
///
/// The resolved path is compared with `root` part by part and never built,
/// so a long argument that leaves `root` costs no more than its first parts.
fn profile_of(path: &Path, root: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let (canonical, rest) = resolve(path)?;
    let mut parts = canonical.components().chain(rest);
    if !root.components().all(|r| parts.next() == Some(r)) {
        return None;
    }
    let Some(Component::Normal(name)) = parts.next() else {
        return None;
    };
    if parts.any(|p| p == Component::ParentDir) {
        return None;
    }
    name.to_str().map(str::to_string)
}

/// `path`'s longest existing prefix, canonicalised, and the parts after it.
///
/// The prefix is found from the top down, and the search stops at the first
/// part that does not resolve. A path resolves only when each of its
/// prefixes does, so this is the prefix a search from the leaf up would
/// find, but an argument with thousands of parts (a prompt passed on the
/// command line) costs one lookup for each part that exists, not one for
/// each part it has.
fn resolve(path: &Path) -> Option<(PathBuf, Components<'_>)> {
    let mut rest = path.components();
    let mut prefix = PathBuf::new();
    let mut canonical = None;
    while let Some(part) = rest.clone().next() {
        prefix.push(part);
        let Ok(c) = prefix.canonicalize() else {
            break;
        };
        canonical = Some(c);
        rest.next();
    }
    Some((canonical?, rest))
}
