//! Git subprocesses that ignore the caller's inherited git environment.
//!
//! # Why this crate exists (issue #1278)
//!
//! When git runs a hook it exports the repository it is working on into the
//! hook's environment: `GIT_DIR`, and depending on the hook also
//! `GIT_INDEX_FILE`, `GIT_PREFIX`, `GIT_WORK_TREE` and friends. Every
//! descendant of that hook inherits them — `cargo`, each test binary, and
//! every `git` those binaries spawn.
//!
//! Those variables outrank the working directory when git decides which
//! repository it is operating on. So a plain
//! `Command::new("git").current_dir(some_repo)` does *not* report on
//! `some_repo` when `GIT_DIR` is set — it silently reports on whatever
//! `GIT_DIR` names. A `git add .` run that way stages files into the wrong
//! repository's index; a `rev-parse --git-dir` probe reports that a directory
//! which is not a repository is one.
//!
//! That is not only a test problem. `amplihack` itself runs as a git hook, so
//! production code that spawns git — the conflict detector, the artifact
//! guard, the recovery stages — inherits exactly the same lie.
//!
//! Use [`command`] / [`command_in`] instead of `Command::new("git")` so the
//! repository is chosen by the argument you passed, never by ambient state.
//! For a non-git child that will itself run git (a shell script, the
//! `amplihack` binary), use [`scrub`].
//!
//! `scripts/check-git-command-sanitised.sh` enforces this: a new
//! `Command::new("git")` anywhere outside this crate fails the build.

use std::path::Path;
use std::process::Command;

/// Environment variables through which git tells a child process which
/// repository to operate on.
///
/// All of these override working-directory-based discovery, so every one of
/// them has to go before a subprocess can be trusted to act on the directory
/// it was pointed at. Identity variables (`GIT_AUTHOR_*`, `GIT_COMMITTER_*`)
/// are deliberately *not* in this list: they do not select a repository, and
/// clearing them would break commits that rely on an ambient identity.
pub const REPOSITORY_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CEILING_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_DIR",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_INDEX_FILE",
    "GIT_INDEX_VERSION",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_QUARANTINE_PATH",
    "GIT_WORK_TREE",
];

/// A command builder whose child environment can have variables removed.
///
/// Implemented for both [`std::process::Command`] and (with the `tokio`
/// feature) `tokio::process::Command`, so [`scrub`] covers sync and async
/// call sites alike rather than leaving async code to reinvent it.
pub trait Scrubbable {
    /// Remove `key` from the environment the child will be spawned with.
    fn remove_env_var(&mut self, key: &str);
}

impl Scrubbable for Command {
    fn remove_env_var(&mut self, key: &str) {
        self.env_remove(key);
    }
}

#[cfg(feature = "tokio")]
impl Scrubbable for tokio::process::Command {
    fn remove_env_var(&mut self, key: &str) {
        self.env_remove(key);
    }
}

/// Remove the inherited repository selection from `command`.
///
/// Use this for children that are not `git` itself but will run git
/// underneath — a shell script, a recipe step, the `amplihack` binary.
pub fn scrub<C: Scrubbable>(command: &mut C) -> &mut C {
    for var in REPOSITORY_ENV_VARS {
        command.remove_env_var(var);
    }
    command
}

/// A `git` command that ignores the caller's inherited git environment.
///
/// Equivalent to `Command::new("git")` with [`REPOSITORY_ENV_VARS`] cleared.
/// The repository is then whatever the working directory (or an explicit
/// `-C` / `--git-dir` argument) selects.
pub fn command() -> Command {
    let mut command = Command::new("git");
    scrub(&mut command);
    command
}

/// A `git` command that runs in `dir` and ignores the caller's inherited git
/// environment.
pub fn command_in(dir: impl AsRef<Path>) -> Command {
    let mut command = command();
    command.current_dir(dir.as_ref());
    command
}

/// The async twin of [`command`], for callers already inside a tokio runtime.
#[cfg(feature = "tokio")]
pub fn tokio_command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    scrub(&mut command);
    command
}

/// The async twin of [`command_in`].
#[cfg(feature = "tokio")]
pub fn tokio_command_in(dir: impl AsRef<Path>) -> tokio::process::Command {
    let mut command = tokio_command();
    command.current_dir(dir.as_ref());
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// The variables git actually leaks into hooks must be covered.
    #[test]
    fn repository_env_vars_cover_the_hook_leak() {
        for expected in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"] {
            assert!(
                REPOSITORY_ENV_VARS.contains(&expected),
                "{expected} must be scrubbed"
            );
        }
    }

    /// Identity must survive: scrubbing selects a repository, it does not
    /// strip the author a commit would be made under.
    #[test]
    fn repository_env_vars_leave_identity_alone() {
        for identity in [
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
        ] {
            assert!(
                !REPOSITORY_ENV_VARS.contains(&identity),
                "{identity} does not select a repository and must not be scrubbed"
            );
        }
    }

    /// Issue #1278: a directory that is not a repository must stay
    /// not-a-repository even when the *inherited* environment names one.
    ///
    /// "Inherited" is the whole point, and it is why this test re-executes
    /// itself: setting `GIT_DIR` with `.env()` on the command under test would
    /// prove nothing, because an explicit value is supposed to win over a
    /// scrub. The only faithful setup is a parent process that genuinely has
    /// `GIT_DIR` in its environment -- exactly what git hands a hook -- so the
    /// parent half of this test builds the repositories and re-runs the child
    /// half with the variable exported.
    #[test]
    fn command_in_ignores_ambient_git_dir() {
        const CHILD: &str = "AMPLIHACK_GIT_1278_CHILD";
        const PLAIN: &str = "AMPLIHACK_GIT_1278_PLAIN";

        let Some(plain) = std::env::var_os(PLAIN) else {
            // Parent half: build a decoy repository and a directory that is
            // not one, then re-run this same test with GIT_DIR exported.
            let decoy = tempfile::tempdir().expect("tempdir");
            assert!(
                command_in(decoy.path())
                    .args(["init", "--quiet"])
                    .status()
                    .expect("git init")
                    .success()
            );
            let plain = tempfile::tempdir().expect("tempdir");

            let status = Command::new(std::env::current_exe().expect("current_exe"))
                .args([
                    "tests::command_in_ignores_ambient_git_dir",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .env("GIT_DIR", decoy.path().join(".git"))
                .env(PLAIN, plain.path())
                .status()
                .expect("re-run self");
            assert!(
                status.success(),
                "child half failed; see its output above (it runs with GIT_DIR exported)"
            );
            return;
        };

        // Child half: GIT_DIR is genuinely in this process's environment.
        assert!(
            std::env::var_os("GIT_DIR").is_some(),
            "parent half must export GIT_DIR"
        );
        let plain = std::path::PathBuf::from(plain);

        // Control: an unsanitised command inherits GIT_DIR and reports that a
        // directory which is not a repository is one. Without this the
        // assertion below could pass simply because git never looked.
        let hijacked = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&plain)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git rev-parse");
        assert!(
            hijacked.success(),
            "precondition: an inherited GIT_DIR is supposed to hijack an \
             unsanitised git subprocess; if this fails the assertion below \
             proves nothing"
        );

        // Sanitised: the same directory is correctly reported as not a
        // repository, because command_in() dropped the inherited GIT_DIR.
        let honest = command_in(&plain)
            .args(["rev-parse", "--git-dir"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git rev-parse");
        assert!(
            !honest.success(),
            "command_in() must ignore an inherited GIT_DIR"
        );
    }

    /// Every listed variable must actually be removed from the child's
    /// environment, not merely overwritten.
    #[test]
    fn scrub_removes_every_repository_var() {
        let mut probe = Command::new("git");
        for var in REPOSITORY_ENV_VARS {
            probe.env(var, "/nonexistent");
        }
        scrub(&mut probe);

        for var in REPOSITORY_ENV_VARS {
            let entry = probe
                .get_envs()
                .find(|(key, _)| *key == std::ffi::OsStr::new(var))
                .unwrap_or_else(|| panic!("{var} missing from env mutations"));
            assert!(entry.1.is_none(), "{var} survived scrub()");
        }
    }
}
