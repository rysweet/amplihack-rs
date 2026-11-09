//! Issue #2006: `pick()` end to end in-process, including ten simultaneous
//! picks over three fixture accounts.
//!
//! Wired from `account/mod.rs` as `#[cfg(all(test, target_os = "linux"))] mod tests_pick_concurrency;`.
//!
//! Contract (docs/reference/account-pick.md):
//!
//! - `reading::Candidate { account: String, home: PathBuf,
//!   live_headroom: Option<f64>, usable: bool, note: String }`.
//! - `pick::Readings { candidates: Vec<Candidate>, read_at: i64 }`.
//! - `pick::PickEnv<'a> { now, proc_root, lease_root, self_pid, uid,
//!   lease_pid: Option<u32>, job_cost_pct, ttl_secs,
//!   read: &'a (dyn Fn(Cli) -> Readings + Sync),
//!   alive: &'a (dyn Fn(u32) -> bool + Sync) }`.
//! - `pick::PickOpts { write_lease: bool }`; `--list` passes `false`.
//! - `pick::pick(&PickEnv, Cli, PickOpts) -> Result<Pick, PickError>`.
//!   `Pick { chosen: Option<usize>, ranked: Vec<Ranked>, read_at: i64,
//!   lease: Option<PathBuf> }`, `chosen` indexing `ranked`.
//!   `Ranked { candidate: Candidate, running_jobs: usize,
//!   active_leases: usize, effective_headroom: Option<f64> }`.
//! - `PickError::NoUsable { ranked, read_at, reason: NoUsableReason }` and
//!   `PickError::Io { op, path, source }`. `NoUsableReason::as_str()` is
//!   one of `no accounts found`, `none signed in or readable`,
//!   `none has headroom after running jobs and leases`.
//! - `read` is called before the lock is taken; the lock covers job
//!   counting, the lease sweep, ranking and the lease write.

use super::Cli;
use super::pick::{self, NoUsableReason, Pick, PickEnv, PickError, PickOpts, Readings};
use super::reading::Candidate;
use super::test_fixtures::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex};

const COST: f64 = 15.0;
const TTL: u64 = 120;

struct World {
    _tmp: tempfile::TempDir,
    homes: Homes,
    procfs: FakeProc,
    lease_root: PathBuf,
    live: Vec<(String, Option<f64>, bool)>,
}

impl World {
    /// Accounts `a` 80, `b` 50 and `c` 20, all usable, no jobs, no leases.
    fn new() -> Self {
        Self::with(vec![
            ("a".into(), Some(80.0), true),
            ("b".into(), Some(50.0), true),
            ("c".into(), Some(20.0), true),
        ])
    }

    fn with(live: Vec<(String, Option<f64>, bool)>) -> Self {
        let tmp = private_tempdir();
        let lease_root = tmp.path().join("leases");
        Self {
            _tmp: tmp,
            homes: Homes::new(),
            procfs: FakeProc::new(),
            lease_root,
            live,
        }
    }

    fn readings(&self) -> Readings {
        Readings {
            candidates: self
                .live
                .iter()
                .map(|(n, l, u)| Candidate {
                    account: n.clone(),
                    home: self.homes.home(n),
                    live_headroom: *l,
                    usable: *u,
                    note: String::new(),
                })
                .collect(),
            read_at: NOW - 3,
        }
    }

    fn lease_dir(&self) -> PathBuf {
        self.lease_root.join("codex")
    }

    fn lease_files(&self) -> Vec<PathBuf> {
        match fs::read_dir(self.lease_dir()) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn lease_homes(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for p in self.lease_files() {
            let v: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
            let home = PathBuf::from(v["home"].as_str().unwrap());
            let name = home.file_name().unwrap().to_string_lossy().into_owned();
            *m.entry(name).or_default() += 1;
        }
        m
    }
}

fn env<'a>(
    w: &'a World,
    read: &'a (dyn Fn(Cli) -> Readings + Sync),
    alive: &'a (dyn Fn(u32) -> bool + Sync),
    lease_pid: Option<u32>,
) -> PickEnv<'a> {
    PickEnv {
        now: NOW,
        proc_root: w.procfs.root.clone(),
        lease_root: w.lease_root.clone(),
        self_pid: SELF_PID,
        uid: UID,
        lease_pid,
        job_cost_pct: COST,
        ttl_secs: TTL,
        read,
        alive,
    }
}

fn chosen_name(p: &Pick) -> String {
    let i = p.chosen.expect("an account was chosen");
    p.ranked[i].candidate.account.clone()
}

const WRITE: PickOpts = PickOpts { write_lease: true };
const LIST: PickOpts = PickOpts { write_lease: false };

// ---------------------------------------------------------------------------
// The spread
// ---------------------------------------------------------------------------

/// E2: ten threads behind a barrier pick at once. Each pick is serialised
/// by the lock and sees the leases of the picks before it, so the split is
/// exactly 6/3/1 whatever order the threads reach the lock in.
#[test]
fn ten_simultaneous_picks_spread_6_3_1() {
    let w = World::new();
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let barrier = Arc::new(Barrier::new(10));
    let results = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..10 {
            let barrier = Arc::clone(&barrier);
            let (w, read, alive, results) = (&w, &read, &alive, &results);
            s.spawn(move || {
                let e = env(w, read, alive, Some(SELF_PID));
                barrier.wait();
                let p = pick::pick(&e, Cli::Codex, WRITE).expect("each pick succeeds");
                results.lock().unwrap().push(chosen_name(&p));
            });
        }
    });
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for n in results.into_inner().unwrap() {
        *counts.entry(n).or_default() += 1;
    }
    let want: BTreeMap<String, usize> = [("a".into(), 6), ("b".into(), 3), ("c".into(), 1)].into();
    assert_eq!(counts, want, "picks by account");
    assert_eq!(w.lease_files().len(), 10, "one lease per pick");
    assert_eq!(w.lease_homes(), want, "leases by account");
    assert_ne!(
        counts.get("a"),
        Some(&10),
        "the 80% account took every pick"
    );
}

#[test]
fn ten_serial_picks_give_the_same_split() {
    let w = World::new();
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let e = env(&w, &read, &alive, None);
    let order: Vec<String> = (0..10)
        .map(|_| chosen_name(&pick::pick(&e, Cli::Codex, WRITE).unwrap()))
        .collect();
    assert_eq!(order, ["a", "a", "a", "b", "a", "b", "a", "b", "c", "a"]);
}

// ---------------------------------------------------------------------------
// Single picks
// ---------------------------------------------------------------------------

#[test]
fn a_pick_writes_one_lease_with_the_lease_pid_and_clock() {
    let w = World::new();
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let p = pick::pick(&env(&w, &read, &alive, Some(31337)), Cli::Codex, WRITE).unwrap();
    assert_eq!(chosen_name(&p), "a");
    let lease = p.lease.expect("a lease path");
    assert_eq!(w.lease_files(), vec![lease.clone()]);
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&lease).unwrap()).unwrap();
    assert_eq!(v["pid"], 31337);
    assert_eq!(v["created_at"], NOW);
    assert_eq!(v["cli"], "codex");
    assert_eq!(v["home"], w.homes.home("a").to_str().unwrap());
    assert_eq!(p.read_at, NOW - 3);
}

#[test]
fn ranked_rows_carry_jobs_leases_and_effective_headroom() {
    let w = World::new();
    w.procfs
        .process(100, 1, UID, 'S', &[&kv("CODEX_HOME", &w.homes.home("a"))]);
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let e = env(&w, &read, &alive, None);
    pick::pick(&e, Cli::Codex, WRITE).unwrap(); // a: 80 - 15 = 65 -> a
    let p = pick::pick(&e, Cli::Codex, LIST).unwrap();
    let a = p
        .ranked
        .iter()
        .find(|r| r.candidate.account == "a")
        .unwrap();
    assert_eq!(a.running_jobs, 1);
    assert_eq!(a.active_leases, 1);
    assert_eq!(a.effective_headroom, Some(50.0));
    let names: Vec<_> = p
        .ranked
        .iter()
        .map(|r| r.candidate.account.as_str())
        .collect();
    // a and b tie at 50; a has more live headroom.
    assert_eq!(names, ["a", "b", "c"]);
}

#[test]
fn list_writes_no_lease_and_sees_no_extra_lease() {
    let w = World::new();
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let e = env(&w, &read, &alive, None);
    for _ in 0..3 {
        let p = pick::pick(&e, Cli::Codex, LIST).unwrap();
        assert!(p.lease.is_none());
        assert_eq!(chosen_name(&p), "a", "list never changes the next choice");
    }
    assert!(w.lease_files().is_empty());
}

#[test]
fn list_with_nothing_usable_is_ok_with_no_choice() {
    let w = World::with(vec![("a".into(), None, false), ("b".into(), None, false)]);
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let p = pick::pick(&env(&w, &read, &alive, None), Cli::Codex, LIST).unwrap();
    assert_eq!(p.chosen, None);
    assert_eq!(p.ranked.len(), 2);
}

#[test]
fn running_jobs_move_the_choice() {
    let w = World::new();
    for pid in [100, 200, 300] {
        w.procfs
            .process(pid, 1, UID, 'S', &[&kv("CODEX_HOME", &w.homes.home("a"))]);
    }
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let p = pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE).unwrap();
    assert_eq!(chosen_name(&p), "b", "a has 80 - 45 = 35 < 50");
}

#[test]
fn expired_and_dead_leases_do_not_count() {
    let w = World::with(vec![
        ("a".into(), Some(60.0), true),
        ("b".into(), Some(50.0), true),
    ]);
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |pid: u32| pid != 4444;
    // Seed: a dead-pid lease and an expired lease on a.
    {
        let dir = super::lease::LeaseDir::open(&w.lease_root, Cli::Codex).unwrap();
        let g = dir.lock().unwrap();
        let home = w.homes.home("a");
        g.write(&super::lease::Lease::new(
            Cli::Codex,
            home.clone(),
            Some(4444),
            NOW,
        ))
        .unwrap();
        g.write(&super::lease::Lease::new(
            Cli::Codex,
            home,
            None,
            NOW - TTL as i64,
        ))
        .unwrap();
    }
    let p = pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE).unwrap();
    assert_eq!(chosen_name(&p), "a", "both leases were swept, so a has 60");
    assert_eq!(w.lease_files().len(), 1, "only the new lease remains");
}

#[test]
fn a_forged_lease_cannot_redirect_the_pick() {
    let w = World::with(vec![("a".into(), Some(80.0), true)]);
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    {
        let dir = super::lease::LeaseDir::open(&w.lease_root, Cli::Codex).unwrap();
        let g = dir.lock().unwrap();
        g.write(&super::lease::Lease::new(
            Cli::Codex,
            PathBuf::from("/attacker/home"),
            None,
            NOW,
        ))
        .unwrap();
    }
    let p = pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE).unwrap();
    let i = p.chosen.unwrap();
    assert_eq!(p.ranked[i].candidate.home, w.homes.home("a"));
    assert!(
        p.ranked
            .iter()
            .all(|r| r.candidate.home != Path::new("/attacker/home")),
        "a lease home became a candidate"
    );
}

#[test]
fn claude_and_codex_leases_are_kept_apart() {
    let w = World::new();
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let e = env(&w, &read, &alive, None);
    pick::pick(&e, Cli::Claude, WRITE).unwrap();
    pick::pick(&e, Cli::Claude, WRITE).unwrap();
    // Two Claude leases on a do not lower a for Codex.
    let p = pick::pick(&e, Cli::Codex, LIST).unwrap();
    let a = p
        .ranked
        .iter()
        .find(|r| r.candidate.account == "a")
        .unwrap();
    assert_eq!(a.active_leases, 0);
    assert!(w.lease_root.join("claude").is_dir());
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

fn no_usable(r: Result<Pick, PickError>) -> (NoUsableReason, usize) {
    match r {
        Err(PickError::NoUsable { reason, ranked, .. }) => (reason, ranked.len()),
        Err(e) => panic!("expected NoUsable, got {e:?}"),
        Ok(p) => panic!("expected NoUsable, got a pick of {:?}", p.chosen),
    }
}

#[test]
fn no_candidates_is_no_accounts_found() {
    let w = World::with(vec![]);
    let read = |_: Cli| Readings {
        candidates: vec![],
        read_at: NOW,
    };
    let alive = |_: u32| true;
    let (reason, n) = no_usable(pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE));
    assert_eq!(reason, NoUsableReason::NoAccounts);
    assert_eq!(reason.as_str(), "no accounts found");
    assert_eq!(n, 0);
    assert!(w.lease_files().is_empty());
}

#[test]
fn none_readable_is_none_signed_in_or_readable() {
    let w = World::with(vec![("a".into(), None, false), ("b".into(), None, false)]);
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let (reason, n) = no_usable(pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE));
    assert_eq!(reason, NoUsableReason::NoneReadable);
    assert_eq!(reason.as_str(), "none signed in or readable");
    assert_eq!(n, 2, "every candidate is still listed");
    assert!(w.lease_files().is_empty());
}

#[test]
fn no_headroom_after_jobs_and_leases() {
    let w = World::with(vec![("a".into(), Some(20.0), true)]);
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    let e = env(&w, &read, &alive, None);
    pick::pick(&e, Cli::Codex, WRITE).unwrap(); // 20 -> 5
    pick::pick(&e, Cli::Codex, WRITE).unwrap(); // 5 -> -10
    let (reason, _) = no_usable(pick::pick(&e, Cli::Codex, WRITE));
    assert_eq!(reason, NoUsableReason::NoHeadroom);
    assert_eq!(
        reason.as_str(),
        "none has headroom after running jobs and leases"
    );
    assert_eq!(w.lease_files().len(), 2, "a failed pick writes no lease");
}

#[test]
fn an_unusable_lease_directory_is_an_io_error() {
    let w = World::new();
    fs::create_dir(&w.lease_root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&w.lease_root, fs::Permissions::from_mode(0o777)).unwrap();
    let readings = w.readings();
    let read = move |_: Cli| readings.clone();
    let alive = |_: u32| true;
    match pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE) {
        Err(PickError::Io { path, .. }) => assert!(path.starts_with(&w.lease_root)),
        other => panic!("expected Io, got {:?}", other.map(|p| p.chosen)),
    }
}

#[test]
fn reads_happen_before_the_lock_is_taken() {
    // While a reader runs, another holder has the lock. If pick() took the
    // lock before reading, the read would never run and the holder would
    // never be released: the reader releases the holder itself, so a
    // correct order finishes and a wrong one deadlocks visibly.
    use std::sync::mpsc;
    let w = World::new();
    let readings = w.readings();
    let dir = super::lease::LeaseDir::open(&w.lease_root, Cli::Codex).unwrap();
    let guard = Mutex::new(Some(dir.lock().unwrap()));
    let (tx, rx) = mpsc::channel::<()>();
    let tx = Mutex::new(Some(tx));
    let read = |_: Cli| {
        // Release the outside holder from inside the read.
        guard.lock().unwrap().take();
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        readings.clone()
    };
    let alive = |_: u32| true;
    let p = pick::pick(&env(&w, &read, &alive, None), Cli::Codex, WRITE).unwrap();
    rx.recv().unwrap();
    assert_eq!(chosen_name(&p), "a");
}
