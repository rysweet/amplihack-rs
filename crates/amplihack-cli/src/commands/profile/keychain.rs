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
//! The lookup is one blocking wait on the child. There is no deadline, no
//! sleep and no polling loop: the answer comes when `security` ends. A
//! `security` that waits on a Keychain dialog holds the caller until the
//! dialog is answered, and supervising the whole command is the caller's
//! business, not this module's. So that a held lookup can be seen, a debug
//! line is logged before `security` is started; see [`probe`].
//!
//! This file reads no environment variable. The home directory is a
//! parameter, and the few variables handed on to the child are read by
//! `usage_cmd::child_vars`. No variable can change the platform or the
//! answer; the only override is the test seam at the foot of the file, which
//! exists in test builds alone.

use super::rotate::identify::Account;
use icu_normalizer::ComposingNormalizerBorrowed;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// Claude Code's service name for a config directory is this prefix and
/// eight hex digits; see [`service_name`].
pub const SERVICE_PREFIX: &str = "Claude Code-credentials-";

/// Found by bare name on the search path, as `claude` itself is.
const PROGRAM: &str = "security";

/// The variables the child is given besides `HOME`, where the process has
/// them. Everything else, API-key variables included, is withheld.
pub(super) const PASSED: [&str; 4] = ["PATH", "TMPDIR", "USER", "LOGNAME"];

/// The debug line [`probe`] logs before it starts `security`, so that a
/// lookup held by a Keychain dialog leaves evidence of where it is.
pub(super) const WAIT_NOTE: &str =
    "starting security; the lookup waits for it to end, with no time limit";

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
    /// `security` was started, but waiting for it failed, so it gave no
    /// exit status. It is not killed: a child that cannot be waited for is
    /// no longer this process's to signal.
    Wait,
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => f.write_str("no home directory is known"),
            Self::NotStarted => f.write_str("`security` could not be started"),
            Self::Exit(n) => write!(f, "`security` exit status {n}"),
            Self::Signal => f.write_str("`security` was ended by a signal"),
            Self::Wait => f.write_str("`security` could not be waited for"),
        }
    }
}

/// What a lookup runs in: the platform and the child's home and
/// environment. One `Env` belongs to one `kind::Ctx`, so to one resolution
/// of the kinds. A command that loads `kind::Ctx` more than once has one
/// `Env` for each. An `Env` keeps no state between lookups, so each lookup
/// is answered by its own `security` and the order of lookups never changes
/// an answer.
///
/// `Debug` is left out on purpose: the home and the search path are private
/// paths and have no place in a log.
pub struct Env {
    macos: bool,
    home: Option<PathBuf>,
    /// The child's environment besides `HOME`.
    vars: Vec<(&'static str, OsString)>,
}

impl Env {
    /// The value for this process: macOS only on macOS, and the process's
    /// own search path. `home` becomes the child's `HOME`.
    ///
    /// In a test build the platform reads as not macOS unless the calling
    /// thread holds a [`test_seam`], so no unit test starts the system
    /// `security` on any host.
    pub fn ambient(home: Option<&Path>) -> Self {
        #[cfg(test)]
        {
            match seam::held() {
                Some(path) => Self::fake(true, home, &path),
                None => Self::new(false, home, Vec::new()),
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
            Self::new(macos, home, vars)
        }
    }

    fn new(macos: bool, home: Option<&Path>, vars: Vec<(&'static str, OsString)>) -> Self {
        Self {
            macos,
            home: home.map(Path::to_path_buf),
            vars,
        }
    }

    /// Any value a test needs: `path` is the child's whole search path.
    #[cfg(test)]
    pub fn fake(macos: bool, home: Option<&Path>, path: &Path) -> Self {
        Self::new(macos, home, vec![("PATH", path.as_os_str().to_os_string())])
    }

    /// Whether Keychain lookups are made at all.
    pub fn macos(&self) -> bool {
        self.macos
    }
}

/// The Keychain service Claude Code uses for the config directory `dir`:
/// [`SERVICE_PREFIX`] and the first eight lower-case hex digits of the
/// SHA-256 of the UTF-8 bytes of the path in Unicode Normalization Form C
/// (see `nfc_bytes`). Nothing else is done to the path: no trailing slash is
/// removed and no component is resolved, so the caller passes exactly the
/// path it exports as `CLAUDE_CONFIG_DIR`.
///
/// The rule is Claude Code's, not amplihack's, and may change between its
/// versions. In the bundle of Claude Code 2.1.330 the config directory is
/// `(process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude"))
/// .normalize("NFC")`, and the service suffix is
/// `createHash("sha256").update(dir).digest("hex").substring(0, 8)` over
/// that value; `update` encodes a string as UTF-8. The rule lives in this
/// one function, pinned by vectors in `tests_keychain.rs` that were
/// computed with `shasum` and checked with Node's `normalize("NFC")`.
pub fn service_name(dir: &Path) -> String {
    let digest = Sha256::digest(nfc_bytes(dir));
    let mut name = String::from(SERVICE_PREFIX);
    for byte in &digest[..4] {
        // Writing to a `String` cannot fail.
        let _ = write!(name, "{byte:02x}");
    }
    name
}

/// The bytes [`service_name`] hashes: the path in Normalization Form C, as
/// UTF-8. A path already in NFC, which includes every ASCII path, is its own
/// bytes unchanged. A decomposed name, such as `cafe` followed by U+0301
/// where Claude Code sees `café`, becomes the composed form, so both
/// spellings of one directory name one item.
///
/// A path that is not valid UTF-8 is hashed as its raw bytes. That case does
/// not arise where the lookup runs: the macOS file systems store names as
/// UTF-8 only.
fn nfc_bytes(dir: &Path) -> Cow<'_, [u8]> {
    let raw = dir.as_os_str().as_bytes();
    let Ok(text) = std::str::from_utf8(raw) else {
        return Cow::Borrowed(raw);
    };
    match ComposingNormalizerBorrowed::new_nfc().normalize(text) {
        Cow::Borrowed(_) => Cow::Borrowed(raw),
        Cow::Owned(composed) => Cow::Owned(composed.into_bytes()),
    }
}

/// Asks the Keychain whether an item named `service` exists.
///
/// Nothing is started, and the answer is [`Keychain::Unreadable`], when the
/// home is unknown or not absolute. Otherwise `security` is started once
/// and waited for with one blocking wait until it ends. No deadline is set,
/// nothing sleeps and nothing polls: a `security` that waits on a Keychain
/// dialog holds the caller until the dialog is answered, and the answer is
/// then whatever its exit status says.
///
/// Before `security` is started, one debug line, `WAIT_NOTE`, is logged
/// with `op="keychain lookup"`. It is written before the child exists, so a
/// command held by a Keychain dialog and run with `RUST_LOG=debug` shows it
/// as its last line about the lookup. It names neither the service nor the
/// home. Bounding the wait is left to whatever started the command. No layer
/// does that yet; issue #2968 records which one should.
///
/// The lookup is never retried: the exit status is the answer, and a second
/// start could raise a second dialog. The rotator's stop flag is not read
/// either, so a stop waits for a lookup in progress to end.
pub fn probe(env: &Env, service: &str) -> Keychain {
    let Some(home) = env.home.as_deref().filter(|h| h.is_absolute()) else {
        return Keychain::Unreadable(Cause::NoHome);
    };
    tracing::debug!(op = "keychain lookup", "{WAIT_NOTE}");
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
    // One blocking wait, which also reaps the child. It fails only when
    // the child is not this process's to wait for (`ECHILD`); a kill then
    // could reach another process that has taken the pid, so none is sent.
    match child.wait() {
        Ok(status) => answer(status),
        Err(error) => {
            tracing::debug!(op = "keychain lookup", error = %error.kind(), "wait failed");
            Keychain::Unreadable(Cause::Wait)
        }
    }
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
pub use seam::test_seam;

/// The test seam. It exists in test builds only, belongs to one thread, and
/// is undone when its guard is dropped.
#[cfg(test)]
mod seam {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    type Held = Option<PathBuf>;

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

    /// While the guard lives, `Env::ambient` on this thread reads as macOS
    /// and finds `security` in `path` alone.
    pub fn test_seam(path: &Path) -> Seam {
        let before = SEAM.with(|s| s.borrow_mut().replace(path.to_path_buf()));
        Seam { before }
    }

    pub(super) fn held() -> Held {
        SEAM.with(|s| s.borrow().clone())
    }
}
