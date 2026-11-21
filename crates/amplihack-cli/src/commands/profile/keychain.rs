//! The macOS Keychain lookup (issue #2885). On macOS, Claude Code keeps a
//! profile's login in the login Keychain and writes no `.credentials.json`.
//! This module asks whether a Keychain item exists for a profile directory,
//! and nothing else. See "Keychain logins" in
//! docs/reference/profile-command.md.
//!
//! The secret is never read. The one command run is
//! `security find-generic-password -s <service>`, with no option that prints
//! a password, with the three standard streams discarded, and with the exit
//! status as the whole answer. Nothing the child prints, and nothing from
//! `.claude.json`, reaches a message or a log.
//!
//! Every outcome that is not a plain "present" or "absent" is
//! [`Keychain::Unreadable`]. No function here turns a failure into "absent"
//! or "present", so a caller can never read a locked or unreachable Keychain
//! as proof of anything.
//!
//! This file reads no environment variable. The home directory is a
//! parameter, and the few variables handed on to the child are read by
//! `usage_cmd::child_vars`. No variable can change the platform or the
//! answer; the only override is the test seam at the foot of the file, which
//! exists in test builds alone.

use super::rotate::identify::Account;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Claude Code's service name for a config directory is this prefix and
/// eight hex digits; see [`service_name`].
pub const SERVICE_PREFIX: &str = "Claude Code-credentials-";

/// Found by bare name on the search path, as `claude` itself is.
const PROGRAM: &str = "security";

/// How long one lookup may take before it is abandoned.
const DEADLINE: Duration = Duration::from_secs(5);

/// How often a running lookup is checked.
const POLL: Duration = Duration::from_millis(10);

/// The variables the child is given besides `HOME`, where the process has
/// them. Everything else, API-key variables included, is withheld.
pub(super) const PASSED: [&str; 4] = ["PATH", "TMPDIR", "USER", "LOGNAME"];

/// What the Keychain said about one profile directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keychain {
    /// An item exists. Whether its login is still valid is not checked, as
    /// it is not for a login file.
    Present,
    /// The Keychain answered, and it holds no item.
    Absent,
    /// The Keychain gave no answer. Never proof of a login, and never proof
    /// of an API key.
    Unreadable(Cause),
}

impl Keychain {
    /// The outcome with no detail, for the debug log.
    fn class(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::Unreadable(_) => "unreadable",
        }
    }
}

/// Why the Keychain gave no answer. The texts are fixed and carry nothing
/// from the child or from the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// No absolute home directory is known, so nothing was started.
    NoHome,
    /// `security` is not on the search path, or could not be run.
    NotStarted,
    /// `security` ended with a status other than 0 and 44.
    Exit(i32),
    /// `security` was ended by a signal.
    Signal,
    /// `security` was still running at the deadline and was stopped.
    Timeout,
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => f.write_str("no home directory is known"),
            Self::NotStarted => f.write_str("`security` could not be started"),
            Self::Exit(n) => write!(f, "`security` exit status {n}"),
            Self::Signal => f.write_str("`security` was ended by a signal"),
            Self::Timeout => f.write_str("`security` did not answer in time"),
        }
    }
}

/// What a lookup runs in: the platform, the child's home and environment,
/// and the deadline. One `Env` belongs to one `kind::Ctx`, so to one
/// resolution of the kinds. A command that loads `kind::Ctx` more than once
/// has one `Env` for each.
///
/// `Debug` is left out on purpose: the home and the search path are private
/// paths and have no place in a log.
pub struct Env {
    macos: bool,
    home: Option<PathBuf>,
    /// The child's environment besides `HOME`.
    vars: Vec<(&'static str, OsString)>,
    deadline: Duration,
    /// Set by the first lookup that ran out of time. Every later lookup in
    /// this `Env` then answers [`Cause::Timeout`] without starting
    /// `security`, so a hanging Keychain costs one deadline per `Env` and
    /// not one per profile. A command with several pays once for each. It
    /// only ever moves towards a refusal.
    timed_out: Cell<bool>,
}

impl Env {
    /// The value for this process: macOS only on macOS, the process's own
    /// search path, and a five second deadline. `home` becomes the child's
    /// `HOME`.
    ///
    /// In a test build the platform reads as not macOS unless the calling
    /// thread holds a [`test_seam`], so no unit test starts the system
    /// `security` on any host.
    pub fn ambient(home: Option<&Path>) -> Self {
        #[cfg(test)]
        {
            match seam::held() {
                Some((path, deadline)) => Self::fake(true, home, &path, deadline),
                None => Self::new(false, home, Vec::new(), DEADLINE),
            }
        }
        #[cfg(not(test))]
        {
            let macos = cfg!(target_os = "macos");
            // Off macOS nothing is ever started, so nothing is read.
            let vars = if macos {
                super::usage_cmd::child_vars(&PASSED)
            } else {
                Vec::new()
            };
            Self::new(macos, home, vars, DEADLINE)
        }
    }

    fn new(
        macos: bool,
        home: Option<&Path>,
        vars: Vec<(&'static str, OsString)>,
        deadline: Duration,
    ) -> Self {
        Self {
            macos,
            home: home.map(Path::to_path_buf),
            vars,
            deadline,
            timed_out: Cell::new(false),
        }
    }

    /// Any value a test needs: `path` is the child's whole search path.
    #[cfg(test)]
    pub fn fake(macos: bool, home: Option<&Path>, path: &Path, deadline: Duration) -> Self {
        Self::new(
            macos,
            home,
            vec![("PATH", path.as_os_str().to_os_string())],
            deadline,
        )
    }

    /// Whether Keychain lookups are made at all.
    pub fn macos(&self) -> bool {
        self.macos
    }
}

/// The Keychain service Claude Code uses for the config directory `dir`:
/// [`SERVICE_PREFIX`] and the first eight lower-case hex digits of the
/// SHA-256 of the path's raw bytes. The path is not normalised in any way,
/// so the caller passes exactly the path it exports as `CLAUDE_CONFIG_DIR`.
///
/// The rule is Claude Code's, not amplihack's, and may change between its
/// versions. It lives in this one function, pinned by two vectors in
/// `tests_keychain.rs` that were computed with `shasum`.
pub fn service_name(dir: &Path) -> String {
    let digest = Sha256::digest(dir.as_os_str().as_bytes());
    let mut name = String::from(SERVICE_PREFIX);
    for byte in &digest[..4] {
        // Writing to a `String` cannot fail.
        let _ = write!(name, "{byte:02x}");
    }
    name
}

/// Asks the Keychain whether an item named `service` exists.
///
/// Nothing is started, and the answer is [`Keychain::Unreadable`], when the
/// home is unknown or not absolute, or when an earlier lookup in `env` ran
/// out of time. Otherwise `security` is started once and waited for until
/// the deadline; at the deadline that one child is killed through its own
/// handle and reaped.
///
/// The lookup is never retried: the exit status is the answer, and a second
/// start could raise a second dialog. The rotator's stop flag is not read
/// either, so a stop waits for a lookup in progress, one deadline at most.
pub fn probe(env: &Env, service: &str) -> Keychain {
    let Some(home) = env.home.as_deref().filter(|h| h.is_absolute()) else {
        return Keychain::Unreadable(Cause::NoHome);
    };
    if env.timed_out.get() {
        return Keychain::Unreadable(Cause::Timeout);
    }
    let started = Command::new(PROGRAM)
        .args(["find-generic-password", "-s", service])
        .env_clear()
        .env("HOME", home)
        .envs(env.vars.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match started {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(op = "keychain lookup", error = %error.kind(), "not started");
            return Keychain::Unreadable(Cause::NotStarted);
        }
    };
    let end = Instant::now() + env.deadline;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return answer(status),
            Ok(None) if Instant::now() < end => std::thread::sleep(POLL),
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(op = "keychain lookup", error = %error.kind(), "wait failed");
                break;
            }
        }
    }
    // Out of time, or the child could not be waited for: stop this child
    // and nothing else. A child that has just ended refuses the kill, which
    // is harmless; the wait below reaps it either way.
    if let Err(error) = child.kill() {
        tracing::debug!(op = "keychain lookup", error = %error.kind(), "kill failed");
    }
    if let Err(error) = child.wait() {
        tracing::debug!(op = "keychain lookup", error = %error.kind(), "not reaped");
    }
    env.timed_out.set(true);
    Keychain::Unreadable(Cause::Timeout)
}

/// The exit status is the whole answer: 0 is an item, 44 is no item, and
/// everything else is no answer at all.
fn answer(status: ExitStatus) -> Keychain {
    match status.code() {
        Some(0) => Keychain::Present,
        Some(44) => Keychain::Absent,
        Some(other) => Keychain::Unreadable(Cause::Exit(other)),
        None => Keychain::Unreadable(Cause::Signal),
    }
}

/// The one detection function: what the Keychain says about the profile
/// directory `dir`, or `None` when it is not asked.
///
/// It is not asked off macOS, and it is not asked when `dir/.claude.json`
/// names no identifiable account, since an item could not then be tied to
/// the profile. In both cases nothing is started. The caller decides the
/// other precondition, that the profile has no login file.
pub fn detect(env: &Env, dir: &Path) -> Option<Keychain> {
    if !env.macos {
        return None;
    }
    // Only whether an account is named is used; none of its values is kept.
    Account::of_dir(dir)?;
    let found = probe(env, &service_name(dir));
    tracing::debug!(op = "keychain lookup", outcome = found.class());
    Some(found)
}

#[cfg(test)]
pub use seam::{test_seam, test_seam_deadline};

/// The test seam. It exists in test builds only, belongs to one thread, and
/// is undone when its guard is dropped.
#[cfg(test)]
mod seam {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    type Held = Option<(PathBuf, Duration)>;

    thread_local! {
        static SEAM: RefCell<Held> = const { RefCell::new(None) };
    }

    /// Puts back what was held before, when dropped.
    pub struct Seam {
        before: Held,
    }

    impl Drop for Seam {
        fn drop(&mut self) {
            let before = self.before.take();
            SEAM.with(|s| *s.borrow_mut() = before);
        }
    }

    /// A test's fake `security` is a script written a moment before, and
    /// macOS checks a new script on its first run, which takes seconds on a
    /// busy host. The seam therefore waits far longer than the real
    /// deadline; a test of the deadline itself names its own.
    const PATIENT: Duration = Duration::from_secs(120);

    /// While the guard lives, `Env::ambient` on this thread reads as macOS
    /// and finds `security` in `path` alone.
    pub fn test_seam(path: &Path) -> Seam {
        test_seam_deadline(path, PATIENT)
    }

    /// [`test_seam`] with a deadline short enough to test a hanging lookup.
    pub fn test_seam_deadline(path: &Path, deadline: Duration) -> Seam {
        let before = SEAM.with(|s| s.borrow_mut().replace((path.to_path_buf(), deadline)));
        Seam { before }
    }

    pub(super) fn held() -> Held {
        SEAM.with(|s| s.borrow().clone())
    }
}
