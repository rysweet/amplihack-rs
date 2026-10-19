//! Direct transport controls here are distinct from an actual native runner run.
//! Opt-in runner acceptance is separate and must be invoked by integration.
use super::fixtures::*;
use serde_json::json;

pub(super) fn native_context(spill: bool, isolated_repo: &std::path::Path) -> serde_json::Value {
    let mut context = json!({"repo_path":isolated_repo.display().to_string()});
    if spill {
        // Mirror final707's measured aggregate budget. Fixture bytes enter via
        // stdout; transport selection occurs before any oversized env spawn.
        let arg_max = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
        assert!(arg_max > 0);
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_STACK, &mut limit) },
            0
        );
        let ceiling = if limit.rlim_cur > 0 && limit.rlim_cur != libc::RLIM_INFINITY {
            (arg_max as usize).min(limit.rlim_cur as usize / 4)
        } else {
            arg_max as usize
        };
        let budget = ceiling.saturating_sub(128 * 1024).max(256 * 1024);
        context["fixture_measured_env_budget"] = json!(budget);
    }
    context
}

pub(super) fn native_verdict(
    context: &serde_json::Value,
    mut object: serde_json::Value,
) -> serde_json::Value {
    if let Some(budget) = context["fixture_measured_env_budget"].as_u64() {
        // Produce over-budget JSON on stdout from a private fixture file. This
        // avoids recipe-size/argv limits and lets final707 choose file transport.
        let evidence = object["evidence"].as_array_mut().unwrap();
        for _ in 0..=(budget as usize / 48_000 + 2) {
            evidence.push(json!({"payload":"x".repeat(48_000)}));
        }
    }
    object
}

#[test]
fn implementation_recipe_emits_json_object_without_string_bridge() {
    let f = Fixture::new().env("RECIPE_VAR_verdict_json", verdict("WORK_VERIFIED"));
    let r = f.run(&body("workflow-tdd", "implementation-terminal-evidence"));
    r.helper_state("IMPLEMENTATION_COMPLETED");
    assert!(
        r.json().is_object(),
        "parse_json must receive genuine JSON, not JSON-encoded String"
    );
    assert_eq!(
        step("workflow-tdd", "implementation-terminal-evidence")["parse_json"],
        true
    );
}

#[test]
fn file_only_large_typed_context_retains_evidence_and_requested_json() {
    // Payload exceeds normal per-value environment budgets; no JSON in argv.
    let object = json!({"verdict":"WORK_VERIFIED","evidence":["x".repeat(200_000),{"nested":[1,2,3]}],
        "rationale":"bounded genuine object fixture"});
    let f = Fixture::new()
        .file(json!({"verdict_json":object,"implementation":"changed source","allow_no_op":false}));
    f.helper().helper_state("IMPLEMENTATION_COMPLETED");
    let r = f.gate();
    r.gate("APPROVED");
    let offset = r
        .stderr
        .find('{')
        .expect("native-compatible diagnostics include verdict object");
    let observed: serde_json::Value = serde_json::from_str(&r.stderr[offset..]).unwrap();
    assert_eq!(
        observed, object,
        "file spill must retain evidence ARRAY and object equality"
    );
}

fn real_runner_acceptance(spill: bool) {
    use std::{fs, path::PathBuf, process::Command};
    // Explicit reviewed binary/evidence inputs, with no historical-pin fallback.
    let binary = PathBuf::from(
        std::env::var_os("AMPLIHACK_CONTEXT_TRANSPORT_RUNNER")
            .expect("integration must supply its verified final707 runner"),
    );
    let evidence = PathBuf::from(
        std::env::var_os("AMPLIHACK_CONTEXT_TRANSPORT_NATIVE_EVIDENCE")
            .expect("integration must retain actual native receipts outside source"),
    );
    assert!(binary.is_absolute() && binary.is_file());
    assert!(evidence.is_absolute() && !evidence.starts_with(repo()));
    let evidence = evidence.join(if spill { "file-spill" } else { "object" });
    fs::create_dir_all(&evidence).unwrap();
    let f = Fixture::new();
    let context = native_context(spill, f.temp.path());
    let object = native_verdict(
        &context,
        json!({"verdict":"WORK_VERIFIED","evidence":["controlled-native-fixture",
        {"payload":"small","array":[1,2,3]}],
        "rationale":"transport test, not a real work-verifier approval"}),
    );
    let fixture_path = evidence.join("verdict-fixture.json");
    fs::write(&fixture_path, object.to_string()).unwrap();
    let observer = format!(
        r#"set -euo pipefail
if [ -n "${{AMPLIHACK_CONTEXT_FILE:-}}" ]; then
  value=$(jq -c '.verdict_json' < "$AMPLIHACK_CONTEXT_FILE")
  transport=file
  [ -z "${{RECIPE_VAR_verdict_json+x}}" ]
else
  value="${{RECIPE_VAR_verdict_json:-}}"
  transport=object
fi
[ "$transport" = "{}" ]
[ -z "${{VERDICT_JSON+x}}" ]
printf '%s' "$value" | jq -e 'type == "object"' >/dev/null
printf '%s\n' "$value"
"#,
        if spill { "file" } else { "object" }
    );
    let recipe = json!({"name":"issue1538-native-consumer-acceptance","version":"1.0.0",
    "context":context,"steps":[
    {"id":"typed-fixture-producer","type":"bash","parse_json":true,"parse_json_required":true,
        "command":format!("cat '{}'",fixture_path.display()),"output":"verdict_json"},
    {"id":"observe-native-transport","type":"bash","parse_json":true,"parse_json_required":true,
        "command":observer,"output":"observed_verdict"},
    {"id":"actual-implementation-helper","type":"bash","parse_json":true,"parse_json_required":true,
        "command":body("workflow-tdd","implementation-terminal-evidence"),"output":"implementation_terminal_evidence"},
    {"id":"actual-enforcer","type":"bash","command":body("workflow-tdd","step-08c-enforce-verdict"),
        "output":"implementation_noop_guard"}
    ]});
    let recipe_path = evidence.join("recipe.yaml");
    fs::write(&recipe_path, serde_yaml::to_string(&recipe).unwrap()).unwrap();
    let output = Command::new(&binary)
        .arg(&recipe_path)
        .arg("--no-auto-stage")
        .args(["--output-format", "json", "--audit-dir"])
        .arg(evidence.join("audit"))
        .env_clear()
        .env(
            "PATH",
            format!(
                "{}:{}",
                f.temp.path().join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("HOME", f.temp.path())
        .env("TMPDIR", f.temp.path())
        .env("AMPLIHACK_HOME", repo())
        .env("REPO_PATH", f.temp.path())
        .current_dir(f.temp.path())
        .output()
        .unwrap();
    fs::write(evidence.join("stdout.json"), &output.stdout).unwrap();
    fs::write(evidence.join("stderr.log"), &output.stderr).unwrap();
    fs::write(
        evidence.join("exit.json"),
        json!({"code":output.status.code(),"binary":binary,
        "controlled_fixture":true,"actual_work_verifier":false})
        .to_string(),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "native consumer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["success"], true);
    assert_eq!(
        result["context"]["verdict_json"], object,
        "requested parse_json OBJECT was lost"
    );
    assert_eq!(
        result["context"]["observed_verdict"], object,
        "actual transport lost ARRAY/object fields"
    );
    let implementation = &result["context"]["implementation_terminal_evidence"];
    assert!(
        implementation.is_object(),
        "permanent String bridge is forbidden"
    );
    assert_eq!(implementation["implementation_completed"], "true");
    assert_eq!(implementation["terminal_no_op"], "false");
    let rows = result["step_results"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "every actual native child must execute");
}

#[test]
#[ignore = "requires verified final707 runner and retained integration-owned native evidence"]
fn real_native_parse_json_object_reaches_actual_helper_and_enforcer() {
    real_runner_acceptance(false);
}

#[test]
#[ignore = "requires verified final707 runner and retained integration-owned native evidence"]
fn real_native_file_spill_reaches_actual_helper_and_enforcer() {
    real_runner_acceptance(true);
}
