//! Each command is a fresh process with temporary credential and state roots.
#![cfg(unix)]
#[cfg(target_os = "linux")]
use crate::support::profile_2242::inode;
use crate::support::profile_2242::{Fixture, Reply, private, value};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn missing_token(f: &Fixture, name: &str) {
    private(&f.slot(name), br#"{"claudeAiOauth":{}}"#);
}

fn episode(f: &Fixture, name: &str) -> serde_json::Value {
    let path = f.state().join("login-state.json");
    assert!(
        path.exists(),
        "needs-login episodes must persist independently of the rotate ledger"
    );
    let state = value(&path);
    assert_eq!(state["version"], 1);
    let entry = state["profiles"][name].clone();
    assert!(
        entry["first_seen"].as_i64().is_some(),
        "persist a first-seen timestamp: {entry}"
    );
    entry
}

fn entry_alerts(f: &Fixture, name: &str) -> usize {
    f.log()
        .lines()
        .filter(|l| l.contains("ALERT") && l.contains("needs-login") && l.contains(name))
        .count()
}

fn summary(output: &std::process::Output) -> String {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<_> = text
        .lines()
        .filter(|l| l.trim_start().starts_with("needs-login:"))
        .collect();
    assert_eq!(lines.len(), 1, "one consolidated needs-login line: {text}");
    lines[0].trim().to_owned()
}

#[test]
fn missing_token_episode_survives_restarts_without_more_authentication_or_alerts() {
    let f = Fixture::new();
    missing_token(&f, "spare");
    let _ = f.rotate();
    let first = episode(&f, "spare");
    assert_eq!(
        fs::metadata(f.state().join("login-state.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let requests = f.requests("spare");
    for _ in 0..3 {
        let _ = f.rotate();
    }
    assert_eq!(episode(&f, "spare")["first_seen"], first["first_seen"]);
    assert_eq!(
        f.requests("spare"),
        requests,
        "unchanged episodes bypass identity, usage and refresh requests"
    );
    assert_eq!(entry_alerts(&f, "spare"), 1);
    let ledger = f.state().join("ledger.json");
    if ledger.exists() {
        assert_ne!(
            value(&ledger)["live"],
            "spare",
            "needs-login cannot become live"
        );
    }
}

#[test]
fn a_legitimately_live_credential_is_not_missing_from_its_empty_profile_slot() {
    let f = Fixture::new();
    assert!(!f.slot("work").exists());
    let _ = f.rotate();
    let path = f.state().join("login-state.json");
    if path.exists() {
        assert!(
            value(&path)["profiles"]["work"].is_null(),
            "resolve live custody before assessing token absence"
        );
    }
    assert_eq!(entry_alerts(&f, "work"), 0);
}

#[test]
fn changing_unusable_credentials_keeps_the_episode_and_repair_allows_a_later_alert() {
    let f = Fixture::new();
    missing_token(&f, "spare");
    let _ = f.rotate();
    let first = episode(&f, "spare");
    // Atomic replacement is different change evidence, but the same episode.
    let replacement = f.tmp.path().join("replacement");
    private(&replacement, br#"{"claudeAiOauth":{},"changed":true}"#);
    fs::rename(&replacement, f.slot("spare")).unwrap();
    let _ = f.rotate();
    assert_eq!(episode(&f, "spare")["first_seen"], first["first_seen"]);
    assert_ne!(
        episode(&f, "spare"),
        first,
        "persist the reassessed credential change evidence"
    );
    assert_eq!(entry_alerts(&f, "spare"), 1);
    f.credentials(&f.slot("spare"), "spare", false, true);
    f.ok(&["profile", "usage", "spare"]);
    assert!(
        value(&f.state().join("login-state.json"))["profiles"]["spare"].is_null(),
        "usable reassessment ends the episode"
    );
    missing_token(&f, "spare");
    let _ = f.rotate();
    let _ = episode(&f, "spare");
    assert_eq!(
        entry_alerts(&f, "spare"),
        2,
        "later episodes alert independently, even within one clock second"
    );
}

#[test]
fn credential_removal_and_creation_are_reassessed_on_an_existing_cycle() {
    let f = Fixture::new();
    fs::remove_file(f.slot("spare")).unwrap();
    let _ = f.rotate();
    let first = episode(&f, "spare");
    missing_token(&f, "spare");
    let _ = f.rotate();
    assert_eq!(episode(&f, "spare")["first_seen"], first["first_seen"]);
    f.credentials(&f.slot("spare"), "spare", false, true);
    f.ok(&["profile", "usage", "spare"]);
    assert!(value(&f.state().join("login-state.json"))["profiles"]["spare"].is_null());
}

#[test]
fn expired_credentials_without_refresh_or_with_rejected_refresh_need_login() {
    for rejected in [false, true] {
        let f = Fixture::new();
        f.credentials(&f.slot("spare"), "spare", true, rejected);
        if rejected {
            f.reply("spare", Reply::Rejected);
        }
        let _ = f.rotate();
        let _ = episode(&f, "spare");
        let requests = f.requests("spare");
        let _ = f.rotate();
        assert_eq!(
            f.requests("spare"),
            requests,
            "definitive rejection is not retried unchanged"
        );
        assert_eq!(entry_alerts(&f, "spare"), 1);
    }
}

#[test]
fn service_failure_and_refresh_lock_do_not_establish_needs_login() {
    for locked in [false, true] {
        let f = Fixture::new();
        f.credentials(&f.slot("spare"), "spare", true, true);
        if locked {
            fs::create_dir(f.root.join("spare/.oauth_refresh.lock")).unwrap();
        } else {
            f.reply("spare", Reply::Unavailable);
        }
        let _ = f.rotate();
        let path = f.state().join("login-state.json");
        if path.exists() {
            assert!(value(&path)["profiles"]["spare"].is_null());
        }
        assert_eq!(entry_alerts(&f, "spare"), 0);
    }
}

#[test]
fn a_temporary_reassessment_failure_keeps_changed_credentials_pending_until_repaired() {
    let f = Fixture::new();
    missing_token(&f, "spare");
    let _ = f.rotate();
    let first = episode(&f, "spare")["first_seen"].clone();
    f.credentials(&f.slot("spare"), "spare", true, true);
    f.reply("spare", Reply::Unavailable);
    let _ = f.rotate();
    assert_eq!(episode(&f, "spare")["first_seen"], first);
    assert_eq!(entry_alerts(&f, "spare"), 1);
    let requests = f.requests("spare").len();
    f.reply("spare", Reply::Healthy);
    // Same credential bytes: a temporary failure must not mark this binding
    // as conclusively assessed and suppress the next existing cycle.
    let _ = f.rotate();
    assert!(f.requests("spare").len() > requests);
    assert!(value(&f.state().join("login-state.json"))["profiles"]["spare"].is_null());
}

#[test]
fn usage_and_status_share_one_sorted_first_seen_list_without_authenticating_unchanged_entries() {
    let f = Fixture::new();
    missing_token(&f, "spare");
    missing_token(&f, "extra");
    let _ = f.rotate();
    let spare = episode(&f, "spare");
    let extra = episode(&f, "extra");
    let requests = (f.requests("spare"), f.requests("extra"));
    let usage = summary(&f.run(&["profile", "usage"]));
    let status = summary(&f.ok(&["profile", "auto-switch", "status"]));
    for line in [&usage, &status] {
        let extra_position = line
            .find("extra")
            .expect("include extra in the consolidated list");
        let spare_position = line
            .find("spare")
            .expect("include spare in the consolidated list");
        assert!(
            extra_position < spare_position,
            "sort affected profiles: {line}"
        );
        assert!(
            line.contains("first") || line.contains("since"),
            "include first-seen information: {line}"
        );
        for entry in [&spare, &extra] {
            let first = chrono::DateTime::from_timestamp(entry["first_seen"].as_i64().unwrap(), 0)
                .unwrap()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            assert!(
                line.contains(&first),
                "include the persisted UTC first-seen time: {line}"
            );
        }
        assert!(
            line.contains("amplihack profile login"),
            "provide an actionable profile login command on the summary line: {line}"
        );
        assert!(
            line.contains("/login"),
            "include the live Claude login action: {line}"
        );
    }
    assert_eq!(
        usage, status,
        "both commands use the same persisted summary"
    );
    // Do not compare relative ages from separate processes: a clock tick
    // between commands must not create a test window.
    assert_eq!((f.requests("spare"), f.requests("extra")), requests);
    assert_eq!(episode(&f, "spare")["first_seen"], spare["first_seen"]);
    assert_eq!(episode(&f, "extra")["first_seen"], extra["first_seen"]);
}

#[test]
fn changed_locally_unexpired_credentials_do_not_clear_needs_login_without_trusted_verification() {
    for reply in [Reply::Rejected, Reply::Unavailable] {
        let f = Fixture::new();
        missing_token(&f, "spare");
        let _ = f.rotate();
        let first = episode(&f, "spare");
        f.credentials(&f.slot("spare"), "spare", false, true);
        f.reply("spare", reply);
        let _ = f.run(&["profile", "usage", "spare"]);
        assert_eq!(
            episode(&f, "spare")["first_seen"],
            first["first_seen"],
            "local token expiry is not evidence of successful authentication"
        );
        assert_eq!(entry_alerts(&f, "spare"), 1);
        let _ = f.rotate();
        assert_eq!(episode(&f, "spare")["first_seen"], first["first_seen"]);
        assert!(
            !f.state().join("swap.json").exists()
                || value(&f.state().join("swap.json"))["live_login"] != "spare"
        );
        f.reply("spare", Reply::Healthy);
        // An uncertain verification remains retryable with the same file.
        // A definitive rejection requires a changed credential before retrying.
        if matches!(reply, Reply::Rejected) {
            let replacement = f.tmp.path().join("repaired-spare");
            f.credentials(&replacement, "spare", false, true);
            fs::rename(replacement, f.slot("spare")).unwrap();
        }
        let before = f.requests("spare").len();
        f.ok(&["profile", "usage", "spare"]);
        assert!(
            f.requests("spare")[before..]
                .iter()
                .any(|p| p == "/api/oauth/profile"),
            "clearing needs-login requires trusted server identity"
        );
        assert!(value(&f.state().join("login-state.json"))["profiles"]["spare"].is_null());
    }
}

#[test]
fn an_authenticated_replacement_for_the_wrong_account_does_not_clear_needs_login() {
    let f = Fixture::new();
    missing_token(&f, "spare");
    let _ = f.rotate();
    let first = episode(&f, "spare");
    let expected = f.root.join(".auto-switch.d/expected/spare.json");
    let owner = fs::read(&expected).unwrap();
    f.credentials(&f.slot("spare"), "extra", false, true);
    let _ = f.run(&["profile", "usage", "spare"]);
    assert_eq!(
        episode(&f, "spare")["first_seen"],
        first["first_seen"],
        "an unrelated authenticated account cannot repair this profile's episode"
    );
    assert_eq!(fs::read(expected).unwrap(), owner);
    assert_eq!(entry_alerts(&f, "spare"), 1);
}

#[test]
fn unreadable_login_health_is_preserved_and_reported_as_unknown() {
    let f = Fixture::new();
    let path = f.state().join("login-state.json");
    private(&path, b"{broken");
    fs::set_permissions(f.state(), fs::Permissions::from_mode(0o700)).unwrap();
    let o = f.run(&["profile", "auto-switch", "status"]);
    assert!(summary(&o).contains("unknown"));
    assert_eq!(fs::read(path).unwrap(), b"{broken");
}

#[test]
fn unsafe_login_health_state_is_reported_without_following_or_replacing_it() {
    for symlink in [false, true] {
        let f = Fixture::new();
        let path = f.state().join("login-state.json");
        let bytes = vec![b' '; 64 * 1024 + 1];
        let outside = f.tmp.path().join("outside-state");
        if symlink {
            private(&outside, b"preserve this private fixture");
            fs::create_dir_all(f.state()).unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
        } else {
            private(&path, &bytes);
        }
        fs::set_permissions(f.state(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(summary(&f.run(&["profile", "auto-switch", "status"])).contains("unknown"));
        if symlink {
            assert!(
                fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read(outside).unwrap(), b"preserve this private fixture");
        } else {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn interrupted_entry_alert_delivery_replays_once_using_the_persisted_episode() {
    for point in ["login-alert-pending", "login-alert-delivered"] {
        let f = Fixture::new();
        missing_token(&f, "spare");
        let o = f
            .command(&["profile", "rotate"])
            .env("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER", point)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(86), "required alert boundary {point}");
        let first = episode(&f, "spare")["first_seen"].clone();
        let _ = f.rotate();
        let _ = f.rotate();
        assert_eq!(entry_alerts(&f, "spare"), 1, "keyed replay at {point}");
        assert_eq!(episode(&f, "spare")["first_seen"], first);
    }
}

#[test]
// Successful rotation requires Linux session inspection and atomic exchange.
#[cfg(target_os = "linux")]
fn unsafe_profile_assessment_is_isolated_and_preserves_its_existing_episode() {
    let f = Fixture::new();
    let spare = inode(&f.slot("spare"));
    let outgoing = inode(&f.live());
    let unsafe_login = inode(&f.slot("extra"));
    let entry = serde_json::json!({"first_seen":1_790_000_000_i64,
        "alert_id":"00000000-0000-4000-8000-000000000042", "alerted":true,
        "evidence":{"dir":f.root.join("extra"),"directory":[0,0,0,0],"file":null}});
    private(
        &f.state().join("login-state.json"),
        serde_json::json!({"version":1,"profiles":{"extra":entry}})
            .to_string()
            .as_bytes(),
    );
    fs::set_permissions(f.root.join("extra"), fs::Permissions::from_mode(0o755)).unwrap();
    let output = f.rotate();
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{diagnostic}");
    assert!(
        diagnostic
            .contains("profile extra: authentication assessment skipped: unsafe profile directory"),
        "{diagnostic}"
    );
    assert_eq!(inode(&f.live()), spare, "{}", f.log());
    assert_eq!(inode(&f.slot("work")), outgoing);
    assert_eq!(inode(&f.slot("extra")), unsafe_login);
    assert_eq!(
        value(&f.state().join("login-state.json"))["profiles"]["extra"],
        entry
    );
}
