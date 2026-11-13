//! Deterministic review regressions: real custody, policy and publication.
use super::super::super::{pin, recover, verified_dirs};
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        verified_dirs::HOOK.with(|h| h.borrow_mut().take());
        verified_dirs::FAIL_SYNC.with(|f| f.set(false));
    }
}

#[test]
fn substituted_parents_cannot_redirect_exchange_parking_or_outgoing_moves() {
    for mode in ["exchange", "outgoing", "duplicate", "recovery"] {
        let e = fixture();
        let mut c = context(&e);
        let duplicate = mode == "duplicate";
        if duplicate {
            private(
                &e.slot("work"),
                std::str::from_utf8(&fs::read(e.live()).unwrap()).unwrap(),
            );
        }
        if mode == "recovery" {
            test_exchange::interrupted("s1", || {
                let _ = drive(&e, &mut c);
            });
        }
        let dir = e
            .root
            .join(if mode == "exchange" { "spare" } else { "work" });
        let original = e._tmp.path().join("original-directory");
        let replaced = dir.clone();
        let retained = original.clone();
        let point = if mode == "exchange" {
            "exchange"
        } else {
            "move"
        };
        let _reset = Reset;
        let mut fired = false;
        verified_dirs::HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |at| {
                if at == point && !fired {
                    fired = true;
                    fs::rename(&replaced, &retained).unwrap();
                    fs::create_dir(&replaced).unwrap();
                    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o700)).unwrap();
                    private(
                        &replaced.join(".credentials.json"),
                        "foreign credential must survive",
                    );
                }
            }))
        });
        let _ = drive(&e, &mut c);
        assert!(original.is_dir(), "rename boundary reached for {mode}");
        assert_eq!(
            fs::read(dir.join(".credentials.json")).unwrap(),
            b"foreign credential must survive",
            "{mode}"
        );
        assert!(e.rs.journal.exists(), "retain uncertain custody for {mode}");
        assert!(
            !e.rs.dir.join("swap.json").exists(),
            "no success for {mode}"
        );
        if mode == "exchange" {
            assert_eq!(ino_at(&original.join(".credentials.json")), Some(e.l));
            assert_eq!(ino_at(&e.live()), Some(e.t));
        } else if !duplicate {
            assert_eq!(ino_at(&original.join(".credentials.json")), Some(e.l));
        } else {
            assert_eq!(ino_at(&e.live()), Some(e.l));
        }
        assert!(!log(&e).contains("swap work->"));
    }
}

#[test]
fn failed_custody_sync_retains_intent_and_recovery_never_exchanges_twice() {
    let e = fixture();
    let mut c = context(&e);
    let _reset = Reset;
    verified_dirs::FAIL_SYNC.with(|f| f.set(true));
    let _ = drive(&e, &mut c);
    // Immediate recovery may finish movement; every barrier still fails.
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert!(e.rs.journal.exists());
    assert!(!e.rs.dir.join("swap.json").exists());
    let intent = json(&e.rs.journal)["completion"].clone();
    let ledger = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(ledger.live.as_deref(), Some("work"));
    c.identity_budget.reset();
    assert!(recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).is_err());
    assert_eq!(json(&e.rs.journal)["completion"], intent);
    assert!(!e.rs.dir.join("swap.json").exists());
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    verified_dirs::FAIL_SYNC.with(|f| f.set(false));
    c.identity_budget.reset();
    assert!(matches!(
        recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).unwrap(),
        recover::RecoverOutcome::RolledForward { .. }
    ));
    let event = json(&e.rs.dir.join("swap.json"));
    assert_eq!(event["seq"], 1);
    assert_eq!(event["reading"]["measured_at"], T);
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert!(!e.rs.journal.exists());
    assert_eq!(
        recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).unwrap(),
        recover::RecoverOutcome::None
    );
    assert_eq!(json(&e.rs.dir.join("swap.json")), event);
}

#[test]
fn recovery_rejects_replaced_live_or_target_directory_bindings() {
    for live in [true, false] {
        let e = fixture();
        let mut c = context(&e);
        test_exchange::interrupted("s1", || {
            let _ = drive(&e, &mut c);
        });
        let dir = if live {
            e.home.join(".claude")
        } else {
            e.root.join("spare")
        };
        let old = e._tmp.path().join("retained");
        fs::rename(&dir, &old).unwrap();
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        // Even the expected credential inode cannot authorize a new directory.
        fs::rename(old.join(".credentials.json"), dir.join(".credentials.json")).unwrap();
        let before = fs::read(&e.rs.journal).unwrap();
        c.identity_budget.reset();
        assert!(recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).is_err());
        assert_eq!(fs::read(&e.rs.journal).unwrap(), before);
        assert!(!e.rs.dir.join("swap.json").exists());
    }
}

#[test]
fn explicit_switch_honors_recovery_origin_spending_and_allows_unblocked_custody() {
    let e = fixture();
    private(
        &e.slot("work"),
        std::str::from_utf8(&fs::read(e.slot("extra")).unwrap()).unwrap(),
    );
    let mut c = context(&e);
    assert_eq!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    let recovery = e
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|n| n.starts_with("recovered-"))
        .unwrap();
    let before_live = ino_at(&e.live());
    pin::write(&e.rs, &n("extra")).unwrap();
    let before_pin = pin::read(&e.rs);
    let mut ledger = e.rs.load_ledger().unwrap().unwrap();
    ledger.overflow_blocked.insert(
        "work".into(),
        super::super::super::ledger::OverflowBlock {
            at: T - 100,
            until: T + 1000,
            reason: "spend_limit".into(),
        },
    );
    e.rs.save_ledger(&ledger).unwrap();
    let result = test_exchange::within(e._tmp.path(), || {
        super::super::super::switch::switch_in(context(&e), &n(&recovery))
    })
    .unwrap()
    .unwrap();
    assert!(
        matches!(result, super::super::super::switch::Switched::DefaultOnly { ref reason, .. }
        if reason.contains("work") && reason.contains("spend")),
        "{result:?}"
    );
    assert_eq!(pin::read(&e.rs), before_pin);
    assert_eq!(ino_at(&e.live()), before_live);
    assert_eq!(ino_at(&e.slot(&recovery)), Some(e.l));
    ledger.overflow_blocked.clear();
    e.rs.save_ledger(&ledger).unwrap();
    let result = test_exchange::within(e._tmp.path(), || {
        super::super::super::switch::switch_in(context(&e), &n(&recovery))
    })
    .unwrap()
    .unwrap();
    assert!(
        matches!(result, super::super::super::switch::Switched::Moved { .. }),
        "{result:?}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(pin::read(&e.rs), pin::PinState::Pinned(n(&recovery)));
}

fn organisations(e: &Env, extra: Option<&str>) {
    let state = State::new(&e.root).unwrap();
    for (name, org) in [
        ("work", Some("org-a")),
        ("spare", Some("org-a")),
        ("extra", extra),
    ] {
        let dir = if name == "work" {
            e.home.join(".claude")
        } else {
            e.root.join(name)
        };
        let mut credential = json(&dir.join(".credentials.json"));
        credential["task1753FakeIdentity"]["organizationUuid"] = serde_json::json!(org);
        private(&dir.join(".credentials.json"), &credential.to_string());
        let account = super::super::tests_fixture_identity::read(&dir).unwrap();
        private(
            &if name == "work" {
                e.home.join(".claude.json")
            } else {
                dir.join(".claude.json")
            },
            &serde_json::json!({"oauthAccount":account.block()}).to_string(),
        );
        crate::commands::profile::expected::record(&state, &n(name), &account).unwrap();
        if name == "work" {
            let mut l = e.rs.load_ledger().unwrap().unwrap();
            l.login_identity = Some(super::super::super::login_identity::Identity::new(&account));
            e.rs.save_ledger(&l).unwrap();
        }
    }
}

#[test]
fn refused_same_org_candidate_allows_unresolved_and_cross_org_fallback() {
    for org in [None, Some("org-b")] {
        let e = fixture();
        organisations(&e, org);
        let replacement = ino_at(&e.slot("extra"));
        fs::create_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
        let mut c = context(&e);
        assert_eq!(
            drive(&e, &mut c).unwrap(),
            CycleOutcome::Swapped,
            "{}",
            log(&e)
        );
        assert_eq!(ino_at(&e.live()), replacement);
        assert_eq!(ino_at(&e.slot("work")), Some(e.l));
        assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
        assert_eq!(json(&e.rs.dir.join("swap.json"))["live_login"], "extra");
    }
}

#[test]
fn refused_plan_observations_still_prevent_spending_on_overflow() {
    for org in [None, Some("org-b")] {
        let e = fixture();
        organisations(&e, org);
        private(
            &e.root.join(".auto-switch"),
            "enabled=off\noverflow=extra\n",
        );
        fs::create_dir(e.root.join("spare/.oauth_refresh.lock")).unwrap();
        let mut c = context(&e);
        assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
        assert_eq!(ino_at(&e.live()), Some(e.l));
        assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
        assert!(!e.rs.dir.join("swap.json").exists());
    }
}

#[test]
fn unsafe_profile_assessment_preserves_its_episode_and_rotates_safe_profiles() {
    let e = fixture();
    let extra = ino_at(&e.slot("extra"));
    let entry = serde_json::json!({"first_seen":T-100, "alert_id":uuid::Uuid::new_v4().to_string(), "alerted":true,
        "evidence":{"dir":e.root.join("extra"), "directory":[0,0,0,0], "file":null}});
    private(
        &e.rs.dir.join("login-state.json"),
        &serde_json::json!({"version":1,"profiles":{"extra":entry}}).to_string(),
    );
    fs::set_permissions(e.root.join("extra"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut c = context(&e);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("extra")), extra);
    assert_eq!(
        json(&e.rs.dir.join("login-state.json"))["profiles"]["extra"],
        entry
    );
    assert!(!reads().iter().any(|p| p.ends_with("/extra")));
}

#[test]
fn unbound_version_two_recovery_retains_credentials_and_journal() {
    let e = fixture();
    let mut c = context(&e);
    test_exchange::interrupted("s1", || {
        let _ = drive(&e, &mut c);
    });
    let mut journal = json(&e.rs.journal);
    journal
        .as_object_mut()
        .unwrap()
        .remove("movement_directories");
    private(&e.rs.journal, &journal.to_string());
    let before = fs::read(&e.rs.journal).unwrap();
    c.identity_budget.reset();
    let result = recover::recover_at(&e.store, &e.rs, &e.home, c.identity_reader(), c.now).unwrap();
    assert!(
        matches!(result, recover::RecoverOutcome::Inconsistent(ref reason) if reason.contains("bindings"))
    );
    assert_eq!(fs::read(&e.rs.journal).unwrap(), before);
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.l));
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn secure_movement_traversal_rejects_a_symlink_hidden_by_profile_canonicalization() {
    let e = fixture();
    let alias = e._tmp.path().join("alias");
    std::os::unix::fs::symlink(e._tmp.path(), &alias).unwrap();
    let store = crate::commands::profile::store::ProfileStore::new(alias.join("root"));
    assert!(store.verify_profile_dir(&n("spare")).is_ok());
    let c = context(&e);
    let result = test_exchange::within(e._tmp.path(), || {
        super::super::super::swap::swap_with(
            &store,
            &e.rs,
            &e.home,
            super::super::super::swap::Source::Profile(n("work")),
            &n("spare"),
            T,
            c.identity_reader(),
            None,
        )
    });
    assert!(result.is_err());
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(!e.rs.journal.exists());
    assert!(!e.rs.dir.join("swap.json").exists());
}
