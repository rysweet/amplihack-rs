//! Issue #2006: lease files, the lock, and the sweep.
//!
//! Wired from `account/mod.rs` as `#[cfg(all(test, unix))] mod tests_lease;`.
//!
//! Contract (docs/reference/account-pick.md, "Leases"):
//!
//! - `lease::Lease { version: u32, cli: Cli, home: PathBuf, pid: Option<u32>,
//!   created_at: i64 }`, `Lease::new(cli, home, pid, created_at)` with
//!   version 1, serialised as
//!   `{"version":1,"cli":"codex","home":"…","pid":48211|null,"created_at":…}`.
//! - `lease::LeaseDir::open(root, cli) -> Result<LeaseDir, PickError>`
//!   creates `<root>` and `<root>/<cli>` with mode 0700 when missing, and
//!   refuses (`PickError::Io`) a symlink, a directory of another owner, or a
//!   group- or world-writable one. It never repairs anything.
//!   `LeaseDir::dir() -> &Path` is `<root>/<cli>`.
//! - `LeaseDir::lock() -> Result<LeaseLock<'_>, PickError>`: a blocking
//!   exclusive `flock` on `<root>/<cli>/.lock`, never following a symlink.
//! - `LeaseLock::load() -> (Vec<Stored>, Vec<String>)` reads names matching
//!   `^[0-9]+-[0-9]+-[0-9]+\.json$` only. A file with an unknown version or
//!   unparseable content is left in place, not returned, and named once in
//!   the warnings. `Stored { path: PathBuf, lease: Lease }`.
//! - `LeaseLock::write(&Lease) -> Result<PathBuf, PickError>` writes
//!   `<pid-or-0>-<nanos>-<seq>.json`, mode 0600, by temp file and rename.
//! - `LeaseLock::sweep(stored, &JobCounts, now, ttl_secs, alive) -> Vec<Stored>`
//!   deletes, and leaves out of the result, every lease whose pid has
//!   exited, whose pid is a counted job on the same home or one of its
//!   ancestors, or whose age is at least `ttl_secs`. A pid outside
//!   `1..=i32::MAX` is malformed: never passed to `alive` and never counted.
//! - `lease::pid_alive(pid) -> bool`: `kill(pid, 0)`, with `EPERM` alive.
//! - `lease::remove(path)`: `NotFound` is success.
//!
//! `alive` and `now` are injected; nothing waits on a clock.

use super::Cli;
use super::lease::{self, Lease, LeaseDir, Stored};
use super::pick::PickError;
use super::procscan::{Job, JobCounts};
use super::test_fixtures::*;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TTL: u64 = 120;

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    homes: Homes,
}

impl Fx {
    fn new() -> Self {
        let tmp = private_tempdir();
        let root = tmp.path().join("leases");
        Self {
            _tmp: tmp,
            root,
            homes: Homes::new(),
        }
    }

    fn dir(&self) -> LeaseDir {
        LeaseDir::open(&self.root, Cli::Codex).expect("open the lease directory")
    }
}

fn no_jobs() -> JobCounts {
    JobCounts::default()
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    v.sort();
    v
}

fn always(_: u32) -> bool {
    true
}

fn never(_: u32) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Directory checks
// ---------------------------------------------------------------------------

#[test]
fn open_creates_private_directories() {
    let fx = Fx::new();
    let d = fx.dir();
    assert_eq!(d.dir(), fx.root.join("codex"));
    for p in [&fx.root, &fx.root.join("codex")] {
        let m = fs::metadata(p).unwrap();
        assert!(m.is_dir());
        assert_eq!(m.mode() & 0o777, 0o700, "{}", p.display());
    }
    let claude = LeaseDir::open(&fx.root, Cli::Claude).unwrap();
    assert_eq!(claude.dir(), fx.root.join("claude"));
}

#[test]
fn a_symlinked_lease_directory_is_refused() {
    let fx = Fx::new();
    let real = fx._tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&real, &fx.root).unwrap();
    let err = LeaseDir::open(&fx.root, Cli::Codex).expect_err("refused");
    assert!(matches!(err, PickError::Io { .. }), "{err:?}");
    assert!(
        !real.join("codex").exists(),
        "nothing created through the link"
    );
}

#[test]
fn a_symlinked_cli_directory_is_refused() {
    let fx = Fx::new();
    fs::create_dir(&fx.root).unwrap();
    fs::set_permissions(&fx.root, fs::Permissions::from_mode(0o700)).unwrap();
    let real = fx._tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&real, fx.root.join("codex")).unwrap();
    assert!(matches!(
        LeaseDir::open(&fx.root, Cli::Codex),
        Err(PickError::Io { .. })
    ));
}

#[test]
fn a_group_or_world_writable_directory_is_refused_and_not_repaired() {
    for mode in [0o770, 0o707, 0o777] {
        let fx = Fx::new();
        fs::create_dir(&fx.root).unwrap();
        fs::set_permissions(&fx.root, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            matches!(
                LeaseDir::open(&fx.root, Cli::Codex),
                Err(PickError::Io { .. })
            ),
            "mode {mode:o} accepted"
        );
        let after = fs::metadata(&fx.root).unwrap().mode() & 0o777;
        assert_eq!(after, mode, "the picker changed the mode");
    }
}

#[test]
fn a_symlinked_lock_file_is_not_followed() {
    let fx = Fx::new();
    let d = fx.dir();
    let target = fx._tmp.path().join("target");
    fs::write(&target, b"untouched").unwrap();
    std::os::unix::fs::symlink(&target, d.dir().join(".lock")).unwrap();
    assert!(matches!(d.lock(), Err(PickError::Io { .. })));
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
}

#[test]
fn the_lock_file_is_private() {
    let fx = Fx::new();
    let d = fx.dir();
    let _g = d.lock().unwrap();
    let m = fs::symlink_metadata(d.dir().join(".lock")).unwrap();
    assert!(m.file_type().is_file());
    assert_eq!(m.mode() & 0o777, 0o600);
}

// ---------------------------------------------------------------------------
// Writing and loading
// ---------------------------------------------------------------------------

#[test]
fn write_then_load_round_trips() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let l = Lease::new(Cli::Codex, fx.homes.home("a"), Some(48211), NOW);
    let path = g.write(&l).unwrap();
    let name = path.file_name().unwrap().to_str().unwrap().to_string();
    assert!(lease::valid_name(&name), "{name}");
    assert!(name.starts_with("48211-"), "{name}");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        raw,
        serde_json::json!({
            "version": 1,
            "cli": "codex",
            "home": fx.homes.home("a"),
            "pid": 48211,
            "created_at": NOW,
        })
    );
    let (loaded, warnings) = g.load();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].lease, l);
    assert_eq!(loaded[0].path, path);
}

#[test]
fn a_standalone_lease_has_a_null_pid_and_a_zero_name() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let path = g
        .write(&Lease::new(Cli::Codex, fx.homes.home("b"), None, NOW))
        .unwrap();
    assert!(
        path.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("0-"),
        "{}",
        path.display()
    );
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(raw["pid"].is_null());
}

#[test]
fn many_writes_in_one_nanosecond_get_distinct_names() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let mut names = std::collections::BTreeSet::new();
    for _ in 0..50 {
        let p = g
            .write(&Lease::new(Cli::Codex, fx.homes.home("a"), None, NOW))
            .unwrap();
        names.insert(p);
    }
    assert_eq!(names.len(), 50);
    assert_eq!(json_files(d.dir()).len(), 50);
    let leftovers: Vec<_> = fs::read_dir(d.dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left behind");
}

#[test]
fn the_name_pattern_is_strict() {
    for ok in ["0-1-0.json", "48211-1794233458123456789-7.json"] {
        assert!(lease::valid_name(ok), "{ok}");
    }
    for bad in [
        "a-1-0.json",
        "1-1.json",
        "1-1-0.json.tmp",
        ".tmp-1-1-0.json",
        "1-1-0.JSON",
        "../1-1-0.json",
        "1--0.json",
        "-1-1-0.json",
        "1-1-0 .json",
        "",
    ] {
        assert!(!lease::valid_name(bad), "{bad:?} accepted");
    }
}

#[test]
fn files_with_other_names_are_ignored_and_left_alone() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let body = serde_json::json!({"version":1,"cli":"codex","home":fx.homes.home("a"),"pid":null,"created_at":NOW}).to_string();
    for name in ["notes.json", "1-1.json", "x-1-1.json"] {
        let p = d.dir().join(name);
        fs::write(&p, &body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let (loaded, _) = g.load();
    assert!(loaded.is_empty(), "{loaded:?}");
    for name in ["notes.json", "1-1.json", "x-1-1.json"] {
        assert!(d.dir().join(name).exists());
    }
}

#[test]
fn unknown_version_or_garbage_is_left_in_place_uncounted_and_warned_once() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let v2 = d.dir().join("1-1-0.json");
    fs::write(&v2, serde_json::json!({"version":2,"cli":"codex","home":fx.homes.home("a"),"pid":null,"created_at":NOW}).to_string()).unwrap();
    let junk = d.dir().join("1-1-1.json");
    fs::write(&junk, format!("not json {SECRET}")).unwrap();
    for p in [&v2, &junk] {
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let (loaded, warnings) = g.load();
    assert!(loaded.is_empty(), "{loaded:?}");
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("1-1-0.json")));
    assert!(warnings.iter().any(|w| w.contains("1-1-1.json")));
    assert!(
        warnings.iter().all(|w| !w.contains(SECRET)),
        "file content printed: {warnings:?}"
    );
    assert!(v2.exists() && junk.exists(), "left in place");
}

#[test]
fn a_lease_readable_by_others_is_not_counted() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = g
        .write(&Lease::new(Cli::Codex, fx.homes.home("a"), None, NOW))
        .unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
    let (loaded, _) = g.load();
    assert!(loaded.is_empty(), "a lease with mode 0644 was counted");
}

#[test]
fn an_oversized_lease_is_not_counted() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = d.dir().join("1-1-0.json");
    let pad = " ".repeat(8192);
    fs::write(&p, format!("{{\"version\":1,\"cli\":\"codex\",\"home\":\"/a\",\"pid\":null,\"created_at\":{NOW}{pad}}}")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let (loaded, _) = g.load();
    assert!(loaded.is_empty());
}

#[test]
fn a_symlinked_lease_file_is_not_followed() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let outside = fx._tmp.path().join("outside.json");
    fs::write(&outside, serde_json::json!({"version":1,"cli":"codex","home":fx.homes.home("a"),"pid":null,"created_at":NOW}).to_string()).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&outside, d.dir().join("1-1-0.json")).unwrap();
    let (loaded, _) = g.load();
    assert!(loaded.is_empty());
}

// ---------------------------------------------------------------------------
// Sweep
// ---------------------------------------------------------------------------

fn write(g: &lease::LeaseLock<'_>, home: &Path, pid: Option<u32>, created_at: i64) -> PathBuf {
    g.write(&Lease::new(Cli::Codex, home.to_path_buf(), pid, created_at))
        .unwrap()
}

fn loaded(g: &lease::LeaseLock<'_>) -> Vec<Stored> {
    g.load().0
}

#[test]
fn a_dead_pid_lease_is_dropped_and_deleted() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let dead = write(&g, &fx.homes.home("a"), Some(111), NOW);
    let live = write(&g, &fx.homes.home("a"), Some(222), NOW);
    let kept = g.sweep(loaded(&g), &no_jobs(), NOW, TTL, &|pid| pid == 222);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].path, live);
    assert!(!dead.exists(), "the dead lease was not deleted");
    assert!(live.exists());
}

#[test]
fn ttl_expiry_is_at_ttl_not_after() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let young = write(&g, &fx.homes.home("a"), None, NOW - (TTL as i64 - 1));
    let exact = write(&g, &fx.homes.home("a"), None, NOW - TTL as i64);
    let old = write(&g, &fx.homes.home("a"), None, NOW - (TTL as i64 + 1));
    let kept = g.sweep(loaded(&g), &no_jobs(), NOW, TTL, &always);
    let paths: Vec<_> = kept.iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, vec![young.clone()]);
    assert!(young.exists());
    assert!(!exact.exists(), "age == ttl must expire");
    assert!(!old.exists());
}

#[test]
fn the_ttl_applies_to_leases_with_a_live_pid_too() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = write(&g, &fx.homes.home("a"), Some(333), NOW - TTL as i64);
    let kept = g.sweep(loaded(&g), &no_jobs(), NOW, TTL, &always);
    assert!(kept.is_empty());
    assert!(!p.exists());
}

#[test]
fn a_null_pid_lease_survives_until_the_ttl() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = write(&g, &fx.homes.home("a"), None, NOW - 1);
    let calls = std::cell::Cell::new(0);
    let kept = g.sweep(loaded(&g), &no_jobs(), NOW, TTL, &|_| {
        calls.set(calls.get() + 1);
        false
    });
    assert_eq!(kept.len(), 1, "no pid means no liveness check");
    assert_eq!(calls.get(), 0, "alive() called for a null pid");
    assert!(p.exists());
}

#[test]
fn a_lease_replaced_by_its_job_is_dropped() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    // amplihack 500 wrote the lease; its child codex 502 is now a job on a.
    let replaced = write(&g, &fx.homes.home("a"), Some(500), NOW);
    let other_home = write(&g, &fx.homes.home("b"), Some(500), NOW);
    let unrelated = write(&g, &fx.homes.home("a"), Some(900), NOW);
    let jobs = JobCounts {
        jobs: vec![Job {
            pid: 502,
            home: fx.homes.home("a"),
            ancestors: vec![501, 500, 1],
        }],
    };
    let kept = g.sweep(loaded(&g), &jobs, NOW, TTL, &always);
    let paths: Vec<_> = kept.iter().map(|s| s.path.clone()).collect();
    assert!(!replaced.exists(), "the job now counts; the lease must go");
    assert!(
        paths.contains(&other_home),
        "a lease on another home is not replaced by this job"
    );
    assert!(paths.contains(&unrelated));
}

#[test]
fn a_lease_whose_pid_is_the_job_itself_is_dropped() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = write(&g, &fx.homes.home("c"), Some(777), NOW);
    let jobs = JobCounts {
        jobs: vec![Job {
            pid: 777,
            home: fx.homes.home("c"),
            ancestors: vec![1],
        }],
    };
    assert!(g.sweep(loaded(&g), &jobs, NOW, TTL, &always).is_empty());
    assert!(!p.exists());
}

#[test]
fn malformed_pids_are_never_signalled_or_counted() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    for pid in [0u32, 2_147_483_648, u32::MAX] {
        let p = d.dir().join(format!("{pid}-1-0.json"));
        fs::write(&p, serde_json::json!({"version":1,"cli":"codex","home":fx.homes.home("a"),"pid":pid,"created_at":NOW}).to_string()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let seen = std::cell::RefCell::new(Vec::new());
    let (stored, _) = g.load();
    let kept = g.sweep(stored, &no_jobs(), NOW, TTL, &|pid| {
        seen.borrow_mut().push(pid);
        true
    });
    assert!(seen.borrow().is_empty(), "signalled {:?}", seen.borrow());
    assert!(kept.is_empty(), "a malformed pid was counted: {kept:?}");
}

#[test]
fn a_foreign_home_is_still_swept() {
    // A lease naming no candidate is never chosen, but the sweep still
    // removes it when its pid is dead.
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = write(&g, Path::new("/somewhere/else"), Some(4444), NOW);
    assert!(g.sweep(loaded(&g), &no_jobs(), NOW, TTL, &never).is_empty());
    assert!(!p.exists());
}

#[test]
fn a_lease_deleted_by_someone_else_is_not_an_error() {
    let fx = Fx::new();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let p = write(&g, &fx.homes.home("a"), Some(1), NOW);
    let stored = loaded(&g);
    fs::remove_file(&p).unwrap();
    assert!(g.sweep(stored, &no_jobs(), NOW, TTL, &never).is_empty());
    lease::remove(&p).expect("NotFound counts as removed");
}

// ---------------------------------------------------------------------------
// pid_alive
// ---------------------------------------------------------------------------

#[test]
fn pid_alive_sees_this_process_and_rejects_malformed_pids() {
    assert!(lease::pid_alive(std::process::id()));
    assert!(!lease::pid_alive(0));
    assert!(!lease::pid_alive(2_147_483_648));
    assert!(!lease::pid_alive(u32::MAX));
}

#[test]
fn pid_alive_counts_eperm_as_alive() {
    // pid 1 belongs to root; kill(1, 0) gives EPERM to anyone else.
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert!(lease::pid_alive(1), "EPERM must count as alive");
    }
}

#[test]
fn pid_alive_is_false_for_an_exited_child() {
    let mut child = std::process::Command::new("/bin/true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap(); // reaped: the pid no longer exists
    assert!(!lease::pid_alive(pid));
}

// ---------------------------------------------------------------------------
// The lock serialises holders
// ---------------------------------------------------------------------------

#[test]
fn a_second_holder_waits_for_the_first() {
    use std::sync::mpsc;
    let fx = Fx::new();
    let root = fx.root.clone();
    let d = fx.dir();
    let g = d.lock().unwrap();
    let (tx, rx) = mpsc::channel();
    let t = std::thread::spawn(move || {
        let d2 = LeaseDir::open(&root, Cli::Codex).unwrap();
        let _g2 = d2.lock().unwrap();
        tx.send(()).unwrap();
    });
    // While the first holder has the lock the second cannot have sent.
    assert!(rx.try_recv().is_err(), "the second holder did not wait");
    drop(g);
    rx.recv()
        .expect("the second holder gets the lock after release");
    t.join().unwrap();
}
