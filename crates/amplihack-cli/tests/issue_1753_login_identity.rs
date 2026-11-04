//! Issue #1753: the login's server identity survives stale session JSON.
//! All credentials are fake and all endpoints are loopback, under the
//! repository's isolated_amplihack fixture and the task-owned TMPDIR.

#![cfg(unix)]

use crate::support::profile_rotate::{Fixture, account_json, ino, private_file, text};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

struct Endpoint {
    base: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    identities: Arc<Mutex<HashMap<String, serde_json::Value>>>,
}

impl Endpoint {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let identities = Arc::new(Mutex::new(HashMap::<String, serde_json::Value>::new()));
        let answers = Arc::clone(&identities);
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(found) => found,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("task-local identity server: {e}"),
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut token = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("authorization")
                    {
                        token = value.trim().to_string();
                    }
                }
                let who = ["work", "spare", "extra"]
                    .into_iter()
                    .find(|p| token == format!("Bearer fake-task1753-{p}"));
                let (status, body) = match who {
                    Some(p) if first.starts_with("GET /api/oauth/profile ") => (
                        "200 OK",
                        answers.lock().unwrap().get(p).cloned().unwrap_or_else(|| {
                            serde_json::json!({"account": {
                                "uuid": format!("uuid-{p}"),
                                "email": format!("{p}@example.com")
                            }})
                        }),
                    ),
                    Some(_) if first.starts_with("GET /api/oauth/usage ") => (
                        "200 OK",
                        serde_json::json!({
                            "five_hour": {"utilization": if token.ends_with("-work") { 90 } else { 10 }, "resets_at":
                                (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339()},
                            "seven_day": {"utilization": 20, "resets_at":
                                (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339()}
                        }),
                    ),
                    _ => (
                        "401 Unauthorized",
                        serde_json::json!({"error": "fake-only"}),
                    ),
                };
                let body = body.to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        Self {
            base,
            stop,
            thread: Some(thread),
            identities,
        }
    }

    fn identity(&self, who: &str, answer: serde_json::Value) {
        self.identities.lock().unwrap().insert(who.into(), answer);
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn fake_login(path: &std::path::Path, account: &str) {
    private_file(
        path,
        &serde_json::json!({"claudeAiOauth": {
            "accessToken": format!("fake-task1753-{account}"),
            "refreshToken": "fake-task1753-unused-refresh",
            "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000,
            "subscriptionType": "max"
        }})
        .to_string(),
    );
}

fn fixture() -> (Fixture, Endpoint) {
    let f = Fixture::new();
    f.assert_isolated();
    assert!(f.tmp.path().starts_with(std::env::temp_dir()));
    fake_login(&f.home_login(), "work");
    fake_login(&f.slot("spare"), "spare");
    fake_login(&f.slot("extra"), "extra");
    let state = f.root.join(".auto-switch.d");
    for dir in [state.clone(), state.join("rotate"), state.join("expected")] {
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for p in ["work", "spare", "extra"] {
        private_file(
            &state.join("expected").join(format!("{p}.json")),
            &serde_json::json!({"account": format!("uuid-{p}"), "org": null}).to_string(),
        );
    }
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::json!({"version": 1, "live": "work", "account": "uuid-work",
            "since": chrono::Utc::now().timestamp() - 1200,
            "swapped_at": chrono::Utc::now().timestamp() - 1200})
        .to_string(),
    );
    (f, Endpoint::new())
}

fn run(f: &Fixture, ep: &Endpoint, args: &[&str]) -> std::process::Output {
    let out = f
        .cmd(args)
        .env("AMPLIHACK_PROFILE_USAGE_API_BASE", &ep.base)
        .env(
            "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
            format!("{}/v1/oauth/token", ep.base),
        )
        .output()
        .unwrap();
    for value in [text(&out.stdout), text(&out.stderr), f.log()] {
        assert!(!value.contains("fake-task1753"), "fake token leaked");
    }
    out
}

#[test]
fn stale_session_b_does_not_refuse_swapping_credential_a_to_c() {
    let (f, ep) = fixture();
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
    );
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(
        out.status.success(),
        "A's trusted login must swap to C despite session JSON B: {}",
        text(&out.stderr)
    );
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("extra")), None);
    f.assert_each_login_once();
    assert!(!f.root.join(".rejected-logins").exists());
    assert!(!f.log().contains("recovered:"));
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "extra");
    assert_eq!(ledger["account"], "uuid-extra");

    // A session that started on B rewrites its old account after the swap.
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
    );
    let mut ledger = ledger;
    ledger["swapped_at"] = (chrono::Utc::now().timestamp() - 1200).into();
    private_file(&f.rotate_dir().join("ledger.json"), &ledger.to_string());
    assert!(
        run(&f, &ep, &["profile", "switch", "--none"])
            .status
            .success()
    );
    for cycle in 0..3 {
        // Rewriting repeatedly and running beyond grace must not be mistaken
        // for the separate consistent recovery readings required by #1882.
        private_file(
            &f.home.join(".claude.json"),
            &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
        );
        let out = run(&f, &ep, &["profile", "rotate"]);
        assert!(out.status.success(), "cycle {cycle}: {}", text(&out.stderr));
        let ledger = f.ledger().unwrap();
        assert_eq!(ledger["live"], "extra");
        assert_eq!(ledger["account"], "uuid-extra");
        assert!(
            ledger["hand_suspect"].is_null(),
            "session rewrite is not trusted suspicion"
        );
        f.assert_each_login_once();
        assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
        assert!(!f.root.join(".rejected-logins").exists());
        assert!(!f.log().contains("recovered:"));
    }
}

#[test]
fn unavailable_trusted_identity_moves_nothing() {
    let (f, ep) = fixture();
    let before = f.logins();
    let ledger = f.ledger().unwrap();
    fake_login(&f.home_login(), "unknown");
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(
        !out.status.success(),
        "unknown identity must refuse the swap"
    );
    assert!(
        !text(&out.stderr).is_empty(),
        "identity refusal must be visible"
    );
    assert_eq!(f.logins(), before);
    assert_eq!(f.ledger().unwrap(), ledger);
    assert!(!f.root.join(".rejected-logins").exists());
}

#[test]
fn credential_conflict_moves_nothing_even_when_session_matches_record() {
    let (f, ep) = fixture();
    let before = f.logins();
    let ledger = f.ledger().unwrap();
    private_file(
        &f.root.join(".auto-switch.d/expected/work.json"),
        &serde_json::json!({"account": "uuid-spare", "org": null}).to_string(),
    );
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
    );
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(
        !out.status.success(),
        "trusted account conflicts with the record"
    );
    assert!(!text(&out.stderr).is_empty(), "conflict must be visible");
    assert_eq!(f.logins(), before);
    assert_eq!(f.ledger().unwrap(), ledger);
    assert!(!f.root.join(".rejected-logins").exists());
}

#[test]
fn incoming_evidence_must_satisfy_its_record_before_any_login_moves() {
    for answer in [
        serde_json::json!({}),
        serde_json::json!({"account": {"email": "extra@example.com"}}),
        serde_json::json!({"account": {"uuid": "uuid-extra"},
            "organization": {"uuid": "another-organization"}}),
    ] {
        let (f, ep) = fixture();
        let before = f.logins();
        let ledger = f.ledger().unwrap();
        ep.identity("extra", answer);
        let out = run(&f, &ep, &["profile", "switch", "extra"]);
        assert!(
            !out.status.success(),
            "uncertain or conflicting target accepted"
        );
        assert!(!text(&out.stderr).is_empty(), "refusal must be actionable");
        assert_eq!(f.logins(), before);
        assert_eq!(f.ledger().unwrap(), ledger);
        f.assert_each_login_once();
        assert!(!f.root.join(".rejected-logins").exists());
    }
}

#[test]
fn unusable_expected_record_is_not_permission_to_adopt_the_login() {
    let (f, ep) = fixture();
    let before = f.logins();
    let ledger = f.ledger().unwrap();
    private_file(&f.root.join(".auto-switch.d/expected/work.json"), "{}");
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(!out.status.success(), "an unusable record must fail closed");
    assert_eq!(f.logins(), before);
    assert_eq!(f.ledger().unwrap(), ledger);
    assert_eq!(
        std::fs::read_to_string(f.root.join(".auto-switch.d/expected/work.json")).unwrap(),
        "{}"
    );
}

#[test]
fn unread_duplicate_evidence_cannot_authorize_backfill_or_movement() {
    let (f, ep) = fixture();
    std::fs::remove_file(f.root.join(".auto-switch.d/expected/work.json")).unwrap();
    fake_login(&f.slot("spare"), "unknown");
    let before = f.logins();
    let ledger = f.ledger().unwrap();
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(
        !out.status.success(),
        "a failed duplicate read is not an unclaimed login"
    );
    assert_eq!(f.logins(), before);
    assert_eq!(f.ledger().unwrap(), ledger);
    assert!(!f.root.join(".auto-switch.d/expected/work.json").exists());
    f.assert_each_login_once();
}

#[test]
fn identity_refusal_is_logged_on_each_affected_rotate_cycle() {
    let (f, ep) = fixture();
    fake_login(&f.home_login(), "unknown");
    let before = f.logins();
    for cycle in 0..3 {
        let alerts = f.log().matches("ALERT:").count();
        let _out = run(&f, &ep, &["profile", "rotate"]);
        assert!(
            f.log().matches("ALERT:").count() > alerts,
            "persistent identity refusal disappeared on cycle {cycle}"
        );
        assert_eq!(f.logins(), before);
        let ledger = f.ledger().unwrap();
        assert_eq!(ledger["live"], "work");
        assert_eq!(ledger["account"], "uuid-work");
        f.assert_each_login_once();
        assert!(!f.root.join(".rejected-logins").exists());
    }
}

#[test]
fn legacy_email_records_and_stale_target_sessions_remain_supported() {
    let (f, ep) = fixture();
    for p in ["work", "spare", "extra"] {
        private_file(
            &f.root.join(format!(".auto-switch.d/expected/{p}.json")),
            &serde_json::json!({"account": format!("{p}@example.com"), "org": null}).to_string(),
        );
    }
    private_file(
        &f.root.join("extra/.claude.json"),
        &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
    );
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(
        out.status.success(),
        "trusted target refused: {}",
        text(&out.stderr)
    );
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    assert_eq!(f.ledger().unwrap()["account"], "uuid-extra");
    f.assert_each_login_once();
    assert!(!f.root.join(".rejected-logins").exists());
}

#[test]
fn stale_session_does_not_block_automatic_rotation() {
    let (f, ep) = fixture();
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
    );
    let out = run(&f, &ep, &["profile", "rotate"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(f.ledger().unwrap()["live"], "extra");
    assert_eq!(f.ledger().unwrap()["account"], "uuid-extra");
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    f.assert_each_login_once();
    assert!(!f.root.join(".rejected-logins").exists());
}
