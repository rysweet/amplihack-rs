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
//!
//! The check compares a per-session ID where the CLI exports one. Claude Code
//! sets `CLAUDECODE=1` in every session, so a Claude Code session in a pane of
//! a server another Claude Code session started holds the same `CLAUDECODE`
//! as the server, and comparing that value cannot tell the two apart.
//! `CLAUDE_CODE_SESSION_ID` can (crusty round 2 of #1490).
//!
//! A rejected `AMPLIHACK_AGENT_BINARY` is named whatever answered in its
//! place, a session marker included. The user named a CLI and got another one,
//! which is #1335 whether the default or a marker supplied the other one
//! (crusty review of #1490 at 7053698c). Only the variable is named, never the
//! value.
//!
//! The hand-off does not erase that evidence. When `amplihack agent-binary
//! --shell` resolves from a marker the tmux server holds, it hands the answer
//! on tagged with the marker's name, and the run that receives it prints the
//! line in its own log. Otherwise a hand-off in a single-quoted tmux command,
//! which runs in the new session and reads the server's markers, would be
//! worse than none: the run would see an explicit value and say nothing
//! (crusty review of #1490 at ef441d81).

use std::ffi::{OsStr, OsString};
use std::process::{Command, Stdio};
use std::time::Duration;

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, ResolutionSource, SOURCE_ENV, SessionMarker,
    inherited_binary_is_default_guess, inherited_tmux_server_marker, validate_binary_name,
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
    /// Set and used, and tagged by `amplihack agent-binary --shell` as read
    /// from this session marker while the tmux server's global environment
    /// held it too (`amplihack_utils::agent_binary::tmux_server_marker_tag`).
    FromTmuxServerMarker(SessionMarker),
    /// Set, but not an allowlisted name (empty, a typo, a path...).
    Rejected,
}

impl EnvBinaryValue {
    /// Classify this process's own `AMPLIHACK_AGENT_BINARY`.
    pub(crate) fn current() -> Self {
        if inherited_binary_is_default_guess() {
            return EnvBinaryValue::InheritedGuess;
        }
        if let Some(marker) = inherited_tmux_server_marker() {
            return EnvBinaryValue::FromTmuxServerMarker(marker);
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

/// Which command is reporting. It decides the notice's lead-in and which
/// advice fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reporter {
    /// `amplihack recipe run`, about to launch agent steps under the answer.
    RecipeRun,
    /// `amplihack agent-binary`, printing the answer, usually to build a
    /// hand-off with `--shell`. It is never told to use that hand-off: it is
    /// the hand-off (crusty round 2 of #1490).
    AgentBinary,
}

impl Reporter {
    /// What the binary is about to be used for.
    fn lead(self) -> &'static str {
        match self {
            Reporter::RecipeRun => "agent steps will run under",
            Reporter::AgentBinary => "resolved the agent binary to",
        }
    }
}

/// Why a set `AMPLIHACK_AGENT_BINARY` was not used. It never quotes the value.
const REJECTED: &str = "AMPLIHACK_AGENT_BINARY is set but is not one of amplifier, claude, \
                        codex or copilot";

/// What to set to choose a CLI after a rejected value: the variable and the
/// names it accepts.
const SET_TO_AN_ALLOWED_NAME: &str =
    "AMPLIHACK_AGENT_BINARY to one of amplifier, claude, codex or copilot";

/// Print [`agent_binary_notice`] to stderr when there is one, and return
/// [`answer_from_tmux_server_marker`] for the same resolution.
///
/// A nested run under a deliberate override that can still see a session
/// marker repeats the line. It is just as true there, and agent-step stderr
/// is shown only when a step fails.
pub(crate) fn report_agent_binary(
    reporter: Reporter,
    resolution: &Resolution,
) -> Option<SessionMarker> {
    let from_tmux_server = marker_may_be_the_tmux_servers(resolution);
    let env_value = EnvBinaryValue::current();
    if let Some(notice) = agent_binary_notice(reporter, resolution, env_value, from_tmux_server) {
        eprintln!("{notice}");
    }
    answer_from_tmux_server_marker(resolution, env_value, from_tmux_server)
}

/// The session marker the answer rests on, when the tmux server's global
/// environment holds it too: the marker answered and the server holds it, or
/// the answer arrived tagged as such by an earlier hand-off. `None` otherwise,
/// including for an explicit value that overrode such a marker, which rests on
/// the explicit value.
///
/// `amplihack agent-binary --shell` hands this on as
/// `AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:<variable>`, so the run on the
/// far side names the marker in its own log (crusty review of #1490 at
/// ef441d81).
pub(crate) fn answer_from_tmux_server_marker(
    resolution: &Resolution,
    env_value: EnvBinaryValue,
    from_tmux_server: bool,
) -> Option<SessionMarker> {
    match (resolution.source, env_value) {
        (ResolutionSource::SessionMarker, _) if from_tmux_server => resolution.session_marker,
        (ResolutionSource::Env, EnvBinaryValue::FromTmuxServerMarker(marker)) => Some(marker),
        _ => None,
    }
}

/// `true` when the session marker the notice would rest on may be this tmux
/// server's copy of whatever started the server, rather than set by a CLI
/// session; see [`tmux_server_holds_marker`].
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
        && tmux_server_holds_marker(OsStr::new("tmux"), marker, |variable| {
            std::env::var_os(variable)
        })
}

/// Variables a CLI sets to a value unique to each session, keyed by the
/// binary its session markers imply.
///
/// Claude Code's markers carry the same value in every session
/// (`CLAUDECODE=1`), so a server started from one Claude Code session holds
/// the same marker as every other Claude Code session in its panes. Copilot
/// exports no per-session variable this list can use, so its markers are
/// still compared by value.
const PER_SESSION_IDS: &[(&str, &str)] = &[("claude", "CLAUDE_CODE_SESSION_ID")];

/// Whether the tmux server's global environment holds what `marker` rests on
/// in this process (`env`), so the marker may be the server's copy of its
/// starter's rather than this session's own.
///
/// When this process holds its CLI's per-session ID ([`PER_SESSION_IDS`]),
/// the ID is compared instead of the marker:
///
/// - the server holds the same ID: this environment is the server's copy, as
///   on the far side of a detached launch without the hand-off;
/// - the server holds a different ID, or none: a session other than the
///   server's starter set it, and the marker is that session's own. That is a
///   Claude Code session running in a pane of a server some other Claude Code
///   session started, which `USER_PREFERENCES.md` has agents do.
///
/// Without an ID, the marker's own value is compared.
fn tmux_server_holds_marker(
    tmux: &OsStr,
    marker: SessionMarker,
    env: impl Fn(&str) -> Option<OsString>,
) -> bool {
    let set = |variable: &'static str| {
        env(variable)
            .filter(|value| !value.is_empty())
            .map(|value| (variable, value))
    };
    let session_id = PER_SESSION_IDS
        .iter()
        .find(|(binary, _)| *binary == marker.binary)
        .and_then(|&(_, id)| set(id));
    session_id
        .or_else(|| set(marker.variable))
        .is_some_and(|(variable, value)| tmux_global_environment_holds(tmux, variable, &value))
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
/// observed rather than inferred, no session marker contradicts it, and no
/// rejected `AMPLIHACK_AGENT_BINARY` lost to it.
///
/// `from_tmux_server` is [`marker_may_be_the_tmux_servers`]: the session
/// marker is one the tmux server's global environment also holds. A marker
/// that answered is then announced, because it may be the server's starter
/// rather than the caller -- a silent copilot run from a Claude Code session
/// without the hand-off. An explicit value that overrode it is not told to
/// step aside for it. A value the hand-off tagged as read from such a marker
/// ([`EnvBinaryValue::FromTmuxServerMarker`]) is announced the same way, since
/// the hand-off removed the marker this run would otherwise have seen.
///
/// The reason must match what the user did. A tagged inherited guess is
/// skipped by the resolver, and setting the same value again does not help
/// while the tag still names it. A rejected value was set, just not to
/// anything usable; the resolver's own warning about it is hidden at the
/// default tracing filter. It is named whichever layer answered instead: a
/// session marker that answered in its place is still a CLI the user did not
/// ask for. The rejected value itself is never echoed.
///
/// Issue #1525: a launcher context is named by the path the resolver actually
/// read, which the walk-up may have found in an ancestor. Every context file
/// it passed over because it could not be used gets a line of its own, with
/// the reason. Otherwise an empty or malformed file goes unmentioned, and the
/// user is left to guess why the default answered.
pub(crate) fn agent_binary_notice(
    reporter: Reporter,
    resolution: &Resolution,
    env_value: EnvBinaryValue,
    from_tmux_server: bool,
) -> Option<String> {
    let lead = reporter.lead();
    let read_from = || {
        let file = resolution
            .context_file
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "a launcher_context.json".to_string());
        format!("read from {file}")
    };
    let why = match (resolution.source, env_value) {
        (ResolutionSource::Env, EnvBinaryValue::FromTmuxServerMarker(marker)) => {
            return Some(handed_on_tmux_server_marker_notice(lead, marker));
        }
        (ResolutionSource::Env, _) => {
            return session_override_notice(lead, resolution, from_tmux_server);
        }
        (ResolutionSource::SessionMarker, env_value) => {
            let marker = resolution.session_marker?;
            return if from_tmux_server {
                Some(tmux_marker_notice(reporter, marker, env_value))
            } else if env_value == EnvBinaryValue::Rejected {
                Some(rejected_under_marker_notice(lead, marker))
            } else {
                None
            };
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
        EnvBinaryValue::Rejected => format!("Set {SET_TO_AN_ALLOWED_NAME} to choose an agent CLI."),
        EnvBinaryValue::Unset
        | EnvBinaryValue::Usable
        | EnvBinaryValue::FromTmuxServerMarker(_) => {
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

/// The line for a rejected `AMPLIHACK_AGENT_BINARY` that a session marker
/// answered in place of, when the marker is not one the tmux server holds.
///
/// A valid value that overrides a marker gets a line, and so does a rejected
/// one that falls to the default. Before this, a rejected one that lost to a
/// marker got nothing: the user named a CLI, got the marker's, and nothing
/// said why (#1335; crusty review of #1490 at 7053698c).
///
/// It names the variable and the marker, never the value. Launcher contexts
/// the walk-up skipped are not listed; they did not decide this. A nested run
/// does not repeat it: `recipe run` hands its steps the marker's answer as a
/// valid value.
fn rejected_under_marker_notice(lead: &str, marker: SessionMarker) -> String {
    let SessionMarker { variable, binary } = marker;
    format!(
        "amplihack: {lead} '{binary}' ({REJECTED}; {variable}, the {binary} session marker \
         in this environment, answered). Set {SET_TO_AN_ALLOWED_NAME} to choose an agent CLI."
    )
}

/// The line for a session marker that answered but that the tmux server's
/// global environment holds as well.
///
/// Without the hand-off a detached `recipe run` sees the server's environment,
/// and a server started from a Copilot session gives it `COPILOT_CLI=1` however
/// it was launched. Before this line, that was a silent copilot run from a
/// Claude Code session: a marker counts as observed, and observed answers are
/// not announced.
///
/// `recipe run` is told about the hand-off, which is how the next launch
/// avoids this. `agent-binary` is not: it is the hand-off, and in the same
/// environment it would carry across this same answer (crusty round 2 of
/// #1490). It is told only how to choose.
///
/// A rejected `AMPLIHACK_AGENT_BINARY` is named first, as in
/// [`rejected_under_marker_notice`], and the advice lists the names it
/// accepts. The rest of the wording stays this line's own, because only this
/// line has to say the marker may be the server's starter's rather than set
/// by a session at all.
fn tmux_marker_notice(
    reporter: Reporter,
    marker: SessionMarker,
    env_value: EnvBinaryValue,
) -> String {
    let SessionMarker { variable, binary } = marker;
    let lead = reporter.lead();
    let (rejected, choose) = if env_value == EnvBinaryValue::Rejected {
        (format!("{REJECTED}; "), SET_TO_AN_ALLOWED_NAME)
    } else {
        (String::new(), BINARY_ENV)
    };
    let how = match reporter {
        Reporter::RecipeRun => format!(
            "To hand a detached run the CLI you launch it from, prefix its command with \
             $(amplihack agent-binary --shell -w <dir>), inside double quotes; to choose one, \
             set {choose}."
        ),
        Reporter::AgentBinary => format!("Set {choose} to choose an agent CLI."),
    };
    format!(
        "amplihack: {lead} '{binary}' ({rejected}{variable} is set, but this tmux server's \
         global environment holds the same value, so it may come from whatever started the \
         server rather than from a {binary} session). {how}"
    )
}

/// The line for a value `amplihack agent-binary --shell` handed on tagged as
/// read from `marker` while the tmux server's global environment held it too.
///
/// The hand-off removes every marker on the far side, so without the tag this
/// run would see only an explicit choice and print nothing. The usual way to
/// get here is the hand-off in a single-quoted tmux command: `$(...)` then
/// runs in the new session, reads the server's markers, and the only line
/// naming one lands in the tmux pane, not in this run's log (crusty review of
/// #1490 at ef441d81). This line puts it back in the log, with the fix.
///
/// The value is still honoured: it is the answer the hand-off resolved, and
/// what this run would have resolved itself with no hand-off at all. The
/// variable comes from `SESSION_MARKERS` and the binary from the allowlist,
/// so echoing them is safe.
fn handed_on_tmux_server_marker_notice(lead: &str, marker: SessionMarker) -> String {
    let SessionMarker { variable, binary } = marker;
    format!(
        "amplihack: {lead} '{binary}' ({BINARY_ENV} was handed on by amplihack agent-binary \
         --shell, which read it from {variable} while the tmux server's global environment \
         held the same value, so it may come from whatever started that server rather than \
         from a {binary} session). If that hand-off was in a single-quoted tmux command, it \
         ran in the new session instead of in your shell: put the command in double quotes. \
         To choose an agent CLI, set {BINARY_ENV}."
    )
}

#[cfg(test)]
mod tests {
    use super::{
        EnvBinaryValue, Reporter, agent_binary_notice, answer_from_tmux_server_marker,
        tmux_global_environment_holds, tmux_server_holds_marker,
    };
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

    const LEAD: Reporter = Reporter::RecipeRun;

    /// Every value but `FromTmuxServerMarker`, which carries a marker and has
    /// tests of its own below.
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

    /// Silent unless a rejected value lost to the marker, which has a test of
    /// its own below; this one only checks that it is the sole exception.
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
                    let notice = agent_binary_notice(LEAD, &resolution, env_value, false);
                    let rejected_value_lost_to_the_marker = source
                        == ResolutionSource::SessionMarker
                        && session_marker.is_some()
                        && env_value == EnvBinaryValue::Rejected;
                    assert_eq!(
                        notice.is_some(),
                        rejected_value_lost_to_the_marker,
                        "{source:?} {session_marker:?} {env_value:?}: {notice:?}"
                    );
                }
            }
        }
    }

    /// Crusty review of #1490 at 7053698c: `COPILOT_CLI=1
    /// AMPLIHACK_AGENT_BINARY=claude-code` resolved to copilot from the
    /// marker and printed nothing, while a valid value over a marker and a
    /// rejected value that fell to the default each got a line. The user named
    /// a CLI and got another one (#1335). The line names the variable and the
    /// marker, lists the allowed names, and never the skipped file.
    #[test]
    fn a_rejected_value_a_session_marker_answered_for_is_named() {
        let mut resolution = resolved("copilot", ResolutionSource::SessionMarker);
        resolution.session_marker = Some(COPILOT_CLI);
        resolution.unusable_contexts = vec![empty_context_at("/r/x.json")];
        for (reporter, lead) in [
            (Reporter::RecipeRun, "agent steps will run under"),
            (Reporter::AgentBinary, "resolved the agent binary to"),
        ] {
            let notice =
                agent_binary_notice(reporter, &resolution, EnvBinaryValue::Rejected, false)
                    .unwrap();
            assert_eq!(
                notice,
                format!(
                    "amplihack: {lead} 'copilot' (AMPLIHACK_AGENT_BINARY is set but is not one \
                     of amplifier, claude, codex or copilot; COPILOT_CLI, the copilot session \
                     marker in this environment, answered). Set AMPLIHACK_AGENT_BINARY to one \
                     of amplifier, claude, codex or copilot to choose an agent CLI."
                )
            );
            // Not the tmux line: nothing says this marker is the server's.
            assert!(!notice.contains("tmux"), "{notice}");
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
            if env_value == EnvBinaryValue::Rejected {
                // The same line, with the rejected variable named first and
                // the allowed names in the advice.
                assert_eq!(
                    notice,
                    "amplihack: agent steps will run under 'copilot' (AMPLIHACK_AGENT_BINARY is \
                     set but is not one of amplifier, claude, codex or copilot; COPILOT_CLI is \
                     set, but this tmux server's global environment holds the same value, so it \
                     may come from whatever started the server rather than from a copilot \
                     session). To hand a detached run the CLI you launch it from, prefix its \
                     command with $(amplihack agent-binary --shell -w <dir>), inside double \
                     quotes; to choose one, set AMPLIHACK_AGENT_BINARY to one of amplifier, \
                     claude, codex or copilot."
                );
            } else {
                assert_eq!(
                    notice,
                    "amplihack: agent steps will run under 'copilot' (COPILOT_CLI is set, but \
                     this tmux server's global environment holds the same value, so it may come \
                     from whatever started the server rather than from a copilot session). To \
                     hand a detached run the CLI you launch it from, prefix its command with \
                     $(amplihack agent-binary --shell -w <dir>), inside double quotes; to choose \
                     one, set AMPLIHACK_AGENT_BINARY."
                );
            }
        }
    }

    /// Crusty review of #1490 at ef441d81: a hand-off in single quotes runs in
    /// the new session, reads the server's `COPILOT_CLI`, and hands it on with
    /// every marker removed. The run receiving it sees no marker at all, only
    /// the value and the tag, and still names the marker -- in its own log.
    #[test]
    fn a_value_handed_on_from_a_marker_the_tmux_server_holds_is_announced() {
        let resolution = resolved("copilot", ResolutionSource::Env);
        let env_value = EnvBinaryValue::FromTmuxServerMarker(COPILOT_CLI);
        let because = "(AMPLIHACK_AGENT_BINARY was handed on by amplihack agent-binary --shell, \
                       which read it from COPILOT_CLI while the tmux server's global environment \
                       held the same value, so it may come from whatever started that server \
                       rather than from a copilot session). If that hand-off was in a \
                       single-quoted tmux command, it ran in the new session instead of in your \
                       shell: put the command in double quotes. To choose an agent CLI, set \
                       AMPLIHACK_AGENT_BINARY.";
        for (reporter, lead) in [
            (Reporter::RecipeRun, "agent steps will run under"),
            (Reporter::AgentBinary, "resolved the agent binary to"),
        ] {
            // Whether this run's own tmux server holds anything does not
            // matter: the evidence came with the tag.
            for from_tmux_server in [false, true] {
                assert_eq!(
                    agent_binary_notice(reporter, &resolution, env_value, from_tmux_server),
                    Some(format!("amplihack: {lead} 'copilot' {because}")),
                    "{reporter:?} {from_tmux_server}"
                );
            }
        }
    }

    /// What `agent-binary --shell` tags: only an answer that rests on a marker
    /// the tmux server holds, read here or received tagged. An explicit value
    /// that overrode such a marker rests on the explicit value.
    #[test]
    fn only_an_answer_resting_on_a_server_held_marker_is_tagged_as_one() {
        let mut from_marker = resolved("copilot", ResolutionSource::SessionMarker);
        from_marker.session_marker = Some(COPILOT_CLI);
        let unset = EnvBinaryValue::Unset;
        assert_eq!(
            answer_from_tmux_server_marker(&from_marker, unset, true),
            Some(COPILOT_CLI)
        );
        assert_eq!(
            answer_from_tmux_server_marker(&from_marker, unset, false),
            None
        );

        let received = EnvBinaryValue::FromTmuxServerMarker(COPILOT_CLI);
        let explicit = resolved("copilot", ResolutionSource::Env);
        assert_eq!(
            answer_from_tmux_server_marker(&explicit, received, false),
            Some(COPILOT_CLI)
        );

        let mut overriding = resolved("claude", ResolutionSource::Env);
        overriding.session_marker = Some(COPILOT_CLI);
        assert_eq!(
            answer_from_tmux_server_marker(&overriding, EnvBinaryValue::Usable, true),
            None
        );
        for source in [ResolutionSource::LauncherContext, ResolutionSource::Default] {
            let mut other = resolved("copilot", source);
            other.session_marker = Some(COPILOT_CLI);
            assert_eq!(
                answer_from_tmux_server_marker(&other, unset, true),
                None,
                "{source:?}"
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

    /// Crusty round 2 of #1490: Claude Code sets `CLAUDECODE=1` in every
    /// session, so a Claude Code session in a pane of a server another one
    /// started was told its marker "may come from whatever started the
    /// server". The session IDs differ, and that decides it.
    #[cfg(unix)]
    #[test]
    fn a_claude_session_is_told_from_the_servers_starter_by_its_session_id() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let server = |name: &str, global_env: &str| {
            let path = dir.path().join(name);
            let body = format!(
                "#!/bin/sh\n[ \"$1 $2\" = \"show-environment -g\" ] || exit 2\n\
                 case \"$3\" in\n{global_env}\
                 *) echo \"unknown variable: $3\" >&2; exit 1 ;;\nesac\n"
            );
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        // Started from a Claude Code session, as crusty's reproduction was.
        let from_claude = server(
            "from-claude",
            "CLAUDECODE) echo CLAUDECODE=1 ;;\n\
             CLAUDE_CODE_SESSION_ID) echo CLAUDE_CODE_SESSION_ID=starter-aaaa ;;\n",
        );
        // Started from a Claude Code that exported no session ID.
        let from_claude_without_id = server(
            "from-claude-without-id",
            "CLAUDECODE) echo CLAUDECODE=1 ;;\n",
        );
        let from_copilot = server("from-copilot", "COPILOT_CLI) echo COPILOT_CLI=1 ;;\n");

        let holds = |tmux: &std::path::Path, marker, env: &[(&str, &str)]| {
            tmux_server_holds_marker(tmux.as_os_str(), marker, |name| {
                env.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.into())
            })
        };
        let caller = [
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "caller-bbbb"),
        ];
        let starters_copy = [
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "starter-aaaa"),
        ];

        // Crusty's reproduction: a different session ID is this session's own.
        assert!(!holds(&from_claude, CLAUDECODE, &caller));
        assert!(!holds(&from_claude_without_id, CLAUDECODE, &caller));
        // The same session ID is the server's copy of its starter.
        assert!(holds(&from_claude, CLAUDECODE, &starters_copy));
        // No ID to compare, or an empty one: the marker's value is all there is.
        assert!(holds(&from_claude, CLAUDECODE, &[("CLAUDECODE", "1")]));
        assert!(holds(
            &from_claude,
            CLAUDECODE,
            &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "")]
        ));
        assert!(holds(
            &from_claude_without_id,
            CLAUDECODE,
            &[("CLAUDECODE", "1")]
        ));
        // Copilot has no session ID here, so its marker is still compared by
        // value, and Claude's ID never speaks for it.
        assert!(holds(&from_copilot, COPILOT_CLI, &[("COPILOT_CLI", "1")]));
        assert!(holds(
            &from_copilot,
            COPILOT_CLI,
            &[
                ("COPILOT_CLI", "1"),
                ("CLAUDE_CODE_SESSION_ID", "caller-bbbb")
            ]
        ));
        assert!(!holds(&from_claude, COPILOT_CLI, &[("COPILOT_CLI", "1")]));
    }

    /// Crusty round 2 of #1490: `agent-binary` was told to prefix a command
    /// with `$(amplihack agent-binary --shell ...)`, i.e. to run itself. In the
    /// same environment that hand-off carries this same answer. It is told only
    /// how to choose.
    #[test]
    fn agent_binary_is_not_told_to_hand_off_through_itself() {
        let mut resolution = resolved("claude", ResolutionSource::SessionMarker);
        resolution.session_marker = Some(CLAUDECODE);
        for env_value in ALL {
            let notice =
                agent_binary_notice(Reporter::AgentBinary, &resolution, env_value, true).unwrap();
            if env_value == EnvBinaryValue::Rejected {
                assert_eq!(
                    notice,
                    "amplihack: resolved the agent binary to 'claude' (AMPLIHACK_AGENT_BINARY \
                     is set but is not one of amplifier, claude, codex or copilot; CLAUDECODE is \
                     set, but this tmux server's global environment holds the same value, so it \
                     may come from whatever started the server rather than from a claude \
                     session). Set AMPLIHACK_AGENT_BINARY to one of amplifier, claude, codex or \
                     copilot to choose an agent CLI."
                );
            } else {
                assert_eq!(
                    notice,
                    "amplihack: resolved the agent binary to 'claude' (CLAUDECODE is set, but \
                     this tmux server's global environment holds the same value, so it may come \
                     from whatever started the server rather than from a claude session). Set \
                     AMPLIHACK_AGENT_BINARY to choose an agent CLI."
                );
            }
            assert!(!notice.contains("agent-binary --shell"), "{notice}");
        }
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
