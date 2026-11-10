//! Exercise the real CLI with a deterministic shell runner, without an account,
//! quota probes, polling, or access to the developer's home.
//! A `.git` boundary at the fixture root stops the launcher-context walk there.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use serde_json::Value;
use std::fs;

const MESSAGE: &str = "Agent terminated early due to an API error: You've hit your session limit · resets 9:20am (UTC)";

#[test]
fn reported_limit_survives_the_cli_boundary_without_transport_retry() {
    // Under a recipe run TMPDIR lies inside ~/.amplihack, whose `.claude/`
    // and `amplifier-bundle/` the CLI's ancestor scans would find ahead of
    // this fixture's (tests/support/scratch_base.rs).
    let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
    let root = tmp.path();
    // Bounds the CLI's launcher-context walk to this fixture.
    fs::create_dir(root.join(".git")).unwrap();
    assert!(root.join(".git").is_dir());
    let home = root.join("home");
    fs::create_dir(&home).unwrap();
    let runner = root.join("recipe-runner-rs");
    let payload = root.join("payload.json");
    let record = root.join("invocations");
    let result = serde_json::json!({
        "recipe_name": "issue-1390",
        "success": false,
        "step_results": [{
            "step_id": "limited-agent",
            "status": "error",
            "error": "agent step failed (exit 1)",
            "recent_stdout": [MESSAGE]
        }],
        "context": {}
    });
    fs::write(&payload, format!("{result}\n")).unwrap();
    // A shell fixture: version and capability probes are answered separately,
    // so the record counts recipe executions only. Paths reach it through the
    // environment, never as script text.
    write_executable(
        &runner,
        r#"#!/bin/sh
for a in "$@"; do
  case "$a" in
    --version) echo 'recipe-runner-rs 0.0.0 (fixture)'; exit 0 ;;
    --capabilities) echo '{"schema_version":1,"version":"fixture","capabilities":["codex_exec"]}'; exit 0 ;;
  esac
done
echo run >> "$ISSUE_1390_RECORD"
cat "$ISSUE_1390_PAYLOAD"
exit 1
"#,
        0o755,
    );

    let recipe = root.join("recipe.yaml");
    fs::write(&recipe, "name: issue-1390\ndescription: Synthetic usage limit\nversion: 1.0.0\nsteps:\n  - id: limited-agent\n    type: bash\n    command: 'true'\n").unwrap();
    let output = crate::isolated_amplihack::isolated(amplihack_bin())
        .args(["recipe", "run"])
        .arg(&recipe)
        .args(["--format", "json", "-w"])
        .arg(root)
        .current_dir(root)
        .env("HOME", &home)
        .env("AMPLIHACK_HOME", home.join(".amplihack"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CODEX_HOME", home.join(".codex"))
        .env(
            crate::isolated_amplihack::PROFILE_ROOT_ENV,
            home.join("profiles"),
        )
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
        // The `.git` boundary above keeps the working-directory walk for
        // launcher context inside this fixture. Separately, an explicit agent
        // binary takes precedence over any persisted context, and the
        // log-pointer assertions below prove that it did.
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("ISSUE_1390_RECORD", &record)
        .env("ISSUE_1390_PAYLOAD", &payload)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    crate::isolated_amplihack::assert_proc_root_honoured(&stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let pointers: Vec<Value> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("amplihack.recipe.log_pointer "))
        .map(|payload| serde_json::from_str(payload).expect("log pointer JSON"))
        .collect();
    assert!(!pointers.is_empty(), "{stderr}");
    for pointer in &pointers {
        assert_eq!(pointer["agent_binary"], "claude", "{pointer}");
        assert_eq!(pointer["agent_binary_source"], "env", "{pointer}");
    }
    let structured: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout={} stderr={stderr}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(structured["success"], false);
    let verdict = &structured["failure_classification"];
    assert_eq!(verdict["class"], "usage_limit");
    assert_eq!(verdict["signal"], "session limit");
    assert_eq!(verdict["retryable"], false);
    assert_eq!(verdict["attempt"], 1);
    assert_eq!(verdict["action"], "terminal");
    assert_eq!(verdict["completed_steps"], serde_json::json!([]));
    assert_eq!(
        verdict["failed_steps"],
        serde_json::json!(["limited-agent"])
    );
    assert!(
        verdict["reasoning"]
            .as_str()
            .unwrap()
            .contains("9:20am (UTC)")
    );
    assert!(verdict["evidence"].as_str().unwrap().contains(MESSAGE));
    let markers: Vec<Value> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("amplihack.recipe.failure_class "))
        .map(|payload| serde_json::from_str(payload).expect("structured diagnostic marker"))
        .collect();
    assert_eq!(markers, vec![verdict.clone()], "{stderr}");
    // Process completion is the synchronization event; no timed window.
    assert_eq!(fs::read_to_string(record).unwrap(), "run\n");
}
