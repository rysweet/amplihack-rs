//! Issue #1337: reusable agentic loop-health evaluation contract.
//!
//! The motivating run: a default-workflow invocation spent 2h47m and produced
//! ZERO commits. Seven consecutive steps each reported exactly `10m 0s` with
//! no artifacts, one child returned `BLOCKED_TERMINAL` at depth 4/3 with exit
//! 79, and the run itself said "The review workflow is still running; I'm
//! waiting for its structured findings." Every one of those was a visible
//! signal and nothing acted on any of them.
//!
//! What is pinned here:
//!
//! - the `loop-health-evaluator` brick exists, parses, and stays inside the
//!   400-line brick budget;
//! - it has exactly one agentic judgement step, gated off on a terminal
//!   policy refusal so the run never retries into the exit-79 guard;
//! - it declares NO numeric iteration cap — not even as a backstop — and NO
//!   per-step timeout;
//! - the executable contract test (STUCK path + malformed-verdict path)
//!   passes against a binary built from this tree.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BRICK_LINE_BUDGET: usize = 400;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn recipe_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/loop-health-evaluator.yaml")
}

/// The deterministic measurement half, split out as its own brick and composed
/// as step-01. Measurement and judgement are different jobs with different
/// failure modes, and only one of them needs a model.
fn collector_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/loop-evidence-collector.yaml")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn parse(path: &Path) -> Value {
    serde_yaml::from_str(&read(path)).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn recipe_text() -> String {
    read(&recipe_path())
}

fn recipe_yaml() -> Value {
    parse(&recipe_path())
}

fn collector_yaml() -> Value {
    parse(&collector_path())
}

/// The bash body of the evidence-collection step, wherever it now lives.
fn collect_command() -> String {
    let collector = collector_yaml();
    field(step(&collector, "step-01-collect-loop-evidence"), "command").to_string()
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("loop-health-evaluator must declare top-level steps")
}

fn step<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    steps(recipe)
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing step `{id}`"))
}

fn field<'a>(step: &'a Value, name: &str) -> &'a str {
    step.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("step is missing a `{name}` field"))
}

#[test]
fn loop_health_evaluator_recipe_exists_and_fits_the_brick_budget() {
    // Both halves are bricks and both are held to the budget. The budget is
    // what forced the split when the contract needed more code than the
    // original design assumed — the answer was a second brick, never fewer
    // comments.
    for path in [recipe_path(), collector_path()] {
        let lines = read(&path).lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "{} is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}",
            path.display()
        );
    }
    assert_eq!(
        collector_yaml().get("name").and_then(Value::as_str),
        Some("loop-evidence-collector"),
        "the collector's `name:` must match its filename stem"
    );
    let recipe = recipe_yaml();
    assert_eq!(
        recipe.get("name").and_then(Value::as_str),
        Some("loop-health-evaluator"),
        "the recipe's `name:` must match its filename stem — `recipe run` resolves by stem \
         while `recipe list` keys on `name:`"
    );
}

/// The measurement half is composed, not inlined. Pinning the seam keeps a
/// future edit from quietly folding 120 lines of bash back into the evaluator
/// and blowing the brick budget again.
#[test]
fn evidence_collection_is_composed_as_its_own_brick() {
    let recipe = recipe_yaml();
    let collect = step(&recipe, "step-01-collect-loop-evidence");
    assert_eq!(collect.get("type").and_then(Value::as_str), Some("recipe"));
    assert_eq!(
        collect.get("recipe").and_then(Value::as_str),
        Some("loop-evidence-collector")
    );
    let collector = collector_yaml();
    let inner = step(&collector, "step-01-collect-loop-evidence");
    assert_eq!(
        inner.get("output").and_then(Value::as_str),
        Some("loop_evidence")
    );
    assert_eq!(
        inner.get("parse_json").and_then(Value::as_bool),
        Some(true),
        "the evidence output is consumed as structured data by step-02's condition"
    );
}

#[test]
fn verdict_contract_has_exactly_three_outcomes() {
    let recipe = recipe_yaml();
    let prompt = field(step(&recipe, "step-02-evaluate-loop-health"), "prompt");
    for token in ["CONTINUE", "DONE", "STUCK"] {
        assert!(
            prompt.contains(token),
            "the evaluator prompt must define the `{token}` outcome"
        );
    }
    assert!(
        prompt.contains("loop_verdict"),
        "the evaluator must emit its verdict under the `loop_verdict` field"
    );
    // The fail-safe direction has to be stated to the model as well as
    // enforced in the gate.
    assert!(
        prompt.contains("never as `CONTINUE`") || prompt.contains("never as CONTINUE"),
        "the prompt must state that a malformed verdict is STUCK, never CONTINUE"
    );
}

#[test]
fn evaluator_receives_real_evidence_not_prose_impressions() {
    let recipe = recipe_yaml();
    let collect = collect_command();
    let collect = collect.as_str();
    let prompt = field(step(&recipe, "step-02-evaluate-loop-health"), "prompt");

    // Every evidence channel required by issue #1337 must be both computed
    // and surfaced to the judge.
    for computed in [
        "commits_since_baseline",
        "diff_lines",
        "repeated_duration_count",
        "repeated_output",
        "findings_new",
        "findings_recurring",
        "findings_resolved",
        "tests_moved",
        "ci_moved",
        "waiting_on_output",
    ] {
        assert!(
            collect.contains(computed),
            "step-01 must compute `{computed}` from real evidence"
        );
        assert!(
            prompt.contains(computed),
            "the evaluator prompt must surface `{computed}` to the judge"
        );
    }
    // `terminal_refusal` is computed but deliberately NOT surfaced to the
    // judge: on a terminal policy refusal the judge is never asked at all.
    assert!(
        collect.contains("terminal_refusal"),
        "step-01 must compute `terminal_refusal`"
    );
    assert!(
        prompt.contains("{{loop_history}}") && prompt.contains("{{loop_last_round_output}}"),
        "the judge must see the accumulated round verdicts and the last round's raw output"
    );
}

/// Issue #1267 — the mechanical failure classification must reach the agentic
/// judge as DATA. A round that produced nothing because the model endpoint was
/// overloaded is not a loop failing to converge, and the evaluator can only say
/// so if the evidence brick measures it and the prompt surfaces it.
#[test]
fn infrastructure_fault_is_measured_and_surfaced_to_the_judge() {
    let recipe = recipe_yaml();
    let collect = collect_command();
    let prompt = field(step(&recipe, "step-02-evaluate-loop-health"), "prompt");

    for computed in ["infrastructure_fault", "infrastructure_fault_class"] {
        assert!(
            collect.contains(computed),
            "step-01 must compute `{computed}` from the run's classification marker"
        );
        assert!(
            prompt.contains(computed),
            "the evaluator prompt must surface `{computed}` to the judge"
        );
    }

    // The marker prefix is a cross-language contract: the recipe greps for the
    // exact string the Rust runner writes. Pin both ends so a rename to one
    // side cannot silently make the detector inert.
    const MARKER: &str = "amplihack.recipe.failure_class";
    assert!(
        collect.contains(MARKER),
        "the evidence brick must grep for the marker the runner actually writes"
    );
    let emitter = read(
        &workspace_root().join("crates/amplihack-cli/src/commands/recipe/run/failure_class.rs"),
    );
    assert!(
        emitter.contains(MARKER),
        "the Rust classifier must still emit `{MARKER}`; the recipe detector greps for it"
    );

    // A `work`-class failure is a real work failure and must NOT be excused as
    // infrastructure — only the two infrastructure classes count.
    assert!(
        collect.contains("transient_transport|environmental"),
        "only transient-transport and environmental classes may count as an \
         infrastructure fault; a work failure must still read as a work failure"
    );
}

#[test]
fn no_numeric_iteration_cap_anywhere() {
    // The core design decision of #1337: the loop terminator is absence of
    // progress, not attempt count. A max-iterations integer cuts off work
    // that was about to converge AND lets a genuinely stuck loop burn the
    // whole budget first. Host safety is structural, one layer down
    // (#1327 sealed depth ceiling, #1332 width cap + memory floor).
    let text = format!("{}\n{}", recipe_text(), read(&collector_path()));
    for banned in [
        "max_iterations",
        "max_iteration",
        "max_rounds",
        "max_attempts",
        "iteration_limit",
        "MAX_LOOPS",
        "max_retries",
    ] {
        for (n, line) in text.lines().enumerate() {
            // The prose that explains WHY there is no counter is allowed to
            // name the thing it rejects.
            if line.trim_start().starts_with('#') {
                continue;
            }
            assert!(
                !line.contains(banned),
                "line {}: loop-health-evaluator.yaml introduces a numeric iteration cap \
                 (`{banned}`), which issue #1337 rejects outright:\n  {line}",
                n + 1
            );
        }
    }
    // `recursion.max_depth` / `max_total_steps` are the runner's own
    // structural guard rails, not the loop terminator, and must stay.
    let recipe = recipe_yaml();
    assert!(
        recipe.get("recursion").is_some(),
        "the structural recursion guard rails must remain declared"
    );
}

#[test]
fn no_per_step_timeout_at_any_scale() {
    // Issue #439 plus the #1337 constraint: nothing here may be bounded at
    // seconds or single-digit-minute scale. The runner owns the ceiling.
    for recipe in [recipe_yaml(), collector_yaml()] {
        for s in steps(&recipe) {
            let id = s.get("id").and_then(Value::as_str).unwrap_or("<unnamed>");
            for key in ["timeout", "timeout_seconds"] {
                assert!(
                    s.get(key).is_none(),
                    "step `{id}` declares `{key}` — no step in this brick may carry a \
                     per-step timeout"
                );
            }
        }
        assert!(
            recipe.get("default_step_timeout").is_none(),
            "neither loop-health brick may pin a recipe-level step timeout"
        );
    }
}

#[test]
fn exit_79_is_terminal_and_never_retried_into() {
    let recipe = recipe_yaml();
    let collect = collect_command();
    assert!(
        collect.contains("BLOCKED_TERMINAL"),
        "step-01 must detect a BLOCKED_TERMINAL child"
    );
    assert!(
        collect.contains("79"),
        "step-01 must detect exit code 79 (the #1327/#1332 policy refusal)"
    );

    // The agentic step is gated OFF on a terminal refusal: exit 79 is already
    // a final answer from a structural guard, so not even a model call is
    // spent deciding whether to re-enter it.
    let evaluate = step(&recipe, "step-02-evaluate-loop-health");
    assert_eq!(
        evaluate.get("condition").and_then(Value::as_str),
        Some("loop_evidence.terminal_refusal == 'false'"),
        "the evaluator agent step must be skipped on a terminal policy refusal"
    );

    let resolve = field(step(&recipe, "step-03-resolve-loop-verdict"), "command");
    assert!(
        resolve.contains("terminal_policy_refusal"),
        "step-03 must attribute a forced STUCK to the terminal policy refusal"
    );
}

#[test]
fn verdict_resolution_uses_the_canonical_orch_helper_pipeline() {
    // The verdict contract must fit docs/reference/structured-verdict-parsing.md,
    // not compete with it: extract-json | extract-field --default | normalise.
    let recipe = recipe_yaml();
    for id in [
        "step-03-resolve-loop-verdict",
        "step-04-enforce-loop-verdict",
    ] {
        let cmd = field(step(&recipe, id), "command");
        assert!(
            cmd.contains("amplihack orch helper extract-json"),
            "{id} must route agent output through `orch helper extract-json`"
        );
        assert!(
            cmd.contains("--field loop_verdict --default STUCK"),
            "{id} must default a missing `loop_verdict` to STUCK, never CONTINUE"
        );
        assert!(
            cmd.contains("amplihack orch helper normalise-loop-verdict"),
            "{id} must normalise the token with `orch helper normalise-loop-verdict`"
        );
        // Agent output is untrusted data: it arrives via env + stdin and is
        // never interpolated into a command position.
        assert!(
            !cmd.contains("{{loop_health_assessment}}"),
            "{id} must not interpolate agent output into the command line"
        );
    }
}

/// Issue #1337, finding B1 — the defect that made the whole brick inert.
///
/// `recipe-runner-rs` exports every step output as `RECIPE_VAR_<name>`, but it
/// only adds the plain-uppercase alias for SCALARS. `loop_evidence` and
/// `loop_health` declare `parse_json: true`, and an agent that OBEYS the
/// "emit only the JSON object" contract makes `loop_health_assessment` an
/// object too — so `LOOP_EVIDENCE`, `LOOP_HEALTH` and `LOOP_HEALTH_ASSESSMENT`
/// are simply ABSENT at runtime. Reading them bare made every run resolve to
/// STUCK and fabricate an exit-79 policy refusal as the reason.
///
/// The end-to-end probe in the shell test is what proves this at runtime; this
/// test is the cheap static guard that keeps a future edit from dropping a
/// fallback again in a repo where the runner may not be installed.
#[test]
fn object_valued_step_outputs_are_read_with_the_recipe_var_fallback() {
    let recipe = recipe_yaml();
    // (env name, runner name) for every step output this brick reads back.
    let object_outputs = [
        ("LOOP_EVIDENCE", "RECIPE_VAR_loop_evidence"),
        ("LOOP_HEALTH", "RECIPE_VAR_loop_health"),
        (
            "LOOP_HEALTH_ASSESSMENT",
            "RECIPE_VAR_loop_health_assessment",
        ),
    ];
    for s in steps(&recipe) {
        let id = s.get("id").and_then(Value::as_str).unwrap_or("<unnamed>");
        let Some(cmd) = s.get("command").and_then(Value::as_str) else {
            continue;
        };
        for (plain, recipe_var) in object_outputs {
            let needle = format!("${{{plain}:-");
            for (n, line) in cmd.lines().enumerate() {
                if !line.contains(&needle) {
                    continue;
                }
                assert!(
                    line.contains(recipe_var),
                    "{id} line {}: `${{{plain}:-...}}` is read without its \
                     `{recipe_var}` fallback. The runner never sets `{plain}` for a \
                     JSON-object step output, so this read is silently empty and the \
                     gate fabricates a STUCK. Use \
                     `${{{plain}:-${{{recipe_var}:-}}}}`.\n  {line}",
                    n + 1
                );
            }
        }
    }
    // And at least one such dual-name read must actually be present, so the
    // guard cannot pass vacuously if the reads are renamed away.
    let all: String = steps(&recipe)
        .iter()
        .filter_map(|s| s.get("command").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    for (_, recipe_var) in object_outputs {
        assert!(
            all.contains(recipe_var),
            "no step reads `{recipe_var}`; the brick cannot see its own step outputs"
        );
    }
}

/// Issue #1337, finding B3 — first-JSON-wins is fail-open for a verdict.
///
/// The extractor and the prompt have to agree on WHICH object is the verdict.
/// The decision taken here is: the LAST object carrying `loop_verdict`.
#[test]
fn verdict_selection_is_last_object_carrying_the_field_and_the_prompt_says_so() {
    let recipe = recipe_yaml();
    let resolve = field(step(&recipe, "step-03-resolve-loop-verdict"), "command");
    assert!(
        resolve.contains("extract-json --require-field loop_verdict"),
        "step-03 must select the object that actually carries `loop_verdict`; \
         first-parseable-object-wins reads a draft verdict over the reconsidered \
         one, and reads quoted-back evidence over the real verdict"
    );
    let prompt = field(step(&recipe, "step-02-evaluate-loop-health"), "prompt");
    assert!(
        prompt.contains("LAST JSON object"),
        "the prompt must tell the model the same selection rule the gate applies"
    );
}

/// The `--require-field` selection, exercised through the real binary against
/// the two cases measured on the shipped extractor.
#[test]
fn require_field_selection_is_wired_into_the_cli() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    let cases = [
        (
            "reconsidered verdict wins over the draft",
            "{\"plan\":\"check\",\"loop_verdict\":\"CONTINUE\"}\n\
             On reflection nothing moved.\n\
             {\"loop_verdict\":\"STUCK\",\"not_converging\":[\"zero commits\"]}\n",
            "STUCK",
        ),
        (
            "evidence quoted back in a fence is not the verdict",
            "Here is the evidence I was given:\n\
             ```json\n{\"commits_since_baseline\": 3, \"diff_lines\": 120}\n```\n\
             It moved. Verdict:\n\
             {\"loop_verdict\": \"CONTINUE\", \"moved\": [\"3 commits\"]}\n",
            "CONTINUE",
        ),
    ];
    for (name, input, want) in cases {
        let selected = helper_stdin(
            &bin,
            &[
                "orch",
                "helper",
                "extract-json",
                "--require-field",
                "loop_verdict",
            ],
            input,
        );
        let verdict = helper_stdin(
            &bin,
            &[
                "orch",
                "helper",
                "extract-field",
                "--field",
                "loop_verdict",
                "--default",
                "STUCK",
            ],
            &selected,
        );
        assert_eq!(verdict.trim(), want, "{name}: selected {selected:?}");
    }
}

/// Issue #1337: `--default true` is the fail-safe the gate leans on hardest,
/// and an explicit JSON `null` used to walk straight past it.
#[test]
fn explicit_json_null_takes_the_default_through_the_cli() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    let out = helper_stdin(
        &bin,
        &[
            "orch",
            "helper",
            "extract-field",
            "--field",
            "terminal_refusal",
            "--default",
            "true",
        ],
        "{\"terminal_refusal\": null}",
    );
    assert_eq!(
        out.trim(),
        "true",
        "`{{\"terminal_refusal\": null}}` must take the `--default true` fail-safe, \
         not read as \"the guard did not fire\""
    );
}

/// Run `amplihack <args>` with `stdin`, returning stdout.
fn helper_stdin(bin: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn amplihack");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "amplihack {args:?} exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn normalise_loop_verdict_helper_is_wired_into_the_cli() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    // The whole point of the default: garbage in, STUCK out.
    for (input, want) in [
        ("CONTINUE", "CONTINUE"),
        ("DONE", "DONE"),
        ("STUCK", "STUCK"),
        ("", "STUCK"),
        ("banana", "STUCK"),
        // Negation-adjacent tokens that CONTAIN a permissive token. A
        // `str::contains` implementation would fail OPEN on every one.
        ("DISCONTINUE", "STUCK"),
        ("CANNOT_CONTINUE", "STUCK"),
        ("NOT_DONE", "STUCK"),
    ] {
        let out = crate_run(&bin, input);
        assert_eq!(
            out.trim(),
            want,
            "`normalise-loop-verdict` on {input:?} must print {want}"
        );
    }
}

fn crate_run(bin: &Path, stdin: &str) -> String {
    use std::io::Write;
    let mut child = Command::new(bin)
        .args(["orch", "helper", "normalise-loop-verdict"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn amplihack");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "normalise-loop-verdict exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Runs the executable contract test — the STUCK path and the
/// malformed-verdict path exercised against the real extracted step bodies.
///
/// Wired here because `.github/workflows/ci.yml` lists the bash recipe tests
/// one by one; running it from `cargo test` gets the same coverage without
/// touching that file.
#[test]
fn loop_health_contract_shell_test_passes() {
    let root = workspace_root();
    let script =
        root.join("amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh");
    assert!(script.is_file(), "missing {}", script.display());

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    let bin_dir = bin.parent().expect("binary parent dir");
    let path = match std::env::var("PATH") {
        Ok(p) => format!("{}:{p}", bin_dir.display()),
        Err(_) => bin_dir.display().to_string(),
    };

    let out = Command::new("bash")
        .arg(&script)
        .current_dir(&root)
        .env("PATH", path)
        .output()
        .expect("run the loop-health contract shell test");

    assert!(
        out.status.success(),
        "test-issue-1337-loop-health-evaluator.sh failed ({:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
