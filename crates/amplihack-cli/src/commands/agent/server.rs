//! Server placement (G1). Before the first resume on each tmux socket in a
//! pass the supervisor asks whether a tmux server runs there. When none
//! does, `new-session` runs through `systemd-run --user --scope`, so the
//! server it starts lives in a scope unit of its own and outlives a stop or
//! restart of the supervisor service. `resume.rs` only calls
//! [`Placement::launch`]. Everything fails closed (P3): a server or session
//! state that cannot be read never leads to a second launch.

use super::report::sanitize;
use super::tmux::{self, SessionState, Tmux};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: usize = 64 * 1024;
const MAX_LINE: usize = 200;
pub const MISSING: &str = "systemd-run is not on PATH";
pub const TIMED_OUT: &str = "systemd-run did not finish within 10 s";

/// What one `systemd-run` call did.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// Exit 0: tmux ran in the scope and created the session.
    Started,
    /// No `systemd-run` on `PATH`; nothing was run.
    Missing,
    /// A non-zero exit, a timeout or a signal; holds the WARNING reason.
    Failed(String),
}

/// How one launch ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Launched {
    /// The session exists. `warning` is the reason the server was started
    /// without its own scope, when it was.
    Started { warning: Option<String> },
    /// tmux refused; the text after `failed: tmux: `.
    Failed(String),
    /// A state could not be read, so nothing more was started. `attempted`
    /// is true when `systemd-run` had already run and may have launched.
    Unknown { reason: String, attempted: bool },
}

/// The calls one launch makes. [`Real`] runs them; tests fake them.
pub trait Launcher {
    /// Whether a server runs on the agent's socket (`list-sessions`).
    fn server_state(&self) -> SessionState;
    /// `new-session` through `systemd-run` in `unit`. `Err` is a refusal
    /// from the tmux checks, made before anything ran.
    fn scoped(&self, unit: &str) -> Result<Scope, String>;
    /// Whether the agent's session exists (`has-session`).
    fn has_session(&self) -> SessionState;
    /// Plain `tmux new-session`.
    fn plain(&self) -> Result<(), String>;
}

/// The placement state of one pass: the sockets already checked, with the
/// reason when their server state was unknown, and the scope counter.
#[derive(Debug, Default)]
pub struct Placement {
    checked: HashMap<Option<PathBuf>, Option<String>>,
    units: u32,
}

impl Placement {
    /// Starts the agent's session on `socket` (`None` is tmux's default).
    /// Only the first launch on a socket in the pass checks its server;
    /// later ones use plain tmux, or are skipped like the first when its
    /// state was unknown.
    pub fn launch(&mut self, socket: Option<&Path>, now: i64, l: &dyn Launcher) -> Launched {
        let key = socket.map(Path::to_path_buf);
        if let Some(seen) = self.checked.get(&key) {
            return match seen {
                Some(reason) => unknown(reason.clone(), false),
                None => plain(l, None),
            };
        }
        let state = l.server_state();
        let reason = match &state {
            SessionState::Unknown(r) => Some(r.clone()),
            _ => None,
        };
        self.checked.insert(key, reason);
        match state {
            SessionState::Present => plain(l, None),
            SessionState::Unknown(reason) => unknown(reason, false),
            SessionState::Absent => {
                self.units += 1;
                match l.scoped(&unit_name(now, self.units)) {
                    Err(line) => Launched::Failed(line),
                    Ok(Scope::Started) => Launched::Started { warning: None },
                    Ok(Scope::Missing) => plain(l, Some(MISSING.into())),
                    // G1-S3: systemd-run can fail after tmux has started.
                    Ok(Scope::Failed(why)) => match l.has_session() {
                        SessionState::Present => Launched::Started { warning: None },
                        SessionState::Absent => plain(l, Some(why)),
                        SessionState::Unknown(reason) => unknown(reason, true),
                    },
                }
            }
        }
    }
}

fn plain(l: &dyn Launcher, warning: Option<String>) -> Launched {
    match l.plain() {
        Ok(()) => Launched::Started { warning },
        Err(line) => Launched::Failed(line),
    }
}

fn unknown(reason: String, attempted: bool) -> Launched {
    Launched::Unknown { reason, attempted }
}

/// `amplihack-tmux-server-TS-N`: the launch's Unix time and the pass's
/// scope counter, both decimal, so two scopes never share a name.
pub fn unit_name(now: i64, n: u32) -> String {
    format!("amplihack-tmux-server-{now}-{n}")
}

/// A finished `systemd-run`: exit 0 started the scope; otherwise the
/// reason is its first stderr line, its exit status or the signal.
pub fn scope_result(exit: Option<i32>, stderr: &[u8]) -> Scope {
    if exit == Some(0) {
        return Scope::Started;
    }
    let line = tmux::failure_line(stderr);
    Scope::Failed(match (line.is_empty(), exit) {
        (false, _) => format!("systemd-run: {line}"),
        (true, Some(code)) => format!("systemd-run exited {code}"),
        (true, None) => "systemd-run was ended by a signal".to_string(),
    })
}

/// The first absolute `PATH` directory holding an executable `name`.
pub fn on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// The real launch of one agent's session.
pub struct Real<'a> {
    pub tmux: &'a Tmux,
    pub session: &'a str,
    pub window: &'a str,
    pub dir: &'a Path,
    pub command: &'a [String],
}

impl Launcher for Real<'_> {
    fn server_state(&self) -> SessionState {
        self.tmux.server_state()
    }

    /// `systemd-run --user --scope --collect --quiet --unit=UNIT -- tmux
    /// ARGS`, under the tmux call rules: one argv, stdin closed, the client
    /// removals, a 10 s timeout and output capped at 64 KiB per stream.
    fn scoped(&self, unit: &str) -> Result<Scope, String> {
        let path = std::env::var_os("PATH");
        let Some(program) = on_path("systemd-run", path.as_deref()) else {
            return Ok(Scope::Missing);
        };
        let args = self
            .tmux
            .new_session_args(self.session, self.window, self.dir, self.command)?;
        let mut cmd = tmux::client(program.as_os_str());
        cmd.args(["--user", "--scope", "--collect", "--quiet"])
            .arg(format!("--unit={unit}"))
            .args(["--", "tmux"])
            .args(&args);
        Ok(
            match crate::util::run_output_with_timeout_limited(cmd, TIMEOUT, MAX_OUTPUT) {
                Ok(out) => scope_result(out.status.code(), &out.stderr),
                Err(e) => {
                    let text = format!("{e:#}");
                    if text.contains("timed out") {
                        Scope::Failed(TIMED_OUT.into())
                    } else {
                        Scope::Failed(format!("systemd-run: {}", sanitize(&text, MAX_LINE)))
                    }
                }
            },
        )
    }

    fn has_session(&self) -> SessionState {
        self.tmux.has_session(self.session)
    }

    fn plain(&self) -> Result<(), String> {
        self.tmux
            .new_session(self.session, self.window, self.dir, self.command)
    }
}
