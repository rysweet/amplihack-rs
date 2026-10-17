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
//! - under `recipe run` and `launch` the installer is never called, not a
//!   byte under HOME changes (stamp, binaries, `settings.json`, staged bundle;
//!   no backup, no `.bundle-compat-cache.json` memo, no install lock), no
//!   recipe YAML is read, and one warning names both binaries and both
//!   versions — even when the installed bundle is incompatible (the #1271
//!   repair is a re-stage too);
//! - with the real `run_install` as the installer, nothing under HOME changes;
//! - a newer stamp that lands while this process waits on the install lock is
//!   still refused (Linux, where `/proc/locks` shows the waiter).
//!
//! The R6 cases pin what must NOT change: the bypass line, and an unparseable
//! stamp still re-staging. An upgrade re-staging is `self_heal`'s own
//! `stamp_mismatch_triggers_install`. The launch bootstrap's refusal is in
//! `issue_1526_launch_bootstrap.rs`.
//!
//! All stamps are derived from `crate::VERSION`, so the tests hold for any
//! release.

use super::helpers::*;
use super::*;
use crate::self_heal::ensure_assets_match_binary_version_with;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

/// First line of the downgrade warning (see
/// `docs/features/self-heal-asset-restage.md#never-an-implicit-downgrade-1526`).
pub(super) const WARNING_HEADER: &str = "amplihack: warning: not re-staging framework assets";

/// A stamp strictly above `crate::VERSION`: the next major release.
pub(super) fn newer_version() -> String {
    let running = semver::Version::parse(crate::VERSION).expect("crate::VERSION is semver");
    format!("{}.0.0", running.major + 1)
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
        crate::self_heal::set_downgrade_warned(false);
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

/// Paths that were added (`+`), removed (`-`) or changed (`~`) between two
/// snapshots: readable in a failure message, unlike two maps of file bytes.
pub(super) fn changes(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<String> {
    let paths: std::collections::BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
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

// ── Installed-bundle fixtures ──
//
// A healthy install is `helpers::create_minimal_staged_assets`, whose bundle
// passes the #1271 check.

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
        running.contains(&format!(
            "{} (v{})",
            paths::sanitize_path(&exe),
            crate::VERSION
        )),
        "`running:` must name this executable and its version; got {running:?}"
    );
    let installed_bin = crate::path_conflicts::preferred_user_bin(home)
        .join(crate::path_conflicts::binary_filename("amplihack"));
    let installed = line("installed:");
    assert!(
        installed.contains(&format!(
            "{} (v{installed_version})",
            paths::sanitize_path(&installed_bin)
        )),
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

// ── The refusal ──

const SENTINEL_SETTINGS: &[u8] =
    b"{\n  \"model\": \"sentinel\",\n  \"permissions\": { \"allow\": [\"Bash(ls:*)\"] }\n}\n";

/// What a newer binary's install left behind: both binaries in `~/.local/bin`
/// and a user's `~/.claude/settings.json`.
fn plant_newer_install(home: &Path, newer: &str) {
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
        ("compatible", create_minimal_staged_assets),
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
            assert_eq!(
                changes(&before, &after),
                Vec::<String>::new(),
                "{case}: a refusal must leave every byte under HOME unchanged (the stamp, \
                 ~/.local/bin, settings.json, the staged bundle) and create nothing: no \
                 backup, no .bundle-compat-cache.json, no .install.lock"
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
    create_minimal_staged_assets(home);
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

/// A stamp the reader rejects — build metadata included, so equal precedence
/// with `+build` — reads back as missing and still stages (#502). With nothing
/// to compare against, it is never a downgrade.
#[test]
fn unparseable_or_build_metadata_stamp_still_stages() {
    for raw in [format!("{}+build.7", crate::VERSION), "garbage".to_string()] {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let _env = IsolatedHome::new(home);
        create_minimal_staged_assets(home);
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

// ── The real installer, end to end ──

/// The #1526 failure as it happened: the real `run_install` behind self-heal,
/// a newer install already in `~/.local/bin`, and a user's `settings.json`.
/// A refusal must leave every byte under HOME as it was.
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
    plant_newer_install(home, &newer);
    version_stamp::write_installed_version(&newer).unwrap();
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

    assert_eq!(
        changes(&before, &snapshot(home)),
        Vec::<String>::new(),
        "an implicit re-stage by an older binary must leave every byte under HOME \
         unchanged: ~/.local/bin, ~/.claude/settings.json, backups, staged assets and \
         the stamp (#1526)"
    );
    assert_eq!(calls.get(), 0, "the installer must never be called");
    result.expect("a refusal is not a failure");
    assert_refusal_warning(&notice, home, &newer);
}

// ── The race under the install lock ──

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

/// A process that passed the pre-lock check (no stamp yet, so it means to
/// install) waits on the install lock; a newer install finishes and releases
/// it. Re-reading the stamp under the lock must refuse, not install over the
/// newer one.
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

    let newer = newer_version();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _env = IsolatedHome::new(home);
    create_minimal_staged_assets(home);

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
