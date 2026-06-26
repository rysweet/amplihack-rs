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
//! Fix: normalize each validator's output through the tolerant
//! `amplihack orch helper extract-json` helper (the same normalizer
//! smart-orchestrator uses) BEFORE `jq --slurpfile`. If a validator produced
//! non-empty output with no parseable JSON object, emit a targeted diagnostic
//! naming the validator + preserving its raw output as an artifact, then fall
//! back to `{}` so the merge degrades to zero votes instead of crashing.
//!
//! These tests cover three layers:
//!   1. Structural — the recipe wires `extract-json` + diagnostic in correctly.
//!   2. Behavioral (pure Rust) — `extract_json` recovers the validator JSON
//!      object from the exact mixed-format inputs that used to crash `jq`.
//!   3. End-to-end (graceful skip) — the real `merge-validations` bash command
//!      no longer emits `Bad JSON` on mixed output and produces valid merged
//!      JSON. Skipped when the `amplihack`/`jq` tooling is unavailable.

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
// Layer 1: Structural — the recipe must route validator output through the
// tolerant extract-json normalizer before jq, with a diagnostic fallback.
// =========================================================================

#[test]
fn merge_validations_normalizes_via_extract_json() {
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("orch helper extract-json"),
        "#820: merge-validations must normalize each validator's raw output \
         through `amplihack orch helper extract-json` before jq.\nCommand:\n{cmd}"
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
        .find("orch helper extract-json")
        .expect("must contain extract-json");
    let jq_pos = cmd
        .find("jq -n")
        .or_else(|| cmd.find("jq "))
        .expect("must contain jq invocation");
    assert!(
        extract_pos < jq_pos,
        "#820: validator output must be normalized BEFORE jq runs.\n\
         extract-json at {extract_pos}, jq at {jq_pos}."
    );
}

#[test]
fn merge_validations_emits_diagnostic_and_preserves_artifact() {
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("WARNING") && cmd.contains("no parseable JSON object"),
        "#820: when a validator produced non-empty output with no parseable \
         JSON, merge-validations must emit a targeted diagnostic instead of a \
         brittle jq crash.\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("Raw output preserved at:"),
        "#820: the diagnostic must preserve the offending validator's raw \
         output as an artifact and report its path.\nCommand:\n{cmd}"
    );
    // The diagnostic must name which validator failed.
    assert!(
        cmd.contains("${_label}") || cmd.contains("$_label"),
        "#820: the diagnostic must name the offending validator (via its \
         label) so the failing prompt/output can be repaired.\nCommand:\n{cmd}"
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
// Layer 2: Behavioral (pure Rust) — the normalizer the recipe now invokes
// must recover the validator JSON object from exactly the mixed-format
// inputs that used to crash jq with "Bad JSON".
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
// "Bad JSON" and produces valid merged JSON. Skips when amplihack/jq are
// unavailable so the suite stays green in minimal environments.
// =========================================================================

fn amplihack_binary() -> Option<PathBuf> {
    // Primary: the `amplihack` binary sits next to the test executable's parent
    // (`<target>/<profile>/amplihack`), regardless of CARGO_TARGET_DIR. The test
    // exe lives at `<target>/<profile>/deps/<name>`.
    if let Ok(exe) = std::env::current_exe() {
        // exe = <target>/<profile>/deps/<test-bin>
        if let Some(profile_dir) = exe.parent().and_then(|deps| deps.parent()) {
            let candidate = profile_dir.join("amplihack");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // Fallbacks: explicit CARGO_TARGET_DIR, then the default workspace target/.
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(repo_root().join("target"));
    for root in roots {
        for profile in ["debug", "release"] {
            let p = root.join(profile).join("amplihack");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn tool_on_path(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn merge_validations_end_to_end_handles_mixed_output() {
    let Some(amplihack) = amplihack_binary() else {
        eprintln!(
            "skip: target/{{debug,release}}/amplihack not built; run `cargo build` \
             to exercise the merge-validations end-to-end path"
        );
        return;
    };
    if !tool_on_path("jq") {
        eprintln!("skip: `jq` not available on PATH");
        return;
    }

    // v1: prose + ```json fence (confirmed). v2: bare JSON (confirmed).
    // v3: log-only garbage (no JSON object → diagnostic, zero votes).
    let v1 = "Let me validate.\n```json\n{\"validated\":[{\"finding_id\":1,\
        \"verdict\":\"confirmed\",\"new_severity\":\"high\",\"reasoning\":\"swallows error\"}]}\n```";
    let v2 = "{\"validated\":[{\"finding_id\":1,\"verdict\":\"confirmed\",\
        \"new_severity\":\"medium\",\"reasoning\":\"agrees\"}]}";
    let v3 = "ERROR: agent crashed\n[trace] stray { brace, no object\n";

    let tmp = std::env::temp_dir().join(format!("qac820-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk tmp dir");

    let cmd = merge_validations_command()
        .replace("{{validation_agent_1}}", v1)
        .replace("{{validation_agent_2}}", v2)
        .replace("{{validation_agent_3}}", v3)
        .replace("{{validation_threshold}}", "2")
        .replace("{{cycle_number}}", "1")
        .replace("{{output_dir}}", tmp.to_str().unwrap());

    // Prepend the built amplihack dir so `amplihack orch helper extract-json`
    // resolves to the binary under test.
    let bin_dir = amplihack.parent().unwrap().to_path_buf();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new("bash")
        .arg("-c")
        .arg(&cmd)
        .env("PATH", path)
        .output()
        .expect("run merge-validations bash");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "#820: merge-validations must succeed on mixed validator output.\n\
         exit: {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        out.status.code()
    );
    assert!(
        !stderr.contains("Bad JSON"),
        "#820: merge-validations must NOT emit a `jq: Bad JSON` error on mixed \
         output.\nstderr:\n{stderr}"
    );
    // The garbage validator must trigger the targeted diagnostic.
    assert!(
        stderr.contains("no parseable JSON object"),
        "#820: the log-only validator must trigger the diagnostic.\nstderr:\n{stderr}"
    );

    let merged: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("#820: merged output must be valid JSON: {e}\nstdout:\n{stdout}")
    });
    // Two validators confirmed finding 1 (>= threshold 2) → confirmed.
    assert_eq!(
        merged["confirmed_count"], 1,
        "#820: finding 1 must be confirmed by the two well-formed validators.\n{merged:#}"
    );
    assert_eq!(merged["validated"][0]["verdict"], "confirmed");

    let _ = std::fs::remove_dir_all(&tmp);
}
