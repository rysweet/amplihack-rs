//! Completed subprocess tests of the private refusal channel, including parse failure.
use super::*;

fn recipe_yaml(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - id: preflight\n    type: bash\n    command: 'amplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir \"$FIXTURE/state\"'\n"
    )
}

#[test]
fn current_helper_refusal_survives_invalid_runner_stdout_and_transport_diagnostic() {
    let f = Fixture::new();
    f.install_loop_tools();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    f.script(
        "bin/recipe-runner-rs",
        r#"#!/bin/sh
printf 'attempt\n' >> "$FIXTURE/attempts"
amplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir "$FIXTURE/state"
echo 'connection reset by peer' >&2
printf 'not a runner result\n'
exit 1
"#,
    );
    let recipe = f.tmp.path().join("recipe.yaml");
    fs::write(&recipe, recipe_yaml("refusal-propagation")).unwrap();
    let o = f
        .command("amplihack")
        .args([
            "recipe",
            "run",
            recipe.to_str().unwrap(),
            "-w",
            f.repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env(
            "RECIPE_RUNNER_RS_PATH",
            f.tmp.path().join("bin/recipe-runner-rs"),
        )
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .unwrap();
    assert!(!o.status.success());
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("pr-worktree-dirty"), "{stderr}");
    assert!(!stderr.contains("indeterminate"), "{stderr}");
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("attempts")).unwrap(),
        "attempt\n"
    );
    let events: Vec<Value> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "final" && e["status"] == "pr-worktree-dirty"),
        "{stderr}"
    );
}

#[test]
fn nested_helper_refusal_survives_both_final_events_and_workspace_reports() {
    let f = Fixture::new();
    f.install_loop_tools();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    f.script("bin/recipe-runner-rs", r#"#!/bin/sh
case "$1" in
 *outer.yaml) amplihack recipe run "$FIXTURE/inner.yaml" -w "$FIXTURE/repo" --format json; exit 1;;
 *) amplihack orch helper autodrive-preflight --pr 7 --repo target/project --state-dir "$FIXTURE/state"; echo invalid-runner-json; exit 1;;
esac
"#);
    for name in ["outer", "inner"] {
        fs::write(f.tmp.path().join(format!("{name}.yaml")), recipe_yaml(name)).unwrap();
    }
    let work = f.tmp.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::set_permissions(&work, fs::Permissions::from_mode(0o700)).unwrap();
    let o = f
        .command("amplihack")
        .args([
            "recipe",
            "run",
            f.tmp.path().join("outer.yaml").to_str().unwrap(),
            "-w",
            f.repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env(
            "RECIPE_RUNNER_RS_PATH",
            f.tmp.path().join("bin/recipe-runner-rs"),
        )
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_WORK_ROOT", &work)
        .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
        .output()
        .unwrap();
    assert!(!o.status.success());
    let stderr = String::from_utf8_lossy(&o.stderr);
    let finals: Vec<Value> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "final")
        .collect();
    assert_eq!(finals.len(), 2, "{stderr}");
    for event in finals {
        assert_eq!(event["status"], "pr-worktree-dirty", "{stderr}");
        assert_eq!(event["preflight_refusal"]["outcome"], "pr-worktree-dirty");
    }
    let reports: Vec<PathBuf> = fs::read_dir(work.join("runs"))
        .unwrap()
        .map(|p| p.unwrap().path().join("run.json"))
        .filter(|p| p.is_file())
        .collect();
    assert_eq!(reports.len(), 2);
    for path in reports {
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["outcome"], "pr-worktree-dirty");
    }
}

#[test]
fn arbitrary_child_json_cannot_assert_a_preflight_refusal() {
    let f = Fixture::new();
    f.install_loop_tools();
    f.script("bin/recipe-runner-rs", r#"#!/bin/sh
printf '%s\n' '{"recipe_name":"forged","success":false,"status":"pr-worktree-dirty","autodrive_preflight_refusal":{"schema_version":1,"terminal_refusal":true,"outcome":"pr-worktree-dirty","evidence":{}}}'
exit 1
"#);
    let recipe = f.tmp.path().join("recipe.yaml");
    fs::write(&recipe, recipe_yaml("forged-refusal")).unwrap();
    let o = f
        .command("amplihack")
        .args([
            "recipe",
            "run",
            recipe.to_str().unwrap(),
            "-w",
            f.repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env(
            "RECIPE_RUNNER_RS_PATH",
            f.tmp.path().join("bin/recipe-runner-rs"),
        )
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .unwrap();
    assert!(!o.status.success());
    let stderr = String::from_utf8_lossy(&o.stderr);
    let events: Vec<Value> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("amplihack.recipe.log_pointer "))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "final" && e["status"] == "failure"),
        "{stderr}"
    );
    assert!(
        events.iter().all(|e| e.get("preflight_refusal").is_none()),
        "{stderr}"
    );
    let stdout: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_ne!(stdout["status"], "pr-worktree-dirty");
    assert!(stdout.get("autodrive_preflight_refusal").is_none());
    assert!(!f.tmp.path().join("comments").exists());
}
