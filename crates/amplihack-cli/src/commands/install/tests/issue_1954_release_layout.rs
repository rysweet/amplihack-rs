//! Issue #1954: the versioned release layout (`release_layout.rs`).
//!
//! These tests define the contract for `release_layout.rs`. They do not compile
//! until the module exists; register this file in `tests/mod.rs` (under
//! `#[cfg(unix)]`) in the same commit that adds it:
//!
//! - `crate::commands::install::release_layout` (`release_layout.rs`), the
//!   only code that builds or deletes paths under `~/.amplihack/releases`:
//!
//!   ```text
//!   const RELEASES_DIR = "releases"; RETIRED_DIR = ".retired"; COMPLETE_MARKER = ".complete";
//!   fn releases_dir(amplihack_dir) -> PathBuf
//!   fn validate_version(&str) -> anyhow::Result<()>                     (S6)
//!   fn is_release_path(releases, path) -> bool                          (S7)
//!   struct ReleaseTxn { allocate(amplihack_dir, version), root(), bin_dir(),
//!                       bundle_dir(), plan_link(&mut, link, target), commit(self) }
//!     Drop without commit removes the never-published root.
//!   struct SwitchReport { switched: Vec<LinkSwitch>, failed: Option<(PathBuf, String)> }
//!     commit switches planned links in the order amplihack-hooks,
//!     amplihack-asset-resolver, amplihack, amplifier-bundle; no rollback.
//!   struct LinkSwitch { link, target, previous: Option<PathBuf> }
//!   fn switch_link(amplihack_dir, link, target) -> anyhow::Result<LinkSwitch>
//!   fn keep_set(releases, links: &[PathBuf], previous: &[PathBuf]) -> Vec<PathBuf>
//!   enum Exchange { Exchanged, FellBack }
//!   fn exchange_dirs(a, b) / exchange_dirs_with(a, b, exchange_fn) -> anyhow::Result<Exchange>
//!   enum Retired { Moved(PathBuf), LeftInPlace(PathBuf) }
//!   fn retire(amplihack_dir, path, label) / retire_with(.., rename_fn) -> anyhow::Result<Retired>
//!   enum InUse { Known(BTreeMap<PathBuf, Vec<u32>>), Unknown, Unsupported }
//!   fn scan_pids(candidates, pids) -> InUse;  fn in_use_paths(candidates) -> InUse
//!   struct CleanupReport { removed: Vec<PathBuf>, kept_in_use: Vec<(PathBuf, Vec<u32>)>, unknown: bool }
//!   fn cleanup_unused(amplihack_dir, keep) / cleanup_unused_with(.., scan_fn) -> anyhow::Result<CleanupReport>
//!   fn make_tree_writable(root) -> std::io::Result<()>                  (S11)
//!   fn kept_in_use_warning(count) -> Option<String>
//!   fn format_kept_in_use(home, kept) -> Option<String>
//!   ```
//!
//! The install lock has its own file, `issue_1954_install_lock.rs`.

use super::super::bundle_compat::validate_framework_bundle_compatibility;
use super::super::release_layout::{
    self, COMPLETE_MARKER, CleanupReport, Exchange, InUse, RETIRED_DIR, ReleaseTxn, Retired,
    cleanup_unused_with, exchange_dirs, exchange_dirs_with, format_kept_in_use, is_release_path,
    keep_set, kept_in_use_warning, make_tree_writable, releases_dir, retire, retire_with,
    scan_pids, switch_link, validate_version,
};
use super::helpers::create_source_repo;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- fixtures

/// A temp dir holding `.amplihack/` (mode 0700) and `.local/bin/`.
struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    amp: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let amp = home.join(".amplihack");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&amp).unwrap();
        fs::create_dir_all(&bin).unwrap();
        set_mode(&amp, 0o700);
        Self {
            _temp: temp,
            home,
            amp,
            bin,
        }
    }

    fn releases(&self) -> PathBuf {
        releases_dir(&self.amp)
    }

    /// A committed release `name` with `bin/amplihack` and a bundle file.
    fn release(&self, name: &str) -> PathBuf {
        let root = self.releases().join(name);
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("amplifier-bundle/recipes")).unwrap();
        fs::write(root.join("bin/amplihack"), name).unwrap();
        fs::write(root.join("amplifier-bundle/recipes/r.yaml"), name).unwrap();
        fs::write(root.join(COMPLETE_MARKER), "").unwrap();
        set_mode(&self.releases(), 0o700);
        root
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn euid_is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn known(entries: &[(&Path, u32)]) -> InUse {
    let mut map = BTreeMap::new();
    for (path, pid) in entries {
        map.entry(path.to_path_buf())
            .or_insert_with(Vec::new)
            .push(*pid);
    }
    InUse::Known(map)
}

/// Our own child, killed and reaped on drop. Tests may stop what they start.
struct OwnChild(std::process::Child);

impl OwnChild {
    fn spawn(command: &mut Command) -> Self {
        Self(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for OwnChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits until `/proc/<pid>/<entry>` names something under `under`.
fn wait_for_proc_link(pid: u32, entry: &str, under: &Path) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if fs::read_link(format!("/proc/{pid}/{entry}")).is_ok_and(|p| p.starts_with(under)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pid {pid} never had {entry} under {}", under.display());
}

// ------------------------------------------------------- naming (D1, S6)

#[test]
fn allocate_uses_the_bare_version_first() {
    let fx = Fixture::new();
    let txn = ReleaseTxn::allocate(&fx.amp, "0.9.42").unwrap();
    assert_eq!(txn.root(), fx.releases().join("0.9.42"));
    assert_eq!(txn.bin_dir(), fx.releases().join("0.9.42/bin"));
    assert_eq!(
        txn.bundle_dir(),
        fx.releases().join("0.9.42/amplifier-bundle")
    );
    assert!(txn.root().is_dir());
    assert_eq!(
        mode(&fx.releases()),
        0o700,
        "releases/ is created 0700 (S1)"
    );
}

#[test]
fn allocate_name_collision_picks_a_new_directory_and_never_reuses_one() {
    let fx = Fixture::new();
    let first = ReleaseTxn::allocate(&fx.amp, "0.9.42").unwrap();
    fs::write(first.root().join("marker"), "first").unwrap();
    let second = ReleaseTxn::allocate(&fx.amp, "0.9.42").unwrap();
    let third = ReleaseTxn::allocate(&fx.amp, "0.9.42").unwrap();

    let second_name = second
        .root()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let pid = std::process::id().to_string();
    let rest = second_name
        .strip_prefix("0.9.42-")
        .unwrap_or_else(|| panic!("{second_name} must be <version>-<ts>-<pid>"));
    let (stamp, tail) = rest.split_once('-').unwrap();
    assert_eq!(stamp.len(), 16, "yyyymmddTHHMMSSZ in {second_name}");
    assert_eq!(&stamp[8..9], "T");
    assert!(stamp.ends_with('Z'));
    assert!(tail.starts_with(&pid), "{second_name} names pid {pid}");

    assert_ne!(
        second.root(),
        third.root(),
        "same second, same pid: still distinct"
    );
    for txn in [&second, &third] {
        assert_eq!(txn.root().parent(), Some(fx.releases().as_path()));
        assert!(
            txn.root()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("0.9.42")
        );
    }
    assert_eq!(
        fs::read_to_string(first.root().join("marker")).unwrap(),
        "first",
        "an existing release is never reused or rewritten"
    );
}

#[test]
fn a_dropped_uncommitted_release_is_removed() {
    let fx = Fixture::new();
    let root = {
        let txn = ReleaseTxn::allocate(&fx.amp, "1.0.0").unwrap();
        fs::create_dir_all(txn.bin_dir()).unwrap();
        fs::write(txn.bin_dir().join("amplihack"), "x").unwrap();
        txn.root().to_path_buf()
    };
    assert!(
        !root.exists(),
        "a release no link ever pointed to is removed"
    );
}

#[test]
fn allocate_rejects_unsafe_versions() {
    let long = "a".repeat(65);
    for bad in [
        "", ".", "..", ".retired", "../x", "a/b", "/abs", "-x", ".hidden", "v 1", "x\n", "1.0\0",
        &long,
    ] {
        assert!(validate_version(bad).is_err(), "{bad:?} must be rejected");
        let fx = Fixture::new();
        let err = ReleaseTxn::allocate(&fx.amp, bad).err();
        assert!(err.is_some(), "allocate({bad:?}) must fail, never sanitise");
        assert!(
            names(&fx.releases()).is_empty(),
            "allocate({bad:?}) must create no release"
        );
    }
    let max = "a".repeat(64);
    for good in ["0.9.42", "1.0.0-rc.1", "1.0.0+build.5", "v2", &max] {
        validate_version(good).unwrap_or_else(|e| panic!("{good:?}: {e:#}"));
    }
}

#[test]
fn allocate_refuses_a_group_writable_releases_dir_and_does_not_repair_it() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.releases()).unwrap();
    set_mode(&fx.releases(), 0o777);
    let err =
        ReleaseTxn::allocate(&fx.amp, "1.0.0").expect_err("a writable releases/ is refused (S1)");
    assert!(
        format!("{err:#}").contains(&fx.releases().display().to_string()),
        "the error names the directory: {err:#}"
    );
    assert_eq!(mode(&fx.releases()), 0o777, "never repaired");
}

// ------------------------------------------------------ confinement (S7)

#[test]
fn is_release_path_rejects_prefix_sibling_and_root() {
    let fx = Fixture::new();
    let release = fx.release("1.0.0");
    let releases = fx.releases();
    let sibling = fx.amp.join("releases-evil/1.0.0");
    fs::create_dir_all(&sibling).unwrap();
    let outside = fx.home.join("outside");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, releases.join("escape")).unwrap();

    assert!(is_release_path(&releases, &release));
    assert!(is_release_path(
        &releases,
        &release.join("amplifier-bundle")
    ));
    assert!(!is_release_path(&releases, &releases), "releases/ itself");
    assert!(
        !is_release_path(&releases, &sibling),
        "string-prefix sibling"
    );
    assert!(!is_release_path(&releases, &releases.join("..")));
    assert!(!is_release_path(
        &releases,
        &releases.join("1.0.0/../../outside")
    ));
    assert!(
        !is_release_path(&releases, &releases.join("escape")),
        "a symlink inside releases/ that resolves outside it"
    );
    assert!(
        !is_release_path(&releases, &releases.join("missing")),
        "canonicalise failure is 'not a release'"
    );
    assert!(!is_release_path(&releases, Path::new("/")));
}

// ---------------------------------------------------- switching (D3, D8)

#[test]
fn switch_link_over_a_regular_file_keeps_the_old_inode_readable() {
    let fx = Fixture::new();
    let release = fx.release("1.0.0");
    let link = fx.bin.join("amplihack");
    fs::write(&link, "legacy").unwrap();
    let open = fs::File::open(&link).unwrap();

    let switch = switch_link(&fx.amp, &link, &release.join("bin/amplihack")).unwrap();

    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&link).unwrap(), release.join("bin/amplihack"));
    assert_eq!(
        switch.previous, None,
        "a regular file is not a previous release"
    );
    let mut text = String::new();
    std::io::Read::read_to_string(&mut &open, &mut text).unwrap();
    assert_eq!(text, "legacy", "a running process keeps the old inode");
}

#[test]
fn switch_link_over_a_link_reports_the_previous_target() {
    let fx = Fixture::new();
    let old = fx.release("1.0.0");
    let new = fx.release("1.0.1");
    let link = fx.bin.join("amplihack");
    symlink(old.join("bin/amplihack"), &link).unwrap();

    let switch = switch_link(&fx.amp, &link, &new.join("bin/amplihack")).unwrap();

    assert_eq!(fs::read_link(&link).unwrap(), new.join("bin/amplihack"));
    assert_eq!(switch.previous, Some(old.join("bin/amplihack")));
    assert_eq!(
        fs::read_to_string(old.join("bin/amplihack")).unwrap(),
        "1.0.0"
    );
}

#[test]
fn switch_link_over_a_legacy_dir_retires_it_with_contents_intact() {
    let fx = Fixture::new();
    let release = fx.release("1.0.0");
    let link = fx.amp.join("amplifier-bundle");
    fs::create_dir_all(link.join("recipes")).unwrap();
    fs::write(link.join("recipes/legacy.yaml"), "legacy").unwrap();

    switch_link(&fx.amp, &link, &release.join("amplifier-bundle")).unwrap();

    assert_eq!(
        fs::read_link(&link).unwrap(),
        release.join("amplifier-bundle")
    );
    let retired = fx.releases().join(RETIRED_DIR);
    let moved: Vec<String> = names(&retired)
        .into_iter()
        .filter(|n| n.ends_with("-amplifier-bundle"))
        .collect();
    assert_eq!(
        moved.len(),
        1,
        "retired as <ts>-<pid>-amplifier-bundle: {:?}",
        names(&retired)
    );
    assert_eq!(
        fs::read_to_string(retired.join(&moved[0]).join("recipes/legacy.yaml")).unwrap(),
        "legacy"
    );
    assert!(
        !names(&fx.releases())
            .iter()
            .any(|n| n.starts_with("legacy-")),
        "no legacy-* release names"
    );
    assert_eq!(mode(&retired), 0o700, ".retired/ is 0700");
}

#[test]
fn tmp_link_never_removes_regular_file() {
    let fx = Fixture::new();
    let release = fx.release("1.0.0");
    let pid = std::process::id();
    let mut planted = Vec::new();
    for n in 0..16 {
        let path = fx.bin.join(format!(".amplihack.tmp-{pid}-{n}"));
        fs::write(&path, format!("user file {n}")).unwrap();
        planted.push(path);
    }
    let link = fx.bin.join("amplihack");

    let result = switch_link(&fx.amp, &link, &release.join("bin/amplihack"));

    for (n, path) in planted.iter().enumerate() {
        assert_eq!(
            fs::read_to_string(path).ok(),
            Some(format!("user file {n}")),
            "a regular file at a tmp-link name is never removed (S14)"
        );
    }
    // Either it found a free name, or it refused; it never deleted a file.
    if result.is_ok() {
        assert_eq!(fs::read_link(&link).unwrap(), release.join("bin/amplihack"));
    }
}

#[test]
fn a_leftover_tmp_symlink_from_a_crash_is_replaced() {
    let fx = Fixture::new();
    let release = fx.release("1.0.0");
    let pid = std::process::id();
    let stale = fx.bin.join(format!(".amplihack.tmp-{pid}-0"));
    symlink("/nonexistent", &stale).unwrap();

    switch_link(
        &fx.amp,
        &fx.bin.join("amplihack"),
        &release.join("bin/amplihack"),
    )
    .unwrap();

    assert_eq!(
        fs::read_link(fx.bin.join("amplihack")).unwrap(),
        release.join("bin/amplihack")
    );
}

#[test]
fn commit_switches_only_planned_links_in_order() {
    let fx = Fixture::new();
    let old = fx.release("1.0.0");
    let bundle_link = fx.amp.join("amplifier-bundle");
    let amplihack_link = fx.bin.join("amplihack");
    let hooks_link = fx.bin.join("amplihack-hooks");
    symlink(old.join("amplifier-bundle"), &bundle_link).unwrap();
    symlink(old.join("bin/amplihack"), &amplihack_link).unwrap();

    let mut txn = ReleaseTxn::allocate(&fx.amp, "1.0.1").unwrap();
    fs::create_dir_all(txn.bin_dir()).unwrap();
    fs::create_dir_all(txn.bundle_dir().join("recipes")).unwrap();
    fs::write(txn.bin_dir().join("amplihack-hooks"), "h").unwrap();
    fs::write(txn.bundle_dir().join("recipes/r.yaml"), "new").unwrap();
    let root = txn.root().to_path_buf();
    // Planned in reverse; commit still switches hooks before the bundle.
    txn.plan_link(&bundle_link, &txn.bundle_dir()).unwrap();
    txn.plan_link(&hooks_link, &txn.bin_dir().join("amplihack-hooks"))
        .unwrap();

    let report = txn.commit().unwrap();

    assert!(report.failed.is_none(), "{:?}", report.failed);
    let order: Vec<PathBuf> = report.switched.iter().map(|s| s.link.clone()).collect();
    assert_eq!(order, vec![hooks_link.clone(), bundle_link.clone()]);
    assert_eq!(
        fs::read_link(&bundle_link).unwrap(),
        root.join("amplifier-bundle")
    );
    assert_eq!(
        fs::read_link(&amplihack_link).unwrap(),
        old.join("bin/amplihack"),
        "an unplanned link keeps its earlier release"
    );
    assert!(
        root.join(COMPLETE_MARKER).is_file(),
        ".complete written before the switch"
    );
}

#[test]
fn plan_link_refuses_a_target_outside_the_release() {
    let fx = Fixture::new();
    let other = fx.release("0.1.0");
    let mut txn = ReleaseTxn::allocate(&fx.amp, "1.0.0").unwrap();
    assert!(
        txn.plan_link(&fx.bin.join("amplihack"), &other.join("bin/amplihack"))
            .is_err()
    );
    assert!(
        txn.plan_link(&fx.bin.join("amplihack"), Path::new("/bin/sh"))
            .is_err()
    );
}

#[test]
fn a_partial_switch_is_reported_and_not_rolled_back() {
    if euid_is_root() {
        eprintln!("skipped: root ignores directory write permission");
        return;
    }
    let fx = Fixture::new();
    let old = fx.release("1.0.0");
    let locked_bin = fx.home.join("locked-bin");
    fs::create_dir_all(&locked_bin).unwrap();
    let hooks_link = fx.bin.join("amplihack-hooks");
    let amplihack_link = locked_bin.join("amplihack");
    symlink(old.join("bin/amplihack"), &hooks_link).unwrap();
    symlink(old.join("bin/amplihack"), &amplihack_link).unwrap();
    set_mode(&locked_bin, 0o555);

    let mut txn = ReleaseTxn::allocate(&fx.amp, "1.0.1").unwrap();
    fs::create_dir_all(txn.bin_dir()).unwrap();
    fs::write(txn.bin_dir().join("amplihack"), "a").unwrap();
    fs::write(txn.bin_dir().join("amplihack-hooks"), "h").unwrap();
    let new_root = txn.root().to_path_buf();
    txn.plan_link(&hooks_link, &txn.bin_dir().join("amplihack-hooks"))
        .unwrap();
    txn.plan_link(&amplihack_link, &txn.bin_dir().join("amplihack"))
        .unwrap();

    let report = txn.commit().unwrap();
    set_mode(&locked_bin, 0o755);

    assert_eq!(report.switched.len(), 1);
    assert_eq!(report.switched[0].link, hooks_link);
    assert_eq!(report.switched[0].previous, Some(old.join("bin/amplihack")));
    let (failed, _why) = report.failed.clone().expect("the amplihack switch failed");
    assert_eq!(failed, amplihack_link);
    assert_eq!(
        fs::read_link(&hooks_link).unwrap(),
        new_root.join("bin/amplihack-hooks"),
        "no rollback"
    );
    assert_eq!(
        fs::read_link(&amplihack_link).unwrap(),
        old.join("bin/amplihack")
    );

    // The per-link keep rule keeps both releases.
    let previous: Vec<PathBuf> = report
        .switched
        .iter()
        .filter_map(|s| s.previous.clone())
        .collect();
    let keep = keep_set(&fx.releases(), &[hooks_link, amplihack_link], &previous);
    assert!(keep.contains(&old), "{keep:?}");
    assert!(keep.contains(&new_root), "{keep:?}");
}

#[test]
fn release_tree_has_no_group_or_other_write() {
    let fx = Fixture::new();
    let mut txn = ReleaseTxn::allocate(&fx.amp, "1.0.0").unwrap();
    fs::create_dir_all(txn.bundle_dir().join("tools")).unwrap();
    fs::create_dir_all(txn.bin_dir()).unwrap();
    let tool = txn.bundle_dir().join("tools/x.sh");
    fs::write(&tool, "#!/bin/sh\n").unwrap();
    set_mode(&tool, 0o777);
    set_mode(&txn.bundle_dir().join("tools"), 0o777);
    fs::write(txn.bin_dir().join("amplihack"), "a").unwrap();
    set_mode(&txn.bin_dir().join("amplihack"), 0o766);
    let root = txn.root().to_path_buf();
    txn.plan_link(&fx.bin.join("amplihack"), &txn.bin_dir().join("amplihack"))
        .unwrap();

    txn.commit().unwrap();

    assert_eq!(mode(&root), 0o755, "release root is 0755 (S16)");
    for path in [
        root.join("bin"),
        root.join("bin/amplihack"),
        root.join("amplifier-bundle"),
        root.join("amplifier-bundle/tools"),
        tool.clone(),
        root.join(COMPLETE_MARKER),
    ] {
        assert_eq!(
            mode(&path) & 0o022,
            0,
            "{} is writable by group or others ({:o})",
            path.display(),
            mode(&path)
        );
    }
    assert_ne!(mode(&tool) & 0o100, 0, "executable bits are kept");
}

// ------------------------------------------------------- exchange (D3/D4)

#[test]
fn exchange_dirs_swaps_two_directories() {
    let fx = Fixture::new();
    let a = fx.amp.join("a");
    let b = fx.amp.join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(a.join("which"), "a").unwrap();
    fs::write(b.join("which"), "b").unwrap();

    let outcome = exchange_dirs(&a, &b).unwrap();

    assert!(matches!(outcome, Exchange::Exchanged | Exchange::FellBack));
    assert_eq!(fs::read_to_string(a.join("which")).unwrap(), "b");
    assert_eq!(fs::read_to_string(b.join("which")).unwrap(), "a");
}

#[test]
fn exchange_falls_back_only_on_einval_or_enosys() {
    for errno in [libc::EINVAL, libc::ENOSYS] {
        let fx = Fixture::new();
        let a = fx.amp.join("staging");
        let b = fx.amp.join("live");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("which"), "new").unwrap();
        fs::write(b.join("which"), "old").unwrap();

        let outcome =
            exchange_dirs_with(&a, &b, |_, _| Err(std::io::Error::from_raw_os_error(errno)))
                .unwrap();

        assert!(matches!(outcome, Exchange::FellBack), "errno {errno}");
        assert_eq!(fs::read_to_string(b.join("which")).unwrap(), "new");
        assert_eq!(fs::read_to_string(a.join("which")).unwrap(), "old");
    }

    let fx = Fixture::new();
    let a = fx.amp.join("staging");
    let b = fx.amp.join("live");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(b.join("which"), "old").unwrap();
    let err = exchange_dirs_with(&a, &b, |_, _| {
        Err(std::io::Error::from_raw_os_error(libc::EPERM))
    })
    .expect_err("any other errno is returned unchanged (S15)");
    let io = err
        .downcast_ref::<std::io::Error>()
        .or_else(|| err.chain().find_map(|c| c.downcast_ref::<std::io::Error>()));
    assert_eq!(io.and_then(|e| e.raw_os_error()), Some(libc::EPERM));
    assert_eq!(
        fs::read_to_string(b.join("which")).unwrap(),
        "old",
        "nothing moved"
    );
}

#[test]
fn exchange_refuses_foreign_or_symlinked_legacy_dir() {
    let fx = Fixture::new();
    let staging = fx.amp.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let elsewhere = fx.home.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep"), "x").unwrap();
    let legacy = fx.amp.join("legacy");
    symlink(&elsewhere, &legacy).unwrap();

    assert!(exchange_dirs(&staging, &legacy).is_err());
    assert!(
        fs::symlink_metadata(&legacy)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(elsewhere.join("keep")).unwrap(), "x");
    assert!(staging.is_dir());
}

// --------------------------------------------------------------- retire (D4)

#[test]
fn retire_moves_into_retired_with_a_labelled_name() {
    let fx = Fixture::new();
    let old = fx.amp.join(".claude/skills.old");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("s.md"), "s").unwrap();

    let outcome = retire(&fx.amp, &old, "skills").unwrap();

    let Retired::Moved(to) = outcome else {
        panic!("expected Moved")
    };
    assert!(!old.exists());
    assert_eq!(to.parent(), Some(fx.releases().join(RETIRED_DIR).as_path()));
    let name = to.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with("-skills"), "{name}");
    assert!(
        name.contains(&format!("-{}-", std::process::id())),
        "{name}"
    );
    assert_eq!(fs::read_to_string(to.join("s.md")).unwrap(), "s");
}

#[test]
fn retire_on_exdev_leaves_the_path_in_place() {
    let fx = Fixture::new();
    let old = fx.amp.join(".claude/agents.old");
    fs::create_dir_all(&old).unwrap();

    let outcome = retire_with(&fx.amp, &old, "agents", |_, _| {
        Err(std::io::Error::from_raw_os_error(libc::EXDEV))
    })
    .unwrap();

    assert!(matches!(outcome, Retired::LeftInPlace(ref p) if p == &old));
    assert!(old.is_dir(), "never deleted as a fallback");
}

#[test]
fn retire_refuses_paths_outside_amplihack_and_symlinks() {
    let fx = Fixture::new();
    let outside = fx.home.join("projects/mine");
    fs::create_dir_all(&outside).unwrap();
    assert!(retire(&fx.amp, &outside, "mine").is_err());
    assert!(outside.is_dir());

    let link = fx.amp.join(".claude/link");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&outside, &link).unwrap();
    assert!(retire(&fx.amp, &link, "link").is_err());
    assert!(outside.is_dir());
}

// -------------------------------------------------------------- cleanup (D7)

#[test]
fn keep_set_holds_each_links_current_and_previous_release() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let r1 = fx.release("0.9.41");
    let r2 = fx.release("0.9.42");
    let hooks = fx.bin.join("amplihack-hooks");
    let bundle = fx.amp.join("amplifier-bundle");
    symlink(r2.join("bin/amplihack"), &hooks).unwrap();
    symlink(r1.join("amplifier-bundle"), &bundle).unwrap();

    let keep = keep_set(
        &fx.releases(),
        &[hooks, bundle],
        &[r1.join("bin/amplihack")],
    );

    assert!(keep.contains(&r1));
    assert!(keep.contains(&r2));
    assert!(!keep.contains(&r0));
    for path in &keep {
        assert_eq!(
            path.parent(),
            Some(fx.releases().as_path()),
            "release roots only"
        );
    }
}

#[test]
fn cleanup_removes_unused_and_keeps_the_keep_set() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let r1 = fx.release("0.9.41");
    let retired = fx
        .releases()
        .join(RETIRED_DIR)
        .join("20261105T220027Z-1-skills");
    fs::create_dir_all(&retired).unwrap();
    set_mode(&fx.releases().join(RETIRED_DIR), 0o700);

    let report: CleanupReport =
        cleanup_unused_with(&fx.amp, std::slice::from_ref(&r1), |_| known(&[])).unwrap();

    assert!(!r0.exists());
    assert!(!retired.exists());
    assert!(r1.exists());
    assert!(report.removed.contains(&r0));
    assert!(report.kept_in_use.is_empty());
    assert!(!report.unknown);
}

#[test]
fn cleanup_keeps_whatever_a_process_uses() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let r1 = fx.release("0.9.41");
    let report = cleanup_unused_with(&fx.amp, &[], |_| known(&[(&r0, 4242)])).unwrap();
    assert!(r0.exists());
    assert!(!r1.exists());
    assert_eq!(report.kept_in_use, vec![(r0.clone(), vec![4242])]);
}

#[test]
fn cleanup_keeps_everything_when_the_scan_is_unknown_or_unsupported() {
    for scan in [InUse::Unknown, InUse::Unsupported] {
        let fx = Fixture::new();
        let r0 = fx.release("0.9.40");
        let unknown = matches!(scan, InUse::Unknown);
        let report = cleanup_unused_with(&fx.amp, &[], move |_| scan).unwrap();
        assert!(r0.exists());
        assert!(report.removed.is_empty());
        if unknown {
            assert!(report.unknown);
        }
    }
}

#[test]
fn cleanup_removes_read_only_trees() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    set_mode(&r0.join("amplifier-bundle/recipes/r.yaml"), 0o444);
    set_mode(&r0.join("amplifier-bundle/recipes"), 0o555);
    set_mode(&r0.join("amplifier-bundle"), 0o555);
    set_mode(&r0, 0o555);

    cleanup_unused_with(&fx.amp, &[], |_| known(&[])).unwrap();

    assert!(!r0.exists());
}

#[test]
fn cleanup_skips_symlinked_candidate() {
    let fx = Fixture::new();
    fx.release("0.9.41");
    let outside = fx.home.join("precious");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("data"), "keep me").unwrap();
    symlink(&outside, fx.releases().join("0.9.40")).unwrap();
    fs::create_dir_all(fx.releases().join(RETIRED_DIR)).unwrap();
    set_mode(&fx.releases().join(RETIRED_DIR), 0o700);
    symlink(&outside, fx.releases().join(RETIRED_DIR).join("x-1-skills")).unwrap();

    cleanup_unused_with(&fx.amp, &[], |_| known(&[])).unwrap();

    assert_eq!(fs::read_to_string(outside.join("data")).unwrap(), "keep me");
}

#[test]
fn cleanup_ignores_keep_paths_outside_releases() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let outside = fx.home.join("projects");
    fs::create_dir_all(&outside).unwrap();
    // A keep list read back from a manifest may hold anything; an outside path
    // neither protects nor exposes anything.
    let report = cleanup_unused_with(&fx.amp, &[outside.clone(), PathBuf::from("/")], |c| {
        assert!(
            c.iter().all(|p| is_release_path(&releases_dir(&fx.amp), p)
                || p.starts_with(releases_dir(&fx.amp).join(RETIRED_DIR))),
            "only release or retired paths are scan candidates: {c:?}"
        );
        known(&[])
    })
    .unwrap();
    assert!(outside.is_dir());
    assert!(!r0.exists());
    assert!(report.removed.iter().all(|p| p.starts_with(fx.releases())));
}

#[test]
fn make_writable_does_not_follow_symlinks() {
    let fx = Fixture::new();
    let outside = fx.home.join("outside.txt");
    fs::write(&outside, "x").unwrap();
    set_mode(&outside, 0o444);
    let outside_dir = fx.home.join("outside-dir");
    fs::create_dir_all(&outside_dir).unwrap();
    set_mode(&outside_dir, 0o555);
    let tree = fx.releases().join("0.9.40");
    fs::create_dir_all(&tree).unwrap();
    symlink(&outside, tree.join("file-link")).unwrap();
    symlink(&outside_dir, tree.join("dir-link")).unwrap();
    set_mode(&tree, 0o555);

    make_tree_writable(&tree).unwrap();

    assert_eq!(mode(&outside), 0o444);
    assert_eq!(mode(&outside_dir), 0o555);
    assert_ne!(mode(&tree) & 0o200, 0, "the tree itself is made writable");
    set_mode(&outside_dir, 0o755);
}

// ---------------------------------------------------- in-use scan (D7, S22)

#[cfg(target_os = "linux")]
#[test]
fn the_scan_finds_a_child_whose_cwd_is_in_a_release() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let child = OwnChild::spawn(
        Command::new("sleep")
            .arg("30")
            .current_dir(r0.join("amplifier-bundle")),
    );
    wait_for_proc_link(child.pid(), "cwd", &r0);

    match scan_pids(std::slice::from_ref(&r0), &[child.pid()]) {
        InUse::Known(map) => assert_eq!(map.get(&r0), Some(&vec![child.pid()])),
        other => panic!("the child's own /proc entries are readable: {other:?}"),
    }

    let report = cleanup_unused_with(&fx.amp, &[], |c| scan_pids(c, &[child.pid()])).unwrap();
    assert!(r0.exists(), "a release a process works in is kept");
    assert_eq!(report.kept_in_use.len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn the_scan_finds_a_child_with_an_open_fd_in_a_release() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let file = r0.join("amplifier-bundle/recipes/r.yaml");
    let child = OwnChild::spawn(
        Command::new("/bin/sh")
            .args(["-c", "exec 3< \"$1\"; exec sleep 30", "sh"])
            .arg(&file)
            .current_dir(&fx.home),
    );
    wait_for_proc_link(child.pid(), "fd/3", &r0);

    let InUse::Known(map) = scan_pids(std::slice::from_ref(&r0), &[child.pid()]) else {
        panic!("known")
    };
    assert_eq!(map.get(&r0), Some(&vec![child.pid()]));
}

#[cfg(target_os = "linux")]
#[test]
fn in_use_scan_never_returns_env_values() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let secret = format!(
        "{}/secret-token-1954",
        r0.join("amplifier-bundle").display()
    );
    let child = OwnChild::spawn(
        Command::new("sleep")
            .arg("30")
            .env("ISSUE_1954_SECRET", &secret)
            .current_dir(&fx.home),
    );
    wait_for_proc_link(child.pid(), "cwd", &fx.home);

    let result = scan_pids(std::slice::from_ref(&r0), &[child.pid()]);

    let InUse::Known(ref map) = result else {
        panic!("known: {result:?}")
    };
    assert_eq!(
        map.get(&r0),
        Some(&vec![child.pid()]),
        "an environment value naming the release counts as use"
    );
    let shown = format!("{result:?}");
    assert!(!shown.contains("secret-token-1954"), "{shown}");
    assert!(!shown.contains("ISSUE_1954_SECRET"), "{shown}");
}

#[cfg(target_os = "linux")]
#[test]
fn the_scan_of_an_exited_pid_is_not_unknown() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(matches!(
        scan_pids(std::slice::from_ref(&r0), &[pid]),
        InUse::Known(ref m) if m.is_empty()
    ));
}

// ---------------------------------------------------------- message text

#[test]
fn kept_in_use_warning_is_exact_and_only_above_five() {
    assert_eq!(kept_in_use_warning(0), None);
    assert_eq!(kept_in_use_warning(5), None);
    assert_eq!(
        kept_in_use_warning(7).as_deref(),
        Some(
            "⚠️  7 amplihack releases kept because running processes use them \
             (~/.amplihack/releases); they are removed by a later install once unused"
        )
    );
}

#[test]
fn uninstall_kept_list_is_exact() {
    let home = Path::new("/home/u");
    assert_eq!(format_kept_in_use(home, &[]), None);
    assert_eq!(
        format_kept_in_use(
            home,
            &[(
                PathBuf::from("/home/u/.amplihack/releases/0.9.41"),
                vec![48113]
            )]
        )
        .as_deref(),
        Some(
            "ℹ Kept 1 release in use by running processes:\n\
             \x20   ~/.amplihack/releases/0.9.41 (pid 48113)\n\
             \x20 Run `amplihack uninstall` again after those processes exit to remove it."
        )
    );
    let two = format_kept_in_use(
        home,
        &[
            (PathBuf::from("/home/u/.amplihack/releases/0.9.41"), vec![1]),
            (PathBuf::from("/home/u/.amplihack/releases/0.9.42"), vec![2]),
        ],
    )
    .unwrap();
    assert!(
        two.starts_with("ℹ Kept 2 releases in use by running processes:\n"),
        "{two}"
    );
    assert!(two.ends_with("to remove them."), "{two}");
}

// ------------------------------------------------------- bundle_compat (D9)

/// A committed release whose bundle passes the compatibility check, with
/// `~/.amplihack/amplifier-bundle` linked to it.
fn linked_release_bundle(fx: &Fixture, name: &str) -> PathBuf {
    let root = fx.releases().join(name);
    create_source_repo(&root);
    fs::write(root.join(COMPLETE_MARKER), "").unwrap();
    set_mode(&fx.releases(), 0o700);
    symlink(
        root.join("amplifier-bundle"),
        fx.amp.join("amplifier-bundle"),
    )
    .unwrap();
    root
}

#[test]
fn bundle_compat_accepts_a_link_to_a_complete_release() {
    let fx = Fixture::new();
    let root = linked_release_bundle(&fx, "1.0.0");
    validate_framework_bundle_compatibility(&root.join("amplifier-bundle"))
        .expect("the release bundle itself is valid");
    validate_framework_bundle_compatibility(&fx.amp)
        .expect("a top-level link into a complete release is accepted");
}

#[test]
fn bundle_compat_rejects_a_link_to_an_incomplete_release() {
    let fx = Fixture::new();
    let root = linked_release_bundle(&fx, "1.0.0");
    fs::remove_file(root.join(COMPLETE_MARKER)).unwrap();
    assert!(validate_framework_bundle_compatibility(&fx.amp).is_err());
}

#[test]
fn bundle_compat_rejects_a_link_outside_releases() {
    let fx = Fixture::new();
    let outside = fx.home.join("src-tree");
    create_source_repo(&outside);
    symlink(
        outside.join("amplifier-bundle"),
        fx.amp.join("amplifier-bundle"),
    )
    .unwrap();
    assert!(validate_framework_bundle_compatibility(&fx.amp).is_err());
}

#[test]
fn bundle_compat_rejects_inner_symlink_in_release() {
    let fx = Fixture::new();
    let root = linked_release_bundle(&fx, "1.0.0");
    let recipes = root.join("amplifier-bundle/recipes");
    let moved = root.join("recipes-real");
    fs::rename(&recipes, &moved).unwrap();
    symlink(&moved, &recipes).unwrap();
    assert!(validate_framework_bundle_compatibility(&fx.amp).is_err());
}

#[test]
fn release_layout_module_names_its_constants() {
    assert_eq!(release_layout::RELEASES_DIR, "releases");
    assert_eq!(RETIRED_DIR, ".retired");
    assert_eq!(COMPLETE_MARKER, ".complete");
}
