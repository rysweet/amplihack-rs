//! Issue #2006: the pure parts of the launch integration.
//!
//! Wired from `account/mod.rs` as `#[cfg(all(test, unix))] mod tests_launch;`.
//!
//! Contract (docs/reference/account-pick.md, "Pins" and "Pins across
//! recipe steps"). Each helper takes an injected variable reader
//! `read: impl Fn(&str) -> Option<OsString>`, so no test changes the
//! process environment:
//!
//! - `launch::codex_pin(read) -> Option<PathBuf>`: `CODEX_HOME` when set and
//!   non-empty, else `AMPLIHACK_CODEX_HOME` when it is a non-empty absolute
//!   path, else `None`.
//! - `launch::claude_config_pinned(read) -> bool`: `CLAUDE_CONFIG_DIR` is
//!   set and non-empty, or `AMPLIHACK_CLAUDE_CONFIG_DIR` is a non-empty
//!   absolute path.
//! - `launch::claude_config_from_alias(read) -> Option<PathBuf>`:
//!   `AMPLIHACK_CLAUDE_CONFIG_DIR` when `CLAUDE_CONFIG_DIR` is unset or
//!   empty and the alias is a non-empty absolute path, else `None`.
//! - `launch::recipe_aliases(read) -> Vec<(&'static str, OsString)>`: what
//!   `amplihack recipe run` sets on the runner's process:
//!   `AMPLIHACK_CODEX_HOME` from `CODEX_HOME` and
//!   `AMPLIHACK_CLAUDE_CONFIG_DIR` from `CLAUDE_CONFIG_DIR`, each only when
//!   the source is set and non-empty and the alias is unset or empty.
//! - `launch::LeaseHandle::new(path)`: dropping it removes exactly `path`;
//!   a missing file is not an error.

use super::launch::{self, LeaseHandle};
use super::test_fixtures::private_tempdir;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

fn reader(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), OsString::from(*v)))
        .collect();
    move |k| map.get(k).cloned()
}

#[test]
fn codex_home_is_the_first_pin() {
    let r = reader(&[("CODEX_HOME", "/x"), ("AMPLIHACK_CODEX_HOME", "/y")]);
    assert_eq!(launch::codex_pin(&r), Some(PathBuf::from("/x")));
}

#[test]
fn the_codex_alias_pins_when_codex_home_is_unset_or_empty() {
    let r = reader(&[("AMPLIHACK_CODEX_HOME", "/y")]);
    assert_eq!(launch::codex_pin(&r), Some(PathBuf::from("/y")));
    let r = reader(&[("CODEX_HOME", ""), ("AMPLIHACK_CODEX_HOME", "/y")]);
    assert_eq!(launch::codex_pin(&r), Some(PathBuf::from("/y")));
}

#[test]
fn a_relative_or_empty_codex_alias_is_no_pin() {
    assert_eq!(
        launch::codex_pin(&reader(&[("AMPLIHACK_CODEX_HOME", "rel")])),
        None
    );
    assert_eq!(
        launch::codex_pin(&reader(&[("AMPLIHACK_CODEX_HOME", "")])),
        None
    );
    assert_eq!(launch::codex_pin(&reader(&[])), None);
}

#[test]
fn a_relative_codex_home_is_still_the_users_choice() {
    // CODEX_HOME is the user's own variable: it passes through unchanged.
    let r = reader(&[("CODEX_HOME", "rel/home")]);
    assert_eq!(launch::codex_pin(&r), Some(PathBuf::from("rel/home")));
}

#[test]
fn claude_config_pins() {
    assert!(launch::claude_config_pinned(&reader(&[(
        "CLAUDE_CONFIG_DIR",
        "/c"
    )])));
    assert!(launch::claude_config_pinned(&reader(&[(
        "AMPLIHACK_CLAUDE_CONFIG_DIR",
        "/c"
    )])));
    assert!(!launch::claude_config_pinned(&reader(&[(
        "CLAUDE_CONFIG_DIR",
        ""
    )])));
    assert!(!launch::claude_config_pinned(&reader(&[])));
}

#[test]
fn recipe_aliases_copy_raw_homes_when_the_alias_is_unset() {
    let r = reader(&[("CODEX_HOME", "/cx"), ("CLAUDE_CONFIG_DIR", "/cl")]);
    let mut got = launch::recipe_aliases(&r);
    got.sort();
    assert_eq!(
        got,
        vec![
            ("AMPLIHACK_CLAUDE_CONFIG_DIR", OsString::from("/cl")),
            ("AMPLIHACK_CODEX_HOME", OsString::from("/cx")),
        ]
    );
}

#[test]
fn recipe_aliases_never_override_an_existing_alias() {
    let r = reader(&[
        ("CODEX_HOME", "/cx"),
        ("AMPLIHACK_CODEX_HOME", "/kept"),
        ("CLAUDE_CONFIG_DIR", "/cl"),
        ("AMPLIHACK_CLAUDE_CONFIG_DIR", "/kept2"),
    ]);
    assert!(launch::recipe_aliases(&r).is_empty());
}

#[test]
fn recipe_aliases_skip_empty_sources() {
    let r = reader(&[("CODEX_HOME", ""), ("CLAUDE_CONFIG_DIR", "")]);
    assert!(launch::recipe_aliases(&r).is_empty());
    assert!(launch::recipe_aliases(&reader(&[])).is_empty());
}

#[test]
fn recipe_aliases_fill_an_empty_alias() {
    let r = reader(&[("CODEX_HOME", "/cx"), ("AMPLIHACK_CODEX_HOME", "")]);
    assert_eq!(
        launch::recipe_aliases(&r),
        vec![("AMPLIHACK_CODEX_HOME", OsString::from("/cx"))]
    );
}

#[test]
fn dropping_a_lease_handle_removes_exactly_its_file() {
    let tmp = private_tempdir();
    let mine = tmp.path().join("1-2-3.json");
    let other = tmp.path().join("1-2-4.json");
    fs::write(&mine, b"{}").unwrap();
    fs::write(&other, b"{}").unwrap();
    drop(LeaseHandle::new(mine.clone()));
    assert!(!mine.exists());
    assert!(other.exists());
    // Already gone: dropping again must not panic.
    drop(LeaseHandle::new(mine));
}

#[test]
fn a_relative_claude_alias_is_ignored() {
    let r = reader(&[("AMPLIHACK_CLAUDE_CONFIG_DIR", "rel/dir")]);
    assert_eq!(launch::claude_config_from_alias(&r), None);
    assert!(!launch::claude_config_pinned(&r));
}

#[test]
fn an_absolute_claude_alias_is_restored() {
    let r = reader(&[("AMPLIHACK_CLAUDE_CONFIG_DIR", "/abs/dir")]);
    assert_eq!(
        launch::claude_config_from_alias(&r),
        Some(PathBuf::from("/abs/dir"))
    );
    assert!(launch::claude_config_pinned(&r));
    let r = reader(&[
        ("CLAUDE_CONFIG_DIR", "/raw"),
        ("AMPLIHACK_CLAUDE_CONFIG_DIR", "/abs/dir"),
    ]);
    assert_eq!(launch::claude_config_from_alias(&r), None);
}
