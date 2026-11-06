//! Level 2: every agent step of a recipe run, including those of nested
//! sub-recipes, uses the CLI of the session that started the run.

use super::fixture::{Cli, Fixture};

fn every_step_uses(cli: Cli) {
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["recipe", "run", "probe.yaml"]);
    fx.session(&mut cmd, cli);
    fx.run_ok(cmd);

    let steps = fx.step_records();
    let roles: Vec<&str> = steps.iter().map(|r| r.get("role")).collect();
    assert_eq!(
        roles,
        ["step:outer", "step:sub", "step:subsub"],
        "{}",
        fx.logs()
    );
    for step in &steps {
        step.assert_cli(cli);
        assert_eq!(step.get("nonce_set"), "true", "{step:?}");
    }
    if cli == Cli::Codex {
        // Known limit D5: a Claude marker beside `session:codex` resolves to
        // claude, so none may reach a codex step.
        for record in fx.records() {
            record.assert_no_markers_of(Cli::Claude);
            assert_eq!(record.get("children_see"), "codex", "{record:?}");
        }
    }
}

#[test]
fn every_step_of_a_claude_run_uses_claude() {
    every_step_uses(Cli::Claude);
}

#[test]
fn every_step_of_a_codex_run_uses_codex() {
    every_step_uses(Cli::Codex);
}
