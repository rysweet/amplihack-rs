//! Level 5: the user explicitly asks for the other CLI, through a user-set
//! `AMPLIHACK_AGENT_BINARY` or `agent.binary` config, and gets it.

use super::fixture::{Cli, Fixture};

/// A `cli` session in which the user sets `AMPLIHACK_AGENT_BINARY` to the
/// other CLI. The launcher's `session:<cli>` tag stays beside it, as it would
/// in the agent's shell; a tag naming another binary does not describe the
/// value, so the value is the user's.
fn session_told_other(cli: Cli) {
    let want = cli.other();
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["recipe", "run", "probe.yaml"]);
    fx.session(&mut cmd, cli);
    cmd.env("AMPLIHACK_AGENT_BINARY", want.name());
    fx.run_ok(cmd);

    let (binary, _) = fx.runner_env();
    assert_eq!(binary, want.name(), "{}", fx.logs());
    let steps = fx.step_records();
    assert_eq!(steps.len(), 3, "{}", fx.logs());
    for step in &steps {
        step.assert_cli(want);
    }
}

/// No session evidence at all; `[agent] binary` in the user config decides.
fn config_selects(cli: Cli) {
    let fx = Fixture::new();
    fx.write_config_binary(cli);
    let cmd = fx.cmd(["recipe", "run", "probe.yaml"]);
    fx.run_ok(cmd);

    assert_eq!(
        fx.runner_env(),
        (
            cli.name().to_string(),
            format!("user_config:{}", cli.name())
        ),
        "{}",
        fx.logs()
    );
    let steps = fx.step_records();
    assert_eq!(steps.len(), 3, "{}", fx.logs());
    for step in &steps {
        step.assert_cli(cli);
    }
}

#[test]
fn claude_session_told_codex_dispatches_codex() {
    session_told_other(Cli::Claude);
}

#[test]
fn codex_session_told_claude_dispatches_claude() {
    session_told_other(Cli::Codex);
}

#[test]
fn config_agent_binary_codex_dispatches_codex() {
    config_selects(Cli::Codex);
}

#[test]
fn config_agent_binary_claude_dispatches_claude() {
    config_selects(Cli::Claude);
}
