//! R1 — Signal onboarding prompt gating + a persisted "declined" sentinel.
//!
//! When a host has no Signal config yet, amplihack should ask **once** whether
//! to link this device and mirror the session to a private Signal group. That
//! prompt must never break or block a session, so the decision to show it is
//! factored into the pure [`should_prompt`] predicate over an explicit
//! [`OnboardingEnv`]. We only prompt when **all** of these hold:
//!
//! * Signal is not already configured (`!config_present`),
//! * we have an interactive controlling terminal (`is_tty`),
//! * we are not in non-interactive/automation mode (`!noninteractive`, i.e.
//!   `AMPLIHACK_NONINTERACTIVE` is unset),
//! * the user has not previously declined (`!declined_before`).
//!
//! A decline is durable: [`mark_onboarding_declined`] writes a sentinel file
//! under an explicit `root` so subsequent launches ([`onboarding_declined`])
//! skip the prompt forever. Keeping `root` explicit makes the sentinel
//! round-trip hermetically testable with no `HOME`/cwd mutation.

use std::io;
use std::path::{Path, PathBuf};

/// The observable inputs that gate whether we surface the onboarding prompt.
#[derive(Debug, Clone)]
pub struct OnboardingEnv {
    /// Whether a usable Signal configuration is already present.
    pub config_present: bool,
    /// Whether stdin/stdout is an interactive terminal.
    pub is_tty: bool,
    /// Whether automation mode is active (`AMPLIHACK_NONINTERACTIVE=1`).
    pub noninteractive: bool,
    /// Whether the user previously declined (sentinel already written).
    pub declined_before: bool,
}

/// The onboarding gate decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingDecision {
    /// Surface the (non-blocking) onboarding prompt.
    Prompt,
    /// Do not prompt.
    Skip,
}

/// Pure gate: prompt only when unconfigured **and** interactive **and** not in
/// automation **and** not previously declined; otherwise skip.
#[must_use]
pub fn should_prompt(env: &OnboardingEnv) -> OnboardingDecision {
    if !env.config_present && env.is_tty && !env.noninteractive && !env.declined_before {
        OnboardingDecision::Prompt
    } else {
        OnboardingDecision::Skip
    }
}

/// Path of the "declined" sentinel under `root`.
#[must_use]
pub fn onboarding_declined_path(root: &Path) -> PathBuf {
    root.join("signal-onboarding-declined")
}

/// Whether the user has previously declined onboarding under `root`.
#[must_use]
pub fn onboarding_declined(root: &Path) -> bool {
    onboarding_declined_path(root).exists()
}

/// Record a durable decline under `root`. Idempotent: repeated calls succeed.
pub fn mark_onboarding_declined(root: &Path) -> io::Result<()> {
    let path = onboarding_declined_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, b"declined\n")
}
