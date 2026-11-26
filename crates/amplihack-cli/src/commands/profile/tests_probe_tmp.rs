//! Issue #3292: a usage probe leaves nothing in the temporary root.
//!
//! Contract:
//! - Every probe attempt that `probe_with_sandbox` starts runs with
//!   `CLAUDE_CODE_TMPDIR` set to a fresh folder directly under
//!   `std::env::temp_dir()`, owner-only (`0700`) while the probe runs.
//! - That folder, and everything `claude` wrote in it, is gone when the
//!   call returns: after a clean exit, a non-zero exit, the deadline, and
//!   with or without `bwrap`.
//! - In `Auto`, the request and the `/usage` fallback each get their own
//!   folder.
//! - A value the built command already had is replaced.
//! - A `build` error returns before any folder is made or program started.
//!
//! The stand-in `claude` records the value it was given and the folder's
//! mode, then writes the session tree Claude Code makes
//! (`claude-<uid>/<cwd name>/<session id>/started`) inside it. No test
//! changes `TMPDIR` or the process environment, and no test lists the
//! temporary root, which other tests share.

use super::ProfileError;
use super::probe_limits::ProbeMode;
use super::probe_sandbox::{NO_PERSIST, Notice, ProbeSandbox, Unavailable};
use super::probe_tmp::{ProbeTmp, VAR};
use super::usage_cmd::probe_with_sandbox;
use super::usage_parse::UsageState;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const NOW: i64 = 1_760_000_000;
const SAFE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const USAGE: &str = "printf 'Current session: 37%% used · resets 4pm (UTC)\\nCurrent week (all models): 61%% used · resets Oct 22, 2am (UTC)\\n'";

/// A `0700` scratch folder with a config dir whose `projects` exists, so a
/// fake `bwrap` has a folder to hide. Holds the shared `env_lock`, as the
/// stand-ins are written by `sh` with the inherited `PATH`.
struct Scratch {
    tmp: tempfile::TempDir,
    _env: std::sync::MutexGuard<'static, ()>,
}

impl Scratch {
    fn new() -> Self {
        let env = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let s = Self { tmp, _env: env };
        fs::create_dir_all(s.config().join("projects")).unwrap();
        s
    }

    fn base(&self) -> PathBuf {
        self.tmp.path().canonicalize().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.base().join("home")
    }

    fn config(&self) -> PathBuf {
        self.home().join(".claude")
    }

    /// Writes an executable script into a new `0700` dir.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let dir = self.base().join(format!("bin-{name}"));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join(name);
        crate::test_support::write_executable(&path, body);
        path
    }

    /// A stand-in `claude`: records `CLAUDE_CODE_TMPDIR` and that folder's
    /// mode, writes a session tree in it, then runs `then`.
    fn claude(&self, then: &str) -> PathBuf {
        let d = self.base();
        let d = d.display();
        self.script(
            "claude",
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"${{{VAR}-unset}}\" >> '{d}/tmpdirs'\n\
                 if [ -n \"${{{VAR}}}\" ] && [ -d \"${{{VAR}}}\" ]; then\n\
                 ls -ld \"${{{VAR}}}\" | cut -c1-10 >> '{d}/modes'\n\
                 mkdir -p \"${{{VAR}}}/claude-$(id -u)/-work-dir/0b6c3f2e-session/started\"\n\
                 fi\n\
                 {then}\n"
            ),
        )
    }

    /// A fake `bwrap` that runs everything after `--` as a child.
    fn bwrap(&self) -> PathBuf {
        self.script(
            "bwrap",
            "#!/bin/sh\n\
             [ $# -gt 0 ] || exit 0\n\
             while [ $# -gt 0 ] && [ \"$1\" != -- ]; do shift; done\n\
             shift\n\
             \"$@\"\n\
             exit $?\n",
        )
    }

    fn command(&self, program: &Path, args: &[String]) -> Command {
        let mut c = Command::new(program);
        c.args(args)
            .env("CLAUDE_CONFIG_DIR", self.config())
            .env("PATH", SAFE_PATH);
        c
    }

    fn probe(
        &self,
        program: &Path,
        sandbox: &ProbeSandbox,
        mode: ProbeMode,
        timeout: Duration,
    ) -> super::usage_cmd::Probe {
        probe_with_sandbox(
            |a| Ok(self.command(program, a)),
            sandbox,
            &self.home(),
            &Notice::new(),
            mode,
            timeout,
            NOW,
            false,
        )
        .unwrap()
    }

    fn recorded(&self) -> Vec<String> {
        lines(&self.base().join("tmpdirs"))
    }

    fn modes(&self) -> Vec<String> {
        lines(&self.base().join("modes"))
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Each recorded folder sat directly under the temporary root, was
/// owner-only while the probe ran, and is gone now.
fn assert_private_and_gone(s: &Scratch, expected: usize) {
    let recorded = s.recorded();
    assert_eq!(recorded.len(), expected, "starts: {recorded:?}");
    let root = std::env::temp_dir();
    for v in &recorded {
        let p = Path::new(v);
        assert!(p.is_absolute(), "not a folder: {v:?}");
        assert_eq!(p.parent(), Some(root.as_path()), "{v:?}");
        assert!(
            fs::symlink_metadata(p).is_err(),
            "left behind after the probe: {v:?}"
        );
    }
    assert_eq!(s.modes(), vec!["drwx------".to_owned(); expected]);
}

#[test]
fn a_successful_text_probe_leaves_no_folder() {
    let s = Scratch::new();
    let claude = s.claude(USAGE);
    let p = s.probe(
        &claude,
        &ProbeSandbox::Unavailable(Unavailable::Test),
        ProbeMode::Text,
        Duration::from_secs(20),
    );
    assert_eq!(p.report.state, UsageState::Ok);
    assert_private_and_gone(&s, 1);
}

#[test]
fn the_request_and_the_fallback_each_get_their_own_folder() {
    let s = Scratch::new();
    // The request gets no answer, so the `/usage` fallback runs.
    let claude = s.claude(&format!("[ \"$2\" = --input-format ] && exit 1\n{USAGE}"));
    let p = s.probe(
        &claude,
        &ProbeSandbox::Unavailable(Unavailable::Test),
        ProbeMode::Auto,
        Duration::from_secs(20),
    );
    assert_eq!(p.report.state, UsageState::Ok);
    assert_private_and_gone(&s, 2);
    let r = s.recorded();
    assert_ne!(r[0], r[1]);
}

#[test]
fn a_failing_probe_leaves_no_folder() {
    let s = Scratch::new();
    let claude = s.claude("echo 'something went wrong' >&2; exit 3");
    let p = s.probe(
        &claude,
        &ProbeSandbox::Unavailable(Unavailable::Test),
        ProbeMode::Text,
        Duration::from_secs(20),
    );
    assert_ne!(p.report.state, UsageState::Ok);
    assert_private_and_gone(&s, 1);
}

#[test]
fn a_probe_ended_at_the_deadline_leaves_no_folder() {
    let s = Scratch::new();
    let claude = s.claude("sleep 30");
    let p = s.probe(
        &claude,
        &ProbeSandbox::Unavailable(Unavailable::Test),
        ProbeMode::Text,
        Duration::from_secs(2),
    );
    assert_eq!(p.report.state, UsageState::Timeout);
    assert_private_and_gone(&s, 1);
}

#[test]
fn a_probe_under_bwrap_gets_the_folder_and_leaves_none() {
    let s = Scratch::new();
    let claude = s.claude(USAGE);
    let bwrap = s.bwrap();
    for mode in [ProbeMode::Text, ProbeMode::Auto] {
        let _ = fs::remove_file(s.base().join("tmpdirs"));
        let _ = fs::remove_file(s.base().join("modes"));
        let p = s.probe(
            &claude,
            &ProbeSandbox::Bwrap(bwrap.clone()),
            mode,
            Duration::from_secs(20),
        );
        assert_eq!(p.report.state, UsageState::Ok);
        // In `Auto` the request gets plain text back, so the fallback runs.
        let expected = if mode == ProbeMode::Text { 1 } else { 2 };
        assert_private_and_gone(&s, expected);
    }
}

#[test]
fn a_value_on_the_built_command_is_replaced() {
    let s = Scratch::new();
    let claude = s.claude(USAGE);
    let elsewhere = s.base().join("elsewhere");
    let p = probe_with_sandbox(
        |a| {
            let mut c = s.command(&claude, a);
            c.env(VAR, &elsewhere);
            Ok(c)
        },
        &ProbeSandbox::Unavailable(Unavailable::Test),
        &s.home(),
        &Notice::new(),
        ProbeMode::Text,
        Duration::from_secs(20),
        NOW,
        false,
    )
    .unwrap();
    assert_eq!(p.report.state, UsageState::Ok);
    assert_private_and_gone(&s, 1);
    assert!(!elsewhere.exists());
}

#[test]
fn a_build_error_starts_nothing() {
    let s = Scratch::new();
    let claude = s.claude(USAGE);
    let r = probe_with_sandbox(
        |_| -> Result<Command, ProfileError> { Err(ProfileError::NotFound("p".into())) },
        &ProbeSandbox::Unavailable(Unavailable::Test),
        &s.home(),
        &Notice::new(),
        ProbeMode::Text,
        Duration::from_secs(20),
        NOW,
        false,
    );
    assert!(matches!(r, Err(ProfileError::NotFound(_))));
    assert!(s.recorded().is_empty());
    assert!(claude.exists());
}

#[test]
fn give_sets_the_variable_on_the_command_and_close_removes_the_tree() {
    let mut cmd = Command::new("true");
    cmd.arg(NO_PERSIST);
    let tmp = ProbeTmp::give(&mut cmd);
    let set: Vec<_> = cmd
        .get_envs()
        .filter(|(k, _)| *k == VAR)
        .map(|(_, v)| PathBuf::from(v.expect("set, not removed")))
        .collect();
    assert_eq!(set.len(), 1);
    let dir = &set[0];
    assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
    let mode = fs::metadata(dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let nested = dir
        .join("claude-1000")
        .join("-work")
        .join("s")
        .join("started");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.parent().unwrap().join("f"), "x").unwrap();
    tmp.close();
    assert!(fs::symlink_metadata(dir).is_err());
}
