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
//! override notice, or answer for a nested step. The assignments that follow
//! set the caller's answer. GNU, BSD (macOS) and BusyBox `env` all take `-u`,
//! and further `NAME=value` words after the hand-off (the templates'
//! `AMPLIHACK_HOME=...`) are read by `env` the same way. This form works on
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
    default_guess_tag, tmux_server_marker_tag,
};
use anyhow::{Context, Result};

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
    println!("{}", render(&resolution, shell, from_tmux_server));
    Ok(())
}

/// The line `amplihack agent-binary` prints.
///
/// With `shell`, an `env` prefix for the command that follows it: `-u` for
/// every session marker, then the two `NAME=value` assignments. Marker names
/// are constants, and both values are allowlisted or built from allowlisted
/// names, so nothing needs quoting. Without it, the binary and the layer that
/// supplied it, for a person (or `detect_cli`) to read.
///
/// `from_tmux_server` is the session marker the answer rests on when the tmux
/// server's global environment holds it too
/// (`agent_binary_notice::answer_from_tmux_server_marker`); it becomes the
/// tag.
pub(crate) fn render(
    resolution: &Resolution,
    shell: bool,
    from_tmux_server: Option<SessionMarker>,
) -> String {
    if !shell {
        return format!("{} ({})", resolution.binary, resolution.source.label());
    }
    let tag = match (resolution.source, from_tmux_server) {
        (ResolutionSource::Default, _) => default_guess_tag(&resolution.binary),
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
        .map(|(variable, _)| format!("-u {variable} "))
        .collect();
    format!(
        "env {unset}{BINARY_ENV}={} {SOURCE_ENV}={tag}",
        resolution.binary
    )
}

#[cfg(test)]
mod tests {
    use super::render;
    use amplihack_utils::agent_binary::{
        Resolution, ResolutionSource, SESSION_MARKERS, SessionMarker,
    };

    /// `env -u` for every marker, from the canonical list.
    fn unset_every_marker() -> String {
        let unset: Vec<String> = SESSION_MARKERS
            .iter()
            .map(|(variable, _)| format!("-u {variable}"))
            .collect();
        format!("env {}", unset.join(" "))
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
            render(&resolved("copilot", ResolutionSource::Default), true, None),
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
        let line = render(
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

    /// Anything observed or read is handed on with an empty tag, so a stale
    /// `default:<same binary>` in the receiving environment cannot veto it.
    #[test]
    fn every_other_answer_is_handed_on_with_the_tag_cleared() {
        for source in [
            ResolutionSource::Env,
            ResolutionSource::SessionMarker,
            ResolutionSource::LauncherContext,
        ] {
            assert_eq!(
                render(&resolved("claude", source), true, None),
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
                render(&resolved("copilot", source), true, Some(copilot_cli)),
                format!(
                    "{} AMPLIHACK_AGENT_BINARY=copilot \
                     AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:COPILOT_CLI",
                    unset_every_marker()
                ),
                "{source:?}"
            );
        }
        // The plain form `detect_cli` reads is unchanged.
        assert_eq!(
            render(
                &resolved("copilot", ResolutionSource::SessionMarker),
                false,
                Some(copilot_cli)
            ),
            "copilot (session_marker)"
        );
    }

    #[test]
    fn the_plain_form_names_the_source() {
        assert_eq!(
            render(
                &resolved("claude", ResolutionSource::SessionMarker),
                false,
                None
            ),
            "claude (session_marker)"
        );
    }
}
