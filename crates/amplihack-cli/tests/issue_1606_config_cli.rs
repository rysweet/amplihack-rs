//! Issue #1606 (TDD): `amplihack config`, the layered user/project config, the
//! legacy `~/.amplihack/config` migration, and the `user_config` agent-binary
//! layer, tested black-box through the real `amplihack` binary.
//!
//! The contract is docs/reference/user-config.md. Every test is hermetic:
//! the child runs with `env_clear()`, a temp `HOME`, a temp project that has
//! its own `.git`, and only the variables the test names. Nothing reads the
//! developer's real `~/.amplihack` (issue #1542).

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const SCHEMA_ORDER: &[&str] = &[
    "agent.binary",
    "paths.home",
    "node.heap_consent",
    "node.max_old_space_mb",
    "recipe.verbose",
    "recipe.step_timeout",
    "recipe.detach",
    "recipe.tmux_session_pattern",
    "recipe.log_dir",
    "recipe.usage_limit_wait",
    "recipe.usage_limit_max_wait_secs",
    "orchestrator.max_depth",
    "sessions.daily_archive",
];

/// Records argv and its environment, then reports success in the runner's
/// JSON format.
const STUB_RUNNER: &str = r#"#!/bin/sh
env > "$RECORD_DIR/env.txt"
printf '%s\n' '{"recipe_name":"issue-1606","success":true,"step_results":[],"context":{}}'
"#;

/// Answers `tmux show-environment -g COPILOT_CLI` the way a server started
/// from a Copilot session would.
const TMUX_STUB: &str = r#"#!/bin/sh
if [ "$1 $2 $3" = "show-environment -g COPILOT_CLI" ]; then
  echo "COPILOT_CLI=1"
  exit 0
fi
if [ "$1 $2 $3" = "show-environment -g AMPLIHACK_SESSION_NONCE" ]; then
  echo "AMPLIHACK_SESSION_NONCE=server-starter"
  exit 0
fi
exit 1
"#;

const RECIPE: &str = r#"name: issue-1606
description: no-op
version: 1.0.0
context: {}
steps:
  - id: noop
    type: bash
    command: "true"
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
    tools: PathBuf,
    records: PathBuf,
}

impl Fx {
    fn new() -> Self {
        Self::with_home_name("home")
    }

    fn with_home_name(home_name: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join(home_name);
        let project = root.join("project");
        let tools = root.join("tools");
        let records = root.join("records");
        for d in [&home, &project, &tools, &records] {
            fs::create_dir_all(d).unwrap();
        }
        // A `.git` directory with a HEAD marks the project root; no git needed.
        mark_repository(&project);
        crate::support::exec_stub::write_executable(
            &tools.join("recipe-runner-rs"),
            STUB_RUNNER,
            0o755,
        );
        // A tmux whose server's global environment holds COPILOT_CLI=1, as a
        // server a Copilot session started does. Only `show-environment -g`
        // is answered; with TMUX unset nothing asks it.
        crate::support::exec_stub::write_executable(&tools.join("tmux"), TMUX_STUB, 0o755);
        Self {
            _tmp: tmp,
            root,
            home,
            project,
            tools,
            records,
        }
    }

    fn user_file(&self) -> PathBuf {
        self.home.join(".amplihack").join("config.toml")
    }

    fn project_file(&self) -> PathBuf {
        self.project.join(".amplihack").join("config.toml")
    }

    fn legacy_file(&self) -> PathBuf {
        self.home.join(".amplihack").join("config")
    }

    fn write_user(&self, body: &str) {
        write_secure(&self.user_file(), body);
    }

    fn write_project(&self, body: &str) {
        let p = self.project_file();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn write_legacy(&self, body: &str) {
        write_secure(&self.legacy_file(), body);
    }

    fn cmd(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(cwd)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .stdin(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    /// `amplihack <args>` from the project directory.
    fn amp(&self, args: &[&str]) -> Output {
        self.amp_env(args, &[])
    }

    fn amp_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.amp_in(&self.project, args, env)
    }

    fn amp_in(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        run_bounded(self.cmd(cwd, args, env))
    }

    fn get(&self, key: &str) -> String {
        let out = self.amp(&["config", "get", key]);
        assert_ok(&out);
        stdout(&out).trim_end_matches('\n').to_string()
    }

    fn get_origin(&self, key: &str) -> String {
        let out = self.amp(&["config", "get", key, "--show-origin"]);
        assert_ok(&out);
        stdout(&out).trim_end_matches('\n').to_string()
    }

    fn list_row(&self, key: &str, env: &[(&str, &str)]) -> String {
        let out = self.amp_env(&["config", "list", "--show-origin"], env);
        assert_ok(&out);
        stdout(&out)
            .lines()
            .find(|l| l.starts_with(&format!("{key} = ")))
            .unwrap_or_else(|| panic!("no row for {key} in:\n{}", stdout(&out)))
            .to_string()
    }
}

/// What git, and the config loader, accept as a repository root.
fn mark_repository(dir: &Path) {
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
}

fn write_secure(path: &Path, body: &str) {
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// Run with a hard 60 s bound so a FIFO or a prompt cannot hang the suite.
fn run_bounded(mut c: Command) -> Output {
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().expect("failed to spawn amplihack");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "amplihack did not exit within 60 s\nstderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("killed by a signal")
}

fn assert_ok(o: &Output) {
    assert_eq!(
        code(o),
        0,
        "expected exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout(o),
        stderr(o)
    );
}

fn assert_exit(o: &Output, want: i32) {
    assert_eq!(
        code(o),
        want,
        "expected exit {want}\nstdout:\n{}\nstderr:\n{}",
        stdout(o),
        stderr(o)
    );
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

// ── test isolation (review of #1647) ────────────────────────────────────────

/// Under the guard a HOME outside the temp dir (a developer's real one) is
/// neither read nor migrated.
#[test]
fn the_guard_keeps_a_non_temp_home_untouched() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("issue-1606-guard");
    let home = tempfile::tempdir_in({
        fs::create_dir_all(&base).unwrap();
        &base
    })
    .unwrap();
    let dir = home.path().join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    write_secure(&dir.join("config.toml"), "[agent]\nbinary = \"codex\"\n");
    write_secure(&dir.join("config"), r#"{"node_options_consent": true}"#);
    let run = |guard: Option<&str>| {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["config", "get", "agent.binary"])
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", home.path())
            .env("TMPDIR", std::env::temp_dir())
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .stdin(Stdio::null());
        if let Some(value) = guard {
            c.env("AMPLIHACK_TEST_HOME_GUARD", value);
        }
        run_bounded(c)
    };
    let guarded = run(Some("1"));
    assert_ok(&guarded);
    assert_eq!(stdout(&guarded), "\n");
    assert!(
        stderr(&guarded).contains("AMPLIHACK_TEST_HOME_GUARD is set and HOME"),
        "a discarded HOME must be explained, not reported as unset: {}",
        stderr(&guarded)
    );
    assert!(
        dir.join("config").is_file(),
        "the legacy file must not be migrated"
    );
    let unguarded = run(None);
    assert_ok(&unguarded);
    assert_eq!(stdout(&unguarded), "codex\n");
}

// ── list / get grammar ──────────────────────────────────────────────────────

#[test]
fn list_prints_every_key_in_schema_order_with_defaults() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "list"]);
    assert_ok(&out);
    let rows: Vec<String> = stdout(&out).lines().map(str::to_string).collect();
    assert_eq!(
        rows,
        vec![
            "agent.binary = (unset)",
            "paths.home = (unset)",
            "node.heap_consent = (unset)",
            "node.max_old_space_mb = (unset)",
            "recipe.verbose = false",
            "recipe.step_timeout = (unset)",
            "recipe.detach = \"none\"",
            "recipe.tmux_session_pattern = \"amp-{recipe}-{repo}-{ts}\"",
            "recipe.log_dir = (unset)",
            "recipe.usage_limit_wait = true",
            "recipe.usage_limit_max_wait_secs = (unset)",
            "orchestrator.max_depth = 3",
            "sessions.daily_archive = true",
        ]
    );
}

#[test]
fn list_show_origin_marks_defaults() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "list", "--show-origin"]);
    assert_ok(&out);
    let text = stdout(&out);
    for key in SCHEMA_ORDER {
        let row = text
            .lines()
            .find(|l| l.starts_with(&format!("{key} = ")))
            .unwrap_or_else(|| panic!("missing row {key}"));
        assert!(row.ends_with("  # default"), "row: {row:?}");
    }
    assert!(!text.contains("Config problems:"));
}

#[test]
fn list_show_origin_names_the_user_file() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    assert_eq!(
        fx.list_row("agent.binary", &[]),
        format!(
            "agent.binary = \"claude\"  # user ({})",
            fx.user_file().display()
        )
    );
}

#[test]
fn get_prints_bare_value_and_origin() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    assert_eq!(fx.get("agent.binary"), "claude");
    assert_eq!(
        fx.get_origin("agent.binary"),
        format!("claude  # user ({})", fx.user_file().display())
    );
}

#[test]
fn get_of_unset_key_prints_empty_line_and_exits_zero() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "get", "recipe.step_timeout"]);
    assert_ok(&out);
    assert_eq!(stdout(&out), "\n");
}

#[test]
fn get_of_unknown_key_is_a_usage_error() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "get", "no.such_key"]);
    assert_exit(&out, 2);
    assert!(stdout(&out).is_empty(), "stdout must carry data only");
}

// ── set / unset ─────────────────────────────────────────────────────────────

#[test]
fn set_writes_user_file_0600_in_a_0700_dir_and_reports() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "set", "agent.binary", "claude"]);
    assert_ok(&out);
    assert_eq!(
        stdout(&out).trim_end(),
        format!(
            "set agent.binary = \"claude\" in {}",
            fx.user_file().display()
        )
    );
    assert_eq!(mode(&fx.user_file()), 0o600);
    assert_eq!(mode(fx.user_file().parent().unwrap()), 0o700);
    assert_eq!(fx.get("agent.binary"), "claude");
}

#[test]
fn set_keeps_comments_in_the_file() {
    let fx = Fx::new();
    fx.write_user("# my settings\n[recipe]\n# keep me\nverbose = false\n");
    assert_ok(&fx.amp(&["config", "set", "recipe.verbose", "true"]));
    let body = fs::read_to_string(fx.user_file()).unwrap();
    assert!(body.contains("# my settings"), "{body}");
    assert!(body.contains("# keep me"), "{body}");
    assert_eq!(fx.get("recipe.verbose"), "true");
}

#[test]
fn set_rejects_unknown_key_without_writing() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "set", "github.accounts", "x"]);
    assert_exit(&out, 2);
    assert!(!fx.user_file().exists());
}

#[test]
fn set_rejects_invalid_values_with_exit_2() {
    let fx = Fx::new();
    for (key, value) in [
        ("orchestrator.max_depth", "99"),
        ("orchestrator.max_depth", "-1"),
        ("orchestrator.max_depth", "three"),
        ("node.max_old_space_mb", "8191"),
        ("node.max_old_space_mb", "32769"),
        ("recipe.step_timeout", "0"),
        ("recipe.verbose", "yes"),
        ("node.heap_consent", "1"),
        ("agent.binary", "/bin/sh"),
        ("agent.binary", "gpt"),
        ("paths.home", "relative/dir"),
        ("paths.home", "/tmp/../etc"),
    ] {
        let out = fx.amp(&["config", "set", key, value]);
        assert_exit(&out, 2);
        assert!(
            stderr(&out).contains(key),
            "error for {key}={value} must name the key: {}",
            stderr(&out)
        );
    }
    assert!(!fx.user_file().exists(), "no invalid value may be written");
}

#[test]
fn set_accepts_boundary_values() {
    let fx = Fx::new();
    for (key, value) in [
        ("orchestrator.max_depth", "0"),
        ("orchestrator.max_depth", "32"),
        ("node.max_old_space_mb", "8192"),
        ("node.max_old_space_mb", "32768"),
        ("recipe.step_timeout", "1"),
        ("agent.binary", "AMPLIFIER"),
    ] {
        assert_ok(&fx.amp(&["config", "set", key, value]));
    }
    assert_eq!(fx.get("agent.binary"), "amplifier");
}

#[test]
fn set_user_only_key_with_project_is_a_usage_error() {
    let fx = Fx::new();
    let dir = fx.root.join("somehome");
    fs::create_dir_all(&dir).unwrap();
    for (key, value) in [
        ("agent.binary", "claude"),
        ("paths.home", dir.to_str().unwrap()),
        ("node.heap_consent", "true"),
        ("node.max_old_space_mb", "16384"),
    ] {
        let out = fx.amp(&["config", "set", key, value, "--project"]);
        assert_exit(&out, 2);
    }
    assert!(!fx.project_file().exists());
}

#[test]
fn project_flag_outside_git_is_a_usage_error() {
    let fx = Fx::new();
    let nogit = fx.root.join("nogit");
    fs::create_dir_all(&nogit).unwrap();
    let out = fx.amp_in(
        &nogit,
        &["config", "set", "recipe.verbose", "true", "--project"],
        &[],
    );
    assert_exit(&out, 2);
}

#[test]
fn set_project_writes_at_the_git_root_from_a_subdirectory() {
    let fx = Fx::new();
    let sub = fx.project.join("a").join("b");
    fs::create_dir_all(&sub).unwrap();
    let out = fx.amp_in(
        &sub,
        &["config", "set", "orchestrator.max_depth", "2", "--project"],
        &[],
    );
    assert_ok(&out);
    assert!(fx.project_file().is_file());
    assert!(!fx.user_file().exists());
}

#[test]
fn set_refuses_a_file_with_a_syntax_error() {
    let fx = Fx::new();
    fx.write_user("[agent\nbinary = \"claude\"\n");
    let before = fs::read(fx.user_file()).unwrap();
    let out = fx.amp(&["config", "set", "recipe.verbose", "true"]);
    assert_exit(&out, 1);
    assert_eq!(fs::read(fx.user_file()).unwrap(), before);
}

#[test]
fn unset_removes_a_key_and_absent_key_is_a_noop() {
    let fx = Fx::new();
    fx.write_user("[recipe]\nverbose = true\n");
    assert_ok(&fx.amp(&["config", "unset", "recipe.verbose"]));
    assert_eq!(fx.get("recipe.verbose"), "false");
    let again = fx.amp(&["config", "unset", "recipe.verbose"]);
    assert_ok(&again);
    assert_eq!(
        stdout(&again).trim_end(),
        format!("recipe.verbose is not set in {}", fx.user_file().display())
    );
}

#[test]
fn unset_unknown_key_is_a_usage_error() {
    let fx = Fx::new();
    assert_exit(&fx.amp(&["config", "unset", "nope.nope"]), 2);
}

#[test]
fn set_through_a_symlink_writes_the_target_and_keeps_the_link() {
    let fx = Fx::new();
    let dotfiles = fx.root.join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    let target = dotfiles.join("amplihack.toml");
    fs::write(&target, "").unwrap();
    fs::create_dir_all(fx.home.join(".amplihack")).unwrap();
    fs::set_permissions(
        fx.home.join(".amplihack"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    symlink(&target, fx.user_file()).unwrap();
    assert_ok(&fx.amp(&["config", "set", "agent.binary", "codex"]));
    assert!(
        fs::symlink_metadata(fx.user_file())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::read_to_string(&target).unwrap().contains("codex"));
}

// ── precedence ──────────────────────────────────────────────────────────────

#[test]
fn precedence_default_user_project_env_for_max_depth() {
    let fx = Fx::new();
    assert_eq!(
        fx.list_row("orchestrator.max_depth", &[]),
        "orchestrator.max_depth = 3  # default"
    );

    fx.write_user("[orchestrator]\nmax_depth = 5\n");
    assert_eq!(
        fx.list_row("orchestrator.max_depth", &[]),
        format!(
            "orchestrator.max_depth = 5  # user ({})",
            fx.user_file().display()
        )
    );

    fx.write_project("[orchestrator]\nmax_depth = 2\n");
    assert_eq!(
        fx.list_row("orchestrator.max_depth", &[]),
        format!(
            "orchestrator.max_depth = 2  # project ({})",
            fx.project_file().display()
        )
    );

    assert_eq!(
        fx.list_row("orchestrator.max_depth", &[("AMPLIHACK_MAX_DEPTH", "7")]),
        "orchestrator.max_depth = 7  # env (AMPLIHACK_MAX_DEPTH)"
    );
}

#[test]
fn a_project_file_cannot_choose_the_agent_cli() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    fx.write_project("[agent]\nbinary = \"codex\"\n");
    let out = fx.amp(&["config", "get", "agent.binary", "--show-origin"]);
    assert_ok(&out);
    assert!(
        stdout(&out).starts_with("claude  # user ("),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("agent.binary: can only be set in the user file"),
        "{}",
        stderr(&out)
    );
    let out = fx.amp(&["config", "set", "agent.binary", "codex", "--project"]);
    assert_exit(&out, 2);
    assert!(stderr(&out).contains("agent.binary"), "{}", stderr(&out));
}

#[test]
fn project_set_refuses_a_symlinked_project_file() {
    let fx = Fx::new();
    let outside = fx.root.join("outside.toml");
    fs::write(&outside, "").unwrap();
    fs::create_dir_all(fx.project.join(".amplihack")).unwrap();
    symlink(&outside, fx.project_file()).unwrap();
    let out = fx.amp(&["config", "set", "recipe.verbose", "true", "--project"]);
    assert_exit(&out, 1);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "");
}

#[test]
fn project_set_refuses_a_project_dir_that_resolves_outside_the_repo() {
    let fx = Fx::new();
    let outside = fx.root.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, fx.project.join(".amplihack")).unwrap();
    let out = fx.amp(&["config", "set", "recipe.verbose", "true", "--project"]);
    assert_exit(&out, 1);
    assert!(!outside.join("config.toml").exists());
}

#[test]
fn project_cannot_raise_max_depth_above_the_user_value() {
    let fx = Fx::new();
    fx.write_user("[orchestrator]\nmax_depth = 3\n");
    fx.write_project("[orchestrator]\nmax_depth = 8\n");
    let out = fx.amp(&["config", "get", "orchestrator.max_depth"]);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "3");
    assert!(
        stderr(&out).contains("orchestrator.max_depth"),
        "a raised project value must be reported: {}",
        stderr(&out)
    );
}

#[test]
fn project_cannot_raise_max_depth_above_the_default() {
    let fx = Fx::new();
    fx.write_project("[orchestrator]\nmax_depth = 10\n");
    assert_eq!(fx.get("orchestrator.max_depth"), "3");
}

#[test]
fn project_step_timeout_applies_when_user_unset_and_only_lowers_otherwise() {
    let fx = Fx::new();
    fx.write_project("[recipe]\nstep_timeout = 1800\n");
    assert_eq!(fx.get("recipe.step_timeout"), "1800");
    fx.write_user("[recipe]\nstep_timeout = 600\n");
    assert_eq!(fx.get("recipe.step_timeout"), "600");
}

#[test]
fn invalid_env_value_is_shown_but_does_not_fail() {
    let fx = Fx::new();
    let row = fx.list_row("orchestrator.max_depth", &[("AMPLIHACK_MAX_DEPTH", "lots")]);
    assert!(
        row.contains("# env (invalid: AMPLIHACK_MAX_DEPTH"),
        "row: {row:?}"
    );
}

#[test]
fn project_file_is_not_read_outside_git() {
    let fx = Fx::new();
    let nogit = fx.root.join("nogit");
    fs::create_dir_all(nogit.join(".amplihack")).unwrap();
    fs::write(
        nogit.join(".amplihack").join("config.toml"),
        "[recipe]\nverbose = true\n",
    )
    .unwrap();
    let out = fx.amp_in(&nogit, &["config", "get", "recipe.verbose"], &[]);
    assert_ok(&out);
    assert_eq!(stdout(&out), "false\n");
}

#[test]
fn project_search_stops_at_the_inner_git_root() {
    let fx = Fx::new();
    fx.write_project("[recipe]\nverbose = true\n");
    let inner = fx.project.join("vendor").join("sub");
    fs::create_dir_all(&inner).unwrap();
    fs::write(inner.join(".git"), "gitdir: ../../.git/modules/sub\n").unwrap();
    let out = fx.amp_in(&inner, &["config", "get", "recipe.verbose"], &[]);
    assert_ok(&out);
    assert_eq!(stdout(&out), "false\n", "outer repo's project file leaked");
}

#[test]
fn project_file_equal_to_user_file_is_read_once_as_user() {
    let fx = Fx::new();
    mark_repository(&fx.home);
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    let out = fx.amp_in(
        &fx.home,
        &["config", "get", "agent.binary", "--show-origin"],
        &[],
    );
    assert_ok(&out);
    assert!(stdout(&out).contains("# user ("), "{}", stdout(&out));
}

// ── diagnostics for hand-edited files ───────────────────────────────────────

#[test]
fn unknown_key_in_user_file_warns_and_rest_applies() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\nfavourite = \"x\"\n");
    let out = fx.amp(&["config", "get", "agent.binary"]);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "claude");
    let err = stderr(&out);
    assert!(
        err.contains(&format!(
            "warning: {}: agent.favourite:",
            fx.user_file().display()
        )),
        "stderr: {err}"
    );
    assert_eq!(
        err.matches("agent.favourite").count(),
        1,
        "warn once: {err}"
    );
}

#[test]
fn bad_value_in_user_file_falls_back_to_default() {
    let fx = Fx::new();
    fx.write_user("[orchestrator]\nmax_depth = 99\n");
    let out = fx.amp(&["config", "get", "orchestrator.max_depth", "--show-origin"]);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "3  # default");
    assert!(stderr(&out).contains("orchestrator.max_depth"));
}

#[test]
fn syntax_error_ignores_the_whole_file_with_a_position() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n[recipe\nverbose = true\n");
    let out = fx.amp(&["config", "list"]);
    assert_ok(&out);
    assert!(stdout(&out).contains("agent.binary = (unset)"));
    let text = stdout(&out);
    assert!(text.contains("Config problems:"), "{text}");
    assert!(
        stderr(&out).contains("TOML syntax error"),
        "{}",
        stderr(&out)
    );
    assert!(
        !stderr(&out).contains("binary = \"claude\""),
        "file contents must not be echoed"
    );
}

#[test]
fn user_only_key_in_project_file_is_reported_and_ignored() {
    let fx = Fx::new();
    fx.write_project("[node]\nheap_consent = true\n");
    let out = fx.amp(&["config", "list"]);
    assert_ok(&out);
    let text = stdout(&out);
    assert!(text.contains("node.heap_consent = (unset)"), "{text}");
    let problems = text
        .split("Config problems:")
        .nth(1)
        .expect("no problems block");
    assert!(
        problems.contains(&format!(
            "  {}: node.heap_consent:",
            fx.project_file().display()
        )),
        "{problems}"
    );
}

#[test]
fn control_characters_from_a_file_are_escaped() {
    let fx = Fx::new();
    fx.write_user("[agent]\n\"evil\\u001b[31m\\u202e\" = 1\n");
    let out = fx.amp(&["config", "list"]);
    assert_ok(&out);
    for stream in [stdout(&out), stderr(&out)] {
        assert!(!stream.contains('\u{1b}'), "raw ESC printed");
        assert!(!stream.contains('\u{202e}'), "raw bidi override printed");
    }
}

#[test]
fn a_fifo_config_file_does_not_hang() {
    let fx = Fx::new();
    fs::create_dir_all(fx.home.join(".amplihack")).unwrap();
    fs::set_permissions(
        fx.home.join(".amplihack"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let path = std::ffi::CString::new(fx.user_file().to_str().unwrap()).unwrap();
    // SAFETY: valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let out = fx.amp(&["config", "list"]);
    assert_ok(&out);
    assert!(stdout(&out).contains("Config problems:"));
}

#[test]
fn an_oversized_config_file_is_ignored() {
    let fx = Fx::new();
    let mut body = String::from("[agent]\nbinary = \"claude\"\n");
    body.push_str(&format!("# {}\n", "x".repeat(70 * 1024)));
    fx.write_user(&body);
    let out = fx.amp(&["config", "get", "agent.binary"]);
    assert_ok(&out);
    assert_eq!(stdout(&out), "\n");
}

#[test]
fn group_writable_user_file_loses_user_only_keys_only() {
    let fx = Fx::new();
    fx.write_user("[recipe]\nverbose = true\n[node]\nheap_consent = true\n");
    fs::set_permissions(fx.user_file(), fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(fx.get("recipe.verbose"), "true");
    let out = fx.amp(&["config", "get", "node.heap_consent"]);
    assert_ok(&out);
    assert_eq!(stdout(&out), "\n");
    assert!(
        stderr(&out).contains("node.heap_consent"),
        "{}",
        stderr(&out)
    );
}

// ── path / edit ─────────────────────────────────────────────────────────────

#[test]
fn path_lists_user_and_project_files() {
    let fx = Fx::new();
    let out = fx.amp(&["config", "path"]);
    assert_ok(&out);
    assert_eq!(
        stdout(&out),
        format!(
            "user: {}\nproject: {}\n",
            fx.user_file().display(),
            fx.project_file().display()
        )
    );
}

#[test]
fn path_outside_git_says_none() {
    let fx = Fx::new();
    let nogit = fx.root.join("nogit");
    fs::create_dir_all(&nogit).unwrap();
    let out = fx.amp_in(&nogit, &["config", "path"], &[]);
    assert_ok(&out);
    assert!(stdout(&out).contains("project: (none: not in a git repository)\n"));
}

#[test]
fn path_shows_legacy_only_while_migration_cannot_complete() {
    if is_root() {
        eprintln!("skipping: root ignores directory permissions");
        return;
    }
    let fx = Fx::new();
    fx.write_legacy(r#"{"node_options_consent": true, "node_options_limit_mb": 16384}"#);
    fs::set_permissions(
        fx.home.join(".amplihack"),
        fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    let out = fx.amp(&["config", "path"]);
    fs::set_permissions(
        fx.home.join(".amplihack"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert_ok(&out);
    assert!(
        stdout(&out).contains(&format!("legacy: {}\n", fx.legacy_file().display())),
        "{}",
        stdout(&out)
    );
}

#[test]
fn edit_passes_a_metacharacter_path_as_one_argument() {
    let fx = Fx::with_home_name("ho me;$(touch PWNED)'x");
    let editor = fx.tools.join("ed-stub");
    crate::support::exec_stub::write_executable(
        &editor,
        "#!/bin/sh\nprintf '[recipe]\\nverbose = true\\n' > \"$1\"\n",
        0o755,
    );
    let out = fx.amp_env(&["config", "edit"], &[("VISUAL", editor.to_str().unwrap())]);
    assert_ok(&out);
    assert!(fx.user_file().is_file());
    assert_eq!(mode(fx.user_file().parent().unwrap()), 0o700);
    assert!(!fx.project.join("PWNED").exists() && !fx.root.join("PWNED").exists());
    assert_eq!(fx.get("recipe.verbose"), "true");
}

#[test]
fn edit_that_leaves_problems_exits_1_and_keeps_the_file() {
    let fx = Fx::new();
    let editor = fx.tools.join("ed-stub");
    crate::support::exec_stub::write_executable(
        &editor,
        "#!/bin/sh\nprintf '[orchestrator]\\nmax_depth = 999\\n' > \"$1\"\n",
        0o755,
    );
    let out = fx.amp_env(&["config", "edit"], &[("EDITOR", editor.to_str().unwrap())]);
    assert_exit(&out, 1);
    assert!(fs::read_to_string(fx.user_file()).unwrap().contains("999"));
}

// ── legacy migration ────────────────────────────────────────────────────────

fn migrated(fx: &Fx) -> PathBuf {
    fx.home.join(".amplihack").join("config.json.migrated")
}

#[test]
fn migration_moves_consent_true_and_renames_the_legacy_file() {
    let fx = Fx::new();
    fx.write_legacy(r#"{"node_options_consent": true, "node_options_limit_mb": 16384}"#);
    assert_eq!(
        fx.get_origin("node.heap_consent"),
        format!("true  # user ({})", fx.user_file().display())
    );
    assert!(!fx.legacy_file().exists());
    assert!(migrated(&fx).is_file());
    // The limit is deliberately not migrated.
    assert_eq!(fx.get("node.max_old_space_mb"), "");
    assert_eq!(mode(&fx.user_file()), 0o600);
}

#[test]
fn migration_moves_consent_false() {
    let fx = Fx::new();
    fx.write_legacy(r#"{"node_options_consent": false, "node_options_limit_mb": 8192}"#);
    assert_eq!(fx.get("node.heap_consent"), "false");
    assert!(migrated(&fx).is_file());
}

#[test]
fn migration_runs_on_a_read_only_command_and_only_once() {
    let fx = Fx::new();
    fx.write_legacy(r#"{"node_options_consent": true}"#);
    assert_ok(&fx.amp(&["config", "path"]));
    assert!(fx.user_file().is_file(), "config path must migrate");
    let first = fs::read(fx.user_file()).unwrap();
    assert_ok(&fx.amp(&["config", "list"]));
    assert_eq!(fs::read(fx.user_file()).unwrap(), first);
}

#[test]
fn migration_does_not_overwrite_an_existing_user_value() {
    let fx = Fx::new();
    fx.write_user("[node]\nheap_consent = false\n");
    fx.write_legacy(r#"{"node_options_consent": true, "node_options_limit_mb": 16384}"#);
    assert_eq!(fx.get("node.heap_consent"), "false");
    assert!(!fx.legacy_file().exists());
}

#[test]
fn migration_keeps_git_identity_and_removes_only_the_two_keys() {
    let fx = Fx::new();
    fx.write_legacy(
        r#"{"node_options_consent": true, "node_options_limit_mb": 16384,
            "git_identity": {"name": "A", "email": "a@example.com"}}"#,
    );
    assert_eq!(fx.get("node.heap_consent"), "true");
    assert!(
        !migrated(&fx).exists(),
        "file with other keys is rewritten, not renamed"
    );
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fx.legacy_file()).unwrap()).unwrap();
    assert_eq!(v["git_identity"]["email"], "a@example.com");
    assert!(v.get("node_options_consent").is_none());
    assert!(v.get("node_options_limit_mb").is_none());
    assert!(fx.get_origin("node.heap_consent").contains("  # user ("));
}

#[test]
fn legacy_file_without_node_keys_is_never_written() {
    let fx = Fx::new();
    let body = r#"{"git_identity": {"name": "A"}}"#;
    fx.write_legacy(body);
    let out = fx.amp(&["config", "path"]);
    assert_ok(&out);
    assert!(!stdout(&out).contains("legacy:"));
    assert_eq!(fs::read_to_string(fx.legacy_file()).unwrap(), body);
    assert!(!fx.user_file().exists());
}

#[test]
fn unset_after_a_kept_legacy_file_stays_unset() {
    let fx = Fx::new();
    fx.write_legacy(r#"{"node_options_consent": true, "git_identity": {"name": "A"}}"#);
    assert_ok(&fx.amp(&["config", "unset", "node.heap_consent"]));
    assert_eq!(fx.get("node.heap_consent"), "");
    assert_eq!(fx.get("node.heap_consent"), "", "legacy value came back");
}

#[test]
fn malformed_legacy_json_is_reported_not_a_panic() {
    let fx = Fx::new();
    for body in ["{not json", "[1,2,3]", r#"{"node_options_consent": "yes"}"#] {
        fx.write_legacy(body);
        let out = fx.amp(&["config", "list"]);
        assert_ok(&out);
        assert!(!stderr(&out).contains("panicked"), "{}", stderr(&out));
        assert!(
            stdout(&out).contains("Config problems:"),
            "{}",
            stdout(&out)
        );
        assert_eq!(fs::read_to_string(fx.legacy_file()).unwrap(), body);
    }
}

#[test]
fn legacy_value_is_still_read_when_migration_cannot_write() {
    if is_root() {
        eprintln!("skipping: root ignores directory permissions");
        return;
    }
    let fx = Fx::new();
    fx.write_legacy(r#"{"node_options_consent": true, "node_options_limit_mb": 16384}"#);
    let dir = fx.home.join(".amplihack");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let out = fx.amp(&["config", "get", "node.heap_consent", "--show-origin"]);
    let again = fx.amp(&["config", "get", "node.heap_consent"]);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_ok(&out);
    assert_eq!(
        stdout(&out).trim_end(),
        format!("true  # legacy ({})", fx.legacy_file().display())
    );
    assert!(
        !stderr(&out).is_empty(),
        "a failed migration must be reported"
    );
    assert_eq!(stdout(&again).trim(), "true");
    assert!(fx.legacy_file().is_file());
}

#[test]
fn set_heap_consent_fails_when_legacy_keys_cannot_be_removed() {
    if is_root() {
        eprintln!("skipping: root ignores directory permissions");
        return;
    }
    let fx = Fx::new();
    // The user file is a symlink to a writable place, so step 2 can succeed
    // while step 3 (removing the legacy keys) cannot.
    let elsewhere = fx.root.join("dotfiles");
    fs::create_dir_all(&elsewhere).unwrap();
    let target = elsewhere.join("config.toml");
    fs::write(&target, "").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fx.write_legacy(r#"{"node_options_consent": true}"#);
    symlink(&target, fx.user_file()).unwrap();
    let dir = fx.home.join(".amplihack");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let out = fx.amp(&["config", "unset", "node.heap_consent"]);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_exit(&out, 1);
    assert!(fx.legacy_file().is_file());
}

// ── agent binary acceptance ─────────────────────────────────────────────────

/// Inside a tmux server a Copilot session started: the server's global
/// environment holds COPILOT_CLI=1 (see [`TMUX_STUB`]), and so does the pane.
const FOREIGN_TMUX: &[(&str, &str)] = &[
    ("AMPLIHACK_AGENT_BINARY", "copilot"),
    ("AMPLIHACK_AGENT_BINARY_SOURCE", "tmux_server:COPILOT_CLI"),
    ("COPILOT_CLI", "1"),
    ("TMUX", "/tmp/fake-tmux-socket,1,0"),
];

/// A live session marker the tmux server does not hold is the session this
/// process runs in, and outranks the config.
#[test]
fn a_live_session_marker_outranks_the_configured_binary() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"copilot\"\n");
    let out = fx.amp_env(&["agent-binary"], &[("CLAUDECODE", "1")]);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "claude (session_marker)");
}

/// `amplihack claude` exports `session:claude` (and Claude Code sets
/// CLAUDECODE); a recipe run in that session runs its steps under claude even
/// with agent.binary = copilot configured.
#[test]
fn amplihack_claude_runs_steps_under_claude_despite_a_configured_copilot() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"copilot\"\n");
    let launched = amplihack_cli::env_builder::EnvBuilder::new()
        .with_launched_agent_binary_from("claude", &|_| None)
        .build();
    let binary = launched["AMPLIHACK_AGENT_BINARY"].clone();
    let tag = launched["AMPLIHACK_AGENT_BINARY_SOURCE"].clone();
    assert_eq!(tag, "session:claude");
    for env in [
        vec![
            ("AMPLIHACK_AGENT_BINARY", binary.as_str()),
            ("AMPLIHACK_AGENT_BINARY_SOURCE", tag.as_str()),
        ],
        vec![
            ("AMPLIHACK_AGENT_BINARY", binary.as_str()),
            ("AMPLIHACK_AGENT_BINARY_SOURCE", tag.as_str()),
            ("CLAUDECODE", "1"),
        ],
    ] {
        recipe_run(&fx, &[], &env);
        assert_eq!(
            runner_env(&fx, "AMPLIHACK_AGENT_BINARY").as_deref(),
            Some("claude")
        );
        assert_eq!(
            runner_env(&fx, "AMPLIHACK_AGENT_BINARY_SOURCE").as_deref(),
            Some("session:claude")
        );
    }
}

/// Review of #1647: a live `amplihack copilot` session in a pane of a tmux
/// server another Copilot session started. COPILOT_CLI=1 matches the
/// server's, but the launch nonce does not: the session is its own, and it
/// outranks a configured claude. With the server's own nonce (the pane is
/// the server's copy), the config wins, and says so.
#[test]
fn a_live_amplihack_copilot_in_a_copilot_started_server_beats_the_config() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    let pane = |nonce: &str| {
        fx.amp_env(
            &["agent-binary"],
            &[
                ("TMUX", "/tmp/fake-tmux-socket,1,0"),
                ("COPILOT_CLI", "1"),
                ("AMPLIHACK_AGENT_BINARY", "copilot"),
                ("AMPLIHACK_AGENT_BINARY_SOURCE", "session:copilot"),
                ("AMPLIHACK_SESSION_NONCE", nonce),
            ],
        )
    };
    let own = pane("this-launch");
    assert_ok(&own);
    assert_eq!(stdout(&own).trim(), "copilot (session_marker)");
    let copy = pane("server-starter");
    assert_ok(&copy);
    assert_eq!(stdout(&copy).trim(), "claude (user_config)");
    assert!(
        stderr(&copy).contains("agent.binary from your config")
            && stderr(&copy).contains("COPILOT_CLI, a copilot session marker"),
        "{}",
        stderr(&copy)
    );
}

/// Review of #1647: `config unset agent.binary` wins over a stale inherited
/// `user_config:` value too.
#[test]
fn an_unset_config_drops_an_inherited_user_config_tag() {
    let fx = Fx::new();
    let inherited = [
        ("AMPLIHACK_AGENT_BINARY", "claude"),
        ("AMPLIHACK_AGENT_BINARY_SOURCE", "user_config:claude"),
    ];
    // No user config at all: the parent's reading still answers.
    let out = fx.amp_env(&["agent-binary"], &inherited);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "claude (user_config)");
    // A user config that no longer sets agent.binary: the tag is stale.
    assert_ok(&fx.amp(&["config", "set", "agent.binary", "claude"]));
    assert_ok(&fx.amp(&["config", "unset", "agent.binary"]));
    let out = fx.amp_env(&["agent-binary"], &inherited);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "copilot (default)");
}

/// Review of #1647: under the test guard a `--project` write obeys the same
/// rule as a read: never outside the temp dir.
#[test]
fn the_guard_refuses_a_project_write_outside_the_temp_dir() {
    let fx = Fx::new();
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("issue-1606-guard-project");
    fs::create_dir_all(&base).unwrap();
    let repo = tempfile::tempdir_in(&base).unwrap();
    mark_repository(repo.path());
    let out = fx.amp_in(
        repo.path(),
        &["config", "set", "recipe.verbose", "true", "--project"],
        &[],
    );
    assert_exit(&out, 1);
    assert!(
        stderr(&out).contains("AMPLIHACK_TEST_HOME_GUARD is set; refusing to write"),
        "{}",
        stderr(&out)
    );
    assert!(!repo.path().join(".amplihack").exists());
}

/// A `config set` takes effect under a tmux server whose panes carry an
/// older `user_config:` value: this process's own reading wins.
#[test]
fn own_config_outranks_an_inherited_user_config_tag() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"codex\"\n");
    let out = fx.amp_env(
        &["agent-binary"],
        &[
            ("AMPLIHACK_AGENT_BINARY", "claude"),
            ("AMPLIHACK_AGENT_BINARY_SOURCE", "user_config:claude"),
        ],
    );
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "codex (user_config)");
    let row = fx.list_row("agent.binary", &[]);
    assert!(row.starts_with("agent.binary = \"codex\""), "{row}");
}

#[test]
fn acceptance_configured_binary_beats_a_foreign_tmux_marker() {
    let fx = Fx::new();
    assert_ok(&fx.amp(&["config", "set", "agent.binary", "claude"]));
    let out = fx.amp_env(&["agent-binary"], FOREIGN_TMUX);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "claude (user_config)");
}

#[test]
fn acceptance_untagged_env_beats_the_configured_binary() {
    let fx = Fx::new();
    assert_ok(&fx.amp(&["config", "set", "agent.binary", "claude"]));
    let out = fx.amp_env(&["agent-binary"], &[("AMPLIHACK_AGENT_BINARY", "codex")]);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "codex (env)");
}

#[test]
fn shell_render_tags_user_config() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    let out = fx.amp_env(&["agent-binary", "--shell"], FOREIGN_TMUX);
    assert_ok(&out);
    let line = stdout(&out);
    assert!(
        line.trim_end().ends_with(
            "AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=user_config:claude"
        ),
        "{line}"
    );
    assert!(line.contains("-u COPILOT_CLI "), "{line}");
}

#[test]
fn without_config_the_resolver_is_unchanged() {
    let fx = Fx::new();
    let out = fx.amp_env(&["agent-binary"], FOREIGN_TMUX);
    assert_ok(&out);
    assert_eq!(stdout(&out).trim(), "copilot (env)");
}

fn log_pointers(out: &Output) -> Vec<serde_json::Value> {
    stderr(out)
        .lines()
        .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
        .map(|j| serde_json::from_str(j).unwrap())
        .collect()
}

fn recipe_run(fx: &Fx, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let recipe = fx.root.join("recipe.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let runner = fx.tools.join("recipe-runner-rs");
    let mut args = vec!["recipe", "run", recipe.to_str().unwrap()];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["-w", fx.project.to_str().unwrap()]);
    let mut all: Vec<(&str, &str)> = vec![
        ("RECIPE_RUNNER_RS_PATH", runner.to_str().unwrap()),
        ("AMPLIHACK_SKIP_POWER_STEERING", "1"),
        ("AMPLIHACK_SKIP_REFLECTION", "1"),
    ];
    all.extend_from_slice(env);
    let out = fx.amp_env(&args, &all);
    assert_ok(&out);
    out
}

fn runner_env(fx: &Fx, key: &str) -> Option<String> {
    let body = fs::read_to_string(fx.records.join("env.txt")).expect("runner not started");
    body.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_string))
}

#[test]
fn acceptance_log_pointer_records_user_config_binary_and_home() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\n");
    let home = fx.home.join(".amplihack");
    let mut env = FOREIGN_TMUX.to_vec();
    env.push(("AMPLIHACK_HOME", home.to_str().unwrap()));
    let out = recipe_run(&fx, &[], &env);
    let pointers = log_pointers(&out);
    assert!(pointers.len() >= 2, "early and final: {}", stderr(&out));
    for p in &pointers {
        assert_eq!(p["agent_binary"], "claude", "{p}");
        assert_eq!(p["agent_binary_source"], "user_config", "{p}");
        assert_eq!(p["amplihack_home"], home.to_str().unwrap(), "{p}");
    }
    assert_eq!(
        runner_env(&fx, "AMPLIHACK_AGENT_BINARY").as_deref(),
        Some("claude")
    );
    assert_eq!(
        runner_env(&fx, "AMPLIHACK_AGENT_BINARY_SOURCE").as_deref(),
        Some("user_config:claude")
    );
}

#[test]
fn log_pointer_records_explicit_env_source() {
    let fx = Fx::new();
    // The configured value is the one that loses. (Running steps under codex
    // would make the runner's codex capability check part of this test.)
    fx.write_user("[agent]\nbinary = \"codex\"\n");
    let out = recipe_run(&fx, &[], &[("AMPLIHACK_AGENT_BINARY", "claude")]);
    let pointers = log_pointers(&out);
    assert!(!pointers.is_empty(), "{}", stderr(&out));
    for p in pointers {
        assert_eq!(p["agent_binary"], "claude");
        assert_eq!(p["agent_binary_source"], "env");
    }
}

// ── recipe.step_timeout export ──────────────────────────────────────────────

#[test]
fn step_timeout_from_config_is_exported() {
    let fx = Fx::new();
    fx.write_user("[recipe]\nstep_timeout = 1800\n");
    recipe_run(&fx, &[], &[("AMPLIHACK_AGENT_BINARY", "claude")]);
    assert_eq!(
        runner_env(&fx, "AMPLIHACK_STEP_TIMEOUT").as_deref(),
        Some("1800")
    );
}

#[test]
fn inherited_step_timeout_env_beats_config() {
    let fx = Fx::new();
    fx.write_user("[recipe]\nstep_timeout = 1800\n");
    recipe_run(
        &fx,
        &[],
        &[
            ("AMPLIHACK_AGENT_BINARY", "claude"),
            ("AMPLIHACK_STEP_TIMEOUT", "60"),
        ],
    );
    assert_eq!(
        runner_env(&fx, "AMPLIHACK_STEP_TIMEOUT").as_deref(),
        Some("60")
    );
}

#[test]
fn step_timeout_flag_beats_env_and_config() {
    let fx = Fx::new();
    fx.write_user("[recipe]\nstep_timeout = 1800\n");
    recipe_run(
        &fx,
        &["--step-timeout", "30"],
        &[
            ("AMPLIHACK_AGENT_BINARY", "claude"),
            ("AMPLIHACK_STEP_TIMEOUT", "60"),
        ],
    );
    assert_eq!(
        runner_env(&fx, "AMPLIHACK_STEP_TIMEOUT").as_deref(),
        Some("30")
    );
}

#[test]
fn step_timeout_unset_everywhere_is_not_exported() {
    let fx = Fx::new();
    recipe_run(&fx, &[], &[("AMPLIHACK_AGENT_BINARY", "claude")]);
    assert_eq!(runner_env(&fx, "AMPLIHACK_STEP_TIMEOUT"), None);
}

// ── doctor ──────────────────────────────────────────────────────────────────

#[test]
fn doctor_prints_effective_config_with_origins() {
    let fx = Fx::new();
    fx.write_user("[agent]\nbinary = \"claude\"\nbogus = 1\n");
    let out = fx.amp(&["doctor"]);
    let text = stdout(&out);
    assert!(text.contains("Effective config"), "{text}");
    assert!(
        text.contains(&format!(
            "agent.binary = \"claude\"  # user ({})",
            fx.user_file().display()
        )),
        "{text}"
    );
    assert!(text.contains("Config problems:"), "{text}");
}
