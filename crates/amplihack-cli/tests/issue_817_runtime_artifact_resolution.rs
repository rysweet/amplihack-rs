//! Regression test for issue #817: the `checkpoint-after-implementation` step
//! (and sibling lifecycle steps) must resolve `workflow_runtime_artifacts.sh`
//! from the installed Amplihack home bundle when the target repository has no
//! `amplifier-bundle/` directory and `AMPLIHACK_HOME` is unset.
//!
//! The test stays coupled to the real recipe: it extracts the actual helper
//! resolution assignments from `workflow-tdd.yaml` and executes them in bash
//! under a controlled environment rather than asserting against a hand-copied
//! snippet.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

/// Extract the contiguous `RUNTIME_ARTIFACT_HELPER=` resolution assignments from
/// the `checkpoint-after-implementation` step. In `workflow-tdd.yaml` the only
/// lines containing `RUNTIME_ARTIFACT_HELPER=` belong to this single resolution
/// chain, so a text filter is sufficient and keeps the test pinned to the real
/// recipe contents.
fn checkpoint_resolution_snippet() -> String {
    let yaml = fs::read_to_string(repo_root().join("amplifier-bundle/recipes/workflow-tdd.yaml"))
        .expect("read workflow-tdd.yaml");
    let lines: Vec<String> = yaml
        .lines()
        .filter(|line| line.contains("RUNTIME_ARTIFACT_HELPER="))
        .map(|line| line.trim().to_string())
        .collect();

    assert!(
        !lines.is_empty(),
        "workflow-tdd.yaml must contain the runtime-artifact resolution chain"
    );
    assert!(
        lines
            .iter()
            .any(|line| line
                .contains(".amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh")),
        "resolution chain must include a ~/.amplihack fallback candidate (issue #817)"
    );

    lines.join("\n")
}

#[test]
fn checkpoint_resolves_runtime_helper_from_amplihack_home_when_repo_has_no_bundle() {
    let tmp = tempfile::TempDir::new().expect("tempdir");

    // Target repo / active worktree: deliberately has no amplifier-bundle/.
    let repo = tmp.path().join("target-repo");
    fs::create_dir_all(&repo).expect("create target repo dir");

    // Installed Amplihack home with the helper present (the issue's FOUND path).
    let home = tmp.path().join("home");
    let installed_helper =
        home.join(".amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh");
    fs::create_dir_all(installed_helper.parent().expect("helper parent"))
        .expect("create installed helper dir");
    fs::write(
        &installed_helper,
        "#!/usr/bin/env bash\npreflight_known_workflow_runtime_artifacts() { :; }\n",
    )
    .expect("write installed helper");

    let script = format!(
        "set -uo pipefail\n{}\nprintf '%s' \"$RUNTIME_ARTIFACT_HELPER\"\n",
        checkpoint_resolution_snippet()
    );

    let output = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(&repo)
        .env_remove("AMPLIHACK_HOME")
        .env_remove("WORKFLOW_RUNTIME_ARTIFACT_HELPER")
        .env("REPO_PATH", &repo)
        .env("HOME", &home)
        .output()
        .expect("run helper resolution snippet");

    let resolved = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "resolution snippet must execute cleanly\nstdout: {resolved}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        resolved.trim(),
        installed_helper.to_string_lossy(),
        "with no bundle under the target repo and AMPLIHACK_HOME unset, the helper \
         must resolve from the installed ~/.amplihack bundle"
    );
    assert!(
        installed_helper.is_file(),
        "resolved helper path must point at the file the workflow will source"
    );
}

#[test]
fn checkpoint_prefers_amplihack_home_over_installed_fallback() {
    let tmp = tempfile::TempDir::new().expect("tempdir");

    let repo = tmp.path().join("target-repo");
    fs::create_dir_all(&repo).expect("create target repo dir");

    // Explicit AMPLIHACK_HOME bundle.
    let amplihack_home = tmp.path().join("explicit-home");
    let explicit_helper =
        amplihack_home.join("amplifier-bundle/tools/workflow_runtime_artifacts.sh");
    fs::create_dir_all(explicit_helper.parent().expect("explicit parent"))
        .expect("create explicit helper dir");
    fs::write(&explicit_helper, "# explicit\n").expect("write explicit helper");

    // Installed ~/.amplihack bundle that must lose to the explicit home.
    let home = tmp.path().join("home");
    let installed_helper =
        home.join(".amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh");
    fs::create_dir_all(installed_helper.parent().expect("installed parent"))
        .expect("create installed helper dir");
    fs::write(&installed_helper, "# installed\n").expect("write installed helper");

    let script = format!(
        "set -uo pipefail\n{}\nprintf '%s' \"$RUNTIME_ARTIFACT_HELPER\"\n",
        checkpoint_resolution_snippet()
    );

    let output = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(&repo)
        .env("AMPLIHACK_HOME", &amplihack_home)
        .env_remove("WORKFLOW_RUNTIME_ARTIFACT_HELPER")
        .env("REPO_PATH", &repo)
        .env("HOME", &home)
        .output()
        .expect("run helper resolution snippet");

    let resolved = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "resolution snippet must execute cleanly"
    );
    assert_eq!(
        resolved.trim(),
        explicit_helper.to_string_lossy(),
        "an explicit AMPLIHACK_HOME bundle must take precedence over the ~/.amplihack fallback"
    );
}
