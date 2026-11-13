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
//!   contract (issue #1337, PR #1347), taken from the driver's own tree, never copied.
//! - Every verdict is structured and read through the canonical
//!   `extract-json | extract-field --default …` pipeline, and every fail-safe
//!   default is the BLOCKING token: `CONCERNS`, `NOT_MERGE_READY`, `STUCK`.
//! - The two absolute prohibitions — a hook-skipping commit flag and a
//!   branch-protection bypass — never appear in an executable position.
//! - Nothing merges without every criterion re-verified in the same run and
//!   bound to one head SHA; an unreadable criterion is a failure.
//! - Exit 79 is terminal and is never retried into.
//! - No step declares a timeout at any scale.
//! - Every step-output read is `RECIPE_VAR_<output>` alone (issues #1511,
//!   #1561): no read of the bare upper-case name in any form, no bare name as
//!   a `RECIPE_VAR_` read's default, no output templated into a `command:`,
//!   and the read counts match the table in the reference.

use serde_yaml::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// The brick budget for a recipe or tool: at most this many lines.
// The #1528 recipe-length test in `artifact_guard_workflow_tests.rs`
// imports this rather than restating it (PR #1586 review, round 6).
// Other brick checks still carry their own copies of the limit
// (`issue_1337_loop_health_evaluator_test.rs`,
// `issue_684_host_aware_workflow_test.rs` and the `*_decomposition_test.rs`
// files), so changing it here does not change them.
pub(crate) const BRICK_LINE_BUDGET: usize = 400;

const AUTODRIVE_RECIPES: [&str; 8] = [
    "auto-drive-to-merge",
    "autodrive-build",
    "autodrive-crusty-round",
    "autodrive-crusty-loop",
    "autodrive-merge-evidence",
    "autodrive-merge-round",
    "autodrive-merge-loop",
    "autodrive-finalize",
];

const AUTODRIVE_TOOLS: [&str; 31] = [
    "autodrive_budget.sh",
    "autodrive_crusty_rereview.sh",
    "autodrive_done_override.sh",
    "autodrive_escalate.sh",
    "autodrive_finding_rules.sh",
    "autodrive_followups.sh",
    "autodrive_followups_check.sh",
    "autodrive_last_round.sh",
    "autodrive_loop.sh",
    "autodrive_loop_record.sh",
    "autodrive_crusty_record.sh",
    "autodrive_crusty_finalize.sh",
    "autodrive_completion_context.sh",
    "autodrive_merge_message.sh",
    "autodrive_merge_ready_files.sh",
    "autodrive_platform_facts.sh",
    "autodrive_pr_head.sh",
    "autodrive_progress.sh",
    "autodrive_qa_docs.sh",
    "autodrive_qa_evidence.sh",
    "autodrive_qa_scope.sh",
    "autodrive_qa_sources.sh",
    "autodrive_refusal.sh",
    "autodrive_round_evidence.sh",
    "autodrive_round_tail.sh",
    "autodrive_scenario_name.sh",
    "autodrive_state.sh",
    "autodrive_trust.sh",
    "autodrive_unmerged.sh",
    "autodrive_worktree_guard.sh",
    "git-repo-env.sh",
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

// Every bash `command:` body in a recipe — the executable positions.
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

// Files that make up the workflow's control path (no prose).
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
        recipe_text("auto-drive-to-merge").contains("no_merge: \"false\""),
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
fn unresolved_loops_use_the_shared_loop_health_evaluator_contract() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    // Resolved to an absolute path in the launch-side bundle (by
    // autodrive_worktree_guard.sh loop-preflight), never by name under
    // `--working-dir <PR worktree>`: a name lookup there would load the PR's
    // own copy of its stop rule (#1528).
    let guard = read(&tool_path("autodrive_worktree_guard.sh"));
    assert!(
        guard.contains("launch_recipe loop-health-evaluator")
            && driver.contains("read -r HEALTH_RECIPE_PATH")
            && driver.contains("recipe run \"$HEALTH_RECIPE_PATH\""),
        "the loop driver must invoke the shared loop-health-evaluator brick from the launch-side bundle"
    );
    assert!(
        driver.contains("recipe run \"$ROUND_RECIPE_PATH\"")
            && !driver.contains("recipe run \"$ROUND_RECIPE\" ")
            && !driver.contains("recipe run loop-health-evaluator"),
        "no recipe may reach `recipe run` by bare name; the reviewed tree would answer the lookup"
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
    // must not appear anywhere in this workflow. The one exception is the
    // #1512 log reader in autodrive_loop.sh, which must name step-04 to find
    // the status line its verdict follows.
    let reader = tool_path("autodrive_loop.sh");
    for path in control_path_files() {
        let text = read(&path);
        for copied in [
            "step-01-collect-loop-evidence",
            "step-02-evaluate-loop-health",
            "step-03-resolve-loop-verdict",
            "step-04-enforce-loop-verdict",
        ] {
            if path == reader && copied == "step-04-enforce-loop-verdict" {
                continue;
            }
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
    // The matcher must see the label itself and nothing that merely shares
    // its prefix: `$ROUND_VERDICT` is a different variable.
    assert!(names_round_label(r#"[ "$ROUND" -gt 5 ]"#));
    assert!(names_round_label("[[ ${ROUND} -ge 3 ]]"));
    assert!(names_round_label("(( ROUND > 9 ))"));
    assert!(names_round_label("((ROUND>9))"));
    assert!(names_round_label(
        r#"[ "$ROUND_CLEAN" = x ] || [ $ROUND -lt 2 ]"#
    ));
    assert!(!names_round_label(r#"[ "$ROUND_CLEAN" = "true" ]"#));
    assert!(!names_round_label(r#"[ "$ROUND_VERDICT" != x ]"#));
    assert!(!names_round_label("(( ROUND_RC > 0 ))"));
    const COMPARISONS: [&str; 12] = [
        "-ge", "-gt", "-le", "-lt", "-eq", "-ne", ">=", "<=", "==", "!=", ">", "<",
    ];
    for (n, line) in driver.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if !names_round_label(line) {
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

// True when `line` names the shell variable `ROUND` itself (`$ROUND`,
// `${ROUND}`, `((ROUND`, `(( ROUND`), not a longer identifier that merely
// starts with it such as `$ROUND_VERDICT` or `$ROUND_CLEAN`.
fn names_round_label(line: &str) -> bool {
    let continues_identifier = |rest: &str| {
        rest.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if line.contains("${ROUND}") {
        return true;
    }
    ["$ROUND", "((ROUND", "(( ROUND"].iter().any(|prefix| {
        line.match_indices(prefix)
            .any(|(i, m)| !continues_identifier(&line[i + m.len()..]))
    })
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
    assert!(evidence.contains("Poll pending work to its natural terminal state"));
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
    let cases = [(
        "autodrive-crusty-round",
        "step-03-extract-crusty-verdict",
        "crusty_verdict",
        "CONCERNS",
        "CRUSTY_REVIEW",
    )];
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
        // into a command position. A review that is only the JSON verdict is
        // an object, which recipe-runner exports as RECIPE_VAR_<output> alone,
        // and an inherited bare name must not outrank it (#1561), so the read
        // is RECIPE_VAR_<output> only.
        let runner_var = format!("RECIPE_VAR_{}", env_var.to_ascii_lowercase());
        assert!(
            cmd.contains(&format!("${{{runner_var}:-}}")),
            "{step_id} must read the agent output from {runner_var}"
        );
        assert!(
            !reads_the_bare_name(cmd, env_var),
            "{step_id} must not read the bare `{env_var}`: an inherited value would \
             outrank the review this round produced (#1561)"
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
    let driver = read(&tool_path("autodrive_loop.sh")).replace(
        ". \"${LOOP_HOME}/autodrive_loop_record.sh\" || exit 1",
        &read(&tool_path("autodrive_loop_record.sh")),
    );
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
    let round = recipe_yaml("autodrive-merge-round");
    let merge = field(
        step(&round, "step-03-extract-merge-ready-verdict"),
        "command",
    );
    assert!(merge.contains("orch helper autodrive round"));
    assert!(merge.contains("RECIPE_VAR_merge_ready_review"));
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

// Every prohibited construct, in every spelling that actually works.
//
// A substring list of `--no-verify` / `--admin` / `--bypass` is not the
// prohibition — it is three of its spellings. Hooks are equally skipped by
// `git commit -nm "x"`, `git commit -m "x" -n`, `git -C . commit -n`,
// `git -c core.hooksPath=/dev/null commit`, and `HUSKY=0 git commit`; the
// merge gate is equally bypassed by `gh api -X PUT .../merge` and by
// `gh pr merge --auto`, neither of which contains `--admin`.
//
// Returns the label of every construct the line matches.
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
    let finalizer = recipe_yaml("autodrive-finalize");
    let prompt = field(step(&finalizer, "finalize-observed-action"), "prompt");
    assert!(prompt.contains("expected-head protection"));
    assert!(prompt.contains("never bypass branch protections"));
}

// ── No silent merge ──────────────────────────────────────────────────────────

#[test]
fn exit_79_is_terminal_and_never_retried_into() {
    let driver = read(&tool_path("autodrive_loop.sh"));
    assert!(
        driver.contains("AUTODRIVE_EXIT_POLICY_REFUSAL=79"),
        "the loop driver must name exit 79 as the terminal policy refusal"
    );
    assert!(
        read(&tool_path("autodrive_refusal.sh")).contains("structural_refusal"),
        "the runner's structural_refusal marker must be recognised alongside exit 79 (#1437)"
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
        .find("recipe run \"$HEALTH_RECIPE_PATH\"")
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
    for recipe in [
        "autodrive-build",
        "autodrive-crusty-loop",
        "autodrive-merge-loop",
    ] {
        let doc = recipe_yaml(recipe);
        let prompt = field(&steps(&doc)[0], "prompt");
        assert!(prompt.contains("already") && prompt.contains("MERGED"));
        assert!(prompt.contains("historical evidence never establishes current readiness"));
    }
    let review = recipe_yaml("autodrive-crusty-round");
    assert!(field(step(&review, "step-02-crusty-review"), "prompt").contains("NEW evidence"));
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
        merge.contains("orch helper autodrive round"),
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
        r#"MERGE_ARGV=(pr merge "$PR" --squash --delete-branch --match-head-commit "$HEAD_SHA" --subject "$MERGE_SUBJECT" --body-file -)"#,
        r#"gh pr merge "$PR" --squash --subject "$MERGE_SUBJECT""#,
        r#"printf '%s' "$MERGE_BODY" | gh pr merge "$PR" --squash --body-file -"#,
        "gh pr view \"$PR\" --json state,mergedAt,title,body",
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

// Key/value pairs of a SKILL.md's frontmatter: the top-level `key: value`
// lines between the first two `---` lines, with surrounding quotes removed.
// `None` when the file has no complete frontmatter block.
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

// Every regular file under `root` that `keep` accepts. Symlinks are skipped,
// so a link loop terminates and nothing outside `root` is read. A directory
// or entry that cannot be read is an error, never a silent skip.
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

// A skill an agent must not invoke: its name and its SKILL.md, for messages.
struct FlaggedSkill {
    name: String,
    skill_md: String,
}

// Every skill under `skills_root` whose frontmatter sets
// `disable-model-invocation: true` (any case, quoted or not). The name is
// the frontmatter `name:`, falling back to the directory name.
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

// One message per `Skill(skill="<name>")` in `text` that names a flagged
// skill. Matches either quote style, whitespace around `(`, `=` and `)`, and
// a backslash before either quote (the escaped form a YAML double-quoted
// string produces); the step id is the nearest earlier `- id:` line.
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

// Runs the detector over every `*.yaml` / `*.yml` under `recipes_root`.
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
    // missing outside-in scenarios. If qa-team ever starts refusing agents, that
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

// `autodrive_repo_type` in autodrive_trust.sh is the one place the
// repository type is decided. Since #2569 it is a fact recorded in
// qa-inputs.json for the criterion-1 judgement, never a stop.

#[cfg(unix)]
#[test]
fn autodrive_repo_type_follows_the_qa_team_table() {
    let trust = tool_path("autodrive_trust.sh");
    let cases: &[(&[&str], &str)] = &[
        (&["Cargo.toml"], "rust-cli"),
        (&["package.json"], "node"),
        (&["pyproject.toml"], "python"),
        (&["setup.py"], "python"),
        (&[], "unknown"),
        // Cargo.toml decides first, as in qa-team's table.
        (&["Cargo.toml", "package.json"], "rust-cli"),
        // #1516: a root Python project wins over package.json.
        (&["package.json", "pyproject.toml"], "python"),
        (&["setup.py", "package.json"], "python"),
        // #1516: a single nested Cargo workspace within depth 3 is Rust;
        // unrelated nested crates, or one only under a pruned directory,
        // decide nothing.
        (&["a/Cargo.toml"], "rust-cli"),
        (&["a/Cargo.toml", "b/Cargo.toml"], "unknown"),
        (&["target/Cargo.toml"], "unknown"),
        // #1516: a root manifest wins over a nested crate (jamestown shape).
        (
            &["pyproject.toml", "grid-encode-decode/Cargo.toml"],
            "python",
        ),
        (&["package.json", "sub/Cargo.toml"], "node"),
    ];
    for (files, want_type) in cases {
        let tmp = tempfile::tempdir().expect("tempdir");
        for f in *files {
            let p = tmp.path().join(f);
            fs::create_dir_all(p.parent().expect("marker parent")).expect("marker dir");
            fs::write(p, "").expect("write marker");
        }
        // #1763 reads the manifests from git, never the work tree: commit
        // them. With AUTODRIVE_QA_BASE unset (a preflight) HEAD is the merge base.
        let git = Command::new("bash")
            .arg("-c")
            .arg(
                "git init -q . && git add -A && git -c user.name=t -c user.email=t@example.invalid \
                 commit -q --allow-empty --no-verify -m init",
            )
            .current_dir(tmp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(git.status.success(), "commit the fixture: {git:?}");
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(r#". "$1" && autodrive_repo_type "$2""#)
            .arg("bash")
            .arg(&trust)
            .arg(tmp.path())
            .env_remove("AUTODRIVE_QA_BASE")
            .env_remove("AUTODRIVE_QA_SCENARIO_DIR");
        let out = cmd.output().expect("run bash");
        assert!(out.status.success(), "{files:?}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{want_type}\n"),
            "files={files:?}"
        );
    }
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

// The sentence every auto-drive agent prompt carries (docs/reference/
// auto-drive-to-merge.md, "Agents do not touch the state directory"). Line
// wrapping in a YAML block scalar is allowed, so prompts are compared with
// whitespace collapsed.

// Words the state directory must never be named by in an agent prompt.

// The five agent steps of the auto-drive workflow, by recipe.

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

// Every (recipe, step id, prompt) for a step that runs an agent, across the
// auto-drive recipes and the loop-health evaluator both loops call.

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
    assert!(
        !reads_the_bare_name(cmd, "AUTODRIVE_TOOLS_DIR"),
        "step-00 must not read an inherited AUTODRIVE_TOOLS_DIR (#1607)"
    );
    for needle in [
        "autodrive_merge_ready_files.sh",
        "${RECIPE_VAR_autodrive_tools_dir:-}",
    ] {
        assert!(cmd.contains(needle), "missing trusted lookup: {needle}");
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
        "qa_judgement",
        "qa-record",
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

// step-02a-qa-inputs runs autodrive_qa_evidence.sh, which measures the facts
// criterion 1 is judged from (#2569). Commands from the environment are run
// as documented and never evaluated.

#[test]
fn qa_inputs_tool_runs_declared_commands_without_eval() {
    let tool = read(&tool_path("autodrive_qa_evidence.sh"));
    let cmd = tool.as_str();
    for needle in [
        "AUTODRIVE_QA_SCENARIO_DIR",
        "AUTODRIVE_QA_COMMAND",
        "AUTODRIVE_QA_COMMANDS",
        "AUTODRIVE_QA_DIR",
        "set -f",
        "bash -c \"$",
        "tests/agentic",
        "npm test",
        "cargo test --workspace --locked --no-fail-fast",
        "pytest",
    ] {
        assert!(
            cmd.contains(needle),
            "autodrive_qa_evidence.sh must contain `{needle}`"
        );
    }
    let eval = regex::Regex::new(r"(^|[;&|\s(])eval\s").unwrap();
    for line in cmd.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        assert!(
            !eval.is_match(line),
            "autodrive_qa_evidence.sh must never eval a command:\n  {line}"
        );
    }
}

#[test]
fn crusty_round_records_the_reviewed_head_sha_every_round() {
    // Point 5 of #1517: a CLEAN round with no commits wrote "head_sha":"", so
    // the clean verdict could not be tied to a commit.
    let recipe = recipe_yaml("autodrive-crusty-round");
    let ctx = field(step(&recipe, "step-01-round-context"), "prompt");
    assert!(
        ctx.contains("sanitized local Git HEAD"),
        "step-01 must read the reviewed head with git, in bash"
    );
    assert!(
        field(step(&recipe, "step-06-write-round-record"), "command")
            .contains("autodrive_crusty_record.sh")
    );
    let cmd = read(&tool_path("autodrive_crusty_record.sh"));
    // #1561: step outputs are read only as RECIPE_VAR_<key>.
    for needle in [
        "RECIPE_VAR_crusty_round_context",
        "RECIPE_VAR_crusty_fix_evidence",
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
    let driver = read(&tool_path("autodrive_loop.sh")).replace(
        ". \"${LOOP_HOME}/autodrive_loop_record.sh\" || exit 1",
        &read(&tool_path("autodrive_loop_record.sh")),
    );
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
    // The ordering is about the WRITE. The driver also READS the manifest
    // before its first round, to label its rounds after the ones a previous
    // loop on the same state dir wrote (PR #1520 review), so the first
    // mention of the file is not the row being written.
    let append = ">> \"${STATE_DIR}/${LOOP_NAME}-records.tsv\"";
    assert_eq!(
        driver.matches(append).count(),
        1,
        "autodrive_loop.sh must append its manifest row in exactly one place: `{append}`"
    );
    assert!(
        !driver
            .replace(append, "")
            .contains("> \"${STATE_DIR}/${LOOP_NAME}-records.tsv\""),
        "nothing else in autodrive_loop.sh may write or truncate <loop>-records.tsv; \
         the manifest is append-only"
    );
    let manifest = driver
        .find(append)
        .expect("the driver must append to <loop>-records.tsv");
    let health = driver
        .find("recipe run \"$HEALTH_RECIPE_PATH\"")
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

    let round = recipe_yaml("autodrive-merge-round");
    let prompt = field(step(&round, "step-01b-crusty-evidence"), "prompt");
    assert!(prompt.contains("CLEAN or accepted MINOR_ONLY"));
    assert!(prompt.contains("evaluator DONE"));
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

/// #2536: the resolver reads `$TOP` under `set -u`; #2340 dropped its
/// assignment and every merge-readiness step failed "TOP: unbound variable".
/// Run it, as step-00 does, in a temporary git repository: once with the
/// install inside that repository (the `"$TOP"/*` match path) and once
/// with the install outside it.
#[test]
fn merge_ready_files_resolver_runs_under_set_u_in_a_git_repository() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("canonical tempdir");
    let repo = root.join("repo");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("home dir");
    // Hooks export GIT_DIR / GIT_INDEX_FILE / GIT_WORK_TREE; left in place they
    // would send the fixture's commit to the real worktree. BASH_ENV would run
    // host code in these shells, and the real HOME holds the user's git files.
    let isolate = |cmd: &mut Command| {
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_PREFIX",
            "BASH_ENV",
        ] {
            cmd.env_remove(var);
        }
        cmd.env("HOME", &home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
    };
    let install_skill = |base: &Path| -> PathBuf {
        let dir = base.join("amplifier-bundle/skills/merge-ready");
        fs::create_dir_all(&dir).expect("skill dir");
        fs::write(dir.join("SKILL.md"), "---\nname: merge-ready\n---\n").expect("SKILL.md");
        fs::write(dir.join("pr-description-template.md"), "# template\n").expect("template");
        dir
    };
    let inside = install_skill(&repo);
    let outside = install_skill(&root.join("install"));
    let mut setup = Command::new("bash");
    setup
        .arg("-c")
        .arg(
            "git init -q . && git add -A && git -c user.name=t -c user.email=t@example.invalid \
             commit -q --no-verify -m init",
        )
        .current_dir(&repo);
    isolate(&mut setup);
    let git = setup.output().expect("run git");
    assert!(git.status.success(), "commit the fixture: {git:?}");

    for (amplihack_home, dir) in [(&repo, &inside), (&root.join("install"), &outside)] {
        let mut resolver = Command::new("bash");
        resolver
            .arg("-u")
            .arg(tool_path("autodrive_merge_ready_files.sh"))
            .current_dir(&repo)
            .env("AMPLIHACK_HOME", amplihack_home)
            .env("REPO_PATH", &repo);
        isolate(&mut resolver);
        let out = resolver.output().expect("run the resolver");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && !stderr.contains("unbound variable"),
            "AMPLIHACK_HOME={}: {out:?}",
            amplihack_home.display()
        );
        let json: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("one line of JSON on stdout");
        let dir = dir.display().to_string();
        assert_eq!(json["skill_dir"], dir.as_str());
        assert_eq!(json["skill_md"], format!("{dir}/SKILL.md").as_str());
        assert_eq!(
            json["template"],
            format!("{dir}/pr-description-template.md").as_str()
        );
        assert!(
            json["skill_md_sha"].as_str().is_some_and(|s| s.len() == 40),
            "skill_md_sha must be a git blob hash: {json}"
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
    let text = read(&workspace_root().join("amplifier-bundle/skills/merge-ready/SKILL.md"));
    assert!(text.contains("runner-pinned launch bundle"));
    assert!(text.contains("accepted `MINOR_ONLY`"));
    assert!(text.contains("all 13") || text.contains("every criterion"));
    assert!(text.contains("at least 3 SEEK -> VALIDATE -> FIX cycles"));
}

#[test]
fn a_required_count_of_zero_beside_a_review_rule_is_not_read_as_zero() {
    // GitHub publishes reviewDecision only when a rule requires at least one
    // approval. At a count of 0 it stays null even while a code-owner,
    // last-push or required-reviewer rule blocks the merge, so reading that 0
    // as "no review required" reports criterion 6 met while a review is
    // missing. The PF-*-code-owner, -last-push and -required-reviewers cases
    // in test-auto-drive-to-merge.sh exercise the behaviour; this keeps the
    // tool reading every field those cases depend on.
    let facts = read(&tool_path("autodrive_platform_facts.sh"));
    for needle in [
        // Classic protection spells it in the plural, rulesets in the singular.
        ".require_code_owner_reviews == true",
        ".require_code_owner_review == true",
        ".require_last_push_approval == true",
        ".required_reviewers",
        "REVIEW_RULE",
    ] {
        assert!(
            facts.contains(needle),
            "autodrive_platform_facts.sh must read `{needle}`"
        );
    }
    assert!(
        !facts.contains("requires no review."),
        "the tool header still says an empty reviewDecision means the branch requires no review"
    );
    let skill = read(&workspace_root().join("amplifier-bundle/skills/merge-ready/SKILL.md"));
    assert!(
        !skill.contains("leaves it empty when the base branch requires no review"),
        "merge-ready SKILL.md still says an empty reviewDecision means no review is required"
    );
}

#[test]
fn a_pending_approval_is_cleared_by_the_round_not_sent_to_a_person() {
    // crusty round-1 on PR #1520: with the count unknown, BLOCKED is also what
    // a failing required check gives, and an agent can fix that. The tool
    // reports PENDING then (the PF-pending-* cases in
    // test-auto-drive-to-merge.sh exercise it, and MERGEREADY-approval-downgrade
    // shows step-03 still downgrades it). The routing itself is prompt text, so
    // only the two sentences that carry it are pinned here.
    let recipe = recipe_yaml("autodrive-merge-round");
    let assess = squash(field(
        step(&recipe, "step-02-merge-ready-assessment"),
        "prompt",
    ));
    assert!(
        assess.contains("For `PENDING` report `approval-pending-other-blockers`"),
        "step-02 must name a PENDING approval apart from approval-state-unreadable"
    );
    let fix = squash(field(step(&recipe, "step-04-address-blockers"), "prompt"));
    let at = fix
        .find("**`approval-pending-other-blockers`**")
        .expect("step-04 must say what to do with approval-pending-other-blockers");
    let end = fix[at..]
        .find("- **`changes-requested`**")
        .map_or(fix.len(), |e| at + e);
    let bullet = &fix[at..end];
    assert!(
        bullet.contains("Clear the other blockers first") && !bullet.contains("needs a person"),
        "step-04 must clear the other blockers for a PENDING approval, not route it to a person:\n  {bullet}"
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
        .find("recipe show \"$HEALTH_RECIPE_PATH\"")
        .expect("the driver must resolve loop-health-evaluator up front");
    let first_round = driver
        .find("recipe run \"$ROUND_RECIPE_PATH\"")
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
        "loop-health",
        "loop-health-evaluator",
        "crusty-old-engineer",
        "merge-ready",
        "qa-team",
        "branch protection",
        "platform confirmation",
        "79",
        "Resume",
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

// ── Commits after the clean round, and the qa evidence chain (#1517 D4-D6) ──

// What `autodrive_crusty_range` prints, besides `ok`.

// What `autodrive_qa_trusted` prints, besides `ok`.

// The search order every auto-drive bash step uses to find a bundle tool.

fn step_command<'a>(recipe: &'a Value, id: &str) -> &'a str {
    field(step(recipe, id), "command")
}

// The subcommands of autodrive_round_evidence.sh and the function each runs.

// Functions in autodrive_round_evidence.sh that no subcommand dispatches to
// but that subcommand bodies call (#2219): measured_downgrade and
// measured_held_by read every measurement through `_measured_read`, and the
// criterion 6 stop line reads its facts through `_approval_detail`. Without
// following them, the reads those bodies make would be invisible to the
// step checks below.

// The bash a merge-round step runs: its own command and, for each
// `bash "$T" <subcommand>` of autodrive_round_evidence.sh it calls, the body
// of the function that subcommand dispatches to, and of every subcommand
// function that body calls in turn. The step bodies moved into that tool when
// the recipe reached its brick budget (PR #1520 review, #1561), so a check on
// what a step does reads both.

#[test]
fn crusty_rereview_runs_between_merge_rounds_at_the_loop_depth() {
    // D4, moved by the crusty review of PR #1520: a code commit after the
    // clean crusty round goes back to crusty BEFORE the next merge round,
    // from the merge-ready loop's own shell. Nested in a merge round as
    // step-00c, the crusty loop ran one recipe runner deeper and the
    // recursion guard refused its rounds with exit 79.
    let round = recipe_yaml("autodrive-merge-round");
    for gone in ["step-00b-crusty-range", "step-00c-crusty-rereview"] {
        assert!(
            !steps(&round)
                .iter()
                .any(|s| s.get("id").and_then(Value::as_str) == Some(gone)),
            "{gone} must not come back: the crusty re-review runs between merge rounds"
        );
    }
    let order = [
        "step-00-merge-ready-files",
        "merge-evidence",
        "step-00d-qa-evidence-hash",
        "step-01-platform-facts",
        "step-01b-crusty-evidence",
        "step-02-merge-ready-assessment",
    ];
    for pair in order.windows(2) {
        assert!(
            step_index(&round, pair[0]) < step_index(&round, pair[1]),
            "`{}` must run before `{}`",
            pair[0],
            pair[1]
        );
    }

    // The merge-ready loop passes the re-review, from the loop driver's own
    // install, as its before-round step.
    let mloop = recipe_yaml("autodrive-merge-loop");
    let cmd = step_command(&mloop, "step-02-merge-ready-loop");
    for needle in [
        "REREVIEW=\"$(dirname \"$LOOP_HELPER\")/autodrive_crusty_rereview.sh\"",
        "--before-round \"$REREVIEW\"",
        "ERROR: autodrive-tools-not-found: ${REREVIEW}",
    ] {
        assert!(
            cmd.contains(needle),
            "autodrive-merge-loop step-02 must contain `{needle}`"
        );
    }

    // The re-review measures the range, runs the crusty loop in its own
    // shell, and records a DONE loop as phase 2 does.
    let tool = read(&tool_path("autodrive_crusty_rereview.sh"));
    for needle in [
        "bash \"$HERE/autodrive_round_evidence.sh\" crusty-range",
        "bash \"$HERE/autodrive_loop.sh\"",
        "--loop-name \"crusty\"",
        "--round-recipe \"autodrive-crusty-round\"",
        "--clean-token \"CLEAN\"",
        "--verdict-field \"crusty_verdict\"",
        // Its judging agents start in the checked review dir, not the PR worktree (#1528).
        "--review-dir \"$REVIEW_DIR\"",
        "--state-dir \"$DIR\"",
        "bash \"$HERE/autodrive_crusty_finalize.sh\"",
        "crusty-rereview-not-needed",
        "crusty-rereview-done",
        "crusty-rereview-not-done",
        "crusty-rereview-refused",
        "crusty-rereview-unavailable",
    ] {
        assert!(
            tool.contains(needle),
            "autodrive_crusty_rereview.sh must contain `{needle}`"
        );
    }
    assert!(
        executable_lines(&tool).all(|l| !runs_a_recipe(l)),
        "autodrive_crusty_rereview.sh must never start a recipe runner itself; its loop does, at its depth"
    );
    let crusty_loop = recipe_yaml("autodrive-crusty-loop");
    assert!(
        field(step(&crusty_loop, "step-03-record-crusty-phase"), "prompt")
            .contains("autodrive_record_crusty_loop_done"),
        "phase 2 and the re-review must record a DONE crusty loop through one function"
    );
    assert!(
        regex::Regex::new(r"(?m)^autodrive_record_crusty_loop_done\(\)\s*\{")
            .unwrap()
            .is_match(&read(&tool_path("autodrive_state.sh"))),
        "autodrive_state.sh must define autodrive_record_crusty_loop_done()"
    );

    // The driver runs the step before every round, in its own shell.
    let driver = read(&tool_path("autodrive_loop.sh"));
    for needle in [
        "--before-round)",
        "exec bash \"$BEFORE_ROUND\" --repo \"$REPO\" --review-dir \"$REVIEW_REAL\" --state-dir \"$STATE_DIR\"",
        "BEFORE_ROUND_FAILED",
        "before_round_result",
        "if terminal_refusal \"$BEFORE_RC\" \"$BEFORE_LOG\"",
    ] {
        assert!(
            driver.contains(needle),
            "autodrive_loop.sh must contain `{needle}`"
        );
    }
    let before_at = driver
        .find("exec bash \"$BEFORE_ROUND\"")
        .expect("before-round call");
    let round_at = driver
        .find("recipe run \"$ROUND_RECIPE_PATH\"")
        .expect("round call");
    assert!(
        before_at < round_at,
        "the before-round step must run before the round in each iteration"
    );
}

// Lines of shell that are not comments.
fn executable_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim_start().starts_with('#'))
}

// Does this line run `recipe run`? (`recipe runner` in prose does not.)
fn runs_a_recipe(line: &str) -> bool {
    regex::Regex::new(r#"recipe\s+run([\s"']|$)"#)
        .unwrap()
        .is_match(line)
}

// The recipes a recipe reaches through `type: recipe` steps, itself included.
fn recipes_reached_from(root: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut todo = vec![root.to_string()];
    while let Some(name) = todo.pop() {
        if seen.contains(&name) {
            continue;
        }
        for s in steps(&recipe_yaml(&name)) {
            if let Some(sub) = s.get("recipe").and_then(Value::as_str) {
                todo.push(sub.to_string());
            }
        }
        seen.push(name);
    }
    seen
}

// The auto-drive tools a shell text runs, through its executable lines only.
fn tools_run_by(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"autodrive_[a-z_]+\.sh").unwrap();
    let mut out: Vec<String> = Vec::new();
    for line in executable_lines(text) {
        for m in re.find_iter(line) {
            if !out.iter().any(|t| t == m.as_str()) {
                out.push(m.as_str().to_string());
            }
        }
    }
    out
}

#[test]
fn no_round_recipe_starts_a_loop_or_a_recipe_run() {
    // PR #1520 review. A round's steps run two session levels below the loop
    // driver that started the round: one for the round's runner, one for its
    // steps. With the default ceiling of 3 a `recipe run` from a round step is
    // refused with exit 79. So nothing a round reaches, through sub-recipes or
    // the tools its bash steps run, may start a loop or a recipe runner.
    let loop_recipes = [
        "auto-drive-to-merge",
        "autodrive-crusty-loop",
        "autodrive-merge-loop",
    ];
    let loop_starters = ["autodrive_loop.sh", "autodrive_crusty_rereview.sh"];
    for round in ["autodrive-crusty-round", "autodrive-merge-round"] {
        for reached in recipes_reached_from(round) {
            assert!(
                !loop_recipes.contains(&reached.as_str()),
                "{round} reaches the loop recipe {reached} through `type: recipe` steps"
            );
            for s in steps(&recipe_yaml(&reached)) {
                let id = s.get("id").and_then(Value::as_str).unwrap_or("?");
                if let Some(prompt) = s.get("prompt").and_then(Value::as_str) {
                    assert!(
                        !runs_a_recipe(prompt) && !loop_starters.iter().any(|t| prompt.contains(t)),
                        "{reached} step {id} (reached from {round}) tells its agent to start a recipe or a loop"
                    );
                }
                let Some(cmd) = s.get("command").and_then(Value::as_str) else {
                    continue;
                };
                for line in executable_lines(cmd) {
                    assert!(
                        !runs_a_recipe(line),
                        "{reached} step {id} (reached from {round}) runs `recipe run`: {line}"
                    );
                }
                let mut todo = tools_run_by(cmd);
                let mut seen: Vec<String> = Vec::new();
                while let Some(t) = todo.pop() {
                    if seen.contains(&t) {
                        continue;
                    }
                    assert!(
                        !loop_starters.contains(&t.as_str()),
                        "{reached} step {id} (reached from {round}) runs {t}, which starts a loop"
                    );
                    let path = tool_path(&t);
                    if path.is_file() {
                        let text = read(&path);
                        for line in executable_lines(&text) {
                            assert!(
                                !runs_a_recipe(line),
                                "{t}, run by {reached} step {id} (reached from {round}), runs `recipe run`: {line}"
                            );
                        }
                        todo.extend(tools_run_by(&text));
                    }
                    seen.push(t);
                }
            }
        }
    }
}

#[test]
fn a_recipe_run_from_a_round_step_is_refused_by_the_real_guard() {
    // The depths, from the recipe structure (docs/reference/auto-drive-to-merge.md,
    // "Recursion context propagation"): `amplihack recipe run` at depth d gives
    // its runner d + 1 (execute.rs, child_depth), the runner gives every step,
    // bash or agent, one more (recipe-runner-rs build_child_env), and a
    // `type: recipe` step runs inside the same runner. Both loop drivers are
    // `type: recipe` steps of the composer, so their bash runs as a step of
    // the composer's runner.
    let composer = recipe_yaml("auto-drive-to-merge");
    for (phase, loop_step) in [
        ("autodrive-crusty-loop", "step-02-crusty-loop"),
        ("autodrive-merge-loop", "step-02-merge-ready-loop"),
    ] {
        let s = steps(&composer)
            .iter()
            .find(|s| s.get("recipe").and_then(Value::as_str) == Some(phase))
            .unwrap_or_else(|| panic!("the composer must run {phase}"));
        assert_eq!(
            s.get("type").and_then(Value::as_str),
            Some("recipe"),
            "{phase} must run inside the composer's runner"
        );
        assert!(
            step_command(&recipe_yaml(phase), loop_step).contains("bash \"$LOOP_HELPER\""),
            "{phase} {loop_step} must run the loop driver in its own shell"
        );
    }
    const SHELL: u32 = 0; // a person starts auto-drive from a shell
    let composer_runner = SHELL + 1;
    let loop_step = composer_runner + 1; // autodrive_loop.sh, its --before-round step and the crusty re-review
    let round_runner = loop_step + 1;
    let round_step = round_runner + 1; // where step-00c's nested crusty loop ran

    let tmp = tempfile::tempdir().expect("tempdir");
    let trees = tmp.path().join("trees");
    let home = tmp.path().join("home");
    let tmpdir = tmp.path().join("tmp");
    for d in [&trees, &home, &tmpdir] {
        fs::create_dir_all(d).expect("mkdir");
    }
    const TREE: &str = "autodrivedepthtest";
    // The ceiling a root run seals: the default, 3.
    fs::write(
        trees.join(format!("{TREE}.json")),
        r#"{"sessions":{},"ceiling":3}"#,
    )
    .expect("seal the tree");
    // A stand-in recipe-runner-rs: it records the depth the guard handed it.
    let runner = tmp.path().join("recipe-runner-rs");
    let log = tmp.path().join("runner.log");
    fs::write(
        &runner,
        "#!/bin/sh\nif [ \"$1\" = --capabilities ]; then printf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}'; exit 0; fi\nprintf 'depth=%s\\n' \"${AMPLIHACK_SESSION_DEPTH:-}\" >> \"$RUNNER_LOG\"\nprintf '{}\\n'\n",
    )
    .expect("write the runner stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let run = |depth: u32| {
        crate::isolated_amplihack::isolated(env!("CARGO_BIN_EXE_amplihack"))
            .args(["recipe", "run"])
            .arg(recipe_path("autodrive-crusty-round"))
            .arg("--dry-run")
            .current_dir(tmp.path())
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("AMPLIHACK_HOME", workspace_root())
            .env("TMPDIR", &tmpdir)
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SESSION_TREE_DIR", &trees)
            .env("AMPLIHACK_TREE_ID", TREE)
            .env("AMPLIHACK_SESSION_DEPTH", depth.to_string())
            .env("AMPLIHACK_MAX_DEPTH", "3")
            .env("AMPLIHACK_MIN_AVAILABLE_MIB", "0")
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("RUNNER_LOG", &log)
            .output()
            .expect("run amplihack recipe run")
    };

    let refused = run(round_step);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        refused.status.code(),
        Some(79),
        "a crusty round started from a round step (depth {round_step}) must be refused with exit 79:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "BLOCKED_TERMINAL orchestration_unavailable: depth {round_step} of max 3"
        )),
        "the refusal must name the depth:\n{stderr}"
    );
    assert!(!log.exists(), "no runner may start past the ceiling");

    let admitted = run(loop_step);
    let stderr = String::from_utf8_lossy(&admitted.stderr);
    assert!(
        !stderr.contains("BLOCKED_TERMINAL"),
        "a crusty round started from the loop driver's depth ({loop_step}) must be admitted:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap_or_default().trim(),
        format!("depth={round_runner}"),
        "the guard must start the crusty round's runner at depth {round_runner}:\n{stderr}"
    );
}

#[test]
fn a_round_log_that_quotes_the_refusal_is_not_a_refusal() {
    // crusty's review of PR #1520 quoted the guard's refusal, and an agent in
    // #1437 handled a descendant's refusal inline and then lost its session.
    // Reading BLOCKED_TERMINAL anywhere in a transcript ended both runs with
    // exit 79. Only the exit code and the runner's own failure_class marker
    // decide, and every call site asks the same function.
    let helper = read(&tool_path("autodrive_refusal.sh"));
    let body = helper
        .split("terminal_refusal() {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("terminal_refusal() must be defined in autodrive_refusal.sh");
    assert!(
        !body.contains("BLOCKED_TERMINAL"),
        "terminal_refusal must not look for the refusal's text in the transcript"
    );
    assert!(body.contains("79"), "terminal_refusal must test exit 79");
    assert!(
        body.contains("structural_refusal"),
        "terminal_refusal must read structural_refusal from the failure_class marker"
    );
    assert!(
        !helper.contains("eval "),
        "autodrive_refusal.sh must not eval anything it read from a log"
    );

    let driver = read(&tool_path("autodrive_loop.sh"));
    let code: Vec<&str> = driver
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect();
    assert!(
        !code
            .iter()
            .any(|line| line.contains("terminal_refusal() {")),
        "terminal_refusal is defined once, in autodrive_refusal.sh"
    );
    assert!(
        code.iter()
            .any(|line| line.contains(". \"${LOOP_HOME}/autodrive_refusal.sh\"")),
        "autodrive_loop.sh must source autodrive_refusal.sh from beside itself"
    );
    assert!(
        !code.iter().any(|line| line.contains("BLOCKED_TERMINAL")),
        "autodrive_loop.sh must not match the refusal's text"
    );
    let calls: Vec<&&str> = code
        .iter()
        .filter(|line| line.contains("terminal_refusal \""))
        .collect();
    assert_eq!(calls.len(), 3, "three call sites: {calls:?}");
    for log in ["\"$BEFORE_LOG\"", "\"$LOG\"", "\"$HEALTH_LOG\""] {
        assert!(
            calls.iter().any(|line| line.contains(log)),
            "terminal_refusal must be called with {log}: {calls:?}"
        );
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
    // autodrive_trust_git unsets every name of AUTODRIVE_TRUST_GIT_UNSET_VARS
    // with `env -u` (#2033); tests/issue_1278_git_env_isolation.sh checks that
    // list against git-repo-env.sh and runs git under it.
    let unset_list = regex::Regex::new(r#"(?m)^AUTODRIVE_TRUST_GIT_UNSET_VARS="([A-Z_ ]+)"$"#)
        .unwrap()
        .captures(&trust)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    assert!(
        trust.contains(
            r#"for v in $AUTODRIVE_TRUST_GIT_UNSET_VARS; do unset_args+=(-u "$v"); done"#
        ) && trust.contains(r#"env "${unset_args[@]}""#),
        "autodrive_trust_git must pass every AUTODRIVE_TRUST_GIT_UNSET_VARS name to `env -u`"
    );
    for var in ["GIT_DIR", "GIT_WORK_TREE"] {
        assert!(
            unset_list.split(' ').any(|v| v == var),
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
fn qa_inputs_tool_loads_its_helpers_from_its_own_directory() {
    let tool = read(&tool_path("autodrive_qa_evidence.sh"));
    for needle in ["autodrive_trust.sh", "umask 077", "mv -f"] {
        assert!(
            tool.contains(needle),
            "autodrive_qa_evidence.sh must reference `{needle}`"
        );
    }
    // The trust helpers come from beside the tool, never from a search a
    // pull request could populate.
    assert!(
        tool.contains("dirname \"${BASH_SOURCE[0]}\"") && !tool.contains("for c in"),
        "autodrive_qa_evidence.sh must source autodrive_trust.sh from its own directory only"
    );
}

// recipe-runner 0.3.8 (`context.rs`, `shell_env_vars`) exports a step output
// that parsed as a JSON object as `RECIPE_VAR_<name>` only; the bare
// upper-case alias is added for scalars alone. A round body that reads
// `${QA_EVIDENCE:-}` therefore sees "" under the real runner, and since #1561
// an inherited bare `QA_EVIDENCE` must not be read at all: every bash read of
// a step output in the two round recipes and the evidence tool is
// `RECIPE_VAR_<name>` alone. The loop recipes' reads are #1511's.
#[test]
fn round_recipes_read_step_outputs_through_recipe_var() {
    // The round recipes' outputs, plus merge-evidence's: the runner merges a
    // sub-recipe's context back into the merge round.
    let mut outputs: Vec<String> = Vec::new();
    for r in [
        "autodrive-crusty-round",
        "autodrive-merge-round",
        "autodrive-merge-evidence",
    ] {
        for line in recipe_text(r).lines() {
            if let Some(name) = line
                .trim_start()
                .strip_prefix("output: \"")
                .and_then(|s| s.strip_suffix('"'))
            {
                outputs.push(name.to_string());
            }
        }
    }
    for name in [
        "crusty_round_context",
        "crusty_evidence",
        "qa_evidence_hash",
        "platform_facts",
        "qa_evidence",
    ] {
        assert!(
            outputs.iter().any(|o| o == name),
            "expected step output `{name}` in the round recipes"
        );
    }
    let mut reads = 0;
    let mut bad = Vec::new();
    // autodrive_round_evidence.sh holds merge-round step bodies (PR #1520
    // review), and reads the same outputs from the same environment.
    let sources = [
        (
            "autodrive-crusty-round.yaml",
            recipe_text("autodrive-crusty-round"),
        ),
        (
            "autodrive-merge-round.yaml",
            recipe_text("autodrive-merge-round"),
        ),
        (
            "autodrive_round_evidence.sh",
            read(&tool_path("autodrive_round_evidence.sh")),
        ),
    ];
    for (r, text) in &sources {
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            for name in &outputs {
                reads += bounded_matches(line, &format!("${{RECIPE_VAR_{name}"))
                    .filter(|&end| line[end..].starts_with(":-"))
                    .count();
                // QA_EVIDENCE inside QA_EVIDENCE_HASH or AUTODRIVE_QA_EVIDENCE
                // is not a read: the matcher checks the name boundary and the
                // `$` before it.
                if reads_the_bare_name(line, &name.to_ascii_uppercase())
                    || recipe_var_read_has_a_bare_tail(line, name)
                {
                    bad.push(format!("{r}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        reads >= 18,
        "found only {reads} step-output reads in the round recipes; the scan is broken"
    );
    bad.dedup();
    assert!(
        bad.is_empty(),
        "step-output reads that let an inherited bare variable win (#1561, #1607); \
         write \"${{RECIPE_VAR_<output>:-<default>}}\" and no bare name:\n{}",
        bad.join("\n")
    );
}

// ── The gadugi harnesses' step reader ────────────────────────────────────────

// The gadugi harnesses and the recipe step each one runs. Criterion 1 runs
// these scenarios with the real gadugi-test, so the harnesses read the
// shipped step body with `tests/gadugi/recipe-step-command.sh` (awk) rather
// than python3 and PyYAML.
const HARNESS_STEPS: [(&str, &str, &str); 3] = [
    (
        "run-step-03b.sh",
        "workflow-prep",
        "step-03b-extract-issue-number",
    ),
    (
        "run-merge-validations.sh",
        "quality-audit-cycle",
        "merge-validations",
    ),
    (
        "run-autodrive-merge-ready-files-scenario.sh",
        "autodrive-merge-round",
        "step-00-merge-ready-files",
    ),
];

fn step_command_script() -> PathBuf {
    workspace_root().join("tests/gadugi/recipe-step-command.sh")
}

// Runs recipe-step-command.sh; returns (exit code, stdout, stderr).
fn run_step_command(recipe: &Path, step_id: &str) -> (Option<i32>, String, String) {
    let out = Command::new("bash")
        .arg(step_command_script())
        .arg(recipe)
        .arg(step_id)
        .output()
        .expect("run recipe-step-command.sh");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn recipe_step_command_matches_serde_yaml_for_every_harness_step() {
    let gadugi = workspace_root().join("tests/gadugi");
    // Every read of the extractor in a harness must be listed above, so a new
    // harness cannot use it without this comparison.
    let mut calls = 0;
    for entry in fs::read_dir(&gadugi).expect("read tests/gadugi") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("sh")
            || path.file_name().and_then(|n| n.to_str()) == Some("recipe-step-command.sh")
        {
            continue;
        }
        calls += read(&path)
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter(|l| l.contains("recipe-step-command.sh"))
            .count();
    }
    assert_eq!(
        calls,
        HARNESS_STEPS.len(),
        "tests/gadugi has {calls} call(s) to recipe-step-command.sh but HARNESS_STEPS \
         lists {}; add the new step so its output is compared with serde_yaml",
        HARNESS_STEPS.len()
    );
    for (harness, recipe, step_id) in HARNESS_STEPS {
        let text = read(&gadugi.join(harness));
        assert!(
            text.contains(&format!("amplifier-bundle/recipes/{recipe}.yaml")),
            "{harness} no longer reads {recipe}.yaml; update HARNESS_STEPS"
        );
        assert!(
            text.contains(&format!("recipe-step-command.sh\" \"$RECIPE\" {step_id})")),
            "{harness} no longer reads step `{step_id}`; update HARNESS_STEPS"
        );
        let parsed = recipe_yaml(recipe);
        let expected = field(step(&parsed, step_id), "command");
        let (code, stdout, stderr) = run_step_command(&recipe_path(recipe), step_id);
        assert_eq!(
            code,
            Some(0),
            "recipe-step-command.sh refused {recipe}.yaml step `{step_id}`: {stderr}"
        );
        assert_eq!(
            stdout, expected,
            "recipe-step-command.sh and serde_yaml disagree on {recipe}.yaml step `{step_id}`"
        );
    }
}

// The extractor reads a subset of YAML by hand. Over every step of every
// bundled recipe it must print exactly what serde_yaml reads, or refuse with
// exit 2 and a named ERROR. It must never print a different body.
#[test]
fn recipe_step_command_matches_serde_yaml_or_refuses_for_every_bundled_step() {
    let dir = workspace_root().join("amplifier-bundle/recipes");
    let mut recipes: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read amplifier-bundle/recipes")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .collect();
    recipes.sort();
    let (mut matched, mut refused) = (0usize, 0usize);
    let mut wrong = Vec::new();
    for path in &recipes {
        let recipe: Value = serde_yaml::from_str(&read(path))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        let Some(all) = recipe.get("steps").and_then(Value::as_sequence) else {
            continue;
        };
        for s in all {
            let (Some(id), Some(cmd)) = (
                s.get("id").and_then(Value::as_str),
                s.get("command").and_then(Value::as_str),
            ) else {
                continue;
            };
            let name = path.file_name().unwrap().to_string_lossy();
            match run_step_command(path, id) {
                (Some(0), stdout, _) if stdout == cmd => matched += 1,
                (Some(2), _, stderr) if stderr.starts_with("ERROR: ") => refused += 1,
                (code, stdout, stderr) => wrong.push(format!(
                    "{name} step `{id}`: exit {code:?}, {} bytes printed, {} expected; {}",
                    stdout.len(),
                    cmd.len(),
                    stderr.trim()
                )),
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "recipe-step-command.sh printed a body serde_yaml does not read:\n{}",
        wrong.join("\n")
    );
    assert!(
        matched >= 150,
        "only {matched} bundled steps matched serde_yaml ({refused} refused); \
         the extractor or this scan is broken"
    );
}

#[test]
fn recipe_step_command_refuses_what_it_does_not_read() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let recipe = tmp.path().join("r.yaml");
    fs::write(
        &recipe,
        "name: r\nsteps:\n  - id: \"strip\"\n    command: |-\n      echo a\n  \
         - id: \"folded\"\n    command: >\n      echo a\n  \
         - id: \"quoted\"\n    command: \"echo a\"\n  \
         - id: \"agent\"\n    prompt: |\n      command: |\n        echo a\n  \
         - id: \"empty\"\n    command: |\n  \
         - id: \"ok\"\n    command: |\n      echo a\n\n      echo b\n\n\n",
    )
    .expect("write recipe");
    for (id, error) in [
        ("strip", "command-is-not-a-literal-block"),
        ("folded", "command-is-not-a-literal-block"),
        ("quoted", "command-is-not-a-literal-block"),
        ("agent", "step-has-no-command"),
        ("empty", "command-block-is-empty"),
        ("missing", "step-not-found"),
    ] {
        let (code, stdout, stderr) = run_step_command(&recipe, id);
        assert_eq!(code, Some(2), "step `{id}` must be refused, got: {stdout}");
        assert!(stdout.is_empty(), "a refused step `{id}` printed: {stdout}");
        assert!(
            stderr.starts_with(&format!("ERROR: {error}: {id}")),
            "step `{id}` must be refused with `{error}`, got: {stderr}"
        );
    }
    let (code, stdout, _) = run_step_command(&recipe, "ok");
    assert_eq!(code, Some(0));
    assert_eq!(
        stdout, "echo a\n\necho b\n",
        "clip chomping keeps one newline"
    );
    let parsed: Value = serde_yaml::from_str(&read(&recipe)).expect("parse fixture");
    assert_eq!(field(step(&parsed, "ok"), "command"), stdout);
}

// Every state_dir guard names the recipe and step it runs in, using the
// recipe's full name: `[autodrive-merge-loop/step-03]`, as
// docs/reference/auto-drive-to-merge.md shows it. A short label such as
// `[merge-loop/step-03]` names no recipe a user can run or search for.
//
// The case patterns are pinned with it, because the label is the only part
// of a guard that may change. autodrive-build's empty-is-INFO branch is the
// documented exception: a standalone build with no preflight has nothing to
// record.

// ── The executable contract ──────────────────────────────────────────────────

// True when `s[end..]` cannot continue a name: it is empty or starts with a
// character other than an ASCII letter, digit or `_`. So `QA` never matches
// `QA_EVIDENCE`, and `qa` never matches `qa_evidence`.
fn boundary_after(s: &str, end: usize) -> bool {
    s[end..]
        .chars()
        .next()
        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
}

// The offset just past each occurrence of `pat` in `line` that ends at a
// name boundary.
fn bounded_matches<'a>(line: &'a str, pat: &'a str) -> impl Iterator<Item = usize> + 'a {
    line.match_indices(pat)
        .map(move |(i, _)| i + pat.len())
        .filter(move |&end| boundary_after(line, end))
}

// Static check 1: `line` reads the bare name `upper` in any form — `${X`
// with or without a default, `${#X`, `${!X` or `$X`. Any of these can read a
// value inherited from outside the run instead of `RECIPE_VAR_<x>` (#1561).
fn reads_the_bare_name(line: &str, upper: &str) -> bool {
    [
        format!("${{{upper}"),
        format!("${{#{upper}"),
        format!("${{!{upper}"),
        format!("${upper}"),
    ]
    .iter()
    .any(|pat| bounded_matches(line, pat).next().is_some())
}

// Static check 2: a `${RECIPE_VAR_<name>` read in `line` has no `:-` default,
// or its default is a bare read of the same output, so an empty step output
// would fall through to an inherited value (`${RECIPE_VAR_x:-${X:-}}`).
fn recipe_var_read_has_a_bare_tail(line: &str, name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    bounded_matches(line, &format!("${{RECIPE_VAR_{name}")).any(|end| {
        let Some(rest) = line[end..].strip_prefix(":-") else {
            return true;
        };
        let rest = rest.trim_start_matches('"');
        [format!("${{{upper}"), format!("${upper}")]
            .iter()
            .any(|pat| rest.starts_with(pat.as_str()) && boundary_after(rest, pat.len()))
    })
}

// Static check 3: `command` templates the step output `name` — `{{`, optional
// spaces, then the name at a boundary. `{{qa_evidence}}`,
// `{{ qa_evidence }}` and `{{platform_facts.pr}}` all count.
fn templates_output(command: &str, name: &str) -> bool {
    command.match_indices('{').any(|(i, _)| {
        command[i + 1..].strip_prefix('{').is_some_and(|rest| {
            let rest = rest.trim_start_matches([' ', '\t']);
            rest.starts_with(name) && boundary_after(rest, name.len())
        })
    })
}

// The read table under "Reading step outputs" in the reference: the count per
// recipe stem, and the total stated on the one line that contains both
// `There are ` and ` in all`. Any shape it does not recognise is an error,
// never an empty table, so check 4 cannot pass by reading nothing.
fn parse_read_table(doc: &str) -> Result<(BTreeMap<String, usize>, usize), String> {
    let lines: Vec<&str> = doc.lines().collect();
    let total_lines: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].contains("There are ") && lines[i].contains(" in all"))
        .collect();
    let [at] = total_lines[..] else {
        return Err(format!(
            "expected exactly one line containing `There are ` and ` in all`, found {}",
            total_lines.len()
        ));
    };
    let stated = lines[at]
        .split_once("There are ")
        .and_then(|(_, rest)| rest.split_once(" in all"))
        .map_or("", |(n, _)| n.trim());
    let total: usize = stated.parse().map_err(|_| {
        format!(
            "line {}: the stated total `{stated}` is not a number",
            at + 1
        )
    })?;
    if total == 0 {
        return Err(format!("line {}: the stated total is 0", at + 1));
    }

    // Skip the blank line, header and separator; the rows end at the first
    // line that does not start with `|`.
    let rows = lines[at + 1..]
        .iter()
        .skip_while(|l| l.trim().is_empty() || (l.starts_with('|') && !l.starts_with("| `")))
        .take_while(|l| l.starts_with('|'));
    let mut counts = BTreeMap::new();
    for row in rows {
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        let ["", name, count, ""] = cells[..] else {
            return Err(format!(
                "row `{row}` is not `| `<recipe>.yaml` | <count> |`"
            ));
        };
        let name = name.trim_matches('`');
        let stem = name.strip_suffix(".yaml").unwrap_or(name);
        let n: usize = count
            .parse()
            .map_err(|_| format!("row `{row}`: the count `{count}` is not a number"))?;
        if counts.insert(stem.to_string(), n).is_some() {
            return Err(format!("`{stem}` appears in two rows"));
        }
    }
    if counts.is_empty() {
        return Err(format!("no table row follows line {}", at + 1));
    }
    let sum: usize = counts.values().sum();
    if sum != total {
        return Err(format!(
            "the rows add up to {sum}, but line {} states {total}",
            at + 1
        ));
    }
    Ok((counts, total))
}

// The documented step-output read counts. The reference is the only source of
// truth; this file holds no count of its own.

// The matchers behind static checks 1, 2 and 3, against the forms the
// reference lists and the near misses the name boundary must reject.
#[test]
fn step_output_read_matchers_respect_the_name_boundary() {
    for line in [
        // The #1511 dual read is a violation since #1561: the bare name wins.
        "${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}",
        "${QA_EVIDENCE:-}",
        "${QA_EVIDENCE}",
        "${QA_EVIDENCE-}",
        "${QA_EVIDENCE:=}",
        "${QA_EVIDENCE:+x}",
        "${QA_EVIDENCE:?}",
        "${QA_EVIDENCE#*}",
        "${QA_EVIDENCE%x}",
        "${QA_EVIDENCE/a/b}",
        "${#QA_EVIDENCE}",
        "${!QA_EVIDENCE}",
        "$QA_EVIDENCE",
        "printf '%s' \"$QA_EVIDENCE\" | amplihack orch helper extract-json",
        // A good read does not excuse a bad one on the same line.
        "A=\"${RECIPE_VAR_qa_evidence:-}\" B=\"$QA_EVIDENCE\"",
    ] {
        assert!(
            reads_the_bare_name(line, "QA_EVIDENCE"),
            "`{line}` reads the bare name and must be a violation"
        );
    }
    for line in [
        "QA=\"${RECIPE_VAR_qa_evidence:-}\"",
        "QA=\"${RECIPE_VAR_qa_evidence:-MISSING}\"",
        "${QA_EVIDENCE_2}",
        "$QA_EVIDENCE_X",
        "${#QA_EVIDENCE_X}",
        "${!QA_EVIDENCE_X}",
        "${RECIPE_VAR_qa_evidence:-}",
        "${AUTODRIVE_QA_EVIDENCE:-}",
        "# QA_EVIDENCE is declared in autodrive-merge-evidence.yaml",
    ] {
        assert!(
            !reads_the_bare_name(line, "QA_EVIDENCE"),
            "`{line}` must not be a violation"
        );
    }
    assert!(
        !reads_the_bare_name("${QA_EVIDENCE} $QA_EVIDENCE", "QA"),
        "`QA` must not match `QA_EVIDENCE`"
    );

    for line in [
        "${RECIPE_VAR_qa_evidence:-${QA_EVIDENCE:-}}",
        "${RECIPE_VAR_qa_evidence:-$QA_EVIDENCE}",
        "${RECIPE_VAR_qa_evidence:-\"${QA_EVIDENCE:-}\"}",
        "${RECIPE_VAR_qa_evidence}",
        "${RECIPE_VAR_qa_evidence-}",
        "${RECIPE_VAR_qa_evidence:=}",
        "A=\"${RECIPE_VAR_qa_evidence:-}\" B=\"${RECIPE_VAR_qa_evidence}\"",
    ] {
        assert!(
            recipe_var_read_has_a_bare_tail(line, "qa_evidence"),
            "`{line}` lets an empty step output fall through and must be a violation"
        );
    }
    for line in [
        "${RECIPE_VAR_qa_evidence:-}",
        "${RECIPE_VAR_qa_evidence:-MISSING}",
        "${RECIPE_VAR_qa_evidence:-${QA_EVIDENCE_HASH:-}}",
        "${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-}}",
        "${RECIPE_VAR_qa_evidence_hash}",
        "${RECIPE_VAR_qa_evidence_hash:-}",
    ] {
        assert!(
            !recipe_var_read_has_a_bare_tail(line, "qa_evidence"),
            "`{line}` must not be a violation"
        );
    }

    for (command, name) in [
        ("echo {{qa_evidence}}", "qa_evidence"),
        ("echo {{ qa_evidence }}", "qa_evidence"),
        ("echo {{\tqa_evidence}}", "qa_evidence"),
        ("gh pr view {{platform_facts.pr}}", "platform_facts"),
        ("echo {{{qa_evidence}}}", "qa_evidence"),
        ("echo — {{qa_evidence}}", "qa_evidence"),
    ] {
        assert!(
            templates_output(command, name),
            "`{command}` templates `{name}` and must be caught"
        );
    }
    for (command, name) in [
        ("echo {{qa_evidence_x}}", "qa_evidence"),
        ("echo {{qa_evidence}}", "qa"),
        ("{qa_evidence}", "qa_evidence"),
        ("echo {{ repo_path }}", "qa_evidence"),
        ("QA=\"${RECIPE_VAR_qa_evidence:-}\"", "qa_evidence"),
        ("echo qa_evidence }}", "qa_evidence"),
    ] {
        assert!(
            !templates_output(command, name),
            "`{command}` does not template `{name}`"
        );
    }
}

// Static check 4 takes its numbers from the reference, so the table parse
// must fail closed on every shape it does not recognise.
#[test]
fn read_table_parse_fails_closed() {
    const TABLE: &str = "\n| Recipe | Step-output reads |\n| --- | --- |\n\
                         | `a.yaml` | 2 |\n| `b.yaml` | 1 |\n\n| `c.yaml` | 9 |\n";

    let (counts, total) = parse_read_table(&format!("Reads. There are 3 in all:\n{TABLE}"))
        .unwrap_or_else(|e| panic!("a well-formed table must parse: {e}"));
    assert_eq!(total, 3);
    assert_eq!(
        counts,
        BTreeMap::from([("a".to_string(), 2), ("b".to_string(), 1)]),
        "the rows end at the first line that does not start with `|`"
    );

    for (case, doc, expected) in [
        ("no total line", TABLE.to_string(), "found 0"),
        (
            "two total lines",
            format!("There are 3 in all:\nThere are 3 in all:\n{TABLE}"),
            "found 2",
        ),
        (
            "total not a number",
            format!("There are three in all:\n{TABLE}"),
            "stated total `three` is not a number",
        ),
        (
            "total is 0",
            format!("There are 0 in all:\n{TABLE}"),
            "stated total is 0",
        ),
        (
            "no row after the total",
            "There are 3 in all:\n\n| Recipe | Step-output reads |\n| --- | --- |\n\nProse.\n"
                .to_string(),
            "no table row follows",
        ),
        (
            "row count not a number",
            "There are 3 in all:\n\n| `a.yaml` | two |\n".to_string(),
            "the count `two` is not a number",
        ),
        (
            "row in the wrong shape",
            "There are 3 in all:\n\n| `a.yaml` | 3\n".to_string(),
            "is not `|",
        ),
        (
            "recipe in two rows",
            "There are 3 in all:\n\n| `a.yaml` | 2 |\n| `a.yaml` | 1 |\n".to_string(),
            "`a` appears in two rows",
        ),
        (
            "rows do not add up",
            format!("There are 4 in all:\n{TABLE}"),
            "add up to 3",
        ),
    ] {
        match parse_read_table(&doc) {
            Ok(parsed) => panic!("{case}: must fail closed, parsed {parsed:?}"),
            Err(e) => assert!(
                e.contains(expected),
                "{case}: expected an error containing `{expected}`, got `{e}`"
            ),
        }
    }
}

// Issue #1511. recipe-runner-rs exports every step output as
// `RECIPE_VAR_<output>` and adds the bare upper-case alias only for SCALAR
// outputs. Every output these recipes pass between steps is a JSON object, so
// a bare `${CRUSTY_LOOP_PREFLIGHT:-}` read is always empty — every loop lost
// its state_dir and every run stopped as STUCK.
//
// Issue #1561 / #1607. The #1511 fix read `${UPPER:-${RECIPE_VAR_x:-}}`, so a
// bare `UPPER` inherited from whatever launched the runner outranked the
// recipe's own value. Every read is now `${RECIPE_VAR_<output>:-<default>}`
// alone, with no bare read anywhere and no bare tail.
//
// Output names are collected across ALL autodrive recipes into one set: a
// recipe reads outputs its sub-recipes declare (`autodrive-merge-round` reads
// `QA_EVIDENCE`, declared in `autodrive-merge-evidence`).
//
// Enforces "The static check" in docs/reference/auto-drive-to-merge.md: two
// sanity checks, then checks 1–4 in the order the reference gives them.

// What auto-drive's own qa-team step hands this suite: the recipe runner
// exports step outputs as AUTODRIVE_* and RECIPE_VAR_* names. PR #1588's
// round-1 QA failed MERGEREADY-no-round-tool because an inherited
// AUTODRIVE_TOOLS_DIR outranked the case's own tools dir. Injecting it on
// every run proves the shell tests start from none of the caller's context.
fn inherited_autodrive_context(root: &Path) -> Vec<(String, String)> {
    let tools = root.join("amplifier-bundle/tools").display().to_string();
    vec![
        ("AUTODRIVE_TOOLS_DIR".to_string(), tools.clone()),
        ("RECIPE_VAR_autodrive_tools_dir".to_string(), tools),
        ("AUTODRIVE_ROUND_LABEL".to_string(), "round-1".to_string()),
    ]
}

// Runs the executable contract test — the STUCK path, the malformed-verdict
// path, and the forbidden-flag guard, exercised against the real extracted
// step bodies and the real tools.
//
// Wired here because `.github/workflows/ci.yml` lists the bash recipe tests
// one by one; running it from `cargo test` gets the same coverage without
// touching that file.
//
// It runs under the #1551 gh merge guard's environment, the one
// `merge_guard::apply_guard` exports to every bash of an auto-drive run: the
// guard directory first on `PATH`, `BASH_ENV` at the shipped `bash-env`, and
// `AMPLIHACK_MERGE_GUARD=1`. That is where this test runs as phase-1 tests and
// as the merge round's QA, and where it failed twice in PR #1644 crusty round
// 11. The script drops the guard at its top, so a run without the guard
// covers nothing this one does not. The other two runs keep the injected
// context instead of letting the script remove it; see [`Inherited`].
#[test]
fn auto_drive_contract_shell_test_passes() {
    run_auto_drive_contract_shell_test(Inherited::Scrubbed);
}

// The same suite with `AUTODRIVE_TEST_KEEP_INHERITED=1` and an inherited
// `AUTODRIVE_TOOLS_DIR` that points somewhere else than the round's own
// `RECIPE_VAR_autodrive_tools_dir` (#1607). Every case must still pass,
// because no step body reads an inherited bare name (#1561). The #1551 merge
// guard and the gh stubs stay in place, so no case can merge a real PR.
#[test]
fn auto_drive_contract_shell_test_passes_with_conflicting_inherited_tools_dir() {
    run_auto_drive_contract_shell_test(Inherited::Conflicting);
}

// The #1607 reproduction: the outer run's tools directory inherited as both
// `AUTODRIVE_TOOLS_DIR` and `RECIPE_VAR_autodrive_tools_dir`, as a nested
// auto-drive round saw them. Every case sets the tools dir it needs, so the
// suite must still pass, MERGEREADY-no-round-tool included.
#[test]
fn auto_drive_contract_shell_test_passes_with_inherited_context() {
    run_auto_drive_contract_shell_test(Inherited::Same);
}

// What the contract shell test inherits from the process that starts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inherited {
    /// The script removes every `AUTODRIVE_*` / `RECIPE_VAR_*` name first.
    Scrubbed,
    /// Kept (`AUTODRIVE_TEST_KEEP_INHERITED=1`): `AUTODRIVE_TOOLS_DIR` is a
    /// decoy whose tools print a sentinel and exit 97, and
    /// `RECIPE_VAR_autodrive_tools_dir` is the real tools directory.
    Conflicting,
    /// Kept: both names are the real tools directory (#1607 as observed).
    Same,
}

// Sentinel printed by every decoy tool of [`Inherited::Conflicting`].
const DECOY_1561_SENTINEL: &str = "DECOY-1561-INHERITED-TOOLS-DIR";

// A tools directory holding a decoy for every tool the real one ships. Each
// decoy prints [`DECOY_1561_SENTINEL`] and exits 97: no network, no gh, no
// git writes. Created under `CARGO_TARGET_TMPDIR`, never under /tmp.
#[cfg(unix)]
fn decoy_tools_dir(root: &Path) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .prefix("decoy-1561-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("decoy tools dir under CARGO_TARGET_TMPDIR");
    let real = root.join("amplifier-bundle/tools");
    let mut n = 0;
    for e in fs::read_dir(&real)
        .expect("read amplifier-bundle/tools")
        .flatten()
    {
        let name = e.file_name();
        if !e.path().is_file() || !name.to_string_lossy().ends_with(".sh") {
            continue;
        }
        let p = dir.path().join(&name);
        fs::write(
            &p,
            format!(
                "printf '%s\\n' '{DECOY_1561_SENTINEL}' >&2\n\
                 printf '%s\\n' '{DECOY_1561_SENTINEL}'\n\
                 return 97 2>/dev/null || exit 97\n"
            ),
        )
        .expect("write decoy tool");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod decoy tool");
        n += 1;
    }
    assert!(
        n >= 10,
        "only {n} decoy tools written; the conflicting run would prove nothing"
    );
    dir
}

fn run_auto_drive_contract_shell_test(inherited: Inherited) {
    let root = workspace_root();
    let script = root.join("amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh");
    assert!(script.is_file(), "missing {}", script.display());

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    let bin_dir = bin.parent().expect("binary parent dir");
    let path = match std::env::var("PATH") {
        Ok(p) => format!("{}:{p}", bin_dir.display()),
        Err(_) => bin_dir.display().to_string(),
    };
    // The script's resolver cases need scratch space outside every git work
    // tree. CARGO_TARGET_TMPDIR is inside this checkout, so pass TMPDIR
    // explicitly, falling back to the system temp dir when it is unset.
    let tmpdir = std::env::var_os("TMPDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    let decoy = (inherited == Inherited::Conflicting).then(|| decoy_tools_dir(&root));
    let mut context = inherited_autodrive_context(&root);
    if let Some(d) = &decoy {
        for (k, v) in context.iter_mut() {
            if k == "AUTODRIVE_TOOLS_DIR" {
                *v = d.path().display().to_string();
            }
        }
    }

    let mut cmd = Command::new("bash");
    cmd.arg(&script)
        .current_dir(&root)
        .env_remove("AMPLIHACK_MERGE_GUARD_USER_BASH_ENV")
        .env("PATH", path)
        .env("TMPDIR", &tmpdir)
        .env_remove("BASH_ENV")
        .env_remove("AMPLIHACK_MERGE_GUARD");
    if inherited != Inherited::Scrubbed {
        cmd.envs(context).env("AUTODRIVE_TEST_KEEP_INHERITED", "1");
    } else {
        cmd.envs(context);
    }
    let out = cmd
        .output()
        .expect("run the auto-drive contract shell test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "test-auto-drive-to-merge.sh failed (inherited={inherited:?}, {:?})\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        out.status.code(),
    );
    let failed: Vec<&str> = stdout
        .lines()
        .chain(stderr.lines())
        .filter(|l| l.trim_start().starts_with("FAIL["))
        .collect();
    assert!(
        failed.is_empty(),
        "test-auto-drive-to-merge.sh reported failures (inherited={inherited:?}):\n{}",
        failed.join("\n")
    );
    assert!(
        stdout.contains("PASS: agentic recipes parse"),
        "MERGEREADY-no-round-tool did not pass (inherited={inherited:?}); a round with no \
         round tool must not find one through an inherited AUTODRIVE_TOOLS_DIR (#1607)\n{stdout}"
    );
    if inherited == Inherited::Conflicting {
        assert!(
            !stdout.contains(DECOY_1561_SENTINEL) && !stderr.contains(DECOY_1561_SENTINEL),
            "a step ran a tool from the inherited AUTODRIVE_TOOLS_DIR decoy (#1561, #1607)"
        );
    }
}

// ── The squash commit message (#2056) ──────────────────────────────────────

// #2329 retired the shell merge gate whose suite carried #2262's MSG-* cases.
// The message is now composed by autodrive_merge_message.sh, sourced by
// autodrive-finalize's finalize-observed-action agent step. This runs the
// helper-level suite and requires the cases #2262 named, so they cannot drop
// out unnoticed.
#[test]
fn the_squash_message_suite_passes_and_keeps_its_named_cases() {
    let root = workspace_root();
    let script = root.join("amplifier-bundle/recipes/tests/test-issue-2056-squash-message.sh");
    assert!(script.is_file(), "missing {}", script.display());
    let out = Command::new("bash")
        .arg(&script)
        .current_dir(&root)
        .env_remove("BASH_ENV")
        .output()
        .expect("run the squash message suite");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "test-issue-2056-squash-message.sh failed ({:?})\n{stdout}\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    for case in [
        "MSG-report-cut-at-marker",
        "MSG-report-cut-at-heading",
        "MSG-template-marker",
        "MSG-control-chars",
        "MSG-unreadable-body",
        "MSG-no-jq-refuses",
        "MSG-finalize-prompt",
    ] {
        assert!(
            stdout.contains(&format!("PASS[{case}]")),
            "{case} did not pass; the squash message must keep #2056's contract\n{stdout}"
        );
    }
}

#[test]
fn finalize_supplies_the_squash_message_instead_of_the_platform_default() {
    let finalizer = recipe_yaml("autodrive-finalize");
    let prompt = field(step(&finalizer, "finalize-observed-action"), "prompt");
    for needle in [
        "issue #2056",
        "autodrive_merge_message.sh",
        "never a copy from the PR's tree",
        // An absolute launch-bundle path: the step runs in the PR worktree,
        // so a repo-relative path would source the PR's own copy.
        "MM=\"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_message.sh\"",
        "[ ! -L \"$MM\" ] && . \"$MM\"",
        "case \"${AMPLIHACK_HOME:-}\" in /*) ;;",
        "--subject \"$subject\" --body-file -",
        "printf '%s' \"$MERGE_BODY\" | gh pr merge",
    ] {
        assert!(
            prompt.contains(needle),
            "finalize-observed-action must say `{needle}` (#2056)"
        );
    }
}

// ── Which `amplihack` the shell test runs ────────────────────────────────────

// Writes a fake `amplihack` for the shell test's probe, which needs
// `orch helper extract-field` and `orch helper extract-json --require-field`.
// A fake without `--require-field` stands in for a build from before #1347.
#[cfg(unix)]
fn write_fake_amplihack(path: &Path, has_require_field: bool) {
    use std::os::unix::fs::PermissionsExt;
    let require_field_rc = if has_require_field { 0 } else { 1 };
    fs::create_dir_all(path.parent().expect("fake parent dir")).expect("create fake dir");
    fs::write(
        path,
        format!(
            "#!/usr/bin/env bash\n\
             [ \"${{1:-}} ${{2:-}}\" = \"orch helper\" ] || exit 1\n\
             case \"${{3:-}}\" in extract-json|extract-field) cat >/dev/null ;; esac\n\
             case \" $* \" in *\" --require-field \"*) exit {require_field_rc} ;; esac\n\
             exit 0\n"
        ),
    )
    .expect("write fake amplihack");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod fake amplihack");
}

// Runs a copy of the shell test from a scratch tree whose recipes, tools and
// skill are empty files. The copy picks its binary exactly as the shipped
// test does; the scratch tree has no `target/`, so only the
// `CARGO_TARGET_DIR` and `PATH` candidates exist. It stops with a
// HARNESS-ERROR at the first step body it cannot extract.
#[cfg(unix)]
fn run_shell_test_copy(
    scratch: &Path,
    cargo_target_dir: Option<&str>,
    path_dir: &Path,
) -> (Option<i32>, String, String) {
    let bundle = scratch.join("amplifier-bundle");
    fs::create_dir_all(bundle.join("recipes/tests")).expect("create scratch recipes");
    fs::create_dir_all(bundle.join("tools")).expect("create scratch tools");
    fs::create_dir_all(bundle.join("skills/auto-drive-to-merge")).expect("create scratch skill");
    for recipe in AUTODRIVE_RECIPES {
        fs::write(bundle.join(format!("recipes/{recipe}.yaml")), "").expect("write empty recipe");
    }
    for tool in AUTODRIVE_TOOLS {
        fs::write(bundle.join(format!("tools/{tool}")), "").expect("write empty tool");
    }
    fs::write(bundle.join("skills/auto-drive-to-merge/SKILL.md"), "").expect("write empty skill");
    let script = bundle.join("recipes/tests/test-auto-drive-to-merge.sh");
    fs::copy(
        workspace_root().join("amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh"),
        &script,
    )
    .expect("copy the shell test");

    let path = match std::env::var("PATH") {
        Ok(p) => format!("{}:{p}", path_dir.display()),
        Err(_) => path_dir.display().to_string(),
    };
    let mut cmd = Command::new("bash");
    cmd.arg(&script)
        .current_dir(scratch)
        .env("PATH", path)
        .env("TMPDIR", scratch)
        .stdin(std::process::Stdio::null());
    match cargo_target_dir {
        Some(dir) => cmd.env("CARGO_TARGET_DIR", dir),
        None => cmd.env_remove("CARGO_TARGET_DIR"),
    };
    let out = cmd
        .output()
        .expect("run a copy of the auto-drive shell test");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// With `CARGO_TARGET_DIR` set, `cargo build` puts the branch build outside
// the checkout. The shell test must find it there ahead of `PATH`, skip a
// `release/` build that lacks `--require-field`, and print the binary it
// chose as its first line. The fakes do not implement
// `normalise-loop-verdict`, so this also pins that this test keeps its own
// probe rather than the loop-health one.
#[cfg(unix)]
#[test]
fn shell_test_picks_and_prints_a_capable_build_under_cargo_target_dir() {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let target = scratch.path().join("cargo-target");
    let on_path = scratch.path().join("on-path");
    write_fake_amplihack(&target.join("release/amplihack"), false);
    write_fake_amplihack(&target.join("debug/amplihack"), true);
    write_fake_amplihack(&on_path.join("amplihack"), true);

    let (code, stdout, stderr) = run_shell_test_copy(
        scratch.path(),
        Some(&target.display().to_string()),
        &on_path,
    );

    let want = format!("amplihack binary: {}/debug/amplihack", target.display());
    assert_eq!(
        stdout.lines().next(),
        Some(want.as_str()),
        "the first stdout line must name the chosen binary (exit {code:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

// CI sets no `CARGO_TARGET_DIR`, and the shell test runs under `set -u`, so
// a bare `$CARGO_TARGET_DIR` would abort it before its first check. Unset and
// empty must both fall through to the binary on `PATH`.
#[cfg(unix)]
#[test]
fn shell_test_without_cargo_target_dir_uses_the_binary_on_path() {
    for cargo_target_dir in [None, Some("")] {
        let scratch = tempfile::tempdir().expect("scratch dir");
        let on_path = scratch.path().join("on-path");
        write_fake_amplihack(&on_path.join("amplihack"), true);

        let (code, stdout, stderr) =
            run_shell_test_copy(scratch.path(), cargo_target_dir, &on_path);

        let want = format!("amplihack binary: {}/amplihack", on_path.display());
        assert_eq!(
            stdout.lines().next(),
            Some(want.as_str()),
            "CARGO_TARGET_DIR={cargo_target_dir:?}: the first stdout line must name the binary on \
             PATH (exit {code:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
    }
}

// ── #1543: evidence measures the PR head, the report reports the merge ───────

// The behavioural regression test for #1543 — a run started from the main
// checkout records the PR head and runs qa-team in the PR worktree; a run
// that skipped build and crusty and merged reports MERGED with the PR. It
// needs the `orch helper` pipeline, so it runs here with the amplihack under
// test first on PATH rather than as a bare ci.yml step.

// Drop double- and single-quoted segments so message text ("refusing to
// enter the merge loop", "run `git pull --ff-only`") is not read as a verb.
fn unquoted(line: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => quote = Some(c),
            None => out.push(c),
        }
    }
    out
}

#[test]
fn the_pr_head_resolver_is_read_only() {
    let path = tool_path("autodrive_pr_head.sh");
    assert!(path.is_file(), "missing tool {}", path.display());
    let text = read(&path);
    const GIT_VERBS: [&str; 14] = [
        "fetch",
        "pull",
        "checkout",
        "switch",
        "config",
        "update-ref",
        "reset",
        "stash",
        "commit",
        "push",
        "merge",
        "rebase",
        "clone",
        "init",
    ];
    const WORKTREE_VERBS: [&str; 6] = ["add", "prune", "remove", "repair", "move", "lock"];
    const GH_VERBS: [&str; 6] = ["merge", "edit", "close", "comment", "review", "api"];
    let mut offenders = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let code = unquoted(line);
        let words: Vec<&str> = code
            .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '$'))
            .filter(|w| !w.is_empty())
            .collect();
        for (i, w) in words.iter().enumerate() {
            let rest = &words[i + 1..];
            let hit = match *w {
                "git" => {
                    rest.iter().any(|r| GIT_VERBS.contains(r))
                        || rest
                            .windows(2)
                            .any(|p| p[0] == "worktree" && WORKTREE_VERBS.contains(&p[1]))
                }
                "gh" => rest.iter().any(|r| GH_VERBS.contains(r)),
                _ => false,
            };
            if hit {
                offenders.push(format!("{}: {}", n + 1, raw.trim()));
                break;
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "autodrive_pr_head.sh must never create, fetch into, check out, \
         commit in, push from or merge anything — it only measures:\n{}",
        offenders.join("\n")
    );
    assert!(
        text.contains("pr view") && text.contains("headRefOid") && text.contains("headRefName"),
        "the resolver must read the PR head from the platform (gh pr view headRefOid/headRefName)"
    );
    assert!(
        text.contains("worktree list --porcelain"),
        "the resolver must find the PR branch's EXISTING worktree via git worktree list --porcelain"
    );
}

#[test]
fn the_composer_tells_both_loops_whether_pr_number_was_supplied() {
    let recipe = recipe_yaml("auto-drive-to-merge");
    // crusty round-1 on PR #1588: the runner copies a sub-recipe's whole
    // context back into the parent, and the crusty loop is given
    // pr_number={{build_result.pr}}, so `{{pr_number}}` read after that phase
    // is the build phase's PR. The operator's value is captured once, before
    // the first sub-recipe, and forwarded under a name no sub-recipe changes.
    let seed = step(&recipe, "autodrive-pr-input");
    assert_eq!(seed.get("type").and_then(Value::as_str), Some("bash"));
    assert_eq!(
        seed.get("output").and_then(Value::as_str),
        Some("autodrive_pr_input")
    );
    assert!(
        field(seed, "command").contains("RECIPE_VAR_pr_number"),
        "autodrive-pr-input must print the operator's pr_number"
    );
    let seed_at = step_index(&recipe, "autodrive-pr-input");
    for (i, s) in steps(&recipe).iter().enumerate() {
        if s.get("type").and_then(Value::as_str) == Some("recipe") {
            assert!(
                seed_at < i,
                "autodrive-pr-input must run before every sub-recipe step"
            );
        }
    }
    for id in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let ctx = step(&recipe, id)
            .get("context")
            .and_then(Value::as_mapping)
            .unwrap_or_else(|| panic!("{id} forwards no context"));
        assert_eq!(
            ctx.get(Value::from("autodrive_pr_input"))
                .and_then(Value::as_str),
            Some("{{autodrive_pr_input}}"),
            "{id} must forward the operator's pr_number captured by \
             autodrive-pr-input, not `{{pr_number}}`, which the crusty loop \
             overwrites with build_result.pr"
        );
        assert_eq!(
            ctx.get(Value::from("existing_branch"))
                .and_then(Value::as_str),
            Some("{{existing_branch}}"),
            "{id} must forward existing_branch"
        );
    }
}

// The merge gate stays on the launch side, bound to the platform's
// headRefOid. Resolver output must never flow into the merge decision.

// #1561 residual: the runner passes an inherited `RECIPE_VAR_<key>` through to
// a nested run that does not declare `<key>`. The merge round therefore
// declares every gate key it reads with an empty default, so a parent's value
// is replaced at the start of the round and a key no step fills takes the
// read's deny default. See "What is still read through a bare name".

// #1607, on a child process: the outer run's real tools are inherited as
// `AUTODRIVE_TOOLS_DIR` (not scrubbed), and this round's own tools directory
// has no round tool. step-03 must compare nothing and fail closed, not run
// the outer tool and keep MERGE_READY. The environment is set only on the
// child `Command`, never with `std::env::set_var`.

const RANGE_TOKENS: [&str; 2] = ["crusty-unreviewed-commits", "crusty-range-unreadable"];

const QA_TRUSTED_TOKENS: [&str; 4] = [
    "qa-manifest-missing",
    "qa-record-modified",
    "qa-evidence-modified",
    "qa-evidence-stale",
];

// ── Issue #2033: repository commands run with the git and run-identity env cleared ──

const GIT_REPO_ENV_LOAD: &str = ". \"{{autodrive_tools_dir}}/git-repo-env.sh\" && amplihack_clear_git_repo_env && amplihack_clear_run_identity_env";

#[test]
fn issue_2033_step_04_loads_the_helper_instead_of_a_partial_unset() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let prompt = field(step(&recipe, "step-04-address-blockers"), "prompt");
    assert!(
        !prompt.contains("unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE"),
        "step-04 must not keep the partial GIT_* unset (#2033)"
    );
    // The test-running instruction also loads this helper. Check the commit
    // block independently so prose order cannot select that other call.
    let commit_block = prompt
        .split("```bash\n")
        .skip(1)
        .filter_map(|section| section.split_once("```").map(|(block, _)| block))
        .find(|block| {
            block
                .lines()
                .any(|line| line.trim_start().starts_with("git commit "))
        })
        .expect("step-04 must contain a fenced bash commit block");
    let cd = commit_block
        .lines()
        .position(|l| l.contains("cd \"{{repo_path}}\""))
        .expect("step-04 enters the PR worktree");
    let (load, line) = commit_block
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("git-repo-env.sh") && l.contains("amplihack_clear_git_repo_env"))
        .unwrap_or_else(|| panic!("step-04 must source git-repo-env.sh and clear (#2033)"));
    assert!(
        line.contains("unset GH_REPO"),
        "step-04 must keep `unset GH_REPO` on the helper line:\n  {line}"
    );
    assert!(
        line.contains(GIT_REPO_ENV_LOAD),
        "step-04 must load the helper from the checked tools dir and clear both lists:\n  {line}"
    );
    assert!(
        line.contains("STOP: cannot load git-repo-env.sh from the tools directory (#2033)")
            && line.contains("exit 1"),
        "step-04 must stop when the helper cannot be loaded:\n  {line}"
    );
    let guard = commit_block
        .lines()
        .position(|l| l.contains("autodrive_worktree_guard.sh"))
        .expect("step-04 checks the PR worktree before committing");
    assert!(
        cd < load && load < guard,
        "step-04 must clear after cd and before the worktree guard (#2033)"
    );
    let first_git = commit_block
        .lines()
        .enumerate()
        .find(|(_, l)| {
            l.trim_start().starts_with("git ") || l.contains("autodrive_worktree_guard.sh")
        })
        .map(|(n, _)| n)
        .expect("step-04 runs git after entering the worktree");
    assert!(
        load < first_git,
        "step-04 must clear before its first git command (#2033)"
    );
}

#[test]
fn issue_2033_step_04_test_runs_load_the_helper_in_the_same_shell_call() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let prompt = field(step(&recipe, "step-04-address-blockers"), "prompt");
    let instruction = prompt
        .lines()
        .find(|line| line.contains("**Running tests or hooks**"))
        .expect("step-04 must instruct the agent how to isolate test and hook runs (#2033)");
    assert!(
        instruction.contains("every shell call") && instruction.contains("in that same call"),
        "step-04 must clear the inherited environment in each shell call that runs tests or hooks:\n  {instruction}"
    );
    assert!(
        instruction.contains(GIT_REPO_ENV_LOAD),
        "test and hook runs must load the checked helper and clear both lists:\n  {instruction}"
    );
    assert!(
        instruction.contains("[ ! -L \"{{autodrive_tools_dir}}/git-repo-env.sh\" ] &&")
            && instruction
                .contains("|| { echo \"STOP: cannot load git-repo-env.sh (#2033)\" >&2; exit 1; }"),
        "test and hook runs must stop if the helper is a symlink or cannot be loaded:\n  {instruction}"
    );
}

#[test]
fn issue_2033_step_00_tools_dir_requires_the_helper() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let cmd = field(step(&recipe, "step-00-tools-dir"), "command");
    let need: String = cmd
        .lines()
        .filter(|l| l.trim_start().starts_with("NEED="))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        need.split(|c: char| c.is_whitespace() || c == '"')
            .any(|w| w == "git-repo-env.sh"),
        "step-00-tools-dir NEED must list git-repo-env.sh so a tools dir without it is refused (#2033):\n{need}"
    );
}

#[test]
fn issue_2033_merge_evidence_steps_clear_before_any_git_command() {
    let recipe = recipe_yaml("autodrive-merge-evidence");
    // One lookup per round: the evidence steps load git-repo-env.sh from the
    // tools directory merge-round's step-00 found, not from a search of their own.
    let round = recipe_yaml("autodrive-merge-round");
    let passed = step(&round, "merge-evidence")
        .get("context")
        .and_then(|c| c.get("autodrive_tools_dir"))
        .and_then(Value::as_str);
    assert_eq!(
        passed,
        Some("{{autodrive_tools_dir}}"),
        "merge-round must pass autodrive_tools_dir to autodrive-merge-evidence (#2033)"
    );
    assert!(
        recipe
            .get("context")
            .and_then(|c| c.get("autodrive_tools_dir"))
            .is_some(),
        "autodrive-merge-evidence must declare autodrive_tools_dir in its context"
    );
    // #2329 made these agent steps. Each agent shell call is a new process
    // that inherits the agent's environment, so the clearing instruction comes
    // first in the prompt and applies to every call that runs git, gh or tests.
    for id in [
        "step-01-sync-with-base",
        "step-02-qa-team-scenarios",
        "step-03-wait-for-ci",
    ] {
        let prompt = field(step(&recipe, id), "prompt");
        let first = prompt.lines().next().unwrap_or_default();
        assert!(
            first.contains("(issue #2033)")
                && first.contains("every shell call")
                && first.contains("in that same call"),
            "{id} must clear the inherited environment before any git or gh call:\n  {first}"
        );
        assert!(
            first.contains(GIT_REPO_ENV_LOAD)
                && first.contains("[ ! -L \"{{autodrive_tools_dir}}/git-repo-env.sh\" ] &&")
                && first.contains(
                    "|| { echo \"STOP: cannot load git-repo-env.sh from the tools directory (#2033)\" >&2; exit 1; }"
                ),
            "{id} must load the checked helper, refuse a symlink and stop when it cannot be loaded:\n  {first}"
        );
        assert!(
            first.contains("never through the git toplevel"),
            "{id} must not find tools through the git toplevel (#2033):\n  {first}"
        );
    }
}
