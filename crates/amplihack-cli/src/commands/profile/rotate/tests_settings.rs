//! `rotate::settings`: flag, then `AMPLIHACK_ROTATE_*`, then default, with
//! the documented ranges (issue #1592).
//!
//! Contract (tests are children of `rotate`, declared in `rotate/mod.rs`):
//! - `TuneFlags { session_at, week_at, margin, upgrade_margin, hold }`, each
//!   `Option<String>`, `Default`.
//! - `settings::resolve(&TuneFlags, interval: Option<&str>, env: &Env,
//!   cfg: &Cfg) -> Result<Settings, ProfileError>`, where `Env` is
//!   `dyn Fn(&str) -> Option<OsString>` and `Cfg` (issue #2007) is
//!   `dyn Fn(&str) -> Result<Option<u64>, String>`, the user file's
//!   `rotator.*` value; `no_config` is a `Cfg` that sets nothing.
//! - `Settings { session_at, week_at, margin, upgrade_margin: u8, hold,
//!   interval: u64 }` and `Settings::to_args(with_interval) -> Vec<String>`.

use super::TuneFlags;
use super::settings::{Settings, no_config, resolve};
use crate::commands::profile::ProfileError;
use std::collections::HashMap;
use std::ffi::OsString;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

fn no_env() -> impl Fn(&str) -> Option<OsString> + use<> {
    env_of(&[])
}

fn flags() -> TuneFlags {
    TuneFlags::default()
}

fn err(r: Result<Settings, ProfileError>) -> String {
    match r {
        Err(e @ ProfileError::RotateConfig(_)) => e.to_string(),
        other => panic!("expected RotateConfig, got {other:?}"),
    }
}

#[test]
fn defaults_match_the_prototype() {
    let s = resolve(&flags(), None, &no_env(), &no_config).unwrap();
    assert_eq!(
        (s.session_at, s.week_at, s.margin, s.upgrade_margin),
        (85, 90, 5, 30)
    );
    assert_eq!((s.hold, s.interval), (3600, 600));
}

#[test]
fn flag_beats_variable_beats_default() {
    let env = env_of(&[
        ("AMPLIHACK_ROTATE_SESSION_AT", "70"),
        ("AMPLIHACK_ROTATE_WEEK_AT", "80"),
        ("AMPLIHACK_ROTATE_MARGIN", "7"),
        ("AMPLIHACK_ROTATE_UPGRADE_MARGIN", "40"),
        ("AMPLIHACK_ROTATE_HOLD", "60"),
        ("AMPLIHACK_ROTATE_INTERVAL", "30"),
    ]);
    let s = resolve(&flags(), None, &env, &no_config).unwrap();
    assert_eq!(
        (
            s.session_at,
            s.week_at,
            s.margin,
            s.upgrade_margin,
            s.hold,
            s.interval
        ),
        (70, 80, 7, 40, 60, 30)
    );
    let f = TuneFlags {
        session_at: Some("50".into()),
        hold: Some("0".into()),
        ..flags()
    };
    let s = resolve(&f, Some("5"), &env, &no_config).unwrap();
    assert_eq!(
        (s.session_at, s.week_at, s.hold, s.interval),
        (50, 80, 0, 5)
    );
}

#[test]
fn an_empty_variable_counts_as_unset() {
    let env = env_of(&[("AMPLIHACK_ROTATE_MARGIN", "")]);
    assert_eq!(resolve(&flags(), None, &env, &no_config).unwrap().margin, 5);
}

#[test]
fn range_boundaries_are_inclusive() {
    for (field, ok, bad) in [
        ("session-at", ["1", "100"], ["0", "101"]),
        ("week-at", ["1", "100"], ["0", "101"]),
        ("margin", ["0", "100"], ["101", "256"]),
        ("upgrade-margin", ["0", "100"], ["101", "1000"]),
    ] {
        let set = |v: &str| {
            let mut f = flags();
            let slot = match field {
                "session-at" => &mut f.session_at,
                "week-at" => &mut f.week_at,
                "margin" => &mut f.margin,
                _ => &mut f.upgrade_margin,
            };
            *slot = Some(v.into());
            f
        };
        for v in ok {
            assert!(
                resolve(&set(v), None, &no_env(), &no_config).is_ok(),
                "--{field} {v}"
            );
        }
        for v in bad {
            let e = err(resolve(&set(v), None, &no_env(), &no_config));
            assert!(e.contains(&format!("--{field}")), "{e}");
        }
    }
}

#[test]
fn interval_is_one_to_a_day_and_hold_has_no_upper_bound() {
    assert!(resolve(&flags(), Some("1"), &no_env(), &no_config).is_ok());
    assert!(resolve(&flags(), Some("86400"), &no_env(), &no_config).is_ok());
    assert!(err(resolve(&flags(), Some("0"), &no_env(), &no_config)).contains("--interval"));
    assert!(err(resolve(&flags(), Some("86401"), &no_env(), &no_config)).contains("--interval"));
    let f = TuneFlags {
        hold: Some("31536000".into()),
        ..flags()
    };
    assert_eq!(
        resolve(&f, None, &no_env(), &no_config).unwrap().hold,
        31_536_000
    );
}

#[test]
fn only_unsigned_decimal_integers_are_accepted() {
    for v in [
        "eighty",
        "+5",
        "-5",
        " 5",
        "5 ",
        "5.0",
        "0x10",
        "",
        "99999999999999999999999",
    ] {
        let f = TuneFlags {
            margin: Some(v.into()),
            ..flags()
        };
        let e = err(resolve(&f, None, &no_env(), &no_config));
        assert!(e.contains("--margin"), "{v:?}: {e}");
    }
}

#[test]
fn a_bad_variable_is_named_and_its_value_escaped_and_cut() {
    let long = format!("\x1b[31m{}", "9".repeat(80));
    let env = env_of(&[("AMPLIHACK_ROTATE_HOLD", &long)]);
    let e = err(resolve(&flags(), None, &env, &no_config));
    assert!(e.contains("AMPLIHACK_ROTATE_HOLD"), "{e}");
    assert!(!e.contains('\x1b'), "control characters escaped: {e:?}");
    assert!(
        !e.contains(&"9".repeat(40)),
        "value cut to 32 characters: {e}"
    );
}

#[test]
fn the_documented_message_shape() {
    let f = TuneFlags {
        session_at: Some("eighty".into()),
        ..flags()
    };
    let e = err(resolve(&f, None, &no_env(), &no_config));
    assert_eq!(
        e,
        "invalid rotate setting --session-at \"eighty\": expected a whole number from 1 to 100"
    );
}

#[test]
fn to_args_round_trips_through_resolve() {
    let env = env_of(&[("AMPLIHACK_ROTATE_WEEK_AT", "77")]);
    let f = TuneFlags {
        margin: Some("9".into()),
        ..flags()
    };
    let s = resolve(&f, Some("42"), &env, &no_config).unwrap();
    let args = s.to_args(true);
    assert!(!args.iter().any(|a| a == "--spawned"), "{args:?}");

    let mut back = flags();
    let mut interval = None;
    for pair in args.chunks(2) {
        let v = Some(pair[1].clone());
        match pair[0].as_str() {
            "--session-at" => back.session_at = v,
            "--week-at" => back.week_at = v,
            "--margin" => back.margin = v,
            "--upgrade-margin" => back.upgrade_margin = v,
            "--hold" => back.hold = v,
            "--overflow-wait" => back.overflow_wait = v,
            "--overflow-unknown-reset" => back.overflow_unknown_reset = v,
            "--interval" => interval = v,
            other => panic!("unexpected flag {other}"),
        }
    }
    let again = resolve(&back, interval.as_deref(), &no_env(), &no_config).unwrap();
    assert_eq!(again, s);
    assert!(!s.to_args(false).iter().any(|a| a == "--interval"));
}

// --- --overflow-wait (issue #1606) -----------------------------------------

#[test]
fn overflow_wait_defaults_to_half_an_hour() {
    assert_eq!(
        resolve(&flags(), None, &no_env(), &no_config)
            .unwrap()
            .overflow_wait,
        1800
    );
}

#[test]
fn overflow_wait_flag_beats_variable_beats_default() {
    let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "60")]);
    assert_eq!(
        resolve(&flags(), None, &env, &no_config)
            .unwrap()
            .overflow_wait,
        60
    );
    let f = TuneFlags {
        overflow_wait: Some("0".into()),
        ..flags()
    };
    assert_eq!(
        resolve(&f, None, &env, &no_config).unwrap().overflow_wait,
        0,
        "0 is allowed"
    );
    let empty = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "")]);
    assert_eq!(
        resolve(&flags(), None, &empty, &no_config)
            .unwrap()
            .overflow_wait,
        1800
    );
}

#[test]
fn overflow_wait_is_zero_to_a_week_and_rejected_not_clamped() {
    let f = |v: &str| TuneFlags {
        overflow_wait: Some(v.into()),
        ..flags()
    };
    assert_eq!(
        resolve(&f("604800"), None, &no_env(), &no_config)
            .unwrap()
            .overflow_wait,
        604_800
    );
    let e = err(resolve(&f("604801"), None, &no_env(), &no_config));
    assert!(e.contains("--overflow-wait"), "{e}");
    assert!(e.contains("0 to 604800"), "{e}");
    for bad in ["-1", "1.5", " 60", "1e3", "abc"] {
        let e = err(resolve(&f(bad), None, &no_env(), &no_config));
        assert!(e.contains("--overflow-wait"), "{bad:?}: {e}");
    }
    let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "999999")]);
    let e = err(resolve(&flags(), None, &env, &no_config));
    assert!(e.contains("AMPLIHACK_ROTATE_OVERFLOW_WAIT"), "{e}");
}

#[test]
fn to_args_passes_overflow_wait_to_the_background_process() {
    let f = TuneFlags {
        overflow_wait: Some("90".into()),
        ..flags()
    };
    let args = resolve(&f, None, &no_env(), &no_config)
        .unwrap()
        .to_args(false);
    let i = args
        .iter()
        .position(|a| a == "--overflow-wait")
        .expect("--overflow-wait in to_args");
    assert_eq!(args[i + 1], "90");
}

#[test]
fn rotate_help_lists_overflow_wait() {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let mut out = Vec::new();
    let rotate = cmd
        .find_subcommand_mut("profile")
        .and_then(|p| p.find_subcommand_mut("rotate"))
        .expect("profile rotate");
    rotate.write_long_help(&mut out).unwrap();
    let help = String::from_utf8(out).unwrap();
    assert!(help.contains("--overflow-wait"), "{help}");
}

#[test]
fn overflow_wait_variable_must_be_a_whole_number_of_seconds() {
    for bad in ["-1", "1.5", "abc", "30m"] {
        let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", bad)]);
        let e = err(resolve(&flags(), None, &env, &no_config));
        assert!(e.contains("AMPLIHACK_ROTATE_OVERFLOW_WAIT"), "{bad:?}: {e}");
    }
    let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "0")]);
    assert_eq!(
        resolve(&flags(), None, &env, &no_config)
            .unwrap()
            .overflow_wait,
        0
    );
}

// --- rotator.* config keys (issue #2007) -------------------------------------
//
// Contract, in `rotate::settings`:
// - `Origin { Flag, Env, Config, Default, Rotator }` and
//   `Threshold<T> { value: T, origin: Origin }`;
// - `thresholds(env, cfg) -> Thresholds { session_at, week_at: Result<
//   Threshold<u8>, _>, interval, codex_credit_floor: Result<Threshold<u64>, _> }`,
//   each field resolved on its own;
// - `codex_credit_floor(cfg) -> Result<Threshold<u64>, ProfileError>`, the one
//   function #2162 calls;
// - `UserConfig::at(home)`, `.get(key) -> Result<Option<u64>, String>`;
// - `#[cfg(test)] config_spec(key) -> Option<(default, min, max)>`, the
//   `Spec` behind each `rotator.*` key.
// Every test injects `env` and `cfg`; none reads the process environment
// or a real HOME.

mod config_2007 {
    use super::super::settings::{
        Origin, UserConfig, codex_credit_floor, config_spec, gate_from_env, no_config, resolve,
        thresholds,
    };
    use super::{env_of, err, flags, no_env};
    use crate::commands::profile::ProfileError;
    use crate::user_config::schema::{ValueKind, key as key_def};
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    type CfgResult = Result<Option<u64>, String>;

    fn cfg_of(pairs: &[(&str, u64)]) -> impl Fn(&str) -> CfgResult + use<> {
        let map: HashMap<String, u64> = pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        move |k| Ok(map.get(k).copied())
    }

    /// A config whose `bad` key cannot be read, with `why` as the reason.
    fn cfg_bad(bad: &'static str, why: &'static str) -> impl Fn(&str) -> CfgResult {
        move |k| {
            if k == bad {
                Err(why.to_string())
            } else {
                Ok(None)
            }
        }
    }

    fn fix_msg(key: &str, why: &str) -> String {
        format!(
            "invalid rotate setting {key}: {why}; fix it with `amplihack config set {key} <value>` or `amplihack config unset {key}`"
        )
    }

    // --- The schema and the resolver agree ----------------------------------

    #[test]
    fn each_config_default_and_range_is_the_rotator_s_own() {
        for key in ["rotator.session_at", "rotator.week_at", "rotator.interval"] {
            let (default, min, max) = config_spec(key).unwrap_or_else(|| panic!("{key}"));
            let def = key_def(key).unwrap();
            assert_eq!(def.kind, ValueKind::Int { min, max }, "{key}");
            assert_eq!(def.default, Some(default.to_string().as_str()), "{key}");
        }
        assert_eq!(config_spec("rotator.session_at"), Some((85, 1, 100)));
        assert_eq!(config_spec("rotator.week_at"), Some((90, 1, 100)));
        assert_eq!(config_spec("rotator.interval"), Some((600, 1, 86_400)));
        assert_eq!(config_spec("rotator.margin"), None);
    }

    #[test]
    fn the_codex_floor_default_is_the_codex_pick_default() {
        use amplihack_utils::codex_capacity::DEFAULT_FLOOR;
        let def = key_def("rotator.codex_credit_floor").unwrap();
        assert_eq!(def.default, Some("3000"));
        assert_eq!(
            def.default,
            Some((DEFAULT_FLOOR as u64).to_string().as_str())
        );
        assert_eq!(DEFAULT_FLOOR, (DEFAULT_FLOOR as u64) as f64, "whole number");
        let f = codex_credit_floor(&no_config).unwrap();
        assert_eq!((f.value, f.origin), (DEFAULT_FLOOR as u64, Origin::Default));
    }

    // --- Precedence ---------------------------------------------------------

    #[test]
    fn with_nothing_set_the_defaults_apply_with_origin_default() {
        let t = thresholds(&no_env(), &no_config);
        let s = t.session_at.unwrap();
        let w = t.week_at.unwrap();
        let i = t.interval.unwrap();
        let c = t.codex_credit_floor.unwrap();
        assert_eq!((s.value, s.origin), (85, Origin::Default));
        assert_eq!((w.value, w.origin), (90, Origin::Default));
        assert_eq!((i.value, i.origin), (600, Origin::Default));
        assert_eq!((c.value, c.origin), (3000, Origin::Default));
        let r = resolve(&flags(), None, &no_env(), &no_config).unwrap();
        assert_eq!((r.session_at, r.week_at, r.interval), (85, 90, 600));
    }

    #[test]
    fn config_values_apply_with_origin_config() {
        let cfg = cfg_of(&[
            ("rotator.session_at", 70),
            ("rotator.week_at", 80),
            ("rotator.interval", 120),
            ("rotator.codex_credit_floor", 0),
        ]);
        let s = resolve(&flags(), None, &no_env(), &cfg).unwrap();
        assert_eq!((s.session_at, s.week_at, s.interval), (70, 80, 120));
        // The other settings have no key and keep their defaults.
        assert_eq!((s.margin, s.upgrade_margin, s.hold), (5, 30, 3600));
        let t = thresholds(&no_env(), &cfg);
        assert_eq!(t.session_at.as_ref().unwrap().origin, Origin::Config);
        assert_eq!(t.week_at.as_ref().unwrap().value, 80);
        assert_eq!(t.interval.as_ref().unwrap().origin, Origin::Config);
        let c = t.codex_credit_floor.unwrap();
        assert_eq!((c.value, c.origin), (0, Origin::Config));
    }

    #[test]
    fn session_at_70_in_config_reaches_both_settings_and_the_gate() {
        let cfg = cfg_of(&[("rotator.session_at", 70)]);
        assert_eq!(
            resolve(&flags(), None, &no_env(), &cfg).unwrap().session_at,
            70
        );
        assert_eq!(gate_from_env(&no_env(), &cfg).unwrap().session_at, 70);
    }

    #[test]
    fn env_beats_config_for_session_at_and_interval() {
        let cfg = cfg_of(&[("rotator.session_at", 70), ("rotator.interval", 120)]);
        let env = env_of(&[
            ("AMPLIHACK_ROTATE_SESSION_AT", "60"),
            ("AMPLIHACK_ROTATE_INTERVAL", "300"),
        ]);
        let s = resolve(&flags(), None, &env, &cfg).unwrap();
        assert_eq!((s.session_at, s.interval), (60, 300));
        let t = thresholds(&env, &cfg);
        let sa = t.session_at.unwrap();
        let iv = t.interval.unwrap();
        assert_eq!((sa.value, sa.origin), (60, Origin::Env));
        assert_eq!((iv.value, iv.origin), (300, Origin::Env));
        assert_eq!(gate_from_env(&env, &cfg).unwrap().session_at, 60);
    }

    #[test]
    fn an_empty_variable_falls_through_to_config() {
        let cfg = cfg_of(&[("rotator.session_at", 70)]);
        let env = env_of(&[("AMPLIHACK_ROTATE_SESSION_AT", "")]);
        assert_eq!(resolve(&flags(), None, &env, &cfg).unwrap().session_at, 70);
    }

    #[test]
    fn flag_beats_env_and_config_for_session_at_and_interval() {
        let cfg = cfg_of(&[("rotator.session_at", 70), ("rotator.interval", 120)]);
        let env = env_of(&[
            ("AMPLIHACK_ROTATE_SESSION_AT", "60"),
            ("AMPLIHACK_ROTATE_INTERVAL", "300"),
        ]);
        let f = super::TuneFlags {
            session_at: Some("50".into()),
            ..flags()
        };
        let s = resolve(&f, Some("30"), &env, &cfg).unwrap();
        assert_eq!((s.session_at, s.interval), (50, 30));
    }

    #[test]
    fn a_flag_wins_even_when_the_config_value_is_bad() {
        // The flag is decided first, so a broken key it overrides never runs.
        let cfg = cfg_bad("rotator.session_at", "must be an integer from 1 to 100");
        let f = super::TuneFlags {
            session_at: Some("50".into()),
            ..flags()
        };
        assert_eq!(resolve(&f, None, &no_env(), &cfg).unwrap().session_at, 50);
    }

    #[test]
    fn the_codex_floor_has_no_variable() {
        let env = env_of(&[
            ("AMPLIHACK_ROTATE_CODEX_FLOOR", "10"),
            ("AMPLIHACK_CODEX_FLOOR", "10"),
        ]);
        let t = thresholds(&env, &cfg_of(&[("rotator.codex_credit_floor", 5000)]));
        let c = t.codex_credit_floor.unwrap();
        assert_eq!((c.value, c.origin), (5000, Origin::Config));
    }

    // --- Bad values fail closed, never clamped or defaulted --------------------

    #[test]
    fn a_bad_config_value_stops_resolve_and_names_the_key_and_the_fix() {
        for key in ["rotator.session_at", "rotator.week_at", "rotator.interval"] {
            let cfg = cfg_bad(key, "must be an integer from 1 to 100");
            let e = err(resolve(&flags(), None, &no_env(), &cfg));
            assert_eq!(e, fix_msg(key, "must be an integer from 1 to 100"), "{key}");
        }
    }

    #[test]
    fn an_out_of_range_config_value_is_rejected_not_clamped() {
        for (key, v) in [
            ("rotator.session_at", 0),
            ("rotator.session_at", 101),
            ("rotator.week_at", 256),
            ("rotator.week_at", 357),
            ("rotator.interval", 0),
            ("rotator.interval", 86_401),
        ] {
            let e = err(resolve(&flags(), None, &no_env(), &cfg_of(&[(key, v)])));
            assert!(e.contains(key), "{key}={v}: {e}");
            assert!(e.contains("config set"), "{key}={v}: {e}");
        }
        let e = codex_credit_floor(&cfg_of(&[("rotator.codex_credit_floor", 100_000_001)]))
            .unwrap_err();
        assert!(matches!(e, ProfileError::RotateConfig(_)), "{e:?}");
        assert!(e.to_string().contains("rotator.codex_credit_floor"), "{e}");
    }

    #[test]
    fn a_bad_config_value_closes_the_gate() {
        for key in ["rotator.session_at", "rotator.week_at"] {
            let cfg = cfg_bad(key, "must be an integer from 1 to 100");
            let e = gate_from_env(&no_env(), &cfg).unwrap_err();
            assert!(e.to_string().contains(key), "{e}");
        }
        // The gate does not use the interval, so its key cannot close it.
        let cfg = cfg_bad("rotator.interval", "must be an integer from 1 to 86400");
        assert!(gate_from_env(&no_env(), &cfg).is_ok());
    }

    #[test]
    fn a_bad_variable_is_an_error_even_with_good_config() {
        let cfg = cfg_of(&[("rotator.session_at", 70)]);
        let env = env_of(&[("AMPLIHACK_ROTATE_SESSION_AT", "abc")]);
        let e = err(resolve(&flags(), None, &env, &cfg));
        assert!(e.contains("AMPLIHACK_ROTATE_SESSION_AT"), "{e}");
        assert!(thresholds(&env, &cfg).session_at.is_err());
    }

    #[test]
    fn a_good_variable_shadows_a_bad_config_value() {
        let cfg = cfg_bad("rotator.interval", "must be an integer from 1 to 86400");
        let env = env_of(&[("AMPLIHACK_ROTATE_INTERVAL", "300")]);
        assert_eq!(resolve(&flags(), None, &env, &cfg).unwrap().interval, 300);
    }

    #[test]
    fn a_bad_key_changes_only_its_own_threshold() {
        let cfg = cfg_bad("rotator.week_at", "must be an integer from 1 to 100");
        let t = thresholds(&no_env(), &cfg);
        let e = t.week_at.unwrap_err().to_string();
        assert!(e.contains("rotator.week_at"), "{e}");
        assert_eq!(t.session_at.unwrap().value, 85);
        assert_eq!(t.interval.unwrap().value, 600);
        assert_eq!(t.codex_credit_floor.unwrap().value, 3000);
    }

    #[test]
    fn a_bad_codex_floor_is_an_error_naming_the_key() {
        let cfg = cfg_bad(
            "rotator.codex_credit_floor",
            "must be an integer from 0 to 100000000",
        );
        let e = codex_credit_floor(&cfg).unwrap_err();
        assert_eq!(
            e.to_string(),
            fix_msg(
                "rotator.codex_credit_floor",
                "must be an integer from 0 to 100000000"
            )
        );
        // The rotator itself has no use for the floor.
        assert!(resolve(&flags(), None, &no_env(), &cfg).is_ok());
    }

    // --- The real user file -------------------------------------------------

    struct Home {
        _tmp: tempfile::TempDir,
        home: std::path::PathBuf,
    }

    fn home_with(body: &str, mode: u32) -> Home {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap().join("home");
        let dir = home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        fs::write(&file, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = mode;
        Home { _tmp: tmp, home }
    }

    #[test]
    fn a_real_user_file_round_trips_into_settings() {
        let h = home_with("[rotator]\ninterval = 900\ncodex_credit_floor = 0\n", 0o600);
        let cfg = UserConfig::at(&h.home);
        assert_eq!(cfg.get("rotator.interval"), Ok(Some(900)));
        assert_eq!(cfg.get("rotator.session_at"), Ok(None));
        let s = resolve(&flags(), None, &no_env(), &|k| cfg.get(k)).unwrap();
        assert_eq!((s.session_at, s.interval), (85, 900));
        assert_eq!(codex_credit_floor(&|k| cfg.get(k)).unwrap().value, 0);
    }

    #[test]
    fn a_hand_written_101_stops_resolve_and_names_the_key() {
        let h = home_with("[rotator]\nsession_at = 101\n", 0o600);
        let cfg = UserConfig::at(&h.home);
        assert!(cfg.get("rotator.session_at").is_err());
        let e = err(resolve(&flags(), None, &no_env(), &|k| cfg.get(k)));
        assert!(
            e.starts_with("invalid rotate setting rotator.session_at: "),
            "{e}"
        );
        assert!(e.contains("from 1 to 100"), "{e}");
        assert!(
            !e.contains(&h.home.display().to_string()),
            "the message names the key, not the path: {e}"
        );
    }

    #[test]
    fn a_string_value_is_an_error_not_the_default() {
        let h = home_with("[rotator]\nweek_at = \"80\"\n", 0o600);
        let cfg = UserConfig::at(&h.home);
        assert!(cfg.get("rotator.week_at").is_err());
        assert!(resolve(&flags(), None, &no_env(), &|k| cfg.get(k)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_write_fails_closed_in_resolve_and_the_gate() {
        let h = home_with("[rotator]\nsession_at = 70\n", 0o664);
        let cfg = UserConfig::at(&h.home);
        let e = resolve(&flags(), None, &no_env(), &|k| cfg.get(k)).unwrap_err();
        assert!(matches!(e, ProfileError::RotateConfig(_)), "{e:?}");
        assert!(gate_from_env(&no_env(), &|k| cfg.get(k)).is_err());
        assert!(codex_credit_floor(&|k| cfg.get(k)).is_err());
    }

    #[test]
    fn a_guarded_home_outside_temp_is_an_error_for_every_key() {
        let cfg = UserConfig::at(Path::new("/"));
        for key in [
            "rotator.session_at",
            "rotator.week_at",
            "rotator.interval",
            "rotator.codex_credit_floor",
        ] {
            assert!(cfg.get(key).is_err(), "{key}");
        }
    }

    #[test]
    fn a_key_the_reader_did_not_load_is_an_error_not_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let cfg = UserConfig::at(&home);
        let e = cfg.get("rotator.no_such_key").unwrap_err();
        assert!(e.contains("not a rotator key"), "{e}");
    }

    #[test]
    fn an_absent_user_file_sets_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let cfg = UserConfig::at(&home);
        let s = resolve(&flags(), None, &no_env(), &|k| cfg.get(k)).unwrap();
        assert_eq!((s.session_at, s.week_at, s.interval), (85, 90, 600));
        assert_eq!(codex_credit_floor(&|k| cfg.get(k)).unwrap().value, 3000);
    }

    #[test]
    fn to_args_still_passes_every_value_as_a_flag() {
        // The background rotator reads no config: the parent resolves it.
        let cfg = cfg_of(&[("rotator.session_at", 70), ("rotator.interval", 120)]);
        let args = resolve(&flags(), None, &no_env(), &cfg)
            .unwrap()
            .to_args(true);
        let after = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        assert_eq!(after("--session-at").as_deref(), Some("70"));
        assert_eq!(after("--interval").as_deref(), Some("120"));
    }

    // --- Whole-file problems (D2 and the documented whole-file hint) -------
    //
    // docs/reference/user-config.md: when the whole user file is at fault,
    // `config set` and `config unset` cannot help, so the message names the
    // key and the reason and points at the file instead. It never contains
    // the file's path.

    const WHOLE_FILE_FIX: &str = "; fix the user config file first (`amplihack config list --show-origin` shows its path and the problem)";

    fn whole_file_msg(key: &str, why: &str) -> String {
        format!("invalid rotate setting {key}: {why}{WHOLE_FILE_FIX}")
    }

    /// The message for a whole-file problem: the documented hint, the
    /// reason, no `config set` advice and no path.
    fn assert_whole_file(e: &str, home: &Path) {
        assert!(e.starts_with("invalid rotate setting rotator."), "{e}");
        assert!(e.ends_with(WHOLE_FILE_FIX), "whole-file hint expected: {e}");
        assert!(
            !e.contains("amplihack config set") && !e.contains("amplihack config unset"),
            "config set cannot fix a whole-file problem: {e}"
        );
        assert!(
            !e.contains(&home.display().to_string()),
            "the message never contains the path: {e}"
        );
        assert!(!e.contains("config.toml"), "no file name either: {e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_group_writable_file_gives_the_documented_whole_file_message() {
        let h = home_with("[rotator]\nweek_at = 80\n", 0o664);
        let cfg = UserConfig::at(&h.home);
        let why = "ignored because the file is writable by group or others";
        // Each key's own message, exactly as documented.
        for key in [
            "rotator.session_at",
            "rotator.week_at",
            "rotator.interval",
            "rotator.codex_credit_floor",
        ] {
            let raw = cfg.get(key).unwrap_err();
            assert!(raw.contains(why), "{key}: {raw}");
            assert!(!raw.contains(&h.home.display().to_string()), "{key}: {raw}");
        }
        let t = thresholds(&no_env(), &|k| cfg.get(k));
        let week = t.week_at.unwrap_err().to_string();
        assert_eq!(week, whole_file_msg("rotator.week_at", why));
        let floor = t.codex_credit_floor.unwrap_err().to_string();
        assert_eq!(floor, whole_file_msg("rotator.codex_credit_floor", why));
        // The commands that act fail closed with the same hint.
        let e = err(resolve(&flags(), None, &no_env(), &|k| cfg.get(k)));
        assert_whole_file(&e, &h.home);
        let g = gate_from_env(&no_env(), &|k| cfg.get(k))
            .unwrap_err()
            .to_string();
        assert_whole_file(&g, &h.home);
    }

    #[test]
    fn an_unparseable_file_gives_the_whole_file_message_without_the_path() {
        let h = home_with("[rotator\nweek_at = = 80\n", 0o600);
        let cfg = UserConfig::at(&h.home);
        for key in [
            "rotator.session_at",
            "rotator.week_at",
            "rotator.interval",
            "rotator.codex_credit_floor",
        ] {
            let raw = cfg.get(key).unwrap_err();
            assert!(
                !raw.contains(&h.home.display().to_string()),
                "{key}: the reason is path-free: {raw}"
            );
            assert!(!raw.is_empty(), "{key}");
        }
        let e = err(resolve(&flags(), None, &no_env(), &|k| cfg.get(k)));
        assert_whole_file(&e, &h.home);
        let floor = codex_credit_floor(&|k| cfg.get(k)).unwrap_err().to_string();
        assert!(
            floor.starts_with("invalid rotate setting rotator.codex_credit_floor: "),
            "{floor}"
        );
        assert_whole_file(&floor, &h.home);
    }

    #[test]
    fn a_problem_with_one_key_keeps_the_config_set_hint() {
        // Only whole-file problems change the hint; a bad value in a sound
        // file is fixed with `config set` or `config unset`.
        let h = home_with("[rotator]\nweek_at = 0\n", 0o600);
        let cfg = UserConfig::at(&h.home);
        let e = thresholds(&no_env(), &|k| cfg.get(k))
            .week_at
            .unwrap_err()
            .to_string();
        assert!(!e.ends_with(WHOLE_FILE_FIX), "{e}");
        assert!(
            e.ends_with(
                "fix it with `amplihack config set rotator.week_at <value>` or `amplihack config unset rotator.week_at`"
            ),
            "{e}"
        );
        // The other keys in the same file are still read.
        let s = thresholds(&no_env(), &|k| cfg.get(k));
        assert_eq!(s.session_at.unwrap().value, 85);
    }

    // --- S1: a project file cannot move the rotator ------------------------

    #[test]
    fn a_project_file_cannot_change_week_at() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".amplihack")).unwrap();
        // A repository with its own config, as a cloned project would have.
        let project = root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(project.join(".amplihack")).unwrap();
        fs::write(
            project.join(".amplihack").join("config.toml"),
            "[rotator]\nweek_at = 100\nsession_at = 1\ncodex_credit_floor = 0\n",
        )
        .unwrap();
        let cfg = UserConfig::at(&home);
        assert_eq!(cfg.get("rotator.week_at"), Ok(None));
        assert_eq!(cfg.get("rotator.session_at"), Ok(None));
        assert_eq!(cfg.get("rotator.codex_credit_floor"), Ok(None));
        let s = resolve(&flags(), None, &no_env(), &|k| cfg.get(k)).unwrap();
        assert_eq!((s.session_at, s.week_at), (85, 90));
        let floor = codex_credit_floor(&|k| cfg.get(k)).unwrap();
        assert_eq!((floor.value, floor.origin), (3000, Origin::Default));
    }
}

// --- Issue #2007 G4 and S1 at the reader --------------------------------------

mod config_2007_isolation {
    use super::super::settings::{Origin, UserConfig, thresholds};
    use super::no_env;
    use std::fs;
    use std::path::Path;

    #[test]
    fn a_project_session_at_10_resolves_with_origin_default_while_the_user_file_is_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let dir = home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        let user = dir.join("config.toml");
        fs::write(&user, "[rotator]\ninterval = 900\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&user, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let project = root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(project.join(".amplihack")).unwrap();
        fs::write(
            project.join(".amplihack").join("config.toml"),
            "[rotator]\nsession_at = 10\n",
        )
        .unwrap();
        let cfg = UserConfig::at(&home);
        let t = thresholds(&no_env(), &|k| cfg.get(k));
        let s = t.session_at.unwrap();
        assert_eq!((s.value, s.origin), (85, Origin::Default));
        let i = t.interval.unwrap();
        assert_eq!((i.value, i.origin), (900, Origin::Config), "user file read");
    }

    #[test]
    fn a_guarded_home_error_does_not_quote_the_home_path() {
        // S1: the reason reaches `thresholds.*.error`; it must be path-free.
        let tmp = tempfile::tempdir().unwrap();
        let _ = tmp; // keep a temp dir alive; the guarded HOME is `/usr`.
        let home = Path::new("/usr");
        let cfg = UserConfig::at(home);
        let t = thresholds(&no_env(), &|k| cfg.get(k));
        let e = t.session_at.unwrap_err().to_string();
        assert!(e.contains("rotator.session_at"), "{e}");
        assert!(e.contains("AMPLIHACK_TEST_HOME_GUARD"), "{e}");
        assert!(!e.contains("/usr"), "no HOME in the reason: {e}");
    }
}
