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
    self, COMPLETE_MARKER, CleanupReport, Exchange, INSTALLED_BY, InUse, InstalledBy, RETIRED_DIR,
    ReleaseTxn, Retired, cleanup_unused_with, exchange_dirs, exchange_dirs_with,
    format_installed_by, format_kept_in_use, is_release_path, keep_set, kept_in_use_warning,
    make_tree_writable, read_installed_by, releases_dir, retire, retire_with, scan_pids,
    switch_link, validate_version, write_installed_by,
};
use super::helpers::create_source_repo;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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

/// A child this test started that holds a working directory and fd 3 open
/// until it is released. It says `ready` on stdout once both are held (and
/// after `exec`, so `/proc/<pid>/environ` is its own); the test blocks on
/// that line, no timer. Dropping it closes the child's stdin, which ends its
/// `read`, then reaps it with `wait()`. No signal is sent.
struct Holder(Child);

impl Holder {
    /// Holds `dir` as cwd and `file` open on fd 3. Paths reach `sh` only as
    /// positional arguments (S-V7).
    fn spawn(dir: &Path, file: &Path, env: &[(&str, &str)]) -> Self {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "cd \"$1\" || exit 1; exec 3<\"$2\" || exit 1; echo ready; read _",
                "sh",
            ])
            .arg(dir)
            .arg(file);
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        let holder = Self(child);
        assert_eq!(
            line,
            "ready\n",
            "the holder must say ready after taking {} and {}",
            dir.display(),
            file.display()
        );
        holder
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        // Closing stdin ends the child's `read`; it then exits by itself.
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
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
        assert_eq!(report.unknown, unknown);
        assert_eq!(
            report.unsupported, !unknown,
            "the platform case is reported too"
        );
    }
}

#[test]
fn unsupported_scan_has_its_own_notice() {
    let notice = release_layout::unsupported_scan_notice();
    assert!(notice.contains("no release was removed"), "{notice}");
    assert_ne!(notice, release_layout::unknown_scan_notice());
}

#[cfg(target_os = "linux")]
mod opaque_process {
    use super::release_layout::{birth_time, may_predate, proc_scan, scan_with};
    use super::{Fixture, cleanup_unused_with};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn start_ticks_are_counted_from_the_last_parenthesis() {
        let stat = "3384 ((sd-pam) x) S 3363 3363 3363 0 -1 4194624 41 0 0 0 0 0 0 0 20 0 1 0 1558 \
                    1 2 3";
        assert_eq!(proc_scan::start_ticks(stat), Some(1558));
        assert_eq!(proc_scan::start_ticks("garbage"), None);
    }

    #[test]
    fn boot_time_is_read_from_the_btime_line() {
        assert_eq!(
            proc_scan::boot_time_secs("cpu 1 2 3\nbtime 1762588000\nprocesses 9\n"),
            Some(1_762_588_000)
        );
        assert_eq!(proc_scan::boot_time_secs("cpu 1 2 3\n"), None);
    }

    #[test]
    fn only_a_release_created_after_the_process_started_is_free() {
        let started = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(may_predate(
            Some(started - Duration::from_secs(60)),
            started
        ));
        assert!(!may_predate(
            Some(started + Duration::from_secs(60)),
            started
        ));
        assert!(may_predate(Some(started), started), "a tie is kept");
        assert!(may_predate(None, started), "an unknown time is kept");
    }

    #[test]
    fn creation_time_never_falls_back_to_a_later_mtime() {
        // A write into a release after commit moves its mtime forward. If
        // that time stood in for a missing birth time, the release would look
        // newer than an opaque process inside it and be removed. The birth
        // time is either the real one (not the moved mtime) or unknown.
        let fx = Fixture::new();
        let release = fx.release("0.9.40");
        let before = std::time::SystemTime::now();
        let future = before + Duration::from_secs(10 * 365 * 24 * 3600);
        std::fs::File::open(&release)
            .unwrap()
            .set_modified(future)
            .unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&release)
                .unwrap()
                .modified()
                .unwrap(),
            future
        );
        match birth_time(&release) {
            None => {}
            Some(born) => assert!(born <= before, "birth time {born:?} is the moved mtime"),
        }
        assert_eq!(birth_time(&fx.amp.join("no-such-release")), None);
    }

    #[test]
    fn an_opaque_process_keeps_the_releases_that_existed_when_it_started() {
        // A setgid or file-capability child (`ping`) of a recipe step whose
        // working directory is in the older release is non-dumpable. It
        // started after that release was created, so it may have inherited
        // its cwd there: the release is kept, privileges or not. The newer
        // release did not exist when it started and is removed.
        let fx = Fixture::new();
        let old = fx.release("0.9.40");
        let new = fx.release("0.9.41");
        let started = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let report = cleanup_unused_with(&fx.amp, &[], |candidates| {
            let created: Vec<_> = candidates
                .iter()
                .map(|c| {
                    Some(if *c == old {
                        started - Duration::from_secs(3600)
                    } else {
                        started + Duration::from_secs(3600)
                    })
                })
                .collect();
            scan_with(candidates, &created, &[777], |_| {
                Err(proc_scan::Opaque::StartedAt(started))
            })
        })
        .unwrap();
        assert!(old.exists(), "existed when the process started: kept");
        assert!(!new.exists(), "created after the process started: removed");
        assert_eq!(report.removed, vec![new]);
        assert_eq!(report.kept_in_use, vec![(old, vec![777])]);
        assert!(!report.unknown);
    }

    // #2024: one release and two non-dumpable same-user processes. Only the
    // process that started after the release existed may have inherited a
    // working directory, descriptor or environment value inside it, so only
    // that process keeps it. The one that started before the release existed
    // must not keep it, or no release would ever be removed while a
    // long-running non-dumpable process is alive.
    const BORN_SECS: u64 = 1_000_000;
    const BEFORE_PID: u32 = 101;
    const AFTER_PID: u32 = 202;

    fn started_at(pid: u32) -> proc_scan::Opaque {
        let born = UNIX_EPOCH + Duration::from_secs(BORN_SECS);
        match pid {
            BEFORE_PID => proc_scan::Opaque::StartedAt(born - Duration::from_secs(3600)),
            AFTER_PID => proc_scan::Opaque::StartedAt(born + Duration::from_secs(3600)),
            other => unreachable!("the scan asked about pid {other}, which the test never listed"),
        }
    }

    #[test]
    fn a_non_dumpable_process_started_after_a_release_keeps_it_and_one_started_before_does_not() {
        let fx = Fixture::new();
        let candidate = fx.release("0.9.40");
        let born = UNIX_EPOCH + Duration::from_secs(BORN_SECS);
        let report = cleanup_unused_with(&fx.amp, &[], |candidates| {
            assert_eq!(candidates, std::slice::from_ref(&candidate));
            scan_with(candidates, &[Some(born)], &[BEFORE_PID, AFTER_PID], |pid| {
                Err(started_at(pid))
            })
        })
        .unwrap();
        assert!(candidate.exists(), "the later process may be inside it");
        assert!(report.removed.is_empty(), "{:?}", report.removed);
        assert_eq!(
            report.kept_in_use,
            vec![(candidate, vec![AFTER_PID])],
            "only the process started after the release keeps it"
        );
        assert!(!report.unknown);
    }

    #[test]
    fn a_non_dumpable_process_started_before_a_release_does_not_keep_it() {
        let fx = Fixture::new();
        let candidate = fx.release("0.9.40");
        let born = UNIX_EPOCH + Duration::from_secs(BORN_SECS);
        let report = cleanup_unused_with(&fx.amp, &[], |candidates| {
            assert_eq!(candidates, std::slice::from_ref(&candidate));
            scan_with(candidates, &[Some(born)], &[BEFORE_PID], |pid| {
                Err(started_at(pid))
            })
        })
        .unwrap();
        assert!(!candidate.exists(), "the process predates the release");
        assert_eq!(report.removed, vec![candidate]);
        assert!(report.kept_in_use.is_empty(), "{:?}", report.kept_in_use);
        assert!(!report.unknown);
    }

    #[test]
    fn the_user_manager_keeps_no_release() {
        // systemd --user (linger on) started at boot, before both releases.
        // It is non-dumpable, but its cgroup marks it as the manager, which
        // runs from `/` and system binaries: it must not pin either release.
        let fx = Fixture::new();
        let r0 = fx.release("0.9.40");
        let r1 = fx.release("0.9.41");
        let boot = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let report = cleanup_unused_with(&fx.amp, &[], |candidates| {
            let created = vec![Some(boot + Duration::from_secs(3600)); candidates.len()];
            scan_with(candidates, &created, &[3363], |_| {
                Err(proc_scan::Opaque::UserManager)
            })
        })
        .unwrap();
        assert!(!r0.exists());
        assert!(!r1.exists());
        assert!(report.kept_in_use.is_empty());
        assert!(!report.unknown);
    }

    #[test]
    fn only_the_exact_user_manager_cgroup_is_exempt() {
        let manager = "0::/user.slice/user-1000.slice/user@1000.service/init.scope\n";
        assert!(proc_scan::is_user_manager_cgroup(manager, 1000));
        // Another user's manager.
        assert!(!proc_scan::is_user_manager_cgroup(manager, 1001));
        // What the manager starts: recipe runs, tmux, terminals.
        assert!(!proc_scan::is_user_manager_cgroup(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-1.scope\n",
            1000
        ));
        assert!(!proc_scan::is_user_manager_cgroup(
            "0::/user.slice/user-1000.slice/session-3.scope\n",
            1000
        ));
        // A container's systemd nested below the user manager.
        assert!(!proc_scan::is_user_manager_cgroup(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/docker-1.scope/init.scope\n",
            1000
        ));
        // cgroup v1 / hybrid: several hierarchies, not exempt.
        assert!(!proc_scan::is_user_manager_cgroup(
            "1:name=systemd:/user.slice/user-1000.slice/user@1000.service/init.scope\n\
             0::/user.slice/user-1000.slice/user@1000.service/init.scope\n",
            1000
        ));
        assert!(!proc_scan::is_user_manager_cgroup("", 1000));
    }

    #[test]
    fn an_opaque_process_with_no_start_time_keeps_every_release() {
        let fx = Fixture::new();
        let r0 = fx.release("0.9.40");
        let r1 = fx.release("0.9.41");
        let report = cleanup_unused_with(&fx.amp, &[], |candidates| {
            scan_with(candidates, &vec![None; candidates.len()], &[3], |_| {
                Err(proc_scan::Opaque::Unknown)
            })
        })
        .unwrap();
        assert!(r0.exists());
        assert!(r1.exists());
        assert!(report.removed.is_empty());
        assert!(report.unknown);
    }

    #[test]
    fn this_hosts_opaque_processes_never_force_an_unknown_scan() {
        // Every same-user process whose fd table is unreadable must be
        // classified from status and stat, so cleanup is not blocked.
        let me = std::fs::metadata("/proc/self").map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid()
        });
        let Ok(me) = me else { return };
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            use std::os::unix::fs::MetadataExt;
            let Ok(meta) = entry.metadata() else { continue };
            if meta.uid() != me || std::fs::read_dir(entry.path().join("fd")).is_ok() {
                continue;
            }
            assert_ne!(
                proc_scan::classify_opaque(pid),
                proc_scan::Opaque::Unknown,
                "pid {pid}"
            );
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
    let child = Holder::spawn(&r0.join("amplifier-bundle"), Path::new("/dev/null"), &[]);

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
    let child = Holder::spawn(&fx.home, &file, &[]);

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
    // `ready` comes after exec, so the scan sees the child's own environment.
    let child = Holder::spawn(
        &fx.home,
        Path::new("/dev/null"),
        &[("ISSUE_1954_SECRET", secret.as_str())],
    );

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

/// A released holder exits by itself (its `read` sees end of file), and the
/// scan no longer counts it: the release is removable afterwards.
#[cfg(target_os = "linux")]
#[test]
fn a_released_holder_no_longer_keeps_its_release() {
    let fx = Fixture::new();
    let r0 = fx.release("0.9.40");
    let child = Holder::spawn(&r0.join("amplifier-bundle"), Path::new("/dev/null"), &[]);
    let pid = child.pid();
    drop(child);

    let report = cleanup_unused_with(&fx.amp, &[], |c| scan_pids(c, &[pid])).unwrap();
    assert!(
        !r0.exists(),
        "nothing uses the release once the holder exited"
    );
    assert_eq!(report.removed, vec![r0]);
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

// ------------------------------------------------- resolve_bundle_link (S7)

#[test]
fn resolve_bundle_link_accepts_a_link_into_a_complete_release() {
    let fx = Fixture::new();
    let root = fx.release("1.0.0");
    let link = fx.amp.join("amplifier-bundle");
    symlink(root.join("amplifier-bundle"), &link).unwrap();
    assert_eq!(
        release_layout::resolve_bundle_link(&link),
        Some(fs::canonicalize(root.join("amplifier-bundle")).unwrap())
    );
}

/// A source checkout with `releases -> /elsewhere` must not have the tree
/// behind it staged, even when that tree looks like a complete release.
#[test]
fn resolve_bundle_link_rejects_a_symlinked_releases_dir() {
    let temp = tempfile::tempdir().unwrap();
    let elsewhere = temp.path().join("elsewhere");
    let release = elsewhere.join("x");
    fs::create_dir_all(release.join("amplifier-bundle/recipes")).unwrap();
    fs::write(release.join(COMPLETE_MARKER), "").unwrap();
    set_mode(&elsewhere, 0o700);

    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    symlink(&elsewhere, repo.join("releases")).unwrap();
    let link = repo.join("amplifier-bundle");
    symlink("releases/x/amplifier-bundle", &link).unwrap();

    assert!(link.is_dir(), "the fixture link must resolve");
    assert_eq!(release_layout::resolve_bundle_link(&link), None);
}

#[test]
fn resolve_bundle_link_rejects_a_group_writable_releases_dir() {
    let fx = Fixture::new();
    let root = fx.release("1.0.0");
    let link = fx.amp.join("amplifier-bundle");
    symlink(root.join("amplifier-bundle"), &link).unwrap();
    set_mode(&fx.releases(), 0o770);
    assert_eq!(release_layout::resolve_bundle_link(&link), None);
}

// ------------------------------------------- provenance record (R6, C6)
//
// Contract:
//
// ```text
// const INSTALLED_BY = ".installed-by";
// struct InstalledBy { version: String, exe: Option<PathBuf>, pid: u32, post_update: bool }
// fn read_installed_by(release_root) -> Option<InstalledBy>
// fn format_installed_by(&InstalledBy) -> String     (the exact file text)
// fn write_installed_by(release_root, version) -> anyhow::Result<()>
// ReleaseTxn::commit writes it before `.complete`.
// ```
//
// The record is evidence of which process committed a release (which binary
// re-stages on the first update). It is not a trust input: production code
// never reads it.

fn record(version: &str, exe: Option<&str>, pid: u32, post_update: bool) -> InstalledBy {
    InstalledBy {
        version: version.to_string(),
        exe: exe.map(PathBuf::from),
        pid,
        post_update,
    }
}

#[test]
fn installed_by_text_is_exact() {
    assert_eq!(INSTALLED_BY, ".installed-by");
    assert_eq!(
        format_installed_by(&record(
            "1.2.108",
            Some("/h/.local/bin/amplihack"),
            4242,
            true
        )),
        "format=1\nversion=1.2.108\nexe=/h/.local/bin/amplihack\npid=4242\npost_update=1\n"
    );
    assert_eq!(
        format_installed_by(&record("1.2.108", None, 7, false)),
        "format=1\nversion=1.2.108\nexe=\npid=7\npost_update=0\n"
    );
}

/// S-V1/S-V2: an exe path that could forge a line is written empty.
#[test]
fn installed_by_drops_an_exe_that_could_forge_a_line() {
    for bad in [
        "/h/bin/amp\nversion=9.9.9",
        "/h/bin/amp\rpid=1",
        "/h/bin/amp\0x",
    ] {
        let text = format_installed_by(&record("1.2.108", Some(bad), 5, false));
        assert!(
            text.contains("\nexe=\n"),
            "{bad:?} must be written as an empty exe: {text:?}"
        );
        assert!(!text.contains("9.9.9"), "{text:?}");
        assert_eq!(text.lines().count(), 5, "{text:?}");
    }
}

#[test]
fn installed_by_round_trips() {
    let fx = Fixture::new();
    let root = fx.release("1.2.108");
    let rec = record("1.2.108", Some("/h/.local/bin/amplihack"), 4242, true);
    fs::write(root.join(INSTALLED_BY), format_installed_by(&rec)).unwrap();
    assert_eq!(read_installed_by(&root), Some(rec));

    let rec = record("1.2.108", None, 7, false);
    fs::write(root.join(INSTALLED_BY), format_installed_by(&rec)).unwrap();
    assert_eq!(
        read_installed_by(&root),
        Some(rec),
        "an empty exe reads as None"
    );
}

/// S-V5/S-V6: the reader never panics and accepts only a sane regular file.
#[test]
fn read_installed_by_rejects_bad_input() {
    let fx = Fixture::new();
    let root = fx.release("1.2.108");
    let path = root.join(INSTALLED_BY);

    assert_eq!(read_installed_by(&root), None, "missing");

    for garbage in [
        "",
        "\0\0\0",
        "not a record",
        "format=1\nversion=\npid=1\n",
        "format=1\nversion=1.2.108\npid=not-a-number\n",
        "format=1\nversion=1.2.108\n",
    ] {
        fs::write(&path, garbage).unwrap();
        assert_eq!(read_installed_by(&root), None, "{garbage:?}");
    }

    let mut big = String::from("format=1\nversion=1.2.108\npid=1\n");
    big.push_str(&"x".repeat(8 * 1024));
    fs::write(&path, &big).unwrap();
    assert_eq!(read_installed_by(&root), None, "larger than 4 KiB");

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(read_installed_by(&root), None, "a directory");
    fs::remove_dir(&path).unwrap();

    let elsewhere = fx.home.join("planted");
    fs::write(
        &elsewhere,
        "format=1\nversion=1.2.108\nexe=/x\npid=1\npost_update=1\n",
    )
    .unwrap();
    symlink(&elsewhere, &path).unwrap();
    assert_eq!(read_installed_by(&root), None, "a symlink is not followed");
}

#[test]
fn read_installed_by_keeps_the_first_key_and_ignores_unknown_ones() {
    let fx = Fixture::new();
    let root = fx.release("1.2.108");
    fs::write(
        root.join(INSTALLED_BY),
        "format=1\nversion=1.2.108\nfuture=x=y\nversion=6.6.6\nexe=/a\npid=9\npid=10\n\
         post_update=1\n",
    )
    .unwrap();
    assert_eq!(
        read_installed_by(&root),
        Some(record("1.2.108", Some("/a"), 9, true))
    );
}

/// S-V3: a symlink planted at the temporary name is removed, not followed,
/// and the record is written.
#[test]
fn write_installed_by_never_follows_a_planted_tmp_symlink() {
    let fx = Fixture::new();
    let root = fx.releases().join("1.2.108");
    fs::create_dir_all(&root).unwrap();
    let victim = fx.home.join("victim");
    fs::write(&victim, "keep").unwrap();
    symlink(&victim, root.join(".installed-by.tmp")).unwrap();

    write_installed_by(&root, "1.2.108").unwrap();

    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert!(fs::symlink_metadata(root.join(".installed-by.tmp")).is_err());
    let rec = read_installed_by(&root).expect("a record");
    assert_eq!(rec.version, "1.2.108");
    assert_eq!(rec.pid, std::process::id());
}

/// A stale regular-file temp from a crash is replaced; a directory there is
/// never removed and the write fails.
#[test]
fn write_installed_by_replaces_a_stale_tmp_file_but_never_removes_a_dir() {
    let fx = Fixture::new();
    let root = fx.releases().join("1.2.108");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".installed-by.tmp"), "stale").unwrap();
    write_installed_by(&root, "1.2.108").unwrap();
    assert!(read_installed_by(&root).is_some());

    let other = fx.releases().join("1.2.109");
    fs::create_dir_all(other.join(".installed-by.tmp/inner")).unwrap();
    assert!(write_installed_by(&other, "1.2.109").is_err());
    assert!(
        other.join(".installed-by.tmp/inner").is_dir(),
        "a directory at the temp name is left alone"
    );
    assert!(read_installed_by(&other).is_none());
}

#[test]
fn write_installed_by_records_this_process() {
    let fx = Fixture::new();
    let root = fx.releases().join("1.2.108");
    fs::create_dir_all(&root).unwrap();
    write_installed_by(&root, "1.2.108").unwrap();
    let rec = read_installed_by(&root).expect("a record");
    assert_eq!(rec.pid, std::process::id());
    let me = fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    assert_eq!(rec.exe.as_deref(), Some(me.as_path()));
    assert_eq!(mode(&root.join(INSTALLED_BY)) & 0o777, 0o644);
}

/// `post_update` records `AMPLIHACK_POST_UPDATE_INSTALL == "1"` and nothing
/// else (S-A5).
#[test]
fn write_installed_by_records_the_post_update_hand_off() {
    let fx = Fixture::new();
    let mut env = super::issue_1526_implicit_downgrade::IsolatedHome::new(&fx.home);
    for (value, expected) in [(Some("1"), true), (Some("yes"), false), (None, false)] {
        match value {
            Some(v) => env.set("AMPLIHACK_POST_UPDATE_INSTALL", v),
            None => env.unset("AMPLIHACK_POST_UPDATE_INSTALL"),
        }
        let root = fx
            .releases()
            .join(format!("1.2.108-case-{}", value.unwrap_or("unset")));
        fs::create_dir_all(&root).unwrap();
        write_installed_by(&root, "1.2.108").unwrap();
        assert_eq!(
            read_installed_by(&root).map(|r| r.post_update),
            Some(expected),
            "AMPLIHACK_POST_UPDATE_INSTALL={value:?}"
        );
    }
}

/// A committed release carries the record, written before `.complete` and
/// made read-only with the rest of the tree.
#[test]
fn a_committed_release_carries_its_provenance_record() {
    let fx = Fixture::new();
    let mut txn = ReleaseTxn::allocate(&fx.amp, "1.2.108").unwrap();
    fs::create_dir_all(txn.bundle_dir()).unwrap();
    fs::write(txn.bundle_dir().join("r.yaml"), "r").unwrap();
    let link = fx.amp.join("amplifier-bundle");
    let bundle = txn.bundle_dir();
    txn.plan_link(&link, &bundle).unwrap();
    let root = txn.root().to_path_buf();
    txn.commit().unwrap();

    let text = fs::read_to_string(root.join(INSTALLED_BY)).unwrap();
    assert!(text.starts_with("format=1\nversion=1.2.108\n"), "{text:?}");
    let rec = read_installed_by(&root).expect("a record");
    assert_eq!(rec.version, "1.2.108");
    assert_eq!(rec.pid, std::process::id());
    assert_eq!(mode(&root.join(INSTALLED_BY)) & 0o777, 0o444);
    assert!(root.join(COMPLETE_MARKER).is_file());
}

/// A failed record write fails the commit before `.complete` and before any
/// link switch, and the unpublished release is removed on drop.
#[test]
fn a_failed_record_write_rolls_the_release_back() {
    let fx = Fixture::new();
    let mut txn = ReleaseTxn::allocate(&fx.amp, "1.2.108").unwrap();
    fs::create_dir_all(txn.bundle_dir()).unwrap();
    // A directory at the temp name makes the write fail (it is never removed).
    fs::create_dir_all(txn.root().join(".installed-by.tmp")).unwrap();
    let link = fx.amp.join("amplifier-bundle");
    let bundle = txn.bundle_dir();
    txn.plan_link(&link, &bundle).unwrap();
    let root = txn.root().to_path_buf();

    assert!(txn.commit().is_err(), "the commit must fail");
    assert!(!root.exists(), "the never-published release is removed");
    assert!(
        fs::symlink_metadata(&link).is_err(),
        "no link was switched to a release without a record"
    );
}

// --------------------------------------- old binaries after the update (A4)

/// A4: an older binary still running after the update (for example a 1.2.107
/// process that started before it) must not re-stage over the newer
/// release. This pins this release's #1526 guard against the release
/// layout: the guard refuses before the installer runs, so `releases/`, the
/// bundle link and `.retired/` are unchanged. The installer passed here
/// would make a release and switch the link, so any call shows up below.
#[test]
fn self_heal_from_older_binary_leaves_newer_release_alone() {
    use super::issue_1526_implicit_downgrade::{
        IsolatedHome, args, assert_home_is, changes, newer_version, snapshot,
    };
    use crate::self_heal::ensure_assets_match_binary_version_with;

    let fx = Fixture::new();
    let _env = IsolatedHome::new(&fx.home);
    let newer = newer_version();
    let release = fx.release(&newer);
    let link = fx.amp.join("amplifier-bundle");
    symlink(release.join("amplifier-bundle"), &link).unwrap();
    super::super::version_stamp::write_installed_version(&newer).unwrap();
    let before = snapshot(&fx.home);

    let calls = std::cell::Cell::new(0u32);
    let mut notice = Vec::new();
    assert_home_is(&fx.home);
    let result = ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "run", "x"]),
        &mut notice,
        || {
            calls.set(calls.get() + 1);
            let mut txn = ReleaseTxn::allocate(&fx.amp, crate::VERSION)?;
            fs::create_dir_all(txn.bundle_dir())?;
            let bundle = txn.bundle_dir();
            txn.plan_link(&link, &bundle)?;
            txn.commit()?;
            Ok(())
        },
    );

    result.expect("a refusal is not a failure");
    assert_eq!(
        calls.get(),
        0,
        "v{} over a v{newer} stamp must not re-stage; notice:\n{}",
        crate::VERSION,
        String::from_utf8_lossy(&notice)
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        release.join("amplifier-bundle"),
        "the bundle link still names the newer release"
    );
    assert_eq!(names(&fx.releases()), vec![newer.clone()]);
    assert!(!fx.releases().join(RETIRED_DIR).exists());
    assert_eq!(changes(&before, &snapshot(&fx.home)), Vec::<String>::new());
}
