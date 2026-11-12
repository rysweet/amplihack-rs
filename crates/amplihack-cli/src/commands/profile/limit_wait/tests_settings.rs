//! The usage-limit wait's settings (issue #1618): read from `config.toml`
//! and the environment through the user-config loader, on by default, off
//! under `CI=true` unless set explicitly, and `1`/`0` accepted in the
//! variable. Every load uses a temporary HOME and a supplied environment.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_settings;`.

use super::settings::{WaitSettings, from_effective};
use crate::user_config::{LoadCtx, load};
use std::fs;
use std::path::Path;

fn settings(home: &Path, env: &[(&str, &str)], ci: Option<&str>) -> WaitSettings {
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
    from_effective(&effective, ci)
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
        ("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "30"),
    ];
    let both = WaitSettings {
        enabled: true,
        max_wait_secs: Some(30),
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

#[test]
fn a_zero_maximum_is_kept_not_replaced_by_the_default() {
    let home = tempfile::tempdir().unwrap();
    user_file(home.path(), "[recipe]\nusage_limit_max_wait_secs = 0\n");
    assert_eq!(settings(home.path(), &[], None).max_wait_secs, Some(0));
}
