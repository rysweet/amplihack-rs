//! Issue #1811 (TDD): kill switch, governor.* keys and the wait grammar
//! (D1, D10).
//!
//! Wire with `#[cfg(test)] mod tests_config;` in `governor/mod.rs`.
//!
//! Expected API in `config`:
//! - `Settings { enabled: bool, max_concurrency: u32, reserve_points: u32,
//!   step: u32, band_pct: u32, eval_interval_secs: u64, wait_secs: u64 }`,
//!   `Default` with the documented defaults.
//! - `resolve(env: &dyn Fn(&str) -> Option<String>, user: &dyn Fn(&str) ->
//!   Option<Value>) -> Settings`, where `user` returns the user-scope value
//!   of a `governor.*` key (`user_config::schema::Value`).
//! - `parse_wait(&str) -> Result<u64, String>`: `^[0-9]{1,6}(s|m|h)?$`,
//!   seconds, capped at 24 h.
//! - `wait_secs(flag: Option<u64>, env, &Settings) -> u64`: flag, then
//!   `AMPLIHACK_GOVERNOR_WAIT`, then the setting.
//! - `GOVERNOR_POLL_SECS == 15`.

use super::config::{GOVERNOR_POLL_SECS, Settings, parse_wait, resolve, wait_secs};
use crate::user_config::schema::Value;
use std::collections::BTreeMap;

type Map = BTreeMap<String, String>;

fn env(pairs: &[(&str, &str)]) -> Map {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn user(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn run(e: &Map, u: &BTreeMap<String, Value>) -> Settings {
    resolve(&|k: &str| e.get(k).cloned(), &|k: &str| u.get(k).cloned())
}

#[test]
fn defaults_match_the_documentation() {
    let s = Settings::default();
    assert!(s.enabled);
    assert_eq!(
        (s.max_concurrency, s.reserve_points, s.step, s.band_pct),
        (4, 10, 1, 10)
    );
    assert_eq!((s.eval_interval_secs, s.wait_secs), (60, 600));
    assert_eq!(GOVERNOR_POLL_SECS, 15);
    assert_eq!(run(&env(&[]), &user(&[])), s);
}

#[test]
fn kill_switch_values() {
    for off in ["off", "0", "false", "OFF", "False"] {
        assert!(
            !run(&env(&[("AMPLIHACK_GOVERNOR", off)]), &user(&[])).enabled,
            "{off}"
        );
    }
    for on in ["on", "1", "true"] {
        let u = user(&[("governor.enabled", Value::Bool(false))]);
        assert!(run(&env(&[("AMPLIHACK_GOVERNOR", on)]), &u).enabled, "{on}");
    }
}

#[test]
fn env_outranks_config_and_config_applies_without_env() {
    let u = user(&[("governor.enabled", Value::Bool(false))]);
    assert!(!run(&env(&[]), &u).enabled);
    let u = user(&[("governor.enabled", Value::Bool(true))]);
    assert!(!run(&env(&[("AMPLIHACK_GOVERNOR", "off")]), &u).enabled);
}

#[test]
fn unknown_kill_switch_value_is_ignored() {
    let u = user(&[("governor.enabled", Value::Bool(false))]);
    assert!(!run(&env(&[("AMPLIHACK_GOVERNOR", "maybe")]), &u).enabled);
    assert!(run(&env(&[("AMPLIHACK_GOVERNOR", "maybe")]), &user(&[])).enabled);
}

#[test]
fn tunables_come_from_user_config() {
    let u = user(&[
        ("governor.max_concurrency", Value::Int(2)),
        ("governor.reserve_points", Value::Int(15)),
        ("governor.step", Value::Int(2)),
        ("governor.band_pct", Value::Int(20)),
        ("governor.eval_interval_secs", Value::Int(30)),
        ("governor.wait_secs", Value::Int(0)),
    ]);
    let s = run(&env(&[]), &u);
    assert_eq!(
        (
            s.max_concurrency,
            s.reserve_points,
            s.step,
            s.band_pct,
            s.eval_interval_secs,
            s.wait_secs
        ),
        (2, 15, 2, 20, 30, 0)
    );
}

#[test]
fn out_of_range_values_are_clamped_again() {
    let u = user(&[
        ("governor.max_concurrency", Value::Int(0)),
        ("governor.reserve_points", Value::Int(1000)),
        ("governor.step", Value::Int(99)),
        ("governor.band_pct", Value::Int(90)),
        ("governor.eval_interval_secs", Value::Int(0)),
        ("governor.wait_secs", Value::Int(u64::MAX)),
    ]);
    let s = run(&env(&[]), &u);
    assert_eq!(
        (
            s.max_concurrency,
            s.reserve_points,
            s.step,
            s.band_pct,
            s.eval_interval_secs,
            s.wait_secs
        ),
        (1, 100, 8, 50, 1, 86_400)
    );
}

#[test]
fn wrong_value_types_fall_back_to_defaults() {
    let u = user(&[
        ("governor.max_concurrency", Value::Text("lots".into())),
        ("governor.enabled", Value::Int(0)),
    ]);
    assert_eq!(run(&env(&[]), &u), Settings::default());
}

#[test]
fn parse_wait_grammar() {
    for (raw, secs) in [
        ("0", 0),
        ("90", 90),
        ("90s", 90),
        ("15m", 900),
        ("2h", 7200),
        ("999999", 86_400),
        ("999999h", 86_400),
        ("24h", 86_400),
    ] {
        assert_eq!(parse_wait(raw), Ok(secs), "{raw}");
    }
}

#[test]
fn parse_wait_rejects_other_forms() {
    for raw in [
        "", "s", "-1", "1d", "1.5m", " 5", "5 ", "1234567", "10M", "1h30m", "+5", "٣",
    ] {
        assert!(parse_wait(raw).is_err(), "{raw:?}");
    }
}

#[test]
fn wait_precedence_is_flag_then_env_then_setting() {
    let s = Settings {
        wait_secs: 300,
        ..Settings::default()
    };
    let e = env(&[("AMPLIHACK_GOVERNOR_WAIT", "2m")]);
    let get = |k: &str| e.get(k).cloned();
    assert_eq!(wait_secs(Some(5), &get, &s), 5);
    assert_eq!(wait_secs(None, &get, &s), 120);
    let none = |_: &str| None;
    assert_eq!(wait_secs(None, &none, &s), 300);
}

#[test]
fn invalid_wait_env_falls_back_to_the_setting() {
    let s = Settings::default();
    let e = env(&[("AMPLIHACK_GOVERNOR_WAIT", "soon")]);
    assert_eq!(wait_secs(None, &|k: &str| e.get(k).cloned(), &s), 600);
}

/// P1 (PR 1831): a wrong type and a clamp each give a warning that names
/// the key, and the value still resolves as before.
#[test]
fn wrong_type_and_clamp_each_give_a_warning_naming_the_key() {
    use super::config::{bool_setting, int_setting};
    let (v, w) = int_setting(
        Some(Value::Text("SECRETVALUE".into())),
        "governor.max_concurrency",
        1,
        64,
        4,
    );
    assert_eq!(v, 4);
    let w = w.expect("a wrong type warns");
    assert!(
        w.contains("governor.max_concurrency is not an integer"),
        "{w}"
    );
    assert!(
        !w.contains("SECRETVALUE"),
        "a wrong-typed value is quoted: {w}"
    );

    let (v, w) = int_setting(Some(Value::Int(500)), "governor.reserve_points", 0, 100, 10);
    assert_eq!(v, 100);
    assert_eq!(
        w.as_deref(),
        Some("governor.reserve_points is 500, outside 0..=100; using 100")
    );

    let (v, w) = bool_setting(Some(Value::Int(1)), "governor.enabled");
    assert_eq!(v, None);
    assert!(w.unwrap().contains("governor.enabled is not a boolean"));
}

#[test]
fn unset_and_in_range_values_give_no_warning() {
    use super::config::{bool_setting, int_setting};
    assert_eq!(int_setting(None, "governor.step", 1, 8, 1), (1, None));
    assert_eq!(
        int_setting(Some(Value::Int(2)), "governor.step", 1, 8, 1),
        (2, None)
    );
    assert_eq!(
        bool_setting(Some(Value::Bool(false)), "governor.enabled"),
        (Some(false), None)
    );
    assert_eq!(bool_setting(None, "governor.enabled"), (None, None));
}

/// `recipe run --detach` hands the flags to the pane's run; they must parse
/// back to the same values.
#[test]
fn governor_args_round_trip_through_the_command_line() {
    #[derive(clap::Parser)]
    struct Probe {
        #[command(flatten)]
        governor: super::GovernorArgs,
    }
    let args = super::GovernorArgs {
        workstream: Some("ws-7".into()),
        governor_class: Some(super::Class::Review),
        governor_wait: Some(90),
    };
    let argv = std::iter::once("probe".to_string()).chain(args.to_args());
    let back = <Probe as clap::Parser>::parse_from(argv).governor;
    assert_eq!(back.workstream.as_deref(), Some("ws-7"));
    assert_eq!(back.governor_class, Some(super::Class::Review));
    assert_eq!(back.governor_wait, Some(90));
    assert!(super::GovernorArgs::default().to_args().is_empty());
}
