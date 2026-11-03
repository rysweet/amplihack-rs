//! `rotate::settings`: flag, then `AMPLIHACK_ROTATE_*`, then default, with
//! the documented ranges (issue #1592).
//!
//! Contract (tests are children of `rotate`, declared in `rotate/mod.rs`):
//! - `TuneFlags { session_at, week_at, margin, upgrade_margin, hold }`, each
//!   `Option<String>`, `Default`.
//! - `settings::resolve(&TuneFlags, interval: Option<&str>, env: &dyn
//!   Fn(&str) -> Option<OsString>) -> Result<Settings, ProfileError>`.
//! - `Settings { session_at, week_at, margin, upgrade_margin: u8, hold,
//!   interval: u64 }` and `Settings::to_args(with_interval) -> Vec<String>`.

use super::TuneFlags;
use super::settings::{Recovery, Settings, resolve};
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
    let s = resolve(&flags(), None, &no_env()).unwrap();
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
    let s = resolve(&flags(), None, &env).unwrap();
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
    let s = resolve(&f, Some("5"), &env).unwrap();
    assert_eq!(
        (s.session_at, s.week_at, s.hold, s.interval),
        (50, 80, 0, 5)
    );
}

#[test]
fn an_empty_variable_counts_as_unset() {
    let env = env_of(&[("AMPLIHACK_ROTATE_MARGIN", "")]);
    assert_eq!(resolve(&flags(), None, &env).unwrap().margin, 5);
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
            assert!(resolve(&set(v), None, &no_env()).is_ok(), "--{field} {v}");
        }
        for v in bad {
            let e = err(resolve(&set(v), None, &no_env()));
            assert!(e.contains(&format!("--{field}")), "{e}");
        }
    }
}

#[test]
fn interval_is_one_to_a_day_and_hold_has_no_upper_bound() {
    assert!(resolve(&flags(), Some("1"), &no_env()).is_ok());
    assert!(resolve(&flags(), Some("86400"), &no_env()).is_ok());
    assert!(err(resolve(&flags(), Some("0"), &no_env())).contains("--interval"));
    assert!(err(resolve(&flags(), Some("86401"), &no_env())).contains("--interval"));
    let f = TuneFlags {
        hold: Some("31536000".into()),
        ..flags()
    };
    assert_eq!(resolve(&f, None, &no_env()).unwrap().hold, 31_536_000);
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
        let e = err(resolve(&f, None, &no_env()));
        assert!(e.contains("--margin"), "{v:?}: {e}");
    }
}

#[test]
fn a_bad_variable_is_named_and_its_value_escaped_and_cut() {
    let long = format!("\x1b[31m{}", "9".repeat(80));
    let env = env_of(&[("AMPLIHACK_ROTATE_HOLD", &long)]);
    let e = err(resolve(&flags(), None, &env));
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
    let e = err(resolve(&f, None, &no_env()));
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
        recovery: Some("off".into()),
        ..flags()
    };
    let s = resolve(&f, Some("42"), &env).unwrap();
    assert_eq!(s.recovery, Recovery::Off);
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
            "--recovery" => back.recovery = v,
            other => panic!("unexpected flag {other}"),
        }
    }
    let again = resolve(&back, interval.as_deref(), &no_env()).unwrap();
    assert_eq!(again, s);
    assert!(!s.to_args(false).iter().any(|a| a == "--interval"));
}

// --- --overflow-wait (issue #1606) -----------------------------------------

#[test]
fn overflow_wait_defaults_to_half_an_hour() {
    assert_eq!(
        resolve(&flags(), None, &no_env()).unwrap().overflow_wait,
        1800
    );
}

#[test]
fn overflow_wait_flag_beats_variable_beats_default() {
    let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "60")]);
    assert_eq!(resolve(&flags(), None, &env).unwrap().overflow_wait, 60);
    let f = TuneFlags {
        overflow_wait: Some("0".into()),
        ..flags()
    };
    assert_eq!(
        resolve(&f, None, &env).unwrap().overflow_wait,
        0,
        "0 is allowed"
    );
    let empty = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "")]);
    assert_eq!(resolve(&flags(), None, &empty).unwrap().overflow_wait, 1800);
}

#[test]
fn overflow_wait_is_zero_to_a_week_and_rejected_not_clamped() {
    let f = |v: &str| TuneFlags {
        overflow_wait: Some(v.into()),
        ..flags()
    };
    assert_eq!(
        resolve(&f("604800"), None, &no_env())
            .unwrap()
            .overflow_wait,
        604_800
    );
    let e = err(resolve(&f("604801"), None, &no_env()));
    assert!(e.contains("--overflow-wait"), "{e}");
    assert!(e.contains("0 to 604800"), "{e}");
    for bad in ["-1", "1.5", " 60", "1e3", "abc"] {
        let e = err(resolve(&f(bad), None, &no_env()));
        assert!(e.contains("--overflow-wait"), "{bad:?}: {e}");
    }
    let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "999999")]);
    let e = err(resolve(&flags(), None, &env));
    assert!(e.contains("AMPLIHACK_ROTATE_OVERFLOW_WAIT"), "{e}");
}

#[test]
fn to_args_passes_overflow_wait_to_the_background_process() {
    let f = TuneFlags {
        overflow_wait: Some("90".into()),
        ..flags()
    };
    let args = resolve(&f, None, &no_env()).unwrap().to_args(false);
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
        let e = err(resolve(&flags(), None, &env));
        assert!(e.contains("AMPLIHACK_ROTATE_OVERFLOW_WAIT"), "{bad:?}: {e}");
    }
    let env = env_of(&[("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "0")]);
    assert_eq!(resolve(&flags(), None, &env).unwrap().overflow_wait, 0);
}

// --- --recovery (issue #1890) -----------------------------------------------

fn recovery_flag(v: &str) -> TuneFlags {
    TuneFlags {
        recovery: Some(v.into()),
        ..flags()
    }
}

#[test]
fn recovery_defaults_to_act() {
    assert_eq!(Recovery::default(), Recovery::Act);
    assert_eq!(
        resolve(&flags(), None, &no_env()).unwrap().recovery,
        Recovery::Act
    );
}

#[test]
fn recovery_accepts_act_log_and_off_from_the_flag_and_the_variable() {
    for (word, want) in [
        ("act", Recovery::Act),
        ("log", Recovery::Log),
        ("off", Recovery::Off),
    ] {
        let s = resolve(&recovery_flag(word), None, &no_env()).unwrap();
        assert_eq!(s.recovery, want, "--recovery {word}");
        let env = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", word)]);
        assert_eq!(resolve(&flags(), None, &env).unwrap().recovery, want);
    }
}

#[test]
fn recovery_flag_beats_variable_beats_default() {
    let env = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", "off")]);
    assert_eq!(
        resolve(&flags(), None, &env).unwrap().recovery,
        Recovery::Off
    );
    assert_eq!(
        resolve(&recovery_flag("log"), None, &env).unwrap().recovery,
        Recovery::Log
    );
    // A valid flag wins even over an invalid variable.
    let bad = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", "bogus")]);
    assert_eq!(
        resolve(&recovery_flag("act"), None, &bad).unwrap().recovery,
        Recovery::Act
    );
}

#[test]
fn an_empty_recovery_variable_counts_as_unset() {
    let env = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", "")]);
    assert_eq!(
        resolve(&flags(), None, &env).unwrap().recovery,
        Recovery::Act
    );
}

#[test]
fn an_invalid_recovery_flag_is_named_and_never_falls_back_to_act() {
    for bad in [
        "LOG", "Log", "ACT", " log", "log ", "", "lo", "logs", "on", "1", "bogus",
    ] {
        let e = err(resolve(&recovery_flag(bad), None, &no_env()));
        assert!(e.contains("--recovery"), "{bad:?}: {e}");
        assert!(e.contains("expected act, log or off"), "{bad:?}: {e}");
        assert!(!e.contains("AMPLIHACK_ROTATE_RECOVERY"), "{bad:?}: {e}");
    }
}

#[test]
fn an_invalid_recovery_variable_is_named() {
    for bad in ["bogus", "LOG", " off", "act\n"] {
        let env = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", bad)]);
        let e = err(resolve(&flags(), None, &env));
        assert!(e.contains("AMPLIHACK_ROTATE_RECOVERY"), "{bad:?}: {e}");
        assert!(e.contains("expected act, log or off"), "{bad:?}: {e}");
    }
}

#[test]
fn the_documented_recovery_message_shape() {
    let env = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", "bogus")]);
    assert_eq!(
        err(resolve(&flags(), None, &env)),
        "invalid rotate setting AMPLIHACK_ROTATE_RECOVERY \"bogus\": expected act, log or off"
    );
    assert_eq!(
        err(resolve(&recovery_flag("LOG"), None, &no_env())),
        "invalid rotate setting --recovery \"LOG\": expected act, log or off"
    );
}

#[test]
fn a_bad_recovery_value_is_escaped_and_cut() {
    let long = format!("\x1b[31m{}", "x".repeat(80));
    let e = err(resolve(&recovery_flag(&long), None, &no_env()));
    assert!(!e.contains('\x1b'), "control characters escaped: {e:?}");
    assert!(
        !e.contains(&"x".repeat(40)),
        "value cut to 32 characters: {e}"
    );
}

#[cfg(unix)]
#[test]
fn a_non_utf8_recovery_variable_is_rejected() {
    use std::os::unix::ffi::OsStringExt;
    let raw = OsString::from_vec(vec![b'l', 0xff, b'g']);
    let env = move |k: &str| (k == "AMPLIHACK_ROTATE_RECOVERY").then(|| raw.clone());
    let e = err(resolve(&flags(), None, &env));
    assert!(e.contains("AMPLIHACK_ROTATE_RECOVERY"), "{e}");
}

#[test]
fn to_args_passes_recovery_only_when_not_act() {
    let act = resolve(&recovery_flag("act"), None, &no_env()).unwrap();
    assert!(!act.to_args(true).iter().any(|a| a == "--recovery"));
    for word in ["log", "off"] {
        let args = resolve(&recovery_flag(word), None, &no_env())
            .unwrap()
            .to_args(false);
        let i = args
            .iter()
            .position(|a| a == "--recovery")
            .expect("--recovery in to_args");
        assert_eq!(args[i + 1], word);
    }
}

#[test]
fn the_overflow_gate_ignores_the_recovery_variable() {
    let env = env_of(&[("AMPLIHACK_ROTATE_RECOVERY", "bogus")]);
    assert!(super::settings::gate_from_env(&env).is_ok());
}

#[test]
fn rotate_help_lists_recovery() {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let mut out = Vec::new();
    let rotate = cmd
        .find_subcommand_mut("profile")
        .and_then(|p| p.find_subcommand_mut("rotate"))
        .expect("profile rotate");
    rotate.write_long_help(&mut out).unwrap();
    let help = String::from_utf8(out).unwrap();
    assert!(help.contains("--recovery"), "{help}");
}
