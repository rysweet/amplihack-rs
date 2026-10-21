//! A read-only view of the Claude login rotator (`amplihack profile rotate`)
//! for the limit-resume hook (issue #1658), which cannot depend on the CLI.
//!
//! - [`resolve_root`]: where the profile root is (`root.rs`).
//! - [`auto_switch_on`]: whether auto-switch is on (`gate.rs`).
//! - [`read_cycle`]: the rotate ledger's last cycle (`ledger.rs`).
//!
//! The CLI owns the same rules (`root_config`, `auto_switch::config`,
//! `rotate::ledger`); its `tests_status_drift.rs` files check that both
//! sides agree. Nothing here creates, changes or locks a file.

mod gate;
mod ledger;
mod root;

pub use gate::{TOGGLE_ENV, TOGGLE_FILE, auto_switch_on, auto_switch_on_from, valid_live_name};
pub use ledger::{CycleStatus, ledger_path, parse_cycle, read_cycle};
pub use root::{ROOT_ENV, resolve_root, resolve_root_from};

use std::time::Duration;

/// How long the hook waits for a fresh rotator cycle with capacity.
pub const MAX_WAIT: Duration = Duration::from_secs(20 * 60);

/// How often the hook reads the ledger while it waits.
pub const POLL_INTERVAL: Duration = Duration::from_secs(15);

/// A cycle counts as fresh only when it was recorded more than this many
/// seconds after the hook started, so its usage probe (30 s by default)
/// began after the limit was hit.
pub const FRESH_MARGIN_SECS: i64 = 60;

/// The `timeout` registered for the `StopFailure` hook. It must exceed
/// `MAX_WAIT + POLL_INTERVAL`, so the hook always decides before Claude Code
/// stops it. Here rather than in the hooks crate because the CLI installs
/// the hook and does not depend on that crate.
pub const LIMIT_RESUME_TIMEOUT_SECS: u64 = 1800;

const _: () = assert!(MAX_WAIT.as_secs() + POLL_INTERVAL.as_secs() < LIMIT_RESUME_TIMEOUT_SECS);
