//! Exercise the real CLI with a deterministic Rust runner, without an account,
//! quota probes, polling, or access to the developer's home.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::Value;
use std::fs;
use std::process::Command;

const MESSAGE: &str = "Agent terminated early due to an API error: You've hit your session limit · resets 9:20am (UTC)";

#[test]
fn reported_limit_survives_the_cli_boundary_without_transport_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let home = root.join("home");
    fs::create_dir(&home).unwrap();
    let source = root.join("runner.rs");
    let runner = root.join("recipe-runner-rs");
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
    // Compile a standalone, std-only fixture. Version/capability probes are
    // answered separately so the record counts recipe executions only.
    fs::write(
        &source,
        format!(
            r#"
use std::io::Write;
fn main() {{
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {{
        println!("recipe-runner-rs 0.0.0 (fixture)");
        return;
    }}
    if args.iter().any(|a| a == "--capabilities") {{
        println!("{{}}", "{{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}}");
        return;
    }}
    let mut record = std::fs::OpenOptions::new().create(true).append(true)
        .open(std::env::var_os("ISSUE_1390_RECORD").unwrap()).unwrap();
    writeln!(record, "run").unwrap();
    println!("{{}}", {payload:?});
    std::process::exit(1);
}}
"#,
            payload = result.to_string()
        ),
    )
    .unwrap();
    let compiled = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&runner)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
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
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("ISSUE_1390_RECORD", &record)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    crate::isolated_amplihack::assert_proc_root_honoured(&stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
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
