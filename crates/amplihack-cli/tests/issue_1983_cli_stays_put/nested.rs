//! Level 1: an amplihack command started inside a claude or codex session
//! hands that session's CLI to everything it starts.

use super::fixture::{Cli, Fixture};

fn recipe_run_stays(cli: Cli) {
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["recipe", "run", "probe.yaml"]);
    fx.session(&mut cmd, cli);
    fx.run_ok(cmd);

    assert_eq!(
        fx.runner_env(),
        (cli.name().to_string(), format!("session:{}", cli.name())),
        "the runner must be handed the session's CLI\n{}",
        fx.logs()
    );
    let steps = fx.step_records();
    assert!(!steps.is_empty(), "no agent step ran\n{}", fx.logs());
    for step in &steps {
        step.assert_cli(cli);
    }
}

#[test]
fn claude_session_runs_recipe_under_claude() {
    recipe_run_stays(Cli::Claude);
}

#[test]
fn codex_session_runs_recipe_under_codex() {
    recipe_run_stays(Cli::Codex);
}

/// test -> `amplihack codex` -> fake codex -> `amplihack recipe run` ->
/// stub runner -> `amplihack codex` -> fake codex, from a parent shell that
/// still holds a Claude session's environment.
#[test]
fn codex_session_launching_an_agent_command_stays_codex() {
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["codex", "--", "exec", "-"]);
    fx.session(&mut cmd, Cli::Claude);
    cmd.env(
        "FAKE_THEN",
        r#""$AMPLIHACK_UNDER_TEST" recipe run "$WORK_DIR/probe.yaml""#,
    );
    fx.run_ok(cmd);

    let records = fx.records();
    let top: Vec<_> = records.iter().filter(|r| r.get("role") == "top").collect();
    assert_eq!(top.len(), 1, "{}", fx.logs());
    top[0].assert_cli(Cli::Codex);
    assert_eq!(
        fx.runner_env(),
        ("codex".to_string(), "session:codex".to_string()),
        "{}",
        fx.logs()
    );
    let steps = fx.step_records();
    assert_eq!(steps.len(), 3, "{}", fx.logs());
    for step in &steps {
        step.assert_cli(Cli::Codex);
        step.assert_no_markers_of(Cli::Claude);
    }
}
