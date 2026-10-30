//! The only module that runs tmux. Every call takes argv only (no shell),
//! a null stdin, a 10 s timeout and output capped at 64 KiB per stream
//! (S15); the client environment loses the session markers (R5.6, A3); and
//! with `-S` the socket checks run first (S12). Nothing here kills a
//! server, session or pane.

use super::report::sanitize;
use super::socket_check::{self, NOT_PRIVATE, SocketCheck};
use super::validate;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: usize = 64 * 1024;
const MAX_LINE: usize = 200;
const SOCKET_REFUSED: &str =
    "tmux socket: its directory must be owned by you and closed to other users";
const UNEXPECTED: &str = "tmux display-message returned an unexpected answer";
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
    /// tmux's exit status and stderr.
    pub fn has_session(&self, session: &str) -> SessionState {
        if let Some(state) = self.socket.as_deref().and_then(classify_socket) {
            return state;
        }
        let mut cmd = self.command();
        cmd.args(["has-session", "-t"]).arg(format!("={session}"));
        match run(cmd) {
            Ok(out) => read_has_session(out.status.code(), &String::from_utf8_lossy(&out.stderr)),
            Err(reason) => read_has_session(None, &reason),
        }
    }

    /// `display-message -p -t TARGET FORMAT`: tmux's stdout. The error is the
    /// whole one-line message.
    pub fn display(&self, target: &str, format: &str) -> Result<String, String> {
        if let Some(socket) = &self.socket {
            match socket_check::check(socket) {
                SocketCheck::NotPrivate => return Err(SOCKET_REFUSED.into()),
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

    /// `new-session -d -s SESSION -n WINDOW -c DIR -- COMMAND...`. The
    /// socket's missing directories are created first (D10). The error is
    /// the text that follows `failed: tmux: `.
    pub fn new_session(
        &self,
        session: &str,
        window: &str,
        dir: &Path,
        command: &[String],
    ) -> Result<(), String> {
        if command.len() < 2 {
            // tmux would hand a single argument to a shell (S16).
            return Err("the pane command must have more than one argument".into());
        }
        if let Some(socket) = &self.socket {
            socket_check::prepare(socket)?;
        }
        let mut cmd = self.command();
        cmd.args(["new-session", "-d", "-s", session, "-n", window, "-c"])
            .arg(dir)
            .arg("--")
            .args(command);
        let out = run(cmd)?;
        if out.status.success() {
            Ok(())
        } else {
            Err(failure_or_status(&out))
        }
    }

    /// `tmux [-S SOCKET]` with a null stdin and the client removals.
    fn command(&self) -> Command {
        let mut cmd = Command::new("tmux");
        if let Some(socket) = &self.socket {
            cmd.arg("-S").arg(socket);
        }
        cmd.stdin(Stdio::null());
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(client_drops) {
                cmd.env_remove(name);
            }
        }
        cmd
    }
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

/// A8 rows 1 to 3 for a recorded socket, decided without tmux; `None`
/// means tmux must be asked.
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

/// A8 rows 4 to 6. `exit` is `None` for a spawn error, a timeout or a
/// signal; tmux's messages are not translated.
pub fn read_has_session(exit: Option<i32>, stderr: &str) -> SessionState {
    const GONE: [&str; 3] = [
        "can't find session",
        "no server running",
        "session not found",
    ];
    match exit {
        Some(0) => return SessionState::Present,
        Some(1) if GONE.iter().any(|g| stderr.contains(g)) => return SessionState::Absent,
        _ => {}
    }
    let line = failure_line(stderr.as_bytes());
    let reason = match (line.is_empty(), exit) {
        (false, _) => line,
        (true, Some(code)) => format!("tmux exited with status {code}"),
        (true, None) => "tmux could not be run".to_string(),
    };
    SessionState::Unknown(reason)
}

/// `SESSION<TAB>WINDOW` and at most a trailing newline, both names valid
/// (S13). Anything else is an error, never a partial value.
pub fn parse_names(out: &str) -> Result<(String, String), String> {
    let body = out.strip_suffix('\n').unwrap_or(out);
    let mut parts = body.split('\t');
    let (Some(session), Some(window), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(UNEXPECTED.into());
    };
    if body.contains('\n') {
        return Err(UNEXPECTED.into());
    }
    validate::session_name(session).map_err(|rule| format!("tmux session name {rule}"))?;
    validate::window_name(window).map_err(|rule| format!("tmux window name {rule}"))?;
    Ok((session.to_string(), window.to_string()))
}

/// A decimal pid and at most a trailing newline (S13).
pub fn parse_pane_pid(out: &str) -> Result<u32, String> {
    let body = out.strip_suffix('\n').unwrap_or(out);
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return Err(UNEXPECTED.into());
    }
    body.parse::<u32>()
        .ok()
        .filter(|&pid| pid > 0)
        .ok_or_else(|| UNEXPECTED.to_string())
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
