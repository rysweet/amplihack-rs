//! `amplihack agent-binary`: print the resolved agent binary and where it came
//! from, so it can be handed to a command started in a new tmux session, which
//! does not inherit this shell's environment (issue #1525). `setsid` and
//! `nohup` do inherit it and need no hand-off.
//!
//! `tmux new-session` does not give the new command the caller's environment.
//! It gives it the server's global environment, which tmux copied from
//! whatever process started the server (tmux(1), GLOBAL AND SESSION
//! ENVIRONMENT). The caller's session markers are missing there, and the
//! starter's are present. A server started from a plain shell sends the run to
//! the default. A server started from a Copilot session hands every later
//! session `COPILOT_CLI=1`, and a run launched into it from Claude Code
//! resolves to copilot from that marker -- an observation as far as the far
//! side can tell (#1335, and the crusty review of #1490). The hand-off carries
//! the caller's whole view across instead:
//!
//! ```sh
//! tmux new-session -d -s run "$(amplihack agent-binary --shell) amplihack recipe run ..."
//! ```
//!
//! `$(...)` inside the double-quoted command expands in the caller's shell,
//! where the caller's markers are. In single quotes it would expand in the new
//! session instead, from the server's markers; see the tag below. It expands
//! to
//!
//! ```text
//! env -u CLAUDECODE ... -u COPILOT_AGENT AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=
//! ```
//!
//! `env -u` removes every `SESSION_MARKERS` variable the far side holds, so no
//! marker of the server's starter can contradict the answer, set off the
//! override notice, or answer for a nested step. It also removes the
//! `PARENT_WATCH_ENV` variables (issue #1976): a server first started from a
//! recipe step holds that step's `AMPLIHACK_PARENT_RECIPE_PID`, `_STARTTIME`
//! and `AMPLIHACK_AUTODRIVE_STATE_DIR`, and a run started in a later session
//! would otherwise watch a parent that has gone and stop at once with exit 75.
//!
//! It removes the effort and model variables too (`inherit::ENV_INPUTS`, issue
//! #1883): `AMPLIHACK_PARENT_EFFORT`, `_EFFORT_BINARY`, `_MODEL` and
//! `_MODEL_BINARY`, which every `amplihack <cli>` launch exports, and `CLAUDE_CODE_EFFORT_LEVEL` and
//! `ANTHROPIC_MODEL`, which a launch sets for its agent when it passes an
//! inherited value. A server first started inside an amplihack session holds
//! that session's values, and a `recipe run` in a later session would rank
//! them above everything but its own `-c` context, so every step would run the
//! server starter's model at its effort, logged as coming "from the parent
//! amplihack launch" (crusty review of #1892, round 6). After the removals the
//! hand-off sets the caller's own value of each one it holds, so the far side
//! sees what a `setsid` launch from the caller would: the effort and model of
//! the caller's session, or none. A value crosses only when it passes the
//! launch path's value check unchanged and every character is one a shell
//! leaves alone in both places the line is read (see [`shell_inert`]); a model
//! such as `opus[1m]` does not, so it is removed and not set, and the caller's
//! terminal gets one line naming the variable and how to pass it instead.
//! `AMPLIHACK_PARENT_EFFORT_BINARY` and `AMPLIHACK_PARENT_MODEL_BINARY` each
//! cross only with the effort or model they describe.
//!
//! The assignments that follow set the caller's answer. GNU, BSD (macOS) and
//! BusyBox `env` all take `-u`, and further `NAME=value` words after the
//! hand-off (the templates' `AMPLIHACK_HOME=...`) are read by `env` the same
//! way. This form works on
//! every tmux; `new-session -e` only exists from tmux 3.2, and cannot unset.
//!
//! The source travels with the value. Handing over `AMPLIHACK_AGENT_BINARY`
//! alone would turn a default guess into an untagged instruction, and the
//! nested `amplihack <cli>` would persist it to `launcher_context.json` --
//! issue #1481 again. So `--shell` always prints both variables: a guess gets
//! `AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>`, and anything else gets a
//! tag that is not a guess -- empty, or the one below -- so that a stale tag in
//! the receiving environment cannot turn it back into a guess. This mirrors `EnvBuilder::with_resolved_agent_binary`.
//! With the markers gone and the tag kept, the far side resolves a guess the
//! way the caller did: to the same default, still announced as one.
//!
//! An answer read from a launcher context is *not* tagged. It crosses with the
//! empty tag, as an explicit value, so the far side's run log does not say it
//! was inferred; the `read from <file>` line this command prints on the
//! caller's terminal is the only record (crusty review of #1490 at 9b207c7e).
//! A tag could not name the file anyway: `$(...)` splits its output on
//! whitespace and does not remove quotes (POSIX Shell Command Language
//! §2.6.5, §2.6.7), so only words without spaces -- a binary, a marker
//! variable -- cross intact, and a path may hold anything. It arises only for
//! a caller with no session marker (cron, a plain shell), since a marker
//! outranks the file.
//!
//! An answer from a session marker crosses with the empty tag too, including
//! one from an `AMPLIHACK_AGENT_BINARY` that a launcher tagged
//! `session:<cli>` (crusty review of #1490 at baaafb18). Elsewhere amplihack
//! exports such an answer tagged as a session description, so that it ranks
//! with the markers of whatever runs below. The hand-off is different: it is
//! the caller deciding what the far side runs, with every marker there
//! removed. Tagged, the far side's run would take its own server's identical
//! `session:<cli>` for the server's copy and name the server for an answer
//! the caller gave.
//!
//! An answer read from a session marker that the tmux server's global
//! environment also holds gets `AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:<marker
//! variable>`. The marker may be the server starter's, not the caller's, and
//! the likeliest way here is this line in a single-quoted tmux command, run in
//! the new session. With an empty tag the far side would see an explicit
//! choice, and the only line naming the marker would be this command's stderr
//! in the tmux pane. With the tag, the run names it in its own log (crusty
//! review of #1490 at ef441d81). The value is still honoured; the tag is not a
//! guess and nothing treats it as one.
//!
//! The plain form, `<binary> (<source>)`, is read by the migrate skill's
//! `detect_cli`, which takes the first word.

use std::path::PathBuf;

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, ResolutionSource, SESSION_MARKERS, SOURCE_ENV, SessionMarker,
    default_guess_tag, tmux_server_marker_tag, user_config_tag,
};
use anyhow::{Context, Result};

use crate::commands::launch::inherit::{
    ENV_INPUTS, PARENT_EFFORT_BINARY_ENV, PARENT_EFFORT_ENV, PARENT_MODEL_BINARY_ENV,
    PARENT_MODEL_ENV, checked,
};
use crate::commands::recipe::run::parent_watch::PARENT_WATCH_ENV;

/// Resolve from `dir` (default: the current directory) and print the answer.
///
/// An inferred answer is explained on stderr, which stays on the caller's
/// terminal when stdout is captured by `$(...)`. The resolver's log lines go
/// there too: the binary's tracing subscriber writes to stderr
/// (`bins/amplihack/src/main.rs`), so `RUST_LOG` cannot reach the stdout that
/// `$(...)` splices into the command line.
pub fn run_agent_binary(shell: bool, dir: Option<PathBuf>) -> Result<()> {
    let dir = match dir {
        Some(dir) => dir,
        None => std::env::current_dir().context("cannot read the current directory")?,
    };
    let resolution = crate::env_builder::resolve_agent_binary_in(&dir);
    let from_tmux_server = crate::agent_binary_notice::report_agent_binary(
        crate::agent_binary_notice::Reporter::AgentBinary,
        &resolution,
    );
    let inherit = if shell {
        inherit_handoff(|name| std::env::var(name).ok())
    } else {
        InheritHandoff::default()
    };
    if let Some(line) = inherit.not_handed_on_line() {
        eprintln!("{line}");
    }
    println!(
        "{}",
        render(&resolution, shell, from_tmux_server, &inherit.set)
    );
    Ok(())
}

/// The caller's effort and model variables, as the hand-off passes them on
/// (issue #1883; see the module doc).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct InheritHandoff {
    /// `NAME=value` assignments for the far side, in `ENV_INPUTS` order.
    pub(crate) set: Vec<(&'static str, String)>,
    /// Variables the caller holds whose value cannot cross the hand-off. The
    /// far side gets them removed, not set.
    pub(crate) not_handed_on: Vec<&'static str>,
}

impl InheritHandoff {
    /// The one stderr line for the caller's terminal, when anything was held
    /// back. It names the variables, never their values (S9).
    pub(crate) fn not_handed_on_line(&self) -> Option<String> {
        if self.not_handed_on.is_empty() {
            return None;
        }
        Some(format!(
            "amplihack: not handed on: {}. Only a valid option value made of letters, \
             digits and - _ . : / @ % + , can cross the hand-off, so a recipe run in the \
             new session does not inherit this effort or model. Pass -c effort=<level> or \
             -c model=<id> to that run, in single quotes inside the double-quoted tmux \
             command.",
            self.not_handed_on.join(", ")
        ))
    }
}

/// A character the hand-off line may hold in a value: one that neither a
/// shell's word splitting and pathname expansion (the line read as the result
/// of an unquoted `$(...)`, POSIX §2.6.5, §2.6.6) nor its parser (the line
/// read as the source of the tmux command, in the new session's shell) treats
/// as anything but itself. `[`, `]`, `*`, `?` would glob, and zsh refuses a
/// glob that matches nothing; `~`, `$`, quotes and whitespace would expand,
/// split or be removed.
fn shell_inert(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '@' | '%' | '+' | ',')
}

/// What the hand-off sets and holds back for the effort and model variables,
/// from `read` (the caller's environment). An unset or empty variable is
/// simply removed on the far side.
pub(crate) fn inherit_handoff(read: impl Fn(&str) -> Option<String>) -> InheritHandoff {
    let mut out = InheritHandoff::default();
    for name in ENV_INPUTS {
        let Some(value) = read(name).filter(|v| !v.is_empty()) else {
            continue;
        };
        if let Some(described) = provenance_of(name) {
            // Provenance travels only with the value it describes, which
            // `ENV_INPUTS` lists first.
            let value_crossed = out.set.iter().any(|(set, _)| *set == described);
            if value_crossed && value.chars().all(shell_inert) {
                out.set.push((name, value));
            }
            continue;
        }
        let crosses =
            checked(&value).as_deref() == Some(value.as_str()) && value.chars().all(shell_inert);
        if crosses {
            out.set.push((name, value));
        } else {
            out.not_handed_on.push(name);
        }
    }
    out
}

/// The value a provenance variable describes, or `None` for a value variable.
fn provenance_of(name: &str) -> Option<&'static str> {
    match name {
        PARENT_EFFORT_BINARY_ENV => Some(PARENT_EFFORT_ENV),
        PARENT_MODEL_BINARY_ENV => Some(PARENT_MODEL_ENV),
        _ => None,
    }
}

/// The line `amplihack agent-binary` prints.
///
/// With `shell`, an `env` prefix for the command that follows it: `-u` for
/// every session marker, every `PARENT_WATCH_ENV` variable and every
/// `inherit::ENV_INPUTS` variable, then the `inherit` assignments
/// ([`inherit_handoff`]), then the two agent-binary assignments. Marker names
/// are constants, the agent-binary values are allowlisted or built from
/// allowlisted names, and an `inherit` value holds only [`shell_inert`]
/// characters, so nothing needs quoting. Without it, the binary and the layer
/// that supplied it, for a person (or `detect_cli`) to read.
///
/// `from_tmux_server` is the session marker the answer rests on when the tmux
/// server's global environment holds it too
/// (`agent_binary_notice::answer_from_tmux_server_marker`); it becomes the
/// tag.
pub(crate) fn render(
    resolution: &Resolution,
    shell: bool,
    from_tmux_server: Option<SessionMarker>,
    inherit: &[(&'static str, String)],
) -> String {
    if !shell {
        return format!("{} ({})", resolution.binary, resolution.source.label());
    }
    let tag = match (resolution.source, from_tmux_server) {
        (ResolutionSource::Default, _) => default_guess_tag(&resolution.binary),
        // Issue #1606: the configured value decided, not any marker the tmux
        // server holds, so it crosses as a configured value whatever the
        // server's environment says.
        (ResolutionSource::UserConfig, _) => user_config_tag(&resolution.binary),
        (_, Some(marker)) => tmux_server_marker_tag(marker),
        (
            ResolutionSource::Env
            | ResolutionSource::SessionMarker
            | ResolutionSource::LauncherContext,
            None,
        ) => String::new(),
    };
    let unset: String = SESSION_MARKERS
        .iter()
        .map(|(variable, _)| *variable)
        .chain(PARENT_WATCH_ENV)
        .chain(ENV_INPUTS)
        .map(|variable| format!("-u {variable} "))
        .collect();
    let set: String = inherit
        .iter()
        .map(|(name, value)| format!("{name}={value} "))
        .collect();
    format!(
        "env {unset}{set}{BINARY_ENV}={} {SOURCE_ENV}={tag}",
        resolution.binary
    )
}

#[cfg(test)]
mod tests {
    use super::{ENV_INPUTS, InheritHandoff, PARENT_WATCH_ENV, inherit_handoff, render};
    use amplihack_utils::agent_binary::{
        Resolution, ResolutionSource, SESSION_MARKERS, SessionMarker,
    };
    use std::collections::BTreeMap;

    /// `env -u` for every marker, parent-watch and effort/model variable,
    /// from the canonical lists.
    fn unset_every_marker() -> String {
        let unset: Vec<String> = SESSION_MARKERS
            .iter()
            .map(|(variable, _)| *variable)
            .chain(PARENT_WATCH_ENV)
            .chain(ENV_INPUTS)
            .map(|variable| format!("-u {variable}"))
            .collect();
        format!("env {}", unset.join(" "))
    }

    /// [`render`] for a caller with no effort or model variables.
    fn render3(r: &Resolution, shell: bool, marker: Option<SessionMarker>) -> String {
        render(r, shell, marker, &[])
    }

    /// [`inherit_handoff`] over a fixed caller environment.
    fn handoff(vars: &[(&str, &str)]) -> InheritHandoff {
        let env: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        inherit_handoff(|name| env.get(name).cloned())
    }

    fn resolved(binary: &str, source: ResolutionSource) -> Resolution {
        Resolution {
            binary: binary.to_string(),
            source,
            context_file: None,
            unusable_contexts: Vec::new(),
            session_marker: None,
        }
    }

    /// A default guess must cross the hand-off still tagged, or the nested
    /// launcher would persist it as a session's choice (#1481).
    #[test]
    fn a_default_guess_is_handed_on_tagged() {
        assert_eq!(
            render3(&resolved("copilot", ResolutionSource::Default), true, None),
            format!(
                "{} AMPLIHACK_AGENT_BINARY=copilot \
                 AMPLIHACK_AGENT_BINARY_SOURCE=default:copilot",
                unset_every_marker()
            )
        );
    }

    /// Crusty review of #1490: a tmux server holds its starter's markers, and
    /// the far side sees them. The hand-off removes every one, so the far side
    /// sees only what the caller resolved.
    #[test]
    fn the_hand_off_unsets_every_session_marker_first() {
        let line = render3(
            &resolved("claude", ResolutionSource::SessionMarker),
            true,
            None,
        );
        let words: Vec<&str> = line.split(' ').collect();
        assert_eq!(words[0], "env");
        for (variable, _) in SESSION_MARKERS {
            let at = words
                .iter()
                .position(|word| word == variable)
                .unwrap_or_else(|| panic!("{variable} is not unset: {line}"));
            assert_eq!(words[at - 1], "-u", "{line}");
            assert!(
                at < words.len() - 2,
                "the unsets come before the assignments: {line}"
            );
        }
        assert_eq!(
            &words[words.len() - 2..],
            [
                "AMPLIHACK_AGENT_BINARY=claude",
                "AMPLIHACK_AGENT_BINARY_SOURCE="
            ]
        );
    }

    /// Crusty review of #2112: a tmux server first started from a recipe
    /// step holds that step's parent-watch variables globally. The hand-off
    /// removes all three before the assignments, so a run in a later session
    /// is a top-level run and does not stop at once with exit 75.
    #[test]
    fn the_hand_off_unsets_the_parent_watch_variables() {
        let line = render3(&resolved("claude", ResolutionSource::Env), true, None);
        let words: Vec<&str> = line.split(' ').collect();
        assert_eq!(words[0], "env");
        for variable in [
            "AMPLIHACK_PARENT_RECIPE_PID",
            "AMPLIHACK_PARENT_RECIPE_STARTTIME",
            "AMPLIHACK_AUTODRIVE_STATE_DIR",
        ] {
            let at = words
                .iter()
                .position(|word| *word == variable)
                .unwrap_or_else(|| panic!("{variable} is not unset: {line}"));
            assert_eq!(words[at - 1], "-u", "{line}");
            assert!(at < words.len() - 2, "the unsets come first: {line}");
        }
    }

    /// Crusty review of #1892, round 6: a tmux server first started inside an
    /// amplihack session holds that session's effort and model variables
    /// globally. The hand-off removes all six, then sets the caller's, before
    /// the agent-binary assignments, so a run in a later session inherits the
    /// caller's values and not the server starter's.
    #[test]
    fn the_hand_off_replaces_the_effort_and_model_variables() {
        let caller = handoff(&[
            ("AMPLIHACK_PARENT_EFFORT", "xhigh"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "claude"),
            ("AMPLIHACK_PARENT_MODEL", "claude-opus-5-5"),
            ("AMPLIHACK_PARENT_MODEL_BINARY", "claude"),
            ("CLAUDE_CODE_EFFORT_LEVEL", "xhigh"),
            ("ANTHROPIC_MODEL", "claude-opus-5-5"),
        ]);
        assert_eq!(caller.not_handed_on, Vec::<&str>::new());
        assert_eq!(caller.not_handed_on_line(), None);
        let line = render(
            &resolved("claude", ResolutionSource::SessionMarker),
            true,
            None,
            &caller.set,
        );
        assert_eq!(
            line,
            format!(
                "{} AMPLIHACK_PARENT_EFFORT=xhigh AMPLIHACK_PARENT_EFFORT_BINARY=claude \
                 AMPLIHACK_PARENT_MODEL=claude-opus-5-5 \
                 AMPLIHACK_PARENT_MODEL_BINARY=claude CLAUDE_CODE_EFFORT_LEVEL=xhigh \
                 ANTHROPIC_MODEL=claude-opus-5-5 AMPLIHACK_AGENT_BINARY=claude \
                 AMPLIHACK_AGENT_BINARY_SOURCE=",
                unset_every_marker()
            )
        );
        let words: Vec<&str> = line.split(' ').collect();
        for variable in [
            "AMPLIHACK_PARENT_EFFORT",
            "AMPLIHACK_PARENT_EFFORT_BINARY",
            "AMPLIHACK_PARENT_MODEL",
            "AMPLIHACK_PARENT_MODEL_BINARY",
            "CLAUDE_CODE_EFFORT_LEVEL",
            "ANTHROPIC_MODEL",
        ] {
            let unset = words
                .iter()
                .position(|word| *word == variable)
                .unwrap_or_else(|| panic!("{variable} is not unset: {line}"));
            assert_eq!(words[unset - 1], "-u", "{line}");
            let set = words
                .iter()
                .position(|word| word.starts_with(&format!("{variable}=")))
                .unwrap_or_else(|| panic!("{variable} is not set: {line}"));
            assert!(unset < set, "the unset comes first: {line}");
        }
    }

    /// A caller outside any amplihack session (a plain shell, cron) holds
    /// none of them, so the far side holds none either: the server's are
    /// removed and nothing is set.
    #[test]
    fn a_caller_without_values_hands_on_their_removal_only() {
        let caller = handoff(&[("AMPLIHACK_PARENT_EFFORT", ""), ("HOME", "/h")]);
        assert_eq!(caller, InheritHandoff::default());
        assert_eq!(
            render(
                &resolved("claude", ResolutionSource::Env),
                true,
                None,
                &caller.set
            ),
            format!(
                "{} AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=",
                unset_every_marker()
            )
        );
    }

    /// A value that a shell would glob, split or expand cannot cross: the
    /// `$(...)` output is read both as an expansion result and as tmux's
    /// command source. It is removed, not set, and named (without its value)
    /// on the caller's terminal. Its provenance does not cross without it.
    #[test]
    fn a_value_a_shell_would_change_is_removed_and_named() {
        let caller = handoff(&[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "claude"),
            ("AMPLIHACK_PARENT_MODEL", "claude-opus-5[1m]"),
            ("AMPLIHACK_PARENT_MODEL_BINARY", "claude"),
            ("CLAUDE_CODE_EFFORT_LEVEL", "high"),
            ("ANTHROPIC_MODEL", "claude opus"),
        ]);
        assert_eq!(
            caller.set,
            vec![
                ("AMPLIHACK_PARENT_EFFORT", "high".to_string()),
                ("AMPLIHACK_PARENT_EFFORT_BINARY", "claude".to_string()),
                ("CLAUDE_CODE_EFFORT_LEVEL", "high".to_string()),
            ]
        );
        assert_eq!(
            caller.not_handed_on,
            vec!["AMPLIHACK_PARENT_MODEL", "ANTHROPIC_MODEL"]
        );
        let line = caller.not_handed_on_line().unwrap();
        assert!(
            line.starts_with("amplihack: not handed on: AMPLIHACK_PARENT_MODEL, ANTHROPIC_MODEL. "),
            "{line}"
        );
        assert!(
            !line.contains("[1m]") && !line.contains("claude opus"),
            "{line}"
        );
        for value in [
            "a*", "a?", "~a", "$a", "a'b", "a\"b", "a;b", "a`b`", "a\\b", "a#b",
        ] {
            let caller = handoff(&[("ANTHROPIC_MODEL", value)]);
            assert_eq!(caller.set, Vec::new(), "{value}");
            assert_eq!(caller.not_handed_on, vec!["ANTHROPIC_MODEL"], "{value}");
        }
    }

    /// A value the launch path would refuse or change (D9: trimmed, no
    /// leading `-`) is not handed on as it stands; ids with the punctuation
    /// real model ids use (Bedrock, Vertex, gateway routes) are.
    #[test]
    fn only_a_value_the_launch_path_takes_unchanged_crosses() {
        for value in ["-x", " high", "high "] {
            let caller = handoff(&[("AMPLIHACK_PARENT_EFFORT", value)]);
            assert_eq!(caller.set, Vec::new(), "{value:?}");
            assert_eq!(caller.not_handed_on, vec!["AMPLIHACK_PARENT_EFFORT"]);
        }
        for value in [
            "us.anthropic.claude-opus-5-5-v1:0",
            "claude-sonnet-4-5@20250929",
            "azure/gpt-5.1",
            "gpt-5.1-codex+beta,x%y",
        ] {
            let caller = handoff(&[("AMPLIHACK_PARENT_MODEL", value)]);
            assert_eq!(
                caller.set,
                vec![("AMPLIHACK_PARENT_MODEL", value.to_string())],
                "{value}"
            );
        }
    }

    /// `AMPLIHACK_PARENT_EFFORT_BINARY` describes the effort; without an
    /// effort that crosses, a stale provenance is not handed on, so a far-side
    /// copilot cannot take a server starter's effort as its own.
    #[test]
    fn effort_provenance_crosses_only_with_its_effort() {
        let caller = handoff(&[("AMPLIHACK_PARENT_EFFORT_BINARY", "copilot")]);
        assert_eq!(caller, InheritHandoff::default());
        let caller = handoff(&[
            ("AMPLIHACK_PARENT_EFFORT", " high"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "copilot"),
        ]);
        assert_eq!(caller.set, Vec::new());
        assert_eq!(caller.not_handed_on, vec!["AMPLIHACK_PARENT_EFFORT"]);
        let caller = handoff(&[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "copilot"),
        ]);
        assert_eq!(
            caller.set,
            vec![
                ("AMPLIHACK_PARENT_EFFORT", "high".to_string()),
                ("AMPLIHACK_PARENT_EFFORT_BINARY", "copilot".to_string()),
            ]
        );
    }

    /// `AMPLIHACK_PARENT_MODEL_BINARY` describes the model; without a model
    /// that crosses, a stale provenance is not handed on.
    #[test]
    fn model_provenance_crosses_only_with_its_model() {
        let caller = handoff(&[("AMPLIHACK_PARENT_MODEL_BINARY", "codex")]);
        assert_eq!(caller, InheritHandoff::default());
        let caller = handoff(&[
            ("AMPLIHACK_PARENT_MODEL", "gpt-5.1"),
            ("AMPLIHACK_PARENT_MODEL_BINARY", "codex"),
        ]);
        assert_eq!(
            caller.set,
            vec![
                ("AMPLIHACK_PARENT_MODEL", "gpt-5.1".to_string()),
                ("AMPLIHACK_PARENT_MODEL_BINARY", "codex".to_string()),
            ]
        );
    }

    /// Anything observed or read is handed on with an empty tag, so a stale
    /// `default:<same binary>` in the receiving environment cannot veto it.
    /// That includes a launcher-context answer: it crosses as an explicit
    /// value, and only the caller's terminal names the file (see the module
    /// doc for why no tag carries it).
    #[test]
    fn every_other_answer_is_handed_on_with_the_tag_cleared() {
        for source in [
            ResolutionSource::Env,
            ResolutionSource::SessionMarker,
            ResolutionSource::LauncherContext,
        ] {
            assert_eq!(
                render3(&resolved("claude", source), true, None),
                format!(
                    "{} AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=",
                    unset_every_marker()
                ),
                "{source:?}"
            );
        }
    }

    /// Crusty review of #1490 at ef441d81: an answer read from a marker the
    /// tmux server holds -- in a single-quoted tmux command, the server's own
    /// -- is handed on naming that marker, so the far side's run can say so.
    /// The markers are still all unset and the value is still set.
    #[test]
    fn an_answer_from_a_marker_the_tmux_server_holds_is_handed_on_naming_it() {
        let copilot_cli = SessionMarker {
            variable: "COPILOT_CLI",
            binary: "copilot",
        };
        for source in [ResolutionSource::SessionMarker, ResolutionSource::Env] {
            assert_eq!(
                render3(&resolved("copilot", source), true, Some(copilot_cli)),
                format!(
                    "{} AMPLIHACK_AGENT_BINARY=copilot \
                     AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:COPILOT_CLI",
                    unset_every_marker()
                ),
                "{source:?}"
            );
        }
        // A launcher's `session:<cli>` export the server holds is named the
        // same way, by the tag's variable (crusty review of #1490 at
        // baaafb18).
        let described = SessionMarker {
            variable: amplihack_utils::agent_binary::SOURCE_ENV,
            binary: "codex",
        };
        assert_eq!(
            render3(
                &resolved("codex", ResolutionSource::SessionMarker),
                true,
                Some(described)
            ),
            format!(
                "{} AMPLIHACK_AGENT_BINARY=codex \
                 AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:AMPLIHACK_AGENT_BINARY_SOURCE",
                unset_every_marker()
            )
        );
        // The plain form `detect_cli` reads is unchanged.
        assert_eq!(
            render3(
                &resolved("copilot", ResolutionSource::SessionMarker),
                false,
                Some(copilot_cli)
            ),
            "copilot (session_marker)"
        );
    }

    /// Issue #1606: a configured `agent.binary` crosses tagged as one, even
    /// when the tmux server holds another CLI's marker.
    #[test]
    fn a_configured_binary_is_handed_on_tagged_user_config() {
        let copilot_cli = SessionMarker {
            variable: "COPILOT_CLI",
            binary: "copilot",
        };
        for marker in [None, Some(copilot_cli)] {
            assert_eq!(
                render3(
                    &resolved("claude", ResolutionSource::UserConfig),
                    true,
                    marker
                ),
                format!(
                    "{} AMPLIHACK_AGENT_BINARY=claude \
                     AMPLIHACK_AGENT_BINARY_SOURCE=user_config:claude",
                    unset_every_marker()
                ),
                "{marker:?}"
            );
        }
        assert_eq!(
            render3(
                &resolved("claude", ResolutionSource::UserConfig),
                false,
                None
            ),
            "claude (user_config)"
        );
    }

    #[test]
    fn the_plain_form_names_the_source() {
        assert_eq!(
            render3(
                &resolved("claude", ResolutionSource::SessionMarker),
                false,
                None
            ),
            "claude (session_marker)"
        );
    }
}
