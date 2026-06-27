//! Tests for issue #820: `quality-audit-cycle` `merge-validations` step fails
//! on mixed validator output.
//!
//! Root cause: `merge-validations` wrote each validator agent's RAW output
//! (`{{validation_agent_N}}`) to a temp file and fed it straight to
//! `jq -n --slurpfile`. Validators emit their JSON verdict wrapped in
//! markdown/prose (typically a ```json fence) and sometimes with leading log
//! preamble, so `jq` aborted the entire audit cycle with
//! `Bad JSON ... Invalid numeric literal` before the fix/verify/summary phases
//! could run.
//!
//! Original #820 fix: normalize each validator's output through the
//! `amplihack orch helper extract-json` helper BEFORE `jq --slurpfile`.
//!
//! SUPERSEDED BY #833: that approach introduced a silent binary-PATH
//! dependency — when the `amplihack` binary is not on `PATH`, every validator
//! degraded to `{}` (silent zero-BS violation) and the original `jq` crash
//! could still reproduce. #833 replaces it with a SELF-CONTAINED `extract_verdict`
//! jq/sed normalizer embedded directly in the step (no external binary). The
//! structural and end-to-end tests below were updated to the #833 contract;
//! the per-validator diagnostic now says a validator's `output unparseable`
//! (naming `vN`) instead of "no parseable JSON object". See
//! `issue_833_merge_validations_json_tolerance.rs` for the full #833 contract.
//!
//! These tests cover three layers:
//!   1. Structural — the recipe wires the self-contained normalizer + diagnostic.
//!   2. Behavioral (pure Rust) — the `extract_json` helper (still used by
//!      smart-orchestrator) recovers a JSON object from mixed-format inputs.
//!   3. End-to-end (graceful skip) — the real `merge-validations` bash command
//!      no longer emits `Bad JSON` on mixed output and produces valid merged
//!      JSON. Skipped when `jq` is unavailable.

use amplihack_cli::commands::orch::extract_json;
use serde_yaml::Value;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn recipe_path() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes/quality-audit-cycle.yaml")
}

fn load_recipe() -> Value {
    let path = recipe_path();
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn merge_validations_command() -> String {
    let recipe = load_recipe();
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have steps");
    let step = steps
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some("merge-validations"))
        .expect("merge-validations step must exist");
    step.get("command")
        .and_then(Value::as_str)
        .expect("merge-validations must have a command field")
        .to_string()
}

// =========================================================================
// Layer 1: Structural — the recipe must normalize validator output through a
// SELF-CONTAINED extractor (no `amplihack` binary dependency, per #833) before
// jq, with a diagnostic fallback.
// =========================================================================

#[test]
fn merge_validations_normalizes_without_extract_json_binary() {
    let cmd = merge_validations_command();
    // #833: the original #820 `orch helper extract-json` dependency was removed
    // (it degraded silently to `{}` when off PATH). Normalization must be
    // self-contained.
    assert!(
        !cmd.contains("orch helper extract-json"),
        "#833 (supersedes #820): merge-validations must NOT depend on the \
         `amplihack orch helper extract-json` binary.\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("extract_verdict"),
        "#833: merge-validations must normalize each validator's raw output \
         through the self-contained `extract_verdict` jq/sed function before \
         jq.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_feeds_normalized_files_to_jq() {
    let cmd = merge_validations_command();
    // The jq --slurpfile inputs must come from the NORMALIZED files, not the
    // raw heredoc temp files.
    for f in ["V1_FILE", "V2_FILE", "V3_FILE"] {
        assert!(
            cmd.contains("--slurpfile") && cmd.contains(&format!("\"${f}\"")),
            "#820: jq must slurp the normalized file via ${f}.\nCommand:\n{cmd}"
        );
    }
    assert!(
        cmd.contains("export V1_FILE=\"$_V1_NORMFILE\"")
            && cmd.contains("export V2_FILE=\"$_V2_NORMFILE\"")
            && cmd.contains("export V3_FILE=\"$_V3_NORMFILE\""),
        "#820: V*_FILE (the jq inputs) must point at the normalized files \
         ($_V*_NORMFILE), not the raw heredoc temp files.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_normalization_runs_before_jq() {
    let cmd = merge_validations_command();
    let extract_pos = cmd
        .find("extract_verdict")
        .expect("must contain the self-contained extract_verdict normalizer");
    let jq_pos = cmd
        .find("jq -n")
        .or_else(|| cmd.find("jq "))
        .expect("must contain jq invocation");
    assert!(
        extract_pos < jq_pos,
        "#833: validator output must be normalized BEFORE jq runs.\n\
         extract_verdict at {extract_pos}, jq at {jq_pos}."
    );
}

#[test]
fn merge_validations_emits_diagnostic_and_preserves_artifact() {
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("WARNING") && cmd.contains("output unparseable"),
        "#833: when a validator produced non-empty output with no parseable \
         JSON, merge-validations must emit a targeted `output unparseable` \
         diagnostic instead of a brittle jq crash.\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("Raw output preserved at:"),
        "#833: the diagnostic must preserve the offending validator's raw \
         output as an artifact and report its path.\nCommand:\n{cmd}"
    );
    // The diagnostic must name which validator failed (vN).
    assert!(
        cmd.contains("[merge-validations] WARNING: validator"),
        "#833: the diagnostic must name the offending validator (vN) so the \
         failing prompt/output can be repaired.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_trap_cleans_normalized_files() {
    let cmd = merge_validations_command();
    // The EXIT trap must remove the new normalized temp files too (no leaks).
    let trap_line = cmd
        .lines()
        .find(|l| l.contains("trap") && l.contains("EXIT"))
        .expect("#820: merge-validations must keep a trap ... EXIT cleanup line");
    for f in ["_V1_NORMFILE", "_V2_NORMFILE", "_V3_NORMFILE"] {
        assert!(
            trap_line.contains(f),
            "#820: EXIT trap must clean up normalized temp file ${f}.\nTrap: {trap_line}"
        );
    }
}

#[test]
fn merge_validations_still_uses_slurpfile_merge() {
    // The deterministic vote-counting merge (slurpfile) must be preserved —
    // we normalize the inputs, we do not replace the merge logic.
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("--slurpfile v1") && cmd.contains("group_by"),
        "#820: the deterministic slurpfile/group_by merge must be preserved."
    );
}

#[test]
fn recipe_parses_as_valid_yaml() {
    let _ = load_recipe();
}

// =========================================================================
// Layer 2: Behavioral (pure Rust) — the `extract_json` helper (still used by
// smart-orchestrator; the recipe itself is now self-contained per #833) must
// recover the validator JSON object from exactly the mixed-format inputs that
// used to crash jq with "Bad JSON".
// =========================================================================

#[test]
fn extract_json_recovers_validator_object_from_fenced_prose() {
    // Real-world validator output: log preamble + prose + a ```json fence.
    let raw = "Let me validate the findings now.\n\
        I read src/foo.rs:42 — this is a genuine silent fallback.\n\n\
        ```json\n\
        {\"validator\":\"agent-1\",\"cycle\":1,\"validated\":[{\"finding_id\":1,\
        \"verdict\":\"confirmed\",\"new_severity\":\"high\",\"reasoning\":\"swallows error\"}],\
        \"confirmed_count\":1,\"false_positive_count\":0}\n\
        ```";
    let v = extract_json(raw).expect("#820: must recover the JSON object from fenced prose");
    assert_eq!(v["validator"], "agent-1");
    assert_eq!(v["validated"][0]["verdict"], "confirmed");
    assert_eq!(v["validated"][0]["finding_id"], 1);
}

#[test]
fn extract_json_recovers_plain_json_validator_object() {
    // A validator that emits a bare JSON object (no fence) must also parse.
    let raw = "{\"validator\":\"agent-2\",\"cycle\":1,\"validated\":[{\"finding_id\":1,\
        \"verdict\":\"confirmed\",\"new_severity\":\"medium\"}]}";
    let v = extract_json(raw).expect("#820: must recover a bare JSON object");
    assert_eq!(v["validated"][0]["new_severity"], "medium");
}

#[test]
fn extract_json_recovers_object_with_leading_log_preamble() {
    // Mixed output: machine log lines, THEN a JSON object on its own line.
    let raw = "2026-06-25T23:39:00Z INFO starting validation\n\
        2026-06-25T23:39:01Z INFO read 3 findings\n\
        {\"validator\":\"agent-3\",\"validated\":[{\"finding_id\":2,\"verdict\":\"false_positive\"}]}";
    let v = extract_json(raw).expect("#820: must recover object after log preamble");
    assert_eq!(v["validated"][0]["verdict"], "false_positive");
}

#[test]
fn extract_json_returns_none_for_log_only_output() {
    // A validator that emitted only logs / a stray brace yields no object —
    // the recipe treats this as zero votes and warns (it must NOT crash jq).
    let raw = "ERROR: agent timed out before producing structured output\n\
        [trace] partial log line with a stray { brace but no closing object\n\
        done.";
    assert!(
        extract_json(raw).is_none(),
        "#820: log-only output must yield no JSON object so the recipe can \
         degrade to zero votes with a diagnostic."
    );
}

// =========================================================================
// Layer 3: End-to-end (graceful skip) — run the REAL merge-validations bash
// command against mixed validator output and assert it no longer emits
// "Bad JSON" and produces valid merged JSON. Self-contained per #833 (needs
// only `jq`, not the amplihack binary). Skips when `jq` is unavailable so the
// suite stays green in minimal environments.
// =========================================================================

fn tool_on_path(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn merge_validations_end_to_end_handles_mixed_output() {
    if !tool_on_path("jq") {
        eprintln!("skip: `jq` not available on PATH");
        return;
    }

    // v1: prose + ```json fence (confirmed). v2: bare JSON (confirmed).
    // v3: prose-only garbage (no JSON object → diagnostic, zero votes).
    let v1 = "Let me validate.\n```json\n{\"validated\":[{\"finding_id\":1,\
        \"verdict\":\"confirmed\",\"new_severity\":\"high\",\"reasoning\":\"swallows error\"}]}\n```";
    let v2 = "{\"validated\":[{\"finding_id\":1,\"verdict\":\"confirmed\",\
        \"new_severity\":\"medium\",\"reasoning\":\"agrees\"}]}";
    let v3 = "ERROR: agent crashed before producing structured output\n";

    let tmp = std::env::temp_dir().join(format!("qac820-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("mk tmp dir");

    let cmd = merge_validations_command()
        .replace("{{validation_agent_1}}", v1)
        .replace("{{validation_agent_2}}", v2)
        .replace("{{validation_agent_3}}", v3)
        .replace("{{validation_threshold}}", "2")
        .replace("{{cycle_number}}", "1")
        .replace("{{output_dir}}", tmp.to_str().unwrap());

    let out = Command::new("bash")
        .arg("-c")
        .arg(&cmd)
        .output()
        .expect("run merge-validations bash");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "#833: merge-validations must succeed on mixed validator output.\n\
         exit: {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        out.status.code()
    );
    assert!(
        !stderr.contains("Bad JSON"),
        "#833: merge-validations must NOT emit a `jq: Bad JSON` error on mixed \
         output.\nstderr:\n{stderr}"
    );
    // The prose-only validator must trigger the targeted diagnostic.
    assert!(
        stderr.contains("unparseable"),
        "#833: the prose-only validator must trigger the `unparseable` \
         diagnostic.\nstderr:\n{stderr}"
    );

    let merged: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("#833: merged output must be valid JSON: {e}\nstdout:\n{stdout}")
    });
    // Two validators confirmed finding 1 (>= threshold 2) → confirmed.
    assert_eq!(
        merged["confirmed_count"], 1,
        "#833: finding 1 must be confirmed by the two well-formed validators.\n{merged:#}"
    );
    assert_eq!(merged["validated"][0]["verdict"], "confirmed");

    let _ = std::fs::remove_dir_all(&tmp);
}
