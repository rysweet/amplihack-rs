//! The stderr notice for an agent binary that was inferred rather than
//! observed, or that overrides the session it runs in (issues #1335, #1481,
//! #1525).
//!
//! The resolver's own warnings go through `tracing`, which is silent at the
//! default filter, and issue #1335 was a run that executed every step under
//! the wrong CLI for hours with nothing in its output saying why. Anything
//! about to launch agents on an inferred answer, or to hand that answer to
//! something that will, says so here instead.
//!
//! An explicit `AMPLIHACK_AGENT_BINARY` that names a different CLI than the
//! live session marker is said aloud too. It still wins: it is layer 1 by
//! design, and a deliberate cross-CLI run needs it to. But the docs tell users
//! to export it to choose a CLI, so a profile line written for one CLI and
//! still exported inside another's session is the same silent wrong-CLI run.

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, ResolutionSource, SOURCE_ENV, inherited_binary_is_default_guess,
    validate_binary_name,
};

/// What `AMPLIHACK_AGENT_BINARY` held when the binary was resolved, as far as
/// the notice needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvBinaryValue {
    /// Not set.
    Unset,
    /// Set to an allowlisted name the resolver would use.
    Usable,
    /// Set, but tagged as a parent's default guess for the same binary.
    InheritedGuess,
    /// Set, but not an allowlisted name (empty, a typo, a path...).
    Rejected,
}

impl EnvBinaryValue {
    /// Classify this process's own `AMPLIHACK_AGENT_BINARY`.
    pub(crate) fn current() -> Self {
        if inherited_binary_is_default_guess() {
            return EnvBinaryValue::InheritedGuess;
        }
        match std::env::var_os(BINARY_ENV) {
            None => EnvBinaryValue::Unset,
            Some(raw) => match raw.to_str().and_then(validate_binary_name) {
                Some(_) => EnvBinaryValue::Usable,
                None => EnvBinaryValue::Rejected,
            },
        }
    }
}

/// Print [`agent_binary_notice`] to stderr when there is one.
///
/// A nested run under a deliberate override that can still see a session
/// marker repeats the line. It is just as true there, and agent-step stderr
/// is shown only when a step fails.
///
/// `lead` says what the binary is about to be used for, e.g. "agent steps
/// will run under".
pub(crate) fn report_agent_binary(lead: &str, resolution: &Resolution) {
    if let Some(notice) = agent_binary_notice(lead, resolution, EnvBinaryValue::current()) {
        eprintln!("{notice}");
    }
}

/// The notice [`report_agent_binary`] prints, or `None` when the binary was
/// observed rather than inferred and no session marker contradicts it.
///
/// The reason must match what the user did. A tagged inherited guess is
/// skipped by the resolver, and setting the same value again does not help
/// while the tag still names it. A rejected value was set, just not to
/// anything usable; the resolver's own warning about it is hidden at the
/// default tracing filter. The rejected value itself is never echoed.
///
/// Issue #1525: a launcher context is named by the path the resolver actually
/// read, which the walk-up may have found in an ancestor. Every context file
/// it passed over because it could not be used gets a line of its own, with
/// the reason. Otherwise an empty or malformed file goes unmentioned, and the
/// user is left to guess why the default answered.
pub(crate) fn agent_binary_notice(
    lead: &str,
    resolution: &Resolution,
    env_value: EnvBinaryValue,
) -> Option<String> {
    const REJECTED: &str = "AMPLIHACK_AGENT_BINARY is set but is not one of amplifier, \
                            claude, codex or copilot";
    let read_from = || {
        let file = resolution
            .context_file
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "a launcher_context.json".to_string());
        format!("read from {file}")
    };
    let why = match (resolution.source, env_value) {
        (ResolutionSource::Env, _) => return session_override_notice(lead, resolution),
        (ResolutionSource::SessionMarker, _) => return None,
        (ResolutionSource::LauncherContext, EnvBinaryValue::Rejected) => {
            format!("{REJECTED}; {}", read_from())
        }
        (ResolutionSource::LauncherContext, _) => read_from(),
        (ResolutionSource::Default, EnvBinaryValue::InheritedGuess) => {
            "AMPLIHACK_AGENT_BINARY was inherited as a parent's default guess and no \
             agent session marker was found"
                .to_string()
        }
        (ResolutionSource::Default, EnvBinaryValue::Rejected) => {
            format!("{REJECTED}, and no agent session marker was found")
        }
        (ResolutionSource::Default, _) => {
            "no AMPLIHACK_AGENT_BINARY or agent session marker was found".to_string()
        }
    };
    let how = match env_value {
        EnvBinaryValue::InheritedGuess => {
            format!("Set AMPLIHACK_AGENT_BINARY and unset {SOURCE_ENV} to choose an agent CLI.")
        }
        EnvBinaryValue::Rejected => "Set AMPLIHACK_AGENT_BINARY to one of amplifier, claude, \
                                     codex or copilot to choose an agent CLI."
            .to_string(),
        EnvBinaryValue::Unset | EnvBinaryValue::Usable => {
            "Set AMPLIHACK_AGENT_BINARY to choose a different agent CLI.".to_string()
        }
    };
    let mut notice = format!("amplihack: {lead} '{}' ({why}). {how}", resolution.binary);
    for unusable in &resolution.unusable_contexts {
        notice.push_str(&format!(
            "\namplihack: ignored {}: it {}. Fix or delete it.",
            unusable.path.display(),
            unusable.reason
        ));
    }
    Some(notice)
}

/// The line for an explicit value that outranked a session marker naming a
/// different CLI, or `None` when they agree or no marker was seen.
///
/// Both names come out of the allowlist, so echoing them is safe. The raw
/// value is not quoted: the resolver trims and lowercases it. Launcher
/// contexts the walk-up skipped are not listed; they did not decide this.
fn session_override_notice(lead: &str, resolution: &Resolution) -> Option<String> {
    let session = resolution
        .session_marker
        .as_deref()
        .filter(|session| *session != resolution.binary)?;
    Some(format!(
        "amplihack: {lead} '{}' ({BINARY_ENV} is set and overrides the {session} \
         session it was started from). Unset {BINARY_ENV} to run under {session}.",
        resolution.binary
    ))
}

#[cfg(test)]
mod tests {
    use super::{EnvBinaryValue, agent_binary_notice};
    use amplihack_utils::agent_binary::{Resolution, ResolutionSource, UnusableContext};
    use std::path::PathBuf;

    const LEAD: &str = "agent steps will run under";

    const ALL: [EnvBinaryValue; 4] = [
        EnvBinaryValue::Unset,
        EnvBinaryValue::Usable,
        EnvBinaryValue::InheritedGuess,
        EnvBinaryValue::Rejected,
    ];

    fn resolved(binary: &str, source: ResolutionSource) -> Resolution {
        Resolution {
            binary: binary.to_string(),
            source,
            context_file: None,
            unusable_contexts: Vec::new(),
            session_marker: None,
        }
    }

    fn empty_context_at(path: &str) -> UnusableContext {
        UnusableContext {
            path: PathBuf::from(path),
            reason: "is empty".to_string(),
        }
    }

    #[test]
    fn an_observed_binary_needs_no_notice() {
        for source in [ResolutionSource::Env, ResolutionSource::SessionMarker] {
            // No marker, or one that agrees with the answer.
            for session_marker in [None, Some("claude")] {
                for env_value in ALL {
                    let mut resolution = resolved("claude", source);
                    resolution.session_marker = session_marker.map(str::to_string);
                    // Not even for a bad file: it did not decide this answer.
                    resolution.unusable_contexts = vec![empty_context_at("/r/x.json")];
                    assert_eq!(agent_binary_notice(LEAD, &resolution, env_value), None);
                }
            }
        }
    }

    /// Crusty round 3 asked what an export that conflicts with a Claude
    /// session resolves to. It resolves to the export, as documented -- and
    /// before this, nothing on stderr said a live session had been overruled.
    #[test]
    fn an_explicit_value_overriding_a_live_session_says_so() {
        let mut resolution = resolved("copilot", ResolutionSource::Env);
        resolution.session_marker = Some("claude".to_string());
        // A skipped file did not decide this either, so it stays out.
        resolution.unusable_contexts = vec![empty_context_at("/r/x.json")];
        let notice = agent_binary_notice(LEAD, &resolution, EnvBinaryValue::Usable).unwrap();
        assert_eq!(
            notice,
            "amplihack: agent steps will run under 'copilot' (AMPLIHACK_AGENT_BINARY is set \
             and overrides the claude session it was started from). Unset \
             AMPLIHACK_AGENT_BINARY to run under claude."
        );
    }

    #[test]
    fn a_default_with_nothing_inherited_says_nothing_was_found() {
        let notice = agent_binary_notice(
            LEAD,
            &resolved("copilot", ResolutionSource::Default),
            EnvBinaryValue::Unset,
        )
        .unwrap();
        assert!(
            notice.starts_with("amplihack: agent steps will run under 'copilot' ("),
            "{notice}"
        );
        assert!(
            notice.contains("no AMPLIHACK_AGENT_BINARY or agent session marker was found"),
            "{notice}"
        );
        assert!(
            !notice.contains("AMPLIHACK_AGENT_BINARY_SOURCE"),
            "{notice}"
        );
        assert_eq!(notice.lines().count(), 1, "{notice}");
    }

    /// Quality-audit S4: the variable was there, only tagged. Saying none was
    /// found sends the user to set a value that the tag would still veto.
    #[test]
    fn a_default_over_an_inherited_guess_names_the_tag() {
        let notice = agent_binary_notice(
            LEAD,
            &resolved("copilot", ResolutionSource::Default),
            EnvBinaryValue::InheritedGuess,
        )
        .unwrap();
        assert!(
            !notice.contains("no AMPLIHACK_AGENT_BINARY"),
            "the variable was present: {notice}"
        );
        assert!(notice.contains("default guess"), "{notice}");
        assert!(
            notice.contains("unset AMPLIHACK_AGENT_BINARY_SOURCE"),
            "{notice}"
        );
    }

    /// Quality-audit cycle 6 S1: a value outside the allowlist was set, so
    /// "none was found" is wrong; say it was rejected and list what is valid.
    #[test]
    fn a_rejected_value_is_named_as_rejected() {
        for source in [ResolutionSource::Default, ResolutionSource::LauncherContext] {
            let notice =
                agent_binary_notice(LEAD, &resolved("copilot", source), EnvBinaryValue::Rejected)
                    .unwrap();
            assert!(!notice.contains("no AMPLIHACK_AGENT_BINARY"), "{notice}");
            assert!(
                notice.contains("is set but is not one of amplifier, claude, codex or copilot"),
                "{notice}"
            );
        }
    }

    /// Crusty concern 9: the walk-up visits ancestors, so the notice names the
    /// file that was read, not a fixed relative path.
    #[test]
    fn a_launcher_context_answer_names_the_file_that_was_read() {
        let mut resolution = resolved("codex", ResolutionSource::LauncherContext);
        resolution.context_file = Some(PathBuf::from(
            "/home/u/repo/.claude/runtime/launcher_context.json",
        ));
        for env_value in [EnvBinaryValue::Unset, EnvBinaryValue::Rejected] {
            let notice = agent_binary_notice(LEAD, &resolution, env_value).unwrap();
            assert!(
                notice.contains("read from /home/u/repo/.claude/runtime/launcher_context.json"),
                "{notice}"
            );
        }
    }

    /// Issue #1525: a file the walk-up passed over is named, with its reason,
    /// whichever inferred layer answered.
    #[test]
    fn every_unusable_context_gets_a_line_with_its_path_and_reason() {
        for source in [ResolutionSource::Default, ResolutionSource::LauncherContext] {
            let mut resolution = resolved("copilot", source);
            resolution.unusable_contexts = vec![
                empty_context_at("/r/a/.claude/runtime/launcher_context.json"),
                UnusableContext {
                    path: PathBuf::from("/r/.claude/runtime/launcher_context.json"),
                    reason: "is not valid JSON (line 1, column 2)".to_string(),
                },
            ];
            let notice = agent_binary_notice(LEAD, &resolution, EnvBinaryValue::Unset).unwrap();
            let lines: Vec<&str> = notice.lines().collect();
            assert_eq!(lines.len(), 3, "{notice}");
            assert_eq!(
                lines[1],
                "amplihack: ignored /r/a/.claude/runtime/launcher_context.json: it is empty. \
                 Fix or delete it."
            );
            assert_eq!(
                lines[2],
                "amplihack: ignored /r/.claude/runtime/launcher_context.json: it is not valid \
                 JSON (line 1, column 2). Fix or delete it."
            );
        }
    }
}
