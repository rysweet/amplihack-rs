//! The probe method and deadline a rotate cycle measures with, as `profile
//! usage` resolves them (moved from `measure.rs` to keep it in budget).

use crate::commands::profile::probe_limits::{self, ProbeMode};
use std::time::Duration;

/// The probe method, as `profile usage` resolves it. A bad variable falls
/// back to the default order and warns on stderr. Only `measure_ordered`
/// calls this, once per cycle, so the warning is printed once per cycle;
/// anything else in the cycle uses [`mode_quiet`].
pub(in crate::commands::profile::rotate) fn mode() -> ProbeMode {
    let (mode, warning) = probe_limits::probe_mode_from_env();
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    mode
}

/// [`mode`] with no warning: for a read later in a cycle whose
/// `measure_ordered` has already warned about the same variable (the
/// freshness gate's re-read, issue #1870 review round 4).
pub(in crate::commands::profile::rotate) fn mode_quiet() -> ProbeMode {
    probe_limits::probe_mode_from_env().0
}

/// The probe deadline, as `profile usage` resolves it without a flag. A bad
/// variable falls back to the default and warns on stderr, as a bad
/// parallelism variable does. Only `measure_ordered` calls this, once per
/// cycle, so the warning is printed once per cycle; anything else in the
/// cycle uses [`timeout_quiet`].
pub(in crate::commands::profile::rotate) fn timeout() -> Duration {
    let (secs, warning) = probe_limits::timeout_from_env(None);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    Duration::from_secs(secs)
}

/// [`timeout`] with no warning: for the freshness gate's time limit, set
/// after `measure_ordered` has already warned this cycle.
pub(in crate::commands::profile::rotate) fn timeout_quiet() -> Duration {
    Duration::from_secs(probe_limits::timeout_from_env(None).0)
}
