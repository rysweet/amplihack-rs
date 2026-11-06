//! Level 4: an `AMPLIHACK_AGENT_BINARY` inherited from a parent of the other
//! kind, whether tagged `session:<other>`, `default:<other>` or untagged,
//! never overrides an explicit `amplihack claude` or `amplihack codex`.

use super::fixture::{Cli, Fixture};

/// How the inherited value is tagged.
#[derive(Clone, Copy)]
enum Tag {
    Session,
    Default,
    Untagged,
}

fn explicit_beats(explicit: Cli, inherited: Cli, tag: Tag) {
    assert_ne!(explicit, inherited);
    let fx = Fixture::new();
    let mut cmd = match explicit {
        Cli::Claude => fx.cmd(["claude", "--", "-p", "x"]),
        Cli::Codex => fx.cmd(["codex", "--", "exec", "-"]),
    };
    cmd.env("AMPLIHACK_AGENT_BINARY", inherited.name());
    match tag {
        Tag::Session => {
            cmd.env(
                "AMPLIHACK_AGENT_BINARY_SOURCE",
                format!("session:{}", inherited.name()),
            );
        }
        Tag::Default => {
            cmd.env(
                "AMPLIHACK_AGENT_BINARY_SOURCE",
                format!("default:{}", inherited.name()),
            );
        }
        Tag::Untagged => {
            cmd.env_remove("AMPLIHACK_AGENT_BINARY_SOURCE");
        }
    }
    fx.run_ok(cmd);

    let records = fx.records();
    assert_eq!(records.len(), 1, "{}", fx.logs());
    records[0].assert_cli(explicit);
    if explicit == Cli::Codex {
        records[0].assert_no_markers_of(Cli::Claude);
    }
}

#[test]
fn claude_beats_inherited_session_codex() {
    explicit_beats(Cli::Claude, Cli::Codex, Tag::Session);
}

#[test]
fn claude_beats_inherited_default_codex() {
    explicit_beats(Cli::Claude, Cli::Codex, Tag::Default);
}

#[test]
fn claude_beats_inherited_untagged_codex() {
    explicit_beats(Cli::Claude, Cli::Codex, Tag::Untagged);
}

#[test]
fn codex_beats_inherited_session_claude() {
    explicit_beats(Cli::Codex, Cli::Claude, Tag::Session);
}

#[test]
fn codex_beats_inherited_default_claude() {
    explicit_beats(Cli::Codex, Cli::Claude, Tag::Default);
}

#[test]
fn codex_beats_inherited_untagged_claude() {
    explicit_beats(Cli::Codex, Cli::Claude, Tag::Untagged);
}
