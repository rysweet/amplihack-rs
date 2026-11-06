//! Level 2: every agent step of a recipe run, including the agent steps of
//! nested sub-recipes, uses the CLI of the session that started the run.
//!
//! These tests run the real `recipe-runner-rs`, not the stub, because the
//! runner itself starts sub-recipes and builds each agent step's environment.
//! A three-level recipe (`issue1983-outer` → `issue1983-sub` →
//! `issue1983-subsub`, see `script::NESTED_RECIPES`) has one agent step per
//! level; each step goes runner → `amplihack <cli>` → fake CLI.
//!
//! The runner is `RECIPE_RUNNER_RS_PATH` from the test's own environment (CI
//! sets it to the runner built at `claude-plugin/recipe-runner.rev`), else
//! `recipe-runner-rs` on `PATH`. When `RECIPE_RUNNER_RS_PATH` or
//! `AMPLIHACK_1983_REQUIRE_RECIPE_RUNNER` is set, a missing runner fails the
//! test; otherwise the test says it is skipping and returns.

use super::fixture::{Cli, Fixture};
use super::script::STUB_RUNNER_REV;
use std::path::{Path, PathBuf};
use std::process::Command;

fn real_runner() -> Option<PathBuf> {
    let required = std::env::var_os("RECIPE_RUNNER_RS_PATH").is_some()
        || std::env::var_os("AMPLIHACK_1983_REQUIRE_RECIPE_RUNNER").is_some();
    let found = std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|dir| dir.join("recipe-runner-rs"))
                .find(|candidate| candidate.is_file())
        })
        .filter(|r| {
            Command::new(r)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        });
    match found {
        Some(r) => Some(r),
        None if required => panic!(
            "recipe-runner-rs is required for the level 2 tests: install it at \
             claude-plugin/recipe-runner.rev (scripts/install-pinned-recipe-runner.sh)"
        ),
        None => {
            eprintln!("recipe-runner-rs unavailable; skipping the level 2 real-runner test");
            None
        }
    }
}

/// Assert the three nested agent steps ran, in order, each under `cli`.
fn assert_nested_steps(fx: &Fixture, cli: Cli) {
    let steps: Vec<_> = fx
        .records()
        .into_iter()
        .filter(|r| r.get("prompt_tag") != "<none>")
        .collect();
    let tags: Vec<&str> = steps.iter().map(|r| r.get("prompt_tag")).collect();
    assert_eq!(
        tags,
        [
            "issue1983-level-outer",
            "issue1983-level-sub",
            "issue1983-level-subsub"
        ],
        "each level's agent step must run once, in order\n{}",
        fx.logs()
    );
    for step in &steps {
        step.assert_cli(cli);
        assert_eq!(step.get("nonce_set"), "true", "{step:?}");
    }
}

fn every_nested_step_uses(cli: Cli) {
    let Some(runner) = real_runner() else { return };
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["recipe", "run", "issue1983-outer.yaml"]);
    fx.session(&mut cmd, cli);
    fx.use_real_runner(&mut cmd, &runner);
    fx.run_ok(cmd);
    assert_nested_steps(&fx, cli);
}

#[test]
fn every_nested_step_of_a_claude_run_uses_claude() {
    every_nested_step_uses(Cli::Claude);
}

#[test]
fn every_nested_step_of_a_codex_run_uses_codex() {
    every_nested_step_uses(Cli::Codex);
}

/// test (Claude Code shell, every Claude marker set) -> `amplihack codex` ->
/// fake codex -> `amplihack recipe run` -> real runner -> sub-recipes ->
/// `amplihack codex` -> fake codex. The Claude markers exist upstream of the
/// run, so the check that none reaches a step can fail.
#[test]
fn codex_run_from_a_claude_shell_keeps_claude_markers_out_of_nested_steps() {
    let Some(runner) = real_runner() else { return };
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["codex", "--", "exec", "-"]);
    fx.session(&mut cmd, Cli::Claude);
    fx.all_claude_markers(&mut cmd);
    cmd.env(
        "FAKE_THEN",
        r#""$AMPLIHACK_UNDER_TEST" recipe run "$WORK_DIR/issue1983-outer.yaml""#,
    );
    fx.use_real_runner(&mut cmd, &runner);
    fx.run_ok(cmd);

    let records = fx.records();
    let top: Vec<_> = records.iter().filter(|r| r.get("role") == "top").collect();
    assert_eq!(top.len(), 1, "{}", fx.logs());
    assert_eq!(
        records.len(),
        4,
        "the top codex plus three steps\n{}",
        fx.logs()
    );
    assert_nested_steps(&fx, Cli::Codex);
    for record in &records {
        record.assert_cli(Cli::Codex);
        record.assert_no_markers_of(Cli::Claude);
    }
}

/// The stub runner used by the level 1 tests copies the agent-step command of
/// one runner revision. Moving the pin must make someone check the copy.
#[test]
fn stub_runner_matches_the_pinned_runner_revision() {
    let pin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../claude-plugin/recipe-runner.rev");
    let rev = std::fs::read_to_string(&pin).unwrap();
    assert!(
        rev.trim().starts_with(STUB_RUNNER_REV),
        "claude-plugin/recipe-runner.rev is now {}, but STUB_RUNNER in \
         tests/issue_1983_cli_stays_put/script.rs was copied from {STUB_RUNNER_REV}: \
         re-derive STUB_RUNNER from the runner's build_agent_command \
         (src/adapters/cli_subprocess.rs) at the new revision, then update \
         STUB_RUNNER_REV",
        rev.trim()
    );
}
