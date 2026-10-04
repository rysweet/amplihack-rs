//! `amplihack agent-binary`: print the resolved agent binary and where it came
//! from, so it can be handed to a process that will not inherit this shell's
//! environment (issue #1525).
//!
//! Once a tmux server is running, `tmux new-session` gives the new command the
//! server's environment, not the caller's. The session markers that tell
//! `amplihack recipe run` which CLI the caller is in do not arrive, and the run
//! falls back to the default (#1335). The hand-off carries the answer across:
//!
//! ```sh
//! tmux new-session -d -s run "$(amplihack agent-binary --shell) amplihack recipe run ..."
//! ```
//!
//! `$(...)` inside the double-quoted command expands in the caller's shell,
//! where the markers are still present, and the inline `VAR=value cmd` form
//! works on every tmux; `new-session -e` only exists from tmux 3.2.
//!
//! The source travels with the value. Handing over `AMPLIHACK_AGENT_BINARY`
//! alone would turn a default guess into an untagged instruction, and the
//! nested `amplihack <cli>` would persist it to `launcher_context.json` --
//! issue #1481 again. So `--shell` always prints both variables: a guess gets
//! `AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>`, and anything else gets an
//! empty tag, so that a stale tag in the receiving environment cannot turn it
//! back into a guess. This mirrors `EnvBuilder::with_resolved_agent_binary`.

use std::path::PathBuf;

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, ResolutionSource, SOURCE_ENV, default_guess_tag,
};
use anyhow::{Context, Result};

/// Resolve from `dir` (default: the current directory) and print the answer.
///
/// An inferred answer is explained on stderr, which stays on the caller's
/// terminal when stdout is captured by `$(...)`.
pub fn run_agent_binary(shell: bool, dir: Option<PathBuf>) -> Result<()> {
    let dir = match dir {
        Some(dir) => dir,
        None => std::env::current_dir().context("cannot read the current directory")?,
    };
    let resolution = crate::env_builder::resolve_agent_binary_in(&dir);
    crate::agent_binary_notice::report_inferred_agent_binary(
        "resolved the agent binary to",
        &resolution,
    );
    println!("{}", render(&resolution, shell));
    Ok(())
}

/// The line `amplihack agent-binary` prints.
///
/// With `shell`, two `NAME=value` assignments for an inline `VAR=val cmd`
/// prefix. Both values are allowlisted or built from allowlisted names, so
/// they need no quoting. Without it, the binary and the layer that supplied
/// it, for a person to read.
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
    format!("{BINARY_ENV}={} {SOURCE_ENV}={tag}", resolution.binary)
}

#[cfg(test)]
mod tests {
    use super::render;
    use amplihack_utils::agent_binary::{Resolution, ResolutionSource};

    fn resolved(binary: &str, source: ResolutionSource) -> Resolution {
        Resolution {
            binary: binary.to_string(),
            source,
            context_file: None,
            unusable_contexts: Vec::new(),
        }
    }

    /// A default guess must cross the hand-off still tagged, or the nested
    /// launcher would persist it as a session's choice (#1481).
    #[test]
    fn a_default_guess_is_handed_on_tagged() {
        assert_eq!(
            render(&resolved("copilot", ResolutionSource::Default), true),
            "AMPLIHACK_AGENT_BINARY=copilot AMPLIHACK_AGENT_BINARY_SOURCE=default:copilot"
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
                "AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=",
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
