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

const AUTODRIVE_TOOLS: [&str; 5] = [
    "autodrive_loop.sh",
    "autodrive_merge_gate.sh",
    "autodrive_merge_ready_files.sh",
    "autodrive_state.sh",
    "autodrive_trust.sh",
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
    // Tools are bricks too (issue #1517): the range and qa evidence checks
    // live in autodrive_trust.sh so that no tool passes the budget.
    for tool in AUTODRIVE_TOOLS {
        let path = tool_path(tool);
        assert!(path.is_file(), "missing tool {tool}");
        let lines = read(&path).lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "{tool} is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
        );
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
    // Every way a shell can compare a counter to a limit, not just the three
    // `test` operators that happen to read as "at least". `-lt` / `-eq` bound
    // a loop just as well from the other side, `(( ROUND > n ))` and
    // `[[ ${ROUND} -gt n ]]` are the same cap in different syntax, and
    // `${ROUND}` is the same variable as `$ROUND`.
    const COMPARISONS: [&str; 12] = [
        "-ge", "-gt", "-le", "-lt", "-eq", "-ne", ">=", "<=", "==", "!=", ">", "<",
    ];
    for (n, line) in driver.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let names_round = line.contains("$ROUND")
            || line.contains("${ROUND}")
            || line.contains("((ROUND")
            || line.contains("(( ROUND");
        if !names_round {
            continue;
        }
        for op in COMPARISONS {
            assert!(
                !line.contains(op),
                "autodrive_loop.sh:{} compares the round label against a limit with `{op}`. \
                 ROUND is a LABEL; the terminator is loop-health-evaluator.\n  {line}",
                n + 1
            );
        }
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

/// Every prohibited construct, in every spelling that actually works.
///
/// A substring list of `--no-verify` / `--admin` / `--bypass` is not the
/// prohibition — it is three of its spellings. Hooks are equally skipped by
/// `git commit -nm "x"`, `git commit -m "x" -n`, `git -C . commit -n`,
/// `git -c core.hooksPath=/dev/null commit`, and `HUSKY=0 git commit`; the
/// merge gate is equally bypassed by `gh api -X PUT .../merge` and by
/// `gh pr merge --auto`, neither of which contains `--admin`.
///
/// Returns the label of every construct the line matches.
fn prohibited_constructs(line: &str) -> Vec<&'static str> {
    let lower = line.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .filter(|t| !t.is_empty())
        .collect();
    let has = |t: &str| tokens.contains(&t);
    let mut hits = Vec::new();

    if lower.contains("--no-verify") {
        hits.push("--no-verify");
    }
    // A single-dash cluster containing `n` anywhere in the argv of a git
    // invocation whose SUBCOMMAND is `commit`: `-n`, `-nm`, `-mn`, `-an`.
    // Resolving the subcommand matters — `git rev-parse "${BASE}^{commit}"` on
    // a line that also runs `[ -n "$BASE" ]` is not a hook-skipping commit.
    if let Some(after_git) = tokens.iter().position(|t| *t == "git") {
        let mut i = after_git + 1;
        while i < tokens.len() && tokens[i].starts_with('-') {
            // `git -C <dir>` and `git -c <k=v>` each consume a value (the
            // line is lower-cased, so both spell `-c` here).
            i += if tokens[i] == "-c" { 2 } else { 1 };
        }
        if tokens.get(i) == Some(&"commit")
            && tokens[i + 1..].iter().any(|t| {
                t.len() >= 2
                    && t.starts_with('-')
                    && !t.starts_with("--")
                    && t[1..].chars().all(|c| c.is_ascii_alphabetic())
                    && t.contains('n')
            })
        {
            hits.push("a short hook-skipping commit flag (-n / -nm / -mn)");
        }
    }
    if lower.contains("core.hookspath") {
        hits.push("core.hooksPath");
    }
    for env in [
        "husky",
        "skip_hooks",
        "no_verify",
        "pre_commit_allow_no_config",
    ] {
        if lower.contains(&format!("{env}=")) {
            hits.push("a hook-skipping environment variable");
            break;
        }
    }
    if lower.contains("--admin") {
        hits.push("--admin");
    }
    if lower.contains("--bypass") {
        hits.push("--bypass");
    }
    // `gh api ... /merge` merges outside the gate's fixed argv entirely.
    if lower.contains("gh api") && lower.contains("/merge") {
        hits.push("gh api .../merge");
    }
    // Auto-merge hands the decision to the platform, unverified.
    if lower.contains("pr merge") && has("--auto") {
        hits.push("gh pr merge --auto");
    }
    hits
}

#[test]
fn forbidden_flags_never_appear_in_an_executable_position() {
    // Never a hook-skipping commit flag; never a branch-protection bypass;
    // never a merge that goes around the gate. A line may NAME one only while
    // marking it as prohibited, and no executable line may name one at all. If
    // a hook or a check fails, the cause is fixed.
    let markers = ["never", "forbidden", "prohibit"];

    let mut scanned = control_path_files();
    scanned.push(workspace_root().join("amplifier-bundle/skills/auto-drive-to-merge/SKILL.md"));
    scanned.push(workspace_root().join("docs/claude/skills/auto-drive-to-merge/SKILL.md"));
    scanned.push(workspace_root().join("docs/reference/auto-drive-to-merge.md"));

    for path in &scanned {
        let text = read(path);
        for (n, line) in text.lines().enumerate() {
            let hits = prohibited_constructs(line);
            if hits.is_empty() {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            assert!(
                markers.iter().any(|m| lower.contains(m)),
                "{}:{} names `{}` without marking it prohibited:\n  {line}",
                path.display(),
                n + 1,
                hits.join("`, `")
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
            let hits = prohibited_constructs(line);
            assert!(
                hits.is_empty(),
                "{}:{} uses `{}` in an executable position:\n  {line}",
                path.display(),
                n + 1,
                hits.join("`, `")
            );
        }
    }
    for name in AUTODRIVE_RECIPES {
        for (id, body) in command_bodies(name) {
            for line in body.lines() {
                if line.trim_start().starts_with('#') {
                    continue;
                }
                let hits = prohibited_constructs(line);
                assert!(
                    hits.is_empty(),
                    "{name}.yaml step `{id}` uses `{}` in an executable position:\n  {line}",
                    hits.join("`, `")
                );
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
    // The durable copy used to be a marked PR COMMENT, and that is now
    // forbidden — see `no_unauthenticated_input_reaches_control_flow`.
    assert!(
        !state.contains("AUTODRIVE_LEDGER_MARKER"),
        "the PR-comment ledger must stay deleted; it was an attacker-writable \
         input into the phase-completion decision"
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

#[test]
fn no_unauthenticated_input_reaches_control_flow() {
    // The resume ledger was a marked comment on the pull request — readable
    // and WRITABLE by anyone who can comment on it. `autodrive_ledger_pull`
    // awk-parsed that comment body straight into `phases.tsv` and
    // `resolved-concerns.txt`, and it ran precisely when the local store was
    // empty: a fresh host, the normal case for a fleet. A forged comment
    // carrying the marker and `phases:\ncrusty-loop\t<date>` made the phase-2
    // preflight decide the crusty loop had already completed — the loop was
    // skipped, the phase-completion step never ran, and nothing downstream
    // noticed. Phase 3 re-measures CI; nothing re-measures crusty's judgement.
    //
    // Local state plus platform truth (`gh pr view --json state`) cover what
    // the ledger was for, minus a fresh-host optimisation. A fresh host redoes
    // a phase; that is the cheaper mistake.
    let banned = [
        "autodrive_ledger_pull",
        "autodrive_ledger_push",
        "autodrive_ledger_comment_id",
        "AUTODRIVE_LEDGER_MARKER",
        "auto-drive-to-merge:ledger",
    ];
    for path in control_path_files() {
        let text = read(&path);
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue; // prose explaining the absence is not the thing
            }
            for b in banned {
                assert!(
                    !line.contains(b),
                    "{}:{} reintroduces the PR-comment ledger (`{b}`):\n  {line}",
                    path.display(),
                    n + 1
                );
            }
        }
        // No pull-request comment may be read into this workflow's state at
        // all, under any function name.
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            assert!(
                !(line.contains("issues/") && line.contains("/comments")),
                "{}:{} reads pull-request comments into the workflow's state; \
                 a comment is writable by anyone who can comment:\n  {line}",
                path.display(),
                n + 1
            );
        }
    }
    // What replaced it: local state written by this host, and the platform.
    let state = read(&tool_path("autodrive_state.sh"));
    assert!(
        state.contains("autodrive_pr_state") && state.contains("gh pr view"),
        "merged-ness must still come from the platform"
    );
    assert!(
        state.contains("NO PR-COMMENT LEDGER"),
        "autodrive_state.sh must record why the ledger is absent, so it is not \
         helpfully restored"
    );
}

#[test]
fn every_verdict_gate_selects_the_last_object_carrying_its_field() {
    // `extract-json` alone is first-parseable-object-wins AND prefers a
    // ```json fence over raw prose. A reviewer that restates its output
    // contract — normal behaviour — hands the parser that example instead of
    // its verdict, and for crusty a quoted `CLEAN` is an unearned advance
    // toward a merge with no second signal behind it. `--require-field NAME`
    // (issue #1337, PR #1347) collects every object in document order and
    // takes the LAST one carrying the field, which is what the prompts ask
    // for; when none carries it, it returns nothing so the blocking
    // `--default` applies.
    let mut offenders = Vec::new();
    for path in control_path_files() {
        let text = read(&path);
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if line.contains("orch helper extract-json") && !line.contains("--require-field") {
                offenders.push(format!("{}:{}  {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "every `extract-json` on the control path must pass `--require-field` so \
         a quoted example cannot be read as the verdict:\n{}",
        offenders.join("\n")
    );
    // The two gates that matter, named explicitly.
    let crusty = recipe_text("autodrive-crusty-round");
    assert!(
        crusty.contains("extract-json --require-field crusty_verdict"),
        "the crusty gate must select the LAST object carrying `crusty_verdict`"
    );
    let merge = recipe_text("autodrive-merge-round");
    assert!(
        merge.contains("extract-json --require-field merge_ready_verdict"),
        "the merge-ready gate must select the LAST object carrying \
         `merge_ready_verdict`"
    );
    // The verdict shapes the reviewer is shown must not be quotable back into
    // the parser as a verdict.
    for skill in [
        "amplifier-bundle/skills/crusty-old-engineer/SKILL.md",
        "docs/claude/skills/crusty-old-engineer/SKILL.md",
    ] {
        let text = read(&workspace_root().join(skill));
        assert!(
            !text.contains("```json"),
            "{skill} carries a fenced json example of its own verdict; a \
             reviewer restating it emits that example straight into the parser"
        );
    }
}

#[test]
fn the_forbidden_flag_scanner_catches_every_spelling() {
    // The guard is only worth what it detects. These all skip hooks or go
    // around the merge gate, and none of them contains `--no-verify`,
    // `--admin`, or `--bypass`.
    for line in [
        r#"git commit -nm "x""#,
        r#"git commit -m "x" -n"#,
        "git -C . commit -n",
        "git -c core.hooksPath=/dev/null commit -m x",
        "HUSKY=0 git commit -m x",
        "SKIP_HOOKS=1 git commit -m x",
        "gh api -X PUT repos/{owner}/{repo}/pulls/7/merge",
        "gh api --method PUT repos/o/r/pulls/7/merge",
        r#"gh pr merge "$PR" --auto --squash"#,
        "git commit --no-verify -m x",
        "gh pr merge 7 --admin",
        "gh api -X PUT repos/o/r/branches/main/protection --bypass",
    ] {
        assert!(
            !prohibited_constructs(line).is_empty(),
            "the forbidden-flag guard does not detect: {line}"
        );
    }
    // ...and it does not fire on the commands this workflow actually runs.
    for line in [
        r#"git commit -m "address crusty review: <what changed>""#,
        r#"git commit -m "clear merge-ready blockers: <what changed>""#,
        "git add -A",
        "git push",
        r#"MERGE_ARGV=(pr merge "$PR" --squash --delete-branch --match-head-commit "$HEAD_SHA")"#,
        "gh pr view \"$PR\" --json state,mergedAt",
        "amplihack hygiene artifact-guard --repo . --mode pre-commit",
    ] {
        assert!(
            prohibited_constructs(line).is_empty(),
            "the forbidden-flag guard false-positives on: {line}  ({:?})",
            prohibited_constructs(line)
        );
    }
}

// ── Skills that refuse model invocation (issue #1517) ───────────────────────
//
// A skill whose frontmatter sets `disable-model-invocation: true` is a command
// a person runs; Claude Code refuses it when an agent calls it. A recipe
// prompt that tells an agent to call one fails in every run. The merge round
// did exactly that with `merge-ready`, the agent reported the criteria as
// unverifiable, and no pull request could ever reach MERGE_READY.

/// Key/value pairs of a SKILL.md's frontmatter: the top-level `key: value`
/// lines between the first two `---` lines, with surrounding quotes removed.
/// `None` when the file has no complete frontmatter block.
fn skill_frontmatter(text: &str) -> Option<Vec<(String, String)>> {
    let mut lines = text.lines().skip_while(|l| l.trim() != "---");
    lines.next()?; // the opening `---`
    let mut pairs = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            return Some(pairs);
        }
        if line.starts_with(char::is_whitespace) || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.split(" #").next().unwrap_or("").trim();
            let value = ['"', '\'']
                .iter()
                .find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q)))
                .unwrap_or(value);
            pairs.push((key.trim().to_string(), value.to_string()));
        }
    }
    None
}

/// Every regular file under `root` that `keep` accepts. Symlinks are skipped,
/// so a link loop terminates and nothing outside `root` is read. A directory
/// or entry that cannot be read is an error, never a silent skip.
fn walk_files(root: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("{}: cannot list: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: cannot read entry: {e}", dir.display()))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: cannot stat: {e}", path.display()))?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() && keep(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn read_utf8(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: unreadable: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{}: not valid UTF-8", path.display()))
}

fn relative_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A skill an agent must not invoke: its name and its SKILL.md, for messages.
struct FlaggedSkill {
    name: String,
    skill_md: String,
}

/// Every skill under `skills_root` whose frontmatter sets
/// `disable-model-invocation: true` (any case, quoted or not). The name is
/// the frontmatter `name:`, falling back to the directory name.
fn flagged_skills(skills_root: &Path, base: &Path) -> Result<Vec<FlaggedSkill>, String> {
    let is_skill_md = |p: &Path| p.file_name().is_some_and(|n| n == "SKILL.md");
    let mut flagged = Vec::new();
    for path in walk_files(skills_root, &is_skill_md)? {
        let text = read_utf8(&path)?;
        let Some(front) = skill_frontmatter(&text) else {
            continue;
        };
        let get = |k: &str| {
            front
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        if !get("disable-model-invocation").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
            continue;
        }
        let name = get("name")
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                path.parent()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        flagged.push(FlaggedSkill {
            name,
            skill_md: relative_display(&path, base),
        });
    }
    Ok(flagged)
}

/// One message per `Skill(skill="<name>")` in `text` that names a flagged
/// skill. Matches either quote style, whitespace around `(`, `=` and `)`, and
/// a backslash before either quote (the escaped form a YAML double-quoted
/// string produces); the step id is the nearest earlier `- id:` line.
fn flagged_invocations(recipe: &str, text: &str, flagged: &[FlaggedSkill]) -> Vec<String> {
    let id_line = regex::Regex::new(r#"^\s*-\s*id:\s*["']?([^"'\s]+)"#).expect("id regex");
    let mut hits = Vec::new();
    for skill in flagged {
        let pattern = format!(
            r#"Skill\(\s*skill\s*=\s*\\?["']{}\\?["']\s*\)"#,
            regex::escape(&skill.name)
        );
        let call = regex::Regex::new(&pattern).expect("invocation regex");
        for m in call.find_iter(text) {
            let id = text[..m.start()]
                .lines()
                .rev()
                .find_map(|l| id_line.captures(l).map(|c| c[1].to_string()))
                .unwrap_or_else(|| "<none>".to_string());
            hits.push(format!(
                "{recipe}: step {id}: {} but {} sets disable-model-invocation: true",
                m.as_str(),
                skill.skill_md
            ));
        }
    }
    hits
}

/// Runs the detector over every `*.yaml` / `*.yml` under `recipes_root`.
fn scan_recipes_for_flagged_invocations(
    recipes_root: &Path,
    flagged: &[FlaggedSkill],
    base: &Path,
) -> Result<Vec<String>, String> {
    let is_yaml = |p: &Path| p.extension().is_some_and(|e| e == "yaml" || e == "yml");
    let mut hits = Vec::new();
    for path in walk_files(recipes_root, &is_yaml)? {
        let text = read_utf8(&path)?;
        hits.extend(flagged_invocations(
            &relative_display(&path, base),
            &text,
            flagged,
        ));
    }
    Ok(hits)
}

#[test]
fn no_recipe_invokes_a_skill_that_refuses_model_invocation() {
    let root = workspace_root();
    let flagged = flagged_skills(&root.join("amplifier-bundle/skills"), &root)
        .unwrap_or_else(|e| panic!("skill scan failed: {e}"));
    // The guard is vacuous if it finds nothing to guard. merge-ready keeps its
    // flag (issue #1517): only a person starts `/merge-ready`.
    assert!(
        flagged.iter().any(|s| s.name == "merge-ready"),
        "merge-ready must still set disable-model-invocation: true; found flagged: {:?}",
        flagged.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    let hits = scan_recipes_for_flagged_invocations(
        &root.join("amplifier-bundle/recipes"),
        &flagged,
        &root,
    )
    .unwrap_or_else(|e| panic!("recipe scan failed: {e}"));
    assert!(
        hits.is_empty(),
        "a recipe tells an agent to invoke a skill that refuses agents; the call \
         fails in every run. Read the skill's files instead:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_invocation_guard_flags_the_old_merge_round_prompt() {
    // The step-02 prompt as it stood on origin/main before the fix, and a
    // fixture skill carrying the flag. The guard must catch the original
    // defect, or it is not guarding anything.
    let tmp = tempfile::tempdir().expect("tempdir");
    let skill_dir = tmp.path().join("skills/merge-ready");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: merge-ready\ndescription: x\ndisable-model-invocation: true\n---\n\n# Merge Ready\n",
    )
    .unwrap();
    let flagged = flagged_skills(&tmp.path().join("skills"), tmp.path()).unwrap();
    assert_eq!(
        flagged.len(),
        1,
        "the fixture skill must be read as flagged"
    );

    let old_round = r#"
  - id: "step-01-platform-facts"
    type: "bash"
    command: |
      echo facts

  - id: "step-02-merge-ready-assessment"
    agent: "amplihack:core:reviewer"
    prompt: |
      # merge-ready assessment — PR #{{platform_facts.pr}} ({{autodrive_round_label}})

      **Invoke the skill:** activate `Skill(skill="merge-ready")` for PR
      #{{platform_facts.pr}} and apply its criteria.
"#;
    let hits = flagged_invocations(
        "amplifier-bundle/recipes/autodrive-merge-round.yaml",
        old_round,
        &flagged,
    );
    assert_eq!(hits.len(), 1, "expected exactly one hit, got {hits:?}");
    assert_eq!(
        hits[0],
        "amplifier-bundle/recipes/autodrive-merge-round.yaml: step step-02-merge-ready-assessment: \
         Skill(skill=\"merge-ready\") but skills/merge-ready/SKILL.md sets disable-model-invocation: true"
    );
}

#[test]
fn the_invocation_guard_matches_every_quoting_and_spacing() {
    let flagged = [FlaggedSkill {
        name: "refuser".to_string(),
        skill_md: "skills/refuser/SKILL.md".to_string(),
    }];
    let text = r#"
  - id: "single-quotes"
    prompt: Skill(skill='refuser')
  - id: spaced
    prompt: Skill( skill = "refuser" )
  - id: "other-skills"
    prompt: |
      Skill(skill="refuser-two") Skill(skill="other") Skill(skill=name)
      Skill(skill="refuser.") Skill(skill="xrefuser")
"#;
    let hits = flagged_invocations("r.yaml", text, &flagged);
    assert_eq!(
        hits,
        vec![
            "r.yaml: step single-quotes: Skill(skill='refuser') but skills/refuser/SKILL.md sets disable-model-invocation: true".to_string(),
            "r.yaml: step spaced: Skill( skill = \"refuser\" ) but skills/refuser/SKILL.md sets disable-model-invocation: true".to_string(),
        ],
        "the guard must match either quote style and inner whitespace, print the \
         call as written, and never match a different skill name"
    );
    // A recipe with no `- id:` above the call still reports, with a placeholder.
    let hits = flagged_invocations("r.yaml", "# Skill(skill=\"refuser\")\n", &flagged);
    assert_eq!(hits.len(), 1, "a call in a comment still counts: {hits:?}");
    assert!(hits[0].contains("step <none>:"), "{hits:?}");
}

#[test]
fn the_invocation_guard_matches_the_escaped_quote_form() {
    // A prompt written as a YAML double-quoted string carries `\"` around the
    // name. The agent still reads `Skill(skill="refuser")`, so the guard must
    // catch the escaped form too (docs/reference/auto-drive-to-merge.md).
    let flagged = [FlaggedSkill {
        name: "refuser".to_string(),
        skill_md: "skills/refuser/SKILL.md".to_string(),
    }];
    let text = r#"
  - id: "plain"
    prompt: "call Skill(skill=\"refuser\") now"
  - id: "single"
    prompt: 'call Skill(skill=\'refuser\') now'
  - id: "spaced-escaped"
    prompt: "Skill( skill = \"refuser\" )"
  - id: "not-this-one"
    prompt: "Skill(skill=\"refuser-two\") Skill(skill=\"xrefuser\")"
"#;
    let hits = flagged_invocations("r.yaml", text, &flagged);
    assert_eq!(
        hits,
        vec![
            "r.yaml: step plain: Skill(skill=\\\"refuser\\\") but skills/refuser/SKILL.md sets disable-model-invocation: true".to_string(),
            "r.yaml: step single: Skill(skill=\\'refuser\\') but skills/refuser/SKILL.md sets disable-model-invocation: true".to_string(),
            "r.yaml: step spaced-escaped: Skill( skill = \\\"refuser\\\" ) but skills/refuser/SKILL.md sets disable-model-invocation: true".to_string(),
        ],
        "the guard must match a backslash before either quote and still never match \
         a different skill name"
    );
}

#[test]
fn the_invocation_guard_scans_yml_files_in_subdirectories() {
    // The recipe walk covers `*.yml` as well as `*.yaml`, in every
    // subdirectory. A call in a nested `bad.yml` must be reported.
    let tmp = tempfile::tempdir().expect("tempdir");
    let nested = tmp.path().join("recipes/sub/deeper");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("bad.yml"),
        "steps:\n  - id: \"x\"\n    prompt: |\n      Skill(skill=\"refuser\")\n",
    )
    .unwrap();
    fs::write(tmp.path().join("recipes/ok.yaml"), "name: ok\n").unwrap();
    fs::write(
        tmp.path().join("recipes/notes.txt"),
        "Skill(skill=\"refuser\")\n",
    )
    .unwrap();
    let flagged = [FlaggedSkill {
        name: "refuser".to_string(),
        skill_md: "skills/refuser/SKILL.md".to_string(),
    }];
    let hits =
        scan_recipes_for_flagged_invocations(&tmp.path().join("recipes"), &flagged, tmp.path())
            .expect("scan must succeed");
    assert_eq!(
        hits,
        vec![
            "recipes/sub/deeper/bad.yml: step x: Skill(skill=\"refuser\") but skills/refuser/SKILL.md sets disable-model-invocation: true"
                .to_string()
        ],
        "a nested *.yml recipe must be scanned; a non-recipe file must not"
    );
}

#[test]
fn qa_team_does_not_refuse_model_invocation() {
    // Step-04 of the merge round calls `Skill(skill="qa-team")` to write
    // missing gadugi scenarios. If qa-team ever starts refusing agents, that
    // call fails in every round the way merge-ready's did (issue #1517).
    let path = workspace_root().join("amplifier-bundle/skills/qa-team/SKILL.md");
    let text = read(&path);
    let front = skill_frontmatter(&text).expect("qa-team SKILL.md must have frontmatter");
    let refuses = front
        .iter()
        .any(|(k, v)| k == "disable-model-invocation" && v.eq_ignore_ascii_case("true"));
    assert!(
        !refuses,
        "qa-team sets disable-model-invocation: true, but autodrive-merge-round.yaml \
         step-04 tells an agent to call Skill(skill=\"qa-team\"); that call would be \
         refused in every round"
    );
    let fix_recipe = recipe_yaml("autodrive-merge-round");
    let fix = field(step(&fix_recipe, "step-04-address-blockers"), "prompt");
    assert!(
        fix.contains(r#"Skill(skill="qa-team")"#),
        "step-04 must still call qa-team by Skill(skill=\"qa-team\"); this guard is \
         only meaningful while it does"
    );
}

#[cfg(unix)]
#[test]
fn the_invocation_guard_walk_skips_symlinks_and_reads_frontmatter_variants() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().expect("tempdir");
    let skills = tmp.path().join("skills");
    let put = |rel: &str, body: &str| {
        let p = skills.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    };
    put(
        "a/SKILL.md",
        "---\nname: alpha\ndisable-model-invocation: true\n---\n",
    );
    // Quoted, mixed case, no `name:` -> falls back to the directory name.
    put(
        "b/SKILL.md",
        "---\ndescription: x\ndisable-model-invocation: \"True\"\n---\n",
    );
    // Nested a level deeper, single-quoted.
    put(
        "group/f/SKILL.md",
        "---\nname: 'f-skill'\ndisable-model-invocation: 'TRUE'\n---\n",
    );
    // Not flagged.
    put(
        "d/SKILL.md",
        "---\nname: delta\ndisable-model-invocation: false\n---\n",
    );
    put(
        "e/SKILL.md",
        "---\nname: echo\n---\n\ndisable-model-invocation: true\n",
    );
    put(
        "g/SKILL.md",
        "no frontmatter\ndisable-model-invocation: true\n",
    );
    // A directory symlink back to the root (a loop) and a symlinked SKILL.md.
    symlink(&skills, skills.join("a/loop")).unwrap();
    fs::create_dir_all(skills.join("c")).unwrap();
    symlink(skills.join("a/SKILL.md"), skills.join("c/SKILL.md")).unwrap();

    let flagged = flagged_skills(&skills, tmp.path()).expect("walk must succeed");
    let mut names: Vec<&str> = flagged.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["alpha", "b", "f-skill"],
        "the walk must terminate on a symlink loop, skip symlinked files, read \
         quoted and mixed-case flags, and only read the frontmatter block"
    );
    let alpha = flagged.iter().find(|s| s.name == "alpha").unwrap();
    assert_eq!(alpha.skill_md, "skills/a/SKILL.md");
}

#[test]
fn the_invocation_guard_fails_on_a_non_utf8_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bad_skill = tmp.path().join("skills/bad/SKILL.md");
    fs::create_dir_all(bad_skill.parent().unwrap()).unwrap();
    fs::write(&bad_skill, [0xff, 0xfe, b'-', b'-', b'-']).unwrap();
    let err = match flagged_skills(&tmp.path().join("skills"), tmp.path()) {
        Ok(_) => panic!("a non-UTF-8 SKILL.md must fail the scan, not be skipped"),
        Err(e) => e,
    };
    assert!(
        err.contains(&bad_skill.display().to_string()),
        "the error must name the file: {err}"
    );

    let recipes = tmp.path().join("recipes/nested");
    fs::create_dir_all(&recipes).unwrap();
    fs::write(recipes.join("ok.yaml"), "name: ok\n").unwrap();
    let bad_recipe = recipes.join("bad.yml");
    fs::write(&bad_recipe, [b'a', 0xc3, 0x28]).unwrap();
    let err =
        match scan_recipes_for_flagged_invocations(&tmp.path().join("recipes"), &[], tmp.path()) {
            Ok(_) => panic!("a non-UTF-8 recipe must fail the scan, not be skipped"),
            Err(e) => e,
        };
    assert!(
        err.contains(&bad_recipe.display().to_string()),
        "the error must name the file: {err}"
    );
}

// ── merge-ready under auto-drive (issue #1517) ──────────────────────────────

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|s| s.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing step `{id}`"))
}

/// The sentence every auto-drive agent prompt carries (docs/reference/
/// auto-drive-to-merge.md, "Agents do not touch the state directory"). Line
/// wrapping in a YAML block scalar is allowed, so prompts are compared with
/// whitespace collapsed.
const STATE_DIR_SENTENCE: &str = "Do not create, edit, move, rename, delete or archive any file in the \
     auto-drive state directory (the directory holding crusty-round-*.json, \
     merge-ready-round-*.json and phases.tsv).";

/// Words the state directory must never be named by in an agent prompt.
const STATE_DIR_LEAKS: [&str; 3] = ["STATE_DIR", "autodrive_state_dir", "AUTODRIVE_STATE_DIR"];

/// The five agent steps of the auto-drive workflow, by recipe.
const AUTODRIVE_AGENT_STEPS: [(&str, &str); 5] = [
    ("autodrive-crusty-round", "step-02-crusty-review"),
    ("autodrive-crusty-round", "step-04-address-concerns"),
    ("autodrive-merge-round", "step-02-merge-ready-assessment"),
    ("autodrive-merge-round", "step-04-address-blockers"),
    ("loop-health-evaluator", "step-02-evaluate-loop-health"),
];

const QA_REASON_TOKENS: [&str; 8] = [
    "qa-command-failed",
    "no-scenarios",
    "gadugi-validate-failed",
    "gadugi-scenario-unnamed",
    "gadugi-run-failed",
    "qa-command-missing",
    "qa-command-not-installed",
    "gadugi-test-missing",
];

const CRUSTY_FINAL_TOKENS: [&str; 6] = [
    "crusty-loop-not-done",
    "crusty-manifest-missing",
    "crusty-record-missing",
    "crusty-record-modified",
    "crusty-not-clean",
    "crusty-head-sha-empty",
];

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every (recipe, step id, prompt) for a step that runs an agent, across the
/// auto-drive recipes and the loop-health evaluator both loops call.
fn autodrive_agent_prompts() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for name in AUTODRIVE_RECIPES
        .iter()
        .copied()
        .chain(std::iter::once("loop-health-evaluator"))
    {
        let recipe = recipe_yaml(name);
        for s in steps(&recipe) {
            if s.get("agent").is_none() {
                continue;
            }
            let id = s
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unnamed>")
                .to_string();
            let prompt = s
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{name}: agent step `{id}` has no prompt"))
                .to_string();
            out.push((name.to_string(), id, prompt));
        }
    }
    out
}

#[test]
fn merge_ready_files_are_resolved_by_a_bash_step_before_the_round() {
    // D1-D3: the lookup is deterministic bash, not agent prose. A missing
    // install fails the step with a named error instead of becoming a
    // NOT_MERGE_READY blocker that repeats every round until STUCK.
    let recipe = recipe_yaml("autodrive-merge-round");
    let s = step(&recipe, "step-00-merge-ready-files");
    assert_eq!(
        s.get("type").and_then(Value::as_str),
        Some("bash"),
        "step-00 must be a bash step"
    );
    assert_eq!(s.get("parse_json").and_then(Value::as_bool), Some(true));
    assert_eq!(
        s.get("output").and_then(Value::as_str),
        Some("merge_ready_files")
    );
    assert!(
        step_index(&recipe, "step-00-merge-ready-files") < step_index(&recipe, "merge-evidence"),
        "step-00 must run before the merge evidence, so a missing install costs no test run"
    );
    let cmd = field(s, "command");
    for needle in [
        "autodrive_merge_ready_files.sh",
        "AMPLIHACK_HOME",
        "REPO_PATH",
        "git rev-parse --show-toplevel",
        "/.copilot",
        "/.amplihack",
        "merge-ready-skill-files-not-found",
        "resolver autodrive_merge_ready_files.sh not found",
    ] {
        assert!(cmd.contains(needle), "step-00 must reference `{needle}`");
    }
    // The resolver is executed, never sourced or evaluated.
    assert!(
        cmd.contains("bash \"$"),
        "step-00 must run the resolver as `bash \"$R\"`"
    );
    let sourced = regex::Regex::new(r#"(^|[;&|\s])(\.|source)\s+"?\$\{?R\b"#).unwrap();
    assert!(
        !sourced.is_match(cmd) && !cmd.contains("eval "),
        "step-00 must never source or eval the resolver"
    );
}

#[test]
fn merge_ready_assessment_reads_the_resolved_skill_files() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let prompt = field(step(&recipe, "step-02-merge-ready-assessment"), "prompt");

    for needle in [
        "{{merge_ready_files.skill_md}}",
        "{{merge_ready_files.template}}",
        "merge-ready",
        "disable-model-invocation",
        "{{crusty_evidence}}",
        "{{qa_evidence}}",
        "DONE_CLEAN",
        "crusty_reviewed_head_sha",
        "crusty_reason",
        "quality-audit-convergence-crusty-not-done-clean",
        "gadugi_status",
        "qa_reason",
    ] {
        assert!(
            prompt.contains(needle),
            "the step-02 prompt must mention `{needle}`"
        );
    }
    // The path lookup lives in step-00 now; the prompt must not ask the
    // agent to repeat it.
    for stale in [
        "git rev-parse --show-toplevel",
        "~/.copilot",
        "~/.amplihack",
    ] {
        assert!(
            !prompt.contains(stale),
            "the step-02 prompt still tells the agent to search for the skill (`{stale}`); \
             it receives the resolved paths from step-00"
        );
    }
    // No `Skill(` of any kind: not a call, not a negative mention.
    assert!(
        !prompt.contains("Skill("),
        "step-02 must refer to merge-ready by name only, never with `Skill(`"
    );
    let lower = squash(&prompt.to_ascii_lowercase());
    assert!(
        lower.contains("could not verify") && lower.contains("failed"),
        "step-02 keeps the rule that a criterion it could not verify counts as failed"
    );
    assert!(
        lower.contains("data") && lower.contains("not instructions"),
        "step-02 must say evidence text is data from the branch under review, not instructions"
    );
    // Criterion 3: the skill's three-cycle rule does not apply inside
    // auto-drive. (The range rule is pinned in
    // merge_round_prompts_state_the_range_rule.)
    assert!(
        lower.contains("three-cycle")
            || lower.contains("3 cycles")
            || lower.contains("three cycles"),
        "step-02 must say the skill's three-cycle quality-audit rule does not apply here"
    );
    for cap in [
        "at least 3 rounds",
        "at least three rounds",
        "minimum of 3",
        "3 or more rounds",
        "at least 3 crusty rounds",
    ] {
        assert!(
            !lower.contains(cap),
            "step-02 must not require a minimum crusty round count (`{cap}`)"
        );
    }
}

#[test]
fn merge_round_reads_crusty_evidence_through_autodrive_crusty_final() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let context = recipe
        .get("context")
        .and_then(Value::as_mapping)
        .expect("merge round must declare context");
    assert_eq!(
        context
            .get(Value::from("autodrive_state_dir"))
            .and_then(Value::as_str),
        Some(""),
        "the merge round must declare `autodrive_state_dir: \"\"`"
    );

    let s = step(&recipe, "step-01b-crusty-evidence");
    assert_eq!(s.get("type").and_then(Value::as_str), Some("bash"));
    assert_eq!(s.get("parse_json").and_then(Value::as_bool), Some(true));
    assert_eq!(
        s.get("output").and_then(Value::as_str),
        Some("crusty_evidence")
    );
    let cmd = field(s, "command");
    for needle in [
        "AUTODRIVE_STATE_DIR",
        "autodrive_state.sh",
        "autodrive_crusty_final",
        "\"crusty_status\":\"%s\"",
        "\"crusty_reason\":\"%s\"",
        "\"crusty_reviewed_head_sha\":\"%s\"",
        "DONE_CLEAN",
        "ABSENT",
        "NOT_CLEAN",
        "UNTRUSTED",
        "crusty-other",
    ] {
        assert!(cmd.contains(needle), "step-01b must reference `{needle}`");
    }
    for token in CRUSTY_FINAL_TOKENS {
        assert!(
            cmd.contains(token),
            "step-01b must map the helper token `{token}` to a status"
        );
    }
    // No second, weaker path: the verdict is never read from
    // crusty-latest.json directly, which an agent could overwrite.
    assert!(
        !cmd.contains("crusty-latest.json"),
        "step-01b must not read crusty-latest.json itself; autodrive_crusty_final \
         checks it against the loop's manifest"
    );
    assert!(
        step_index(&recipe, "step-01b-crusty-evidence")
            < step_index(&recipe, "step-02-merge-ready-assessment"),
        "step-01b must run before the merge-ready assessment"
    );

    // The measured downgrade covers crusty, UNTRUSTED included.
    let verdict = field(
        step(&recipe, "step-03-extract-merge-ready-verdict"),
        "command",
    );
    for needle in [
        "CRUSTY_EVIDENCE",
        "crusty_status",
        "DONE_CLEAN",
        "UNTRUSTED",
    ] {
        assert!(
            verdict.contains(needle),
            "step-03 must downgrade on crusty evidence (`{needle}`)"
        );
    }
}

#[test]
fn every_autodrive_agent_prompt_forbids_touching_the_state_dir() {
    // Point 4 of #1517: a blocker-clearing agent wrote its own crusty records
    // into the state dir and archived the loop-written ones. Every agent is
    // told, in one fixed sentence, to leave that directory alone.
    let prompts = autodrive_agent_prompts();
    let mut found: Vec<(String, String)> = prompts
        .iter()
        .map(|(r, id, _)| (r.clone(), id.clone()))
        .collect();
    found.sort();
    let mut expected: Vec<(String, String)> = AUTODRIVE_AGENT_STEPS
        .iter()
        .map(|(r, id)| (r.to_string(), id.to_string()))
        .collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "the set of auto-drive agent steps changed; add the new step to \
         AUTODRIVE_AGENT_STEPS and give it the state-directory sentence"
    );
    assert!(prompts.len() >= 5, "expected at least five agent steps");

    let sentence = squash(STATE_DIR_SENTENCE);
    for (recipe, id, prompt) in &prompts {
        assert!(
            squash(prompt).contains(&sentence),
            "{recipe}: agent step `{id}` must contain, word for word:\n  {sentence}"
        );
        for leak in STATE_DIR_LEAKS {
            assert!(
                !prompt.contains(leak),
                "{recipe}: agent step `{id}` must never name the state dir (`{leak}`)"
            );
        }
    }
}

#[test]
fn merge_round_agent_prompts_never_touch_crusty_state() {
    // The gate reads phases.tsv, crusty-records.tsv and crusty-latest.json as
    // criterion-3 evidence. An agent that can find or rewrite them can forge
    // it. Every agent prompt in the workflow, not only the merge round's.
    for (recipe, id, prompt) in autodrive_agent_prompts() {
        for leak in STATE_DIR_LEAKS {
            assert!(
                !prompt.contains(leak),
                "{recipe}: agent step `{id}` must never be given the state dir (`{leak}`)"
            );
        }
    }
    let fix_recipe = recipe_yaml("autodrive-merge-round");
    let fix = field(step(&fix_recipe, "step-04-address-blockers"), "prompt");
    assert!(
        fix.contains("phases.tsv") && fix.contains("crusty-round-*.json"),
        "step-04 must forbid touching phases.tsv and the crusty round records by name"
    );
}

#[test]
fn merge_round_blocker_step_writes_missing_gadugi_scenarios() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let fix = field(step(&recipe, "step-04-address-blockers"), "prompt");
    for needle in [
        r#"Skill(skill="qa-team")"#,
        "gadugi_scenario_dir",
        "gadugi_status",
        "NO_SCENARIOS",
        "VALIDATE_FAILED",
        "RUN_FAILED",
        "gadugi-test validate -d",
        "--scenario",
        "#207",
        "gadugi-scenario-dir-outside-repo",
        "quality-audit-convergence-crusty-not-done-clean",
    ] {
        assert!(fix.contains(needle), "step-04 must mention `{needle}`");
    }
    let lower = squash(&fix.to_ascii_lowercase());
    assert!(
        lower.contains("every repository type"),
        "step-04 must say gadugi scenarios are required in every repository type, \
         overriding qa-team's cargo test substitution for Rust CLI repositories"
    );
    assert!(
        lower.contains("rust"),
        "step-04's override must name Rust CLI repositories explicitly"
    );
    assert!(
        lower.contains("failing scenario"),
        "step-04 must say a failing scenario is fixed, never deleted"
    );
    // One scenario per process (gadugi-agentic-test #207): every run names a
    // scenario, and none runs a whole directory.
    let runs: Vec<&str> = fix
        .lines()
        .filter(|l| l.contains("gadugi-test run"))
        .collect();
    assert!(
        !runs.is_empty(),
        "step-04 must tell the agent how to run scenarios"
    );
    for line in &runs {
        assert!(
            line.contains("--scenario"),
            "step-04 runs a whole directory in one gadugi-test process:\n  {line}"
        );
    }
    let validate = fix.find("gadugi-test validate -d").unwrap();
    let run = fix.find("gadugi-test run").unwrap();
    assert!(validate < run, "step-04 must validate before it runs");
}

#[test]
fn qa_evidence_runs_one_gadugi_process_per_scenario() {
    let recipe = recipe_yaml("autodrive-merge-evidence");
    let cmd = field(step(&recipe, "step-02-qa-team-scenarios"), "command");
    for needle in [
        "gadugi-test validate -d \"$",
        "--scenario \"$",
        "#207",
        "mktemp -d",
        "cp -P",
        "trap",
        "AUTODRIVE_QA_SCENARIO_DIR",
        "AUTODRIVE_QA_COMMAND",
        "AUTODRIVE_QA_COMMANDS",
        "AUTODRIVE_QA_DIR",
        "set -f",
        "bash -c \"$",
        "tests/agentic",
        "-maxdepth 1",
        "npm test",
        "cargo test --workspace --locked --no-fail-fast",
        "pytest",
    ] {
        assert!(
            cmd.contains(needle),
            "the qa evidence step must contain `{needle}`"
        );
    }
    // Every gadugi-test run line selects one scenario; the whole-directory
    // run of the first #1517 commit is gone.
    for line in cmd.lines().filter(|l| l.contains("gadugi-test run")) {
        if line.trim_start().starts_with('#') {
            continue;
        }
        assert!(
            line.contains("--scenario"),
            "the qa evidence step runs a whole directory in one process (#207):\n  {line}"
        );
    }
    assert!(
        !cmd.contains("gadugi-test run -d \"$SDIR\""),
        "the qa evidence step must never run the scenario directory itself"
    );
    let validate = cmd.find("gadugi-test validate -d").unwrap();
    let run = cmd
        .lines()
        .scan(0usize, |at, l| {
            let start = *at;
            *at += l.len() + 1;
            Some((start, l))
        })
        .find(|(_, l)| l.contains("gadugi-test run") && !l.trim_start().starts_with('#'))
        .map(|(at, _)| at)
        .expect("a gadugi-test run line");
    assert!(
        validate < run,
        "gadugi-test validate must run before any gadugi-test run"
    );
    for field_name in [
        "qa_status",
        "qa_reason",
        "qa_repo_type",
        "qa_command",
        "qa_suite_commands_count",
        "qa_exit_code",
        "head_sha",
        "gadugi_status",
        "gadugi_validate_exit_code",
        "gadugi_run_exit_code",
        "gadugi_scenario_count",
        "gadugi_scenario_dir",
        "gadugi_scenarios_validated",
        "gadugi_scenarios_run",
        "gadugi_scenarios_passed",
        "gadugi_scenarios_failed",
        "gadugi_failed_scenarios",
    ] {
        assert!(
            cmd.contains(&format!("\"{field_name}\":\"%s\"")),
            "the qa evidence JSON must record `{field_name}` as a string"
        );
    }
    for token in QA_REASON_TOKENS {
        assert!(
            cmd.contains(token),
            "the qa evidence step must be able to emit qa_reason `{token}`"
        );
    }
    for status in [
        "NOT_INSTALLED",
        "NO_SCENARIOS",
        "VALIDATE_FAILED",
        "RUN_FAILED",
    ] {
        assert!(
            cmd.contains(status),
            "gadugi_status must include `{status}`"
        );
    }
    // Commands from the environment are never concatenated into one script,
    // sourced, or evaluated.
    let eval = regex::Regex::new(r"(^|[;&|\s(])eval\s").unwrap();
    for line in cmd.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        assert!(
            !eval.is_match(line),
            "the qa evidence step must never eval a command:\n  {line}"
        );
    }
    assert!(
        !cmd.contains("--timeout"),
        "no timeout is passed to gadugi-test (issue #439)"
    );
    assert!(
        !cmd.contains("does NOT require the gadugi"),
        "the comment exempting Rust repos from gadugi must stay gone"
    );
}

#[test]
fn crusty_round_records_the_reviewed_head_sha_every_round() {
    // Point 5 of #1517: a CLEAN round with no commits wrote "head_sha":"", so
    // the clean verdict could not be tied to a commit.
    let recipe = recipe_yaml("autodrive-crusty-round");
    let ctx = field(step(&recipe, "step-01-round-context"), "command");
    assert!(
        ctx.contains("git rev-parse HEAD"),
        "step-01 must read the reviewed head with git, in bash"
    );
    let cmd = field(step(&recipe, "step-06-write-round-record"), "command");
    for needle in [
        "CRUSTY_ROUND_CONTEXT",
        "CRUSTY_FIX_EVIDENCE",
        "\"reviewed_head_sha\":\"%s\"",
        "\"head_sha\":\"%s\"",
        "crusty-head-sha-unavailable",
    ] {
        assert!(
            cmd.contains(needle),
            "crusty step-06 must reference `{needle}`"
        );
    }
    assert!(
        cmd.contains("printf '{\"crusty_verdict\":\"%s\","),
        "the crusty record must stay one printf line that starts with crusty_verdict"
    );
}

#[test]
fn crusty_records_are_trusted_only_through_the_loop_manifest() {
    // Point 4 of #1517: the assessment trusts only records the loop wrote.
    let driver = read(&tool_path("autodrive_loop.sh"));
    for needle in [
        "-records.tsv",
        "hash-object --no-filters --stdin",
        "env -u GIT_DIR -u GIT_WORK_TREE",
        "cmp",
        "umask 077",
    ] {
        assert!(
            driver.contains(needle),
            "autodrive_loop.sh must write the record manifest with `{needle}`"
        );
    }
    let latest = driver
        .find("-latest.json\"")
        .expect("the driver must still copy the round record to <loop>-latest.json");
    let manifest = driver
        .find("-records.tsv")
        .expect("the driver must append to <loop>-records.tsv");
    let health = driver
        .find("recipe run loop-health-evaluator")
        .expect("the driver must run loop-health-evaluator");
    assert!(
        latest < manifest && manifest < health,
        "the manifest row must be written after the -latest.json copy and before any \
         agent (the loop-health evaluator) runs"
    );

    let state = read(&tool_path("autodrive_state.sh"));
    assert!(
        state.contains("autodrive_crusty_final()"),
        "autodrive_state.sh must define autodrive_crusty_final"
    );
    for needle in [
        "crusty-records.tsv",
        "crusty-latest.json",
        "mktemp",
        "LC_ALL=C",
    ] {
        assert!(
            state.contains(needle),
            "autodrive_crusty_final must use `{needle}`"
        );
    }
    for token in CRUSTY_FINAL_TOKENS {
        assert!(
            state.contains(token),
            "autodrive_crusty_final must be able to return `{token}`"
        );
    }

    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    for needle in [
        "autodrive_crusty_final",
        "crusty-records.tsv",
        "are not loop-written evidence",
        "crusty_reviewed_head_sha=",
        "qa_reason",
    ] {
        assert!(
            gate.contains(needle),
            "the merge gate section 6/6b must use `{needle}`"
        );
    }
    let round_recipe = recipe_yaml("autodrive-merge-round");
    let round = field(step(&round_recipe, "step-01b-crusty-evidence"), "command");
    assert!(
        round.contains("autodrive_crusty_final"),
        "step-01b and the gate must share one criterion-3 check"
    );
}

#[test]
fn no_round_minimum_in_any_recipe_or_tool() {
    // D12: the crusty loop stops at its first CLEAN round, so a minimum round
    // count would block forever any PR that was clean in round 1 or 2. The
    // manual three-cycle rule lives in the merge-ready SKILL.md and the docs,
    // which this guard does not scan.
    let root = workspace_root();
    let keep = |p: &Path| {
        p.extension()
            .is_some_and(|e| e == "yaml" || e == "yml" || e == "sh")
    };
    let mut files = walk_files(&root.join("amplifier-bundle/recipes"), &keep)
        .unwrap_or_else(|e| panic!("recipe walk failed: {e}"));
    files.extend(
        walk_files(&root.join("amplifier-bundle/tools"), &|_| true)
            .unwrap_or_else(|e| panic!("tool walk failed: {e}")),
    );
    let minimum = regex::Regex::new(
        r"(?i)(at\s+least\s+(3|three)\s+(crusty\s+)?(rounds|cycles)|minimum\s+of\s+(3|three)\s+(crusty\s+)?rounds|(3|three)\s+or\s+more\s+(crusty\s+)?rounds|min_rounds|minimum_rounds|min_crusty_rounds)",
    )
    .unwrap();
    let mut hits = Vec::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue; // binary tools are not prose
        };
        for (n, line) in text.lines().enumerate() {
            if minimum.is_match(line) {
                hits.push(format!(
                    "{}:{}: {}",
                    relative_display(&path, &root),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a recipe or tool asks for a minimum number of crusty rounds:\n{}",
        hits.join("\n")
    );
}

#[test]
fn merge_ready_files_resolver_is_read_only_and_set_u_safe() {
    let path = tool_path("autodrive_merge_ready_files.sh");
    let text = read(&path);
    for needle in [
        "set -u",
        "${HOME:-}",
        "${REPO_PATH:-}",
        "${AMPLIHACK_HOME:-}",
        "pwd -P",
        "hash-object --no-filters --stdin",
        "merge-ready-template-not-found",
        "merge-ready-skill-files-not-found: searched",
        "INFO: merge-ready criteria from",
        "\"skill_dir\":\"%s\"",
        "\"skill_md\":\"%s\"",
        "\"template\":\"%s\"",
        "\"skill_md_sha\":\"%s\"",
        "skills/merge-ready",
    ] {
        assert!(
            text.contains(needle),
            "autodrive_merge_ready_files.sh must contain `{needle}`"
        );
    }
    // Read-only: no command that writes, moves or deletes, and no redirection
    // into a file. Redirection to stderr or /dev/null is fine.
    let command = regex::Regex::new(
        r"(^|[;&|(]\s*|\b(then|do|else)\s+)(rm|mv|cp|mkdir|touch|tee|ln|chmod|install)\b",
    )
    .unwrap();
    let redirect = regex::Regex::new(r#"(^|[^0-9&<>])>>?\s*["$~/A-Za-z]"#).unwrap();
    for (n, line) in text.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with('#') {
            continue;
        }
        let code = code
            .replace("2>&1", "")
            .replace(">&2", "")
            .replace("2>/dev/null", "")
            .replace(">/dev/null", "");
        assert!(
            !command.is_match(&code) && !redirect.is_match(&code),
            "autodrive_merge_ready_files.sh:{} writes to the filesystem; it must be read-only:\n  {line}",
            n + 1
        );
    }
}

#[test]
fn merge_loop_passes_the_state_dir_to_rounds() {
    let text = recipe_text("autodrive-merge-loop");
    assert!(
        text.contains(r#"--context "autodrive_state_dir=${DIR}""#),
        "the merge loop must pass its state dir to every round"
    );
    // The composer must not hand rounds the crusty loop's stdout: on a resumed
    // run the crusty loop is skipped and that result is empty.
    assert!(
        !recipe_text("autodrive-merge-round").contains("crusty_loop_result"),
        "crusty evidence comes from the state dir, never from crusty_loop_result"
    );
}

#[test]
fn no_recipe_declares_the_qa_overrides_as_context() {
    // The runner exports context keys as environment variables; an empty
    // context default would hide the value the user exported. All four
    // AUTODRIVE_QA_* overrides are read from the environment only.
    let root = workspace_root();
    let is_yaml = |p: &Path| p.extension().is_some_and(|e| e == "yaml" || e == "yml");
    let names = "autodrive_qa_(command|commands|dir|scenario_dir)";
    let decl = regex::Regex::new(&format!(r"(?i)^\s*{names}\s*:")).unwrap();
    let pass = regex::Regex::new(&format!(r"(?i)(-c|--context)\s+.?{names}=")).unwrap();
    for path in walk_files(&root.join("amplifier-bundle/recipes"), &is_yaml).unwrap() {
        let text = read_utf8(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            assert!(
                !decl.is_match(line) && !pass.is_match(line),
                "{}:{} declares an AUTODRIVE_QA_* override as a context key; it is read \
                 from the environment only:\n  {line}",
                path.display(),
                n + 1
            );
        }
    }
}

#[test]
fn merge_gate_requires_gadugi_and_crusty_evidence() {
    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    for needle in [
        "gadugi_status",
        "gadugi_scenario_count",
        "STATE_DIR_GIVEN",
        "autodrive_state.sh",
        "autodrive_phase_done",
        "crusty-loop",
        "crusty-latest.json",
        "crusty_verdict",
        "not private to this user",
    ] {
        assert!(
            gate.contains(needle),
            "the merge gate must check `{needle}`"
        );
    }
    // An empty --state-dir must not become the /tmp fallback before the gate
    // decides whether a state dir was given (R-S1.1).
    let given = gate
        .find("STATE_DIR_GIVEN=")
        .expect("STATE_DIR_GIVEN must be assigned");
    let fallback = gate
        .find(r#"STATE_DIR="${STATE_DIR:-${TMPDIR:-/tmp}}""#)
        .expect("the evidence-bundle fallback must remain");
    assert!(
        given < fallback,
        "STATE_DIR_GIVEN must be computed before the /tmp fallback is applied"
    );
    // The state helper comes from beside the gate, never from a search root
    // that a pull request could populate.
    assert!(
        !gate.contains(".copilot") && !gate.contains(".amplihack/amplifier-bundle"),
        "the gate must source autodrive_state.sh from its own directory only"
    );
}

#[test]
fn merge_gate_keeps_every_existing_block() {
    // AC8: the gate only gets stricter. Every refusal present before #1517
    // must still be there, word for word.
    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    for existing in [
        r#"block "pull request #${PR} metadata is unreadable; an unreadable platform state never merges""#,
        r#"block "pull request #${PR} is in state '${PR_STATE}', not OPEN""#,
        r#"block "head SHA for #${PR} is unreadable ('${HEAD_SHA}'); every criterion must bind to one SHA""#,
        r##"block "#${PR} is a draft""##,
        r#"block "mergeable='${MERGEABLE}' (CONFLICTING or UNKNOWN never merges)""#,
        r#"block "branch is BEHIND base; this repository requires strict up-to-date branches before merge""#,
        r#"block "mergeStateStatus='${MERGE_STATE}' is not a mergeable state""#,
        r#"block "a review requests changes""#,
        r#"block "review-thread state is unreadable; an unreadable criterion is a failure, not a pass""#,
        r#"block "${THREADS} unresolved review thread(s)""#,
        r#"block "jq is unavailable, so the CI rollup cannot be read; a criterion we cannot read is a failure""#,
        r#"block "CI status for #${PR} is unreadable; an unreadable CI status is a failure, not a pass""#,
        r#"block "no CI checks reported for #${PR}; zero checks is not a green build""#,
        r#"block "${PENDING:-unreadable} CI check(s) still pending""#,
        r#"block "${FAILING:-unreadable} CI check(s) failing or cancelled""#,
        r#"block "no qa-team scenario evidence file was produced in this run""#,
        r#"block "qa-team scenarios did not pass in this run (qa_status=${QA_STATUS})""#,
        r#"block "the qa-team evidence records no head_sha; evidence that is not bound to a SHA never merges""#,
        r#"block "the qa-team evidence was captured against ${QA_SHA} but the head is now ${HEAD_SHA}; evidence must bind to the SHA being merged""#,
        r#"block "no merge-ready round record was produced in this run""#,
        r#"block "merge-ready verdict is '${MR_VERDICT}', not MERGE_READY""#,
        r#"block "the merge-ready evidence was captured against ${MR_SHA:-<none>} but the head is now ${HEAD_SHA}; evidence must bind to the SHA being merged""#,
        // Added by the first #1517 commit (7b79aa04); the manifest checks
        // only add to these.
        r#"block "gadugi-test scenarios were not validated and run to a pass in this run (gadugi_status=${GADUGI_STATUS})""#,
        r#"block "gadugi_scenario_count='${GADUGI_COUNT}' is not a positive integer; zero scenarios is not a qa-team pass""#,
        r#"block "no --state-dir was given, so the crusty loop's DONE/CLEAN state cannot be read; crusty evidence is never taken from the TMPDIR fallback""#,
        r#"block "the crusty-loop phase is not recorded as done in ${STATE_DIR}; criterion 3 needs this run's crusty loop to have ended DONE""#,
        r#"block "the crusty loop's final crusty_verdict is not CLEAN in ${STATE_DIR}/crusty-latest.json; criterion 3 is not met""#,
        // Added by the manifest commits of #1517; the range and qa evidence
        // checks only add to these.
        r#"block "crusty records in ${STATE_DIR} are not loop-written evidence (${CRUSTY_FINAL}); criterion 3 is not met""#,
        r#"block "state dir ${STATE_DIR} is not private to this user (not owned by this user, a symlink, or group/world-writable); crusty state there is not evidence""#,
        r#"block "autodrive_state.sh is missing beside the merge gate (${GATE_HOME:-<unknown>}); the crusty-loop marker cannot be read""#,
    ] {
        assert!(
            gate.contains(existing),
            "an existing merge-gate refusal was changed or removed:\n  {existing}"
        );
    }
}

#[test]
fn state_helper_documents_the_crusty_marker_as_gate_evidence() {
    let state = read(&tool_path("autodrive_state.sh"));
    assert!(
        !state.contains("a resume optimisation, never evidence"),
        "autodrive_state.sh still says the phase marker is never evidence; the \
         gate now reads the crusty-loop marker as criterion-3 evidence"
    );
    for needle in [
        "crusty-latest.json",
        "crusty-records.tsv",
        "autodrive-crusty-loop.yaml",
        "autodrive_loop.sh",
    ] {
        assert!(
            state.contains(needle),
            "autodrive_state.sh must name `{needle}` where it explains the marker"
        );
    }
    // The header names the manifest among the files only the loop writes.
    let header_end = state
        .find("autodrive_mark_phase_done()")
        .expect("autodrive_mark_phase_done must be defined");
    assert!(
        state[..header_end].contains("crusty-records.tsv"),
        "the phase-completion comment must name crusty-records.tsv as a file only the loop writes"
    );
}

#[test]
fn merge_ready_skill_documents_running_under_auto_drive() {
    let path = workspace_root().join("amplifier-bundle/skills/merge-ready/SKILL.md");
    let text = read(&path);
    let front = skill_frontmatter(&text).expect("merge-ready must have frontmatter");
    assert!(
        front
            .iter()
            .any(|(k, v)| k == "disable-model-invocation" && v == "true"),
        "merge-ready keeps disable-model-invocation: true (issue #1517, D1)"
    );
    let required = text.find("## Required outcome").expect("Required outcome");
    let section = text
        .find("## Running under auto-drive")
        .expect("merge-ready must have a `## Running under auto-drive` section");
    let stop = text.find("## When to stop").expect("When to stop section");
    assert!(
        required < section && section < stop,
        "`## Running under auto-drive` goes after Required outcome and before When to stop"
    );
    let body = &text[section..stop];
    for needle in [
        "gadugi-test validate",
        "gadugi-test run",
        "--scenario",
        "AUTODRIVE_QA_SCENARIO_DIR",
        "AUTODRIVE_QA_COMMAND",
        "AUTODRIVE_QA_COMMANDS",
        "AUTODRIVE_QA_DIR",
        "crusty-old-engineer",
        "CLEAN",
        "reviewed_head_sha",
        "quality-audit",
        "at least 3",
        "merge-ready-skill-files-not-found",
    ] {
        assert!(
            body.contains(needle),
            "the auto-drive section must mention `{needle}`"
        );
    }
    let reference = read(&workspace_root().join("docs/reference/auto-drive-to-merge.md"));
    for needle in [
        "AUTODRIVE_QA_SCENARIO_DIR",
        "AUTODRIVE_QA_COMMAND",
        "AUTODRIVE_QA_COMMANDS",
        "AUTODRIVE_QA_DIR",
        "gadugi_status",
        "qa_reason",
        "gadugi-agentic-test #207",
        "crusty_status",
        "DONE_CLEAN",
        "UNTRUSTED",
        "autodrive_crusty_final",
        "crusty-records.tsv",
        "reviewed_head_sha",
        "step-00-merge-ready-files",
        "autodrive_merge_ready_files.sh",
        "no_recipe_invokes_a_skill_that_refuses_model_invocation",
        "no_round_minimum_in_any_recipe_or_tool",
        "qa_team_does_not_refuse_model_invocation",
    ] {
        assert!(
            reference.contains(needle),
            "docs/reference/auto-drive-to-merge.md must document `{needle}`"
        );
    }
    for token in QA_REASON_TOKENS.iter().chain(CRUSTY_FINAL_TOKENS.iter()) {
        assert!(
            reference.contains(&format!("`{token}`")),
            "docs/reference/auto-drive-to-merge.md must document the token `{token}`"
        );
    }
    // The sentence is quoted as a Markdown blockquote; drop the `>` markers.
    let unquoted = reference
        .lines()
        .map(|l| l.trim_start().strip_prefix('>').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        squash(&unquoted).contains(&squash(STATE_DIR_SENTENCE)),
        "the reference must quote the state-directory sentence word for word"
    );
    for (recipe, id) in AUTODRIVE_AGENT_STEPS {
        assert!(
            reference.contains(&format!("`{id}`")) && reference.contains(recipe),
            "the reference must list the agent step `{recipe}` / `{id}`"
        );
    }
}

#[test]
fn every_criterion_is_read_completely_and_bound_to_the_merged_sha() {
    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    let round = recipe_text("autodrive-merge-round");

    // Review threads: `reviewThreads(first:100)` with no pageInfo follow-up
    // silently truncates. 101 threads with the last one unresolved reports 0
    // unresolved and passes the gate.
    for (label, text) in [("merge gate", &gate), ("merge round", &round)] {
        assert!(
            text.contains("reviewThreads"),
            "{label} must read review threads"
        );
        assert!(
            text.contains("--paginate") && text.contains("pageInfo"),
            "{label} reads reviewThreads without paging; a PR past the first \
             page would report 0 unresolved threads and pass the gate"
        );
    }

    // qa evidence: existence + qa_status=PASS is not enough. The gate binds
    // the round record to HEAD_SHA; the qa evidence must bind too, or a PASS
    // from an earlier round stands in for a tree that is no longer merged.
    let evidence = recipe_text("autodrive-merge-evidence");
    assert!(
        evidence.contains(r#""head_sha":"%s""#),
        "the qa evidence must record the head SHA it was measured on"
    );
    assert!(
        gate.contains("QA_SHA") && gate.contains("qa-team evidence was captured against"),
        "the merge gate must refuse qa evidence captured against another SHA"
    );
    assert!(
        gate.contains("records no head_sha"),
        "the merge gate must refuse qa evidence that is not bound to any SHA"
    );

    // `$?` inside the `then` of an `if ! cmd` is the NEGATION's status, always
    // 0 — which makes the exit-79 branch dead and reports "exit 0".
    assert!(
        !gate.contains("if ! gh \"${MERGE_ARGV[@]}\""),
        "the merge status must be captured from `gh` itself, not from inside \
         the `then` of an `if !`, where `$?` is always 0"
    );
    assert!(
        gate.contains("gh \"${MERGE_ARGV[@]}\"\nMERGE_RC=$?"),
        "`gh` must run on its own line with `MERGE_RC=$?` immediately after"
    );
}

#[test]
fn the_loop_refuses_before_round_one_when_its_terminator_is_missing() {
    // A missing `loop-health-evaluator` otherwise costs a full round — a
    // crusty review and a builder fix pass, with commits pushed — before the
    // loop dies with "returned STUCK (or an unreadable verdict)", which blames
    // the loop for a missing dependency.
    let driver = read(&tool_path("autodrive_loop.sh"));
    let preflight = driver
        .find("recipe show loop-health-evaluator")
        .expect("the driver must resolve loop-health-evaluator up front");
    let first_round = driver
        .find("recipe run \"$ROUND_RECIPE\"")
        .expect("the driver must run the round recipe");
    assert!(
        preflight < first_round,
        "the dependency check must run BEFORE the first round, not after it"
    );
    assert!(
        driver.contains("MISSING_DEPENDENCY"),
        "the refusal must name the missing dependency as the cause"
    );
    let while_loop = driver.find("while :;").expect("the driver must loop");
    assert!(
        preflight < while_loop,
        "the dependency check must sit outside the round loop so it costs one \
         resolution, not one per round"
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

// ── Commits after the clean round, and the qa evidence chain (#1517 D4-D6) ──

/// What `autodrive_crusty_range` prints, besides `ok`.
const RANGE_TOKENS: [&str; 2] = ["crusty-unreviewed-commits", "crusty-range-unreadable"];

/// What `autodrive_qa_trusted` prints, besides `ok`.
const QA_TRUSTED_TOKENS: [&str; 4] = [
    "qa-manifest-missing",
    "qa-record-modified",
    "qa-evidence-modified",
    "qa-evidence-stale",
];

/// The search order every auto-drive bash step uses to find a bundle tool.
const TOOL_SEARCH_NEEDLES: [&str; 5] = [
    "AMPLIHACK_HOME",
    "REPO_PATH",
    "git rev-parse --show-toplevel",
    "/.copilot",
    "/.amplihack",
];

fn step_command<'a>(recipe: &'a Value, id: &str) -> &'a str {
    field(step(recipe, id), "command")
}

#[test]
fn merge_round_rereviews_commits_after_the_clean_round() {
    // D4: a code commit after the clean crusty round goes back to crusty at
    // the start of the next merge round, through a nested crusty loop that
    // keeps its own termination logic.
    let recipe = recipe_yaml("autodrive-merge-round");
    let order = [
        "step-00-merge-ready-files",
        "step-00b-crusty-range",
        "step-00c-crusty-rereview",
        "merge-evidence",
        "step-00d-qa-evidence-hash",
        "step-01-platform-facts",
        "step-01b-crusty-evidence",
        "step-02-merge-ready-assessment",
    ];
    for pair in order.windows(2) {
        assert!(
            step_index(&recipe, pair[0]) < step_index(&recipe, pair[1]),
            "`{}` must run before `{}`",
            pair[0],
            pair[1]
        );
    }

    let s00b = step(&recipe, "step-00b-crusty-range");
    assert_eq!(s00b.get("type").and_then(Value::as_str), Some("bash"));
    assert_eq!(s00b.get("parse_json").and_then(Value::as_bool), Some(true));
    assert_eq!(
        s00b.get("output").and_then(Value::as_str),
        Some("crusty_range")
    );
    let cmd = field(s00b, "command");
    for needle in [
        "AUTODRIVE_STATE_DIR",
        "autodrive_state.sh",
        "autodrive_trust.sh",
        "autodrive_rereview_decision",
        "baseRefName",
    ]
    .iter()
    .chain(TOOL_SEARCH_NEEDLES.iter())
    {
        assert!(cmd.contains(needle), "step-00b must reference `{needle}`");
    }
    assert!(
        !cmd.contains("{{"),
        "step-00b reads its inputs from the environment, never from {{...}} in the command"
    );

    let s00c = step(&recipe, "step-00c-crusty-rereview");
    assert_eq!(
        s00c.get("type").and_then(Value::as_str),
        Some("recipe"),
        "step-00c runs the crusty loop as a nested recipe"
    );
    assert_eq!(
        s00c.get("recipe").and_then(Value::as_str),
        Some("autodrive-crusty-loop"),
        "step-00c must run autodrive-crusty-loop, so crusty's own termination applies"
    );
    let condition = field(s00c, "condition");
    assert!(
        condition.contains("crusty_range.rereview") && condition.contains("'true'"),
        "step-00c must run only when crusty_range.rereview == 'true' (got `{condition}`)"
    );
    assert!(
        s00c.get("continue_on_error").is_none(),
        "step-00c must not set continue_on_error: a failed or refused (exit 79) re-review fails the round"
    );
    let ctx = s00c
        .get("context")
        .and_then(Value::as_mapping)
        .expect("step-00c must pass context to the crusty loop");
    for (key, want) in [
        ("autodrive_state_dir", "{{autodrive_state_dir}}"),
        ("repo_path", "{{repo_path}}"),
        ("pr_number", "{{pr_number}}"),
    ] {
        assert_eq!(
            ctx.get(Value::from(key)).and_then(Value::as_str),
            Some(want),
            "step-00c must pass `{key}: \"{want}\"` so the re-run writes the same state dir"
        );
    }
}

#[test]
fn merge_round_hashes_qa_evidence_before_any_agent_step() {
    // D5: the qa evidence is hashed by bash after the evidence step and before
    // any agent runs; step-03 compares, step-05 records.
    let recipe = recipe_yaml("autodrive-merge-round");
    let s00d = step(&recipe, "step-00d-qa-evidence-hash");
    assert_eq!(s00d.get("type").and_then(Value::as_str), Some("bash"));
    assert_eq!(s00d.get("parse_json").and_then(Value::as_bool), Some(true));
    assert_eq!(
        s00d.get("output").and_then(Value::as_str),
        Some("qa_evidence_hash")
    );
    let cmd = field(s00d, "command");
    for needle in [
        "AUTODRIVE_QA_EVIDENCE",
        "autodrive_trust.sh",
        "autodrive_qa_evidence_sha",
        "qa_evidence_sha",
    ]
    .iter()
    .chain(TOOL_SEARCH_NEEDLES.iter())
    {
        assert!(cmd.contains(needle), "step-00d must reference `{needle}`");
    }
    assert!(!cmd.contains("{{"), "step-00d must not interpolate {{...}}");
    let first_agent = steps(&recipe)
        .iter()
        .position(|s| s.get("agent").is_some())
        .expect("the merge round has agent steps");
    assert!(
        step_index(&recipe, "step-00d-qa-evidence-hash") < first_agent,
        "step-00d must hash qa-evidence.json before the first agent step"
    );

    let verdict = step_command(&recipe, "step-03-extract-merge-ready-verdict");
    for needle in [
        "QA_EVIDENCE_HASH",
        "AUTODRIVE_QA_EVIDENCE",
        "autodrive_qa_evidence_sha",
        "qa_evidence=modified",
        "UNREVIEWED_COMMITS",
    ] {
        assert!(
            verdict.contains(needle),
            "step-03 must downgrade on `{needle}`"
        );
    }

    let record = step_command(&recipe, "step-05-write-round-record");
    for needle in [
        "QA_EVIDENCE_HASH",
        "\"qa_evidence_sha\":\"%s\"",
        "umask 077",
    ] {
        assert!(record.contains(needle), "step-05 must reference `{needle}`");
    }
    // The record must keep merge_ready_verdict as its first key: the gate's
    // trust check requires a record that starts with it.
    assert!(
        record.contains("'{\"merge_ready_verdict\":\"%s\""),
        "step-05's record must start with merge_ready_verdict"
    );
}

#[test]
fn merge_round_crusty_evidence_checks_the_range() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let cmd = step_command(&recipe, "step-01b-crusty-evidence");
    for needle in [
        "autodrive_trust.sh",
        "autodrive_base_sha",
        "autodrive_crusty_range",
        "UNREVIEWED_COMMITS",
        "\"crusty_first_unreviewed_sha\":\"%s\"",
    ]
    .iter()
    .chain(RANGE_TOKENS.iter())
    {
        assert!(cmd.contains(needle), "step-01b must reference `{needle}`");
    }
}

#[test]
fn merge_round_prompts_state_the_range_rule() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let assess = field(step(&recipe, "step-02-merge-ready-assessment"), "prompt");
    let flat = squash(assess);
    assert!(
        !flat.contains("is not compared with the current head"),
        "step-02 still says the reviewed SHA `is not compared with the current head`; \
         the range rule replaced that line (#1517 D4)"
    );
    for needle in [
        "UNREVIEWED_COMMITS",
        "crusty-review-required",
        "crusty_first_unreviewed_sha",
        "crusty-range-unreadable",
        "base merge",
    ] {
        assert!(
            flat.contains(needle),
            "the step-02 prompt must state the range rule (`{needle}`)"
        );
    }
    let fix = field(step(&recipe, "step-04-address-blockers"), "prompt");
    let flat = squash(fix);
    let lower = flat.to_ascii_lowercase();
    for needle in ["crusty-review-required", "crusty-range-unreadable"] {
        assert!(
            flat.contains(needle),
            "step-04 must tell the agent not to try to clear `{needle}`"
        );
    }
    for needle in ["rewrite history", "reset", "force-push", "refs"] {
        assert!(
            lower.contains(needle),
            "step-04 must forbid the agent to {needle}"
        );
    }
    for prompt in [assess, fix] {
        for leak in ["{{crusty_range", "{{qa_evidence_hash"] {
            assert!(
                !prompt.contains(leak),
                "agent prompts never see the raw range or hash step output (`{leak}`)"
            );
        }
    }
}

#[test]
fn the_range_allowlist_is_defined_once_in_autodrive_trust() {
    let root = workspace_root();
    let define = regex::Regex::new(
        r"(^|[\s;])(readonly|declare|typeset|local|export)?(\s+-[A-Za-z]+)*\s*AUTODRIVE_RANGE_ALLOWLIST(\[[^\]]*\])?\+?=",
    )
    .unwrap();
    let keep = |p: &Path| {
        !p.components().any(|c| c.as_os_str() == "tests")
            && p.extension()
                .is_some_and(|e| e == "yaml" || e == "yml" || e == "sh")
    };
    let mut files = walk_files(&root.join("amplifier-bundle/recipes"), &keep)
        .unwrap_or_else(|e| panic!("recipe walk failed: {e}"));
    files.extend(
        walk_files(&root.join("amplifier-bundle/tools"), &|_| true)
            .unwrap_or_else(|e| panic!("tool walk failed: {e}")),
    );
    let mut definers = Vec::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if text
            .lines()
            .any(|l| !l.trim_start().starts_with('#') && define.is_match(l))
        {
            definers.push(relative_display(&path, &root));
        }
    }
    assert_eq!(
        definers,
        vec!["amplifier-bundle/tools/autodrive_trust.sh".to_string()],
        "AUTODRIVE_RANGE_ALLOWLIST must be defined in autodrive_trust.sh and nowhere else"
    );
    let trust = read(&tool_path("autodrive_trust.sh"));
    assert!(
        trust
            .lines()
            .any(|l| l.contains("readonly") && l.contains("AUTODRIVE_RANGE_ALLOWLIST")),
        "AUTODRIVE_RANGE_ALLOWLIST must be read-only"
    );
    for entry in [
        "PR_DESCRIPTION.md",
        ".github/pull_request_template.md",
        ".autodrive/evidence/",
    ] {
        assert!(
            trust.contains(entry),
            "the allowlist must contain `{entry}`"
        );
    }
}

#[test]
fn autodrive_trust_is_a_side_effect_free_library() {
    let trust = read(&tool_path("autodrive_trust.sh"));
    for func in [
        "autodrive_base_sha",
        "autodrive_range_allowed",
        "autodrive_crusty_range",
        "autodrive_rereview_decision",
        "autodrive_qa_evidence_sha",
        "autodrive_qa_trusted",
        "autodrive_scenario_results",
    ] {
        assert!(
            regex::Regex::new(&format!(r"(?m)^{func}\(\)\s*\{{"))
                .unwrap()
                .is_match(&trust),
            "autodrive_trust.sh must define `{func}()`"
        );
    }
    for token in RANGE_TOKENS.iter().chain(QA_TRUSTED_TOKENS.iter()) {
        assert!(
            trust.contains(token),
            "autodrive_trust.sh must print the token `{token}`"
        );
    }
    // History cannot be rewritten under the walk, and git cannot prompt.
    for needle in [
        "GIT_NO_REPLACE_OBJECTS=1",
        "GIT_GRAFT_FILE=/dev/null",
        "GIT_TERMINAL_PROMPT=0",
        "shallow",
        "merge-tree --write-tree",
        "diff-tree",
        "--no-renames",
        "check-ref-format --branch",
        "+refs/heads/",
    ] {
        assert!(
            trust.contains(needle),
            "autodrive_trust.sh must use `{needle}`"
        );
    }
    for var in ["GIT_DIR", "GIT_WORK_TREE"] {
        assert!(
            trust.contains(&format!("-u {var}"))
                || trust.contains(&format!("unset {var}"))
                || regex::Regex::new(&format!(r"unset [A-Z_ ]*\b{var}\b"))
                    .unwrap()
                    .is_match(&trust),
            "autodrive_trust.sh must run git with `{var}` unset"
        );
    }
    // autodrive_clear_phase lives with the other phase helpers and rewrites
    // phases.tsv privately.
    let state = read(&tool_path("autodrive_state.sh"));
    let clear = state
        .find("autodrive_clear_phase()")
        .expect("autodrive_state.sh must define autodrive_clear_phase()");
    let body = &state[clear..];
    let body = &body[..body.find("\n}").map_or(body.len(), |i| i + 2)];
    for needle in ["umask 077", "mktemp", "mv -f", "^[a-z][a-z-]*$"] {
        assert!(
            body.contains(needle),
            "autodrive_clear_phase must use `{needle}`"
        );
    }
    // A library: sourcing it runs nothing, and it never evaluates text.
    for line in trust.lines() {
        let code = line.trim_start();
        if code.starts_with('#') || code.is_empty() {
            continue;
        }
        assert!(
            !code.starts_with("eval ") && !code.contains(" eval ") && !code.contains("sh -c"),
            "autodrive_trust.sh must not eval or run built strings: {line}"
        );
    }
    let out = Command::new("bash")
        .arg("-uc")
        .arg(r#". "$1" && . "$2" && echo sourced"#)
        .arg("_")
        .arg(tool_path("autodrive_state.sh"))
        .arg(tool_path("autodrive_trust.sh"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("source autodrive_trust.sh");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "sourced\n",
        "sourcing autodrive_trust.sh under set -u must print nothing and succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn merge_gate_checks_the_range_and_the_qa_evidence_chain() {
    let gate = read(&tool_path("autodrive_merge_gate.sh"));
    let view = gate
        .lines()
        .find(|l| l.contains("gh pr view \"$PR\" --json state,mergedAt"))
        .expect("section 0 reads the PR with gh pr view --json");
    assert!(
        view.contains("baseRefName"),
        "section 0 must read baseRefName in the same gh pr view call: {view}"
    );
    for needle in [
        "${GATE_HOME}/autodrive_trust.sh",
        "autodrive_qa_trusted",
        "autodrive_base_sha",
        "autodrive_crusty_range",
        "git cat-file -e",
        "merge-ready-records.tsv",
        "merge-ready-latest.json",
        "qa-evidence.json",
        "qa-other",
        "crusty-range-other",
        "after the clean crusty round is not a base merge or a description or evidence change; criterion 3 is not met",
        "the commits after the clean crusty round cannot be read; criterion 3 is not met",
        "mktemp",
    ]
    .iter()
    .chain(QA_TRUSTED_TOKENS.iter())
    .chain(RANGE_TOKENS.iter())
    {
        assert!(gate.contains(needle), "the merge gate must check `{needle}`");
    }
    // The record is read once, into a private copy; section 7 reads the copy.
    let section7 = gate
        .find("# --- 7.")
        .expect("the gate keeps a section 7 for the merge-ready verdict");
    assert!(
        !gate[section7..].contains("cat \"$ROUND_RECORD\""),
        "section 7 must read the verified private copy, never \"$ROUND_RECORD\" again"
    );
}

#[test]
fn qa_evidence_records_one_result_per_scenario_file() {
    let recipe = recipe_yaml("autodrive-merge-evidence");
    let cmd = step_command(&recipe, "step-02-qa-team-scenarios");
    for needle in [
        "\"gadugi_scenario_results\":\"%s\"",
        "autodrive_scenario_results",
        "autodrive_trust.sh",
        "umask 077",
        "INVALID",
    ]
    .iter()
    .chain(TOOL_SEARCH_NEEDLES.iter())
    {
        assert!(
            cmd.contains(needle),
            "the qa evidence step must reference `{needle}`"
        );
    }
}

#[test]
fn the_reference_documents_the_range_rule_and_the_qa_chain() {
    let reference = read(&workspace_root().join("docs/reference/auto-drive-to-merge.md"));
    for needle in [
        "autodrive_trust.sh",
        "AUTODRIVE_RANGE_ALLOWLIST",
        "PR_DESCRIPTION.md",
        ".github/pull_request_template.md",
        ".autodrive/evidence/",
        "step-00b-crusty-range",
        "step-00c-crusty-rereview",
        "step-00d-qa-evidence-hash",
        "UNREVIEWED_COMMITS",
        "crusty-review-required",
        "crusty_first_unreviewed_sha",
        "qa_evidence_sha",
        "merge-ready-records.tsv",
        "gadugi_scenario_results",
        "autodrive_clear_phase",
        "autodrive_qa_trusted",
        "autodrive_crusty_range",
        "autodrive_base_sha",
        "same user",
        "--match-head-commit",
    ]
    .iter()
    .chain(RANGE_TOKENS.iter())
    .chain(QA_TRUSTED_TOKENS.iter())
    {
        assert!(
            reference.contains(needle),
            "docs/reference/auto-drive-to-merge.md must document `{needle}`"
        );
    }
    let skill = read(&workspace_root().join("amplifier-bundle/skills/merge-ready/SKILL.md"));
    let section = skill
        .find("## Running under auto-drive")
        .expect("merge-ready keeps its auto-drive section");
    for needle in [
        "crusty-review-required",
        "crusty-range-unreadable",
        "at least 3",
    ] {
        assert!(
            skill[section..].contains(needle),
            "the merge-ready auto-drive section must mention `{needle}`"
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
