//! Issue #1707: run the issue-claim check's shell suites from `cargo test`.
//!
//! `amplifier-bundle/tools/workflow_issue_claim_check.sh` is tested by two
//! shell suites that drive the real helper against real git repositories and
//! a fake `gh`. CI runs them as separate Lint steps, so the workspace test
//! suite never exercised the helper. This test runs both suites and fails if
//! either reports a failed check.
//!
//! Each suite's `bash` is started through the shared helper
//! [`crate::isolated_amplihack::isolated`], the one way a test starts a child
//! in this harness. There is no second keep-list here: the suites get exactly
//! the environment that helper builds (its `KEPT_VARS`, `CARGO*`, `RUST*` and
//! `LC_*`, plus `AMPLIHACK_TEST_HOME_GUARD=1` and
//! `AMPLIHACK_ROTATE_AUTOSTART=0`), so a suite that one day starts
//! `amplihack` gets the same safeguards as every other test. On top of that
//! this file sets `HOME` and `TMPDIR` to the test's own temp dir and puts the
//! PyYAML venv, when one is needed, first on `PATH`. `BASH_ENV`, every other
//! `AMPLIHACK_*` variable, `CLAUDE_CONFIG_DIR`, `GH_*`, `GIT_*` and
//! `PR_NUMBER` are left unset, so neither the developer's `~/.amplihack`, the
//! profile root nor an auto-drive shell's `gh` merge guard reaches the suites
//! or the `bash` children they start. The suites set `AMPLIHACK_HOME` per
//! invocation where they need it.
//!
//! The suites read recipe YAML with `python3` and PyYAML. When the system
//! `python3` cannot import `yaml`, the test builds one virtual environment
//! with PyYAML using `uv` and puts its `bin` first on the suites' `PATH`.
//! The venv is built once per test process (behind a `OnceLock`) inside the
//! shared per-process temp home from
//! [`crate::isolated_amplihack::shared_temp_home`], so it is removed at exit
//! together with that directory. `cargo test` runs both tests in one process,
//! so it builds the venv once; `nextest` runs each test in its own process,
//! so it builds it twice, and the second build is served from uv's cache.
//! `uv` runs with the real `HOME` so that it uses the real `~/.cache/uv`: on
//! a cold cache it downloads PyYAML from PyPI, so such a host needs `uv` and
//! network access once. If the venv cannot be built the test fails rather
//! than skipping, so a missing dependency cannot pass as coverage.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::isolated_amplihack::{HOME_GUARD_ENV, ROTATE_AUTOSTART_ENV, isolated, shared_temp_home};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns a directory to prepend to `PATH` so that `python3` can import
/// `yaml`, or `None` when the system `python3` already can. Built at most
/// once per test process.
///
/// The fallback is a virtual environment built by `uv`. A real interpreter is
/// needed on `PATH`: a `python3` script that calls `uv run` would be found
/// again by uv's own interpreter discovery, and the suites run with a fake
/// `HOME`, which would hide uv's cache.
fn pyyaml_path_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if command_succeeds("python3", &["-c", "import yaml"]) {
            return None;
        }
        // Build the venv in a `TempDir` that is still owned here: if the
        // build fails, the assert below unwinds, the `TempDir` drops and the
        // directory is removed. `get_or_init` leaves `DIR` uninitialised
        // after a panic, so under `cargo test` the other suite test runs
        // this closure again in a fresh directory and reports the same build
        // failure, rather than tripping over state left by the first attempt.
        let dir = tempfile::Builder::new()
            .prefix("issue1707-pyyaml-")
            .tempdir_in(shared_temp_home())
            .expect("tempdir for the PyYAML venv");
        let venv = dir.path().join("venv");
        let venv_str = venv.to_str().expect("utf-8 temp path");
        let python = venv.join("bin/python3");
        let python_str = python.to_str().expect("utf-8 temp path");
        assert!(
            command_succeeds("uv", &["venv", "--quiet", "--python", "python3", venv_str])
                && command_succeeds(
                    "uv",
                    &[
                        "pip", "install", "--quiet", "--python", python_str, "pyyaml"
                    ]
                )
                && command_succeeds(python_str, &["-c", "import yaml"]),
            "the claim-check suites need python3 with PyYAML: install python3-yaml, \
             or uv (which fetches PyYAML from PyPI on a cold cache)"
        );

        // The build succeeded. Keep the directory: it lives under the shared
        // temp home, which `isolated_amplihack` removes at process exit.
        Some(dir.keep().join("venv").join("bin"))
    })
    .as_deref()
}

/// A `bash` command for `script`, built by the shared [`isolated`] helper,
/// with `HOME` and `TMPDIR` under `tmp` and `pyyaml_dir`, when given, first
/// on `PATH`.
fn isolated_suite_command(script: &Path, tmp: &Path, pyyaml_dir: Option<&Path>) -> Command {
    let mut command = isolated("bash");
    command.arg(script);
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(dir) = pyyaml_dir {
        let mut joined = dir.as_os_str().to_os_string();
        joined.push(":");
        joined.push(&path);
        path = joined;
    }
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).expect("create the suite HOME");
    command
        .env("PATH", path)
        .env("HOME", home)
        .env("TMPDIR", tmp);
    command
}
fn run_suite(script: &str) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = repo_root();
    let output = isolated_suite_command(&root.join(script), tmp.path(), pyyaml_path_dir())
        .current_dir(&root)
        .output()
        .expect("run shell suite");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let failed: Vec<&str> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("FAIL"))
        .collect();
    assert!(
        output.status.success() && failed.is_empty(),
        "{script} failed (status {:?}); failing checks: {failed:#?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
}

/// The suites' environment is the shared [`isolated`] helper's, so it carries
/// nothing from the developer's or an auto-drive shell beyond that helper's
/// keep-list: no `BASH_ENV` (which would source the merge guard's `gh` shim
/// into every bash child), no `AMPLIHACK_*` other than the helper's two
/// safeguards, no `CLAUDE_CONFIG_DIR`, no `GIT_*`/`GH_*`, no `PR_NUMBER`, and
/// a `HOME` under the test's temp dir. The safeguards themselves must be set,
/// which shows the command came from the shared helper and not a copy.
#[test]
fn issue_1707_suite_environment_is_isolated() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let probe = tmp.path().join("probe.sh");
    std::fs::write(&probe, "env\n").expect("write probe");
    let output = isolated_suite_command(&probe, tmp.path(), None)
        .output()
        .expect("run probe");
    assert!(output.status.success(), "probe failed: {output:?}");
    let env = String::from_utf8_lossy(&output.stdout);
    let names: Vec<&str> = env
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    for name in &names {
        assert!(
            (!name.starts_with("AMPLIHACK_")
                || *name == HOME_GUARD_ENV
                || *name == ROTATE_AUTOSTART_ENV)
                && !name.starts_with("GIT_")
                && !name.starts_with("GH_")
                && !matches!(
                    *name,
                    "BASH_ENV" | "ENV" | "CLAUDE_CONFIG_DIR" | "PR_NUMBER" | "EXISTING_BRANCH"
                ),
            "{name} reached the suite environment:\n{env}"
        );
    }
    let value_of = |name: &str| {
        env.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
    };
    assert_eq!(value_of(HOME_GUARD_ENV), Some("1"), "{env}");
    assert_eq!(value_of(ROTATE_AUTOSTART_ENV), Some("0"), "{env}");
    let home = value_of("HOME").expect("HOME is set");
    assert!(
        Path::new(home).starts_with(tmp.path()),
        "HOME {home} is outside the test temp dir {}",
        tmp.path().display()
    );
}

#[test]
fn issue_1361_duplicate_pr_claim_check_suite_passes() {
    run_suite("tests/issue_1361_duplicate_pr_claim_check.sh");
}

#[test]
fn issue_1448_review_is_not_a_duplicate_suite_passes() {
    run_suite("tests/issue_1448_review_is_not_a_duplicate.sh");
}
