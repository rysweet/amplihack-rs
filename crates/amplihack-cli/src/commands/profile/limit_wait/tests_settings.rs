//! The usage-limit wait's settings (issues #1618, #1761): read from
//! `config.toml` and the environment through the user-config loader, on by
//! default, off under `CI=true` unless set explicitly, and `1`/`0` accepted
//! in the variable. The run budget (default 21600 s) and the waits per step
//! (default 5, 1 to 100) come from the same loader; a project file can only
//! lower them. Every load uses a temporary HOME and a supplied environment.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_settings;`.

use super::settings::{
    BUDGET_KEY, DEFAULT_BUDGET_SECS, DEFAULT_MAX_WAITS_PER_STEP, MAX_WAITS_KEY, WaitSettings,
    from_effective,
};
use crate::user_config::schema;
use crate::user_config::{Effective, LoadCtx, Origin, load};
use std::fs;
use std::path::Path;

fn effective(home: &Path, cwd: &Path, project: bool, env: &[(&str, &str)]) -> Effective {
    let lookup = |name: &str| {
        env.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    };
    load(&LoadCtx {
        home: Some(home.to_path_buf()),
        cwd: cwd.to_path_buf(),
        read_project: project,
        env: &lookup,
        migrate: false,
    })
}

fn settings(home: &Path, env: &[(&str, &str)], ci: Option<&str>) -> WaitSettings {
    from_effective(&effective(home, home, false, env), ci)
}

fn user_file(home: &Path, body: &str) {
    let dir = home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), body).unwrap();
}

const ON: WaitSettings = WaitSettings {
    enabled: true,
    max_wait_secs: None,
    off_in_ci: false,
    budget_secs: 21_600,
    max_waits_per_step: 5,
};
const OFF: WaitSettings = WaitSettings {
    enabled: false,
    ..ON
};

const BUDGET_VAR: &str = "AMPLIHACK_USAGE_LIMIT_WAIT_BUDGET_SECS";
const WAITS_VAR: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAITS_PER_STEP";

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
        ..ON
    };
    assert_eq!(settings(home.path(), &[], None), file);
    let env = [
        ("AMPLIHACK_USAGE_LIMIT_WAIT", "true"),
        ("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "30"),
    ];
    let both = WaitSettings {
        max_wait_secs: Some(30),
        ..ON
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

// --- issue #1761: the run budget and the waits per step -------------------

#[test]
fn the_budget_and_the_per_step_cap_default_to_six_hours_and_five() {
    let home = tempfile::tempdir().unwrap();
    let s = settings(home.path(), &[], None);
    assert_eq!((s.budget_secs, s.max_waits_per_step), (21_600, 5));
}

#[test]
fn the_fallback_constants_equal_the_schema_defaults() {
    assert_eq!(DEFAULT_BUDGET_SECS, 21_600);
    assert_eq!(DEFAULT_MAX_WAITS_PER_STEP, 5);
    assert_eq!(BUDGET_KEY, "recipe.usage_limit_wait_budget_secs");
    assert_eq!(MAX_WAITS_KEY, "recipe.usage_limit_max_waits_per_step");
    let default = |k: &str| schema::key(k).and_then(|d| d.default);
    assert_eq!(
        default(BUDGET_KEY),
        Some(DEFAULT_BUDGET_SECS.to_string().as_str())
    );
    assert_eq!(
        default(MAX_WAITS_KEY),
        Some(DEFAULT_MAX_WAITS_PER_STEP.to_string().as_str())
    );
}

#[test]
fn the_file_and_the_variables_set_the_budget_and_the_cap() {
    let home = tempfile::tempdir().unwrap();
    user_file(
        home.path(),
        "[recipe]\nusage_limit_wait_budget_secs = 3600\nusage_limit_max_waits_per_step = 2\n",
    );
    let s = settings(home.path(), &[], None);
    assert_eq!((s.budget_secs, s.max_waits_per_step), (3600, 2));
    for (budget, waits, want) in [
        ("600", "3", (600, 3)),
        ("0", "1", (0, 1)),
        ("18446744073709551615", "100", (u64::MAX, 100)),
    ] {
        let env = [(BUDGET_VAR, budget), (WAITS_VAR, waits)];
        let s = settings(home.path(), &env, None);
        assert_eq!((s.budget_secs, s.max_waits_per_step), want, "{env:?}");
    }
}

#[test]
fn an_invalid_variable_gives_the_file_value_or_the_default() {
    let bad = [
        ("abc", "0"),
        ("-1", "101"),
        ("1.5", "x"),
        ("60s", "-3"),
        ("18446744073709551616", "4294967301"),
    ];
    let home = tempfile::tempdir().unwrap();
    for (budget, waits) in bad {
        let env = [(BUDGET_VAR, budget), (WAITS_VAR, waits)];
        let e = effective(home.path(), home.path(), false, &env);
        for key in [BUDGET_KEY, MAX_WAITS_KEY] {
            assert!(
                matches!(e.entry(key).origin, Origin::EnvInvalid { .. }),
                "{key} for {env:?}"
            );
        }
        let s = from_effective(&e, None);
        assert_eq!(
            (s.budget_secs, s.max_waits_per_step),
            (21_600, 5),
            "{env:?}"
        );
    }
    user_file(
        home.path(),
        "[recipe]\nusage_limit_wait_budget_secs = 900\nusage_limit_max_waits_per_step = 7\n",
    );
    for (budget, waits) in bad {
        let env = [(BUDGET_VAR, budget), (WAITS_VAR, waits)];
        let s = settings(home.path(), &env, None);
        assert_eq!((s.budget_secs, s.max_waits_per_step), (900, 7), "{env:?}");
    }
}

/// A temp HOME and a git checkout whose `.amplihack/config.toml` is `body`.
fn with_project(body: &str) -> (tempfile::TempDir, Effective) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (home, project) = (root.join("home"), root.join("project"));
    fs::create_dir_all(home.join(".amplihack")).unwrap();
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::write(project.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::create_dir_all(project.join(".amplihack")).unwrap();
    fs::write(project.join(".amplihack").join("config.toml"), body).unwrap();
    let e = effective(&home, &project, true, &[]);
    (tmp, e)
}

#[test]
fn a_project_file_cannot_raise_the_budget_or_the_cap() {
    let (_tmp, e) = with_project(
        "[recipe]\nusage_limit_wait_budget_secs = 99999\nusage_limit_max_waits_per_step = 50\n",
    );
    let s = from_effective(&e, None);
    assert_eq!((s.budget_secs, s.max_waits_per_step), (21_600, 5));
    for key in [BUDGET_KEY, MAX_WAITS_KEY] {
        assert!(
            e.diagnostics.iter().any(|d| d.key.as_deref() == Some(key)
                && d.reason.contains("a project file can only lower it")),
            "no diagnostic for {key}: {:?}",
            e.diagnostics
        );
    }
}

#[test]
fn a_project_file_can_lower_the_budget_and_the_cap() {
    let (_tmp, e) = with_project(
        "[recipe]\nusage_limit_wait_budget_secs = 60\nusage_limit_max_waits_per_step = 2\n",
    );
    let s = from_effective(&e, None);
    assert_eq!((s.budget_secs, s.max_waits_per_step), (60, 2));
    assert!(e.diagnostics.is_empty(), "{:?}", e.diagnostics);
}
