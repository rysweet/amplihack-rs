//! Issue #2820: one process of the same user whose environment cannot be
//! read (not dumpable, changed credentials, or an environment over the
//! scan's bound). It is judged from its command line and the name in its
//! `status`, never from its environment, and `/proc/<pid>/exe` is never read:
//!
//! - A Claude or amplihack executable ([`claude_executable`]) makes doubtful
//!   only the profiles its arguments name ([`Verdict::Counted`]). One that
//!   names none makes no profile live ([`Verdict::ProfileUnknown`]): it most
//!   likely uses `~/.claude`, the live login, which is not a candidate.
//! - Any other process is no evidence ([`Verdict::NotClaude`]). It is noted
//!   only when its command line mentions `claude` or `amplihack`, which is
//!   what the rule before #2820 counted, so the log names what changed.
//!
//! A profile name taken from an argument is only ever a lookup key: it is
//! never used to build a path that is opened.

use super::fsx;
use crate::commands::agent::procfs;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

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
    /// A Claude or amplihack executable naming these profiles: each is
    /// doubtful, and doubtful counts as live.
    Counted(BTreeSet<String>),
    /// Not a Claude or amplihack executable: no evidence for any profile.
    NotClaude,
    /// A Claude or amplihack executable naming no profile: no profile is
    /// live because of it.
    ProfileUnknown,
}

/// The judgement of the process in `/proc/<pid>` (`dir`), whose `status`
/// name is `comm`, against the canonical profile root `root`. `None` for a
/// process that is not a Claude or amplihack executable and whose command
/// line does not mention either: it is neither evidence nor worth a line.
pub fn judge(dir: &Path, pid: u32, comm: &str, root: &Path) -> Option<Unreadable> {
    let argv = argv(dir);
    let verdict = if claude_executable(comm, &argv) {
        let names = named_profiles(&argv, root);
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

/// The profile `path` is, or is under: the first component under `root`
/// once `path` is resolved. Only an absolute path counts. `root` itself, and
/// anything outside it, names nothing.
fn profile_of(path: &Path, root: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let resolved = resolve(path)?;
    match resolved.strip_prefix(root).ok()?.components().next()? {
        Component::Normal(name) => name.to_str().map(str::to_string),
        _ => None,
    }
}

/// `path` with its nearest existing ancestor canonicalised and the rest
/// joined on. The rest may hold only plain names: a `..`, a root or a prefix
/// in a part that does not exist cannot be resolved, so the path names
/// nothing.
fn resolve(path: &Path) -> Option<PathBuf> {
    let (ancestor, canonical) = path
        .ancestors()
        .find_map(|a| a.canonicalize().ok().map(|c| (a, c)))?;
    let rest = path.strip_prefix(ancestor).ok()?;
    if !rest.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(canonical.join(rest))
}
