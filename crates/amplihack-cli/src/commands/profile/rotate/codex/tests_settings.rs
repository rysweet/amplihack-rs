//! Codex rotate settings (issue #2005).
//!
//! Contract:
//! - `TuneFlags` gains `codex`, `codex_floor` and `codex_home_link`, each
//!   `Option<String>`.
//! - `settings::Settings` gains `codex: bool` (default on; `on`/`off`/`1`/
//!   `0`) and `codex_floor: u32` (default 3000, `0` or more) and stays
//!   `Copy + Eq`. `Settings::to_args` passes both as flags.
//! - `codex::settings::resolve_codex(flag: Option<&str>, env: &dyn
//!   Fn(&str) -> Option<OsString>, home: &Path) -> Result<CodexSettings,
//!   ProfileError>`, `CodexSettings { link: PathBuf }`, default
//!   `<home>/.codex`; it must be absolute with no `..` component.

use super::super::TuneFlags;
use super::super::settings::resolve;
use super::settings::resolve_codex;
use crate::commands::profile::ProfileError;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

fn config_err<T: std::fmt::Debug>(r: Result<T, ProfileError>) -> String {
    match r {
        Err(e @ ProfileError::RotateConfig(_)) => e.to_string(),
        other => panic!("expected RotateConfig, got {other:?}"),
    }
}

#[test]
fn codex_defaults_to_on_with_floor_3000() {
    let s = resolve(&TuneFlags::default(), None, &env_of(&[])).unwrap();
    assert!(s.codex);
    assert_eq!(s.codex_floor, 3000);
}

#[test]
fn codex_accepts_on_off_1_and_0_and_the_flag_wins() {
    for (raw, want) in [("on", true), ("off", false), ("1", true), ("0", false)] {
        let t = TuneFlags {
            codex: Some(raw.into()),
            ..TuneFlags::default()
        };
        assert_eq!(
            resolve(&t, None, &env_of(&[])).unwrap().codex,
            want,
            "{raw}"
        );
        let s = resolve(
            &TuneFlags::default(),
            None,
            &env_of(&[("AMPLIHACK_ROTATE_CODEX", raw)]),
        )
        .unwrap();
        assert_eq!(s.codex, want, "variable {raw}");
    }
    let t = TuneFlags {
        codex: Some("on".into()),
        ..TuneFlags::default()
    };
    let s = resolve(&t, None, &env_of(&[("AMPLIHACK_ROTATE_CODEX", "off")])).unwrap();
    assert!(s.codex);
    let s = resolve(
        &TuneFlags::default(),
        None,
        &env_of(&[("AMPLIHACK_ROTATE_CODEX", "")]),
    )
    .unwrap();
    assert!(s.codex, "empty is unset");
}

#[test]
fn bad_codex_values_name_the_flag_or_variable() {
    let t = TuneFlags {
        codex: Some("yes".into()),
        ..TuneFlags::default()
    };
    assert!(config_err(resolve(&t, None, &env_of(&[]))).contains("--codex \"yes\""));
    let e = config_err(resolve(
        &TuneFlags::default(),
        None,
        &env_of(&[("AMPLIHACK_ROTATE_CODEX", "2")]),
    ));
    assert!(e.contains("AMPLIHACK_ROTATE_CODEX \"2\""), "{e}");
}

#[test]
fn the_floor_is_a_whole_number_from_zero() {
    let t = TuneFlags {
        codex_floor: Some("0".into()),
        ..TuneFlags::default()
    };
    assert_eq!(resolve(&t, None, &env_of(&[])).unwrap().codex_floor, 0);
    let s = resolve(
        &TuneFlags::default(),
        None,
        &env_of(&[("AMPLIHACK_ROTATE_CODEX_FLOOR", "5000")]),
    )
    .unwrap();
    assert_eq!(s.codex_floor, 5000);
    for bad in ["-1", "1.5", "x", "99999999999"] {
        let t = TuneFlags {
            codex_floor: Some(bad.into()),
            ..TuneFlags::default()
        };
        assert!(
            config_err(resolve(&t, None, &env_of(&[]))).contains("--codex-floor"),
            "{bad}"
        );
    }
}

#[test]
fn to_args_passes_the_codex_settings_as_flags() {
    let t = TuneFlags {
        codex: Some("off".into()),
        codex_floor: Some("42".into()),
        ..TuneFlags::default()
    };
    let args = resolve(&t, None, &env_of(&[])).unwrap().to_args(false);
    let pair = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .map(|i| args[i + 1].clone())
    };
    assert_eq!(pair("--codex").as_deref(), Some("off"));
    assert_eq!(pair("--codex-floor").as_deref(), Some("42"));
}

#[test]
fn the_link_defaults_to_home_dot_codex() {
    let s = resolve_codex(None, &env_of(&[]), Path::new("/home/u")).unwrap();
    assert_eq!(s.link, PathBuf::from("/home/u/.codex"));
    let s = resolve_codex(
        None,
        &env_of(&[("AMPLIHACK_ROTATE_CODEX_HOME_LINK", "")]),
        Path::new("/home/u"),
    )
    .unwrap();
    assert_eq!(s.link, PathBuf::from("/home/u/.codex"), "empty is unset");
}

#[test]
fn the_link_flag_beats_the_variable() {
    let env = env_of(&[("AMPLIHACK_ROTATE_CODEX_HOME_LINK", "/v/link")]);
    assert_eq!(
        resolve_codex(None, &env, Path::new("/h")).unwrap().link,
        PathBuf::from("/v/link")
    );
    assert_eq!(
        resolve_codex(Some("/f/link"), &env, Path::new("/h"))
            .unwrap()
            .link,
        PathBuf::from("/f/link")
    );
}

#[test]
fn a_relative_or_dotdot_link_is_refused_naming_its_source() {
    let e = config_err(resolve_codex(Some("codex"), &env_of(&[]), Path::new("/h")));
    assert!(
        e.contains("invalid rotate setting --codex-home-link \"codex\": expected an absolute path with no .. component"),
        "{e}"
    );
    config_err(resolve_codex(
        Some("/a/../b"),
        &env_of(&[]),
        Path::new("/h"),
    ));
    let e = config_err(resolve_codex(
        None,
        &env_of(&[("AMPLIHACK_ROTATE_CODEX_HOME_LINK", "x/y")]),
        Path::new("/h"),
    ));
    assert!(e.contains("AMPLIHACK_ROTATE_CODEX_HOME_LINK"), "{e}");
}
