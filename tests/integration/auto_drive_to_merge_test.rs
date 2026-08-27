//! `auto-drive-to-merge`: wrap `default-workflow` and drive the PR it produces
//! all the way to a merge — crusty as the maintainer's proxy until zero
//! concerns remain, then the merge-ready criteria until every one holds, then
//! merge behind an evidence gate.
//!
//! What is pinned here — the properties whose absence would be silent and
//! expensive:
//!
//! - Neither loop is terminated by a numeric iteration cap. Not a `max_rounds`,
//!   not a "backstop" integer. Both loops delegate to the `loop-health-evaluator`
//!   contract (issue #1337, PR #1347) and it is invoked by name, never copied.
//! - Every verdict is structured and read through the canonical
//!   `extract-json | extract-field --default …` pipeline, and every fail-safe
//!   default is the BLOCKING token: `CONCERNS`, `NOT_MERGE_READY`, `STUCK`.
//! - The two absolute prohibitions — a hook-skipping commit flag and a
//!   branch-protection bypass — never appear in an executable position.
//! - Nothing merges without every criterion re-verified in the same run and
//!   bound to one head SHA; an unreadable criterion is a failure.
//! - Exit 79 is terminal and is never retried into.
//! - No step declares a timeout at any scale.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BRICK_LINE_BUDGET: usize = 400;

const AUTODRIVE_RECIPES: [&str; 7] = [
    "auto-drive-to-merge",
    "autodrive-build",
    "autodrive-crusty-round",
    "autodrive-crusty-loop",
    "autodrive-merge-evidence",
    "autodrive-merge-round",
    "autodrive-merge-loop",
];

const AUTODRIVE_TOOLS: [&str; 3] = [
    "autodrive_loop.sh",
    "autodrive_merge_gate.sh",
    "autodrive_state.sh",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml"))
}

fn tool_path(name: &str) -> PathBuf {
    workspace_root().join(format!("amplifier-bundle/tools/{name}"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe_text(name: &str) -> String {
    read(&recipe_path(name))
}

fn recipe_yaml(name: &str) -> Value {
    serde_yaml::from_str(&recipe_text(name))
        .unwrap_or_else(|e| panic!("parse {}: {e}", recipe_path(name).display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must declare top-level steps")
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

/// Every bash `command:` body in a recipe — the executable positions.
fn command_bodies(name: &str) -> Vec<(String, String)> {
    let recipe = recipe_yaml(name);
    steps(&recipe)
        .iter()
        .filter_map(|s| {
            let id = s.get("id").and_then(Value::as_str)?.to_string();
            let cmd = s.get("command").and_then(Value::as_str)?.to_string();
            Some((id, cmd))
        })
        .collect()
}

/// Files that make up the workflow's control path (no prose).
fn control_path_files() -> Vec<PathBuf> {
    AUTODRIVE_RECIPES
        .iter()
        .map(|r| recipe_path(r))
        .chain(AUTODRIVE_TOOLS.iter().map(|t| tool_path(t)))
        .collect()
}

// ── Structure ────────────────────────────────────────────────────────────────

#[test]
fn every_autodrive_recipe_parses_and_fits_the_brick_budget() {
    for name in AUTODRIVE_RECIPES {
        let path = recipe_path(name);
        assert!(path.is_file(), "missing {}", path.display());
        let text = read(&path);
        let lines = text.lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "{name}.yaml is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
        );
        let recipe = recipe_yaml(name);
        assert_eq!(
            recipe.get("name").and_then(Value::as_str),
            Some(name),
            "{name}.yaml's `name:` must match its filename stem — `recipe run` \
             resolves by stem while `recipe list` keys on `name:`"
        );
        assert!(!steps(&recipe).is_empty(), "{name}.yaml declares no steps");
    }
    for tool in AUTODRIVE_TOOLS {
        assert!(tool_path(tool).is_file(), "missing tool {tool}");
    }
}

#[test]
fn composer_delegates_to_the_three_phases_in_order() {
    let recipe = recipe_yaml("auto-drive-to-merge");
    let sub: Vec<&str> = steps(&recipe)
        .iter()
        .filter_map(|s| s.get("recipe").and_then(Value::as_str))
        .collect();
    assert_eq!(
        sub,
        vec![
            "autodrive-build",
            "autodrive-crusty-loop",
            "autodrive-merge-loop"
        ],
        "the composer must stay thin: build, then the crusty loop, then the \
         merge-ready loop — in that order"
    );
    // Phase 1 must not merge. auto-drive owns the merge decision.
    let build = step(&recipe, "autodrive-build");
    assert!(
        recipe_text("auto-drive-to-merge").contains("no_merge: \"true\""),
        "the composer must set no_merge so default-workflow does not merge"
    );
    assert!(
        build.get("context").is_some(),
        "the build step must forward context explicitly"
    );
    let build_recipe = recipe_text("autodrive-build");
    assert!(
        build_recipe.contains("no_merge: \"true\"")
            && build_recipe.contains("should_merge: \"false\""),
        "autodrive-build must take the merge decision away from default-workflow"
    );
    assert!(
        build_recipe.contains("recipe: \"default-workflow\""),
        "phase 1 must run default-workflow — the whole point is to wrap it"
    );
}

// ── The loop terminator ──────────────────────────────────────────────────────

#[test]
fn both_loops_terminate_on_the_loop_health_evaluator_contract() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    assert!(
        driver.contains("recipe run loop-health-evaluator"),
        "the loop driver must invoke the shared loop-health-evaluator brick by name"
    );
    for ctx in [
        "loop_name=",
        "loop_round_label=",
        "loop_history=",
        "loop_last_round_output=",
        "loop_baseline_ref=",
        "loop_child_exit_code=",
        "loop_findings_current=",
        "loop_findings_previous=",
        "loop_test_signal=",
        "loop_ci_signal=",
    ] {
        assert!(
            driver.contains(ctx),
            "the loop driver must hand `{ctx}` to the evaluator — the evaluator \
             judges measured evidence, not a prose summary of it"
        );
    }
    // Both phases must go through the one driver rather than growing their own.
    for phase in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let text = recipe_text(phase);
        assert!(
            text.contains("autodrive_loop.sh"),
            "{phase} must drive its loop through the shared driver"
        );
    }
    // The contract must be USED, never copied: the evaluator's own step ids
    // must not appear anywhere in this workflow.
    for path in control_path_files() {
        let text = read(&path);
        for copied in [
            "step-01-collect-loop-evidence",
            "step-02-evaluate-loop-health",
            "step-03-resolve-loop-verdict",
            "step-04-enforce-loop-verdict",
        ] {
            assert!(
                !text.contains(copied),
                "{} copies the loop-health contract (`{copied}`) instead of \
                 invoking it — there is one loop-health contract, in #1347",
                path.display()
            );
        }
    }
}

#[test]
fn no_numeric_iteration_cap_anywhere() {
    // Not a max-rounds integer, not a backstop, not a wall-clock budget. An
    // integer cap cuts off the round that was about to converge AND lets a
    // stuck loop burn the whole budget first. Host safety lives one layer
    // down in #1327 / #1332, which refuse with exit 79.
    let forbidden = [
        "max_iterations",
        "max_rounds",
        "max_attempts",
        "max_retries",
        "iteration_cap",
        "iteration_limit",
        "round_limit",
    ];
    for path in control_path_files() {
        let text = read(&path);
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue; // prose explaining the absence is not a cap
            }
            let lower = line.to_ascii_lowercase();
            for token in forbidden {
                assert!(
                    !lower.contains(token),
                    "{}:{} introduces `{token}`. Neither loop may be terminated \
                     by a count — the terminator is loop-health-evaluator.\n  {line}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
    // The round counter that does exist is a LABEL. Nothing may compare it.
    let driver = read(&tool_path("autodrive_loop.sh"));
    assert!(
        driver.contains("ROUND is a LABEL"),
        "the loop driver must state that its round counter is a label"
    );
    for line in driver.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        assert!(
            !(line.contains("$ROUND")
                && (line.contains("-ge") || line.contains("-gt") || line.contains("-le"))),
            "the round label is compared against a limit: {line}"
        );
    }
}

#[test]
fn no_short_timeouts_anywhere() {
    // Issue #439: the runner owns the ceiling. Nothing here is bounded at
    // seconds or single-digit-minute scale — CI polling, test suites, builds
    // and model calls all run to their natural end.
    for name in AUTODRIVE_RECIPES {
        let recipe = recipe_yaml(name);
        assert!(
            recipe.get("default_step_timeout").is_none(),
            "{name}.yaml declares a default_step_timeout"
        );
        for s in steps(&recipe) {
            let id = s.get("id").and_then(Value::as_str).unwrap_or("<unnamed>");
            for key in ["timeout", "timeout_seconds"] {
                assert!(
                    s.get(key).is_none(),
                    "{name}.yaml step `{id}` declares `{key}`"
                );
            }
        }
    }
    // The only wait in the workflow is the CI poll, and its interval is 60s.
    let evidence = recipe_text("autodrive-merge-evidence");
    assert!(
        evidence.contains("sleep 60"),
        "the CI wait must poll on a 60-second interval"
    );
    for path in control_path_files() {
        let text = read(&path);
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            for short in [
                "sleep 1",
                "sleep 2",
                "sleep 3",
                "sleep 5",
                "timeout 5",
                "timeout 10",
                "timeout 30",
                "timeout 60",
                "timeout 120",
                "timeout 300",
            ] {
                assert!(
                    !line.contains(short),
                    "{}:{} introduces a short bound `{short}`: {line}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
}

// ── Structured verdicts ──────────────────────────────────────────────────────

#[test]
fn verdict_gates_use_the_canonical_orch_helper_pipeline() {
    let cases = [
        (
            "autodrive-crusty-round",
            "step-03-extract-crusty-verdict",
            "crusty_verdict",
            "CONCERNS",
            "CRUSTY_REVIEW",
        ),
        (
            "autodrive-merge-round",
            "step-03-extract-merge-ready-verdict",
            "merge_ready_verdict",
            "NOT_MERGE_READY",
            "MERGE_READY_REVIEW",
        ),
    ];
    for (recipe_name, step_id, verdict_field, blocking_default, env_var) in cases {
        let recipe = recipe_yaml(recipe_name);
        let s = step(&recipe, step_id);
        let cmd = field(s, "command");
        assert_eq!(
            s.get("parse_json").and_then(Value::as_bool),
            Some(true),
            "{step_id} must emit a parse_json object so engine conditions can read it"
        );
        assert!(
            cmd.contains("amplihack orch helper extract-json"),
            "{step_id} must route agent output through `orch helper extract-json`"
        );
        assert!(
            cmd.contains(&format!(
                "--field {verdict_field} --default {blocking_default}"
            )),
            "{step_id} must default a missing `{verdict_field}` to the BLOCKING \
             token `{blocking_default}` — never to the permissive one"
        );
        // Agent output is untrusted data: env var + stdin, never interpolated
        // into a command position.
        assert!(
            cmd.contains(&format!("${{{env_var}:-}}")),
            "{step_id} must read the agent output from the environment"
        );
        assert!(
            cmd.contains("printf '%s' \""),
            "{step_id} must feed the agent output on stdin with printf '%s'"
        );
        assert!(
            !cmd.contains("eval ") && !cmd.contains("bash -c \"$"),
            "{step_id} must never evaluate agent output"
        );
        // Exact-token allow-list, so a token that merely CONTAINS a clean
        // token cannot smuggle a pass through.
        assert!(
            cmd.contains("case \"$VERDICT\" in"),
            "{step_id} must match the verdict against an exact-token allow-list"
        );
    }
}

#[test]
fn advancing_a_phase_requires_both_the_round_verdict_and_the_loop_verdict() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    assert!(
        driver.contains("ROUND_CLEAN=\"true\""),
        "the driver must track whether the round's own verdict was the clean token"
    );
    assert!(
        driver.contains("inconsistent pair never advances a phase"),
        "a DONE verdict over a non-clean round verdict must never advance a phase"
    );
    // A missing round record is never a clean round.
    assert!(
        driver.contains("treated as NOT clean"),
        "a missing or unparseable round record must never be read as clean"
    );
    // Measurement outranks the model in the merge round.
    let merge = recipe_text("autodrive-merge-round");
    assert!(
        merge.contains("downgrading MERGE_READY to NOT_MERGE_READY"),
        "a MERGE_READY verdict must be downgraded when measured evidence disagrees"
    );
    for signal in ["qa_status", "ci_status", "conflict", "unresolved_threads"] {
        assert!(
            merge.contains(signal),
            "the downgrade must consider the measured `{signal}`"
        );
    }
}

#[test]
fn an_unreadable_loop_verdict_is_stuck_never_continue() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    assert!(
        driver.contains("LOOP_VERDICT=\"STUCK\""),
        "the loop verdict must start at the fail-safe STUCK"
    );
    let idx_default = driver
        .find("LOOP_VERDICT=\"STUCK\"")
        .expect("fail-safe default");
    let idx_continue = driver
        .find("LOOP_VERDICT=\"CONTINUE\"")
        .expect("CONTINUE branch");
    assert!(
        idx_default < idx_continue,
        "CONTINUE must be reached only by positively reading the marker; the \
         default must already be STUCK"
    );
    assert!(
        driver.contains("failing safe to STUCK"),
        "an evaluator that exits 0 with no readable verdict must fail safe to STUCK"
    );
}

// ── The two absolute prohibitions ────────────────────────────────────────────

#[test]
fn forbidden_flags_never_appear_in_an_executable_position() {
    // Never a hook-skipping commit flag; never a branch-protection bypass.
    // A line may NAME one only while marking it as prohibited, and no
    // executable line may name one at all. If a hook or a check fails, the
    // cause is fixed.
    let patterns = ["--no-verify", "--admin", "--bypass"];
    let markers = ["never", "forbidden", "prohibit"];

    let mut scanned = control_path_files();
    scanned.push(workspace_root().join("amplifier-bundle/skills/auto-drive-to-merge/SKILL.md"));
    scanned.push(workspace_root().join("docs/claude/skills/auto-drive-to-merge/SKILL.md"));
    scanned.push(workspace_root().join("docs/reference/auto-drive-to-merge.md"));

    for path in &scanned {
        let text = read(path);
        for (n, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            if !patterns.iter().any(|p| lower.contains(p)) {
                continue;
            }
            assert!(
                markers.iter().any(|m| lower.contains(m)),
                "{}:{} names a prohibited flag without marking it prohibited:\n  {line}",
                path.display(),
                n + 1
            );
        }
    }
    // Executable positions: shell tools and every recipe `command:` body.
    for tool in AUTODRIVE_TOOLS {
        let path = tool_path(tool);
        for (n, line) in read(&path).lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            for p in patterns {
                assert!(
                    !lower.contains(p),
                    "{}:{} uses `{p}` in an executable position:\n  {line}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
    for name in AUTODRIVE_RECIPES {
        for (id, body) in command_bodies(name) {
            for line in body.lines() {
                if line.trim_start().starts_with('#') {
                    continue;
                }
                let lower = line.to_ascii_lowercase();
                for p in patterns {
                    assert!(
                        !lower.contains(p),
                        "{name}.yaml step `{id}` uses `{p}` in an executable position:\n  {line}"
                    );
                }
            }
        }
    }
    // The prohibition is structural at the merge, not merely advisory.
    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    assert!(
        gate.contains(
            r#"MERGE_ARGV=(pr merge "$PR" --squash --delete-branch --match-head-commit "$HEAD_SHA")"#
        ),
        "the merge argv must be a fixed literal list that takes no caller flags"
    );
    assert!(
        gate.contains(r#"if [ "${MERGE_ARGV[*]}" != "${EXPECTED_ARGV[*]}" ]"#),
        "the merge argv must be asserted unchanged immediately before execution"
    );
}

// ── No silent merge ──────────────────────────────────────────────────────────

#[test]
fn merge_gate_verifies_every_criterion_and_binds_them_to_one_head_sha() {
    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    for criterion in [
        "isDraft",
        "mergeable",
        "mergeStateStatus",
        "reviewDecision",
        "reviewThreads",
        "gh pr checks",
        "qa_status",
        "merge_ready_verdict",
    ] {
        assert!(
            gate.contains(criterion),
            "the merge gate must re-verify `{criterion}` in the run that merges"
        );
    }
    // Every criterion binds to one SHA, and GitHub itself enforces the binding.
    assert!(
        gate.contains("--match-head-commit"),
        "the merge must refuse if the head moved after the evidence was captured"
    );
    assert!(
        gate.contains("evidence must bind to the SHA being merged"),
        "evidence captured against a different head SHA must be a blocker"
    );
    // Unreadable is a failure, never a pass.
    for unreadable in [
        "metadata is unreadable",
        "CI status for #${PR} is unreadable",
        "review-thread state is unreadable",
    ] {
        assert!(
            gate.contains(unreadable),
            "the merge gate must treat `{unreadable}` as a blocker"
        );
    }
    assert!(
        gate.contains("zero checks is not a green build"),
        "a PR reporting no checks at all must not be treated as green"
    );
    // The evidence bundle is written BEFORE anything is merged.
    let bundle_at = gate
        .find("merge evidence written to")
        .expect("evidence bundle");
    let merge_at = gate
        .find("gh \"${MERGE_ARGV[@]}\"")
        .expect("the merge call");
    assert!(
        bundle_at < merge_at,
        "the evidence bundle must be written before the merge, not after"
    );
    // And the platform must confirm the merge afterwards.
    assert!(
        gate.contains("the platform does not confirm MERGED"),
        "a gh success the platform does not confirm must be reported as not merged"
    );
    // Merged work is never redone.
    assert!(
        gate.contains("ALREADY_MERGED"),
        "an already-merged PR must short-circuit rather than re-merge"
    );
}

#[test]
fn exit_79_is_terminal_and_never_retried_into() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    assert!(
        driver.contains("AUTODRIVE_EXIT_POLICY_REFUSAL=79"),
        "the loop driver must name exit 79 as the terminal policy refusal"
    );
    assert!(
        driver.contains("BLOCKED_TERMINAL"),
        "BLOCKED_TERMINAL must be recognised alongside exit 79"
    );
    assert!(
        driver.contains("never retried into") || driver.contains("NEVER retried"),
        "the driver must document that the guard is never retried into"
    );
    // The refusal is detected BEFORE the evaluator runs, so not even a model
    // call is spent deciding whether to re-enter a sealed guard.
    let refusal_at = driver
        .find("if terminal_refusal \"$ROUND_RC\"")
        .expect("round refusal check");
    let evaluator_at = driver
        .find("recipe run loop-health-evaluator")
        .expect("evaluator call");
    assert!(
        refusal_at < evaluator_at,
        "the terminal refusal must be checked before the evaluator is invoked"
    );
    for phase in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let text = recipe_text(phase);
        assert!(
            text.contains("exit 79"),
            "{phase} must surface and propagate the exit-79 policy refusal"
        );
    }
}

#[test]
fn recursion_context_is_propagated_and_the_ceiling_is_never_raised() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    for var in [
        "AMPLIHACK_TREE_ID",
        "AMPLIHACK_SESSION_DEPTH",
        "AMPLIHACK_MAX_DEPTH",
    ] {
        assert!(driver.contains(var), "{var} must be propagated to children");
    }
    assert!(
        driver.contains("assert_ceiling_untouched"),
        "the driver must abort if the inherited depth ceiling changes"
    );
    assert!(
        driver.contains("AUTODRIVE_INHERITED_MAX_DEPTH"),
        "the inherited ceiling must be captured so a change can be detected"
    );
    assert!(
        driver.contains("SEQUENTIAL AT CONSTANT DEPTH")
            || driver.contains("sequential at constant depth"),
        "rounds must run sequentially at constant depth so a long loop never \
         walks toward the recursion ceiling"
    );
}

// ── Resumability ─────────────────────────────────────────────────────────────

#[test]
fn a_dead_run_resumes_without_redoing_merged_work_or_reopening_concerns() {
    let state = read(&tool_path("autodrive_state.sh"));
    assert!(
        state.contains("autodrive_pr_state"),
        "the platform must be the authority on whether a PR is merged"
    );
    assert!(
        state.contains("printf 'UNKNOWN\\n'"),
        "an unreadable platform state must be UNKNOWN, never assumed not-merged"
    );
    assert!(
        state.contains("autodrive_record_resolved")
            && state.contains("autodrive_resolved_concerns"),
        "resolved concern ids must be recorded so a resumed run does not reopen them"
    );
    assert!(
        state.contains("AUTODRIVE_LEDGER_MARKER"),
        "the durable ledger must be findable by a stable marker"
    );
    for phase in [
        "autodrive-build",
        "autodrive-crusty-loop",
        "autodrive-merge-loop",
    ] {
        let text = recipe_text(phase);
        assert!(
            text.contains("autodrive_state.sh"),
            "{phase} must consult the durable state store"
        );
        assert!(
            text.contains("should_run"),
            "{phase} must be able to decide it has nothing left to do"
        );
    }
    let build = recipe_text("autodrive-build");
    assert!(
        build.contains("already merged; merged work is never rebuilt"),
        "phase 1 must not rebuild merged work"
    );
    assert!(
        build.contains("does not rebuild it"),
        "phase 1 must be a no-op when an open PR already exists"
    );
    let crusty = recipe_text("autodrive-crusty-round");
    assert!(
        crusty.contains("autodrive_resolved_concerns_file"),
        "the crusty round must be told which concerns a previous run resolved"
    );
    assert!(
        crusty.contains("only if you have NEW evidence"),
        "crusty may re-raise a resolved concern only with new evidence"
    );
}

// ── Skill + registration ─────────────────────────────────────────────────────

#[test]
fn skill_is_discoverable_and_states_the_contract() {
    let bundled = workspace_root().join("amplifier-bundle/skills/auto-drive-to-merge/SKILL.md");
    let mirror = workspace_root().join("docs/claude/skills/auto-drive-to-merge/SKILL.md");
    assert!(bundled.is_file(), "missing {}", bundled.display());
    assert!(mirror.is_file(), "missing {}", mirror.display());
    assert_eq!(
        read(&bundled),
        read(&mirror),
        "the two SKILL.md mirrors must stay byte-identical"
    );

    let text = read(&bundled);
    let front = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(front, _)| front)
        .expect("SKILL.md must open with YAML frontmatter at byte 0");
    let front: Value = serde_yaml::from_str(front).expect("frontmatter must parse");
    assert_eq!(
        front.get("name").and_then(Value::as_str),
        Some("auto-drive-to-merge"),
        "the skill name must match its directory so it is invocable by name"
    );
    assert!(
        matches!(front.get("description"), Some(Value::String(_))),
        "`description` must be a string scalar (issue #890)"
    );
    assert_eq!(
        front.get("user-invocable").and_then(Value::as_bool),
        Some(true),
        "the skill must be invocable, like dev-orchestrator"
    );

    for required in [
        "no iteration cap",
        "loop-health-evaluator",
        "crusty-old-engineer",
        "merge-ready",
        "qa-team",
        "Two absolute prohibitions",
        "No silent merge",
        "Exit code 79 is terminal",
        "Resumability",
    ] {
        assert!(
            text.contains(required),
            "the skill must document `{required}`"
        );
    }

    // The crusty structured verdict contract, added without breaking the
    // skill's standalone use.
    let crusty =
        read(&workspace_root().join("amplifier-bundle/skills/crusty-old-engineer/SKILL.md"));
    assert!(
        crusty.contains("crusty_verdict"),
        "crusty must define the structured verdict this workflow consumes"
    );
    assert!(
        crusty.contains("only when the caller explicitly asks for it"),
        "the structured block must be opt-in so standalone use is unchanged"
    );
    assert!(
        crusty.contains("never as `CLEAN`"),
        "an unreadable crusty verdict must fail safe to CONCERNS"
    );
    assert_eq!(
        crusty,
        read(&workspace_root().join("docs/claude/skills/crusty-old-engineer/SKILL.md")),
        "the crusty SKILL.md mirrors must stay byte-identical"
    );
}

#[test]
fn autodrive_recipes_are_registered_in_the_recipe_manifest() {
    let path = workspace_root().join("amplifier-bundle/recipes/_recipe_manifest.json");
    let raw = read(&path);
    let manifest: serde_json::Value =
        serde_json::from_str(&raw).expect("_recipe_manifest.json must be valid JSON");
    let object = manifest.as_object().expect("manifest must be an object");
    for name in AUTODRIVE_RECIPES {
        let entry = object
            .get(name)
            .unwrap_or_else(|| panic!("{name} must be registered in _recipe_manifest.json"));
        assert!(
            matches!(entry, serde_json::Value::String(h) if !h.trim().is_empty()),
            "{name}'s manifest entry must be a non-empty hash string"
        );
    }
}

#[test]
fn reference_doc_exists_and_declares_the_1347_dependency() {
    let doc = workspace_root().join("docs/reference/auto-drive-to-merge.md");
    let text = read(&doc);
    assert!(
        text.contains("#1347"),
        "the reference must declare the loop-health-evaluator dependency on PR #1347"
    );
    assert!(
        text.contains("not** reimplemented or copied"),
        "the reference must say the loop-health contract is used, never copied"
    );
    for section in [
        "Why there is no iteration cap",
        "Structured verdicts",
        "Two absolute prohibitions",
        "No silent merge",
        "Exit code 79 is terminal",
        "Resumability",
        "No short timeouts",
    ] {
        assert!(
            text.contains(section),
            "the reference must cover `{section}`"
        );
    }
}

// ── The executable contract ──────────────────────────────────────────────────

/// Runs the executable contract test — the STUCK path, the malformed-verdict
/// path, and the forbidden-flag guard, exercised against the real extracted
/// step bodies and the real tools.
///
/// Wired here because `.github/workflows/ci.yml` lists the bash recipe tests
/// one by one; running it from `cargo test` gets the same coverage without
/// touching that file.
#[test]
fn auto_drive_contract_shell_test_passes() {
    let root = workspace_root();
    let script = root.join("amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh");
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
        .expect("run the auto-drive contract shell test");

    assert!(
        out.status.success(),
        "test-auto-drive-to-merge.sh failed ({:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
