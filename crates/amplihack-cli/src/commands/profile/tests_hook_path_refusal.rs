//! Issue #1699: a test that ran `amplihack install` with a temp HOME and an
//! inherited `AMPLIHACK_CLAUDE_PROFILE_ROOT` wrote the temp HOME's
//! `amplihack-hooks` path into the user's real profiles. The refresh now
//! refuses, before touching any profile file, a hook binary that does not
//! exist, or that sits under a temp dir or HOME while the root is outside
//! HOME. Every test uses temp dirs; tests that change the environment hold
//! `env_lock()` and restore it on drop, including on panic.

use super::ROOT_ENV;
use super::settings_refresh::{
    Refresh, default_temp_dirs, hook_path_refusal, refresh_in, refresh_in_checked,
    refresh_profile_hooks_checked, temp_dirs_from,
};
use super::store::ProfileStore;
use crate::test_support::{EnvGuard, env_lock};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const STALE_BIN: &str = "/opt/old/amplihack-hooks";

/// A temp tree with a "real" profile root outside its HOME:
/// `home/.local/bin/amplihack-hooks` and `real/profiles/{a,b}/settings.json`.
struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let w = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(w.home()).unwrap();
        stub(&w.bin());
        for name in ["a", "b"] {
            profile(&w.real_root(), name, Some(&old_settings()));
        }
        w
    }
    fn base(&self) -> &Path {
        self.dir.path()
    }
    fn home(&self) -> PathBuf {
        self.base().join("home")
    }
    fn bin(&self) -> PathBuf {
        self.home().join(".local/bin/amplihack-hooks")
    }
    fn real_root(&self) -> PathBuf {
        self.base().join("real/profiles")
    }
    fn home_root(&self) -> PathBuf {
        self.home().join(".claude-profiles")
    }
}

fn stub(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn profile(root: &Path, name: &str, settings: Option<&str>) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    if let Some(text) = settings {
        fs::write(dir.join("settings.json"), text).unwrap();
        fs::set_permissions(dir.join("settings.json"), fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn old_settings() -> String {
    json!({
        "language": "en",
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": format!("{STALE_BIN} stop")}]}]},
    })
    .to_string()
}

/// Stands in for install's hook update: points the Stop hook at `bin`.
fn point_hooks_at(bin: &Path) -> impl Fn(&mut Value) + '_ {
    move |settings| {
        settings["hooks"]["Stop"] = json!([{
            "hooks": [{"type": "command", "command": format!("{} stop", bin.display())}]
        }]);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    kind: &'static str,
    len: u64,
    sha256: Option<String>,
    link: Option<PathBuf>,
    mtime: SystemTime,
}

/// Every entry under `root`, never following symlinks: kind, length,
/// contents hash, link target and mtime. Directories are included so a
/// temp, lock or backup file that came and went still changes their mtime.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn walk(path: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
        let meta = fs::symlink_metadata(path).unwrap();
        let ft = meta.file_type();
        let (kind, sha256, link) = if ft.is_symlink() {
            ("link", None, Some(fs::read_link(path).unwrap()))
        } else if ft.is_file() {
            let hash = Sha256::digest(fs::read(path).unwrap());
            ("file", Some(format!("{hash:x}")), None)
        } else {
            ("dir", None, None)
        };
        out.insert(
            path.to_path_buf(),
            Entry {
                kind,
                len: meta.len(),
                sha256,
                link,
                mtime: meta.modified().unwrap(),
            },
        );
        if ft.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(&entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

fn names(results: &[(String, Refresh)]) -> Vec<&str> {
    let mut n: Vec<&str> = results.iter().map(|(n, _)| n.as_str()).collect();
    n.sort_unstable();
    n
}

fn reason(result: &Refresh) -> &str {
    match result {
        Refresh::Skipped(reason) => reason,
        other => panic!("expected Skipped, got {other:?}"),
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Sets HOME and the root. The guard restores both on drop, also on panic.
fn env_for(home: &Path, root: &Path) -> EnvGuard {
    let guard = EnvGuard::capture(&["HOME", ROOT_ENV]);
    // SAFETY: edition 2024 requires unsafe; callers hold env_lock().
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var(ROOT_ENV, root);
    }
    guard
}

// ---------------------------------------------------------------- T1 / AC1

#[test]
fn temp_home_hook_path_never_reaches_a_root_outside_home() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let w = World::new();
    assert!(
        w.real_root().starts_with(w.base()),
        "fixture root is a temp dir"
    );
    assert!(!w.real_root().starts_with(w.home()), "root is outside HOME");
    // A non-profile entry is filtered out, as in `refresh_in`.
    fs::create_dir_all(w.real_root().join(".auto-switch.d")).unwrap();
    let before = snapshot(w.base().join("real").as_path());
    let _env = env_for(&w.home(), &w.real_root());

    let called = Cell::new(false);
    let bin = w.bin();
    let update = point_hooks_at(&bin);
    let results = refresh_profile_hooks_checked(&w.bin(), &|s| {
        called.set(true);
        update(s)
    });

    assert_eq!(names(&results), ["a", "b"], "{results:?}");
    for (name, result) in &results {
        let why = reason(result);
        assert!(why.contains("outside HOME"), "{name}: {why}");
    }
    assert!(!called.get(), "no settings.json is parsed or updated");
    assert_eq!(
        snapshot(w.base().join("real").as_path()),
        before,
        "file set, hashes and mtimes under the real root are unchanged"
    );
}

#[test]
fn t1b_binary_under_an_injected_temp_dir_gives_the_temp_reason() {
    let w = World::new();
    let store = ProfileStore::new(w.real_root());
    let temp = vec![w.base().to_path_buf()];
    let before = snapshot(&w.real_root());
    let bin = w.bin();
    let results = refresh_in_checked(
        &store,
        &bin,
        Some(w.home().as_path()),
        &temp,
        &point_hooks_at(&bin),
    );
    assert_eq!(names(&results), ["a", "b"]);
    for (_, result) in &results {
        let why = reason(result);
        assert!(why.contains("is under temp dir"), "{why}");
        assert!(why.contains("outside HOME"), "{why}");
        assert!(why.contains(&w.bin().display().to_string()), "{why}");
    }
    assert_eq!(snapshot(&w.real_root()), before);
}

#[test]
fn every_refused_profile_gets_the_same_reason_even_without_settings() {
    let w = World::new();
    profile(&w.real_root(), "bare", None);
    let store = ProfileStore::new(w.real_root());
    let bin = w.bin();
    let results = refresh_in_checked(
        &store,
        &bin,
        Some(w.home().as_path()),
        &[],
        &point_hooks_at(&bin),
    );
    assert_eq!(names(&results), ["a", "b", "bare"]);
    let reasons: Vec<&str> = results.iter().map(|(_, r)| reason(r)).collect();
    assert!(reasons.iter().all(|r| *r == reasons[0]), "{reasons:?}");
    assert!(reasons[0].contains("is under HOME"), "{}", reasons[0]);
    assert!(!w.real_root().join("bare/settings.json").exists());
}

#[test]
fn checked_refresh_keeps_refresh_in_order_and_filter() {
    let w = World::new();
    fs::create_dir_all(w.real_root().join(".auto-switch.d")).unwrap();
    let store = ProfileStore::new(w.real_root());
    let bin = w.bin();
    let refused = refresh_in_checked(
        &store,
        &bin,
        Some(w.home().as_path()),
        &[],
        &point_hooks_at(&bin),
    );
    let unchecked = refresh_in(&store, &|_| {});
    let a: Vec<&str> = refused.iter().map(|(n, _)| n.as_str()).collect();
    let b: Vec<&str> = unchecked.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------- T2 / AC2

#[test]
fn a_missing_directory_or_dangling_hook_binary_is_refused_for_every_profile() {
    let w = World::new();
    for name in ["a", "b"] {
        profile(&w.home_root(), name, Some(&old_settings()));
    }
    profile(&w.home_root(), "bare", None);
    let dir_bin = w.base().join("bin-is-a-dir");
    fs::create_dir_all(&dir_bin).unwrap();
    let dangling = w.home().join(".local/bin/dangling-hooks");
    symlink(w.base().join("nowhere"), &dangling).unwrap();
    let store = ProfileStore::new(w.home_root());

    for bin in [w.home().join(".local/bin/missing-hooks"), dir_bin, dangling] {
        let before = snapshot(&w.home_root());
        let results = refresh_in_checked(
            &store,
            &bin,
            Some(w.home().as_path()),
            &[],
            &point_hooks_at(&bin),
        );
        assert_eq!(names(&results), ["a", "b", "bare"], "{}", bin.display());
        for (name, result) in &results {
            let why = reason(result);
            assert!(
                why.contains("does not exist or is not a file"),
                "{name}: {why}"
            );
        }
        assert_eq!(snapshot(&w.home_root()), before, "{}", bin.display());
    }
}

// ---------------------------------------------------------------- T3 / AC3

#[test]
fn an_existing_binary_with_the_root_inside_home_is_written() {
    let w = World::new();
    profile(&w.home_root(), "work", Some(&old_settings()));
    let store = ProfileStore::new(w.home_root());
    let temp = vec![w.base().to_path_buf()];
    let bin = w.bin();
    let results = refresh_in_checked(
        &store,
        &bin,
        Some(w.home().as_path()),
        &temp,
        &point_hooks_at(&bin),
    );
    assert_eq!(results, vec![("work".to_string(), Refresh::Updated)]);
    let s = read(&w.home_root().join("work/settings.json"));
    let cmd = s["hooks"]["Stop"].to_string();
    assert!(cmd.contains(&w.bin().display().to_string()), "{cmd}");
    assert!(!cmd.contains(STALE_BIN), "{cmd}");
    assert_eq!(s["language"], "en");
}

// ---------------------------------------------------------------- T4 / AC4

#[test]
fn status_line_is_kept_while_hooks_move_to_the_new_binary() {
    let w = World::new();
    let status =
        json!({"type": "command", "command": "/usr/local/bin/amplihack profile statusline --x"});
    let mut s: Value = serde_json::from_str(&old_settings()).unwrap();
    s["statusLine"] = status.clone();
    profile(&w.home_root(), "work", Some(&s.to_string()));
    let store = ProfileStore::new(w.home_root());
    let bin = w.bin();
    let results = refresh_in_checked(
        &store,
        &bin,
        Some(w.home().as_path()),
        &[],
        &point_hooks_at(&bin),
    );
    assert_eq!(results, vec![("work".to_string(), Refresh::Updated)]);
    let after = read(&w.home_root().join("work/settings.json"));
    assert_eq!(
        after["statusLine"], status,
        "statusLine is byte-equal as JSON"
    );
    let cmd = after["hooks"]["Stop"].to_string();
    assert!(cmd.contains(&w.bin().display().to_string()), "{cmd}");
    assert!(!cmd.contains(STALE_BIN), "{cmd}");
}

// ---------------------------------------------------------------- T5: the rule

#[test]
fn rule_allows_a_binary_outside_both_bases() {
    let w = World::new();
    let opt = w.base().join("opt/amplihack-hooks");
    stub(&opt);
    let temp = vec![w.base().join("tmp")];
    fs::create_dir_all(&temp[0]).unwrap();
    assert_eq!(
        hook_path_refusal(&opt, &w.real_root(), Some(w.home().as_path()), &temp),
        None
    );
}

#[test]
fn rule_compares_whole_components_so_tmpevil_is_not_under_tmp() {
    let w = World::new();
    let temp = vec![w.base().join("tmp")];
    fs::create_dir_all(&temp[0]).unwrap();
    let evil = w.base().join("tmpevil/amplihack-hooks");
    stub(&evil);
    assert_eq!(
        hook_path_refusal(&evil, &w.real_root(), Some(w.home().as_path()), &temp),
        None
    );
    // And `homeevil` is not under `home`.
    let homeevil = w.base().join("homeevil/amplihack-hooks");
    stub(&homeevil);
    assert_eq!(
        hook_path_refusal(&homeevil, &w.real_root(), Some(w.home().as_path()), &[]),
        None
    );
}

#[test]
fn rule_refuses_through_a_symlinked_temp_dir_both_ways() {
    let w = World::new();
    let real_tmp = w.base().join("private-tmp");
    let link_tmp = w.base().join("link-tmp");
    fs::create_dir_all(&real_tmp).unwrap();
    symlink(&real_tmp, &link_tmp).unwrap();
    let bin = real_tmp.join("amplihack-hooks");
    stub(&bin);
    let home = Some(w.home());
    let via_link = hook_path_refusal(
        &bin,
        &w.real_root(),
        home.as_deref(),
        std::slice::from_ref(&link_tmp),
    );
    assert!(
        via_link
            .as_deref()
            .is_some_and(|r| r.contains("is under temp dir")),
        "{via_link:?}"
    );
    let bin_via_link = link_tmp.join("amplihack-hooks");
    let via_real = hook_path_refusal(&bin_via_link, &w.real_root(), home.as_deref(), &[real_tmp]);
    assert!(
        via_real
            .as_deref()
            .is_some_and(|r| r.contains("is under temp dir")),
        "{via_real:?}"
    );
}

#[test]
fn rule_fails_closed_without_a_usable_home() {
    let w = World::new();
    let relative = PathBuf::from("relative/home");
    for home in [None, Some(Path::new("/")), Some(relative.as_path())] {
        let got = hook_path_refusal(&w.bin(), &w.real_root(), home, &[]);
        assert_eq!(
            got.as_deref(),
            Some("cannot determine HOME; refusing to write hook paths"),
            "{home:?}"
        );
    }
}

#[test]
fn rule_checks_home_before_the_binary() {
    let w = World::new();
    let got = hook_path_refusal(Path::new("/no/such/bin"), &w.real_root(), None, &[]);
    assert_eq!(
        got.as_deref(),
        Some("cannot determine HOME; refusing to write hook paths")
    );
}

#[test]
fn rule_allows_a_temp_binary_when_the_root_is_inside_home() {
    let w = World::new();
    fs::create_dir_all(w.home_root()).unwrap();
    let temp = vec![w.base().to_path_buf()];
    assert_eq!(
        hook_path_refusal(&w.bin(), &w.home_root(), Some(w.home().as_path()), &temp),
        None
    );
}

#[test]
fn rule_refuses_a_home_binary_when_the_root_is_outside_home() {
    let w = World::new();
    let got = hook_path_refusal(&w.bin(), &w.real_root(), Some(w.home().as_path()), &[]).unwrap();
    assert!(got.contains("is under HOME"), "{got}");
    assert!(got.contains("outside HOME"), "{got}");
    assert!(got.contains(&w.home().display().to_string()), "{got}");
    assert!(got.contains(&w.real_root().display().to_string()), "{got}");
}

#[test]
fn rule_refuses_a_temp_home_link_to_a_binary_outside_both_bases() {
    // S1: register_hooks writes the path as given, so the lexical form counts.
    let w = World::new();
    let target = w.base().join("opt/amplihack-hooks");
    stub(&target);
    let link = w.home().join(".local/bin/linked-hooks");
    symlink(&target, &link).unwrap();
    let got = hook_path_refusal(&link, &w.real_root(), Some(w.home().as_path()), &[]);
    assert!(
        got.as_deref().is_some_and(|r| r.contains("outside HOME")),
        "{got:?}"
    );
}

#[test]
fn rule_refuses_a_relative_binary() {
    let w = World::new();
    let got = hook_path_refusal(
        Path::new("bin/amplihack-hooks"),
        &w.real_root(),
        Some(w.home().as_path()),
        &[],
    );
    assert!(
        got.as_deref()
            .is_some_and(|r| r.contains("is not an absolute path")),
        "{got:?}"
    );
}

#[test]
fn rule_refuses_a_root_inside_home_that_links_outside_home() {
    let w = World::new();
    let link_root = w.home().join(".claude-profiles");
    symlink(w.real_root(), &link_root).unwrap();
    let got = hook_path_refusal(&w.bin(), &link_root, Some(w.home().as_path()), &[]);
    assert!(
        got.as_deref().is_some_and(|r| r.contains("outside HOME")),
        "{got:?}"
    );
}

#[test]
fn rule_handles_a_root_that_does_not_exist_yet() {
    // norm() falls back to the nearest existing ancestor.
    let w = World::new();
    let missing_inside = w.home().join("not/yet/profiles");
    assert_eq!(
        hook_path_refusal(&w.bin(), &missing_inside, Some(w.home().as_path()), &[]),
        None
    );
    let missing_outside = w.base().join("elsewhere/profiles");
    assert!(hook_path_refusal(&w.bin(), &missing_outside, Some(w.home().as_path()), &[]).is_some());
}

#[test]
fn rule_reasons_never_carry_control_characters() {
    let w = World::new();
    // Missing binary: the "does not exist" reason.
    let missing = w.home().join("bad\x1b[31m\nname");
    let got = hook_path_refusal(&missing, &w.real_root(), Some(w.home().as_path()), &[]).unwrap();
    assert!(!got.chars().any(char::is_control), "{got:?}");
    assert!(got.contains("does not exist or is not a file"), "{got:?}");
    // Existing binary: the containment reason.
    let odd = w.home().join("odd\x1b[31m\ndir/amplihack-hooks");
    stub(&odd);
    let got = hook_path_refusal(&odd, &w.real_root(), Some(w.home().as_path()), &[]).unwrap();
    assert!(!got.chars().any(char::is_control), "{got:?}");
    assert!(got.contains("outside HOME"), "{got:?}");
}

// The candidate filter is tested without setting TMPDIR: tests that do not
// hold env_lock() create temp dirs from it in parallel.

#[test]
fn default_temp_dirs_drop_empty_relative_and_root_values() {
    let tmp = fs::canonicalize("/tmp").unwrap();
    for value in ["", "rel", "/"] {
        let dirs = temp_dirs_from(vec![PathBuf::from(value), PathBuf::from("/tmp")]);
        assert_eq!(dirs, std::slice::from_ref(&tmp), "{value:?}");
    }
}

#[test]
fn default_temp_dirs_list_each_dir_once_canonicalised() {
    let d = tempfile::tempdir().unwrap();
    let canon = fs::canonicalize(d.path()).unwrap();
    // temp_dir() and $TMPDIR often name the same dir, in different forms.
    let dirs = temp_dirs_from(vec![
        d.path().to_path_buf(),
        d.path().join("."),
        canon.clone(),
    ]);
    assert_eq!(dirs, [canon]);
}

#[test]
fn default_temp_dirs_hold_only_usable_absolute_dirs() {
    let dirs = default_temp_dirs();
    assert!(!dirs.is_empty());
    for d in &dirs {
        assert!(d.is_absolute() && d != Path::new("/"), "{dirs:?}");
    }
    assert!(
        dirs.contains(&fs::canonicalize("/tmp").unwrap()),
        "{dirs:?}"
    );
}
