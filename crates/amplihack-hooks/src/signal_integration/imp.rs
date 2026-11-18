//! Concrete Signal integration (compiled only under the `signal` feature).
//!
//! This module carries **only** the session-start onboarding notice. It performs
//! no Signal group I/O: no group creation, no message posting, no inbox draining,
//! and no background subscriber. Signal groups are created solely by the explicit
//! `amplihack signal chat <topic>` command (in the `amplihack-signal` crate and
//! the `amplihack signal chat` CLI command), which this module does not touch.

use std::path::PathBuf;

use amplihack_signal::config::SignalConfig;

/// Root directory holding per-host Signal onboarding sentinels.
///
/// This MUST be independent of the current working directory: the SessionStart
/// hook can run with different cwds across hosts (e.g. Copilot CLI invokes hooks
/// from its plugin directory while the agent's cwd is the project root). Anchor
/// it at the stable `~/.amplihack/runtime/signal` base (the same `~/.amplihack`
/// home used for `signal-config.toml`) so all participants agree regardless of
/// cwd.
fn signal_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("AMPLIHACK_SIGNAL_STATE_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".amplihack").join("runtime").join("signal")
}

/// Load the Signal config, treating an unloadable/absent config as "the channel
/// is simply not configured" rather than an operational failure. Returns `None`
/// to mean "Signal is not configured".
fn load_config_or_disabled() -> Option<SignalConfig> {
    match SignalConfig::load() {
        Ok(c) => Some(c),
        Err(err) => {
            tracing::debug!("signal channel not configured (config not loaded): {err}");
            None
        }
    }
}

/// Normalize a session id, treating a missing or blank id as "no session".
fn normalize_session_id(session_id: Option<&str>) -> Option<&str> {
    session_id.filter(|s| !s.trim().is_empty())
}

// ---------------------------------------------------------------------------
// SessionStart
// ---------------------------------------------------------------------------

/// Session-start Signal hook. This **never** creates a Signal group, posts a
/// message, persists any per-session state, or spawns a background process.
/// Automatic per-session group creation was removed because it flooded the
/// operator's Signal with thousands of empty groups (one per top-level
/// session/recipe launch); groups are now created only by the explicit
/// `amplihack signal chat <topic>` command.
///
/// The only remaining behavior is a one-time, purely-local onboarding notice on
/// an interactive host when Signal is not yet configured (no network I/O, no
/// group). All failures are non-fatal.
pub fn on_session_start(session_id: Option<&str>, warnings: &mut Vec<String>) {
    let Some(session_id) = normalize_session_id(session_id) else {
        return;
    };
    if let Err(err) = start(session_id) {
        let msg = format!("signal: session-start integration failed: {err}");
        tracing::warn!("{msg}");
        warnings.push(msg);
    }
}

fn start(_session_id: &str) -> anyhow::Result<()> {
    // Session start performs NO Signal group I/O. When Signal is unconfigured on
    // an interactive host, surface the one-time, purely-local onboarding notice.
    // A configured channel does nothing here: groups are created only by the
    // explicit `amplihack signal chat` command, never on session start.
    if load_config_or_disabled().is_none() {
        maybe_prompt_onboarding();
    }
    Ok(())
}

/// One-time, **non-blocking** onboarding notice shown when Signal is not yet
/// configured on an interactive host. Hooks cannot run an interactive prompt
/// (stdout is parsed as JSON, and the ~30s budget forbids the QR/device-link
/// flow), so this surfaces guidance on stderr at most once per host and records
/// a "notified" sentinel so it never nags on subsequent turns/sessions. The
/// decision is gated by the pure [`super::onboarding::should_prompt`].
fn maybe_prompt_onboarding() {
    use super::onboarding::{OnboardingDecision, OnboardingEnv, should_prompt};

    let root = signal_root();
    let env = OnboardingEnv {
        config_present: false, // reached only from the unconfigured branch
        is_tty: is_stderr_tty(),
        noninteractive: std::env::var_os("AMPLIHACK_NONINTERACTIVE").is_some(),
        declined_before: super::onboarding::onboarding_declined(&root),
    };
    if should_prompt(&env) != OnboardingDecision::Prompt {
        return;
    }

    // Show at most once per host (independent of the "declined" sentinel).
    let notified = root.join("signal-onboarding-notified");
    if notified.exists() {
        return;
    }
    if let Err(error) = crate::state_dir::create_private_dir_all(&root)
        .and_then(|()| amplihack_utils::secure_files::write_private_file(&notified, b"1\n"))
    {
        tracing::warn!("could not record Signal onboarding notice: {error}");
    }

    eprintln!(
        "\n[amplihack] Signal session mirroring is available but not configured on \
         this host.\n  Link signal-cli on this device to mirror your whole session \
         to a private Signal\n  group and send replies back from your phone. See \
         docs/SIGNAL_ONBOARDING.md to enable,\n  or run onboarding to decline \
         permanently (suppresses this notice).\n"
    );
}

/// Whether stderr is an interactive terminal.
#[cfg(unix)]
fn is_stderr_tty() -> bool {
    // SAFETY: `isatty` on a valid fd has no memory-safety implications.
    unsafe { libc::isatty(libc::STDERR_FILENO) == 1 }
}

/// Non-Unix fallback: treat stderr as non-interactive and suppress the
/// interactive onboarding notice rather than depending on the Unix-only
/// `isatty`.
#[cfg(not(unix))]
fn is_stderr_tty() -> bool {
    false
}
