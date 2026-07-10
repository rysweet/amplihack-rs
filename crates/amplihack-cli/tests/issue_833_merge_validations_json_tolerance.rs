//! Tests for issue #833: `quality-audit-cycle` `merge-validations` aborts with
//! `jq: Bad JSON in --slurpfile ... Invalid numeric literal` when a validation
//! agent's output is not strict JSON.
//!
//! Root cause (and why PR #820 / commit 7b6c3951 was insufficient): #820 tried
//! to tolerate mixed validator output by piping each validator's raw text
//! through `amplihack orch helper extract-json` before `jq --slurpfile`. That
//! introduced a **silent binary-PATH dependency**: when the `amplihack` binary
//! is not on `PATH`, every validator degrades to `{}` via `2>/dev/null` and the
//! audit silently counts zero votes (a zero-BS violation), or — when the helper
//! is absent and the fallback path differs — the raw `jq` crash still
//! reproduces.
//!
//! Fix (#833): replace the binary-dependent normalizer with a **self-contained**
//! tiered `jq`/`sed` extractor (`extract_verdict`) embedded directly in the
//! step (jq is already required by the merge). Each validator is classified
//! independently as:
//!   * `PARSED` — a JSON verdict object was recovered (contributes votes).
//!   * `EMPTY` — no / whitespace-only output → `{}`, zero votes, NO warning.
//!   * `UNPARSEABLE` — non-empty output, no JSON object survived → loud WARN naming the validator, raw output preserved as an artifact, `{}`, zero votes, merge CONTINUES.
//!
//! A single malformed validator never aborts the merge. The step fails hard
//! (`exit 1`) with a clear diagnostic ONLY when the parsed count is `0` and at
//! least one validator was `UNPARSEABLE` — never with a raw `jq` error. An
//! all-EMPTY cycle is a clean audit and proceeds.
//!
//! These tests are TDD-style: they specify the post-fix contract and therefore
//! FAIL against the current (PR #820) implementation, then pass once the
//! self-contained extractor lands.
//!
//! Layers:
//!   1. Structural — the recipe wires the self-contained extractor, the
//!      classification + WARN + artifact diagnostics, the FATAL gate, and keeps
//!      every security property; the binary dependency is gone.
//!   2. Behavioral (bash exec, graceful skip) — the REAL `merge-validations`
//!      command, run with substituted fixtures, recovers JSON from prose/fences/
//!      logs/concatenated values, tolerates a single unparseable validator,
//!      fails hard only when nothing parsed, and never emits `jq: Bad JSON`.

use serde_yaml::Value;
use std::path::PathBuf;
use std::process::Command;

// =========================================================================
// Shared helpers (mirror the issue_646 / issue_820 patterns).
// =========================================================================

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
// Layer 1: Structural — self-contained extractor, classification, FATAL gate,
// preserved security properties.
// =========================================================================

#[test]
fn recipe_parses_as_valid_yaml() {
    let _ = load_recipe();
}

#[test]
fn merge_validations_step_still_exists() {
    // The fix must not rename or remove the step (issue #646 inventory relies
    // on it).
    let _ = merge_validations_command();
}

#[test]
fn merge_validations_drops_extract_json_binary_dependency() {
    // #833 D1: the fatal flaw in PR #820 was depending on the `amplihack`
    // binary via `orch helper extract-json` (silent `{}` when off PATH). The
    // normalizer must be self-contained — no binary-PATH dependency.
    let cmd = merge_validations_command();
    assert!(
        !cmd.contains("orch helper extract-json"),
        "#833: merge-validations must NOT depend on `amplihack orch helper \
         extract-json` (silent zero-BS degradation when off PATH). Use the \
         self-contained jq/sed extractor instead.\nCommand:\n{cmd}"
    );
    assert!(
        !cmd.contains("extract-json"),
        "#833: no `extract-json` binary subcommand may remain in \
         merge-validations.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_uses_self_contained_extract_verdict() {
    // The replacement is an in-step shell function that turns raw validator
    // output into a single normalized JSON object using only jq/sed.
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("extract_verdict"),
        "#833: merge-validations must define/use a self-contained \
         `extract_verdict` normalizer (jq/sed only, no external binary).\n\
         Command:\n{cmd}"
    );
}

#[test]
fn merge_validations_normalizes_before_jq() {
    // Validator output must be normalized BEFORE the slurpfile merge runs.
    let cmd = merge_validations_command();
    let norm_pos = cmd
        .find("extract_verdict")
        .expect("#833: must contain the extract_verdict normalizer");
    let jq_pos = cmd
        .find("jq -n")
        .or_else(|| cmd.find("jq "))
        .expect("#833: must contain a jq invocation");
    assert!(
        norm_pos < jq_pos,
        "#833: validator output must be normalized via extract_verdict BEFORE \
         jq runs.\nextract_verdict at {norm_pos}, jq at {jq_pos}."
    );
}

#[test]
fn merge_validations_classifies_parsed_empty_unparseable() {
    // The three-way classification is the core of the contract.
    let cmd = merge_validations_command();
    for token in ["PARSED", "EMPTY", "UNPARSEABLE"] {
        assert!(
            cmd.contains(token),
            "#833: merge-validations must classify each validator as \
             PARSED / EMPTY / UNPARSEABLE; missing `{token}`.\nCommand:\n{cmd}"
        );
    }
}

#[test]
fn merge_validations_warns_per_unparseable_validator() {
    // UNPARSEABLE → targeted stderr WARNING naming the validator, with the raw
    // output preserved as an artifact. EMPTY must NOT warn (separate test
    // behaviorally), so the warning text is keyed on "unparseable".
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("[merge-validations] WARNING: validator"),
        "#833: an UNPARSEABLE validator must emit a targeted \
         `[merge-validations] WARNING: validator <vN> ...` diagnostic.\n\
         Command:\n{cmd}"
    );
    assert!(
        cmd.contains("output unparseable"),
        "#833: the per-validator warning must say the validator's `output \
         unparseable` (distinct from EMPTY).\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("counting zero votes"),
        "#833: the warning must state the validator contributes zero votes.\n\
         Command:\n{cmd}"
    );
    assert!(
        cmd.contains("Raw output preserved at:"),
        "#833: the warning must preserve the raw output as an artifact and \
         report its path.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_preserves_raw_artifact_per_validator() {
    // Artifact path is built from trusted context only: output_dir + cycle +
    // fixed vN label — never from validator content (path-traversal safety).
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("validator_") && cmd.contains("_raw.txt"),
        "#833: unparseable validator output must be preserved to a \
         `validator_vN_raw.txt` artifact.\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("cycle_"),
        "#833: the raw artifact must live under a per-cycle directory \
         (cycle_<n>).\nCommand:\n{cmd}"
    );
    // Path derives from the trusted context vars, not validator content.
    assert!(
        cmd.contains("{{output_dir}}") || cmd.contains("OUTPUT_DIR"),
        "#833: the artifact path must derive from the trusted output_dir \
         context var.\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("{{cycle_number}}") || cmd.contains("CYCLE"),
        "#833: the artifact path must derive from the trusted cycle_number \
         context var.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_has_all_unparseable_fatal_gate() {
    // D4: hard-fail ONLY when nothing parsed and ≥1 unparseable — with a clear
    // diagnostic, never a raw jq error.
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("[merge-validations] FATAL"),
        "#833: there must be a FATAL diagnostic for the all-unparseable gate.\n\
         Command:\n{cmd}"
    );
    assert!(
        cmd.contains("all validators produced unparseable output"),
        "#833: the FATAL diagnostic must explain that all validators produced \
         unparseable output.\nCommand:\n{cmd}"
    );
    assert!(
        cmd.contains("exit 1"),
        "#833: the all-unparseable gate must `exit 1` (hard fail) before the \
         merge proceeds to fix.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_fatal_gate_keyed_on_parsed_count() {
    // The gate is keyed on the PARSED count (== 0), not a literal "all three
    // unparseable" check — so EMPTY + UNPARSEABLE (zero parsed) is also fatal.
    let cmd = merge_validations_command();
    let lower = cmd.to_lowercase();
    assert!(
        lower.contains("parsed_count") || lower.contains("parsed count"),
        "#833: the FATAL gate must track a parsed-validator count.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_preserves_single_quoted_heredocs() {
    // Security: validator content captured via single-quoted heredocs only.
    let cmd = merge_validations_command();
    for line in cmd.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("cat >") && trimmed.contains("<<") {
            assert!(
                trimmed.contains("<<'"),
                "#833: heredoc delimiters must remain single-quoted to prevent \
                 shell expansion of adversarial validator content. Found: {trimmed}"
            );
        }
    }
}

#[test]
fn merge_validations_preserves_long_unique_delimiters() {
    let cmd = merge_validations_command();
    for d in [
        "__AMPLIHACK_SAFE_HEREDOC_V1_TMPWRITE__",
        "__AMPLIHACK_SAFE_HEREDOC_V2_TMPWRITE__",
        "__AMPLIHACK_SAFE_HEREDOC_V3_TMPWRITE__",
    ] {
        assert!(
            cmd.contains(d),
            "#833: long unique heredoc delimiter `{d}` must be preserved \
             (delimiter-collision injection mitigation).\nCommand:\n{cmd}"
        );
    }
}

#[test]
fn merge_validations_preserves_chmod_600_tmpfiles() {
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("chmod 600"),
        "#833: temp files must be created with `chmod 600`.\nCommand:\n{cmd}"
    );
}

#[test]
fn merge_validations_preserves_trap_exit_cleanup() {
    let cmd = merge_validations_command();
    let trap_line = cmd
        .lines()
        .find(|l| l.contains("trap") && l.contains("EXIT"))
        .expect("#833: merge-validations must keep a `trap ... EXIT` cleanup line");
    // The cleanup must still remove the validator temp files (no leaks), even
    // on the FATAL exit path.
    assert!(
        trap_line.contains("rm -f"),
        "#833: the EXIT trap must `rm -f` the temp files.\nTrap: {trap_line}"
    );
}

#[test]
fn merge_validations_keeps_deterministic_slurpfile_merge() {
    // The majority-vote merge logic is unchanged — only the inputs are
    // normalized. It already tolerates `{}` via `?`.
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("--slurpfile v1")
            && cmd.contains("--slurpfile v2")
            && cmd.contains("--slurpfile v3")
            && cmd.contains("group_by"),
        "#833: the deterministic slurpfile/group_by majority-vote merge must be \
         preserved.\nCommand:\n{cmd}"
    );
}

// =========================================================================
// Layer 2: Behavioral (bash exec, graceful skip) — run the REAL command.
// The whole point of #833 is that this is self-contained, so these tests need
// only `bash` + `jq`, NOT the amplihack binary.
// =========================================================================

struct MergeRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn tool_on_path(tool: &str, arg: &str) -> bool {
    Command::new(tool)
        .arg(arg)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tooling_available() -> bool {
    tool_on_path("bash", "--version") && tool_on_path("jq", "--version")
}

/// Substitute the recipe placeholders and run the real merge-validations
/// command under bash. `out_dir` becomes `{{output_dir}}` so artifact paths
/// land somewhere inspectable.
fn run_merge(
    v1: &str,
    v2: &str,
    v3: &str,
    threshold: &str,
    cycle: &str,
    out_dir: &str,
) -> MergeRun {
    let cmd = merge_validations_command()
        .replace("{{validation_agent_1}}", v1)
        .replace("{{validation_agent_2}}", v2)
        .replace("{{validation_agent_3}}", v3)
        .replace("{{validation_threshold}}", threshold)
        .replace("{{cycle_number}}", cycle)
        .replace("{{output_dir}}", out_dir);

    let out = Command::new("bash")
        .arg("-c")
        .arg(&cmd)
        .output()
        .expect("run merge-validations bash");

    MergeRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn unique_out_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qac833-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mk tmp out dir");
    dir
}

const CONFIRM_V: &str = r#"{"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"high","reasoning":"swallows error"}],"confirmed_count":1,"false_positive_count":0}"#;

fn assert_no_bad_json(run: &MergeRun) {
    assert!(
        !run.stderr.contains("Bad JSON"),
        "#833: merge-validations must never surface a raw `jq: Bad JSON` error.\n\
         stderr:\n{}",
        run.stderr
    );
}

fn parse_stdout(run: &MergeRun) -> serde_json::Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "#833: merged stdout must be valid JSON: {e}\nstdout:\n{}\nstderr:\n{}",
            run.stdout, run.stderr
        )
    })
}

#[test]
fn behavioral_strict_json_all_three_confirms() {
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("strict");
    let run = run_merge(CONFIRM_V, CONFIRM_V, "", "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(0),
        "#833: strict JSON validators must succeed.\nstderr:\n{}",
        run.stderr
    );
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 1,
        "#833: two strict-JSON confirmations (≥ threshold 2) → finding 1 \
         confirmed.\n{merged:#}"
    );
    assert!(
        !run.stderr.contains("WARNING") && !run.stderr.contains("FATAL"),
        "#833: clean strict input must not warn or fatal.\nstderr:\n{}",
        run.stderr
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_recovers_fenced_json() {
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("fenced");
    let fenced = format!("Let me validate the findings.\n```json\n{CONFIRM_V}\n```");
    let run = run_merge(&fenced, &fenced, "", "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(run.code, Some(0), "stderr:\n{}", run.stderr);
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 1,
        "#833: JSON inside a ```json fence must be recovered.\n{merged:#}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_recovers_json_after_log_preamble() {
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("preamble");
    let withlog = format!(
        "2026-06-25T23:39:00Z INFO starting validation\n\
         2026-06-25T23:39:01Z INFO read 3 findings\n{CONFIRM_V}"
    );
    let run = run_merge(&withlog, &withlog, "", "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(run.code, Some(0), "stderr:\n{}", run.stderr);
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 1,
        "#833: JSON after a log preamble must be recovered.\n{merged:#}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_concatenated_prefers_verdict_object() {
    // D2: when multiple JSON values are present, prefer the object containing a
    // `validated` key over an unrelated leading log object. If the extractor
    // wrongly picked the leading `{"level":...}` object, the verdict object
    // would be lost and confirmed_count would be 0.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("concat");
    let concat = format!("{{\"level\":\"info\",\"msg\":\"validating\"}}\n{CONFIRM_V}");
    let run = run_merge(&concat, &concat, "", "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(run.code, Some(0), "stderr:\n{}", run.stderr);
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 1,
        "#833 D2: the extractor must prefer the object with a `validated` key \
         over a leading log object.\n{merged:#}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_one_unparseable_validator_continues() {
    // req #2: a single prose-only validator must NOT abort the merge. It is
    // classified UNPARSEABLE → warned, raw output preserved, zero votes; the
    // other two still drive the majority vote.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("oneunparse");
    let prose = "I reviewed the code and everything looks fine to me, no JSON here.";
    let run = run_merge(CONFIRM_V, CONFIRM_V, prose, "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(0),
        "#833: one unparseable validator must NOT abort the merge.\nstderr:\n{}",
        run.stderr
    );
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 1,
        "#833: the two parseable validators must still confirm finding 1.\n{merged:#}"
    );
    // Targeted WARN naming v3 (validation_agent_3 → v3).
    assert!(
        run.stderr.contains("WARNING") && run.stderr.contains("unparseable"),
        "#833: the unparseable validator must trigger a targeted WARNING.\n\
         stderr:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("v3"),
        "#833: the WARNING must name which validator (v3) was unparseable.\n\
         stderr:\n{}",
        run.stderr
    );
    // Raw artifact preserved at the documented path.
    let artifact = out.join("cycle_3").join("validator_v3_raw.txt");
    assert!(
        artifact.exists(),
        "#833: the unparseable validator's raw output must be preserved at \
         {}.\nstderr:\n{}",
        artifact.display(),
        run.stderr
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_all_unparseable_fatal_exit_1() {
    // req #3: hard-fail only when ALL validators are unparseable — with a clear
    // diagnostic, never a raw jq error.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("allunparse");
    let prose1 = "no json here, just prose one";
    let prose2 = "ERROR: agent crashed before emitting structured output";
    let prose3 = "[trace] stray { brace but no closing object ... done";
    let run = run_merge(prose1, prose2, prose3, "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(1),
        "#833: all-unparseable input must fail hard (exit 1).\nstderr:\n{}\nstdout:\n{}",
        run.stderr,
        run.stdout
    );
    assert!(
        run.stderr.contains("FATAL")
            && run
                .stderr
                .contains("all validators produced unparseable output"),
        "#833: the all-unparseable failure must be a clear FATAL diagnostic, \
         not a raw jq crash.\nstderr:\n{}",
        run.stderr
    );
    // All three raw artifacts preserved and listed.
    for vn in ["v1", "v2", "v3"] {
        let artifact = out.join("cycle_3").join(format!("validator_{vn}_raw.txt"));
        assert!(
            artifact.exists(),
            "#833: raw artifact for {vn} must be preserved at {}.",
            artifact.display()
        );
        assert!(
            run.stderr.contains(&format!("validator_{vn}_raw.txt")),
            "#833: the FATAL diagnostic must list the {vn} artifact path.\n\
             stderr:\n{}",
            run.stderr
        );
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_all_empty_is_clean_audit() {
    // An all-EMPTY cycle is NOT fatal: it is a clean audit that proceeds with
    // zero confirmed findings and no warnings.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("allempty");
    let run = run_merge("", "   ", "\n\t ", "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(0),
        "#833: an all-empty cycle must be a clean audit (exit 0), NOT fatal.\n\
         stderr:\n{}",
        run.stderr
    );
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 0,
        "#833: all-empty → zero confirmed findings.\n{merged:#}"
    );
    assert!(
        !run.stderr.contains("WARNING") && !run.stderr.contains("FATAL"),
        "#833: EMPTY validators must NOT warn or fatal.\nstderr:\n{}",
        run.stderr
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_empty_plus_unparseable_is_fatal_listing_only_unparseable() {
    // The gate is keyed on parsed_count == 0 (with ≥1 UNPARSEABLE), so a mix of
    // EMPTY + UNPARSEABLE with zero parsed is fatal. EMPTY validators produced
    // no output, so the diagnostic lists ONLY the unparseable artifact(s).
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("emptyunparse");
    let prose1 = "purely prose, no json object at all";
    let run = run_merge(prose1, "", "   ", "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(1),
        "#833: EMPTY + UNPARSEABLE with zero parsed must be fatal (exit 1).\n\
         stderr:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("FATAL"),
        "#833: zero-parsed mix must emit the FATAL diagnostic.\nstderr:\n{}",
        run.stderr
    );
    // Only v1 (the unparseable one) has a preserved artifact and appears.
    let v1_artifact = out.join("cycle_3").join("validator_v1_raw.txt");
    assert!(
        v1_artifact.exists(),
        "#833: the unparseable validator (v1) raw output must be preserved.\n\
         stderr:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("validator_v1_raw.txt"),
        "#833: the FATAL diagnostic must list the unparseable v1 artifact.\n\
         stderr:\n{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("validator_v2_raw.txt")
            && !run.stderr.contains("validator_v3_raw.txt"),
        "#833: EMPTY validators (v2, v3) produced no output and must be OMITTED \
         from the FATAL artifact list.\nstderr:\n{}",
        run.stderr
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn behavioral_never_emits_bad_json_on_mixed_input() {
    // The defining regression: mixed validator output must never bubble up the
    // raw `jq: Bad JSON in --slurpfile` crash that motivated #833.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("nobadjson");
    let fenced = format!("prose preamble\n```json\n{CONFIRM_V}\n```");
    let prose = "ERROR: not json, just a log line { with a stray brace";
    let run = run_merge(&fenced, CONFIRM_V, prose, "2", "3", out.to_str().unwrap());
    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(0),
        "#833: mixed (fenced + bare + prose) input must merge cleanly.\n\
         stderr:\n{}",
        run.stderr
    );
    let _ = std::fs::remove_dir_all(&out);
}
