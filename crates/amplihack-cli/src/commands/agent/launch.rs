//! The command a resumed tmux pane runs (step 5b section 7, S16): `env`
//! removes the nested-session and launcher variables (D6, A3) and sets the
//! recorded ones, then `amplihack claude --supervise NAME` resumes the
//! conversation with the recorded options and the constant message (R5.7,
//! S23). Every element is one argv entry; there is no shell.

use super::registry::Entry;
use amplihack_utils::agent_binary::SESSION_MARKERS;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const EXE_REFUSED: &str = "the amplihack binary path must be absolute and UTF-8, without '=' or \
                           control characters, for env to run it; nothing was started";

/// Launcher and nesting variables removed in the pane, after the session
/// markers. `TMUX` and `TMUX_PANE` are kept: the pane's are right (D6).
const LAUNCHER_NAMES: &[&str] = &[
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_PROJECT_DIR",
    "AMPLIHACK_SESSION_ID",
    "AMPLIHACK_DEPTH",
    "AMPLIHACK_SESSION_DEPTH",
    "AMPLIHACK_TREE_ID",
    "CLAUDE_CONFIG_DIR",
    "AMPLIHACK_CLAUDE_PROFILE",
];

/// The `env -u` names: every `SESSION_MARKERS` name in constant order, then
/// [`LAUNCHER_NAMES`], first occurrence kept.
pub fn pane_unset() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for name in SESSION_MARKERS
        .iter()
        .map(|(n, _)| *n)
        .chain(LAUNCHER_NAMES.iter().copied())
    {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The only text the supervisor puts into a conversation (S23).
pub fn message(lost_at: &str, now: &str) -> String {
    format!(
        "amplihack agent supervisor: the terminal session for this agent was lost at {lost_at}. \
         This conversation was resumed at {now}. Before continuing, check what your interrupted \
         step had already done (git status, open pull requests, running processes) and do not \
         repeat finished steps."
    )
}

/// The pane argv for agent `name`. The message comes before `--add-dir`,
/// which takes every following value up to the next option.
pub fn pane_command(name: &str, e: &Entry, exe: &Path, msg: &str) -> Vec<String> {
    let mut argv = vec!["env".to_string()];
    for n in pane_unset() {
        argv.extend(["-u".to_string(), n.to_string()]);
    }
    argv.extend(e.env.iter().map(|(k, v)| format!("{k}={v}")));
    argv.push(exe.to_string_lossy().into_owned());
    argv.push("claude".into());
    let flags = &e.amplihack_flags;
    if flags.subprocess_safe {
        argv.push("--subprocess-safe".into());
    }
    if flags.no_reflection {
        argv.push("--no-reflection".into());
    }
    if let Some(p) = &flags.profile {
        argv.extend(["--profile".to_string(), p.clone()]);
    }
    argv.extend([
        "--supervise".to_string(),
        name.to_string(),
        "--".to_string(),
    ]);
    if let Some(id) = &e.conversation_id {
        argv.extend(["--resume".to_string(), id.clone()]);
    }
    argv.push(msg.to_string());
    if let Some(m) = &e.model {
        argv.extend(["--model".to_string(), m.clone()]);
    }
    if let Some(eff) = &e.effort {
        argv.extend(["--effort".to_string(), eff.clone()]);
    }
    for d in &e.add_dirs {
        argv.extend(["--add-dir".to_string(), d.clone()]);
    }
    if let Some(mode) = &e.permission_mode {
        argv.extend(["--permission-mode".to_string(), mode.clone()]);
    } else if e.skip_permissions {
        argv.push("--dangerously-skip-permissions".into());
    }
    argv
}

/// The Claude arguments of a standing session's relaunch (issue #2912):
/// `--continue`, then the options recorded for the session, in the order
/// [`pane_command`] uses. The conversation is never named: `--continue`
/// picks the most recent one for the current directory in the config
/// directory, which is the behaviour the issue asks for. The wrapper never
/// changes its working directory, so that is the directory of the original
/// launch. No message is added, because the conversation carries on rather
/// than being resumed after a loss.
///
/// Each value-taking flag is parsed by Claude Code in the `--flag=value`
/// form exactly as in the `--flag value` form (checked against Claude Code
/// 2.1.330), and the `=` form also stops the variadic `--add-dir` from
/// taking any argument that follows it.
///
/// Rebuilt from the registry record rather than filtered out of the
/// original argv, so no flag parser is involved and the relaunch can carry
/// no option the launch did not record. A permission mode wins over
/// `--dangerously-skip-permissions` (I4), as it does at registration, so
/// the relaunch never has more privilege than the launch.
///
/// Every value-taking flag is emitted as `--flag=value`, and a recorded
/// value that begins with `-` is dropped with a debug line rather than
/// passed on: it could otherwise be read as another flag (V1). The record
/// is re-read from disk before this runs, so it is validated at the moment
/// it becomes a command line.
pub fn continue_args(e: &Entry) -> Vec<String> {
    let mut argv = vec!["--continue".to_string()];
    if let Some(m) = &e.model {
        push_opt(&mut argv, "--model", m);
    }
    if let Some(eff) = &e.effort {
        push_opt(&mut argv, "--effort", eff);
    }
    for d in &e.add_dirs {
        push_opt(&mut argv, "--add-dir", d);
    }
    if let Some(mode) = &e.permission_mode {
        push_opt(&mut argv, "--permission-mode", mode);
    } else if e.skip_permissions {
        argv.push("--dangerously-skip-permissions".into());
    }
    argv
}

/// One `--flag=value` for [`continue_args`], or nothing at all when the
/// recorded value begins with `-` and could be read as another flag.
fn push_opt(argv: &mut Vec<String>, flag: &str, value: &str) {
    if value.starts_with('-') {
        tracing::debug!("agent: {flag} is left out of the relaunch: its value begins with -");
        return;
    }
    argv.push(format!("{flag}={value}"));
}

/// The binary path must be absolute, UTF-8 and free of `=` and control
/// characters: `env` would read a path holding `=` as an assignment (S16).
pub fn check_exe(p: &Path) -> Result<(), String> {
    let ok = p.is_absolute()
        && p.to_str()
            .is_some_and(|s| !s.contains('=') && !s.chars().any(char::is_control));
    if ok { Ok(()) } else { Err(EXE_REFUSED.into()) }
}

/// `raw` without a trailing ` (deleted)` when the plain path exists (D22),
/// then [`check_exe`].
pub fn resolve_exe(raw: &Path) -> Result<PathBuf, String> {
    let bytes = raw.as_os_str().as_bytes();
    let path = match bytes.strip_suffix(b" (deleted)") {
        Some(plain) if Path::new(std::ffi::OsStr::from_bytes(plain)).exists() => {
            PathBuf::from(std::ffi::OsStr::from_bytes(plain))
        }
        _ => raw.to_path_buf(),
    };
    check_exe(&path)?;
    Ok(path)
}

/// This binary's path, computed once when the supervisor starts (S16).
pub fn amplihack_exe() -> anyhow::Result<PathBuf> {
    let raw = std::env::current_exe().map_err(|e| {
        anyhow::anyhow!("cannot find the amplihack binary: {e}; nothing was started")
    })?;
    resolve_exe(&raw).map_err(anyhow::Error::msg)
}
