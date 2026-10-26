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
use super::settings::{Settings, resolve};
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
        ..flags()
    };
    let s = resolve(&f, Some("42"), &env).unwrap();
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
            "--interval" => interval = v,
            other => panic!("unexpected flag {other}"),
        }
    }
    let again = resolve(&back, interval.as_deref(), &no_env()).unwrap();
    assert_eq!(again, s);
    assert!(!s.to_args(false).iter().any(|a| a == "--interval"));
}
