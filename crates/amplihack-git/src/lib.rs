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
//! Use [`command`] / [`command_in`] (or, for a git a person may have to
//! answer, [`interactive_command`]) instead of `Command::new("git")` so the
//! repository is chosen by the argument you passed, never by ambient state.
//! For a non-git child that will itself run git (a shell script, the
//! `amplihack` binary), use [`scrub`].
//!
//! `scripts/check-git-command-sanitised.sh` enforces this: a new
//! `Command::new("git")` anywhere outside this crate fails the build.
//!
//! # Credential prompts (issue #1764)
//!
//! When a remote asks for a credential and no helper or askpass program
//! supplies one, git opens `/dev/tty` and asks. Under plain `timeout`, which
//! moves its command into a background process group, that read stops git on
//! `SIGTTIN`: nothing fails, and the run waits until something kills it.
//!
//! So [`command`], [`command_in`], [`tokio_command`] and [`tokio_command_in`]
//! also set `GIT_TERMINAL_PROMPT=0`, and git fails at once with
//! `terminal prompts disabled` instead. Credential helpers and an askpass
//! program the user has configured run before that check and keep working.
//! The variable is set with `.env()`, so a later `env_clear()` on the same
//! command removes it. [`scrub`] does not set it: a non-git child gets only
//! what its caller gives it.
//!
//! [`interactive_command`] scrubs the repository variables but leaves the
//! prompt variable alone. It is for the one case where a person can answer
//! the prompt; see its docs.

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

/// The variable that stops git from asking for a credential on the terminal
/// (issue #1764).
///
/// Deliberately outside [`REPOSITORY_ENV_VARS`]: that list names what
/// [`scrub`] removes, and `tests/issue_1278_git_env_isolation.sh` unsets
/// every name in it.
const TERMINAL_PROMPT_VAR: &str = "GIT_TERMINAL_PROMPT";

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

/// A `git` command that ignores the caller's inherited git environment and
/// turns off git's own credential prompt on the terminal.
///
/// Equivalent to `Command::new("git")` with [`REPOSITORY_ENV_VARS`] cleared
/// and `GIT_TERMINAL_PROMPT=0` set. The repository is then whatever the
/// working directory (or an explicit `-C` / `--git-dir` argument) selects.
///
/// `GIT_TERMINAL_PROMPT` does not reach OpenSSH, so an `ssh://` or `git@`
/// remote can still prompt.
pub fn command() -> Command {
    let mut command = interactive_command();
    command.env(TERMINAL_PROMPT_VAR, "0");
    command
}

/// A `git` command that runs in `dir`, ignores the caller's inherited git
/// environment and turns off git's own credential prompt on the terminal.
/// [`command`] says what that does not cover.
pub fn command_in(dir: impl AsRef<Path>) -> Command {
    let mut command = command();
    command.current_dir(dir.as_ref());
    command
}

/// A `git` command that ignores the caller's inherited git environment but
/// may ask for a credential on the terminal.
///
/// It neither sets nor removes `GIT_TERMINAL_PROMPT`, `GIT_ASKPASS` or
/// `SSH_ASKPASS`. Use it only when all three of these hold:
///
/// - a person started the command at a terminal;
/// - the URL comes from that person's own arguments, not from a recipe, an
///   agent or a config file;
/// - the child stays in the caller's foreground process group, with stdio
///   inherited, so the person sees the prompt and can answer it.
///
/// Everything else uses [`command`]. `scripts/check-git-command-sanitised.sh`
/// fails the build on a caller that is not in its `interactive_allowed` list.
pub fn interactive_command() -> Command {
    let mut command = Command::new("git");
    scrub(&mut command);
    command
}

/// The async twin of [`command`], for callers already inside a tokio runtime.
#[cfg(feature = "tokio")]
pub fn tokio_command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    scrub(&mut command);
    command.env(TERMINAL_PROMPT_VAR, "0");
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
    use std::ffi::OsStr;
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

    /// The variables through which git asks for a credential (issue #1764).
    const PROMPT_VARS: [&str; 3] = ["GIT_TERMINAL_PROMPT", "GIT_ASKPASS", "SSH_ASKPASS"];

    /// What `builder` does to `key` in the child's environment: `None` if it
    /// leaves the inherited value alone, `Some(None)` if it removes it, and
    /// `Some(Some(value))` if it sets it.
    fn env_entry<'a>(builder: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
        builder
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(key))
            .map(|(_, value)| value)
    }

    /// Issue #1764: a git that amplihack starts must fail at once when a
    /// remote asks for a credential that nothing supplies. Under plain
    /// `timeout` a terminal prompt stops git on `SIGTTIN` and the run waits
    /// indefinitely. Only the prompt variable is set: an askpass program the
    /// user has configured keeps working.
    #[test]
    fn default_builders_turn_terminal_prompts_off() {
        let dir = std::env::temp_dir();
        for (name, builder) in [("command()", command()), ("command_in()", command_in(&dir))] {
            assert_eq!(
                env_entry(&builder, "GIT_TERMINAL_PROMPT"),
                Some(Some(OsStr::new("0"))),
                "{name} must set GIT_TERMINAL_PROMPT=0"
            );
            for askpass in ["GIT_ASKPASS", "SSH_ASKPASS"] {
                assert_eq!(
                    env_entry(&builder, askpass),
                    None,
                    "{name} must leave {askpass} alone"
                );
            }
        }
    }

    /// The async twins make the same promise as [`command`] and
    /// [`command_in`].
    #[cfg(feature = "tokio")]
    #[test]
    fn tokio_builders_turn_terminal_prompts_off() {
        let dir = std::env::temp_dir();
        for (name, builder) in [
            ("tokio_command()", tokio_command()),
            ("tokio_command_in()", tokio_command_in(&dir)),
        ] {
            assert_eq!(
                env_entry(builder.as_std(), "GIT_TERMINAL_PROMPT"),
                Some(Some(OsStr::new("0"))),
                "{name} must set GIT_TERMINAL_PROMPT=0"
            );
        }
    }

    /// The interactive builder exists for a person at a terminal who can
    /// answer a prompt, so it neither sets nor removes any prompt variable.
    #[test]
    fn interactive_builder_leaves_terminal_prompts_alone() {
        let builder = interactive_command();
        assert_eq!(builder.get_program(), "git", "must run git");
        for var in PROMPT_VARS {
            assert_eq!(
                env_entry(&builder, var),
                None,
                "interactive_command() must neither set nor remove {var}"
            );
        }
    }

    /// Turning prompts off must not cost the #1278 scrub: every builder,
    /// interactive or not, removes every repository variable.
    #[test]
    fn every_builder_scrubs_the_repository_vars() {
        fn assert_scrubbed(name: &str, builder: &Command) {
            for var in REPOSITORY_ENV_VARS {
                assert_eq!(
                    env_entry(builder, var),
                    Some(None),
                    "{name} must remove {var}"
                );
            }
        }

        let dir = std::env::temp_dir();
        for (name, builder) in [
            ("command()", command()),
            ("command_in()", command_in(&dir)),
            ("interactive_command()", interactive_command()),
        ] {
            assert_scrubbed(name, &builder);
        }
        #[cfg(feature = "tokio")]
        {
            for (name, builder) in [
                ("tokio_command()", tokio_command()),
                ("tokio_command_in()", tokio_command_in(&dir)),
            ] {
                assert_scrubbed(name, builder.as_std());
            }
        }
    }
}
