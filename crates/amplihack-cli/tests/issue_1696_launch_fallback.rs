//! Portable observable contracts; active Linux swaps are covered separately.
#![cfg(unix)]

use super::issue_1696_fixture::{Fixture, Preserved, private};
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// The private attempt hand-off is a file, never credentials in an environment
/// variable. These tests define its internal name for the implementation step.
const CONTEXT_ENV: &str = "AMPLIHACK_CLAUDE_LAUNCH_CONTEXT";

#[test]
fn recorded_default_hosting_conflict_uses_named_fallback_and_preserves_resume_arguments() {
    for entry in ["claude", "launch"] {
        for enabled in ["1", "0"] {
            let f = Fixture::new();
            f.seed_hosted_lease();
            let before = Preserved::capture(&f);
            let out = f.run(
                f.launch(entry)
                    .env("CLAUDE_CONFIG_DIR", f.dir("X"))
                    .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", enabled),
            );
            before.assert_unchanged();
            f.assert_selected(&out, "Z");
            let args = fs::read(f.records.join("args.Z")).unwrap();
            let args: Vec<_> = args.split(|b| *b == 0).filter(|b| !b.is_empty()).collect();
            assert!(
                args.windows(2)
                    .any(|w| w == [b"--resume".as_slice(), b"session with spaces".as_slice()]),
                "{args:?}"
            );
            let note = String::from_utf8_lossy(&out.stderr);
            assert!(
                note.contains('X') && note.contains('Y') && note.contains('Z'),
                "{note}"
            );
        }
    }
}

#[test]
fn default_conflict_without_usable_fallback_reports_immediate_recovery() {
    for entry in ["claude", "launch"] {
        let f = Fixture::new();
        f.seed_hosted_lease();
        f.seed_usage(&[
            ("X", 100.0, 100.0),
            ("Y", 100.0, 100.0),
            ("Z", 100.0, 100.0),
        ]);
        let before = Preserved::capture(&f);
        let out = f.run(&mut f.launch(entry));
        before.assert_unchanged();
        f.assert_recovery(&out);
    }
}

#[test]
fn ordinary_default_exhaustion_preserves_resume_arguments_and_child_outcome() {
    for entry in ["claude", "launch"] {
        let f = Fixture::new();
        let out = f.run(f.launch(entry).env("CLAUDE_EXIT", "37"));
        assert_eq!(
            out.status.code(),
            Some(37),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            f.attempts(),
            ["X"],
            "exhaustion alone must not trigger fallback"
        );
        let args = fs::read(f.records.join("args.X")).unwrap();
        let args: Vec<_> = args.split(|b| *b == 0).filter(|b| !b.is_empty()).collect();
        assert!(
            args.windows(2)
                .any(|w| w == [b"--resume".as_slice(), b"session with spaces".as_slice()]),
            "{args:?}"
        );
        assert_eq!(fs::read(f.root.join(".default")).unwrap(), b"X\n");
    }
}

#[test]
fn explicit_and_unjoined_inherited_hosted_profiles_are_never_redirected() {
    for entry in ["claude", "launch"] {
        for inherited in [false, true] {
            let f = Fixture::new();
            f.seed_hosted_lease();
            let before = Preserved::capture(&f);
            let mut cmd = if inherited {
                let mut cmd = f.launch(entry);
                cmd.env("AMPLIHACK_CLAUDE_PROFILE", "X");
                cmd
            } else {
                f.cmd(&[
                    entry,
                    "--profile",
                    "X",
                    "--subprocess-safe",
                    "--",
                    "--resume",
                ])
            };
            // Do not let inherited exhaustion substitution hide this contract.
            cmd.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0");
            let out = f.run(&mut cmd);
            before.assert_unchanged();
            assert!(!out.status.success());
            assert!(f.attempts().is_empty());
            assert!(String::from_utf8_lossy(&out.stderr).contains("holds the login"));
        }
    }
}

#[test]
fn leased_login_stays_blocked_with_automatic_switching_disabled() {
    let f = Fixture::new();
    f.seed_hosted_lease();
    let before = Preserved::capture(&f);
    let out = f.run(
        f.cmd(&[
            "claude",
            "--profile",
            "Y",
            "--subprocess-safe",
            "--",
            "--resume",
        ])
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0"),
    );
    before.assert_unchanged();
    assert!(!out.status.success());
    assert!(f.attempts().is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("its login is in use"));
}

#[test]
fn explicit_and_inherited_destinations_take_precedence_over_conflicting_default() {
    for entry in ["claude", "launch"] {
        for inherited in [false, true] {
            let f = Fixture::new();
            f.seed_hosted_lease();
            let before = Preserved::capture(&f);
            let mut cmd = if inherited {
                let mut cmd = f.launch(entry);
                cmd.env("AMPLIHACK_CLAUDE_PROFILE", "Z");
                cmd
            } else {
                f.cmd(&[
                    entry,
                    "--profile",
                    "Z",
                    "--subprocess-safe",
                    "--",
                    "--resume",
                ])
            };
            let out = f.run(&mut cmd);
            before.assert_unchanged();
            f.assert_selected(&out, "Z");
        }
    }
}

#[test]
fn corrupt_default_pointer_is_not_a_fallback_trigger() {
    for contents in ["../escape\n", "missing\n"] {
        let f = Fixture::new();
        private(&f.root.join(".default"), contents);
        let out = f.run(&mut f.launch("claude"));
        assert!(!out.status.success());
        assert!(f.attempts().is_empty());
        assert_eq!(
            fs::read_to_string(f.root.join(".default")).unwrap(),
            contents
        );
    }
}

#[test]
fn supervised_reentry_rejects_invalid_pinned_context_before_claude_starts() {
    for entry in ["claude", "launch"] {
        for invalid in [
            "malformed",
            "version",
            "tuple",
            "root",
            "join",
            "missing",
            "unsafe",
        ] {
            let f = Fixture::new();
            f.seed_hosted_lease();
            let before = Preserved::capture(&f);
            let initial = f.run(
                f.cmd(&[entry, "--subprocess-safe", "--", "-p", "capture context"])
                    .env("AMPLIHACK_USAGE_LIMIT_WAIT", "1"),
            );
            f.assert_selected(&initial, "Z");
            let mut captured: serde_json::Value = serde_json::from_slice(
                &fs::read(f.records.join("context.Z"))
                    .expect("supervisor must pass a pinned context"),
            )
            .unwrap();
            let context = f.home.join("runtime/attempt-launch.json");
            private(&context, captured.to_string());
            fs::remove_file(f.records.join("attempts")).unwrap();
            // Establish a valid replay first. Otherwise an unrelated decoder
            // error could make every negative mutation falsely look covered.
            let control = f.run(
                f.launch(entry)
                    .env("AMPLIHACK_AUTO_SWITCH_SUPERVISED", "1")
                    .env("AMPLIHACK_CLAUDE_PROFILE", "Z")
                    .env("CLAUDE_CONFIG_DIR", f.dir("Z"))
                    .env(CONTEXT_ENV, &context),
            );
            f.assert_selected(&control, "Z");
            fs::remove_file(f.records.join("attempts")).unwrap();
            match invalid {
                "version" => captured["format_version"] = serde_json::json!(999),
                "tuple" => captured["directory"] = serde_json::json!(f.dir("X")),
                "root" => captured["root"] = serde_json::json!(f.home.join("other-root")),
                "join" => captured["join_policy"] = serde_json::json!("existing_session"),
                _ => {}
            }
            let body = if invalid == "malformed" {
                "not json".to_string()
            } else {
                captured.to_string()
            };
            fs::remove_file(&context).unwrap();
            if invalid != "missing" {
                private(&context, body);
            }
            if invalid == "unsafe" {
                fs::set_permissions(&context, fs::Permissions::from_mode(0o666)).unwrap();
            }
            let out = f.run(
                f.launch(entry)
                    .env("AMPLIHACK_AUTO_SWITCH_SUPERVISED", "1")
                    .env("AMPLIHACK_CLAUDE_PROFILE", "Z")
                    .env("CLAUDE_CONFIG_DIR", f.dir("Z"))
                    .env(CONTEXT_ENV, &context),
            );
            assert!(
                !out.status.success(),
                "{invalid} context accepted by {entry}"
            );
            assert!(f.attempts().is_empty(), "{invalid} context spawned Claude");
            before.assert_unchanged();
            let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
            assert!(
                err.contains("context") || err.contains("destination"),
                "{err}"
            );
        }
    }
}

#[test]
fn selected_destination_invalidation_is_refused_before_spawn_portably() {
    for invalidation in super::issue_1696_fixture::INVALIDATIONS {
        let f = Fixture::new();
        f.seed_hosted_lease();
        f.assert_invalidation_refused(invalidation);
    }
}

#[test]
fn fallback_descendants_retain_directory_identity_and_never_join_permission() {
    let f = Fixture::new();
    f.seed_hosted_lease();
    let before = Preserved::capture(&f);
    let out = f.run(
        f.launch("claude")
            .env("DESCEND_ONCE", "1")
            .env("CLAUDE_CONFIG_DIR", f.dir("X")),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(f.attempts(), ["Z", "Z"]);
    before.assert_unchanged();
    f.assert_child_login("Z", &f.dir("Z"));
    assert_eq!(
        fs::read_to_string(f.records.join("join.Z")).unwrap(),
        "never\n"
    );
}

#[test]
fn portable_fallback_preserves_exclusions_usage_and_identity_safeguards() {
    for reason in [
        "excluded",
        "weekly",
        "missing",
        "empty",
        "malformed",
        "identity",
        "state",
        "ledger",
    ] {
        let f = Fixture::new();
        f.seed_hosted_lease();
        match reason {
            "excluded" => private(&f.root.join(".auto-switch"), "enabled=on\nexclude=Z\n"),
            "weekly" => f.seed_usage(&[("Z", 1.0, 100.0)]),
            "missing" => fs::rename(
                f.dir("Z").join(".credentials.json"),
                f.home.join("unused-login"),
            )
            .unwrap(),
            "empty" => private(&f.dir("Z").join(".credentials.json"), "{}"),
            "malformed" => private(&f.dir("Z").join(".credentials.json"), "not json"),
            "identity" => private(&f.dir("Z").join(".claude.json"), "{}"),
            "state" => private(&f.state().join("usage.json"), "not json"),
            "ledger" => {
                fs::create_dir_all(f.state().join("rotate")).unwrap();
                fs::set_permissions(f.state().join("rotate"), fs::Permissions::from_mode(0o700))
                    .unwrap();
                private(&f.state().join("rotate/ledger.json"), "not json");
            }
            _ => unreachable!(),
        }
        let before = Preserved::capture(&f);
        let out = f.run(&mut f.launch("claude"));
        before.assert_unchanged();
        f.assert_recovery(&out);
        if reason == "state" || reason == "ledger" {
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(if reason == "state" {
                    "usage"
                } else {
                    "ledger"
                })
            );
        }
    }
}

#[test]
fn supervised_inherited_selection_does_not_gain_join_permission_from_environment_projection() {
    let f = Fixture::new();
    f.seed_hosted_lease();
    let before = Preserved::capture(&f);
    let out = f.run(
        f.cmd(&["claude", "--subprocess-safe", "--", "-p", "cannot join"])
            .env("AMPLIHACK_CLAUDE_PROFILE", "X")
            .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "1"),
    );
    assert!(!out.status.success());
    assert!(f.attempts().is_empty());
    before.assert_unchanged();
    assert!(String::from_utf8_lossy(&out.stderr).contains("holds the login"));
}

#[test]
fn portable_supervisor_retry_validates_each_destination() {
    for entry in ["claude", "launch"] {
        let f = Fixture::new();
        f.seed_hosted_lease();
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
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(f.attempts(), ["Z", "W"]);
        for name in ["Z", "W"] {
            f.assert_child_login(name, &f.dir(name));
            let context: serde_json::Value = serde_json::from_slice(
                &fs::read(f.records.join(format!("context.{name}"))).unwrap(),
            )
            .unwrap();
            assert_eq!(context["profile"], name);
            assert_eq!(context["join_policy"], "never");
            assert_eq!(context["directory"], f.dir(name).to_str().unwrap());
        }
        before.assert_unchanged();
    }
}

#[test]
fn descendant_refuses_credential_changes_to_the_pinned_fallback() {
    for print in ["", "1"] {
        let f = Fixture::new();
        f.seed_hosted_lease();
        let mutation = f.home.join("descendant-mutation.sh");
        private(
            &mutation,
            "printf '{}' > \"$CLAUDE_CONFIG_DIR/.credentials.json\"\n",
        );
        let pointer = fs::read(f.root.join(".default")).unwrap();
        let ledger = fs::read(f.state().join("ledger.json")).unwrap();
        let borrowed = fs::read(f.dir("X").join(".credentials.json")).unwrap();
        let out = f.run(
            f.launch("claude")
                .env("DESCEND_ONCE", "1")
                .env("DESCEND_PRINT", print)
                .env("DESCEND_MUTATION", mutation),
        );
        assert!(!out.status.success());
        assert_eq!(f.attempts(), ["Z"]);
        assert_eq!(fs::read(f.root.join(".default")).unwrap(), pointer);
        assert_eq!(fs::read(f.state().join("ledger.json")).unwrap(), ledger);
        assert_eq!(
            fs::read(f.dir("X").join(".credentials.json")).unwrap(),
            borrowed
        );
    }
}

#[test]
fn explicit_selection_overrides_an_inherited_launch_context() {
    let f = Fixture::new();
    f.seed_hosted_lease();
    let context = f.home.join("runtime/unrelated-context.json");
    private(&context, "malformed");
    let out = f.run(
        f.cmd(&[
            "claude",
            "--profile",
            "Z",
            "--subprocess-safe",
            "--",
            "--resume",
        ])
        .env(CONTEXT_ENV, context),
    );
    f.assert_selected(&out, "Z");
}

#[test]
fn the_original_hosted_session_keeps_its_inherited_join_permission() {
    let f = Fixture::new();
    f.seed_hosted_lease();
    let before = Preserved::capture(&f);
    let out = f.run(
        f.launch("claude")
            .env("AMPLIHACK_CLAUDE_PROFILE", "X")
            .env("CLAUDE_CONFIG_DIR", f.dir("X"))
            .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0"),
    );
    before.assert_unchanged();
    f.assert_selected(&out, "X");
}

#[test]
fn portable_affinity_follows_the_hosted_login_and_keeps_capacity_relief_rules() {
    for same_has_capacity in [true, false] {
        let f = Fixture::new();
        f.seed_hosted_lease();
        f.add("W");
        private(
            &f.state().join("expected/X.json"),
            serde_json::json!({"account":"uuid-X","org":"org-other"}).to_string(),
        );
        f.set_organisation("Z", "org-other");
        let same = if same_has_capacity { 50.0 } else { 100.0 };
        f.seed_usage(&[
            ("X", 100.0, 100.0),
            ("Y", 100.0, 100.0),
            ("W", same, same),
            ("Z", 1.0, 1.0),
        ]);
        let before = Preserved::capture(&f);
        let out = f.run(&mut f.launch("claude"));
        f.assert_selected(&out, if same_has_capacity { "W" } else { "Z" });
        before.assert_unchanged();
    }
}
