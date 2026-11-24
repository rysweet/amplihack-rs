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

use crate::isolated_amplihack::isolated;

// The brick budget for a recipe or tool: at most this many lines.
// The #1528 recipe-length test in `artifact_guard_workflow_tests.rs`
// imports this rather than restating it (PR #1586 review, round 6).
// Other brick checks still carry their own copies of the limit
// (`issue_1337_loop_health_evaluator_test.rs`,
// `issue_684_host_aware_workflow_test.rs` and the `*_decomposition_test.rs`
// files), so changing it here does not change them.
pub(crate) const BRICK_LINE_BUDGET: usize = 400;

// #3129: the shared goal loop's recipes come first. The old loop recipes stay
// listed because they are one-release aliases (#3049).
const AUTODRIVE_RECIPES: [&str; 10] = [
    "auto-drive-to-merge",
    "autodrive-goal-loop",
    "autodrive-goal-turn",
    "autodrive-build",
    "autodrive-crusty-round",
    "autodrive-crusty-loop",
    "autodrive-merge-evidence",
    "autodrive-merge-round",
    "autodrive-merge-loop",
    "autodrive-finalize",
];

// #3129: the goal loop's driver, turn tool and library, and the pull request
// goal's tools, where the merge action and its trust checks now live.
const AUTODRIVE_TOOLS: [&str; 39] = [
    "autodrive_goal_loop.sh",
    "autodrive_goal_turn.sh",
    "autodrive_goal_lib.sh",
    "autodrive_pr_goal.sh",
    "autodrive_pr_reading.sh",
    "autodrive_round_word.sh",
    "autodrive_budget.sh",
    "autodrive_crusty_rereview.sh",
    "autodrive_done_override.sh",
    "autodrive_escalate.sh",
    "autodrive_finding_rules.sh",
    "autodrive_followups.sh",
    "autodrive_followups_file.sh",
    "autodrive_followups_check.sh",
    "autodrive_last_round.sh",
    "autodrive_loop.sh",
    "autodrive_loop_record.sh",
    "autodrive_crusty_record.sh",
    "autodrive_crusty_posted.sh",
    "autodrive_crusty_finalize.sh",
    "autodrive_completion_context.sh",
    "autodrive_merge.sh",
    "autodrive_merge_message.sh",
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
    // #3129: one goal loop replaced the crusty loop and the merge-ready loop.
    assert_eq!(
        sub,
        vec!["autodrive-build", "autodrive-goal-loop"],
        "the composer must stay thin: build, then the shared goal loop, in that order"
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

// #3129 removed the loop-health STUCK stop from the auto-drive loops, so
// nothing here runs loop-health-evaluator any more (hook repair in the build
// stage still does). The contract must still never be copied into this
// workflow: the evaluator's own step ids appear nowhere on its control path.
#[test]
fn the_loop_health_contract_is_never_copied_into_auto_drive() {
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
                    "{}:{} introduces `{token}`. No loop may be ended by a count: \
                     nothing counts turns, rounds or time (#3129).\n  {line}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
    // #3129 removed the old loop driver's round counter, the ROUND label this
    // test also held free of comparisons, with the driver itself.
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
    // #3129 removed merge-evidence's CI poll: waiting is the goal loop's `wait`
    // action, `gh pr checks --watch`, with no interval of its own.
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
}

// ── No silent merge ──────────────────────────────────────────────────────────

#[test]
fn exit_79_is_terminal_and_never_retried_into() {
    // #3129: the goal loop's driver replaced autodrive_loop.sh. Exit 79 from
    // the goal's reading, the turn run or an action ends the run at once and
    // is passed up unchanged.
    let driver = read(&tool_path("autodrive_goal_loop.sh"));
    assert!(
        driver.contains("79) end_run refused \"exit 79 from ${2}: a terminal policy refusal"),
        "the loop driver must name exit 79 as the terminal policy refusal"
    );
    assert!(
        read(&tool_path("autodrive_refusal.sh")).contains("structural_refusal"),
        "the runner's structural_refusal marker must be recognised alongside exit 79 (#1437)"
    );
    assert!(
        driver.contains("never retried"),
        "the driver must document that the guard is never retried into"
    );
    // The refusal is detected BEFORE any mechanical action runs, so not even
    // one action is spent after the guard sealed. (The old driver checked it
    // before its loop-health evaluator, which #3129 removed.)
    let refusal_at = driver
        .find("passed_up \"$rc\" \"the turn run\"")
        .expect("turn run refusal check");
    let action_at = driver
        .find("goal_call act \"$ACTION\"")
        .expect("the mechanical action");
    assert!(
        refusal_at < action_at,
        "the terminal refusal must be checked before the turn's action runs"
    );
    // The loop recipe surfaces and propagates the refusal: its step exits with
    // the driver's 79, run directly and under step recovery (the path every
    // auto-drive takes, since step-recovery-init runs first).
    exit_79_reaches_the_goal_loop_step(false);
    exit_79_reaches_the_goal_loop_step(true);
}

// Runs autodrive-goal-loop's `goal-loop` step against a launch bundle whose
// driver exits 79, and asserts the step exits 79 too.
fn exit_79_reaches_the_goal_loop_step(with_recovery: bool) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tools = tmp.path().join("home/amplifier-bundle/tools");
    fs::create_dir_all(&tools).expect("tools dir");
    fs::write(tools.join("autodrive_goal_loop.sh"), "exit 79\n").expect("stub driver");
    fs::copy(
        tool_path("recipe_context_value.sh"),
        tools.join("recipe_context_value.sh"),
    )
    .expect("copy recipe_context_value.sh");
    let recipe = recipe_yaml("autodrive-goal-loop");
    let body = step_command(&recipe, "goal-loop");
    let script = if with_recovery {
        format!(
            "RECIPE_VAR_step_recovery_dir=\"$(amplihack orch helper step-recovery init)\" || exit 98\n\
             export RECIPE_VAR_step_recovery_dir\n{body}"
        )
    } else {
        body.to_string()
    };
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_amplihack"))
        .parent()
        .expect("the binary's folder")
        .to_path_buf();
    let path = match std::env::var("PATH") {
        Ok(p) => format!("{}:{p}", bin_dir.display()),
        Err(_) => bin_dir.display().to_string(),
    };
    let out = isolated("bash")
        .arg("-c")
        .arg(script)
        .current_dir(tmp.path())
        .env("PATH", path)
        .env("HOME", tmp.path())
        .env("TMPDIR", tmp.path())
        .env("AMPLIHACK_HOME", tmp.path().join("home"))
        .output()
        .expect("run the goal-loop step");
    assert_eq!(
        out.status.code(),
        Some(79),
        "autodrive-goal-loop (step recovery {with_recovery}) must surface and propagate the \
         exit-79 policy refusal; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn recursion_context_is_propagated_and_the_ceiling_is_never_raised() {
    // #3129: the goal loop's driver starts every turn as a fresh
    // `amplihack recipe run` from its own step, so the turns inherit the
    // recursion context unchanged. The old driver's explicit export of the
    // three variables and its ceiling-change check went with it.
    let driver = read(&tool_path("autodrive_goal_loop.sh"));
    for var in ["AMPLIHACK_SESSION_DEPTH=", "AMPLIHACK_MAX_DEPTH="] {
        assert!(
            !driver.contains(var),
            "the driver must never set {var} for its turns"
        );
    }
    assert!(
        driver.contains("at constant depth"),
        "turns must run sequentially at constant depth so a long loop never \
         walks toward the recursion ceiling"
    );
}

// ── Resumability ─────────────────────────────────────────────────────────────

#[test]
fn a_dead_run_resumes_without_redoing_merged_work_or_reopening_concerns() {
    // #3129: the crusty and merge-ready loop recipes are one-release aliases
    // with no agent prompt. The goal loop resumes from live state: every turn
    // reads the PR afresh.
    let doc = recipe_yaml("autodrive-build");
    let prompt = field(&steps(&doc)[0], "prompt");
    assert!(prompt.contains("already") && prompt.contains("MERGED"));
    assert!(prompt.contains("historical evidence never establishes current readiness"));
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
        // all, under any function name. #2628 (Ryan: criterion 3 is an
        // agentic step) makes the one exception: the Crusty verdict comment,
        // read and written only by the two agent steps below, which count a
        // comment only from the drive's own login or an OWNER, MEMBER or
        // COLLABORATOR. No tool or bash step reads a comment. (#3129 removed
        // the merge round's step-01b-crusty-evidence, the other reader.)
        let verdict_prompts: Vec<String> = [("autodrive-crusty-round", "step-03c-post-verdict")]
            .iter()
            .filter(|(recipe, _)| path == recipe_path(recipe))
            .map(|(recipe, id)| field(step(&recipe_yaml(recipe), id), "prompt").to_string())
            .collect();
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() && verdict_prompts.iter().any(|p| p.contains(trimmed)) {
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
    // Agent steps call `Skill(skill="qa-team")` (workflow-precommit-test and
    // workflow-publish). If qa-team ever starts refusing agents, those calls
    // fail in every run the way merge-ready's did (issue #1517). #3129
    // removed the merge round's step-04, which also called it.
    let path = workspace_root().join("amplifier-bundle/skills/qa-team/SKILL.md");
    let text = read(&path);
    let front = skill_frontmatter(&text).expect("qa-team SKILL.md must have frontmatter");
    let refuses = front
        .iter()
        .any(|(k, v)| k == "disable-model-invocation" && v.eq_ignore_ascii_case("true"));
    assert!(
        !refuses,
        "qa-team sets disable-model-invocation: true, but agent steps call \
         Skill(skill=\"qa-team\"); that call would be refused in every run"
    );
}

#[test]
fn criterion_1_follows_the_qa_team_repo_type_table() {
    // crusty round-7 on PR #1520: requiring gadugi scenarios in every
    // repository type was a policy change that #1517 did not ask for. #1517
    // proposed the per-repository-type rule, and qa-team's own table says it:
    // a Rust CLI repository substitutes `cargo test` and does not require
    // gadugi. qa-team keeps that line, with no auto-drive exception under it,
    // and auto-drive follows the table.
    let skill = read(&workspace_root().join("amplifier-bundle/skills/qa-team/SKILL.md"));
    let rust_line = skill
        .find("**For Rust CLI repos**")
        .expect("qa-team must keep its Rust CLI line");
    let line = &skill[rust_line
        ..skill[rust_line..]
            .find('\n')
            .map_or(skill.len(), |e| rust_line + e)];
    assert!(
        line.contains("substitute `cargo test`")
            && line.contains("Do **not** require the gadugi-agentic-test framework"),
        "qa-team's Rust CLI line must keep its cargo test substitution: {line}"
    );
    assert!(
        !skill.contains("Exception: auto-drive-to-merge"),
        "qa-team must not carry an auto-drive exception to its Rust CLI line"
    );
    // #3129 removed the merge round's step-04, whose prompt applied this
    // table to criterion 1; its assertions went with it.
}

// `autodrive_gadugi_required` in autodrive_trust.sh is the one place the
// repo-type rule is written. Every step that decides whether gadugi-test
// is needed asks it, and none decides by reading marker files itself.

#[cfg(unix)]
#[test]
fn autodrive_gadugi_required_follows_the_qa_team_table() {
    let trust = tool_path("autodrive_trust.sh");
    let cases: &[(&[&str], Option<&str>, &str, &str)] = &[
        (&["Cargo.toml"], None, "rust-cli", "false"),
        (&["package.json"], None, "node", "true"),
        (&["pyproject.toml"], None, "python", "false"),
        (&["setup.py"], None, "python", "false"),
        (&[], None, "unknown", "false"),
        // Cargo.toml decides first, as in qa-team's table.
        (&["Cargo.toml", "package.json"], None, "rust-cli", "false"),
        // The operator asks for gadugi in any repository type.
        (
            &["Cargo.toml"],
            Some("tests/gadugi/scenarios"),
            "rust-cli",
            "true",
        ),
        (&["pyproject.toml"], Some("scenarios"), "python", "true"),
        (&[], Some("x"), "unknown", "true"),
        // Set but empty is not set.
        (&["Cargo.toml"], Some(""), "rust-cli", "false"),
        // #1516: a root Python project wins over package.json, so a mixed
        // root no longer requires gadugi.
        (&["package.json", "pyproject.toml"], None, "python", "false"),
        (&["setup.py", "package.json"], None, "python", "false"),
        // #1516: a single nested Cargo workspace within depth 3 is Rust;
        // unrelated nested crates, or one only under a pruned directory,
        // decide nothing.
        (&["a/Cargo.toml"], None, "rust-cli", "false"),
        (&["a/Cargo.toml", "b/Cargo.toml"], None, "unknown", "false"),
        (&["target/Cargo.toml"], None, "unknown", "false"),
        // #1516: a root manifest wins over a nested crate (jamestown shape).
        (
            &["pyproject.toml", "grid-encode-decode/Cargo.toml"],
            None,
            "python",
            "false",
        ),
        (&["package.json", "sub/Cargo.toml"], None, "node", "true"),
    ];
    for (files, scen, want_type, want_req) in cases {
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
            .arg(r#". "$1" && autodrive_repo_type "$2" && autodrive_gadugi_required "$2""#)
            .arg("bash")
            .arg(&trust)
            .arg(tmp.path())
            .env_remove("AUTODRIVE_QA_BASE")
            .env_remove("AUTODRIVE_QA_SCENARIO_DIR");
        if let Some(v) = scen {
            cmd.env("AUTODRIVE_QA_SCENARIO_DIR", v);
        }
        let out = cmd.output().expect("run bash");
        assert!(out.status.success(), "{files:?}/{scen:?}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{want_type}\n{want_req}\n"),
            "files={files:?} AUTODRIVE_QA_SCENARIO_DIR={scen:?}"
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

// Every (recipe, step id, prompt) for a step that runs an agent, across the
// auto-drive recipes and the loop-health evaluator both loops call.

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
    // #3129 removed the old loop driver that wrote the manifest row and the
    // merge round that read it; autodrive_crusty_final keeps its contract.
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

    // #3129 rewrote the skill for the goal loop: the loop-health evaluator,
    // the merge-ready criteria, the qa-team step, platform confirmation and
    // resume are gone from it. It states the merge authority and the history
    // rules, and links the specification, which states the rest.
    for required in [
        "captured invocation",
        "--match-head-commit",
        "Never rebase",
        "force-push",
        "docs/reference/autodrive-goal-loop.md",
    ] {
        assert!(
            text.contains(required),
            "the skill must document `{required}`"
        );
    }
    let spec = read(&workspace_root().join("docs/reference/autodrive-goal-loop.md"));
    for required in ["branch protection", "--admin", "79"] {
        assert!(
            spec.contains(required),
            "the goal loop specification the skill links must document `{required}`"
        );
    }
    // The agent step is told exit 79 is terminal in its own prompt.
    assert!(
        recipe_text("autodrive-goal-turn")
            .contains("Exit 79 from any command is a terminal refusal"),
        "the turn prompt must say exit 79 is terminal"
    );

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
    // #3129: the goal loop is the loop now, and its review action runs the
    // Crusty round from the driver's depth. The merge round is a one-release
    // alias that runs one turn of the goal loop, not a round.
    let loop_recipes = [
        "auto-drive-to-merge",
        "autodrive-goal-loop",
        "autodrive-crusty-loop",
        "autodrive-merge-loop",
    ];
    let loop_starters = [
        "autodrive_goal_loop.sh",
        "autodrive_loop.sh",
        "autodrive_crusty_rereview.sh",
    ];
    {
        let round = "autodrive-crusty-round";
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
    // `type: recipe` step runs inside the same runner. The goal loop (#3129)
    // is a `type: recipe` step of the composer, so its driver runs as a step
    // of the composer's runner, and the driver's review action starts each
    // Crusty round from there.
    let composer = recipe_yaml("auto-drive-to-merge");
    let phase = "autodrive-goal-loop";
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
        step_command(&recipe_yaml(phase), "goal-loop")
            .contains("bash \"$T/autodrive_goal_loop.sh\""),
        "{phase} goal-loop must run the loop driver in its own shell"
    );
    const SHELL: u32 = 0; // a person starts auto-drive from a shell
    let composer_runner = SHELL + 1;
    let loop_step = composer_runner + 1; // autodrive_goal_loop.sh and its goal tool's review action
    let round_runner = loop_step + 1;
    let round_step = round_runner + 1; // where a nested crusty round would run

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
        "#!/bin/sh\n\
         case \"${1:-}\" in\n\
         --capabilities) printf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}'; exit 0 ;;\n\
         --version) echo fixture; exit 0 ;;\n\
         esac\n\
         printf 'depth=%s\\n' \"${AMPLIHACK_SESSION_DEPTH:-}\" >> \"$RUNNER_LOG\"\nprintf '{}\\n'\n",
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
            // `env_clear` dropped the probe knob `isolated` set (issue #2019).
            .env(crate::isolated_amplihack::PROBE_WAIT_ENV, "exit")
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

    // #3129: the goal loop's driver replaced autodrive_loop.sh. Its before-round
    // step and loop-health evaluator, two of the three old call sites, are
    // gone; the turn run is where a round ran, so its log is the one call site.
    let driver = read(&tool_path("autodrive_goal_loop.sh"));
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
        code.iter().any(|line| line.contains(
            "for lib in autodrive_goal_lib.sh autodrive_refusal.sh autodrive_state.sh; do"
        )) && code
            .iter()
            .any(|line| line.contains(". \"$LOOP_HOME/$lib\"")),
        "autodrive_goal_loop.sh must source autodrive_refusal.sh from beside itself"
    );
    assert!(
        !code.iter().any(|line| line.contains("BLOCKED_TERMINAL")),
        "autodrive_goal_loop.sh must not match the refusal's text"
    );
    let calls: Vec<&&str> = code
        .iter()
        .filter(|line| line.contains("terminal_refusal \""))
        .collect();
    assert!(
        calls.iter().any(|line| line.contains("\"$TD/turn.log\"")),
        "terminal_refusal must be called with the turn run's log \"$TD/turn.log\", so a \
         structural_refusal marker with a non-79 exit ends the run as refused (#1437): \
         {calls:?}"
    );
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
        "autodrive_scenario_results",
    ] {
        assert!(
            regex::Regex::new(&format!(r"(?m)^{func}\(\)\s*\{{"))
                .unwrap()
                .is_match(&trust),
            "autodrive_trust.sh must define `{func}()`"
        );
    }
    for token in RANGE_TOKENS.iter() {
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
    // #3129 made the merge round and merge evidence one-release aliases with no
    // step outputs, so only the Crusty round's outputs remain.
    let name = "crusty_round_context";
    assert!(
        outputs.iter().any(|o| o == name),
        "expected step output `{name}` in the round recipes"
    );
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
        // #3129: with the merge round's reads gone, the Crusty round's own
        // step bodies in its tools carry the reads this scan counts.
        (
            "autodrive_crusty_record.sh",
            read(&tool_path("autodrive_crusty_record.sh")),
        ),
        (
            "autodrive_crusty_concerns_evidence.sh",
            read(&tool_path("autodrive_crusty_concerns_evidence.sh")),
        ),
        (
            "autodrive_crusty_posted.sh",
            read(&tool_path("autodrive_crusty_posted.sh")),
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
    // #2575 removed the record-only reads (round-record, merge-ready-verdict),
    // and #2808 removed both review-index reads; count the remaining reads.
    assert!(
        reads >= 8,
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
const HARNESS_STEPS: [(&str, &str, &str); 5] = [
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
        "run-issue-2784-existing-pr-other-base-scenario.sh",
        "workflow-publish",
        "step-16-create-draft-pr",
    ),
    // Issue #2343: the finalization pair, run end to end against a local bare
    // remote and a stub gh reporting a check still in progress.
    (
        "run-issue-2343-finalization-outcome-scenario.sh",
        "workflow-finalize",
        "collect-finalization-evidence",
    ),
    (
        "run-issue-2343-finalization-outcome-scenario.sh",
        "workflow-finalize",
        "validate-agentic-finalization",
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

    // Issue #2712: `isolated()` keeps a recipe run's AMPLIHACK_WORK_ROOT out
    // of the suite, so its worktrees cannot land in the shared work root.
    let mut cmd = isolated("bash");
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
    // Issue #2712: `isolated()` keeps a recipe run's AMPLIHACK_WORK_ROOT out
    // of the suite, so its worktrees cannot land in the shared work root.
    let out = isolated("bash")
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
fn the_merge_action_supplies_the_squash_message_instead_of_the_platform_default() {
    // #2575: the merge sources autodrive_merge.sh, which sources the trusted
    // squash-message helper (#2056). #3129 moved the caller from the finalize
    // `merge` step to the goal loop's merge action (autodrive_pr_goal.sh act
    // merge), which sources it from its own launch-bundle tools folder.
    let action = read(&tool_path("autodrive_pr_goal.sh"));
    assert!(action.contains(". \"$HERE/autodrive_merge.sh\""));
    // The finalize step refused a symlinked autodrive_merge.sh
    // (`[ ! -L "$M" ]`): the launch bundle's tools are regular files only.
    assert!(
        action.lines().any(|l| l.contains("autodrive_merge.sh")
            && (l.contains("[ ! -L") || l.contains("goal_bundle_file"))),
        "the merge action must refuse a symlinked autodrive_merge.sh before it sources it"
    );
    let tool = read(&tool_path("autodrive_merge.sh"));
    for needle in [
        "#2056",
        "mm=\"${AMPLIHACK_HOME}/amplifier-bundle/tools/autodrive_merge_message.sh\"",
        "if [ ! -f \"$mm\" ] || [ -L \"$mm\" ]; then",
        "case \"${AMPLIHACK_HOME:-}\" in",
        "--subject \"$subject\" --body-file -",
        "printf '%s' \"$MERGE_BODY\" | gh pr merge",
    ] {
        assert!(
            tool.contains(needle),
            "autodrive_merge.sh must say `{needle}` (#2056)"
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
    // #3129: the shell test also checks for the two recipes that run the goal
    // loop or hand off from it, which the lists above leave out.
    for recipe in AUTODRIVE_RECIPES
        .iter()
        .chain(&["auto-drive-light", "autodrive-handoff"])
    {
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
    // Issue #2712: `isolated()` keeps a recipe run's AMPLIHACK_WORK_ROOT out
    // of the suite, so its worktrees cannot land in the shared work root.
    let mut cmd = isolated("bash");
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
    // #3129 removed the crusty and merge-ready loop steps that this test
    // also held to forwarding autodrive_pr_input and existing_branch. The goal
    // loop is given the PR that autodrive-goal-pr names.
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
