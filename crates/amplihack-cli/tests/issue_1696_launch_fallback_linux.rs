//! Linux lease/liveness contracts adapted from saved d75ba219, reconciled
//! with current organisation affinity. No rotator-home fallback is included.
#![cfg(target_os = "linux")]

use super::issue_1696_fixture::{Fixture, Preserved, now, private};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn leased() -> Fixture {
    let f = Fixture::new();
    f.swap();
    f
}

fn assert_session_access(f: &Fixture, before: &Preserved) {
    before.assert_unchanged();
    let out = f.run(
        f.launch("claude")
            .env("AMPLIHACK_CLAUDE_PROFILE", "X")
            .env("CLAUDE_CONFIG_DIR", f.dir("X"))
            .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0"),
    );
    assert!(
        out.status.success(),
        "original session lost access: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let env = fs::read_to_string(f.records.join("env.X")).unwrap();
    assert!(env.starts_with(&format!("{}\nX\n", f.dir("X").display())));
    f.assert_child_login("X", &f.dir("X"));
    before.assert_unchanged();
}

fn eligible_fallback(entry: &str) {
    for enabled in ["1", "0"] {
        let f = leased();
        let before = Preserved::capture(&f);
        let out = f.run(
            f.launch(entry)
                .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", enabled)
                .env("CLAUDE_CONFIG_DIR", f.dir("X")),
        );
        before.assert_unchanged();
        f.assert_selected(&out, "Z");
        let note = String::from_utf8_lossy(&out.stderr);
        assert!(
            note.contains('X') && note.contains('Y') && note.contains('Z'),
            "announce source, hosted login and selection: {note}"
        );
        let args = fs::read(f.records.join("args.Z")).unwrap();
        let args: Vec<_> = args.split(|b| *b == 0).filter(|b| !b.is_empty()).collect();
        assert!(
            args.windows(2)
                .any(|w| w == [b"--resume".as_slice(), b"session with spaces".as_slice()]),
            "{args:?}"
        );
        assert_session_access(&f, &before);
    }
}

#[test]
fn claude_uses_eligible_fallback_despite_ambient_override() {
    eligible_fallback("claude");
}

#[test]
fn launch_uses_eligible_fallback_despite_ambient_override() {
    eligible_fallback("launch");
}

fn no_fallback(entry: &str) {
    let f = leased();
    f.seed_usage(&[
        ("X", 100.0, 100.0),
        ("Y", 100.0, 100.0),
        ("Z", 100.0, 100.0),
    ]);
    let before = Preserved::capture(&f);
    let out = f.run(&mut f.launch(entry));
    before.assert_unchanged();
    f.assert_recovery(&out);
    assert_session_access(&f, &before);
}

#[test]
fn claude_without_usable_fallback_reports_immediate_recovery() {
    no_fallback("claude");
}

#[test]
fn launch_without_usable_fallback_reports_immediate_recovery() {
    no_fallback("launch");
}

#[test]
fn excluded_weekly_exhausted_and_parked_logins_are_not_fallbacks() {
    for entry in ["claude", "launch"] {
        for reason in ["excluded", "weekly", "parked", "lent"] {
            let f = leased();
            match reason {
                "excluded" => private(
                    &f.root.join(".auto-switch"),
                    "enabled=on\nthreshold=90\nexclude=Z\n",
                ),
                "weekly" => f.seed_usage(&[("X", 0.0, 0.0), ("Y", 0.0, 0.0), ("Z", 1.0, 100.0)]),
                "parked" | "lent" => {
                    f.seed_usage(&[("X", 0.0, 0.0), ("Y", 0.0, 0.0), ("Z", 100.0, 100.0)])
                }
                _ => unreachable!(),
            }
            let before = Preserved::capture(&f);
            let out = f.run(&mut f.launch(entry));
            before.assert_unchanged();
            f.assert_recovery(&out);
        }
    }
}

#[test]
fn missing_login_and_unmarked_api_key_do_not_establish_usability() {
    for reason in [
        "no-login",
        "empty-login",
        "invalid-login",
        "unmarked-api-key",
        "wrong-identity",
    ] {
        let f = leased();
        let login = f.dir("Z").join(".credentials.json");
        match reason {
            "no-login" => fs::rename(&login, f.home.join("unavailable-Z.json")).unwrap(),
            "empty-login" => private(&login, "{}"),
            "invalid-login" => private(&login, "not json"),
            "unmarked-api-key" => {
                private(&f.root.join(".auto-switch"), "enabled=on\nkind=Z:api-key\n");
                fs::rename(&login, f.home.join("unavailable-Z.json")).unwrap();
            }
            "wrong-identity" => private(
                &f.dir("Z").join(".claude.json"),
                json!({"oauthAccount":{"accountUuid":"unexpected","organizationUuid":"org-team"}})
                    .to_string(),
            ),
            _ => unreachable!(),
        }
        let out = f.run(&mut f.launch("claude"));
        f.assert_recovery(&out);
    }
}

#[test]
fn overflow_requires_exhausted_plan_logins_and_an_open_unblocked_gate() {
    for (plan, wait, blocked, expected) in [
        (100.0, "1800", false, true),
        (87.0, "1800", false, false),
        (100.0, "7200", false, false),
        (100.0, "1800", true, false),
    ] {
        let f = leased();
        private(
            &f.root.join(".auto-switch"),
            "enabled=on\nthreshold=90\noverflow=Z\n",
        );
        f.seed_usage(&[("X", plan, plan), ("Y", plan, plan), ("Z", 0.0, 0.0)]);
        if blocked {
            fs::create_dir_all(f.state().join("rotate")).unwrap();
            fs::set_permissions(f.state().join("rotate"), fs::Permissions::from_mode(0o700))
                .unwrap();
            private(
                &f.state().join("rotate/ledger.json"),
                json!({"version":1,"overflow_blocked":{
                    "Z":{"at":now(),"until":now()+86400,"reason":"spend_limit"}
                }})
                .to_string(),
            );
        }
        let before = Preserved::capture(&f);
        let out = f.run(
            f.launch("claude")
                .env("AMPLIHACK_ROTATE_OVERFLOW_WAIT", wait),
        );
        before.assert_unchanged();
        if expected {
            f.assert_selected(&out, "Z");
        } else {
            f.assert_recovery(&out);
        }
    }
}

#[test]
fn available_plan_login_precedes_overflow_even_with_less_room() {
    let f = leased();
    f.add("W");
    private(
        &f.root.join(".auto-switch"),
        "enabled=on\nthreshold=90\noverflow=Z\n",
    );
    f.seed_usage(&[
        ("X", 100.0, 100.0),
        ("Y", 100.0, 100.0),
        ("W", 50.0, 50.0),
        ("Z", 0.0, 0.0),
    ]);
    let before = Preserved::capture(&f);
    let out = f.run(
        f.launch("claude")
            .env("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "1800"),
    );
    before.assert_unchanged();
    f.assert_selected(&out, "W");
}

#[test]
fn known_readings_precede_unknown_and_missing_readings_remain_eligible() {
    for (readings, expected) in [
        (vec![("W", 20.0, 20.0), ("Z", 10.0, 10.0)], "Z"),
        (vec![("W", 10.0, 10.0), ("Z", 10.0, 10.0)], "W"),
        (vec![("Z", 30.0, 30.0)], "Z"),
        (vec![], "W"),
    ] {
        let f = leased();
        f.add("W");
        f.seed_usage(&readings);
        let out = f.run(&mut f.launch("claude"));
        f.assert_selected(&out, expected);
    }
}

#[test]
fn affinity_uses_the_hosted_login_and_preserves_same_organisation_preference() {
    let f = leased();
    f.add("W");
    // X's own login belongs elsewhere, Y's borrowed login and W are peers.
    private(
        &f.state().join("expected/X.json"),
        json!({"account":"uuid-X","org":"org-other"}).to_string(),
    );
    f.set_organisation("Z", "org-other");
    f.seed_usage(&[
        ("X", 100.0, 100.0),
        ("Y", 100.0, 100.0),
        ("W", 50.0, 50.0),
        ("Z", 1.0, 1.0),
    ]);
    let out = f.run(&mut f.launch("claude"));
    f.assert_selected(&out, "W");
}

#[test]
fn corrupt_candidate_state_fails_closed_with_its_cause() {
    for (path, cause) in [("usage.json", "usage"), ("rotate/ledger.json", "ledger")] {
        let f = leased();
        let damaged = f.state().join(path);
        private(&damaged, "not json");
        fs::set_permissions(damaged.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        let before = Preserved::capture(&f);
        let out = f.run(&mut f.launch("claude"));
        before.assert_unchanged();
        assert!(!out.status.success());
        assert!(f.attempts().is_empty());
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .to_lowercase()
                .contains(cause),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(fs::read_to_string(damaged).unwrap(), "not json");
    }
}

#[test]
fn fallback_identity_survives_a_descendant_launch() {
    let f = leased();
    let before = Preserved::capture(&f);
    let out = f.run(
        f.launch("claude")
            .env("DESCEND_ONCE", "1")
            .env("CLAUDE_CONFIG_DIR", f.dir("X")),
    );
    before.assert_unchanged();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(f.attempts(), ["Z", "Z"]);
    f.assert_child_login("Z", &f.dir("Z"));
    let env = fs::read_to_string(f.records.join("env.Z")).unwrap();
    assert_eq!(
        env,
        format!("{}\nZ\n{}\n", f.dir("Z").display(), f.root.display())
    );
}

#[test]
fn destination_invalidated_after_selection_is_refused_before_any_claude_attempt() {
    for invalidation in super::issue_1696_fixture::INVALIDATIONS {
        leased().assert_invalidation_refused(invalidation);
    }
}

#[test]
fn usage_limit_supervision_pins_the_selected_fallback_destination() {
    for entry in ["claude", "launch"] {
        let f = leased();
        let before = Preserved::capture(&f);
        let out = f.run(
            f.cmd(&[entry, "--subprocess-safe", "--", "-p", "supervised step"])
                .env("AMPLIHACK_USAGE_LIMIT_WAIT", "1")
                .env("CLAUDE_CONFIG_DIR", f.dir("X")),
        );
        before.assert_unchanged();
        f.assert_selected(&out, "Z");
        let context: serde_json::Value = serde_json::from_slice(
            &fs::read(f.records.join("context.Z"))
                .expect("supervised attempt must carry a pinned context"),
        )
        .unwrap();
        assert_eq!(context["format_version"], 1);
        assert_eq!(context["profile"], "Z");
        assert_eq!(context["directory"], f.dir("Z").to_str().unwrap());
        assert_eq!(context["root"], f.root.to_str().unwrap());
        assert_eq!(context["join_policy"], "never");
    }
}

#[test]
fn supervised_retry_receives_its_own_validated_profile_and_directory() {
    for entry in ["claude", "launch"] {
        let f = leased();
        f.add("W");
        f.seed_usage(&[
            ("X", 100.0, 100.0),
            ("Y", 100.0, 100.0),
            ("Z", 10.0, 10.0),
            ("W", 20.0, 20.0),
        ]);
        let before = Preserved::capture(&f);
        let out = f.run(
            f.cmd(&[entry, "--subprocess-safe", "--", "-p", "supervised step"])
                .env("AMPLIHACK_USAGE_LIMIT_WAIT", "1")
                .env("LIMIT_PROFILE", "Z")
                .env("CLAUDE_CONFIG_DIR", f.dir("X")),
        );
        before.assert_unchanged();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(f.attempts(), ["Z", "W"]);
        for name in ["Z", "W"] {
            f.assert_child_login(name, &f.dir(name));
            let env = fs::read_to_string(f.records.join(format!("env.{name}"))).unwrap();
            assert_eq!(
                env,
                format!("{}\n{name}\n{}\n", f.dir(name).display(), f.root.display())
            );
        }
    }
}
