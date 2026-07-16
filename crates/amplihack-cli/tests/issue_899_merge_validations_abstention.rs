//! Tests for issue #899: `quality-audit` `merge-validations` must treat an
//! **unparseable validator output as an abstention (zero votes)** and continue
//! applying the configured threshold to the validators that *did* parse —
//! rather than exiting `1` and aborting the whole audit.
//!
//! Context: the #899 reproduction exited `1` against a **stale installed
//! asset**, not current source. The abstention semantics were already made
//! correct by #837/#833 (`extract_verdict` + `classify_validator` +
//! `parsed_count == 0 && unparseable_count >= 1` FATAL gate). There is
//! therefore no residual *code* bug in the merge logic. This deliverable is
//! **regression hardening + traceability**: it locks the exact #899 scenario
//! (mixed parsed + unparseable → continue) so it cannot silently regress, and
//! the recipe carries an explicit `#899` marker at the fail-closed guard.
//!
//! Out of scope (must NOT change): `extract_verdict`, `classify_validator`,
//! the `jq` slurpfile merge, threshold defaults, and validator prompts. The
//! recipe change accompanying these tests is a **comment-only** `#899`
//! traceability note at the FATAL guard.
//!
//! These tests drive the REAL shipped `merge-validations` command body
//! (extracted from `amplifier-bundle/recipes/quality-audit-cycle.yaml`), so
//! they need only `bash` + `jq` — never the `amplihack` binary. They mirror
//! the `issue_833` `run_merge()` behavioral pattern.
//!
//! Acceptance criteria encoded here, for **2 parsed + 1 unparseable @
//! threshold 2**:
//!   1. exit code `0` (the audit continues; it is NOT aborted)
//!   2. stdout is valid merged `validated_findings` JSON
//!   3. the unparseable validator contributes **zero votes** — the threshold is
//!      evaluated against the parsed validators only; `confirmed_count`
//!      reflects the 2 that parsed
//!   4. a single-line WARNING names the validator + the raw-artifact path
//!   5. the malformed raw output is preserved at
//!      `cycle_<N>/validator_<label>_raw.txt`
//!   6. the preserved raw output is byte-for-byte the malformed payload
//! Plus:
//!   * all-unparseable → still exit `1` (FATAL gate keyed on
//!     `parsed_count == 0`) — the fail-closed floor is preserved.
//!   * a malformed payload containing shell metacharacters is handled as DATA
//!     only: preserved literally, zero votes, no side-effect file created.

use serde_yaml::Value;
use std::path::PathBuf;
use std::process::Command;

// =========================================================================
// Shared helpers (mirror the issue_833 / issue_820 behavioral pattern).
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
/// command under bash. `out_dir` becomes `{{output_dir}}` so per-cycle
/// artifacts land somewhere inspectable.
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
    let dir = std::env::temp_dir().join(format!("qac899-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mk tmp out dir");
    dir
}

/// A strict-JSON `confirmed` verdict for finding_id 1 — one clean vote.
const CONFIRM_V: &str = r#"{"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"high","reasoning":"swallows error"}],"confirmed_count":1,"false_positive_count":0}"#;

fn assert_no_bad_json(run: &MergeRun) {
    assert!(
        !run.stderr.contains("Bad JSON"),
        "#899: merge-validations must never surface a raw `jq: Bad JSON` \
         error (that was the class of failure the abstention path replaces).\n\
         stderr:\n{}",
        run.stderr
    );
}

fn parse_stdout(run: &MergeRun) -> serde_json::Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "#899: merged stdout must be valid `validated_findings` JSON: {e}\n\
             stdout:\n{}\nstderr:\n{}",
            run.stdout, run.stderr
        )
    })
}

// =========================================================================
// Layer 1: Structural — the recipe still wires the abstention machinery and
// the #899 traceability marker lives at the fail-closed guard.
// =========================================================================

#[test]
fn recipe_parses_as_valid_yaml() {
    let _ = load_recipe();
}

#[test]
fn merge_validations_step_still_exists() {
    let _ = merge_validations_command();
}

#[test]
fn fatal_gate_is_keyed_on_zero_parsed_survivors() {
    // The abstention contract: the merge only fails hard when NOTHING parsed.
    // Any parsed survivor must let the merge continue. This asserts the guard
    // is `parsed_count == 0 && unparseable_count >= 1`, never a broader
    // "any unparseable -> abort".
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("parsed_count") && cmd.contains("unparseable_count"),
        "#899: the guard must classify parsed vs unparseable validators so a \
         single unparseable validator abstains instead of aborting.\n\
         Command:\n{cmd}"
    );
    assert!(
        cmd.contains("\"$parsed_count\" -eq 0") && cmd.contains("\"$unparseable_count\" -ge 1"),
        "#899: the FATAL gate must fire ONLY on `parsed_count == 0 && \
         unparseable_count >= 1` (fail-closed floor), never abort while a \
         parsed survivor exists.\nCommand:\n{cmd}"
    );
}

#[test]
fn recipe_carries_issue_899_traceability_marker() {
    // Traceability: the fail-closed guard carries an explicit `#899` marker so
    // future readers know the abstention-continues contract is load-bearing.
    let cmd = merge_validations_command();
    assert!(
        cmd.contains("#899") || cmd.contains("899"),
        "#899: the merge-validations step must carry a `#899` traceability \
         comment at the fail-closed guard tying the parsed/unparseable gate to \
         this issue.\nCommand:\n{cmd}"
    );
}

// =========================================================================
// Layer 2: Behavioral — run the REAL command body under bash + jq.
// =========================================================================

#[test]
fn primary_two_parsed_one_unparseable_continues_at_threshold_2() {
    // The #899 scenario: 2 parseable validators confirm finding 1, the 3rd is
    // prose-only (unparseable). It must ABSTAIN (zero votes) and the audit must
    // CONTINUE — exit 0, valid merged JSON, threshold applied to the 2 parsed
    // only, targeted single-line WARNING, raw artifact preserved verbatim.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("primary");
    let malformed = "I reviewed everything and it looks fine — no JSON verdict here.";
    let run = run_merge(
        CONFIRM_V,
        CONFIRM_V,
        malformed,
        "2",
        "5",
        out.to_str().unwrap(),
    );

    assert_no_bad_json(&run);

    // (1) exit 0 — the audit is NOT aborted.
    assert_eq!(
        run.code,
        Some(0),
        "#899 AC1: one unparseable validator must ABSTAIN, not abort. Expected \
         exit 0.\nstderr:\n{}\nstdout:\n{}",
        run.stderr,
        run.stdout
    );

    // (2) stdout is valid merged validated_findings JSON.
    let merged = parse_stdout(&run);

    // (3) unparseable contributes zero votes; threshold evaluated on the 2
    // parsed only → finding 1 confirmed, confirmed_count == 1.
    assert_eq!(
        merged["confirmed_count"], 1,
        "#899 AC3: the two parsed validators (>= threshold 2) confirm finding \
         1; the unparseable validator contributes ZERO votes.\n{merged:#}"
    );
    let finding = merged["validated"]
        .as_array()
        .and_then(|a| a.iter().find(|f| f["finding_id"] == 1))
        .expect("#899: merged output must contain finding 1");
    assert_eq!(
        finding["verdict"], "confirmed",
        "#899: finding 1 must be confirmed by the 2 parsed votes.\n{finding:#}"
    );
    assert_eq!(
        finding["votes"]["confirmed"], 2,
        "#899 AC3: exactly the 2 parsed validators voted; the unparseable one \
         added no phantom vote.\n{finding:#}"
    );

    // (4) single-line WARNING names the validator (v3) + the raw-artifact path.
    let warn_line = run
        .stderr
        .lines()
        .find(|l| l.contains("WARNING") && l.contains("unparseable"))
        .unwrap_or_else(|| {
            panic!(
                "#899 AC4: an unparseable validator must emit a targeted \
                 WARNING.\nstderr:\n{}",
                run.stderr
            )
        });
    assert!(
        warn_line.contains("v3"),
        "#899 AC4: the WARNING must name which validator (v3) abstained.\n\
         line: {warn_line}"
    );
    assert!(
        warn_line.contains("Raw output preserved at"),
        "#899 AC4: the WARNING must point at the preserved raw-output path.\n\
         line: {warn_line}"
    );
    assert!(
        !run.stderr.contains("FATAL"),
        "#899: a single abstention must NOT trip the FATAL gate.\nstderr:\n{}",
        run.stderr
    );

    // (5) raw artifact preserved at cycle_<N>/validator_<label>_raw.txt.
    let artifact = out.join("cycle_5").join("validator_v3_raw.txt");
    assert!(
        artifact.exists(),
        "#899 AC5: the unparseable validator's raw output must be preserved at \
         {}.\nstderr:\n{}",
        artifact.display(),
        run.stderr
    );

    // (6) preserved raw output is byte-for-byte the malformed payload.
    let preserved = std::fs::read_to_string(&artifact).expect("read preserved artifact");
    assert_eq!(
        preserved.trim_end_matches('\n'),
        malformed,
        "#899 AC6: the preserved raw artifact must be the malformed payload \
         verbatim (evidence, not a mangled copy)."
    );

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn all_unparseable_still_fatal_exit_1() {
    // The fail-closed floor is preserved: when NOTHING parsed (zero survivors)
    // and >=1 validator was unparseable, the merge must still fail hard with a
    // clear FATAL diagnostic — never a raw jq crash.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("allunparse");
    let p1 = "no json here, prose one";
    let p2 = "ERROR: agent crashed before structured output";
    let p3 = "[trace] stray { brace but no closing object";
    let run = run_merge(p1, p2, p3, "2", "5", out.to_str().unwrap());

    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(1),
        "#899: all-unparseable (zero parsed survivors) must still exit 1 — the \
         abstention change must NOT weaken the fail-closed floor.\n\
         stderr:\n{}\nstdout:\n{}",
        run.stderr,
        run.stdout
    );
    assert!(
        run.stderr.contains("FATAL")
            && run
                .stderr
                .contains("all validators produced unparseable output"),
        "#899: the all-unparseable failure must be a clear FATAL diagnostic.\n\
         stderr:\n{}",
        run.stderr
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn malformed_payload_with_shell_metacharacters_is_data_only() {
    // Security: an unparseable payload containing shell metacharacters must be
    // handled as DATA. It must be preserved literally, contribute zero votes,
    // and MUST NOT execute — no side-effect file may appear. The 2 parsed
    // validators still confirm finding 1.
    if !tooling_available() {
        eprintln!("skip: bash/jq not available");
        return;
    }
    let out = unique_out_dir("injection");
    let sentinel = out.join("pwned_sentinel");
    let _ = std::fs::remove_file(&sentinel);

    // Crafted metacharacters: command substitution, backticks, redirection.
    let malicious = format!(
        "$(touch {s}); `touch {s}`; > {s}; not-json ${{IFS}} still-prose",
        s = sentinel.display()
    );

    let run = run_merge(
        CONFIRM_V,
        CONFIRM_V,
        &malicious,
        "2",
        "7",
        out.to_str().unwrap(),
    );

    assert_no_bad_json(&run);
    assert_eq!(
        run.code,
        Some(0),
        "#899: malformed metacharacter payload must abstain, not abort.\n\
         stderr:\n{}",
        run.stderr
    );

    // No injection side-effect executed.
    assert!(
        !sentinel.exists(),
        "#899 SECURITY: validator output containing shell metacharacters must \
         be treated as DATA — the injected `touch {}` must NOT execute.",
        sentinel.display()
    );

    // Zero phantom votes: only the 2 parsed validators confirmed.
    let merged = parse_stdout(&run);
    assert_eq!(
        merged["confirmed_count"], 1,
        "#899 SECURITY: the malformed payload must not inject a phantom vote \
         past the threshold.\n{merged:#}"
    );
    let finding = merged["validated"]
        .as_array()
        .and_then(|a| a.iter().find(|f| f["finding_id"] == 1))
        .expect("#899: merged output must contain finding 1");
    assert_eq!(
        finding["votes"]["confirmed"], 2,
        "#899 SECURITY: exactly the 2 parsed validators voted.\n{finding:#}"
    );

    // Raw payload preserved verbatim (literal metacharacters, not expanded).
    let artifact = out.join("cycle_7").join("validator_v3_raw.txt");
    assert!(
        artifact.exists(),
        "#899: the malformed payload must be preserved for forensics at {}.",
        artifact.display()
    );
    let preserved = std::fs::read_to_string(&artifact).expect("read preserved artifact");
    assert!(
        preserved.contains("$(touch") && preserved.contains("`touch"),
        "#899 SECURITY: the raw artifact must contain the shell metacharacters \
         LITERALLY (proof they were never interpolated).\npreserved:\n{preserved}"
    );

    let _ = std::fs::remove_dir_all(&out);
}
