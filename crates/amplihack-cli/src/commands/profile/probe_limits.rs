//! The probe method, the per-probe deadline and the probe pool size (issue
//! #1698), shared by `profile usage` and the rotator. The resolvers are pure;
//! the three `*_from_*` wrappers are the only code that reads the environment
//! or `/proc/loadavg`. Nothing here prints: each resolver returns the warning
//! for the caller.

use std::ffi::OsStr;
use std::io::Read;

pub(super) const PROBE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PROBE";
pub(super) const TIMEOUT_ENV: &str = "AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS";
pub(super) const PARALLEL_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PARALLEL";
/// Issue #1844: how old a passive reading may be and still be used.
pub(super) const PASSIVE_MAX_AGE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS";
pub(super) const DEFAULT_PASSIVE_MAX_AGE_SECS: u64 = 300;
pub(super) const MAX_PASSIVE_MAX_AGE_SECS: u64 = 3600;
/// Issue #1844: direct reads run at once. `PARALLEL_ENV` bounds only the
/// `claude` fallbacks.
pub(super) const DIRECT_PARALLEL_ENV: &str = "AMPLIHACK_PROFILE_USAGE_DIRECT_PARALLEL";
pub(super) const DEFAULT_DIRECT_PARALLEL: usize = 8;
/// One probe took 71.5 s at load 194 on a 72-CPU host (issue #1698); 120 s
/// leaves about 1.7 times that.
pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub(super) const MAX_TIMEOUT_SECS: u64 = 600;
/// The largest value the parallelism variable accepts.
pub(super) const MAX_PARALLEL: usize = 16;
/// The derived bound never goes below this: at load 194 one probe at a time
/// would make a 14-profile cycle take about 16 minutes, and a second probe
/// adds little to a host already that busy.
const MIN_DERIVED: usize = 2;
/// The derived bound never goes above this: each probe scans every session
/// log, so more at once only competes for the same disk and CPU.
const MAX_DERIVED: usize = 4;
/// Free CPUs per probe in the derived bound.
const CPUS_PER_PROBE: f64 = 8.0;
/// `/proc/loadavg` is one short line; nothing past this is read.
const LOADAVG_CAP: u64 = 256;
/// Characters of a bad value shown in a warning.
const SHOWN: usize = 32;

/// The value of a bad variable as a warning shows it: escaped and cut.
fn shown(text: &str) -> String {
    let cut: String = text.chars().take(SHOWN).collect();
    format!("{cut:?}")
}

/// How a probe reads the usage figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeMode {
    /// Unset (issue #1844): a fresh passive reading, then the direct read,
    /// then the `claude` fallback.
    Ordered,
    /// A fresh passive reading, then the direct read only; `claude` is never
    /// started for a reading.
    Direct,
    /// The `get_usage` request, then `claude -p /usage` when it gives no
    /// accepted answer.
    Auto,
    /// `claude -p /usage` only.
    Text,
}

/// The probe method: `direct`, `auto` or `text`, matched exactly. Unset or
/// empty is the default order. Any other value gives the warning to print,
/// and the default order.
pub(super) fn probe_mode(env: Option<&OsStr>) -> (ProbeMode, Option<String>) {
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (ProbeMode::Ordered, None);
    };
    match raw.to_str() {
        Some("direct") => (ProbeMode::Direct, None),
        Some("auto") => (ProbeMode::Auto, None),
        Some("text") => (ProbeMode::Text, None),
        _ => {
            let warning = format!(
                "warning: ignored {PROBE_ENV} {} (use direct, auto or text); using the default order",
                shown(&raw.to_string_lossy())
            );
            (ProbeMode::Ordered, Some(warning))
        }
    }
}

/// The passive-reading age limit in seconds: the variable (0 to 3600), then
/// 300. `0` turns passive readings off. An empty variable counts as unset.
pub(super) fn passive_max_age(env: Option<&OsStr>) -> (u64, Option<String>) {
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (DEFAULT_PASSIVE_MAX_AGE_SECS, None);
    };
    match raw.to_str().and_then(|t| t.parse::<u64>().ok()) {
        Some(secs) if secs <= MAX_PASSIVE_MAX_AGE_SECS => (secs, None),
        _ => {
            let warning = format!(
                "warning: ignored {PASSIVE_MAX_AGE_ENV} {} (use 0 to {MAX_PASSIVE_MAX_AGE_SECS}); using {DEFAULT_PASSIVE_MAX_AGE_SECS}",
                shown(&raw.to_string_lossy())
            );
            (DEFAULT_PASSIVE_MAX_AGE_SECS, Some(warning))
        }
    }
}

/// Direct reads run at once for `n` profiles: the variable (1 to 16), then
/// 8, capped at `n` and at least 1.
pub(super) fn direct_parallel(env: Option<&OsStr>, n: usize) -> (usize, Option<String>) {
    let cap = |v: usize| v.min(n).max(1);
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (cap(DEFAULT_DIRECT_PARALLEL), None);
    };
    match raw.to_str().and_then(|t| t.parse::<usize>().ok()) {
        Some(v) if (1..=MAX_PARALLEL).contains(&v) => (cap(v), None),
        _ => {
            let warning = format!(
                "warning: ignored {DIRECT_PARALLEL_ENV} {} (use 1 to {MAX_PARALLEL}); using {DEFAULT_DIRECT_PARALLEL}",
                shown(&raw.to_string_lossy())
            );
            (cap(DEFAULT_DIRECT_PARALLEL), Some(warning))
        }
    }
}

/// The probe deadline: the flag, then the variable (1 to 600), then 120. An
/// empty variable counts as unset. A bad variable gives the warning to print.
pub(super) fn timeout_secs(flag: Option<u64>, env: Option<&OsStr>) -> (u64, Option<String>) {
    if let Some(secs) = flag {
        return (secs, None);
    }
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (DEFAULT_TIMEOUT_SECS, None);
    };
    match raw.to_str().and_then(|t| t.parse::<u64>().ok()) {
        Some(secs) if (1..=MAX_TIMEOUT_SECS).contains(&secs) => (secs, None),
        _ => {
            let warning = format!(
                "warning: ignored {TIMEOUT_ENV} {} (use 1 to {MAX_TIMEOUT_SECS}); using {DEFAULT_TIMEOUT_SECS}",
                shown(&raw.to_string_lossy())
            );
            (DEFAULT_TIMEOUT_SECS, Some(warning))
        }
    }
}

/// Probes run at once for `n` profiles. Derived from the CPUs not already
/// busy, `clamp(floor((cpus - load1) / 8), 2, 4)`, with all CPUs free when
/// the load is unknown; a valid variable (1 to 16, empty counts as unset)
/// replaces it. The result is capped at `n` and is at least 1. A bad
/// variable gives a warning naming the derived bound before that cap.
pub(super) fn parallel(
    env: Option<&OsStr>,
    cpus: usize,
    load1: Option<f64>,
    n: usize,
) -> (usize, Option<String>) {
    let load = load1.filter(|l| l.is_finite() && *l >= 0.0).unwrap_or(0.0);
    let free = (cpus as f64 - load).max(0.0);
    // `as` saturates, so a huge host cannot overflow before the clamp.
    let derived = ((free / CPUS_PER_PROBE) as usize).clamp(MIN_DERIVED, MAX_DERIVED);
    let cap = |v: usize| v.min(n).max(1);
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (cap(derived), None);
    };
    match raw.to_str().and_then(|t| t.parse::<usize>().ok()) {
        Some(v) if (1..=MAX_PARALLEL).contains(&v) => (cap(v), None),
        _ => {
            let warning = format!(
                "warning: ignored {PARALLEL_ENV} {} (use 1 to {MAX_PARALLEL}); using {derived}",
                shown(&raw.to_string_lossy())
            );
            (cap(derived), Some(warning))
        }
    }
}

/// The one-minute load from the text of `/proc/loadavg`; `None` when the
/// first field is missing, negative or not a finite number.
pub(super) fn parse_load1(text: &str) -> Option<f64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|l| l.is_finite() && *l >= 0.0)
}

/// [`probe_mode`] with the real environment.
pub(super) fn probe_mode_from_env() -> (ProbeMode, Option<String>) {
    probe_mode(std::env::var_os(PROBE_ENV).as_deref())
}

/// [`passive_max_age`] with the real environment.
pub(super) fn passive_max_age_from_env() -> (u64, Option<String>) {
    passive_max_age(std::env::var_os(PASSIVE_MAX_AGE_ENV).as_deref())
}

/// [`direct_parallel`] with the real environment.
pub(super) fn direct_parallel_from_env(n: usize) -> (usize, Option<String>) {
    direct_parallel(std::env::var_os(DIRECT_PARALLEL_ENV).as_deref(), n)
}

/// [`timeout_secs`] with the real environment.
pub(super) fn timeout_from_env(flag: Option<u64>) -> (u64, Option<String>) {
    timeout_secs(flag, std::env::var_os(TIMEOUT_ENV).as_deref())
}

/// [`parallel`] with the real environment, CPU count and load.
pub(super) fn parallel_from_host(n: usize) -> (usize, Option<String>) {
    let cpus = std::thread::available_parallelism().map_or(1, |c| c.get());
    parallel(
        std::env::var_os(PARALLEL_ENV).as_deref(),
        cpus,
        read_load1(),
        n,
    )
}

/// The one-minute load, or `None` where `/proc/loadavg` is missing or
/// unreadable (any non-Linux host).
fn read_load1() -> Option<f64> {
    let file = std::fs::File::open("/proc/loadavg").ok()?;
    let mut text = String::new();
    file.take(LOADAVG_CAP).read_to_string(&mut text).ok()?;
    parse_load1(&text)
}
