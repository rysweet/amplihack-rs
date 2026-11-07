//! Level 4: an `AMPLIHACK_AGENT_BINARY` inherited from a parent of the other
//! kind, whether tagged `session:<other>`, `default:<other>` or untagged,
//! never overrides an explicit `amplihack claude` or `amplihack codex`.

use super::fixture::{Cli, Fixture};

/// `explicit` is launched under an inherited `AMPLIHACK_AGENT_BINARY` naming
/// the other CLI, tagged `<tag>:<other>`, or untagged when `tag` is `None`.
fn explicit_beats(explicit: Cli, tag: Option<&str>) {
    let inherited = explicit.other();
    let fx = Fixture::new();
    let mut cmd = match explicit {
        Cli::Claude => fx.cmd(["claude", "--", "-p", "x"]),
        Cli::Codex => fx.cmd(["codex", "--", "exec", "-"]),
    };
    cmd.env("AMPLIHACK_AGENT_BINARY", inherited.name());
    if let Some(tag) = tag {
        cmd.env(
            "AMPLIHACK_AGENT_BINARY_SOURCE",
            format!("{tag}:{}", inherited.name()),
        );
    }
    fx.run_ok(cmd);

    let records = fx.records();
    assert_eq!(records.len(), 1, "{}", fx.logs());
    records[0].assert_cli(explicit);
}

#[test]
fn claude_beats_inherited_session_codex() {
    explicit_beats(Cli::Claude, Some("session"));
}

#[test]
fn claude_beats_inherited_default_codex() {
    explicit_beats(Cli::Claude, Some("default"));
}

#[test]
fn claude_beats_inherited_untagged_codex() {
    explicit_beats(Cli::Claude, None);
}

#[test]
fn codex_beats_inherited_session_claude() {
    explicit_beats(Cli::Codex, Some("session"));
}

#[test]
fn codex_beats_inherited_default_claude() {
    explicit_beats(Cli::Codex, Some("default"));
}

#[test]
fn codex_beats_inherited_untagged_claude() {
    explicit_beats(Cli::Codex, None);
}
