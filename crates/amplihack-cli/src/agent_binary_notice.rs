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
//! session marker in this environment is said aloud too. It still wins: it is
//! layer 1 by design, and a deliberate cross-CLI run needs it to. But the docs
//! tell users to export it to choose a CLI, so a profile line written for one
//! CLI and still exported inside another's session is the same silent
//! wrong-CLI run.
//!
//! A session marker is evidence about this process's environment, which is not
//! always the caller's. tmux copies the environment of whatever process started
//! its server into the server's global environment, and every later
//! `new-session` starts from that copy (tmux(1), GLOBAL AND SESSION
//! ENVIRONMENT). A server started from a Copilot session hands `COPILOT_CLI=1`
//! to a run launched from Claude Code. The notice checks for that, names the
//! variable, and never claims the run was started from a session it cannot see
//! (crusty review of #1490).

use std::ffi::OsStr;
use std::process::{Command, Stdio};
use std::time::Duration;

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, ResolutionSource, SOURCE_ENV, SessionMarker,
    inherited_binary_is_default_guess, validate_binary_name,
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
    let from_tmux_server = marker_may_be_the_tmux_servers(resolution);
    if let Some(notice) = agent_binary_notice(
        lead,
        resolution,
        EnvBinaryValue::current(),
        from_tmux_server,
    ) {
        eprintln!("{notice}");
    }
}

/// `true` when the session marker the notice would rest on is one this tmux
/// server's global environment holds with the same value, so it may have been
/// copied from whatever started the server rather than set by a CLI session.
///
/// Asked only when the marker matters to the notice: it answered, or an
/// explicit value overrode it. Outside tmux, or when tmux cannot be asked, the
/// answer is `false` and the notice reads as it did before.
fn marker_may_be_the_tmux_servers(resolution: &Resolution) -> bool {
    let Some(marker) = resolution.session_marker else {
        return false;
    };
    let matters = match resolution.source {
        ResolutionSource::SessionMarker => true,
        ResolutionSource::Env => marker.binary != resolution.binary,
        ResolutionSource::LauncherContext | ResolutionSource::Default => false,
    };
    matters
        && std::env::var_os("TMUX").is_some_and(|tmux| !tmux.is_empty())
        && std::env::var_os(marker.variable).is_some_and(|value| {
            tmux_global_environment_holds(OsStr::new("tmux"), marker.variable, &value)
        })
}

/// How long `tmux show-environment` may take. It answers from the server's
/// memory; a server that has not answered by then is treated as not holding
/// the marker, and the run goes on.
const TMUX_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether `tmux show-environment -g <variable>` prints `<variable>=<value>`.
///
/// Inside a tmux pane the client finds the server through `$TMUX`. Any failure
/// -- no tmux binary, no server, a timeout, an unset or removed variable --
/// is `false`: nothing then suggests the marker came from the server.
fn tmux_global_environment_holds(tmux: &OsStr, variable: &str, value: &OsStr) -> bool {
    let mut command = Command::new(tmux);
    command
        .args(["show-environment", "-g", variable])
        .stdin(Stdio::null());
    let Ok(output) = crate::util::run_output_with_timeout(command, TMUX_QUERY_TIMEOUT) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Some(value) = value.to_str() else {
        return false;
    };
    let expected = format!("{variable}={value}");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line == expected)
}

/// The notice [`report_agent_binary`] prints, or `None` when the binary was
/// observed rather than inferred and no session marker contradicts it.
///
/// `from_tmux_server` is [`marker_may_be_the_tmux_servers`]: the session
/// marker is one the tmux server's global environment also holds. A marker
/// that answered is then announced, because it may be the server's starter
/// rather than the caller -- a silent copilot run from a Claude Code session
/// without the hand-off. An explicit value that overrode it is not told to
/// step aside for it.
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
    from_tmux_server: bool,
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
        (ResolutionSource::Env, _) => {
            return session_override_notice(lead, resolution, from_tmux_server);
        }
        (ResolutionSource::SessionMarker, _) => {
            return resolution
                .session_marker
                .filter(|_| from_tmux_server)
                .map(|marker| tmux_marker_notice(lead, marker));
        }
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
/// It names the variable and says what it implies, nothing more: a marker is
/// in this environment, which is not proof that a session of that CLI started
/// this run. When the tmux server holds the same marker it may be the server
/// starter's, and telling the user to unset the explicit value would hand the
/// run to exactly the wrong CLI, so that advice is left out.
///
/// The variable comes from `SESSION_MARKERS` and both binaries from the
/// allowlist, so echoing them is safe. The raw value is not quoted: the
/// resolver trims and lowercases it. Launcher contexts the walk-up skipped are
/// not listed; they did not decide this.
fn session_override_notice(
    lead: &str,
    resolution: &Resolution,
    from_tmux_server: bool,
) -> Option<String> {
    let SessionMarker {
        variable,
        binary: session,
    } = resolution
        .session_marker
        .filter(|marker| marker.binary != resolution.binary)?;
    let binary = &resolution.binary;
    Some(if from_tmux_server {
        format!(
            "amplihack: {lead} '{binary}' ({BINARY_ENV} is set). {variable}, a {session} \
             session marker, is set too, but this tmux server's global environment holds \
             the same value, so it may come from whatever started the server rather than \
             from a {session} session."
        )
    } else {
        format!(
            "amplihack: {lead} '{binary}' ({BINARY_ENV} is set and overrides {variable}, \
             the {session} session marker in this environment). If this is a {session} \
             session, unset {BINARY_ENV} to run under {session}."
        )
    })
}

/// The line for a session marker that answered but that the tmux server's
/// global environment holds as well.
///
/// Without the hand-off a detached `recipe run` sees the server's environment,
/// and a server started from a Copilot session gives it `COPILOT_CLI=1` however
/// it was launched. Before this line, that was a silent copilot run from a
/// Claude Code session: a marker counts as observed, and observed answers are
/// not announced.
fn tmux_marker_notice(lead: &str, marker: SessionMarker) -> String {
    let SessionMarker { variable, binary } = marker;
    format!(
        "amplihack: {lead} '{binary}' ({variable} is set, but this tmux server's global \
         environment holds the same value, so it may come from whatever started the server \
         rather than from a {binary} session). To hand a detached run the CLI you launch it \
         from, prefix its command with $(amplihack agent-binary --shell -w <dir>); to choose \
         one, set {BINARY_ENV}."
    )
}

#[cfg(test)]
mod tests {
    use super::{EnvBinaryValue, agent_binary_notice, tmux_global_environment_holds};
    use amplihack_utils::agent_binary::{
        Resolution, ResolutionSource, SessionMarker, UnusableContext,
    };
    use std::ffi::OsStr;
    use std::path::PathBuf;

    const CLAUDECODE: SessionMarker = SessionMarker {
        variable: "CLAUDECODE",
        binary: "claude",
    };
    const COPILOT_CLI: SessionMarker = SessionMarker {
        variable: "COPILOT_CLI",
        binary: "copilot",
    };

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
            for session_marker in [None, Some(CLAUDECODE)] {
                for env_value in ALL {
                    let mut resolution = resolved("claude", source);
                    resolution.session_marker = session_marker;
                    // Not even for a bad file: it did not decide this answer.
                    resolution.unusable_contexts = vec![empty_context_at("/r/x.json")];
                    assert_eq!(
                        agent_binary_notice(LEAD, &resolution, env_value, false),
                        None
                    );
                }
            }
        }
    }

    /// An explicit value that agrees with the marker overrides nothing,
    /// wherever the marker came from.
    #[test]
    fn an_explicit_value_agreeing_with_a_tmux_held_marker_needs_no_notice() {
        let mut resolution = resolved("claude", ResolutionSource::Env);
        resolution.session_marker = Some(CLAUDECODE);
        assert_eq!(
            agent_binary_notice(LEAD, &resolution, EnvBinaryValue::Usable, true),
            None
        );
    }

    /// Crusty round 3 asked what an export that conflicts with a Claude
    /// session resolves to. It resolves to the export, as documented -- and
    /// before this, nothing on stderr said a session marker had been
    /// overruled. The line names the variable and does not claim the run was
    /// started from that session: a marker is all it can see.
    #[test]
    fn an_explicit_value_overriding_a_session_marker_says_so() {
        let mut resolution = resolved("copilot", ResolutionSource::Env);
        resolution.session_marker = Some(CLAUDECODE);
        // A skipped file did not decide this either, so it stays out.
        resolution.unusable_contexts = vec![empty_context_at("/r/x.json")];
        let notice = agent_binary_notice(LEAD, &resolution, EnvBinaryValue::Usable, false).unwrap();
        assert_eq!(
            notice,
            "amplihack: agent steps will run under 'copilot' (AMPLIHACK_AGENT_BINARY is set \
             and overrides CLAUDECODE, the claude session marker in this environment). If \
             this is a claude session, unset AMPLIHACK_AGENT_BINARY to run under claude."
        );
        assert!(!notice.contains("started from"), "{notice}");
    }

    /// Crusty review of #1490, risk 1: a tmux server started from a Copilot
    /// session holds `COPILOT_CLI=1`, and a run launched into it from Claude
    /// Code with `AMPLIHACK_AGENT_BINARY=claude` was told it "overrides the
    /// copilot session it was started from" and to unset the variable -- which
    /// would have run it under copilot. When the server holds the marker, the
    /// line says so and gives no such advice.
    #[test]
    fn an_explicit_value_over_a_tmux_held_marker_is_not_told_to_yield() {
        let mut resolution = resolved("claude", ResolutionSource::Env);
        resolution.session_marker = Some(COPILOT_CLI);
        let notice = agent_binary_notice(LEAD, &resolution, EnvBinaryValue::Usable, true).unwrap();
        assert_eq!(
            notice,
            "amplihack: agent steps will run under 'claude' (AMPLIHACK_AGENT_BINARY is set). \
             COPILOT_CLI, a copilot session marker, is set too, but this tmux server's global \
             environment holds the same value, so it may come from whatever started the \
             server rather than from a copilot session."
        );
        assert!(!notice.contains("Unset"), "{notice}");
        assert!(!notice.contains("unset"), "{notice}");
        assert!(!notice.contains("started from"), "{notice}");
    }

    /// Crusty review of #1490, risk 1, without the hand-off: the far side of
    /// a detached launch answered `copilot (session_marker)` from the server's
    /// copy of its starter's environment, and said nothing. A marker the tmux
    /// server holds is now announced, with the way to hand the caller's CLI
    /// across.
    #[test]
    fn a_session_marker_the_tmux_server_holds_is_announced() {
        let mut resolution = resolved("copilot", ResolutionSource::SessionMarker);
        resolution.session_marker = Some(COPILOT_CLI);
        for env_value in ALL {
            let notice = agent_binary_notice(LEAD, &resolution, env_value, true).unwrap();
            assert_eq!(
                notice,
                "amplihack: agent steps will run under 'copilot' (COPILOT_CLI is set, but this \
                 tmux server's global environment holds the same value, so it may come from \
                 whatever started the server rather than from a copilot session). To hand a \
                 detached run the CLI you launch it from, prefix its command with \
                 $(amplihack agent-binary --shell -w <dir>); to choose one, set \
                 AMPLIHACK_AGENT_BINARY."
            );
        }
    }

    /// The tmux check is asked through a stand-in `tmux` that prints what
    /// `show-environment -g` would. Only an exact `NAME=value` line counts.
    #[cfg(unix)]
    #[test]
    fn the_tmux_server_holds_a_marker_only_with_the_same_value() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let stub = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        // Echoes its arguments' variable back as tmux does for a set one.
        let holds = stub(
            "holds",
            r#"[ "$1 $2" = "show-environment -g" ] && echo "$3=1""#,
        );
        // tmux prints `-NAME` for a variable removed with `set-environment -gu`.
        let removed = stub("removed", r#"echo "-$3""#);
        // tmux exits 1 with "unknown variable" for one it never had.
        let unknown = stub("unknown", r#"echo "unknown variable: $3" >&2; exit 1"#);

        let value = OsStr::new("1");
        assert!(tmux_global_environment_holds(
            holds.as_os_str(),
            "COPILOT_CLI",
            value
        ));
        assert!(!tmux_global_environment_holds(
            holds.as_os_str(),
            "COPILOT_CLI",
            OsStr::new("2")
        ));
        assert!(!tmux_global_environment_holds(
            removed.as_os_str(),
            "COPILOT_CLI",
            value
        ));
        assert!(!tmux_global_environment_holds(
            unknown.as_os_str(),
            "COPILOT_CLI",
            value
        ));
        assert!(!tmux_global_environment_holds(
            dir.path().join("no-such-tmux").as_os_str(),
            "COPILOT_CLI",
            value
        ));
    }

    #[test]
    fn a_default_with_nothing_inherited_says_nothing_was_found() {
        let notice = agent_binary_notice(
            LEAD,
            &resolved("copilot", ResolutionSource::Default),
            EnvBinaryValue::Unset,
            false,
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
            false,
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
            let notice = agent_binary_notice(
                LEAD,
                &resolved("copilot", source),
                EnvBinaryValue::Rejected,
                false,
            )
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
            let notice = agent_binary_notice(LEAD, &resolution, env_value, false).unwrap();
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
            let notice =
                agent_binary_notice(LEAD, &resolution, EnvBinaryValue::Unset, false).unwrap();
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
