//! Issue #2040: `swap_check` picks its target from the cached running-session
//! read, so a status-line update with nothing to swap to costs no forced
//! `/proc` scan. A target whose eligibility rests on "no session runs in its
//! directory" is confirmed with a forced read before any login moves.

use super::config::Config;
use super::fsx;
use super::liveness::Liveness;
use super::state::{CREDS, State, UsageFile};
use super::swap::swap_check_with;
use super::usage::{RateLimits, Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::proc_snapshot::ProcSource;
use crate::test_support::{EnvGuard, env_lock};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const OAUTH: &str = r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn private(p: &Path, body: &str) {
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A private root where `work` and `spare` hold OAuth logins.
struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    state: State,
    profiles: Vec<ProfileName>,
}

fn fx() -> Fx {
    fx_with(&[("spare", None), ("work", None)])
}

/// A private root where each of `logins` holds an OAuth login, recorded as
/// belonging to its organisation when one is given.
fn fx_with(logins: &[(&str, Option<&str>)]) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir(&home).unwrap();
    let root = base.join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let mut profiles = Vec::new();
    for (name, _) in logins {
        let dir = root.join(name);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(&dir.join(CREDS), OAUTH);
        profiles.push(n(name));
    }
    private(&root.join(".auto-switch"), "enabled=on\n");
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    for (name, org) in logins {
        if let Some(org) = org {
            let expected = serde_json::json!({"account": format!("u-{name}"), "org": org});
            state.write_sub_json("expected", name, &expected).unwrap();
        }
    }
    Fx {
        _tmp: tmp,
        home,
        root,
        state,
        profiles,
    }
}

fn cfg() -> Config {
    Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow: Vec::new(),
        kind: Default::default(),
    }
}

fn reading(pct: f64, now: i64) -> Reading {
    Reading {
        five_hour: Some(Window {
            used_percentage: pct,
            resets_at: now + 4 * 3600,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: now + 86_400,
        }),
        observed_at: now - 10,
        source: ReadingSource::Statusline,
    }
}

/// Live in `names`.
fn on(names: &[&str]) -> Liveness {
    Liveness::Known(names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>())
}

/// Each read answers the next of `answers` (the last repeats) and is counted.
fn source(answers: Vec<Liveness>) -> (ProcSource, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&reads);
    let s = ProcSource::from_fn(move |_| {
        let i = c.fetch_add(1, Ordering::SeqCst);
        answers[i.min(answers.len() - 1)].clone()
    });
    (s, reads)
}

/// Runs `swap_check` for a session in `work`, whose account is used up,
/// with `spare` at `spare_pct`; returns the number of reads.
fn run(f: &Fx, spare_pct: f64, answers: Vec<Liveness>) -> usize {
    run_with(f, &[("spare", spare_pct)], answers)
}

/// Runs `swap_check` for a session in `work`, whose account is used up,
/// with each of `others` at its reading; returns the number of reads.
fn run_with(f: &Fx, others: &[(&str, f64)], answers: Vec<Liveness>) -> usize {
    let now = fsx::now();
    let mut accounts = BTreeMap::from([("work".to_string(), reading(100.0, now))]);
    for (name, pct) in others {
        accounts.insert(name.to_string(), reading(*pct, now));
    }
    f.state
        .save_usage(&UsageFile {
            version: 1,
            accounts,
        })
        .unwrap();
    let window = Some(Window {
        used_percentage: 100.0,
        resets_at: now + 4 * 3600,
    });
    let rl = RateLimits {
        five_hour: window,
        seven_day: window,
    };
    let (procs, reads) = source(answers);
    swap_check_with(&f.state, &f.profiles, &n("work"), &rl, &cfg(), &procs).unwrap();
    reads.load(Ordering::SeqCst)
}

fn env(f: &Fx) -> (EnvGuard, EnvGuard) {
    (
        EnvGuard::set([("HOME", f.home.to_str().unwrap())]),
        EnvGuard::unset([
            "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
            "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
            "AMPLIHACK_ROTATE_SESSION_AT",
            "AMPLIHACK_ROTATE_WEEK_AT",
        ]),
    )
}

#[test]
fn with_every_account_used_up_only_the_cached_read_is_taken() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx();
    let _env = env(&f);
    let reads = run(&f, 100.0, vec![on(&["work"])]);
    assert_eq!(reads, 1, "no forced read when nothing can be swapped to");
    assert!(f.state.load_ledger().unwrap().hosts.is_empty());
}

#[test]
fn a_target_picked_from_the_cache_is_confirmed_by_a_forced_read() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx();
    let _env = env(&f);
    let reads = run(&f, 10.0, vec![on(&["work"])]);
    assert_eq!(reads, 2, "the cached read, then one forced read");
    assert!(
        f.state.load_ledger().unwrap().hosts.contains_key("work"),
        "the login moved"
    );
}

#[test]
fn a_target_the_forced_read_finds_live_is_not_swapped_to() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx();
    let _env = env(&f);
    let before = fs::read(f.root.join("work").join(CREDS)).unwrap();
    // Cached: only `work` runs. Forced: a session has started in `spare`.
    let reads = run(&f, 10.0, vec![on(&["work"]), on(&["work", "spare"])]);
    assert_eq!(reads, 2);
    assert!(f.state.load_ledger().unwrap().hosts.is_empty(), "no move");
    assert_eq!(fs::read(f.root.join("work").join(CREDS)).unwrap(), before);
}

#[test]
fn a_busy_forced_read_moves_nothing() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx();
    let _env = env(&f);
    let reads = run(&f, 10.0, vec![on(&["work"]), Liveness::Everything]);
    assert_eq!(reads, 2);
    assert!(f.state.load_ledger().unwrap().hosts.is_empty(), "no move");
}

#[test]
fn a_forced_read_taken_by_recovery_is_not_repeated() {
    use super::state::{HostEntry, Ledger};
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx();
    let _env = env(&f);
    // `third`'s login is hosted in `other`. The cached read shows no session
    // in `other`, so recovery forces a read, which finds it still running.
    let hosts = BTreeMap::from([(
        "other".to_string(),
        HostEntry {
            live: "third".into(),
            swapped_at: 1,
            pre_swap_fingerprint: None,
        },
    )]);
    let version = f.state.load_ledger().unwrap().version;
    f.state.save_ledger(&Ledger { version, hosts }).unwrap();
    let reads = run(&f, 10.0, vec![on(&["work"]), on(&["work", "other"])]);
    assert_eq!(reads, 2, "recovery's forced read confirms the target");
    let hosts = f.state.load_ledger().unwrap().hosts;
    assert!(hosts.contains_key("other"), "the session in other runs on");
    assert!(hosts.contains_key("work"), "the login moved");
}

/// Issue #2040 with #2136's organisation affinity: the forced read picks the
/// target again and works out its organisation restriction again, from that
/// read alone. `work` and `same` are in `org-a`, `other` in `org-b`. The
/// cached read sees no session in `same`, so the restriction keeps the swap
/// inside `org-a` and the cached pick is `same`. The forced read finds a
/// session in `same`; the restriction worked out from it allows the
/// necessary crossing to `other`, and the login moves there. A restriction
/// kept from the cached pick would allow only `same`, and nothing would move.
#[test]
fn the_forced_pick_works_out_its_own_organisation_restriction() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx_with(&[
        ("work", Some("org-a")),
        ("same", Some("org-a")),
        ("other", Some("org-b")),
    ]);
    let _env = env(&f);
    let reads = run_with(
        &f,
        &[("same", 10.0), ("other", 10.0)],
        vec![on(&["work"]), on(&["work", "same"])],
    );
    assert_eq!(reads, 2, "the cached read, then one forced read");
    let hosts = f.state.load_ledger().unwrap().hosts;
    assert_eq!(
        hosts.get("work").map(|h| h.live.as_str()),
        Some("other"),
        "the swap used the forced pick and its restriction"
    );
}

/// The same fixture, with the forced read finding sessions in both other
/// directories: the forced pick finds no target, so nothing moves even
/// though the cached pick had one.
#[test]
fn a_forced_pick_with_no_target_moves_nothing_across_organisations() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx_with(&[
        ("work", Some("org-a")),
        ("same", Some("org-a")),
        ("other", Some("org-b")),
    ]);
    let _env = env(&f);
    let before = fs::read(f.root.join("work").join(CREDS)).unwrap();
    let reads = run_with(
        &f,
        &[("same", 10.0), ("other", 10.0)],
        vec![on(&["work"]), on(&["work", "same", "other"])],
    );
    assert_eq!(reads, 2);
    assert!(f.state.load_ledger().unwrap().hosts.is_empty(), "no move");
    assert_eq!(fs::read(f.root.join("work").join(CREDS)).unwrap(), before);
}
