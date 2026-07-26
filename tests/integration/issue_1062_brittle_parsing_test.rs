//! Issue #1062: fix the brittle-parsing findings from the default-workflow
//! recipe brittleness audit.
//!
//! These tests are written **first** (TDD, step 7) and MUST FAIL against the
//! current, un-migrated code. They pin the target contract for each finding:
//!
//!   A4 (Rust helper): `amplihack orch helper normalise-verdict` centralises
//!       LLM verdict-synonym handling (mirrors the existing `normalise-type`),
//!       using EQUALITY not containment (so `NOT_VERIFIED`/`NOT_ACHIEVED` never
//!       collapse to `WORK_VERIFIED`).
//!   A1 (workflow-tdd step-08c-enforce-verdict): the free-text verdict scrape
//!       (`grep -E '^{.*"verdict"'` + `awk` last-line + `jq` fallback + bash
//!       `case` synonym block) is replaced by
//!       `orch helper extract-json | extract-field --field verdict | normalise-verdict`.
//!       Fail-safe branches (#615/#624/#425) are PRESERVED.
//!   A2 (workflow-pr-review step-17a): the hand-rolled JSON-form regex is
//!       replaced by the helper; the narrow prose `VERDICT: FAILED` fail-safe
//!       token stays (issue #962), and the three-outcome contract holds.
//!   A3 (workflow-design step-06b-checkpoint-doc-review): the English-keyword
//!       NLU grep (`fail|error|cannot|...`) is removed; STATUS derives from an
//!       agent-emitted structured field. Non-fatal contract (#834) preserved.
//!   B  (workflow-terminal-state): no-merge intent stops being guessed from
//!       user prose via `detect_no_merge_directive` regex; it reads a
//!       structured classifier-emitted field, while keeping the explicit
//!       `NO_MERGE` flag plumbing.
//!   C  (smart-reflect-loop): goal-status loop control stops using substring
//!       `'PARTIAL' in reflection_1`; the reviewer emits a structured
//!       `goal_status` (parse_json) tested with equality.
//!   D1 (smart-execute-routing derive-recursion-guard): `grep -qE '"status" *: *"ok"'`
//!       over `session_info` JSON is replaced by
//!       `orch helper extract-field --field status`.
//!   D2 (session-tree register + smart-classify-route setup-session): `register`
//!       gains an additive `--json` mode; the default `TREE_ID=.. DEPTH=..`
//!       line stays byte-exact; the consumer reads it via `extract-field`
//!       instead of `grep -oE 'TREE_ID=...'`.
//!
//! Findings explicitly OUT OF SCOPE (must remain untouched): jq over `gh --json`
//! output, SHA/charset/ref-format validation regex, gh/git stderr
//! error-classification greps, token-redaction sed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Absolute path to the freshly-built `amplihack` binary under test.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}

/// A `PATH` value that includes the built binary's directory, so recipe bash
/// bodies that call the bare command `amplihack ...` resolve to THIS build.
fn path_with_bin() -> String {
    let dir = Path::new(bin())
        .parent()
        .expect("binary has a parent dir")
        .to_string_lossy()
        .to_string();
    match std::env::var("PATH") {
        Ok(p) => format!("{dir}:{p}"),
        Err(_) => dir,
    }
}

/// `amplifier-bundle/recipes/<name>.yaml` relative to the test crate.
fn recipe_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn read_recipe(name: &str) -> String {
    let p = recipe_path(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Return the raw `command:` block string of a step by id, from `<recipe>.yaml`.
fn step_command(recipe: &str, step_id: &str) -> String {
    step_field(recipe, step_id, "command")
        .unwrap_or_else(|| panic!("{recipe}: step `{step_id}` has no `command:` block"))
}

/// Return the raw string value of a scalar/block field of a step by id.
fn step_field(recipe: &str, step_id: &str, field: &str) -> Option<String> {
    let text = read_recipe(recipe);
    let value: serde_yaml::Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {recipe}.yaml: {e}"));
    let steps = value
        .get("steps")
        .and_then(|s| s.as_sequence())
        .unwrap_or_else(|| panic!("{recipe}.yaml: top-level `steps:` missing"));
    for step in steps {
        let id = step.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if id == step_id {
            return step.get(field).and_then(|v| v.as_str()).map(str::to_string);
        }
    }
    panic!("{recipe}.yaml: step `{step_id}` not found");
}

/// Is `parse_json: true` set on the step?
fn step_parse_json(recipe: &str, step_id: &str) -> bool {
    let text = read_recipe(recipe);
    let value: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let steps = value.get("steps").and_then(|s| s.as_sequence()).unwrap();
    for step in steps {
        let id = step.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if id == step_id {
            return step
                .get("parse_json")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        }
    }
    panic!("{recipe}.yaml: step `{step_id}` not found");
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run `amplihack <args>` feeding `stdin`, with an optional extra env map and
/// the built binary already on PATH.
fn run_cli(args: &[&str], stdin: &str, extra_env: &[(&str, &str)]) -> Run {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .env("PATH", path_with_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn amplihack");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait amplihack");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// Execute a recipe bash body with `amplihack` on PATH and the supplied env.
fn run_bash_body(body: &str, envs: &[(&str, &str)]) -> Run {
    let mut script = tempfile::NamedTempFile::new().expect("tempfile");
    script.write_all(body.as_bytes()).expect("write body");
    let path = script.path().to_path_buf();

    let mut cmd = Command::new("bash");
    cmd.arg(&path)
        .env_clear()
        .env("PATH", path_with_bin())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn bash");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn normalise_verdict(input: &str) -> Run {
    run_cli(&["orch", "helper", "normalise-verdict"], input, &[])
}

// ===========================================================================
// A4 — `amplihack orch helper normalise-verdict`
// ===========================================================================

#[test]
fn a4_normalise_verdict_subcommand_is_wired() {
    let run = run_cli(&["orch", "helper", "normalise-verdict", "--help"], "", &[]);
    assert_eq!(
        run.code, 0,
        "`orch helper normalise-verdict --help` must exit 0 (subcommand wired into clap); stderr={}",
        run.stderr
    );
}

#[test]
fn a4_positive_synonyms_map_to_work_verified() {
    for raw in ["VERIFIED", "SUCCESS", "APPROVED", "PASS", "PASSED"] {
        let run = normalise_verdict(raw);
        assert_eq!(run.code, 0, "normalise-verdict({raw:?}) must exit 0");
        assert_eq!(
            run.stdout.trim(),
            "WORK_VERIFIED",
            "positive synonym {raw:?} must normalise to WORK_VERIFIED (mirror the tdd `case` block)"
        );
    }
}

#[test]
fn a4_failure_synonyms_map_to_hollow_success() {
    for raw in ["FAILED", "NO_WORK", "EMPTY", "NO_ARTIFACTS"] {
        let run = normalise_verdict(raw);
        assert_eq!(
            run.stdout.trim(),
            "HOLLOW_SUCCESS",
            "failure synonym {raw:?} must normalise to HOLLOW_SUCCESS"
        );
    }
}

#[test]
fn a4_inconclusive_synonyms_map_to_insufficient_evidence() {
    for raw in ["INCONCLUSIVE", "UNKNOWN", "UNCLEAR", "PARTIAL"] {
        let run = normalise_verdict(raw);
        assert_eq!(
            run.stdout.trim(),
            "INSUFFICIENT_EVIDENCE",
            "inconclusive synonym {raw:?} must normalise to INSUFFICIENT_EVIDENCE"
        );
    }
}

#[test]
fn a4_canonical_verdicts_pass_through_unchanged() {
    for raw in ["WORK_VERIFIED", "HOLLOW_SUCCESS", "INSUFFICIENT_EVIDENCE"] {
        let run = normalise_verdict(raw);
        assert_eq!(
            run.stdout.trim(),
            raw,
            "canonical verdict {raw:?} must pass through unchanged"
        );
    }
}

#[test]
fn a4_unknown_verdict_fail_safes_to_insufficient_evidence() {
    // Issue #624 philosophy: an LLM producing a novel verdict string must never
    // hard-fail a recipe that already has real artifacts.
    for raw in ["WHATEVER", "totally-made-up", ""] {
        let run = normalise_verdict(raw);
        assert_eq!(
            run.stdout.trim(),
            "INSUFFICIENT_EVIDENCE",
            "unknown verdict {raw:?} must fail-safe to INSUFFICIENT_EVIDENCE"
        );
    }
}

#[test]
fn a4_is_case_insensitive_and_trims_whitespace() {
    for (raw, expected) in [
        ("verified", "WORK_VERIFIED"),
        ("  Pass \n", "WORK_VERIFIED"),
        ("failed", "HOLLOW_SUCCESS"),
        ("Partial", "INSUFFICIENT_EVIDENCE"),
    ] {
        let run = normalise_verdict(raw);
        assert_eq!(
            run.stdout.trim(),
            expected,
            "normalise-verdict must be case-insensitive and whitespace-trimming: {raw:?}"
        );
    }
}

#[test]
fn a4_uses_equality_not_containment_regression() {
    // SECURITY / correctness (design R2): substring matching would collapse
    // `NOT_VERIFIED` and `NOT_ACHIEVED` into WORK_VERIFIED because they *contain*
    // "VERIFIED"/"ACHIEVED". Equality-based mapping must NOT do that.
    for raw in ["NOT_VERIFIED", "NOT_ACHIEVED", "UNVERIFIED"] {
        let run = normalise_verdict(raw);
        assert_ne!(
            run.stdout.trim(),
            "WORK_VERIFIED",
            "{raw:?} must NEVER normalise to WORK_VERIFIED (equality, not containment)"
        );
    }
}

// ===========================================================================
// A1 — workflow-tdd step-08c-enforce-verdict
// ===========================================================================

const TDD_GATE: &str = "step-08c-enforce-verdict";

#[test]
fn a1_tdd_gate_uses_orch_helper_extractor() {
    let cmd = step_command("workflow-tdd", TDD_GATE);
    assert!(
        cmd.contains("orch helper extract-json"),
        "{TDD_GATE} must extract the verdict via `orch helper extract-json`"
    );
    assert!(
        cmd.contains("extract-field") && cmd.contains("verdict"),
        "{TDD_GATE} must read the verdict with `extract-field --field verdict`"
    );
    assert!(
        cmd.contains("normalise-verdict"),
        "{TDD_GATE} must centralise synonym handling via `normalise-verdict`"
    );
}

#[test]
fn a1_tdd_gate_drops_brittle_line_scrape() {
    let cmd = step_command("workflow-tdd", TDD_GATE);
    // The line-based grep for a JSON object and the awk "last line" scrape are
    // exactly what multiline verifier JSON defeats — they must be gone.
    assert!(
        !cmd.contains(r#"grep -E '^[[:space:]]*\{.*"verdict"'"#),
        "{TDD_GATE} must not line-grep for a JSON object (multiline JSON defeats it)"
    );
    assert!(
        !cmd.contains("awk 'NF {line=$0} END {print line}'"),
        "{TDD_GATE} must not use the awk last-JSON-line scrape"
    );
    // The scattered bash synonym `case` is replaced by `normalise-verdict`.
    assert!(
        !cmd.contains("VERIFIED|SUCCESS|APPROVED|PASS"),
        "{TDD_GATE} must not carry an inline bash synonym `case` block (moved to normalise-verdict)"
    );
}

#[test]
fn a1_tdd_gate_preserves_failsafe_branches() {
    let cmd = step_command("workflow-tdd", TDD_GATE);
    // Do NOT tear down existing fail-safe/opt-out branches or issue-referenced
    // defensive comments — only replace the brittle parsing mechanism.
    for needle in [
        "set -euo pipefail",
        "INSUFFICIENT_EVIDENCE",
        "HOLLOW_SUCCESS",
        "ALLOW_NO_OP",
        "orchestration", // #425 sentinel opt-out
        "issue #615",
    ] {
        assert!(
            cmd.contains(needle),
            "{TDD_GATE} must preserve fail-safe branch/comment containing {needle:?}"
        );
    }
    assert!(
        cmd.contains("exit 1"),
        "{TDD_GATE} must keep the fatal HOLLOW_SUCCESS `exit 1` (issue #962 fatal allowlist)"
    );
}

/// Run the migrated tdd gate body with the given inputs.
fn run_tdd_gate(verdict_json: &str, implementation: &str, allow_no_op: &str) -> Run {
    let body = step_command("workflow-tdd", TDD_GATE);
    run_bash_body(
        &body,
        &[
            ("VERDICT_JSON", verdict_json),
            ("IMPLEMENTATION", implementation),
            ("ALLOW_NO_OP", allow_no_op),
        ],
    )
}

#[test]
fn a1_tdd_gate_work_verified_synonym_exits_zero() {
    // `VERIFIED` is a synonym the old bash `case` handled; normalise-verdict
    // must handle it after migration.
    let run = run_tdd_gate(r#"{"verdict": "VERIFIED"}"#, "src/foo.rs changed", "false");
    assert_eq!(
        run.code, 0,
        "WORK_VERIFIED synonym must exit 0; stderr={}",
        run.stderr
    );
}

#[test]
fn a1_tdd_gate_parses_multiline_json_the_old_scrape_missed() {
    // The whole point of finding A1: pretty-printed multiline JSON defeated the
    // line-based grep. extract-json handles it.
    let verdict = "Here is my verdict:\n{\n  \"verdict\": \"HOLLOW_SUCCESS\",\n  \"rationale\": \"no artifacts\"\n}\n";
    let run = run_tdd_gate(verdict, "anything", "false");
    assert_eq!(
        run.code, 1,
        "multiline HOLLOW_SUCCESS JSON must be parsed and stay fatal (exit 1); stderr={}",
        run.stderr
    );
}

#[test]
fn a1_tdd_gate_failed_synonym_is_fatal() {
    let run = run_tdd_gate(r#"{"verdict": "FAILED"}"#, "anything", "false");
    assert_eq!(
        run.code, 1,
        "FAILED synonym -> HOLLOW_SUCCESS must be fatal (exit 1); stderr={}",
        run.stderr
    );
}

#[test]
fn a1_tdd_gate_unknown_verdict_fail_safes_to_zero() {
    let run = run_tdd_gate(r#"{"verdict": "WHATEVER"}"#, "anything", "false");
    assert_eq!(
        run.code, 0,
        "unknown verdict must fail-safe to exit 0 (issue #624); stderr={}",
        run.stderr
    );
}

#[test]
fn a1_tdd_gate_empty_input_fail_safes_to_zero_with_warning() {
    let run = run_tdd_gate("", "anything", "false");
    assert_eq!(run.code, 0, "empty verdict must fail-safe to exit 0");
    assert!(
        run.stderr.contains("WARN"),
        "empty verdict must warn loudly on stderr; stderr={}",
        run.stderr
    );
}

#[test]
fn a1_tdd_gate_allow_no_op_short_circuits() {
    // #425 fast-path must still short-circuit BEFORE verdict parsing.
    let run = run_tdd_gate(r#"{"verdict": "HOLLOW_SUCCESS"}"#, "no diff", "true");
    assert_eq!(
        run.code, 0,
        "ALLOW_NO_OP=true must short-circuit to exit 0 before parsing; stderr={}",
        run.stderr
    );
}

// ===========================================================================
// A2 — workflow-pr-review step-17a-testing-evidence-gate
// ===========================================================================

const PR_GATE: &str = "step-17a-testing-evidence-gate";

#[test]
fn a2_pr_gate_replaces_json_regex_with_helper() {
    let cmd = step_command("workflow-pr-review", PR_GATE);
    // The hand-rolled JSON-form regex half must be gone...
    assert!(
        !cmd.contains(r#""verdict"[[:space:]]*:[[:space:]]*"[^"]*(FAILED|NOT_VERIFIED)"#),
        "{PR_GATE} must not hand-roll a JSON `verdict` regex; route JSON through the helper"
    );
    // ...replaced by the tested extractor.
    assert!(
        cmd.contains("orch helper extract-json") && cmd.contains("extract-field"),
        "{PR_GATE} must extract an embedded JSON verdict via `orch helper extract-json | extract-field`"
    );
}

#[test]
fn a2_pr_gate_retains_prose_failure_failsafe() {
    let cmd = step_command("workflow-pr-review", PR_GATE);
    // The narrow prose token is a documented fail-safe (issue #962), NOT the
    // brittle mechanism — it must be retained.
    assert!(
        cmd.contains("VERDICT:") && cmd.to_uppercase().contains("FAILED"),
        "{PR_GATE} must keep the prose `VERDICT: FAILED` fail-safe token (issue #962)"
    );
    assert!(
        cmd.contains("issue #962") || cmd.contains("#962"),
        "{PR_GATE} must keep its issue #962 defensive rationale"
    );
}

fn run_pr_gate(gate_value: &str) -> Run {
    let body = step_command("workflow-pr-review", PR_GATE);
    run_bash_body(&body, &[("LOCAL_TESTING_GATE", gate_value)])
}

#[test]
fn a2_pr_gate_prose_failure_stays_fatal() {
    // Existing #962 three-outcome contract (FAIL-VISIBLE).
    let gate = "Step 13: Local Testing Results\nExecuted: cargo test. VERDICT: FAILED — 3 of 19 tests failing.";
    let run = run_pr_gate(gate);
    assert_ne!(
        run.code, 0,
        "explicit prose failure verdict must stay fatal"
    );
}

#[test]
fn a2_pr_gate_embedded_json_failure_is_fatal() {
    // NEW: a machine-readable JSON verdict embedded in the evidence is now read
    // via the helper (not a regex) and a FAILED verdict stays fatal.
    let gate = "Step 13: Local Testing Results\nverifier said: {\"verdict\": \"FAILED\", \"rationale\": \"3 failing\"}";
    let run = run_pr_gate(gate);
    assert_ne!(
        run.code, 0,
        "embedded JSON FAILED verdict must be fatal via the helper; stderr={}",
        run.stderr
    );
}

#[test]
fn a2_pr_gate_empty_degrades_visibly() {
    // #962: empty gate must NOT abort/discard work — degrade with WARNING+exit0.
    let run = run_pr_gate("");
    assert_eq!(
        run.code, 0,
        "empty gate must degrade, not abort (issue #962)"
    );
    assert!(
        run.stdout.to_uppercase().contains("WARNING") || run.stderr.contains("WARNING"),
        "empty gate must degrade VISIBLY with a WARNING"
    );
}

#[test]
fn a2_pr_gate_benign_failword_is_not_fatal() {
    // #962: "0 tests failed, 19 passed" must not be misread as a failure verdict.
    let gate =
        "Step 13: Local Testing Results\nExecuted: cargo test. Summary: 0 tests failed, 19 passed.";
    let run = run_pr_gate(gate);
    assert_eq!(
        run.code, 0,
        "benign 'N tests failed, M passed' must not be misread as fatal; stderr={}",
        run.stderr
    );
}

// ===========================================================================
// A3 — workflow-design step-06b-checkpoint-doc-review
// ===========================================================================

const DOC_CHECKPOINT: &str = "step-06b-checkpoint-doc-review";
const DOC_REVIEW: &str = "step-06b-documentation-review";

#[test]
fn a3_doc_review_emits_structured_status() {
    // The doc-review agent must emit a machine-readable status field rather than
    // relying on downstream English-keyword NLU.
    assert!(
        step_parse_json("workflow-design", DOC_REVIEW),
        "{DOC_REVIEW} must set `parse_json: true` so its status/verdict is structured"
    );
    let prompt = step_field("workflow-design", DOC_REVIEW, "prompt")
        .unwrap_or_else(|| panic!("{DOC_REVIEW} must declare a prompt"));
    assert!(
        prompt.contains("status"),
        "{DOC_REVIEW} prompt must instruct the agent to emit a structured `status` field"
    );
}

#[test]
fn a3_doc_checkpoint_drops_keyword_nlu_grep() {
    let cmd = step_command("workflow-design", DOC_CHECKPOINT);
    // Pure English-keyword NLU in bash is the brittle mechanism to remove.
    assert!(
        !cmd.contains("fail|error|cannot|could not|unable|missing|incomplete|does not|blocker"),
        "{DOC_CHECKPOINT} must not derive STATUS by keyword-matching free-text feedback"
    );
    // STATUS must instead come from the structured agent field.
    assert!(
        cmd.contains("extract-field")
            || cmd.contains("DOC_REVIEW_STATUS")
            || cmd.contains("doc_review_status"),
        "{DOC_CHECKPOINT} must derive STATUS from the agent-emitted structured field"
    );
}

#[test]
fn a3_doc_checkpoint_stays_non_fatal_and_safe() {
    // Preserve the issue #834 contract: non-fatal, WARNING+NEEDS_ATTENTION,
    // untrusted feedback consumed as data (printf '%s'), no exit 1.
    let cmd = step_command("workflow-design", DOC_CHECKPOINT);
    assert!(
        !cmd.contains("exit 1"),
        "{DOC_CHECKPOINT} must remain non-fatal (no exit 1)"
    );
    assert!(
        cmd.contains("NEEDS_ATTENTION"),
        "{DOC_CHECKPOINT} must keep the NEEDS_ATTENTION marker"
    );
    assert!(
        cmd.contains("WARNING"),
        "{DOC_CHECKPOINT} must keep a WARNING on stderr"
    );
    assert!(
        cmd.contains("printf '%s'"),
        "{DOC_CHECKPOINT} must keep consuming untrusted feedback safely via printf '%s' (issue #834 S2)"
    );
}

// ===========================================================================
// B — workflow-terminal-state no-merge intent
// ===========================================================================

#[test]
fn b_terminal_state_drops_prose_intent_regex() {
    let text = read_recipe("workflow-terminal-state");
    // The prose-guessing regex over the user's task description is the failure
    // mode (unusual phrasings silently auto-merge). It must be gone.
    assert!(
        !text.contains("detect_no_merge_directive"),
        "workflow-terminal-state must not guess no-merge intent from user prose via detect_no_merge_directive"
    );
    assert!(
        !text.contains("leave[[:space:]][^.]*open"),
        "workflow-terminal-state must not carry the brittle prose no-merge regex"
    );
}

#[test]
fn b_terminal_state_keeps_explicit_flag_plumbing() {
    let text = read_recipe("workflow-terminal-state");
    // The structured flag path is retained; only the prose-regex SOURCE is
    // removed. The decision still reads the explicit no_merge flag.
    assert!(
        text.contains("NO_MERGE") || text.contains("no_merge"),
        "workflow-terminal-state must keep the explicit structured no_merge flag plumbing"
    );
}

#[test]
fn b_classifier_emits_structured_no_merge() {
    // The classifier already reads task_description; it must emit a structured
    // no_merge/intent field the engine consumes, instead of a bash regex.
    let text = read_recipe("smart-classify-route");
    assert!(
        text.contains("no_merge"),
        "smart-classify-route classifier must emit a structured `no_merge` intent field"
    );
}

// ===========================================================================
// C — smart-reflect-loop goal-status control
// ===========================================================================

#[test]
fn c_reflect_loop_replaces_substring_conditions_with_equality() {
    let text = read_recipe("smart-reflect-loop");
    // Substring matching free-text prose can false-trigger on a stray "partial"
    // in a reviewer narrative.
    for brittle in [
        "'PARTIAL' in reflection_1",
        "'NOT_ACHIEVED' in reflection_1",
        "'HOLLOW' in reflection_1",
        "'PARTIAL' in reflection_2",
        "'NOT_ACHIEVED' in reflection_2",
    ] {
        assert!(
            !text.contains(brittle),
            "smart-reflect-loop must not gate the loop on substring match `{brittle}`"
        );
    }
    assert!(
        text.contains("goal_status"),
        "smart-reflect-loop conditions must test a structured `goal_status` field"
    );
    assert!(
        text.contains("goal_status ==") || text.contains("goal_status=="),
        "smart-reflect-loop must use equality (==) against goal_status, not containment"
    );
}

#[test]
fn c_reflect_steps_emit_structured_goal_status() {
    // The reviewer/reflection steps must emit parse_json so goal_status is a
    // real field, not scraped from prose.
    for step in ["reflect-round-1", "reflect-round-2"] {
        // reflect-round-1 may be named differently; tolerate absence by checking
        // any step whose output feeds a loop condition. We assert at least the
        // canonical reviewer step carries parse_json.
        if step_field("smart-reflect-loop", step, "output").is_some() {
            assert!(
                step_parse_json("smart-reflect-loop", step),
                "{step} must set parse_json:true so goal_status is structured"
            );
        }
    }
}

// ===========================================================================
// D1 — smart-execute-routing derive-recursion-guard
// ===========================================================================

const GUARD_STEP: &str = "derive-recursion-guard";

#[test]
fn d1_guard_uses_extract_field_over_json() {
    let cmd = step_command("smart-execute-routing", GUARD_STEP);
    assert!(
        !cmd.contains(r#"grep -qE '"status" *: *"ok"'"#),
        "{GUARD_STEP} must not regex-scan session_info JSON for status"
    );
    assert!(
        cmd.contains("extract-field") && cmd.contains("status"),
        "{GUARD_STEP} must derive status via `orch helper extract-field --field status`"
    );
}

fn run_guard(session_info: &str) -> Run {
    let body = step_command("smart-execute-routing", GUARD_STEP);
    run_bash_body(&body, &[("SESSION_INFO", session_info)])
}

#[test]
fn d1_guard_allows_ok_status() {
    let run = run_guard(r#"{"session_id":"a","tree_id":"t","depth":0,"status":"ok"}"#);
    assert!(
        run.stdout.contains("ALLOWED"),
        "status=ok must yield ALLOWED; stdout={} stderr={}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn d1_guard_blocks_registration_failed() {
    let run = run_guard(
        r#"{"session_id":"none","tree_id":"none","depth":0,"status":"registration_failed"}"#,
    );
    assert!(
        run.stdout.contains("BLOCKED"),
        "status=registration_failed must yield BLOCKED; stdout={} stderr={}",
        run.stdout,
        run.stderr
    );
}

// ===========================================================================
// D2 — session-tree register --json + smart-classify-route consumer
// ===========================================================================

#[test]
fn d2_register_default_line_is_byte_exact() {
    // The additive --json flag must NOT disturb the byte-exact default contract
    // consumed by smart-orchestrator.yaml: `TREE_ID=<id> DEPTH=<n>\n`.
    let tmp = tempfile::TempDir::new_in("/tmp").unwrap();
    let run = run_cli(
        &["session-tree", "register", "sess1234"],
        "",
        &[
            ("AMPLIHACK_SESSION_TREE_DIR", tmp.path().to_str().unwrap()),
            ("AMPLIHACK_TREE_ID", "treeabc"),
            ("AMPLIHACK_SESSION_DEPTH", "0"),
        ],
    );
    assert_eq!(run.code, 0, "register must exit 0; stderr={}", run.stderr);
    assert_eq!(
        run.stdout, "TREE_ID=treeabc DEPTH=0\n",
        "default register stdout must remain byte-exact"
    );
}

#[test]
fn d2_register_json_flag_emits_structured_status() {
    let tmp = tempfile::TempDir::new_in("/tmp").unwrap();
    let run = run_cli(
        &["session-tree", "register", "sess5678", "--json"],
        "",
        &[
            ("AMPLIHACK_SESSION_TREE_DIR", tmp.path().to_str().unwrap()),
            ("AMPLIHACK_TREE_ID", "treexyz"),
            ("AMPLIHACK_SESSION_DEPTH", "0"),
        ],
    );
    assert_eq!(
        run.code, 0,
        "register --json must exit 0; stderr={}",
        run.stderr
    );
    let v: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "register --json must emit valid single-line JSON: {e}; got {:?}",
            run.stdout
        )
    });
    // The JSON must carry the same tree_id/depth as the byte-exact text line so
    // the consumer (setup-session) can read them via `extract-field` instead of
    // `grep -oE 'TREE_ID=...'`. setup-session wraps this with session_id/status.
    assert_eq!(
        v.get("tree_id").and_then(|x| x.as_str()),
        Some("treexyz"),
        "register --json must expose tree_id for extract-field"
    );
    assert_eq!(
        v.get("depth").and_then(|x| x.as_u64()),
        Some(0),
        "register --json must expose depth for extract-field"
    );
}

#[test]
fn d2_classify_route_consumer_uses_json_not_text_grep() {
    let cmd = step_command("smart-classify-route", "setup-session");
    assert!(
        !cmd.contains("grep -oE 'TREE_ID="),
        "setup-session must not grep TREE_ID out of `register` text output"
    );
    assert!(
        cmd.contains("register") && cmd.contains("--json"),
        "setup-session must call `session-tree register ... --json`"
    );
    assert!(
        cmd.contains("extract-field"),
        "setup-session must read tree_id/depth via `orch helper extract-field`"
    );
}
