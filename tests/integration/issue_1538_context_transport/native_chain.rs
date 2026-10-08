//! Controlled native producer/consumer tests, without model approvals or flags.
use super::{
    fixtures::*,
    native::{native_context, native_verdict},
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command};

fn shell_step(id: &str, command: String, output: &str) -> Value {
    json!({"id":id,"type":"bash","command":command,"output":output,
        "parse_json":true,"parse_json_required":true})
}

fn acceptance(spill: bool) {
    let binary = PathBuf::from(std::env::var_os("AMPLIHACK_CONTEXT_TRANSPORT_RUNNER").unwrap());
    let root =
        PathBuf::from(std::env::var_os("AMPLIHACK_CONTEXT_TRANSPORT_NATIVE_EVIDENCE").unwrap());
    assert!(binary.is_absolute() && binary.is_file());
    assert!(root.is_absolute() && !root.starts_with(repo()));
    for (case, token, noop, expected, code) in [
        (
            "implemented",
            "WORK_VERIFIED",
            false,
            "IMPLEMENTED_VERIFIED",
            0,
        ),
        (
            "hollow",
            "HOLLOW_SUCCESS",
            false,
            "FAILED_IMPLEMENTATION",
            1,
        ),
        ("noop", "INSUFFICIENT_EVIDENCE", true, "ALLOW_NO_OP", 0),
        ("reporting", "WORK_VERIFIED", false, "FAILED_REPORTING", 1),
    ] {
        let f = Fixture::new();
        let evidence = root
            .join(if spill { "chain-file" } else { "chain-object" })
            .join(case);
        fs::create_dir_all(&evidence).unwrap();
        let mut context = native_context(spill, f.temp.path());
        let object: Value =
            native_verdict(&context, serde_json::from_str(&verdict(token)).unwrap());
        let fixture_path = evidence.join("verdict.json");
        fs::write(&fixture_path, object.to_string()).unwrap();
        context["allow_no_op"] = json!(noop);
        // Inputs are controlled validation/report fixtures. Completion objects
        // are exclusively emitted by the current actual product producers.
        context["precommit_results"] = json!("controlled validation fixture");
        context["local_testing_gate"] = json!("controlled testing fixture");
        context["agentic_finalizer_narrative"] = json!(if case == "reporting" {
            ""
        } else {
            "controlled report"
        });
        let helper = repo().join("amplifier-bundle/tools/workflow_agentic_finalization.sh");
        let observed = format!(
            r#"set -euo pipefail
if [ -n "${{AMPLIHACK_CONTEXT_FILE:-}}" ]; then
  [ -z "${{RECIPE_VAR_implementation_terminal_evidence+x}}" ]
  transport=file
  jq -c '.implementation_terminal_evidence' < "$AMPLIHACK_CONTEXT_FILE"
else
  transport=object
  printf '%s' "$RECIPE_VAR_implementation_terminal_evidence"
fi
[ "$transport" = "{}" ]
[ -z "${{IMPLEMENTATION_COMPLETED+x}}" ]
[ -z "${{VERIFICATION_COMPLETED+x}}" ]
"#,
            if spill { "file" } else { "object" }
        );
        let validation_file = evidence.join("actual-validation.json");
        let validation_exit = evidence.join("actual-validation-exit.txt");
        let validation_err = evidence.join("actual-validation.stderr");
        let validation = format!(
            r#"set -uo pipefail
bash '{}' validate > '{}' 2> '{}'
code=$?
printf '%s\n' "$code" > '{}'
[ "$code" -eq {code} ] || exit 1
cat '{}'
"#,
            helper.display(),
            validation_file.display(),
            validation_err.display(),
            validation_exit.display(),
            validation_file.display()
        );
        let error2 = evidence.join("actual-error2.stderr");
        let error2_command = format!(
            r#"set -uo pipefail
bash '{}' invalid > '{}' 2> '{}'
code=$?
[ "$code" -eq 2 ] || exit 1
printf '{{"actual_exit":%s}}' "$code"
"#,
            helper.display(),
            evidence.join("actual-error2.stdout").display(),
            error2.display()
        );
        let steps = vec![
            shell_step(
                "typed-verdict",
                format!("cat '{}'", fixture_path.display()),
                "verdict_json",
            ),
            shell_step(
                "actual-implementation",
                body("workflow-tdd", "implementation-terminal-evidence"),
                "implementation_terminal_evidence",
            ),
            shell_step(
                "actual-verification",
                body("workflow-precommit-test", "verification-terminal-evidence"),
                "verification_terminal_evidence",
            ),
            shell_step(
                "observe-completion-transport",
                observed,
                "observed_implementation",
            ),
            shell_step(
                "actual-reporting",
                body("workflow-finalize", "finalizer-step-status"),
                "finalizer_step_status",
            ),
            shell_step(
                "actual-collect",
                format!("bash '{}' collect", helper.display()),
                "finalization_evidence",
            ),
            shell_step("actual-validate", validation, "workflow_result"),
            shell_step(
                "actual-complete",
                format!("bash '{}' complete", helper.display()),
                "complete_result",
            ),
            shell_step("actual-error2", error2_command, "error2_result"),
        ];
        let recipe = json!({"name":"controlled-native-completion-chain","version":"1.0.0","context":context,"steps":steps});
        let path = evidence.join("recipe.yaml");
        fs::write(&path, serde_yaml::to_string(&recipe).unwrap()).unwrap();
        let out = Command::new(&binary)
            .arg(&path)
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
            // Explicit stale negative inputs prove authoritative producers win.
            .env(
                "IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED",
                "false",
            )
            .env(
                "VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED",
                "false",
            )
            .current_dir(f.temp.path())
            .output()
            .unwrap();
        fs::write(evidence.join("stdout.json"), &out.stdout).unwrap();
        fs::write(evidence.join("stderr.log"), &out.stderr).unwrap();
        fs::write(evidence.join("exit.json"),json!({"code":out.status.code(),"controlled_fixture":true,"whole_workflow_acceptance":false}).to_string()).unwrap();
        assert!(
            out.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let r: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(r["success"], true);
        assert_eq!(r["step_results"].as_array().unwrap().len(), 9);
        let c = &r["context"];
        assert_eq!(c["verdict_json"], object);
        assert!(c["implementation_terminal_evidence"].is_object());
        assert!(c["verification_terminal_evidence"].is_object());
        assert_eq!(
            c["observed_implementation"],
            c["implementation_terminal_evidence"]
        );
        assert_eq!(
            c["finalization_evidence"]["completion"]["implementation_completed"],
            c["implementation_terminal_evidence"]["implementation_completed"]
        );
        assert_eq!(
            c["finalization_evidence"]["completion"]["verification_completed"],
            c["verification_terminal_evidence"]["verification_completed"]
        );
        assert_eq!(c["workflow_result"]["terminal_state"], expected);
        assert_eq!(
            c["workflow_result"]["terminal_success"],
            if code == 0 { "true" } else { "false" }
        );
        assert_eq!(
            c["complete_result"]["workflow_result"],
            c["workflow_result"]
        );
        assert_eq!(c["error2_result"]["actual_exit"], 2);
        assert_eq!(
            fs::read_to_string(validation_exit).unwrap().trim(),
            code.to_string()
        );
        if code != 0 {
            assert!(
                fs::read_to_string(validation_err)
                    .unwrap()
                    .contains(expected)
            );
        }
        assert!(
            fs::read_to_string(error2)
                .unwrap()
                .contains("requires mode")
        );
    }
}

#[test]
#[ignore = "requires verified final707 runner and retained integration-owned native evidence"]
fn real_native_object_completion_chain_preserves_strict_outcomes() {
    acceptance(false);
}

#[test]
#[ignore = "requires verified final707 runner and retained integration-owned native evidence"]
fn real_native_file_completion_chain_preserves_strict_outcomes() {
    acceptance(true);
}
