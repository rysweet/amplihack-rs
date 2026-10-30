//! Finding and reading the `claude` process that `agent register` records
//! (R3, design 4.9): the process named by `--pid`, the nearest `claude` in a
//! tmux pane, or the nearest `claude` ancestor. Only this user's live
//! processes are read (S2), one message covers every pid that cannot be used
//! (S4), and a pid reused while it was read is treated as not found (S11).

use super::cmdline::{self, AmplihackClaude, ClaudeOpts};
use super::env_record;
use super::procfs::{self, Snapshot};
use super::sessions;
use super::tmux::{self, Tmux};
use anyhow::{anyhow, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const OFF_LINUX: &str =
    "--pid and --pane need Linux /proc; pass --session, --window, --dir and --conversation";
const NOT_IN_PANE: &str = "no claude process runs in that tmux pane";

/// Which process `register` reads.
pub enum Target {
    Pid(u32),
    Pane(String),
    /// The nearest `claude` ancestor; with a conversation, only when its own
    /// known conversation is that one.
    Ancestor {
        conversation: Option<String>,
    },
}

/// What was read from a `claude` process and the processes around it.
pub struct Discovered {
    pub claude_pid: u32,
    pub proc_start: u64,
    /// The sessions file's `sessionId`, else `--resume` without
    /// `--fork-session` (B3).
    pub conversation: Option<String>,
    /// The sessions file's `cwd`, else the `cwd` link; not yet validated.
    pub launch_dir: Option<PathBuf>,
    pub opts: ClaudeOpts,
    /// The nearest `amplihack claude` ancestor's flags.
    pub amplihack: Option<AmplihackClaude>,
    /// Allowlisted variables of that ancestor, else of the `claude` process.
    pub env: BTreeMap<String, String>,
    /// The first field of `TMUX` in the `claude` process's environment.
    pub tmux_socket: Option<PathBuf>,
    /// `TMUX_PANE` in the `claude` process's environment.
    pub tmux_pane: Option<String>,
    /// Values found but not recorded, without the `warning: ` prefix.
    pub warnings: Vec<String>,
}

/// Reads the target. `Ok(None)` means no process is used: there is no
/// `claude` ancestor, or its conversation is not the one asked for.
pub fn discover(
    target: &Target,
    root: &Path,
    dirs: &[PathBuf],
    pane_tmux: &Tmux,
) -> anyhow::Result<Option<Discovered>> {
    if !cfg!(target_os = "linux") {
        return match target {
            Target::Ancestor { .. } => Ok(None),
            _ => bail!(OFF_LINUX),
        };
    }
    let uid = amplihack_utils::secure_files::euid();
    match target {
        Target::Pid(pid) => {
            let not_claude = || anyhow!("process {pid} is not a claude process of this user");
            let snap = procfs::snapshot(root, uid).map_err(|_| not_claude())?;
            if !is_claude_proc(&snap, *pid) {
                return Err(not_claude());
            }
            read(&snap, root, dirs, *pid)?
                .ok_or_else(not_claude)
                .map(Some)
        }
        Target::Pane(t) => {
            let pane_pid = pane_tmux.pane_pid(t).map_err(|e| anyhow!(e))?;
            let snap = procfs::snapshot(root, uid).map_err(|e| anyhow!(e))?;
            let pid = std::iter::once(pane_pid)
                .chain(snap.descendants(pane_pid))
                .find(|&p| is_claude_proc(&snap, p))
                .ok_or_else(|| anyhow!(NOT_IN_PANE))?;
            read(&snap, root, dirs, pid)?
                .ok_or_else(|| anyhow!(NOT_IN_PANE))
                .map(Some)
        }
        Target::Ancestor { conversation } => {
            let Ok(snap) = procfs::snapshot(root, uid) else {
                return Ok(None);
            };
            let nearest = snap
                .ancestors(snap.self_pid)
                .into_iter()
                .find(|&p| is_claude_proc(&snap, p));
            let Some(pid) = nearest else {
                return Ok(None);
            };
            // Read only to compare: a coordinator registering another
            // session must not record its own process (design 4.9).
            if let Some(want) = conversation
                && conversation_of(&snap, dirs, pid).as_deref() != Some(want.as_str())
            {
                return Ok(None);
            }
            read(&snap, root, dirs, pid)
        }
    }
}

/// A process in the snapshot whose command line could be read and that
/// counts as Claude (A5).
fn is_claude_proc(snap: &Snapshot, pid: u32) -> bool {
    snap.readable_claude(pid)
}

/// The conversation of `pid`: its sessions file when `pid` and `procStart`
/// match, else `--resume` unless the session is a fork.
fn conversation_of(snap: &Snapshot, dirs: &[PathBuf], pid: u32) -> Option<String> {
    let start = snap.procs.get(&pid)?.starttime;
    if let Some(file) = sessions::for_process(dirs, pid, start) {
        return Some(file.session_id);
    }
    let (opts, _) = cmdline::parse_claude(snap.argv(pid)?);
    cmdline::resume_as_conversation(&opts).map(String::from)
}

/// Everything `register` records from `pid`. `Ok(None)` when the process
/// vanished or its pid was reused while it was read (S11).
fn read(
    snap: &Snapshot,
    root: &Path,
    dirs: &[PathBuf],
    pid: u32,
) -> anyhow::Result<Option<Discovered>> {
    let (Some(p), Some(argv)) = (snap.procs.get(&pid), snap.argv(pid)) else {
        return Ok(None);
    };
    let (opts, mut warnings) = cmdline::parse_claude(argv);
    if opts.print {
        bail!("process {pid} is a claude -p run, not an interactive session");
    }
    let session = sessions::for_process(dirs, pid, p.starttime);
    let conversation = match &session {
        Some(file) => Some(file.session_id.clone()),
        None => cmdline::resume_as_conversation(&opts).map(String::from),
    };
    let launch_dir = session
        .and_then(|file| file.cwd)
        .or_else(|| procfs::cwd(root, pid));
    let amp_pid = snap.ancestors(pid).into_iter().find(|&a| {
        snap.argv(a)
            .and_then(cmdline::parse_amplihack_claude)
            .is_some()
    });
    let amplihack = amp_pid
        .and_then(|a| snap.argv(a))
        .and_then(cmdline::parse_amplihack_claude);
    let own = procfs::read_environ(root, pid);
    let (tmux_socket, tmux_pane) = own.as_ref().map_or((None, None), |pairs| tmux_vars(pairs));
    let env_pid = amp_pid.unwrap_or(pid);
    let pairs = if env_pid == pid {
        own
    } else {
        drop(own);
        procfs::read_environ(root, env_pid)
    };
    let env = allowlisted(pairs, env_pid, &mut warnings);
    if procfs::start_time(root, pid) != Some(p.starttime) {
        return Ok(None);
    }
    Ok(Some(Discovered {
        claude_pid: pid,
        proc_start: p.starttime,
        conversation,
        launch_dir,
        opts,
        amplihack,
        env,
        tmux_socket,
        tmux_pane,
        warnings,
    }))
}

/// `TMUX`'s socket and `TMUX_PANE` from an environment; nothing else is
/// looked at.
fn tmux_vars(pairs: &[(String, String)]) -> (Option<PathBuf>, Option<String>) {
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(k, v)| k == name && !v.is_empty())
            .map(|(_, v)| v.as_str())
    };
    let socket = get("TMUX").and_then(tmux::socket_from_env);
    (socket, get("TMUX_PANE").map(String::from))
}

/// The allowlisted variables of `pid`'s environment. The buffer is consumed
/// here and never formatted (S11); refused values are reported by name.
fn allowlisted(
    pairs: Result<Vec<(String, String)>, String>,
    pid: u32,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let Ok(pairs) = pairs else {
        warnings.push(format!(
            "could not read the environment of process {pid}; no variables were recorded"
        ));
        return BTreeMap::new();
    };
    let (kept, refused) = env_record::record(pairs);
    for (name, rule) in refused {
        warnings.push(format!(
            "ignored {name} from the environment of process {pid}: {rule}"
        ));
    }
    kept
}
