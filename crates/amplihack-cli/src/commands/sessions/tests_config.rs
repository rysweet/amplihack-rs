//! `sessions::config` (issue #1746): flag, then variable, then default.
//!
//! Contract:
//! - `resolve(days: Option<u32>, dest: Option<PathBuf>, apply: bool,
//!   env: &dyn Fn(&str) -> Option<OsString>, home: Option<PathBuf>)
//!   -> Result<Opts, ConfigError>`.
//! - `Opts { home, days, dest, apply }`; `dest` is always absolute.
//! - Variables: `AMPLIHACK_SESSIONS_ARCHIVE_DAYS`,
//!   `AMPLIHACK_SESSIONS_ARCHIVE_DEST`; empty counts as unset.
//! - Defaults: 3 days, `<HOME>/.amplihack/session-archive`.
//! - `ConfigError::{NoHome, BadDays}` display fixed lines that never repeat
//!   the value.
//! - `resolve` reads the environment only through `env`.

use super::config::{ConfigError, resolve};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

const DAYS: &str = "AMPLIHACK_SESSIONS_ARCHIVE_DAYS";
const DEST: &str = "AMPLIHACK_SESSIONS_ARCHIVE_DEST";

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

fn home() -> Option<PathBuf> {
    Some(PathBuf::from("/home/u"))
}

#[test]
fn defaults() {
    let o = resolve(None, None, false, &env(&[]), home()).unwrap();
    assert_eq!(o.days, 3);
    assert_eq!(o.dest, PathBuf::from("/home/u/.amplihack/session-archive"));
    assert_eq!(o.home, PathBuf::from("/home/u"));
    assert!(!o.apply);
}

#[test]
fn variable_overrides_default_and_flag_overrides_variable() {
    let e = env(&[(DAYS, "9"), (DEST, "/var/a")]);
    let o = resolve(None, None, true, &e, home()).unwrap();
    assert_eq!(
        (o.days, o.dest.clone(), o.apply),
        (9, PathBuf::from("/var/a"), true)
    );
    let o = resolve(Some(2), Some(PathBuf::from("/var/b")), false, &e, home()).unwrap();
    assert_eq!((o.days, o.dest), (2, PathBuf::from("/var/b")));
}

#[test]
fn empty_variable_counts_as_unset() {
    let o = resolve(None, None, false, &env(&[(DAYS, ""), (DEST, "")]), home()).unwrap();
    assert_eq!(o.days, 3);
    assert_eq!(o.dest, PathBuf::from("/home/u/.amplihack/session-archive"));
}

#[test]
fn invalid_days_variable_is_an_error_that_does_not_echo_it() {
    for bad in [
        "0",
        "-1",
        "abc",
        "1.5",
        " 3",
        "99999999999999999999",
        "SECRET-VALUE",
    ] {
        let err = resolve(None, None, false, &env(&[(DAYS, bad)]), home()).unwrap_err();
        assert!(matches!(err, ConfigError::BadDays), "{bad}: {err:?}");
        let msg = err.to_string();
        assert_eq!(
            msg,
            "AMPLIHACK_SESSIONS_ARCHIVE_DAYS must be a whole number of at least 1"
        );
        assert!(!msg.contains("SECRET"));
    }
}

#[test]
fn flag_wins_so_an_invalid_variable_is_never_parsed() {
    // The flag wins, so the variable is never parsed.
    let o = resolve(Some(5), None, false, &env(&[(DAYS, "junk")]), home()).unwrap();
    assert_eq!(o.days, 5);
}

#[test]
fn no_home_is_an_error() {
    let err = resolve(None, None, false, &env(&[]), None).unwrap_err();
    assert!(matches!(err, ConfigError::NoHome));
    assert_eq!(err.to_string(), "HOME is not set");
}

#[test]
fn relative_dest_is_made_absolute() {
    let o = resolve(
        None,
        Some(PathBuf::from("rel/dir")),
        false,
        &env(&[]),
        home(),
    )
    .unwrap();
    assert!(o.dest.is_absolute(), "{}", o.dest.display());
    assert!(o.dest.ends_with("rel/dir"));
    let o = resolve(None, None, false, &env(&[(DEST, "rel2")]), home()).unwrap();
    assert!(o.dest.is_absolute());
}

#[test]
fn only_the_injected_env_is_read() {
    // A key the resolver asks for that is not one of the two variables would
    // show up here.
    let asked = std::cell::RefCell::new(Vec::<String>::new());
    let e = |k: &str| {
        asked.borrow_mut().push(k.to_string());
        None
    };
    resolve(None, None, false, &e, home()).unwrap();
    for k in asked.borrow().iter() {
        assert!(k == DAYS || k == DEST, "read {k}");
    }
}
