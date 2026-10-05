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
//! where the caller's markers are, to
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
//! `AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>`, and anything else gets an
//! empty tag, so that a stale tag in the receiving environment cannot turn it
//! back into a guess. This mirrors `EnvBuilder::with_resolved_agent_binary`.
//! With the markers gone and the tag kept, the far side resolves a guess the
//! way the caller did: to the same default, still announced as one.
//!
//! The plain form, `<binary> (<source>)`, is read by the migrate skill's
//! `detect_cli`, which takes the first word.

use std::path::PathBuf;

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, ResolutionSource, SESSION_MARKERS, SOURCE_ENV, default_guess_tag,
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
    crate::agent_binary_notice::report_agent_binary(
        crate::agent_binary_notice::Reporter::AgentBinary,
        &resolution,
    );
    println!("{}", render(&resolution, shell));
    Ok(())
}

/// The line `amplihack agent-binary` prints.
///
/// With `shell`, an `env` prefix for the command that follows it: `-u` for
/// every session marker, then the two `NAME=value` assignments. Marker names
/// are constants, and both values are allowlisted or built from allowlisted
/// names, so nothing needs quoting. Without it, the binary and the layer that
/// supplied it, for a person (or `detect_cli`) to read.
pub(crate) fn render(resolution: &Resolution, shell: bool) -> String {
    if !shell {
        return format!("{} ({})", resolution.binary, resolution.source.label());
    }
    let tag = match resolution.source {
        ResolutionSource::Default => default_guess_tag(&resolution.binary),
        ResolutionSource::Env
        | ResolutionSource::SessionMarker
        | ResolutionSource::LauncherContext => String::new(),
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
    use amplihack_utils::agent_binary::{Resolution, ResolutionSource, SESSION_MARKERS};

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
            render(&resolved("copilot", ResolutionSource::Default), true),
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
        let line = render(&resolved("claude", ResolutionSource::SessionMarker), true);
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
                render(&resolved("claude", source), true),
                format!(
                    "{} AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=",
                    unset_every_marker()
                ),
                "{source:?}"
            );
        }
    }

    #[test]
    fn the_plain_form_names_the_source() {
        assert_eq!(
            render(&resolved("claude", ResolutionSource::SessionMarker), false),
            "claude (session_marker)"
        );
    }
}
