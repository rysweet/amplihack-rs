//! `probe_limits` (issue #1698): the per-probe deadline and the probe pool
//! size, shared by `profile usage` and the rotator.
//!
//! Contract:
//! - `TIMEOUT_ENV` = `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS`,
//!   `PARALLEL_ENV` = `AMPLIHACK_PROFILE_USAGE_PARALLEL`.
//! - `DEFAULT_TIMEOUT_SECS` = 120, `MAX_TIMEOUT_SECS` = 600,
//!   `MAX_PARALLEL` = 16.
//! - `timeout_secs(flag, env) -> (u64, Option<String>)`: the flag, then the
//!   variable (1 to 600), then 120. Empty means unset. A bad value gives a
//!   warning with the value shown by `{:?}`, cut to 32 characters.
//! - `parallel(env, cpus, load1, n) -> (usize, Option<String>)`:
//!   `free = max(cpus - load1, 0)` (all CPUs when `load1` is `None`),
//!   `bound = clamp(floor(free / 8), 2, 4)`; a valid variable (1 to 16)
//!   replaces the bound; the result is `min(value, n)` and at least 1. A bad
//!   value warns with the derived bound before the cap at `n`.
//! - `parse_load1(text) -> Option<f64>`: the first field of `/proc/loadavg`;
//!   `None` when empty, negative or not finite.
//! - `PROBE_ENV` = `AMPLIHACK_PROFILE_USAGE_PROBE`;
//!   `probe_mode(env) -> (ProbeMode, Option<String>)`: `auto` or `text`,
//!   matched byte for byte; empty means unset; anything else warns, with the
//!   value shown as for the other variables, and uses `Auto`.
//!
//! Every test calls the pure functions; none reads or changes the process
//! environment or `/proc`.

use super::probe_limits::{
    DEFAULT_TIMEOUT_SECS, MAX_PARALLEL, MAX_TIMEOUT_SECS, PARALLEL_ENV, PROBE_ENV, ProbeMode,
    TIMEOUT_ENV, parallel, parse_load1, probe_mode, timeout_secs,
};
use std::ffi::OsStr;

fn env(v: &str) -> Option<&OsStr> {
    Some(OsStr::new(v))
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn constants_are_the_documented_values() {
    assert_eq!(PROBE_ENV, "AMPLIHACK_PROFILE_USAGE_PROBE");
    assert_eq!(TIMEOUT_ENV, "AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS");
    assert_eq!(PARALLEL_ENV, "AMPLIHACK_PROFILE_USAGE_PARALLEL");
    assert_eq!(DEFAULT_TIMEOUT_SECS, 120);
    assert_eq!(MAX_TIMEOUT_SECS, 600);
    assert_eq!(MAX_PARALLEL, 16);
}

#[test]
fn default_deadline_leaves_headroom_over_the_measured_probe() {
    // One probe took 71.5 s at load 194 (issue #1698).
    const MEASURED_SECS: f64 = 71.5;
    assert!(DEFAULT_TIMEOUT_SECS as f64 >= 1.5 * MEASURED_SECS);
    const { assert!(DEFAULT_TIMEOUT_SECS <= MAX_TIMEOUT_SECS) };
}

// ---------------------------------------------------------------------------
// Deadline
// ---------------------------------------------------------------------------

#[test]
fn timeout_flag_wins_then_env_then_default() {
    assert_eq!(timeout_secs(None, None), (120, None));
    assert_eq!(timeout_secs(Some(2), env("45")), (2, None));
    assert_eq!(timeout_secs(None, env("45")), (45, None));
    assert_eq!(timeout_secs(None, env("1")), (1, None));
    assert_eq!(timeout_secs(None, env("600")), (600, None));
    assert_eq!(
        timeout_secs(None, env("")),
        (120, None),
        "empty means unset"
    );
}

#[test]
fn bad_timeout_env_warns_with_the_value_escaped_and_cut() {
    let (secs, warning) = timeout_secs(None, env("soon"));
    assert_eq!(secs, 120);
    assert_eq!(
        warning.as_deref(),
        Some(
            "warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS \"soon\" (use 1 to 600); using 120"
        )
    );
    for bad in ["0", "601", "-1", "1.5", " 30", "99999999999999999999999"] {
        let (secs, warning) = timeout_secs(None, env(bad));
        assert_eq!(secs, 120, "{bad:?}");
        assert!(warning.is_some(), "{bad:?}");
    }
    let long = format!("\u{1b}[2J{}", "y".repeat(200));
    let (_, warning) = timeout_secs(None, env(&long));
    let warning = warning.unwrap();
    assert!(!warning.contains('\u{1b}'), "{warning:?}");
    assert!(
        !warning.contains(&"y".repeat(33)),
        "value cut to 32 characters: {warning}"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_timeout_env_warns_and_uses_the_default() {
    use std::os::unix::ffi::OsStrExt;
    let (secs, warning) = timeout_secs(None, Some(OsStr::from_bytes(b"3\xff0")));
    assert_eq!(secs, 120);
    assert!(warning.is_some());
}

// ---------------------------------------------------------------------------
// Parallelism: derived bound
// ---------------------------------------------------------------------------

/// The derived value for `n` profiles, with no variable set.
fn derived(cpus: usize, load1: Option<f64>, n: usize) -> usize {
    let (p, warning) = parallel(None, cpus, load1, n);
    assert!(warning.is_none(), "no variable, no warning");
    p
}

#[test]
fn derived_bound_matches_the_documented_hosts() {
    // The table in docs/reference/profile-command.md.
    assert_eq!(derived(72, Some(0.0), 14), 4, "72 CPUs, idle");
    assert_eq!(derived(72, Some(38.0), 14), 4, "72 CPUs, load 38");
    assert_eq!(derived(72, Some(194.0), 14), 2, "72 CPUs, load 194");
    assert_eq!(derived(8, Some(0.0), 14), 2, "8 CPUs, idle");
}

#[test]
fn derived_bound_is_floor_of_free_over_eight_between_two_and_four() {
    // free 24 -> 3; free 31.9 -> 3 (rounded down); free 32 -> 4.
    assert_eq!(derived(72, Some(48.0), 14), 3);
    assert_eq!(derived(72, Some(40.1), 14), 3);
    assert_eq!(derived(72, Some(40.0), 14), 4);
    // free 15.9 -> 1, raised to the floor of 2.
    assert_eq!(derived(16, Some(0.1), 14), 2);
    // A huge host never goes above 4.
    assert_eq!(derived(1024, Some(0.0), 14), 4);
    // Load above the CPU count gives free 0, not a negative number.
    assert_eq!(derived(4, Some(500.0), 14), 2);
}

#[test]
fn a_single_cpu_host_still_gets_the_floor() {
    assert_eq!(derived(1, Some(0.0), 14), 2);
    assert_eq!(derived(0, None, 14), 2, "cpus 0 never panics");
}

#[test]
fn missing_load_counts_every_cpu_as_free() {
    assert_eq!(derived(72, None, 14), 4);
    assert_eq!(derived(24, None, 14), 3);
    assert_eq!(derived(8, None, 14), 2);
}

#[test]
fn a_non_finite_or_negative_load_counts_as_missing() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -3.0] {
        assert_eq!(derived(24, Some(bad), 14), 3, "{bad}");
    }
}

#[test]
fn the_result_is_capped_at_the_number_of_profiles_and_never_zero() {
    assert_eq!(derived(72, Some(0.0), 3), 3);
    assert_eq!(derived(72, Some(0.0), 1), 1);
    assert_eq!(derived(72, Some(194.0), 1), 1);
    assert_eq!(derived(72, Some(0.0), 0), 1, "n 0 still gives 1");
}

// ---------------------------------------------------------------------------
// Parallelism: the variable
// ---------------------------------------------------------------------------

#[test]
fn a_valid_variable_replaces_the_derived_bound() {
    assert_eq!(parallel(env("3"), 72, Some(194.0), 14), (3, None));
    assert_eq!(parallel(env("1"), 72, Some(0.0), 14), (1, None));
    assert_eq!(parallel(env("16"), 72, Some(0.0), 20), (16, None));
    assert_eq!(
        parallel(env("8"), 8, Some(0.0), 14),
        (8, None),
        "restores the old behaviour on an idle host"
    );
}

#[test]
fn a_valid_variable_is_still_capped_at_the_number_of_profiles() {
    assert_eq!(parallel(env("16"), 72, Some(0.0), 3), (3, None));
    assert_eq!(parallel(env("5"), 72, Some(0.0), 0), (1, None));
}

#[test]
fn an_empty_variable_counts_as_unset() {
    assert_eq!(parallel(env(""), 72, Some(194.0), 14), (2, None));
    assert_eq!(parallel(env(""), 72, Some(0.0), 14), (4, None));
}

#[test]
fn a_bad_variable_warns_and_uses_the_derived_bound() {
    let (p, warning) = parallel(env("lots"), 72, Some(194.0), 14);
    assert_eq!(p, 2);
    assert_eq!(
        warning.as_deref(),
        Some("warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL \"lots\" (use 1 to 16); using 2")
    );
    for bad in ["0", "17", "-1", "2.5", " 3", "x", "99999999999999999999999"] {
        let (p, warning) = parallel(env(bad), 72, Some(0.0), 14);
        assert_eq!(p, 4, "{bad:?}");
        let warning = warning.unwrap_or_else(|| panic!("{bad:?} must warn"));
        assert!(
            warning.starts_with("warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL "),
            "{warning}"
        );
        assert!(warning.ends_with("(use 1 to 16); using 4"), "{warning}");
    }
}

#[test]
fn the_warning_shows_the_bound_before_the_cap_at_the_number_of_profiles() {
    // Two profiles on an idle 72-CPU host: the bound is 4, two probes run.
    let (p, warning) = parallel(env("0"), 72, Some(0.0), 2);
    assert_eq!(p, 2);
    assert_eq!(
        warning.as_deref(),
        Some("warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL \"0\" (use 1 to 16); using 4")
    );
}

#[test]
fn a_hostile_variable_is_escaped_and_cut_in_the_warning() {
    let long = format!("\u{1b}[2J\n{}", "z".repeat(200));
    let (_, warning) = parallel(env(&long), 72, Some(0.0), 14);
    let warning = warning.unwrap();
    assert!(!warning.contains('\u{1b}'), "{warning:?}");
    assert!(!warning.contains('\n'), "one line only: {warning:?}");
    assert!(
        !warning.contains(&"z".repeat(33)),
        "value cut to 32 characters: {warning}"
    );
}

#[cfg(unix)]
#[test]
fn a_non_utf8_variable_warns_and_uses_the_derived_bound() {
    use std::os::unix::ffi::OsStrExt;
    let (p, warning) = parallel(Some(OsStr::from_bytes(b"\xff")), 72, Some(194.0), 14);
    assert_eq!(p, 2);
    assert!(warning.is_some());
}

// ---------------------------------------------------------------------------
// Probe method
// ---------------------------------------------------------------------------

#[test]
fn the_probe_method_is_auto_unless_text_is_asked_for() {
    assert_eq!(probe_mode(None), (ProbeMode::Auto, None));
    assert_eq!(probe_mode(env("")), (ProbeMode::Auto, None));
    assert_eq!(probe_mode(env("auto")), (ProbeMode::Auto, None));
    assert_eq!(probe_mode(env("text")), (ProbeMode::Text, None));
}

#[test]
fn any_other_probe_method_warns_and_uses_auto() {
    for bad in ["Text", " auto", "auto ", "AUTO", "txt", "text\n", "0"] {
        let (mode, warning) = probe_mode(env(bad));
        assert_eq!(mode, ProbeMode::Auto, "{bad:?}");
        assert_eq!(
            warning.as_deref(),
            Some(
                format!(
                    "warning: ignored AMPLIHACK_PROFILE_USAGE_PROBE {bad:?} (use auto or text); using auto"
                )
                .as_str()
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn a_hostile_probe_method_is_escaped_and_cut_in_the_warning() {
    let long = "m".repeat(40);
    let (_, warning) = probe_mode(env(&long));
    let warning = warning.unwrap();
    assert!(
        warning.contains(&format!("{:?}", "m".repeat(32))),
        "{warning}"
    );
    assert!(!warning.contains(&"m".repeat(33)), "{warning}");
    let (_, warning) = probe_mode(env("\u{1b}[31m\u{202e}"));
    let warning = warning.unwrap();
    assert!(!warning.contains('\u{1b}'), "{warning:?}");
    assert!(!warning.contains('\u{202e}'), "{warning:?}");
    assert!(warning.contains("\\u{1b}"), "{warning:?}");
}

#[cfg(unix)]
#[test]
fn a_non_utf8_probe_method_warns_and_uses_auto() {
    use std::os::unix::ffi::OsStrExt;
    let (mode, warning) = probe_mode(Some(OsStr::from_bytes(b"\xff")));
    assert_eq!(mode, ProbeMode::Auto);
    let warning = warning.unwrap();
    assert!(warning.contains("\u{fffd}"), "{warning}");
    assert!(
        warning.ends_with("(use auto or text); using auto"),
        "{warning}"
    );
}

// ---------------------------------------------------------------------------
// /proc/loadavg
// ---------------------------------------------------------------------------

#[test]
fn parse_load1_reads_the_first_field() {
    assert_eq!(
        parse_load1("194.31 180.02 150.77 12/3456 98765\n"),
        Some(194.31)
    );
    assert_eq!(parse_load1("0.00 0.01 0.05 1/100 1"), Some(0.0));
    assert_eq!(parse_load1("3"), Some(3.0));
}

#[test]
fn parse_load1_rejects_what_is_not_a_load() {
    for bad in [
        "", "\n", "   ", "x 1 2", "-1.0 0 0", "NaN 0 0", "inf 0 0", "-inf 1 1",
    ] {
        assert_eq!(parse_load1(bad), None, "{bad:?}");
    }
}
