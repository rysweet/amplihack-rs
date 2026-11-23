//! Issue #2911, review of PR #2992: a step ended by a lost prompt after a
//! usage limit is resumed at that step by a plain relaunch of the same
//! command, with no `--resume` flag and no edit to the recipe or the run's
//! history.
//!
//! `issue_2911_prompt_loss_ending` runs `amplihack claude` on its own and
//! pins the ending (`failed reason=prompt_cut_short`), and
//! `limit_wait::tests_ending_evidence` pins the classification. Neither runs
//! the recipe runner, so neither shows that the limit the supervisor reports
//! over the recovery channel reaches the run's recovery record and turns the
//! next launch into a resume. This test runs the whole path on the pinned
//! runner, through `issue_1741_fixture`.
//!
//! The run's `design` step is a bash step that runs the real `amplihack
//! claude --subprocess-safe -- -p` as a supervised agent step, with the
//! limit wait on and the wait seams of `issue_1618_fixture`. Its stdin is a
//! copy of the write end of a pipe (`0>&1` inside `| cat`), so the prompt
//! copier's first read fails and the prompt is cut short, as in
//! `a_prompt_cut_short_is_a_lost_prompt_and_hands_the_run_the_reset`. A stub
//! `claude` then reports a session limit.

#![cfg(unix)]

use crate::issue_1618_fixture::{LIMIT_TEXT, TEST_NOW};
use crate::issue_1741_fixture::{Fixture, describe, quote};
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// `s` as one single-quoted shell word.
fn word(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A stub `claude` that answers `--version`, counts its calls in
/// `claude-calls`, keeps the prompt it was given in `claude-stdin.<n>` and
/// reports a session limit on every call.
fn stub_claude(root: &Path) -> String {
    format!(
        r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
root={root}
n=$(( $(cat "$root/claude-calls" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$root/claude-calls"
cat > "$root/claude-stdin.$n"
echo "attempt $n output"
printf '%s\n' {limit} >&2
exit 1
"#,
        root = quote(root),
        limit = word(LIMIT_TEXT),
    )
}

/// The `design` step's tool. It records the step in the fixture's trace
/// like the fixture's own tool. In mode `lost-prompt` it runs the
/// supervised agent step, with everything that step needs exported here
/// (the fixture's run turns the limit wait off), its stdin a copy of the
/// write end of the pipe to `cat`, and its stderr kept in `design.stderr`;
/// otherwise it succeeds with a JSON output.
fn design_tool(root: &Path, agent_tools: &Path) -> String {
    format!(
        r#"#!/bin/bash
set -u
root={root}
printf '%s\n' design >> "$root/events"
mode=success
if [ -f "$root/mode.design" ]; then mode=$(cat "$root/mode.design"); fi
if [ "$mode" != lost-prompt ]; then
  printf '%s\n' '{{"label":"designed"}}'
  exit 0
fi
export PATH={tools}:"$PATH"
export AMPLIHACK_USAGE_LIMIT_WAIT=1
export AMPLIHACK_CLAUDE_BINARY_PATH={tools}/claude
export AMPLIHACK_CLAUDE_PROFILE_ROOT="$root/profiles"
export CLAUDE_CONFIG_DIR="$root/claude-config"
export TZ=America/Los_Angeles
export AMPLIHACK_NONINTERACTIVE=1
export AMPLIHACK_ROTATE_AUTOSTART=0
export AMPLIHACK_GOVERNOR=off
export AMPLIHACK_SKIP_POWER_STEERING=1
export AMPLIHACK_SKIP_REFLECTION=1
export AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW={now}
export AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE=1000000
{amp} claude --subprocess-safe -- -p 0>&1 2>"$root/design.stderr" | cat
exit "${{PIPESTATUS[0]}}"
"#,
        root = quote(root),
        tools = quote(agent_tools),
        now = TEST_NOW,
        amp = quote(&amplihack_bin()),
    )
}

/// Points the fixture recipe's `design` step at [`design_tool`] and writes
/// the stub `claude`. The recipe is written once, so both launches run the
/// same file.
///
/// The launcher keeps its session files in `.claude/runtime/` under the
/// step's working directory, which here is the observed checkout. The
/// worktrees the bundled workflow creates or adopts exclude that folder
/// (issue #2335, docs/reference/workflow-runtime-artifacts.md), and this
/// repository's `.gitignore` ignores it. The fixture checkout is neither, so
/// it is excluded here through `info/exclude`, which changes nothing
/// tracked. Without it the runner sees untracked files appear (it observes
/// the checkout with `git ls-files --others --exclude-standard`), reports
/// `classify` as `repository_changed` and restarts there instead: the run
/// is still resumed, from an earlier step. docs/reference/usage-limit-waits.md
/// lists that as a limit.
fn install_agent_step(f: &Fixture) {
    let exclude = f
        .repo
        .join(f.git(&["rev-parse", "--git-path", "info/exclude"]));
    fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    let mut ignored = fs::read_to_string(&exclude).unwrap_or_default();
    ignored.push_str("/.claude/runtime/\n");
    fs::write(&exclude, ignored).unwrap();

    let tools = f.root.join("agent-tools");
    fs::create_dir(&tools).unwrap();
    for dir in ["profiles", "claude-config"] {
        fs::create_dir(f.root.join(dir)).unwrap();
    }
    write_executable(&tools.join("claude"), stub_claude(&f.root), 0o700);
    let tool = f.root.join("design-tool");
    write_executable(&tool, design_tool(&f.root, &tools), 0o700);
    let mut design = f.step("design");
    design["command"] = Value::String(quote(&tool));
    let recipe = f.document(
        "smart-orchestrator",
        vec![f.step("classify"), design, f.step("implement")],
    );
    f.write_recipe(&f.recipe, &recipe);
}

/// A supervised agent step meets a usage limit with its prompt cut short.
/// The supervisor ends it as a lost prompt and reports the limit over the
/// recovery channel; the run fails and is recorded as stopped by a usage
/// limit at that step. The same command, launched again once the stand-in
/// for the agent can succeed, resumes at that step: `classify` is restored
/// from the first launch rather than run again, `design` runs again and
/// `implement` runs. (In production `design` would be sent its whole prompt
/// again; the stand-in here does not run an agent on the relaunch.)
///
/// The supervised step's stderr goes to a file, not to the runner, so no
/// limit message reaches the step's error text. The runner's own text check
/// (`recovery::replay::usage_limit`) and amplihack's failure classifier
/// therefore cannot class the run from words; the only way it can be classed
/// `usage_limit` is the structured report, which the test pins as the
/// result's `failure_cause` and the classifier's `durable scoped usage-limit
/// evidence` signal. In production that stderr does reach the runner, which
/// can only add a second, textual reason to resume.
///
/// What this proves: a plain relaunch of the identical command needs no
/// human step to resume a run that a lost prompt ended after a limit. What
/// it does not prove: which agentic layer issues that relaunch, or when.
/// The relaunch here is made by the test, at once, with no reset waited for.
#[test]
fn a_step_ended_by_a_lost_prompt_after_a_limit_is_resumed_at_that_step_by_a_plain_relaunch() {
    let f = Fixture::new();
    install_agent_step(&f);
    f.mode("design", "lost-prompt");

    let first = f.run(&[]);
    assert!(
        !first.output.status.success(),
        "{}",
        describe(&first.output)
    );
    let step_log = fs::read_to_string(f.root.join("design.stderr"))
        .unwrap_or_else(|e| panic!("the supervised step ran: {e}\n{}", describe(&first.output)));
    let ending: Vec<_> = step_log
        .lines()
        .filter(|l| l.starts_with("amplihack.recipe.usage_limit_wait "))
        .collect();
    assert_eq!(
        ending,
        ["amplihack.recipe.usage_limit_wait failed reason=prompt_cut_short"],
        "the step ended as a lost prompt, with no wait\nstep stderr:\n{step_log}\n{}",
        describe(&first.output)
    );
    assert_eq!(
        fs::read_to_string(f.root.join("claude-calls"))
            .unwrap()
            .trim(),
        "1",
        "one attempt and no retry: {step_log}"
    );
    assert!(
        fs::read(f.root.join("claude-stdin.1")).unwrap().is_empty(),
        "the attempt was given none of its prompt"
    );

    let result = first.result();
    let cause = &result["failure_cause"];
    assert_eq!(cause["class"], "usage_limit", "{result:#}");
    assert_eq!(cause["origin"]["step_id"], "design", "{result:#}");
    let class = &result["failure_classification"];
    assert_eq!(class["class"], "usage_limit", "{result:#}");
    assert_eq!(
        class["signal"], "durable scoped usage-limit evidence",
        "classed from the structured report, not from text: {result:#}"
    );
    let seen = format!(
        "{result}\n{}",
        String::from_utf8_lossy(&first.output.stderr)
    );
    assert!(
        !seen.to_lowercase().contains("hit your session limit"),
        "no limit message reached the runner: {seen}"
    );
    assert_eq!(f.events(), ["classify", "design"]);

    // Only the stand-in changes; the command and the recipe do not.
    f.mode("design", "success");
    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    let recovery = second.assert_resumed(&first, "design");
    assert_eq!(recovery["reason"], "usage_limit", "{recovery:#}");
    let restored = recovery["restored_steps"]
        .as_array()
        .unwrap_or_else(|| panic!("restored_steps: {recovery:#}"));
    assert!(
        restored.iter().any(|s| s["step_id"] == "classify"),
        "classify restored: {recovery:#}"
    );
    assert!(
        !restored.iter().any(|s| s["step_id"] == "design"),
        "design is not restored, it runs again: {recovery:#}"
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "design", "implement"],
        "classify ran once in all, design again and then implement"
    );
}
