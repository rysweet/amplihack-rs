//! The usage-limit wait's settings (issue #1618): read from `config.toml`
//! and the environment through the user-config loader, on by default, off
//! under `CI=true` unless set explicitly, and `1`/`0` accepted in the
//! variable. Every load uses a temporary HOME and a supplied environment.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_settings;`.

use super::plan::MIN_WAIT_SECS;
use super::settings::{SHORT_WAIT_SECS, WaitSettings, from_effective, length_warning};
use crate::user_config::{LoadCtx, load};
use std::fs;
use std::path::Path;

fn settings(home: &Path, env: &[(&str, &str)], ci: Option<&str>) -> WaitSettings {
    settings_and_warning(home, env, ci).0
}

/// The settings and the length warning `resolve` would print for them.
fn settings_and_warning(
    home: &Path,
    env: &[(&str, &str)],
    ci: Option<&str>,
) -> (WaitSettings, Option<String>) {
    let lookup = |name: &str| {
        env.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    };
    let effective = load(&LoadCtx {
        home: Some(home.to_path_buf()),
        cwd: home.to_path_buf(),
        read_project: false,
        env: &lookup,
        migrate: false,
    });
    let s = from_effective(&effective, ci);
    let warning = length_warning(&effective, &s);
    (s, warning)
}

fn user_file(home: &Path, body: &str) {
    let dir = home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), body).unwrap();
}

/// The schema default (issue #1801).
const SIX_HOURS: u64 = 21_600;

const ON: WaitSettings = WaitSettings {
    enabled: true,
    max_wait_secs: Some(SIX_HOURS),
    off_in_ci: false,
};
const OFF: WaitSettings = WaitSettings {
    enabled: false,
    max_wait_secs: Some(SIX_HOURS),
    off_in_ci: false,
};

#[test]
fn the_wait_is_on_by_default_and_off_under_ci() {
    let home = tempfile::tempdir().unwrap();
    for ci in [
        None,
        Some(""),
        Some("  "),
        Some("false"),
        Some("False"),
        Some("FALSE"),
        Some(" false "),
        Some("0"),
        Some(" 0"),
    ] {
        assert_eq!(settings(home.path(), &[], ci), ON, "CI={ci:?}");
    }
    let off_in_ci = WaitSettings {
        off_in_ci: true,
        ..OFF
    };
    for ci in ["true", "TRUE", "1", "yes", "github", " true "] {
        assert_eq!(settings(home.path(), &[], Some(ci)), off_in_ci, "CI={ci}");
    }
}

#[test]
fn an_explicit_setting_wins_over_ci() {
    let home = tempfile::tempdir().unwrap();
    let env = [("AMPLIHACK_USAGE_LIMIT_WAIT", "1")];
    assert_eq!(settings(home.path(), &env, Some("true")), ON);
    user_file(home.path(), "[recipe]\nusage_limit_wait = true\n");
    assert_eq!(settings(home.path(), &[], Some("true")), ON);
}

#[test]
fn the_file_sets_both_keys_and_the_variable_outranks_it() {
    let home = tempfile::tempdir().unwrap();
    user_file(
        home.path(),
        "[recipe]\nusage_limit_wait = false\nusage_limit_max_wait_secs = 600\n",
    );
    let file = WaitSettings {
        enabled: false,
        max_wait_secs: Some(600),
        off_in_ci: false,
    };
    assert_eq!(settings(home.path(), &[], None), file);
    let env = [
        ("AMPLIHACK_USAGE_LIMIT_WAIT", "true"),
        ("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "300"),
    ];
    let both = WaitSettings {
        enabled: true,
        max_wait_secs: Some(300),
        off_in_ci: false,
    };
    assert_eq!(settings(home.path(), &env, None), both);
    let off = [("AMPLIHACK_USAGE_LIMIT_WAIT", "0")];
    assert!(!settings(home.path(), &off, None).enabled);
}

#[test]
fn an_invalid_variable_falls_back_to_the_file() {
    let home = tempfile::tempdir().unwrap();
    user_file(home.path(), "[recipe]\nusage_limit_wait = false\n");
    let env = [("AMPLIHACK_USAGE_LIMIT_WAIT", "maybe")];
    assert_eq!(settings(home.path(), &env, None), OFF);
}

#[test]
fn a_user_file_or_the_variable_can_go_above_the_default() {
    let home = tempfile::tempdir().unwrap();
    user_file(home.path(), "[recipe]\nusage_limit_max_wait_secs = 86400\n");
    assert_eq!(settings(home.path(), &[], None).max_wait_secs, Some(86_400));
    let env = [("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "90000")];
    assert_eq!(
        settings(home.path(), &env, None).max_wait_secs,
        Some(90_000)
    );
}

// An invalid maximum in the file or the variable is reported by the loader
// and ignored; the next setting down applies, ending at the schema default
// (docs/reference/user-config.md).
#[test]
fn an_invalid_maximum_in_the_file_falls_back_to_the_default() {
    for body in [
        "[recipe]\nusage_limit_max_wait_secs = -1\n",
        "[recipe]\nusage_limit_max_wait_secs = \"six hours\"\n",
        "[recipe]\nusage_limit_max_wait_secs = 1.5\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        user_file(home.path(), body);
        assert_eq!(
            settings(home.path(), &[], None).max_wait_secs,
            Some(SIX_HOURS),
            "{body:?}"
        );
    }
}

#[test]
fn an_invalid_maximum_variable_falls_back_to_the_file_then_the_default() {
    let home = tempfile::tempdir().unwrap();
    for bad in ["", "-5", "abc", "1e3", "99999999999999999999999"] {
        let env = [("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", bad)];
        assert_eq!(
            settings(home.path(), &env, None).max_wait_secs,
            Some(SIX_HOURS),
            "{bad:?}"
        );
    }
    user_file(home.path(), "[recipe]\nusage_limit_max_wait_secs = 600\n");
    let env = [("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "abc")];
    assert_eq!(settings(home.path(), &env, None).max_wait_secs, Some(600));
}

// Issue #2911: the key is the length of *one* wait, not a budget for the
// whole step, and a configured value below one wait's floor is raised to it
// rather than dropped. The wait loop polls every 30 seconds, so a shorter
// wait has no wake condition that could end it: a chain of those is the busy
// loop the pacing rule exists to prevent.
//
// The floor is applied here, where the value is read, so that `lower_only`
// keeps working. Dropping a too-small value instead would silently raise a
// project's configured number to the 21 600-second default.

#[test]
fn issue_2911_a_length_below_one_poll_interval_is_raised_to_the_floor() {
    for body in [0u64, 1, 30, 59] {
        let home = tempfile::tempdir().unwrap();
        user_file(
            home.path(),
            &format!("[recipe]\nusage_limit_max_wait_secs = {body}\n"),
        );
        assert_eq!(
            settings(home.path(), &[], None).max_wait_secs,
            Some(MIN_WAIT_SECS),
            "a configured {body}"
        );
        let value = body.to_string();
        let env = [("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", value.as_str())];
        assert_eq!(
            settings(home.path(), &env, None).max_wait_secs,
            Some(MIN_WAIT_SECS),
            "the variable set to {body}"
        );
    }
}

#[test]
fn issue_2911_a_length_at_or_above_the_floor_is_kept_as_configured() {
    for body in [MIN_WAIT_SECS, MIN_WAIT_SECS + 1, 600, SIX_HOURS] {
        let home = tempfile::tempdir().unwrap();
        user_file(
            home.path(),
            &format!("[recipe]\nusage_limit_max_wait_secs = {body}\n"),
        );
        assert_eq!(
            settings(home.path(), &[], None).max_wait_secs,
            Some(body),
            "a configured {body}"
        );
    }
}

/// A zero no longer means "do not wait": the way to turn the wait off is
/// `recipe.usage_limit_wait = false`, and that is unaffected by the floor.
#[test]
fn issue_2911_turning_the_wait_off_is_its_own_key() {
    let home = tempfile::tempdir().unwrap();
    user_file(
        home.path(),
        "[recipe]\nusage_limit_wait = false\nusage_limit_max_wait_secs = 0\n",
    );
    let got = settings(home.path(), &[], None);
    assert!(!got.enabled);
    assert_eq!(got.max_wait_secs, Some(MIN_WAIT_SECS));
}

// Review of PR #2992: before #2911 a configured `0` meant "fail at once".
// It now means a 60-second wait, and a host that still carries one is told
// so each time a step reads it, rather than finding out from a wait it did
// not expect.

#[test]
fn issue_2911_a_length_raised_to_the_floor_is_reported() {
    for body in [0u64, 59] {
        let home = tempfile::tempdir().unwrap();
        user_file(
            home.path(),
            &format!("[recipe]\nusage_limit_max_wait_secs = {body}\n"),
        );
        let (_, warning) = settings_and_warning(home.path(), &[], None);
        let warning = warning.unwrap_or_else(|| panic!("a configured {body}: no warning"));
        assert!(warning.starts_with("warning: "), "{warning}");
        assert!(
            warning.contains("recipe.usage_limit_max_wait_secs"),
            "{warning}"
        );
        assert!(
            warning.contains("recipe.usage_limit_wait = false"),
            "{warning}"
        );
        let value = body.to_string();
        let env = [("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", value.as_str())];
        assert!(
            settings_and_warning(home.path(), &env, None).1.is_some(),
            "the variable set to {body}"
        );
    }
}

/// Review of PR #2992: a length at or above the floor but below the
/// rotator's own cycle is not raised, and is not silent either. The value
/// is the outer bound of a capacity or login-change wait, and a step still
/// limited when such a wait ends runs its agent again, so such a value
/// re-launches the agent that often for as long as the limit lasts. The
/// line says so, names the default, and says a wait for a reset is not
/// shortened by it (round 6 of the review).
#[test]
fn a_short_length_above_the_floor_is_reported_as_the_poll_it_is() {
    for body in [MIN_WAIT_SECS, SHORT_WAIT_SECS - 1] {
        let home = tempfile::tempdir().unwrap();
        user_file(
            home.path(),
            &format!("[recipe]\nusage_limit_max_wait_secs = {body}\n"),
        );
        let (settings, warning) = settings_and_warning(home.path(), &[], None);
        assert_eq!(settings.max_wait_secs, Some(body), "not raised: {body}");
        let warning = warning.unwrap_or_else(|| panic!("a configured {body}: no warning"));
        assert!(warning.starts_with("warning: "), "{warning}");
        assert!(
            warning.contains("recipe.usage_limit_max_wait_secs"),
            "{warning}"
        );
        assert!(
            warning.contains("runs its agent again"),
            "the cost is named: {warning}"
        );
        assert!(
            warning.contains("A wait for a reset is not shortened by it"),
            "and what it does not touch: {warning}"
        );
        assert!(
            warning.contains("recipe.usage_limit_wait = false"),
            "{warning}"
        );
    }
    // The wait off, so the length is never used.
    let home = tempfile::tempdir().unwrap();
    user_file(
        home.path(),
        "[recipe]\nusage_limit_wait = false\nusage_limit_max_wait_secs = 120\n",
    );
    assert_eq!(settings_and_warning(home.path(), &[], None).1, None);
}

#[test]
fn issue_2911_no_length_warning_without_cause() {
    let home = tempfile::tempdir().unwrap();
    // Nothing configured: the default is above both thresholds.
    assert_eq!(settings_and_warning(home.path(), &[], None).1, None);
    // A length at or above the short-wait threshold.
    for body in [SHORT_WAIT_SECS, 21_600] {
        user_file(
            home.path(),
            &format!("[recipe]\nusage_limit_max_wait_secs = {body}\n"),
        );
        assert_eq!(
            settings_and_warning(home.path(), &[], None).1,
            None,
            "{body}"
        );
    }
    // A zero with the wait turned off, or off because this is CI: the
    // length is never used, so there is nothing to say.
    user_file(
        home.path(),
        "[recipe]\nusage_limit_wait = false\nusage_limit_max_wait_secs = 0\n",
    );
    assert_eq!(settings_and_warning(home.path(), &[], None).1, None);
    user_file(home.path(), "[recipe]\nusage_limit_max_wait_secs = 0\n");
    assert_eq!(settings_and_warning(home.path(), &[], Some("true")).1, None);
}
