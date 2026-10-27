//! A read-only view of the Claude login rotator (`amplihack profile rotate`)
//! and the auto-switch settings, shared by the CLI and the hooks (which
//! cannot depend on the CLI).
//!
//! - [`resolve_root`], [`locate_root`]: where the profile root is (`root.rs`).
//! - [`auto_switch_on`], [`parse_toggle`]: the auto-switch toggle (`gate.rs`).
//! - [`read_cycle`]: the rotate ledger's last cycle (`ledger.rs`).
//! - [`rotator_down`], [`daemon_state`]: whether the rotator runs (`daemon.rs`).
//!
//! Nothing here creates, changes or locks a file.

#[cfg(unix)]
mod daemon;
mod gate;
mod ledger;
mod root;

#[cfg(unix)]
pub use daemon::{
    DaemonState, daemon_state, foreign_rotator, is_rotator, parse_pid, pid_path, rotator_down,
    rotator_down_at,
};
pub use gate::{
    DEFAULT_THRESHOLD, EnvToggle, MAX_TOGGLE_BYTES, TOGGLE_ENV, TOGGLE_FILE, Toggle, ToggleFile,
    auto_switch_on, auto_switch_on_from, env_toggle, parse_threshold, parse_toggle, read_toggle,
    valid_live_name,
};
pub use ledger::{CycleStatus, Ledger, ledger_path, parse_cycle, read_cycle, read_ledger};
pub use root::{
    PROFILE_ROOT_FILE, ROOT_ENV, RootError, RootSource, check_root_value, config_dir,
    home_from_env, locate_root, read_saved_root, resolve_root, resolve_root_from,
};

use std::time::Duration;

/// How long the hook waits for a fresh rotator cycle with capacity: one
/// week, the longest any usage limit lasts, so a session stopped while every
/// login is spent resumes when the first one resets (issue #1659).
pub const MAX_WAIT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How often the hook reads the ledger during the first `SLOW_POLL_AFTER`.
pub const POLL_INTERVAL: Duration = Duration::from_secs(15);

/// After this long the hook reads the ledger every `SLOW_POLL_INTERVAL`.
pub const SLOW_POLL_AFTER: Duration = Duration::from_secs(60 * 60);

/// How often the hook reads the ledger once a wait has gone on for an hour:
/// a week of waiting is about 2,000 small reads.
pub const SLOW_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// For a ledger without `last_cycle.measured_at` (a rotator older than
/// #1659), a cycle counts as fresh only when it was recorded more than this
/// many seconds after the hook started, so its usage probe (30 s by
/// default) began after the limit was hit.
pub const FRESH_MARGIN_SECS: i64 = 60;

/// The `timeout` registered for the `StopFailure` hook. It must exceed
/// `MAX_WAIT + SLOW_POLL_INTERVAL`, so the hook always decides before Claude Code
/// stops it. Here rather than in the hooks crate because the CLI installs
/// the hook and does not depend on that crate.
pub const LIMIT_RESUME_TIMEOUT_SECS: u64 = 608_400;

const _: () =
    assert!(MAX_WAIT.as_secs() + SLOW_POLL_INTERVAL.as_secs() < LIMIT_RESUME_TIMEOUT_SECS);
