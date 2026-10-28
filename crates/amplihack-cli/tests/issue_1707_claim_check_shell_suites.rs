//! Issue #1707: run the issue-claim check's shell suites from `cargo test`.
//!
//! `amplifier-bundle/tools/workflow_issue_claim_check.sh` is tested by two
//! shell suites that drive the real helper against real git repositories and
//! a fake `gh`. CI runs them as separate Lint steps, so the workspace test
//! suite never exercised the helper. This test runs both suites and fails if
//! either reports a failed check.
//!
//! Each suite runs in a cleared environment, as `issue_1551_merge_guard.rs`
//! starts its bash children. Only [`KEPT_VARS`] (and `LC_*`) are carried over;
//! `HOME` points at a directory under the test's temp dir. `BASH_ENV`, every
//! `AMPLIHACK_*` variable, `CLAUDE_CONFIG_DIR`, `GH_*`, `GIT_*` and
//! `PR_NUMBER` are left unset, so neither the developer's `~/.amplihack`, the
//! profile root nor an auto-drive shell's `gh` merge guard reaches the suites
//! or the `bash` children they start. The suites set `AMPLIHACK_HOME` per
//! invocation where they need it.
//!
//! The suites read recipe YAML with `python3` and PyYAML. When the system
//! `python3` cannot import `yaml`, the test builds one virtual environment
//! with PyYAML using `uv` and puts its `bin` first on the suites' `PATH`.
//! The venv is built once per test process (behind a `OnceLock`) and removed
//! at exit. `cargo test` runs both tests in one process, so it builds the
//! venv once; `nextest` runs each test in its own process, so it builds it
//! twice, and the second build is served from uv's cache. `uv` runs with the
//! real `HOME` so that it uses the real `~/.cache/uv`: on a cold cache it
//! downloads PyYAML from PyPI, so such a host needs `uv` and network access
//! once. If the venv cannot be built the test fails rather than skipping, so
//! a missing dependency cannot pass as coverage.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Variables carried over from the test process, the same set
/// `tests/support/isolated_amplihack.rs` keeps. Everything else is cleared.
const KEPT_VARS: &[&str] = &[
    "PATH", "TMPDIR", "LANG", "TERM", "USER", "LOGNAME", "SHELL", "TZ",
];

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
        // this closure again and reports the same build failure, rather
        // than tripping over state left by the first attempt. Nothing is
        // kept or registered for removal at exit until the build succeeds.
        let dir = tempfile::Builder::new()
            .prefix("issue1707-pyyaml-")
            .tempdir()
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

        // The build succeeded, so this closure finishes and `DIR` is set:
        // it cannot run again, and `ROOT` is set exactly once. A static is
        // never dropped, so remove the venv at process exit instead.
        let root = dir.keep();
        static ROOT: OnceLock<PathBuf> = OnceLock::new();
        ROOT.set(root.clone())
            .expect("ROOT is set only after a successful build, which initialises DIR");
        extern "C" fn remove_at_exit() {
            if let Some(root) = ROOT.get() {
                let _ = std::fs::remove_dir_all(root);
            }
        }
        // SAFETY: registering a plain `extern "C"` function has no
        // preconditions; it runs once, at normal process exit.
        unsafe { libc::atexit(remove_at_exit) };

        let venv = root.join("venv");
        Some(venv.join("bin"))
    })
    .as_deref()
}

/// A `bash` command for `script` with a cleared environment: only
/// [`KEPT_VARS`] and `LC_*` from the test process, `PATH` with `pyyaml_dir`
/// in front when given, and `HOME` and `TMPDIR` under `tmp`.
fn isolated_suite_command(script: &Path, tmp: &Path, pyyaml_dir: Option<&Path>) -> Command {
    let mut command = Command::new("bash");
    command.arg(script).env_clear();
    for (key, value) in std::env::vars_os() {
        let Some(name) = key.to_str() else { continue };
        if KEPT_VARS.contains(&name) || name.starts_with("LC_") {
            command.env(&key, value);
        }
    }
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

/// The suites' environment carries nothing from the developer's or an
/// auto-drive shell beyond [`KEPT_VARS`]: no `BASH_ENV` (which would source
/// the merge guard's `gh` shim into every bash child), no `AMPLIHACK_*`, no
/// `CLAUDE_CONFIG_DIR`, no `GIT_*`/`GH_*`, no `PR_NUMBER`, and a `HOME` under
/// the test's temp dir.
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
            !name.starts_with("AMPLIHACK_")
                && !name.starts_with("GIT_")
                && !name.starts_with("GH_")
                && !matches!(
                    *name,
                    "BASH_ENV" | "ENV" | "CLAUDE_CONFIG_DIR" | "PR_NUMBER" | "EXISTING_BRANCH"
                ),
            "{name} reached the suite environment:\n{env}"
        );
    }
    let home = env
        .lines()
        .find_map(|line| line.strip_prefix("HOME="))
        .expect("HOME is set");
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
