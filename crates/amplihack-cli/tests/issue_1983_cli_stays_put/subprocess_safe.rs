//! Level 3: `amplihack <cli> --subprocess-safe` started from a parent of the
//! other kind runs the CLI it names.

use super::fixture::{Cli, Fixture};

#[test]
fn claude_subprocess_safe_from_a_codex_parent_runs_claude() {
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["claude", "--subprocess-safe", "--", "-p", "x"]);
    fx.session(&mut cmd, Cli::Codex);
    fx.run_ok(cmd);

    let records = fx.records();
    assert_eq!(records.len(), 1, "{}", fx.logs());
    records[0].assert_cli(Cli::Claude);
}

#[test]
fn codex_subprocess_safe_from_a_claude_parent_runs_codex() {
    let fx = Fixture::new();
    let mut cmd = fx.cmd(["codex", "--subprocess-safe", "--", "exec", "-"]);
    fx.session(&mut cmd, Cli::Claude);
    fx.all_claude_markers(&mut cmd);
    fx.run_ok(cmd);

    let records = fx.records();
    assert_eq!(records.len(), 1, "{}", fx.logs());
    records[0].assert_cli(Cli::Codex);
    records[0].assert_no_markers_of(Cli::Claude);
}
