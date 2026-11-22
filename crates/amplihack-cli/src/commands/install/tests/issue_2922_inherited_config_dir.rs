//! Issue #2922 (TDD): an inherited `CLAUDE_CONFIG_DIR` must never receive a
//! test fixture's settings.
//!
//! The stand-in "real" profile is exported first, which is what a test run
//! inside an agent launch inherits. A test then takes the shared isolated
//! home and runs the writers. The stand-in must be unchanged byte for byte
//! and the temporary home's own `settings.json` must be written.
//!
//! Fixture content is synthetic. A failure names changed paths only, never
//! file bytes.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::snapshot;
use super::*;
use crate::test_support::claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS;
use crate::test_support::{EnvGuard, HomeGuard, env_lock, tempdir_outside_framework_roots};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const STAND_IN_SETTINGS: &str =
    r#"{"model":"stand-in-model","env":{"PLACEHOLDER":"not-a-secret"},"hooks":{}}"#;

fn changed_paths(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .cloned()
        .collect()
}

struct Setup {
    _scratch: tempfile::TempDir,
    _bin: tempfile::TempDir,
    home: PathBuf,
    stand_in: PathBuf,
    hooks_bin: PathBuf,
    codex_bin: PathBuf,
    codex_home: PathBuf,
}

fn setup() -> Setup {
    let scratch = tempdir_outside_framework_roots();
    let bin = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    let stand_in = scratch.path().join("real-profile");
    let codex_home = home.join(".codex");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&stand_in).unwrap();
    fs::write(stand_in.join("settings.json"), STAND_IN_SETTINGS).unwrap();
    // Abort on setup failure, before any writer runs.
    assert!(stand_in.is_absolute() && stand_in.is_dir());
    assert!(stand_in.starts_with(scratch.path()));
    let stubs = bin.path().join("stub_bin");
    let hooks_bin = create_exe_stub(&stubs, "amplihack-hooks");
    // This host has a real `codex` on PATH. The Codex provider must reach a
    // stub, not that binary, and must keep its own state inside the temporary
    // home (`CODEX_HOME`), or the run leaves the sandbox the test built.
    let codex_bin = create_exe_stub(&stubs, "codex");
    Setup {
        _scratch: scratch,
        _bin: bin,
        home,
        stand_in,
        hooks_bin,
        codex_bin,
        codex_home,
    }
}

fn run_writers(s: &Setup, provider: &str) {
    create_minimal_staged_assets(&s.home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    let mut notice = Vec::new();
    crate::freshness::with_provider(provider, || {
        ensure_framework_installed_with(&mut notice).expect("the launch writers must succeed")
    });
}

fn check(provider: &str) {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = setup();
    // The inherited environment: exported before the helper is taken.
    let mut inherited = EnvGuard::capture(&CLAUDE_PROFILE_ENV_VARS);
    inherited.assign("CLAUDE_CONFIG_DIR", &s.stand_in);
    inherited.assign("AMPLIHACK_CLAUDE_PROFILE", "stand-in-profile");
    let _bin = EnvGuard::set([
        (
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
            s.hooks_bin.to_str().unwrap(),
        ),
        ("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1"),
        ("AMPLIHACK_SKIP_MMDC", "1"),
        ("AMPLIHACK_CODEX_BINARY_PATH", s.codex_bin.to_str().unwrap()),
        ("CODEX_HOME", s.codex_home.to_str().unwrap()),
    ]);
    let before = snapshot(&s.stand_in);
    {
        let _home = HomeGuard::set(&s.home);
        run_writers(&s, provider);
        let written = s.home.join(".claude/settings.json");
        assert!(
            written.is_file(),
            "{provider}: the temporary home's settings.json was not written, \
             so this test would pass vacuously"
        );
        let text = fs::read_to_string(&written).unwrap();
        assert!(
            !text.contains("stand-in-model"),
            "{provider}: the temporary home picked up stand-in content"
        );
    }
    let after = snapshot(&s.stand_in);
    assert_eq!(
        changed_paths(&before, &after),
        Vec::<PathBuf>::new(),
        "{provider}: the inherited CLAUDE_CONFIG_DIR profile changed"
    );
    assert_eq!(
        fs::read(s.stand_in.join("settings.json")).unwrap(),
        STAND_IN_SETTINGS.as_bytes(),
        "{provider}: settings.json of the stand-in is not byte-identical"
    );
}

#[test]
fn the_claude_provider_leaves_an_inherited_profile_byte_identical() {
    check("claude");
}

#[test]
fn the_codex_provider_leaves_an_inherited_profile_byte_identical() {
    check("codex");
}

#[test]
fn the_inherited_variables_are_back_after_the_home_guard_drops() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = setup();
    let mut inherited = EnvGuard::capture(&CLAUDE_PROFILE_ENV_VARS);
    inherited.assign("CLAUDE_CONFIG_DIR", &s.stand_in);
    {
        let _home = HomeGuard::set(&s.home);
        assert_eq!(std::env::var_os("CLAUDE_CONFIG_DIR"), None);
    }
    assert_eq!(
        std::env::var_os("CLAUDE_CONFIG_DIR").as_deref(),
        Some(Path::new(&s.stand_in).as_os_str())
    );
}
