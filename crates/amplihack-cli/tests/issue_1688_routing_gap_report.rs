//! Runs the real `file-routing-bug` step of `smart-execute-routing.yaml`
//! (#1684, review of #1688).
//!
//! The step body was moved into `amplifier-bundle/tools/smart_routing_gap_report.sh`
//! and the step now only finds and runs that helper. This is the error-recovery
//! path that reports the orchestrator's own routing failures, so a rename, a
//! changed lookup or a bundle-layout change must fail here rather than turn into
//! a stderr warning nobody reads.
//!
//! Like `issue_817_runtime_artifact_resolution.rs`, the test reads the step body
//! out of the recipe instead of copying it, and runs it under bash in a
//! controlled environment:
//!
//! - a fake `HOME` holding the helper only at `~/.amplihack/amplifier-bundle/tools/`,
//! - a `PATH` built from symlinks to the few tools the helper needs, plus (in
//!   one case) a stub `gh` that records its arguments,
//! - this run's `RECIPE_VAR_*` values alongside stale bare decoys.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::support::exec_stub::write_executable;

const HELPER_REL: &str = "amplifier-bundle/tools/smart_routing_gap_report.sh";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

/// The `command` of the `file-routing-bug` step, read from the real recipe.
fn file_routing_bug_command() -> String {
    let path = repo_root().join("amplifier-bundle/recipes/smart-execute-routing.yaml");
    let text = fs::read_to_string(&path).expect("read smart-execute-routing.yaml");
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse recipe");
    let steps = doc["steps"].as_sequence().expect("recipe has steps");
    let step = steps
        .iter()
        .find(|s| s["id"].as_str() == Some("file-routing-bug"))
        .expect("smart-execute-routing.yaml must keep the file-routing-bug step");
    assert_eq!(
        step["type"].as_str(),
        Some("bash"),
        "file-routing-bug is a bash step"
    );
    assert_eq!(
        step["output"].as_str(),
        Some("infra_error_details"),
        "file-routing-bug must keep writing infra_error_details"
    );
    step["command"]
        .as_str()
        .expect("file-routing-bug has a command")
        .to_string()
}

/// A bin directory holding symlinks to the host tools the step and helper use.
/// `gh` is deliberately not linked; tests that want one add a stub.
fn tool_bin(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("create bin");
    for tool in ["bash", "cat", "head", "mktemp", "date", "hostname", "which"] {
        let found = ["/usr/bin", "/bin", "/usr/local/bin"]
            .iter()
            .map(|d| Path::new(d).join(tool))
            .find(|p| p.is_file());
        if let Some(real) = found {
            std::os::unix::fs::symlink(real, bin.join(tool)).expect("link tool");
        }
    }
    assert!(
        bin.join("bash").exists(),
        "bash must be available on the host"
    );
    assert!(
        bin.join("mktemp").exists(),
        "mktemp must be available on the host"
    );
    bin
}

/// Write a stub `gh` that records each argument NUL-terminated and prints a URL.
fn stub_gh(bin: &Path, record: &Path) {
    write_executable(
        &bin.join("gh"),
        format!(
            "#!/bin/bash\nprintf '%s\\0' \"$@\" > '{}'\necho https://github.com/example/repo/issues/4242\n",
            record.display()
        ),
        0o755,
    );
}

/// Install the real helper under `<home>/.amplihack/amplifier-bundle/tools/`.
fn install_helper_in_home(home: &Path) -> PathBuf {
    let dest = home.join(".amplihack").join(HELPER_REL);
    fs::create_dir_all(dest.parent().unwrap()).expect("create helper dir");
    fs::copy(repo_root().join(HELPER_REL), &dest).expect("copy helper");
    dest
}

struct Run {
    output: Output,
    stdout: String,
    stderr: String,
}

/// Run the step body with this run's RECIPE_VAR_* values and stale bare decoys.
/// `cwd` holds no bundle; AMPLIHACK_HOME and REPO_PATH are unset unless given.
fn run_step(tmp: &Path, bin: &Path, home: &Path, amplihack_home: Option<&Path>) -> Run {
    let cwd = tmp.join("target-repo");
    fs::create_dir_all(&cwd).expect("create cwd");
    let mut cmd = Command::new(bin.join("bash"));
    cmd.arg("-c")
        .arg(file_routing_bug_command())
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", bin)
        .env("HOME", home)
        // This run's own step outputs.
        .env("RECIPE_VAR_task_type", "Development")
        .env("RECIPE_VAR_workstream_count", "3")
        .env("RECIPE_VAR_force_single_workstream", "false")
        .env("RECIPE_VAR_recursion_guard", "ALLOWED")
        .env("RECIPE_VAR_adaptive_recipe", "default-workflow")
        .env("TASK_DESCRIPTION", "fix the routing gap for run 1688")
        // Bare names left by a parent run; none of these may be reported.
        .env("TASK_TYPE", "StaleDecoyType")
        .env("ADAPTIVE_RECIPE", "stale-decoy-recipe")
        .env("WORKSTREAM_COUNT", "9999")
        .env("FORCE_SINGLE_WORKSTREAM", "stale-decoy-force")
        .env("RECURSION_GUARD", "stale-decoy-guard");
    if let Some(h) = amplihack_home {
        cmd.env("AMPLIHACK_HOME", h);
    }
    let output = cmd.output().expect("run file-routing-bug step");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Run {
        output,
        stdout,
        stderr,
    }
}

fn assert_this_runs_values(text: &str, what: &str) {
    for want in [
        "task_type: `Development`",
        "workstream_count: `3`",
        "force_single_workstream: `false`",
        "recursion_guard: `ALLOWED`",
        "adaptive_recipe selected: `default-workflow`",
        "fix the routing gap for run 1688",
    ] {
        assert!(text.contains(want), "{what} must contain {want:?}:\n{text}");
    }
    for decoy in [
        "StaleDecoyType",
        "stale-decoy-recipe",
        "9999",
        "stale-decoy-force",
        "stale-decoy-guard",
    ] {
        assert!(
            !text.contains(decoy),
            "{what} must not report the bare decoy {decoy:?}:\n{text}"
        );
    }
}

#[test]
fn files_issue_with_this_runs_values_from_helper_in_installed_home() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = tool_bin(tmp.path());
    let record = tmp.path().join("gh-args");
    stub_gh(&bin, &record);
    let home = tmp.path().join("home");
    install_helper_in_home(&home);

    let run = run_step(tmp.path(), &bin, &home, None);

    assert!(
        run.output.status.success(),
        "step must exit 0\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert_eq!(
        run.stdout.trim(),
        "https://github.com/example/repo/issues/4242",
        "infra_error_details must be the filed issue URL\nstderr: {}",
        run.stderr
    );

    let raw = fs::read(&record).expect("stub gh must have been called");
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    assert_eq!(args.first().map(String::as_str), Some("issue"));
    assert_eq!(args.get(1).map(String::as_str), Some("create"));
    let value_of = |flag: &str| {
        let i = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("gh must get {flag}: {args:?}"));
        args.get(i + 1).cloned().expect("flag value")
    };
    let title = value_of("--title");
    assert_eq!(
        title,
        "smart-orchestrator routing gap: Development task, workstream_count=3"
    );
    assert_this_runs_values(&value_of("--body"), "gh --body");
    assert_eq!(value_of("--label"), "bug");
}

#[test]
fn without_gh_writes_diag_file_with_this_runs_values() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = tool_bin(tmp.path());
    assert!(
        !bin.join("gh").exists(),
        "this case must run with no gh on PATH"
    );
    let home = tmp.path().join("home");
    install_helper_in_home(&home);

    let run = run_step(tmp.path(), &bin, &home, None);

    assert!(
        run.output.status.success(),
        "step must exit 0 without gh\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    let line = run.stdout.trim();
    let file = line.strip_prefix("diag:").unwrap_or_else(|| {
        panic!(
            "stdout must be diag:<file>, got {line:?}\nstderr: {}",
            run.stderr
        )
    });
    let path = PathBuf::from(file);
    assert!(path.is_file(), "diag file {file} must exist");
    let body = fs::read_to_string(&path).expect("read diag file");
    let _ = fs::remove_file(&path);
    assert_this_runs_values(&body, "diag file");
}

#[test]
fn missing_helper_exits_zero_with_sentinel() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = tool_bin(tmp.path());
    let record = tmp.path().join("gh-args");
    stub_gh(&bin, &record);
    // HOME has neither ~/.amplihack nor ~/.copilot; cwd has no bundle;
    // AMPLIHACK_HOME and REPO_PATH are unset.
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("create home");

    let run = run_step(tmp.path(), &bin, &home, None);

    assert!(
        run.output.status.success(),
        "a missing helper must not fail recovery\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert_eq!(
        run.stdout.trim(),
        "diag:helper-not-found",
        "infra_error_details must record the missing helper\nstderr: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains("smart_routing_gap_report.sh"),
        "the warning must name the missing helper: {}",
        run.stderr
    );
    assert!(
        !record.exists(),
        "gh must not be called when the helper is missing"
    );
}

#[test]
fn amplihack_home_helper_wins_over_installed_home() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin = tool_bin(tmp.path());
    let home = tmp.path().join("home");
    install_helper_in_home(&home);
    let explicit = tmp.path().join("explicit-home");
    let explicit_helper = explicit.join(HELPER_REL);
    fs::create_dir_all(explicit_helper.parent().unwrap()).expect("create explicit dir");
    fs::write(&explicit_helper, "echo from-amplihack-home\n").expect("write explicit helper");

    let run = run_step(tmp.path(), &bin, &home, Some(&explicit));

    assert!(run.output.status.success(), "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout.trim(),
        "from-amplihack-home",
        "AMPLIHACK_HOME must be searched before ~/.amplihack\nstderr: {}",
        run.stderr
    );
}
