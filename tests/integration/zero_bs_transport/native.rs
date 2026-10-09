//! Native String output must reach the subsequent actual Bash gate on held707.
use super::*;
use serde_json::json;
use std::{io::Write, os::unix::fs::OpenOptionsExt};
fn save(root: &std::path::Path, name: &str, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(name))
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
#[test]
#[ignore = "requires protected launch's genuine RECIPE_RUNNER_RS_PATH and /d0 retained evidence"]
fn held707_bounded_string_launches_actual_following_bash_gate() {
    let runner = PathBuf::from(
        std::env::var_os("RECIPE_RUNNER_RS_PATH")
            .expect("supported genuine runner binding required"),
    );
    let root = PathBuf::from(
        std::env::var_os("AMPLIHACK_ZERO_BS_NATIVE_EVIDENCE")
            .expect("retained /d0 evidence required"),
    );
    assert!(runner.is_absolute() && runner.is_file());
    assert!(root.starts_with("/d0"));
    let evidence = tempfile::Builder::new()
        .prefix("native-zero-bs-")
        .tempdir_in(&root)
        .unwrap()
        .keep();
    let mut f = Fixture::new();
    f.temp.disable_cleanup(true);
    f.large();
    let yaml = super::super::load_yaml("workflow-pr-review");
    let steps: Vec<serde_json::Value> = yaml["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter(|s| [STEP, "step-19d-verification-gate"].contains(&s["id"].as_str().unwrap()))
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
    assert_eq!(steps.len(), 2);
    let recipe = json!({"name":"bounded-zero-bs-native-regression","version":"1.0.0",
        "context":{"repo_path":f.repo(),"worktree_setup":{"worktree_path":f.repo()},
            "terminal_state":{"terminal_success":"false"},"philosophy_check":"controlled review input", "patterns_check":"controlled review input"},"steps":steps});
    save(
        &evidence,
        "actual-recipe.yaml",
        serde_yaml::to_string(&recipe).unwrap().as_bytes(),
    );
    let path = evidence.join("actual-recipe.yaml");
    let audit = evidence.join("audit");
    let argv = vec![
        runner.to_string_lossy().into_owned(),
        path.to_string_lossy().into_owned(),
        "--no-auto-stage".into(),
        "--output-format".into(),
        "json".into(),
        "--audit-dir".into(),
        audit.to_string_lossy().into_owned(),
    ];
    save(&evidence, "argv.json", json!(argv).to_string().as_bytes());
    save(
        &evidence,
        "selected-bash.json",
        json!({"AMPLIHACK_BASH":f.shell}).to_string().as_bytes(),
    );
    let before = fs::read(super::super::recipe_path("workflow-pr-review")).unwrap();
    save(&evidence, "source-before.yaml", &before);
    let out = f
        .child(&runner)
        .env("AMPLIHACK_BASH", &f.shell)
        .args(&argv[1..])
        .output()
        .unwrap();
    save(&evidence,"exit.json",json!({"exit_code":out.status.code(),"controlled_fixture":true,"canonical_workflow_success":false}).to_string().as_bytes());
    save(&evidence, "stdout.json", &out.stdout);
    save(&evidence, "stderr.log", &out.stderr);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["success"], true);
    let string = result["context"]["zero_bs_check"]
        .as_str()
        .expect("genuine native String");
    assert!(string.len() <= 8192);
    let j: Value =
        serde_json::from_str(string.strip_prefix("=== ZERO-BS SUMMARY ===\n").unwrap()).unwrap();
    let data = report(&j);
    assert!(data.len() > 202_307);
    save(&evidence, "full-report", &data);
    assert!(
        result["context"]["step_19_gate_status"]
            .as_str()
            .unwrap()
            .contains("GATE PASSED")
    );
    assert_eq!(result["step_results"].as_array().unwrap().len(), 2);
    for row in result["step_results"].as_array().unwrap() {
        assert_eq!(row["status"], "completed");
    }
    // A real operational nonzero must stop native execution before the gate.
    f.shim("git", "if [ \"$1\" = grep ]; then echo native-tracked-failure >&2; exit 23; fi\nexec /usr/bin/git \"$@\"");
    let mut negative_argv = argv.clone();
    *negative_argv.last_mut().unwrap() = evidence
        .join("failure-audit")
        .to_string_lossy()
        .into_owned();
    save(
        &evidence,
        "failure-argv.json",
        json!(negative_argv).to_string().as_bytes(),
    );
    let negative = f
        .child(&runner)
        .env("AMPLIHACK_BASH", &f.shell)
        .args(&negative_argv[1..])
        .output()
        .unwrap();
    save(
        &evidence,
        "failure-exit.json",
        json!({"exit_code":negative.status.code(),"expected_child_exit":23})
            .to_string()
            .as_bytes(),
    );
    save(&evidence, "failure-stdout.json", &negative.stdout);
    save(&evidence, "failure-stderr.log", &negative.stderr);
    assert_eq!(negative.status.code(), Some(1));
    let failed: Value = serde_json::from_slice(&negative.stdout).unwrap();
    assert_eq!(failed["success"], false);
    assert_eq!(failed["step_results"].as_array().unwrap().len(), 1);
    assert_eq!(failed["step_results"][0]["status"], "failed");
    assert!(
        failed["step_results"][0]["error"]
            .as_str()
            .unwrap()
            .contains("23")
    );
    assert!(failed["context"].get("step_19_gate_status").is_none());
    let after = fs::read(super::super::recipe_path("workflow-pr-review")).unwrap();
    save(&evidence, "source-after.yaml", &after);
    assert_eq!(before, after);
    let fixture_path = f.temp.keep();
    println!(
        "retained native evidence {} fixture {}",
        evidence.display(),
        fixture_path.display()
    );
}
