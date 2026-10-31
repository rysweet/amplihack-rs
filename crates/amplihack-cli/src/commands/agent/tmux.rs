//! The only module that runs tmux, apart from `server.rs`, which runs the
//! `new-session` built here under `systemd-run`. Every call takes argv only (no shell),
//! a null stdin, a 10 s timeout and output capped at 64 KiB per stream
//! (S15); the client environment loses the session markers (R5.6, A3); and
//! with `-S` the socket checks run first (S12). Nothing here kills a
//! server, session or pane.

use super::report::sanitize;
use super::socket_check::{self, NOT_PRIVATE, REFUSED, SocketCheck};
use super::tmux_parse::UNEXPECTED;
pub use super::tmux_parse::{parse_names, parse_pane_pid, read_has_session, read_list_sessions};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: usize = 64 * 1024;
const MAX_LINE: usize = 200;
const NO_SERVER: &str = "tmux display-message failed: no tmux server is running on the socket";

/// Removed from the environment of every tmux client, with every name
/// starting `COPILOT_` or `GITHUB_COPILOT`.
pub const CLIENT_REMOVE: &[&str] = &[
    "TMUX",
    "TMUX_PANE",
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_PROJECT_DIR",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CLAUDE_CONFIG_DIR",
    "AMPLIHACK_CLAUDE_PROFILE",
];

pub fn client_drops(name: &str) -> bool {
    CLIENT_REMOVE.contains(&name)
        || name.starts_with("COPILOT_")
        || name.starts_with("GITHUB_COPILOT")
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Present,
    Absent,
    /// Counts as present; holds the reason.
    Unknown(String),
}

/// A tmux server: the recorded socket, or tmux's default without `-S`.
#[derive(Debug, Clone)]
pub struct Tmux {
    socket: Option<PathBuf>,
}

impl Tmux {
    pub fn new(socket: Option<&Path>) -> Self {
        Self {
            socket: socket.map(Path::to_path_buf),
        }
    }

    /// `has-session -t =SESSION`, read as A8: the socket rows first, then
    /// tmux's exit status and stderr. Without a recorded socket the rows use
    /// tmux's default, which a client without `-S` connects to because
    /// `TMUX` is removed from its environment.
    pub fn has_session(&self, session: &str) -> SessionState {
        let socket = self.socket.clone().unwrap_or_else(default_socket);
        if let Some(state) = classify_socket(&socket) {
            return state;
        }
        let mut cmd = self.command();
        cmd.args(["has-session", "-t"]).arg(format!("={session}"));
        match run(cmd) {
            Ok(out) => read_has_session(out.status.code(), &String::from_utf8_lossy(&out.stderr)),
            Err(reason) => read_has_session(None, &reason),
        }
    }

    /// `list-sessions`, read for server placement (G1): the socket rows of
    /// A8 first, then exit 0 is a running server and exit 1 with
    /// `no server running` none; anything else is unknown.
    pub fn server_state(&self) -> SessionState {
        let socket = self.socket.clone().unwrap_or_else(default_socket);
        if let Some(state) = classify_socket(&socket) {
            return state;
        }
        let mut cmd = self.command();
        cmd.arg("list-sessions");
        match run(cmd) {
            Ok(out) => read_list_sessions(out.status.code(), &String::from_utf8_lossy(&out.stderr)),
            Err(reason) => read_list_sessions(None, &reason),
        }
    }

    /// `display-message -p -t TARGET FORMAT`: tmux's stdout. The error is the
    /// whole one-line message.
    pub fn display(&self, target: &str, format: &str) -> Result<String, String> {
        if let Some(socket) = &self.socket {
            match socket_check::check(socket) {
                SocketCheck::NotPrivate => return Err(REFUSED.into()),
                SocketCheck::MissingParent => return Err(NO_SERVER.into()),
                SocketCheck::Ok => {}
            }
        }
        let mut cmd = self.command();
        cmd.args(["display-message", "-p", "-t", target, format]);
        let out = run(cmd).map_err(|r| format!("tmux display-message failed: {r}"))?;
        if !out.status.success() {
            let line = failure_or_status(&out);
            return Err(format!("tmux display-message failed: {line}"));
        }
        String::from_utf8(out.stdout).map_err(|_| UNEXPECTED.to_string())
    }

    /// The session and window of a pane (S13).
    pub fn names(&self, target: &str) -> Result<(String, String), String> {
        parse_names(&self.display(target, "#{session_name}\t#{window_name}")?)
    }

    /// The pid of the process a pane runs (S13).
    pub fn pane_pid(&self, target: &str) -> Result<u32, String> {
        parse_pane_pid(&self.display(target, "#{pane_pid}")?)
    }

    /// The arguments after `tmux` of `[-S SOCKET] new-session -d -s SESSION
    /// -n WINDOW -c DIR -- COMMAND...`. The socket's missing directories are
    /// created first (D10). The error is the text that follows
    /// `failed: tmux: `.
    pub fn new_session_args(
        &self,
        session: &str,
        window: &str,
        dir: &Path,
        command: &[String],
    ) -> Result<Vec<OsString>, String> {
        if command.len() < 2 {
            // tmux would hand a single argument to a shell (S16).
            return Err("the pane command must have more than one argument".into());
        }
        let mut args: Vec<OsString> = Vec::new();
        if let Some(socket) = &self.socket {
            socket_check::prepare(socket)?;
            args.extend(["-S".into(), socket.into()]);
        }
        let head = ["new-session", "-d", "-s", session, "-n", window, "-c"];
        args.extend(head.map(OsString::from));
        args.extend([dir.into(), "--".into()]);
        args.extend(command.iter().map(OsString::from));
        Ok(args)
    }

    /// Plain `tmux new-session`, with [`Tmux::new_session_args`].
    pub fn new_session(
        &self,
        session: &str,
        window: &str,
        dir: &Path,
        command: &[String],
    ) -> Result<(), String> {
        let mut cmd = client(OsStr::new("tmux"));
        cmd.args(self.new_session_args(session, window, dir, command)?);
        let out = run(cmd)?;
        if out.status.success() {
            Ok(())
        } else {
            Err(failure_or_status(&out))
        }
    }

    /// `tmux [-S SOCKET]` with a null stdin and the client removals.
    fn command(&self) -> Command {
        let mut cmd = client(OsStr::new("tmux"));
        if let Some(socket) = &self.socket {
            cmd.arg("-S").arg(socket);
        }
        cmd
    }
}

/// `program` with a null stdin and the client removals (R5.6, A3).
pub fn client(program: &OsStr) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(client_drops) {
            cmd.env_remove(name);
        }
    }
    cmd
}

/// Runs `cmd` with the bounds of S15; a spawn error or timeout is the
/// sanitised reason.
fn run(cmd: Command) -> Result<Output, String> {
    crate::util::run_output_with_timeout_limited(cmd, TIMEOUT, MAX_OUTPUT)
        .map_err(|e| sanitize(&format!("{e:#}"), MAX_LINE))
}

fn failure_or_status(out: &Output) -> String {
    let line = failure_line(&out.stderr);
    if !line.is_empty() {
        return line;
    }
    match out.status.code() {
        Some(code) => format!("tmux exited with status {code}"),
        None => "tmux was ended by a signal".to_string(),
    }
}

/// A8 rows 1 to 3 for a socket, decided without tmux; `None` means tmux
/// must be asked.
pub fn classify_socket(socket: &Path) -> Option<SessionState> {
    match socket_check::check(socket) {
        SocketCheck::NotPrivate => Some(SessionState::Unknown(NOT_PRIVATE.into())),
        SocketCheck::MissingParent => Some(SessionState::Absent),
        SocketCheck::Ok => match std::fs::symlink_metadata(socket) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(SessionState::Absent),
            _ => None,
        },
    }
}

/// The first line of tmux's stderr, sanitised, at most 200 characters.
pub fn failure_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let first = text.lines().next().unwrap_or("").trim_end_matches('\r');
    sanitize(first, MAX_LINE)
}

/// tmux's default socket for this process: `${TMUX_TMPDIR:-/tmp}/tmux-UID/default`.
pub fn default_socket() -> PathBuf {
    default_socket_for(
        std::env::var_os("TMUX_TMPDIR").as_deref(),
        amplihack_utils::secure_files::euid(),
    )
}

/// The default socket for a `TMUX_TMPDIR` value; empty or unset is `/tmp`.
pub fn default_socket_for(tmux_tmpdir: Option<&OsStr>, uid: u32) -> PathBuf {
    let base = tmux_tmpdir
        .filter(|d| !d.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    base.join(format!("tmux-{uid}")).join("default")
}

/// The socket in a `$TMUX` value: its first comma-separated field.
pub fn socket_from_env(tmux_value: &str) -> Option<PathBuf> {
    let first = tmux_value.split(',').next()?;
    (!first.is_empty()).then(|| PathBuf::from(first))
}
