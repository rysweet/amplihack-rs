//! Issue #1870 (review round 3): the freshness gate's re-read reads the
//! target the way `AMPLIHACK_PROFILE_USAGE_PROBE` says, as the rest of the
//! cycle does. Under `text` (documented as "No passive or direct reads") it
//! runs `claude -p /usage`, never a direct read; unset, a failed direct read
//! falls back to `claude`, as the measurement does.
//!
//! `amplihack profile rotate` runs once against a temporary HOME and
//! profile root and the fixture's fake `claude`, which counts its `/usage`
//! probes per account. The fixture's logins cannot be read directly, so a
//! re-read that ignored the mode would fail and log
//! `reread: spare failed; treated as unknown` without a second probe.
//!
//! `spare` is made stale by a `swapped_away` entry 200 s ahead of now, the
//! furthest a loaded ledger keeps is 300 s. Its re-read is therefore read
//! (and logged) but still not strictly later than the damping time, so it
//! is then made unknown and the swap goes to `extra`.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A fixture with a rotate ledger in which `spare` was swapped away from
/// 200 s from now, and `spare` 10/20 and `extra` 50/30 as candidates for
/// the live `work` at 90/40. `extra`'s week is below `work`'s, so it ranks
/// ahead of it (issue #2013, 7 November addendum).
fn damped_spare() -> Fixture {
    let f = Fixture::new();
    // A first cycle with no room anywhere writes the ledger and swaps
    // nothing.
    f.usage("spare", 100, 100);
    f.usage("extra", 100, 100);
    f.run(&["profile", "rotate"]);
    let path = f.rotate_dir().join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("the first cycle wrote a ledger"))
            .unwrap();
    assert_eq!(ledger["live"], "work", "no swap yet: {ledger}");
    ledger["swapped_away"] = serde_json::json!({"spare": unix_now() + 200});
    fs::write(&path, ledger.to_string()).unwrap();
    f.usage("spare", 10, 20);
    f.usage("extra", 50, 30);
    f
}

/// Runs one rotate with `probe` as `AMPLIHACK_PROFILE_USAGE_PROBE` (unset
/// for `None`), without the fixture's wait past the damping time. Returns
/// the log the run added.
fn rotate(f: &Fixture, probe: Option<&str>) -> String {
    let before = f.log();
    let mut c = f.cmd(&["profile", "rotate"]);
    if let Some(v) = probe {
        c.env("AMPLIHACK_PROFILE_USAGE_PROBE", v);
    }
    let out = c.output().unwrap();
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "rotate failed: {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    );
    let log = f.log();
    log.strip_prefix(&before).unwrap_or(&log).to_string()
}

fn assert_reread_through_claude(probe: Option<&str>) {
    let f = damped_spare();
    let probes_before = f.usage_probes("spare");
    let log = rotate(&f, probe);
    assert_eq!(
        f.usage_probes("spare") - probes_before,
        2,
        "{probe:?}: spare is read by the cycle and again by the gate, both through claude:\n{log}"
    );
    assert!(
        log.contains("reread: spare reading stale s=10% w=20%; fresh s=10% w=20%"),
        "{probe:?}: the re-read gave a reading:\n{log}"
    );
    assert!(
        !log.contains("reread: spare failed"),
        "{probe:?}: the re-read did not fail for want of a direct read:\n{log}"
    );
    assert!(
        log.contains("swap work->extra"),
        "{probe:?}: still damped, so the swap goes to the next candidate:\n{log}"
    );
    assert_eq!(f.home_account(), "uuid-extra");
}

#[test]
fn under_text_the_reread_runs_claude_and_makes_no_direct_read() {
    assert_reread_through_claude(Some("text"));
}

#[test]
fn under_auto_the_reread_runs_claude() {
    assert_reread_through_claude(Some("auto"));
}

#[test]
fn unset_a_failed_direct_reread_falls_back_to_claude() {
    assert_reread_through_claude(None);
}

/// Review round 4: a cycle that re-reads warns about a bad probe variable
/// once, not once more for the gate and again for each re-read.
#[test]
fn a_bad_probe_variable_warns_once_per_cycle_even_with_a_reread() {
    let f = damped_spare();
    let probes_before = f.usage_probes("spare");
    let out = f
        .cmd(&["profile", "rotate"])
        .env("AMPLIHACK_PROFILE_USAGE_PROBE", "bogus")
        .env("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "bogus")
        .output()
        .unwrap();
    assert_no_secrets(&out);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "rotate failed: {stderr}");
    assert_eq!(
        f.usage_probes("spare") - probes_before,
        2,
        "the gate re-read spare, so the warnings had a chance to repeat:\n{}",
        f.log()
    );
    for var in [
        "AMPLIHACK_PROFILE_USAGE_PROBE",
        "AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS",
    ] {
        assert_eq!(
            stderr.matches(&format!("warning: ignored {var} ")).count(),
            1,
            "{var} is warned about once per cycle:\n{stderr}"
        );
    }
}
