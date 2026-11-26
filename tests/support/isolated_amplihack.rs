//! Issue #1606 (review of #1647): the one way a test starts the `amplihack`
//! binary, so that no test can read or migrate a developer's real
//! `~/.amplihack` user config.
//!
//! Shared by every test harness through `#[path]`:
//! `crates/amplihack-cli/tests/main.rs`, `bins/amplihack/tests/main.rs` and
//! `tests/integration/main.rs`. `tests/integration/isolated_amplihack_scan.rs`
//! fails when a test builds a `Command` from the binary's path without
//! [`isolated`].
//!
//! The child gets:
//! - a cleared environment with only [`KEPT_VARS`] (and `CARGO*`, `RUST*`,
//!   `LC_*`) carried over, so nothing from the developer's shell or agent
//!   session leaks in;
//! - `HOME` set to a temp directory shared by this test process (a test that
//!   needs its own sets `HOME` again afterwards);
//! - `AMPLIHACK_TEST_HOME_GUARD=1`, on this child only: with it the config
//!   loader ignores any `HOME` outside the temp dir, and the binary never
//!   autostarts the Claude login rotator;
//! - `AMPLIHACK_ROTATE_AUTOSTART=0` (issue #1690), so a launch never starts a
//!   detached `profile rotate watch --spawned` daemon that outlives the test.
//!   The guard above is the stop that still holds when a test clears the
//!   environment and sets only the guard again.
//! - no `AMPLIHACK_CLAUDE_PROFILE_ROOT` (issue #1699), so an install under the
//!   temp `HOME` cannot refresh the developer's real profiles. `env_clear()`
//!   already drops it; the explicit removal holds if the kept list changes.
//! - none of the other [`claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS`]
//!   either (issue #2922), so an inherited `CLAUDE_CONFIG_DIR` cannot make the
//!   child write its settings into a developer's live profile. As above,
//!   `env_clear()` already drops them and the explicit removal is what holds
//!   if the kept list changes.
//! - the test process's `PATH` without any directory that holds an
//!   `amplihack-asset-resolver` ([`path_without_resolver`], issue #2182), so a
//!   child never stages a host resolver found on `PATH`;
//! - `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=true` and `SSH_ASKPASS=true`
//!   ([`NO_GIT_PROMPT_ENV`], issue #1764) and a null stdin, through
//!   [`no_git_prompt`], so no git the child starts can wait on a credential
//!   prompt. `.cargo/config.toml` puts the same variables into the test
//!   process, but `env_clear()` drops them, so they are set again here. A
//!   test that calls `.env()` or `.stdin()` afterwards wins, and a test that
//!   clears another command's environment calls [`no_git_prompt`] itself
//!   after `env_clear()`.
//! - `AMPLIHACK_TEST_PROBE_WAIT=exit` ([`PROBE_WAIT_ENV`], issue #2019), so a
//!   debug build's `--version` probe of a stub waits for the stub to exit
//!   instead of giving up after 3 s on a busy host. Release builds never read
//!   it.
//! - no `AMPLIHACK_WORK_ROOT`, `AMPLIHACK_WORK_ROOT_EXPLICIT`,
//!   `AMPLIHACK_RUN_DIR` or `AMPLIHACK_RUN_ID` (issue #2712). `env_clear()`
//!   already drops them; what matters is that a test which starts a recipe
//!   shell suite starts it through [`isolated`], because a suite that
//!   inherits an explicit work root puts its worktrees under
//!   `<work root>/worktrees` instead of its own scratch, outside its `EXIT`
//!   trap. The suites clear the same names themselves by sourcing
//!   `amplifier-bundle/recipes/tests/recipe-test-env.sh`, which also clears
//!   the five `AMPLIHACK_RUN_IDENTITY_VARS` of
//!   `amplifier-bundle/tools/git-repo-env.sh`; this is the other half of that
//!   contract, for the Rust side.
//! - `AMPLIHACK_TEST_PROC_ROOT` ([`PROC_ROOT_ENV`], issue #2042) set to
//!   [`shared_empty_proc_root`], so `profile` and `sessions` commands read an
//!   empty fake process table instead of the host's `/proc`. A test that
//!   clears the environment calls [`reapply_proc_root`] after `env_clear()`;
//!   a test with its own tree sets the variable again afterwards.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Variables carried over from the test process. The temp dir must survive,
/// or the child's temp dir would differ from the test's and the guard would
/// reject a HOME the test created under it.
const KEPT_VARS: &[&str] = &[
    "PATH", "TMPDIR", "LANG", "TERM", "USER", "LOGNAME", "SHELL", "TZ",
];

/// The variable the user-config loader checks.
pub const HOME_GUARD_ENV: &str = "AMPLIHACK_TEST_HOME_GUARD";

/// The variable that turns the rotator's launch autostart off.
pub const ROTATE_AUTOSTART_ENV: &str = "AMPLIHACK_ROTATE_AUTOSTART";

/// The debug-only knob that makes the launch-target `--version` probe wait
/// for the candidate's exit instead of its 3 s / 10 s deadline (issue #2019).
pub const PROBE_WAIT_ENV: &str = "AMPLIHACK_TEST_PROBE_WAIT";

/// The variable naming the Claude login profile root.
pub const PROFILE_ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

/// The debug-only variable that makes `profile rotate` and `profile switch`
/// exchange two logins with a stand-in made of plain renames, in place of
/// the host's atomic exchange, which only Linux has (issue #2935). The
/// binary honours it only together with [`HOME_GUARD_ENV`], and only for
/// the value [`LOGIN_EXCHANGE_STAND_IN`]. The stand-in moves two files only
/// when both lie inside the child's temp dir, so a fixture keeps its `HOME`
/// and its profile root under the `TMPDIR` it gives the child. A rotation
/// fixture sets it on every system, so that what its tests decide does not
/// depend on the host.
pub const LOGIN_EXCHANGE_ENV: &str = "AMPLIHACK_TEST_LOGIN_EXCHANGE";

/// The one value [`LOGIN_EXCHANGE_ENV`] takes.
pub const LOGIN_EXCHANGE_STAND_IN: &str = "renames";

/// The variable naming the process table the binary reads instead of
/// `/proc` (issue #2042). The binary honours it only under the home guard,
/// for a directory under the temp dir owned by this user and not writable by
/// others.
pub const PROC_ROOT_ENV: &str = "AMPLIHACK_TEST_PROC_ROOT";

/// The variables that stop git from asking for a credential (issue #1764),
/// with their values: the same three that `.cargo/config.toml` sets.
pub const NO_GIT_PROMPT_ENV: [(&str, &str); 3] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", "true"),
    ("SSH_ASKPASS", "true"),
];

/// Set [`NO_GIT_PROMPT_ENV`] and a null stdin on `command`, and return it.
///
/// Call it after `env_clear()`, which drops both the values cargo's `[env]`
/// table gave the test process and any `GIT_TERMINAL_PROMPT=0` an
/// `amplihack_git` builder set. A later `.env()` or `.stdin()` call wins. A
/// test that hands the command to rexpect's `spawn_command` sets
/// `.stdin(Stdio::inherit())` afterwards: rexpect makes its pseudo-terminal
/// stdin before `exec`, and the null stdin would replace it.
pub fn no_git_prompt(command: &mut Command) -> &mut Command {
    for (name, value) in NO_GIT_PROMPT_ENV {
        command.env(name, value);
    }
    command.stdin(Stdio::null())
}

/// A temp `HOME` for tests that do not make their own: one per test process,
/// created by `tempfile` (a fresh random name, mode 0700, owned by this user,
/// so never a directory someone else made or an earlier run left), and
/// removed when the process exits.
pub fn shared_temp_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("amplihack-test-home-")
            .tempdir()
            .expect("create the per-process test HOME")
            .keep();
        // A static is never dropped, so a TempDir kept here would outlive the
        // process; remove the directory at exit instead.
        extern "C" fn remove_at_exit() {
            if let Some(home) = HOME.get() {
                let _ = std::fs::remove_dir_all(home);
            }
        }
        // SAFETY: registering a plain `extern "C"` function has no
        // preconditions; it runs once, at normal process exit.
        unsafe { libc::atexit(remove_at_exit) };
        dir
    })
}

#[path = "claude_profile_isolation.rs"]
// `cli_integration` loads this file twice, here and through
// `support/mod.rs`: one source, the same listed variables, and this file must not
// name `support` because `tests/integration` and `bins/amplihack/tests` have
// no such module (issue #2922). The included file carries its own
// `#![allow(dead_code)]`, so repeating it here is a duplicated attribute.
#[allow(clippy::duplicate_mod)]
pub mod claude_profile_isolation;

#[path = "proc_tree_seed.rs"]
// `cli_integration` loads this file twice, here and through the other of
// `isolated_amplihack.rs` / `fake_proc.rs`: one source, the same constants,
// and `fake_proc.rs` must not name `isolated_amplihack` so the lib tests can
// include it too (issue #2196).
#[allow(dead_code, clippy::duplicate_mod)]
mod proc_tree_seed;
#[allow(unused_imports)]
pub use proc_tree_seed::{SELF_ENVIRON, seed_proc_tree};

/// An empty fake process table shared by this test process:
/// `<shared_temp_home>/fake-proc/`, mode 0700, seeded by [`seed_proc_tree`]
/// and holding no pid entries, so the liveness scan sees no sessions and the
/// rotator reads as stale or stopped. Written once and never again: parallel
/// tests share it. Removed with the shared HOME at exit.
pub fn shared_empty_proc_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = shared_temp_home().join("fake-proc");
        DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap_or_else(|e| panic!("create {}: {e}", root.display()));
        seed_proc_tree(&root);
        root
    })
}

/// The start of the stderr line a debug `amplihack` prints, once per
/// process, when it refuses [`PROC_ROOT_ENV`] and reads the host `/proc`.
pub const PROC_ROOT_REFUSED: &str = "AMPLIHACK_TEST_PROC_ROOT ignored";

/// Panics if `stderr` shows the child refused its fake process table, for
/// example because its `TMPDIR` or home guard was lost after `env_clear()`.
pub fn assert_proc_root_honoured(stderr: &str) {
    assert!(
        !stderr.contains(PROC_ROOT_REFUSED),
        "the child read the host /proc (issue #2042): {stderr}"
    );
}

/// Set [`PROC_ROOT_ENV`] to [`shared_empty_proc_root`] on `command`, and
/// return it. Call it after `env_clear()`; a later `.env()` call wins.
pub fn reapply_proc_root(command: &mut Command) -> &mut Command {
    command.env(PROC_ROOT_ENV, shared_empty_proc_root())
}

/// The name of the asset resolver a non-test build discovers on `PATH`.
pub const RESOLVER_BIN: &str = "amplihack-asset-resolver";

/// `path` without the directories that hold an [`RESOLVER_BIN`], so a child
/// cannot discover and stage a host resolver through `PATH` (issue #2182).
/// Every other directory stays, so tools such as a `RUSTC_WRAPPER`, `cargo`
/// and `recipe-runner-rs` are still found.
pub fn path_without_resolver(path: &OsStr) -> std::ffi::OsString {
    std::env::join_paths(std::env::split_paths(path).filter(|dir| !dir.join(RESOLVER_BIN).exists()))
        .expect("directories split from PATH join again")
}

/// A `Command` for the `amplihack` binary at `bin`, isolated as described in
/// the module docs.
pub fn isolated(bin: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(bin);
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let Some(name) = key.to_str() else { continue };
        let kept = KEPT_VARS.contains(&name)
            || name.starts_with("CARGO")
            || name.starts_with("RUST")
            || name.starts_with("LC_");
        if kept {
            command.env(&key, value);
        }
    }
    command.env_remove(PROFILE_ROOT_ENV);
    claude_profile_isolation::remove_from_command(&mut command);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path_without_resolver(&path));
    }
    no_git_prompt(&mut command);
    // The toolchain lives under the real HOME unless these say otherwise; a
    // child that runs cargo must still find it after HOME moves.
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for (var, dir) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
            if std::env::var_os(var).is_none() && home.join(dir).is_dir() {
                command.env(var, home.join(dir));
            }
        }
    }
    command
        .env("HOME", shared_temp_home())
        .env(HOME_GUARD_ENV, "1")
        .env(ROTATE_AUTOSTART_ENV, "0")
        .env(PROBE_WAIT_ENV, "exit");
    reapply_proc_root(&mut command);
    command
}

/// A fixture root of the test's own, and the environment that points a child
/// at it (issue #2931).
///
/// Use it when a test gives the binary a `HOME` it made. Setting `HOME` by
/// hand on an [`isolated`] command leaves `TMPDIR`, the profile root and the
/// process table where the test process had them, and two faults follow:
///
/// - the test home check discards a `HOME` outside the child's temp dir, so a
///   fixture made under `scratch_base()` (which falls back to `/var/tmp`
///   when `TMPDIR` lies inside a framework install) reads as "HOME is not
///   set". Here `TMPDIR` is the root and `HOME` lies inside it, whatever the
///   test process's own `TMPDIR` is.
/// - on BSD file systems (macOS) a new entry takes its folder's group, and
///   under `/var/tmp` that is not the user's own, so checks that refuse a
///   foreign group refuse the fixture. Here the root's group is set to the
///   effective group before anything is made in it.
///
/// Issue #1699 stands: the profile root named here lies inside the root, so
/// an install under this `HOME` cannot refresh real profiles. Issue #2922
/// asks for the same private environment for every test that sets `HOME`;
/// this type is written for that work to adopt, and only the fixtures behind
/// the #2931 failures use it so far.
///
/// Everything a test writes goes under [`PrivateEnv::root`], which is removed
/// when the value is dropped.
pub struct PrivateEnv {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    proc_root: PathBuf,
}

/// A [`PrivateEnv`] whose root is a new folder inside `base`: mode 0700 from
/// creation, a random name, canonical, and in the effective group. Holds
/// `home/` (0700) and a seeded, empty process table `fake-proc/` (0700).
/// Panics on any I/O failure.
pub fn private_env_in(base: impl AsRef<Path>) -> PrivateEnv {
    use std::os::unix::fs::PermissionsExt;
    let base = base.as_ref();
    let tmp = tempfile::Builder::new()
        .prefix("private-env-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(base)
        .unwrap_or_else(|e| panic!("create a private root in {}: {e}", base.display()));
    let root = tmp
        .path()
        .canonicalize()
        .unwrap_or_else(|e| panic!("resolve {}: {e}", tmp.path().display()));
    // Before any entry is made, so every entry inherits it where the file
    // system hands a folder's group down. Not recursive: the root is empty.
    // SAFETY: getegid has no preconditions.
    let group = unsafe { libc::getegid() };
    std::os::unix::fs::chown(&root, None, Some(group))
        .unwrap_or_else(|e| panic!("set the group of {}: {e}", root.display()));
    let home = root.join("home");
    let proc_root = root.join("fake-proc");
    for dir in [&home, &proc_root] {
        DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    }
    seed_proc_tree(&proc_root);
    PrivateEnv {
        _tmp: tmp,
        root,
        home,
        proc_root,
    }
}

impl PrivateEnv {
    /// The root: canonical, mode 0700, in the effective group.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/home`, the child's `HOME`.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `<root>/profiles`, the child's Claude login profile root. Not created.
    pub fn profile_root(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// `<root>/claude-config`, the child's `CLAUDE_CONFIG_DIR`. Not created.
    pub fn claude_config_dir(&self) -> PathBuf {
        self.root.join("claude-config")
    }

    /// [`isolated`]`(bin)` with every variable that names user state pointed
    /// inside the root: `HOME`, `TMPDIR` (the root itself, so the test home
    /// check accepts the `HOME`), the profile root, the process table,
    /// `CLAUDE_CONFIG_DIR` and `AMPLIHACK_HOME`. A later `.env()` call wins.
    pub fn command(&self, bin: impl AsRef<OsStr>) -> Command {
        let mut command = isolated(bin);
        command
            .env("HOME", &self.home)
            .env("TMPDIR", &self.root)
            .env(PROC_ROOT_ENV, &self.proc_root)
            .env(PROFILE_ROOT_ENV, self.profile_root())
            .env("CLAUDE_CONFIG_DIR", self.claude_config_dir())
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"));
        command
    }
}
