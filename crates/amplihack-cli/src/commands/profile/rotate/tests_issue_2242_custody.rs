//! Alternate outgoing custody must make a later candidate usable, not merely attempted.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

fn conflict(e: &Env) -> u64 {
    private(
        &e.slot("work"),
        std::str::from_utf8(&fs::read(e.slot("extra")).unwrap()).unwrap(),
    );
    ino_at(&e.slot("work")).unwrap()
}

fn recovery_profile(e: &Env) -> (String, PathBuf) {
    let found: Vec<_> = e
        .store
        .list()
        .unwrap()
        .into_iter()
        .filter_map(|name| {
            let dir = e.store.verify_profile_dir(&n(&name)).unwrap();
            (ino_at(&dir.join(".credentials.json")) == Some(e.l)).then_some((name, dir))
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "outgoing inode must belong to exactly one discoverable profile"
    );
    found.into_iter().next().unwrap()
}

fn receipts(root: &std::path::Path) -> Vec<(PathBuf, serde_json::Value)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            out.extend(receipts(&path));
        } else if metadata.is_file()
            && path.file_name().unwrap() != ".credentials.json"
            && let Ok(value) =
                serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap())
            && value.get("origin").and_then(|v| v.as_str()) == Some("work")
        {
            out.push((path, value));
        }
    }
    out
}

#[test]
fn different_account_refusal_then_fallback_commits_private_discoverable_outgoing_custody() {
    let e = fixture();
    let occupied = conflict(&e);
    let expected_path = e.root.join(".auto-switch.d/expected/work.json");
    let expected_before = fs::read(&expected_path).unwrap();
    let extra = ino_at(&e.slot("extra"));
    let mut c = context(&e);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert_eq!(ino_at(&e.live()), extra, "the later candidate becomes live");
    assert_eq!(
        ino_at(&e.slot("spare")),
        Some(e.t),
        "the first refused candidate stays put"
    );
    assert_eq!(
        ino_at(&e.slot("work")),
        Some(occupied),
        "the conflicting login stays put"
    );
    assert_eq!(
        fs::read(expected_path).unwrap(),
        expected_before,
        "conflict ownership is never relabelled"
    );
    assert_eq!(
        measurements(),
        1,
        "fallback uses this cycle's retained observations"
    );
    let (name, dir) = recovery_profile(&e);
    assert!(!["work", "spare", "extra"].contains(&name.as_str()));
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let credential = fs::metadata(dir.join(".credentials.json")).unwrap();
    assert_eq!(
        credential.nlink(),
        1,
        "custody uses a move, never another link"
    );
    assert_eq!(credential.permissions().mode() & 0o777, 0o600);
    let account = super::super::tests_fixture_identity::read(&dir).unwrap();
    assert_eq!(
        account.key(),
        "u-work",
        "discoverable custody retains verified ownership"
    );
    let receipt = receipts(&e.rs.dir)
        .into_iter()
        .find(|(_, v)| {
            let text = v.to_string();
            text.contains(&name) && text.contains("u-work")
        })
        .expect("a durable receipt binds origin, verified account and recovery destination");
    assert_eq!(
        fs::metadata(&receipt.0).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!receipt.1.to_string().contains("SENTINEL"));
    let event = json(&e.rs.dir.join("swap.json"));
    assert_eq!(
        event["from_login"], "work",
        "logical source is independent of custody destination"
    );
    assert_eq!(event["live_login"], "extra");
    assert_eq!(event["seq"], 1);
    assert_eq!(event["reading"]["session"], 25);
    assert_eq!(event["reading"]["measured_at"], T);
    let parsed = amplihack_utils::rotator_status::swap_record::parse_swap(
        &fs::read(e.rs.dir.join("swap.json")).unwrap(),
        clock(),
    )
    .expect("the existing #2141 reader accepts a completed fallback event");
    assert_eq!(parsed.swapped_at, T);
    assert_eq!(
        e.rs.load_ledger().unwrap().unwrap().live.as_deref(),
        Some("extra")
    );
    assert!(!e.rs.journal.exists());
    let log = log(&e);
    let refusal = log
        .lines()
        .position(|l| l.contains("spare") && l.contains("refus"));
    let success = log.lines().position(|l| l.contains("swap work->extra"));
    assert!(refusal.is_some() && refusal < success, "{log}");
    assert_eq!(
        log.lines()
            .filter(|l| l.contains("ALERT")
                && l.contains("work@example.com")
                && l.contains("extra@example.com"))
            .count(),
        1
    );
    assert!(!log.contains("SENTINEL"));
    assert!(log.contains("account=u-work") && log.contains("account=u-extra"));
}

#[test]
fn unresolved_conflict_alert_is_suppressed_until_credential_evidence_changes() {
    let e = fixture();
    let occupied = conflict(&e);
    // Prevent the later candidate from succeeding so this conflict remains current.
    fs::create_dir(e.root.join("extra/.oauth_refresh.lock")).unwrap();
    for _ in 0..2 {
        let mut c = context(&e); // fresh context exercises persisted suppression
        assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    }
    let count = || {
        log(&e)
            .lines()
            .filter(|l| {
                l.contains("ALERT")
                    && l.contains("work@example.com")
                    && l.contains("extra@example.com")
            })
            .count()
    };
    assert_eq!(count(), 1);
    // A replacement for the same verified account is materially new evidence.
    let mut changed = json(&e.slot("work"));
    changed["claudeAiOauth"]["accessToken"] = "SENTINEL-REPLACED".into();
    let replacement = e.root.join("replacement");
    private(&replacement, &changed.to_string());
    fs::rename(replacement, e.slot("work")).unwrap();
    assert_ne!(ino_at(&e.slot("work")), Some(occupied));
    let mut c = context(&e);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    assert_eq!(
        count(),
        2,
        "changed credential evidence permits one new conflict alert"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn recovery_custody_inherits_the_origin_exclusion() {
    let e = fixture();
    conflict(&e);
    let mut c = context(&e);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    let (recovery, dir) = recovery_profile(&e);
    private(
        &e.root.join(".auto-switch"),
        "enabled=off\nthreshold=95\nexclude=work\n",
    );
    let state = State::new(&e.root).unwrap();
    let snap = super::super::super::observe::snapshot(&c, &state).unwrap();
    assert!(
        crate::commands::profile::usage_cmd::exclude_list(&e.root, |_| {})
            .unwrap()
            .contains(&n(&recovery))
    );
    assert!(
        crate::commands::profile::auto_switch::config::effective(&e.root)
            .config
            .exclude
            .contains(&n(&recovery))
    );
    assert!(
        !snap.candidates.contains(&n(&recovery)),
        "a new custody name cannot bypass the origin's exclusion"
    );
    assert_eq!(ino_at(&dir.join(".credentials.json")), Some(e.l));
    let alias = n("renamed-custody");
    let renamed = e.root.join(alias.as_str());
    fs::rename(&dir, &renamed).unwrap();
    assert!(super::super::super::custody::origin(&e.store, &alias).is_err());
    assert!(
        crate::commands::profile::usage_cmd::exclude_list(&e.root, |_| {})
            .unwrap()
            .contains(&alias),
        "renaming recovery custody cannot remove the origin's exclusion"
    );
    assert_eq!(ino_at(&renamed.join(".credentials.json")), Some(e.l));
}

#[test]
fn a_recovery_name_cannot_turn_a_credit_backed_origin_into_plan_capacity() {
    let e = fixture();
    conflict(&e);
    let mut c = context(&e);
    assert_eq!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    let (recovery, _) = recovery_profile(&e);
    let decision = || {
        let state = State::new(&e.root).unwrap();
        let snap = super::super::super::observe::snapshot(&c, &state).unwrap();
        let probes = snap
            .candidates
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    if name.as_str() == recovery {
                        ok(0.0, 0.0)
                    } else {
                        ok(100.0, 100.0)
                    },
                )
            })
            .collect();
        super::super::super::observe::decide_measured(&c, &snap, ok(90.0, 20.0), probes, clock())
            .unwrap()
            .decision
    };
    assert!(
        matches!(decision(), super::super::super::decide::Decision::Swap { to, .. } if to.as_str() == recovery)
    );
    private(
        &e.root.join(".auto-switch"),
        "enabled=off\nthreshold=95\noverflow=work:1\n",
    );
    assert!(
        !matches!(decision(), super::super::super::decide::Decision::Swap { to, .. } if to.as_str() == recovery)
    );
}

#[test]
fn interrupted_alternate_custody_recovers_before_another_candidate_can_move() {
    // Explicit fault boundaries, never elapsed-time races. Each boundary must
    // have durable intent before any movement that recovery is asked to finish.
    for point in ["outgoing-reserved", "outgoing-intent", "outgoing-moved"] {
        let e = fixture();
        let occupied = conflict(&e);
        let extra = ino_at(&e.slot("extra"));
        let mut c = context(&e);
        c.measure = Some(|ctx, snap| {
            // Recovery may finish the interrupted swap before this new
            // cycle measures. Give that incoming account its own low usage,
            // rather than accidentally requesting a second, unrelated swap.
            if matches!(&snap.live, super::super::super::identify::LiveId::Known(name) if name.as_str() == "extra")
            {
                sample(25.0, 20.0, 0, T + 10_000);
            }
            measure(ctx, snap)
        });
        test_exchange::interrupted(point, || {
            let _ = drive(&e, &mut c);
        });
        assert!(e.rs.journal.exists(), "retain custody intent at {point}");
        assert_eq!(json(&e.rs.journal)["version"], 2);
        assert_eq!(ino_at(&e.slot("work")), Some(occupied));
        assert!(!e.rs.dir.join("swap.json").exists());
        advance(120);
        let _ = drive(&e, &mut c);
        assert_eq!(
            ino_at(&e.live()),
            extra,
            "recover or retry in this explicit cycle: {point}: {}",
            log(&e)
        );
        assert_eq!(ino_at(&e.slot("work")), Some(occupied));
        let _ = recovery_profile(&e);
        assert!(!e.rs.journal.exists());
        let event = json(&e.rs.dir.join("swap.json"));
        assert_eq!(event["seq"], 1);
        assert_eq!(event["from_login"], "work");
        assert_eq!(event["live_login"], "extra");
        advance(120);
        sample(5.0, 10.0, 0, T + 10_000);
        let _ = drive(&e, &mut c);
        assert_eq!(
            json(&e.rs.dir.join("swap.json")),
            event,
            "custody recovery cannot republish a new event"
        );
    }
}

#[test]
fn recovery_profiles_with_missing_provenance_cannot_be_selected() {
    let e = fixture();
    conflict(&e);
    let mut c = context(&e);
    assert_eq!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    let (name, dir) = recovery_profile(&e);
    fs::rename(
        dir.join(".rotate-custody.json"),
        e._tmp.path().join("retained-receipt"),
    )
    .unwrap();
    let state = State::new(&e.root).unwrap();
    let snap = super::super::super::observe::snapshot(&c, &state).unwrap();
    assert!(!snap.candidates.contains(&n(&name)));
    assert!(
        snap.skips
            .iter()
            .any(|(profile, reason)| profile == &n(&name)
                && *reason == super::super::super::candidates::Skip::CustodyUnknown)
    );
    assert_eq!(ino_at(&dir.join(".credentials.json")), Some(e.l));
}

#[test]
fn recovery_profiles_cannot_bypass_an_active_origin_session() {
    let e = fixture();
    conflict(&e);
    let mut c = context(&e);
    assert_eq!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    let (name, dir) = recovery_profile(&e);
    c.scan = |_| {
        crate::commands::profile::auto_switch::kit::Liveness::Known(
            ["work".to_owned()].into_iter().collect(),
        )
    };
    let state = State::new(&e.root).unwrap();
    let snap = super::super::super::observe::snapshot(&c, &state).unwrap();
    assert!(!snap.candidates.contains(&n(&name)));
    assert_eq!(ino_at(&dir.join(".credentials.json")), Some(e.l));
}

#[test]
fn a_substituted_participant_stops_recovery_without_touching_another_writers_directory() {
    for replace_origin in [false, true] {
        let e = fixture();
        conflict(&e);
        let mut c = context(&e);
        test_exchange::interrupted("outgoing-reserved", || {
            let _ = drive(&e, &mut c);
        });
        let journal = json(&e.rs.journal);
        let destination = journal["outgoing"]["destination"].as_str().unwrap();
        let dir = e
            .root
            .join(if replace_origin { "work" } else { destination });
        fs::rename(&dir, e._tmp.path().join("original-reservation")).unwrap();
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let replacement = fs::metadata(&dir).unwrap().ino();
        let _ = drive(&e, &mut c);
        assert_eq!(fs::metadata(&dir).unwrap().ino(), replacement);
        assert!(e.rs.journal.exists());
        assert_eq!(ino_at(&e.live()), Some(e.l));
        assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
        assert!(!e.rs.dir.join("swap.json").exists());
    }
}

#[test]
fn recovery_reservations_are_exclusive_and_never_replace_existing_directories() {
    let e = fixture();
    let name = n("recovered-00000000-0000-4000-8000-000000000001");
    let dir = e.store.reserve_recovery(&name).unwrap();
    private(&dir.join("another-writer"), "preserve this fixture");
    let original = fs::metadata(&dir).unwrap().ino();
    assert!(e.store.reserve_recovery(&name).is_err());
    assert_eq!(fs::metadata(&dir).unwrap().ino(), original);
    assert_eq!(
        fs::read(dir.join("another-writer")).unwrap(),
        b"preserve this fixture"
    );
}

#[test]
fn version_two_recovery_reconsiders_a_previously_inconsistent_placement() {
    let e = fixture();
    conflict(&e);
    let extra = ino_at(&e.slot("extra"));
    let mut c = context(&e);
    test_exchange::interrupted("s1", || {
        let _ = drive(&e, &mut c);
    });
    let journal = json(&e.rs.journal);
    let destination = journal["outgoing"]["destination"].as_str().unwrap();
    let obstruction = e.root.join(destination).join(".credentials.json");
    fs::create_dir(&obstruction).unwrap();
    let _ = drive(&e, &mut c);
    assert_eq!(json(&e.rs.journal)["phase"], "inconsistent");
    assert!(
        obstruction.is_dir(),
        "recovery preserves the foreign object"
    );
    assert!(!e.rs.dir.join("swap.json").exists());
    // An explicit repair and cycle, without any retry scheduler.
    fs::remove_dir(&obstruction).unwrap();
    c.identity_budget.reset();
    let recovered = super::super::super::recover::recover_at(
        &e.store,
        &e.rs,
        &e.home,
        c.identity_reader(),
        c.now,
    )
    .unwrap();
    assert!(matches!(
        recovered,
        super::super::super::recover::RecoverOutcome::RolledForward { .. }
    ));
    assert_eq!(ino_at(&e.live()), extra);
    let _ = recovery_profile(&e);
    assert_eq!(json(&e.rs.dir.join("swap.json"))["seq"], 1);
    assert!(!e.rs.journal.exists());
}

#[test]
fn alternate_custody_cannot_commit_receipts_or_success_before_directory_sync() {
    use super::super::super::{recover, verified_dirs};
    let e = fixture();
    let occupied = conflict(&e);
    let extra = ino_at(&e.slot("extra"));
    let mut c = context(&e);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            verified_dirs::FAIL_SYNC.with(|f| f.set(false));
        }
    }
    let _reset = Reset;
    verified_dirs::FAIL_SYNC.with(|f| f.set(true));
    let _ = drive(&e, &mut c);
    let journal = json(&e.rs.journal);
    let saved = e
        .root
        .join(journal["outgoing"]["destination"].as_str().unwrap());
    assert_eq!(ino_at(&e.live()), extra);
    assert_eq!(ino_at(&saved.join(".credentials.json")), Some(e.l));
    assert_eq!(ino_at(&e.slot("work")), Some(occupied));
    for _ in 0..2 {
        assert!(!saved.join(".rotate-custody.json").exists());
        assert!(!e.rs.dir.join("swap.json").exists());
        assert!(e.rs.journal.exists());
        c.identity_budget.reset();
        assert!(recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).is_err());
    }
    verified_dirs::FAIL_SYNC.with(|f| f.set(false));
    c.identity_budget.reset();
    assert!(matches!(
        recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).unwrap(),
        recover::RecoverOutcome::RolledForward { .. }
    ));
    assert_eq!(ino_at(&e.live()), extra);
    assert_eq!(ino_at(&saved.join(".credentials.json")), Some(e.l));
    assert_eq!(ino_at(&e.slot("work")), Some(occupied));
    assert!(saved.join(".rotate-custody.json").exists());
    assert_eq!(json(&e.rs.dir.join("swap.json"))["seq"], 1);
    assert!(!e.rs.journal.exists());
}
