//! Issue #1526 — an implicit re-stage must never replace a newer install with
//! an older build.
//!
//! A host had v0.18.39 installed in `~/.local/bin`. A detached recipe run whose
//! `PATH` listed `~/.cargo/bin` first ran a `cargo install --git` build of the
//! `main` commit v0.18.39 was cut from, which reported `0.18.0`. Startup
//! self-heal saw stamp `0.18.39`, treated "differs" as "re-stage", replaced both
//! installed binaries with the older build and rewrote
//! `~/.claude/settings.json`, with no install command anywhere in sight.
//!
//! These tests drive the real self-heal entry point
//! (`self_heal::ensure_assets_match_binary_version_with`) with a stamp newer
//! than `crate::VERSION`:
//!
//! - (a) under `recipe run` and `launch` the installer is never called, not a
//!   byte under HOME changes (stamp, binaries, `settings.json`, staged bundle;
//!   no backup, no `.bundle-compat-cache.json` memo, no install lock), no
//!   recipe YAML is read, and one warning names both binaries and both
//!   versions — even when the installed bundle is incompatible (the #1271
//!   repair is a re-stage too);
//! - (b) with the real `run_install` as the installer, nothing under HOME
//!   changes: binaries, `settings.json`, backups, stamp;
//! - (d) a newer stamp that lands while this process waits on the install
//!   lock is still refused (Linux, where `/proc/locks` shows the waiter).
//!
//! The R6 cases pin what must NOT change: the bypass line, and an upgrade or
//! an unparseable stamp still re-staging.
//!
//! Everything here uses only items that already exist on `main` (plus
//! `semver`, already a dependency), so copying this file onto an unfixed
//! checkout shows the behavioural failure rather than a compile error. All
//! stamps are derived from `crate::VERSION`, so the tests hold for any release.

use super::helpers::*;
use super::*;
use crate::self_heal::ensure_assets_match_binary_version_with;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

/// First line of the downgrade warning (see
/// `docs/features/self-heal-asset-restage.md#the-warning`).
pub(super) const WARNING_HEADER: &str = "amplihack: warning: not re-staging framework assets";

// ── Versions, all derived from crate::VERSION ──

fn running_version() -> semver::Version {
    semver::Version::parse(crate::VERSION).expect("crate::VERSION is semver")
}

/// A stamp strictly above `crate::VERSION`: the next major release.
pub(super) fn newer_version() -> String {
    let newer = format!("{}.0.0", running_version().major + 1);
    let parsed = semver::Version::parse(&newer).unwrap();
    assert_eq!(
        parsed.cmp_precedence(&running_version()),
        std::cmp::Ordering::Greater,
        "test setup: {newer} must outrank {}",
        crate::VERSION
    );
    newer
}

/// A stamp strictly below `crate::VERSION`: decrement the first non-zero
/// component and zero the rest (`0.0.0` becomes `0.0.0-0`).
fn older_version() -> String {
    let v = running_version();
    let older = if v.major > 0 {
        format!("{}.0.0", v.major - 1)
    } else if v.minor > 0 {
        format!("0.{}.0", v.minor - 1)
    } else if v.patch > 0 {
        format!("0.0.{}", v.patch - 1)
    } else {
        "0.0.0-0".to_string()
    };
    let parsed = semver::Version::parse(&older).unwrap();
    assert_eq!(
        parsed.cmp_precedence(&v),
        std::cmp::Ordering::Less,
        "test setup: {older} must rank below {}",
        crate::VERSION
    );
    older
}

// ── Environment isolation (SR-D4) ──

/// Holds the crate-wide env lock and points `HOME` at a temp dir, clearing
/// every variable that would let installer code escape it. Each variable is
/// saved the first time it is touched; all are restored on drop, newest first.
///
/// Drop order is load-bearing: `Drop::drop` restores the variables before the
/// `_lock` field is released, even on panic, so no other test observes this
/// test's HOME and nothing here can resolve to the developer's real one.
pub(super) struct IsolatedHome {
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl IsolatedHome {
    pub(super) fn new(home: &Path) -> Self {
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut env = Self {
            saved: Vec::new(),
            _lock: lock,
        };
        env.put("HOME", Some(home.as_os_str()));
        for name in [
            "AMPLIHACK_SKIP_AUTO_INSTALL",
            "AMPLIHACK_HOME",
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
            "AMPLIHACK_REPAIR_ORIGINAL_PATH",
        ] {
            env.put(name, None);
        }
        env
    }

    pub(super) fn set(&mut self, name: &'static str, value: impl AsRef<OsStr>) {
        self.put(name, Some(value.as_ref()));
    }

    fn put(&mut self, name: &'static str, value: Option<&OsStr>) {
        if !self.saved.iter().any(|(saved, _)| *saved == name) {
            self.saved.push((name, std::env::var_os(name)));
        }
        // SAFETY: edition 2024 requires unsafe; `self` holds env_lock().
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        for (name, prior) in self.saved.drain(..).rev() {
            // SAFETY: `_lock` is released only after this returns.
            unsafe {
                match prior {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// Tripwire: refuse to run installer code unless HOME is the test's temp dir.
pub(super) fn assert_home_is(home: &Path) {
    assert_eq!(
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        Some(home),
        "HOME must be the test's temp dir before any installer code runs"
    );
    assert!(
        home.starts_with(std::env::temp_dir()),
        "HOME {home:?} is not under the temp dir; refusing to run installer code"
    );
}

// ── Filesystem observation ──

/// Every entry under `root`, keyed by path relative to `root`: file bytes,
/// symlink targets (not followed), and a marker for directories.
pub(super) fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                out.insert(rel, target.to_string_lossy().into_owned().into_bytes());
            } else if meta.is_dir() {
                out.insert(rel, b"<dir>".to_vec());
                walk(root, &path, out);
            } else {
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Paths under `prefix` that were added (`+`), removed (`-`) or changed (`~`)
/// between two snapshots: readable in a failure message, unlike two maps of
/// file bytes. An empty `prefix` covers the whole snapshot.
pub(super) fn changes(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
    prefix: &str,
) -> Vec<String> {
    let paths: std::collections::BTreeSet<&PathBuf> = before
        .keys()
        .chain(after.keys())
        .filter(|rel| rel.starts_with(prefix))
        .collect();
    paths
        .into_iter()
        .filter_map(|rel| match (before.get(rel), after.get(rel)) {
            (None, Some(_)) => Some(format!("+ {}", rel.display())),
            (Some(_), None) => Some(format!("- {}", rel.display())),
            (Some(old), Some(new)) if old != new => Some(format!("~ {}", rel.display())),
            _ => None,
        })
        .collect()
}

/// Settings and install-time backups a re-stage writes. A refusal must leave
/// none behind. Searched recursively: the install backup really lives at
/// `~/.amplihack/.claude/runtime/sessions/`, not where the name suggests.
pub(super) fn backups_under(root: &Path) -> Vec<PathBuf> {
    snapshot(root)
        .into_keys()
        .filter(|rel| {
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name.starts_with("settings.json.backup.")
                || (name.starts_with("install_") && name.ends_with("_backup.json"))
        })
        .collect()
}

// ── Installed-bundle fixtures (mirror self_heal's own tests) ──

fn write_compatible_installed_bundle(home: &Path) {
    let recipes = home.join(".amplihack/amplifier-bundle/recipes");
    fs::create_dir_all(&recipes).unwrap();
    fs::write(
        recipes.join("smart-orchestrator.yaml"),
        r#"name: "smart-orchestrator"
description: "Composable smart task orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
"#,
    )
    .unwrap();
    for recipe in bundle_compat::REQUIRED_SMART_RECIPES {
        fs::write(
            recipes.join(format!("{recipe}.yaml")),
            format!(
                "name: \"{recipe}\"\nsteps:\n  - id: smoke\n    type: bash\n    command: 'true'\n"
            ),
        )
        .unwrap();
    }
    fs::write(
        recipes.join("_recipe_manifest.json"),
        r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-execute-routing": "11612506ae846a47",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
    )
    .unwrap();
}

/// The pre-composable smart-orchestrator the #1271 repair exists to replace.
fn write_stale_installed_smart_orchestrator(home: &Path) {
    let recipes = home.join(".amplihack/amplifier-bundle/recipes");
    fs::create_dir_all(&recipes).unwrap();
    fs::write(
        recipes.join("smart-orchestrator.yaml"),
        r#"name: "smart-orchestrator"
description: "stale v1.5.1 smart task orchestrator"
steps:
  - id: "parse-decomposition"
    type: "shell"
    command: |
      HELPER="$(amplihack resolve-bundle-asset helper-path)"
      python3 - <<'PY'
      import importlib
      helper = importlib.import_module("orch_helper")
      helper.parse_decomposition()
      PY
"#,
    )
    .unwrap();
}

// ── Self-heal drivers ──

pub(super) fn args(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

/// An installer that counts its calls and, like the real one, writes the
/// running version into the stamp.
pub(super) fn counting_installer(calls: &Cell<u32>) -> impl FnOnce() -> anyhow::Result<()> + '_ {
    move || {
        calls.set(calls.get() + 1);
        version_stamp::write_installed_version(crate::VERSION)
    }
}

fn stamp_bytes() -> Vec<u8> {
    fs::read(version_stamp::installed_version_path().unwrap()).expect("the stamp exists")
}

/// The refusal reached the user exactly once, named both binaries and both
/// versions, said what was left alone and how to resolve it, and claimed no
/// re-stage.
pub(super) fn assert_refusal_warning(notice: &str, home: &Path, installed_version: &str) {
    assert_eq!(
        notice.matches(WARNING_HEADER).count(),
        1,
        "exactly one downgrade warning per process; got:\n{notice}"
    );
    assert_eq!(
        notice.matches('\n').count(),
        5,
        "the warning is one five-line block and nothing else; got:\n{notice}"
    );

    let line = |label: &str| {
        notice
            .lines()
            .find(|l| l.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no `{label}` line in the warning:\n{notice}"))
            .to_string()
    };
    let exe = std::env::current_exe().expect("the test binary has a current_exe");
    let running = line("running:");
    assert!(
        running.contains(&format!("{exe:?} (v{})", crate::VERSION)),
        "`running:` must name this executable and its version; got {running:?}"
    );
    let installed_bin = crate::path_conflicts::preferred_user_bin(home)
        .join(crate::path_conflicts::binary_filename("amplihack"));
    let installed = line("installed:");
    assert!(
        installed.contains(&format!("{installed_bin:?} (v{installed_version})")),
        "`installed:` must name ~/.local/bin/amplihack and the stamp version; got {installed:?}"
    );
    assert!(
        notice.contains("left unchanged"),
        "the warning must say the install was left alone; got:\n{notice}"
    );
    assert!(
        notice.contains("amplihack install"),
        "the warning must name the explicit remedy; got:\n{notice}"
    );
    assert!(
        !notice.contains("re-staged for"),
        "a refusal must never claim a re-stage; got:\n{notice}"
    );
}

// ── (a) The refusal ──

const SENTINEL_SETTINGS: &[u8] =
    b"{\n  \"model\": \"sentinel\",\n  \"permissions\": { \"allow\": [\"Bash(ls:*)\"] }\n}\n";

/// What a newer binary's install left behind: both binaries in `~/.local/bin`
/// and a user's `~/.claude/settings.json`. Returns the settings path.
fn plant_newer_install(home: &Path, newer: &str) -> PathBuf {
    let local_bin = crate::path_conflicts::preferred_user_bin(home);
    for name in ["amplihack", "amplihack-hooks"] {
        let path = create_exe_stub(&local_bin, &crate::path_conflicts::binary_filename(name));
        fs::write(
            path,
            format!(
                "#!/bin/sh\n# sentinel {name} v{newer}\n{}\n",
                "x".repeat(1100)
            ),
        )
        .unwrap();
    }
    let settings = home.join(".claude").join("settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(&settings, SENTINEL_SETTINGS).unwrap();
    settings
}

/// R8(1). The version check comes first, so the refusal does no I/O at all:
/// no install lock, and no #1271 bundle verdict either — an older binary must
/// not parse a newer install's recipe YAML, nor write the
/// `.bundle-compat-cache.json` memo keyed on its own version. That holds for a
/// healthy bundle and for one the #1271 repair would re-stage, because that
/// repair is a re-stage too.
#[test]
fn newer_stamp_refuses_implicit_restage_and_changes_nothing_under_home() {
    type WriteBundle = fn(&Path);
    let newer = newer_version();
    let bundles: [(&str, WriteBundle); 2] = [
        ("compatible", write_compatible_installed_bundle),
        ("incompatible", write_stale_installed_smart_orchestrator),
    ];
    for argv in [
        &["amplihack", "recipe", "run", "x"][..],
        &["amplihack", "launch"][..],
    ] {
        for (bundle, write_bundle) in bundles {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let _env = IsolatedHome::new(home);
            write_bundle(home);
            plant_newer_install(home, &newer);
            version_stamp::write_installed_version(&newer).unwrap();
            let before = snapshot(home);

            let calls = Cell::new(0u32);
            let mut notice = Vec::new();
            assert_home_is(home);
            let (result, reads) = bundle_compat::read_counter::count_reads(|| {
                ensure_assets_match_binary_version_with(
                    &args(argv),
                    &mut notice,
                    counting_installer(&calls),
                )
            });
            let notice = String::from_utf8(notice).unwrap();
            let after = snapshot(home);
            let case = format!("`{}` over a {bundle} bundle", argv[1..].join(" "));

            result.unwrap_or_else(|e| panic!("{case}: a refusal is not a failure: {e:#}"));
            assert_eq!(
                calls.get(),
                0,
                "{case}: v{} over a v{newer} stamp must not call the installer; notice:\n{notice}",
                crate::VERSION
            );
            assert_eq!(
                reads, 0,
                "{case}: the refusal comes before the bundle verdict, so it must not read \
                 the installed bundle's recipe YAML"
            );
            for written in [
                ".bundle-compat-cache.json",
                ".bundle-compat-cache.json.tmp",
                ".install.lock",
            ] {
                assert!(
                    !home.join(".amplihack").join(written).exists(),
                    "{case}: the refusal comes before the bundle verdict and the install \
                     lock, so it must not create ~/.amplihack/{written}"
                );
            }
            assert_eq!(
                backups_under(home),
                Vec::<PathBuf>::new(),
                "{case}: a refusal must not write settings or install backups"
            );
            assert_eq!(
                changes(&before, &after, ""),
                Vec::<String>::new(),
                "{case}: a refusal must leave every byte under HOME unchanged: the \
                 stamp, ~/.local/bin, settings.json and the staged bundle"
            );
            assert_refusal_warning(&notice, home, &newer);
        }
    }
}

// ── R6: what must not change ──

/// The bypass is checked before the downgrade guard: with it set, an older
/// binary prints the existing bypass line, byte for byte, and never the
/// downgrade warning.
#[test]
fn bypass_with_a_newer_stamp_prints_only_the_unchanged_bypass_line() {
    let newer = newer_version();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let mut env = IsolatedHome::new(home);
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    write_compatible_installed_bundle(home);
    version_stamp::write_installed_version(&newer).unwrap();

    let calls = Cell::new(0u32);
    let mut notice = Vec::new();
    ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "run", "x"]),
        &mut notice,
        counting_installer(&calls),
    )
    .expect("bypass must succeed");

    assert_eq!(calls.get(), 0, "bypass never installs");
    assert_eq!(
        String::from_utf8(notice).unwrap(),
        format!(
            "amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage \
             (stamp={newer} current={}; installed_bundle=\"compatible\")\n",
            crate::VERSION
        ),
        "the bypass diagnostic must stay byte-identical and stand alone"
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(newer.as_str())
    );
}

/// The #488 upgrade path: a binary newer than the stamp still re-stages.
#[test]
fn older_stamp_still_restages_forward() {
    let older = older_version();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _env = IsolatedHome::new(home);
    write_compatible_installed_bundle(home);
    version_stamp::write_installed_version(&older).unwrap();

    let calls = Cell::new(0u32);
    let mut notice = Vec::new();
    ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "run", "x"]),
        &mut notice,
        counting_installer(&calls),
    )
    .expect("an upgrade re-stage must succeed");

    assert_eq!(
        calls.get(),
        1,
        "v{} over a v{older} stamp must re-stage",
        crate::VERSION
    );
    assert_eq!(
        String::from_utf8(notice).unwrap(),
        format!(
            "amplihack: framework assets re-staged for v{}\n",
            crate::VERSION
        ),
        "an upgrade prints the normal notice and no downgrade warning"
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(crate::VERSION)
    );
}

/// A stamp the reader rejects — build metadata included, so equal precedence
/// with `+build` — reads back as missing and still stages (#502). With nothing
/// to compare against, it is never a downgrade.
#[test]
fn unparseable_or_build_metadata_stamp_still_stages() {
    for raw in [format!("{}+build.7", crate::VERSION), "garbage".to_string()] {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let _env = IsolatedHome::new(home);
        write_compatible_installed_bundle(home);
        let stamp_path = version_stamp::installed_version_path().unwrap();
        fs::create_dir_all(stamp_path.parent().unwrap()).unwrap();
        fs::write(stamp_path, &raw).unwrap();

        let calls = Cell::new(0u32);
        let mut notice = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut notice,
            counting_installer(&calls),
        )
        .expect("an unparseable stamp must self-heal");
        let notice = String::from_utf8(notice).unwrap();

        assert_eq!(
            calls.get(),
            1,
            "stamp {raw:?} reads as missing and must stage"
        );
        assert!(
            !notice.contains(WARNING_HEADER),
            "stamp {raw:?} is not a downgrade; got:\n{notice}"
        );
    }
}

// ── (b) The real installer, end to end ──

/// The #1526 failure as it happened: the real `run_install` behind self-heal,
/// a newer install already in `~/.local/bin`, and a user's `settings.json`.
/// A refusal must leave every byte under HOME as it was, the
/// bundle-compatibility memo included.
#[test]
fn real_run_install_behind_self_heal_never_touches_a_newer_install() {
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path();
    // The source repo and stub tools live outside HOME so the HOME snapshot
    // sees only what the installer would write.
    let fixture = tempfile::tempdir().unwrap();
    let stub_bin = fixture.path().join("stub_bin");
    create_exe_stub(&stub_bin, "python3");
    let hooks_stub = create_exe_stub(&stub_bin, "amplihack-hooks");
    let runner_stub = create_exe_stub(&stub_bin, "recipe-runner-rs");
    let repo = fixture.path().join("local-repo");
    create_source_repo(&repo);

    let mut env = IsolatedHome::new(home);
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    env.set("RECIPE_RUNNER_RS_PATH", &runner_stub);
    // Network tripwire: the unfixed path must never fall back to cargo install.
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
    let mut path_dirs = vec![stub_bin.clone()];
    path_dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    env.set("PATH", std::env::join_paths(path_dirs).unwrap());

    let newer = newer_version();
    let local_bin = crate::path_conflicts::preferred_user_bin(home);
    let settings = plant_newer_install(home, &newer);
    version_stamp::write_installed_version(&newer).unwrap();
    let stamp_before = stamp_bytes();
    let before = snapshot(home);

    let calls = Cell::new(0u32);
    let mut notice = Vec::new();
    assert_home_is(home);
    let result = ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "run", "x"]),
        &mut notice,
        || {
            calls.set(calls.get() + 1);
            assert_home_is(home);
            run_install(Some(repo.clone()), false, false)
        },
    );
    let notice = String::from_utf8_lossy(&notice).into_owned();
    let after = snapshot(home);

    assert_eq!(
        String::from_utf8_lossy(&fs::read(&settings).unwrap()),
        String::from_utf8_lossy(SENTINEL_SETTINGS),
        "an implicit re-stage by an older binary rewrote ~/.claude/settings.json (#1526)"
    );
    assert_eq!(
        changes(&before, &after, ".local/bin"),
        Vec::<String>::new(),
        "an implicit re-stage by an older binary replaced binaries in ~/.local/bin (#1526)"
    );
    assert_eq!(
        backups_under(home),
        Vec::<PathBuf>::new(),
        "a refusal must not write settings or install backups"
    );
    assert!(
        !local_bin
            .join(crate::path_conflicts::binary_filename(
                "amplihack-asset-resolver"
            ))
            .exists(),
        "a refusal must not deploy amplihack-asset-resolver"
    );
    assert_eq!(
        changes(&before, &after, ".claude"),
        Vec::<String>::new(),
        "a refusal must not publish anything into ~/.claude"
    );
    assert!(
        !home.join(".amplihack").join(".claude").exists(),
        "a refusal must not stage framework assets"
    );
    assert_eq!(
        stamp_bytes(),
        stamp_before,
        "a refusal must not rewrite the stamp"
    );
    assert_eq!(calls.get(), 0, "the installer must never be called");
    result.expect("a refusal is not a failure");
    assert_refusal_warning(&notice, home, &newer);
    assert_eq!(
        changes(&before, &after, ""),
        Vec::<String>::new(),
        "a refusal must leave every byte under HOME unchanged"
    );
}

// ── (d) The race under the install lock ──

/// Poll `/proc/locks` until a blocked flock waiter (`->`) on inode `ino`
/// appears. Returns false on timeout.
#[cfg(target_os = "linux")]
fn wait_for_blocked_flock_waiter(ino: u64, timeout: std::time::Duration) -> bool {
    let needle = format!(":{ino} ");
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(locks) = fs::read_to_string("/proc/locks")
            && locks
                .lines()
                .any(|l| l.contains("->") && l.contains("FLOCK") && l.contains(&needle))
        {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// A process that passed the pre-lock check with an older stamp waits on the
/// install lock; a newer install finishes and releases it. Re-reading the
/// stamp under the lock must refuse, not install over the newer one.
///
/// Deterministic: the newer stamp is written only after `/proc/locks` shows
/// this process blocked on the lock file, so the worker is guaranteed to be
/// past the pre-lock check.
#[cfg(target_os = "linux")]
#[test]
fn newer_stamp_landing_while_waiting_on_the_install_lock_is_still_refused() {
    use fs4::fs_std::FileExt;
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    let older = older_version();
    let newer = newer_version();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _env = IsolatedHome::new(home);
    write_compatible_installed_bundle(home);
    version_stamp::write_installed_version(&older).unwrap();

    // Hold the install lock the way a concurrent (newer) installer would.
    let lock_path = home.join(".amplihack").join(".install.lock");
    let holder = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    FileExt::lock_exclusive(&holder).unwrap();
    let ino = holder.metadata().unwrap().ino();

    let calls = AtomicU32::new(0);
    assert_home_is(home);
    let (observed, result, notice) = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut notice = Vec::new();
            let result = ensure_assets_match_binary_version_with(
                &args(&["amplihack", "recipe", "run", "x"]),
                &mut notice,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    version_stamp::write_installed_version(crate::VERSION)
                },
            );
            (result, notice)
        });

        let observed = wait_for_blocked_flock_waiter(ino, std::time::Duration::from_secs(10));
        if observed {
            // The newer install completes while the worker is blocked.
            version_stamp::write_installed_version(&newer).unwrap();
        }
        // Always release, or a timed-out worker would hang the scope's join.
        FileExt::unlock(&holder).unwrap();
        let (result, notice) = worker.join().expect("the self-heal worker panicked");
        (observed, result, notice)
    });
    drop(holder);
    let notice = String::from_utf8(notice).unwrap();

    assert!(
        observed,
        "no blocked flock waiter observed on inode {ino} within 10s; /proc/locks:\n{}",
        fs::read_to_string("/proc/locks").unwrap_or_default()
    );
    result.expect("a refusal under the lock is not a failure");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the stamp re-read under the lock is v{newer}; v{} must not install over it; notice:\n{notice}",
        crate::VERSION
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(newer.as_str()),
        "the newer install's stamp must survive"
    );
    assert_refusal_warning(&notice, home, &newer);
}
