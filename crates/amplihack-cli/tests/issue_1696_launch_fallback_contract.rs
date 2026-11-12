//! Remaining #1696 contracts against the continued implementation checkpoint.
//! All credentials and endpoints belong to Fixture; no login Keychain access.
//! AMPLIHACK_TEST_KEYCHAIN_STORE defines the debug-only storage-injection
//! contract for the implementation step, never a public configuration option.
#![cfg(unix)]

use super::issue_1696_fixture::{Fixture, Preserved, private};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const CONTEXT_ENV: &str = "AMPLIHACK_CLAUDE_LAUNCH_CONTEXT";

fn leased() -> Fixture {
    let f = Fixture::new();
    f.seed_hosted_lease();
    f
}

fn supervised(f: &Fixture, entry: &str) -> Command {
    let mut cmd = f.cmd(&[entry, "--subprocess-safe", "--", "-p", "supervised step"]);
    cmd.env("AMPLIHACK_USAGE_LIMIT_WAIT", "1");
    cmd
}

/// A captured intent may be replayed, but its identity must be authenticated
/// afresh by the process which will actually create the next Claude child.
fn capture_context(f: &Fixture, entry: &str) -> (PathBuf, Value) {
    let out = f.run(&mut supervised(f, entry));
    f.assert_selected(&out, "Z");
    let captured: Value =
        serde_json::from_slice(&fs::read(f.records.join("context.Z")).unwrap()).unwrap();
    let path = f.home.join("runtime/reentry.json");
    private(&path, captured.to_string());
    fs::remove_file(f.records.join("attempts")).unwrap();
    (path, captured)
}

fn reentry(f: &Fixture, entry: &str, path: &std::path::Path) -> Command {
    let mut cmd = f.launch(entry);
    cmd.env("AMPLIHACK_AUTO_SWITCH_SUPERVISED", "1")
        .env("AMPLIHACK_CLAUDE_PROFILE", "Z")
        .env("CLAUDE_CONFIG_DIR", f.dir("Z"))
        .env(CONTEXT_ENV, path);
    cmd
}

fn assert_refused(f: &Fixture, out: &Output) {
    assert!(
        !out.status.success(),
        "unsafe launch accepted: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        f.attempts().is_empty(),
        "refusal must precede Claude creation"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("using profile 'Z'"),
        "premature success: {err}"
    );
}

#[test]
fn fallback_child_exit_37_and_ordered_arguments_survive_both_entry_points() {
    for entry in ["claude", "launch"] {
        let f = leased();
        let before = Preserved::capture(&f);
        // Include empty and shell-looking arguments: argv must not be split,
        // filtered or interpreted as shell code by any launch hand-off.
        let forwarded = [
            "--resume",
            "session with spaces",
            "",
            "$(false)",
            "two\nlines",
        ];
        let mut args = vec![entry, "--subprocess-safe", "--"];
        args.extend(forwarded);
        let out = f.run(f.cmd(&args).env("CLAUDE_EXIT", "37"));
        assert_eq!(out.status.code(), Some(37), "{entry}: {out:?}");
        assert_eq!(f.attempts(), ["Z"], "must exercise the fallback child");
        let received = fs::read(f.records.join("args.Z")).unwrap();
        let expected: Vec<u8> = forwarded
            .iter()
            .flat_map(|arg| arg.bytes().chain(std::iter::once(0)))
            .collect();
        assert!(
            received.ends_with(&expected),
            "forwarded argv changed: {received:?}"
        );
        f.assert_child_login("Z", &f.dir("Z"));
        before.assert_unchanged();
    }
}

#[test]
fn supervised_fallback_preserves_nonzero_exit_without_relaunching() {
    for entry in ["claude", "launch"] {
        let f = leased();
        let before = Preserved::capture(&f);
        let out = f.run(
            supervised(&f, entry)
                .env("CLAUDE_EXIT", "37")
                .env("RECORD_STDIN", "1"),
        );
        assert_eq!(out.status.code(), Some(37), "{entry}: {out:?}");
        assert_eq!(f.attempts(), ["Z"], "ordinary child failure is final");
        assert!(
            fs::read(f.records.join("args.Z"))
                .unwrap()
                .ends_with(b"-p\0")
        );
        // Preserve the existing #2030 prompt-on-stdin contract.
        assert_eq!(
            fs::read(f.records.join("stdin.Z")).unwrap(),
            b"supervised step"
        );
        before.assert_unchanged();
    }
}

#[test]
fn supervisor_context_transports_intent_without_credential_continuity_evidence() {
    let f = leased();
    let (_, context) = capture_context(&f, "claude");
    assert_eq!(context["profile"], "Z");
    assert_eq!(context["directory"], json!(f.dir("Z")));
    assert_eq!(context["root"], json!(f.root));
    assert_eq!(context["join_policy"], "never");
    assert!(
        context.get("evidence").is_none(),
        "credential proof is process-local and must not cross a supervisor boundary"
    );
}

#[test]
fn reentry_authenticates_the_unchanged_destination_again_before_launch() {
    for entry in ["claude", "launch"] {
        let f = leased();
        let before = Preserved::capture(&f);
        let (path, _) = capture_context(&f, entry);
        let initial_reads = f.identity_requests().iter().filter(|p| *p == "Z").count();
        assert!(initial_reads > 0, "initial selection must authenticate Z");
        let out = f.run(&mut reentry(&f, entry, &path));
        f.assert_selected(&out, "Z");
        assert!(
            f.identity_requests().iter().filter(|p| *p == "Z").count() > initial_reads,
            "launching process trusted serialised evidence instead of authenticating Z"
        );
        before.assert_unchanged();
    }
}

#[test]
fn reentry_refuses_changed_remote_identity_or_revocation_despite_unchanged_files() {
    for identity in [
        Some(("uuid-other", "org-team")),
        Some(("uuid-Z", "org-other")),
        None,
    ] {
        let f = leased();
        let before = Preserved::capture(&f);
        let (path, _) = capture_context(&f, "claude");
        f.set_remote_identity("Z", identity);
        let out = f.run(&mut reentry(&f, "claude", &path));
        before.assert_unchanged();
        assert_refused(&f, &out);
        f.assert_recovery(&out);
    }
}

#[test]
fn initial_selection_rejects_an_authenticated_account_or_organisation_mismatch() {
    for identity in [
        Some(("uuid-other", "org-team")),
        Some(("uuid-Z", "org-other")),
        None,
    ] {
        let f = leased();
        f.set_remote_identity("Z", identity);
        let before = Preserved::capture(&f);
        let out = f.run(&mut f.launch("claude"));
        before.assert_unchanged();
        assert_refused(&f, &out);
        f.assert_recovery(&out);
        assert!(f.identity_requests().iter().any(|p| p == "Z"));
    }
}

#[test]
fn excluded_fallback_credentials_are_never_requested_from_the_identity_service() {
    let f = leased();
    private(&f.root.join(".auto-switch"), "enabled=on\nexclude=Z\n");
    let before = Preserved::capture(&f);
    let out = f.run(&mut f.launch("claude"));
    f.assert_recovery(&out);
    assert!(
        f.identity_requests().is_empty(),
        "excluded and leased logins were probed"
    );
    before.assert_unchanged();
}

#[test]
fn full_pool_revalidation_refuses_a_cross_organisation_target_after_peer_capacity_returns() {
    for entry in ["claude", "launch"] {
        for supervise in [false, true] {
            let f = leased();
            f.add("W");
            f.set_organisation("Z", "org-other");
            let initial = [
                ("X", 100.0, 100.0),
                ("Y", 100.0, 100.0),
                ("W", 100.0, 100.0),
                ("Z", 10.0, 10.0),
            ];
            f.seed_usage(&initial);
            let before = Preserved::capture(&f);
            let initial_usage = fs::read(f.state().join("usage.json")).unwrap();
            f.seed_usage(&[
                ("X", 100.0, 100.0),
                ("Y", 100.0, 100.0),
                ("W", 20.0, 20.0),
                ("Z", 10.0, 10.0),
            ]);
            let updated = f.home.join("peer-capacity.json");
            private(&updated, fs::read(f.state().join("usage.json")).unwrap());
            private(&f.state().join("usage.json"), initial_usage);
            let mutation = f.home.join("peer-capacity.sh");
            private(
                &mutation,
                "cp \"$PEER_CAPACITY\" \"$AMPLIHACK_CLAUDE_PROFILE_ROOT/.auto-switch.d/usage.json\"\n",
            );
            let mut cmd = if supervise {
                supervised(&f, entry)
            } else {
                f.launch(entry)
            };
            let out = f.run(
                cmd.env("MUTATE_AT_DISCOVERY", mutation)
                    .env("PEER_CAPACITY", updated),
            );
            assert!(
                f.records.join("invalidated").exists(),
                "selection boundary not reached: {out:?}"
            );
            before.assert_unchanged();
            assert_refused(&f, &out);
            f.assert_recovery(&out);
        }
    }
}

#[test]
fn supervisor_context_refuses_links_and_oversized_objects_without_removing_them() {
    use std::os::unix::fs::symlink;
    for invalid in ["symlink", "hardlink", "oversized", "unknown-field"] {
        let f = leased();
        let before = Preserved::capture(&f);
        let (path, mut context) = capture_context(&f, "claude");
        let control = f.run(&mut reentry(&f, "claude", &path));
        f.assert_selected(&control, "Z");
        fs::remove_file(f.records.join("attempts")).unwrap();
        let held = f.home.join("runtime/owned-context.json");
        fs::rename(&path, &held).unwrap();
        match invalid {
            "symlink" => symlink(&held, &path).unwrap(),
            "hardlink" => fs::hard_link(&held, &path).unwrap(),
            "oversized" => {
                let mut body = fs::read(&held).unwrap();
                body.extend(vec![b' '; 64 * 1024]);
                private(&path, body);
            }
            "unknown-field" => {
                context["unexpected_authority"] = json!(true);
                private(&path, context.to_string());
            }
            _ => unreachable!(),
        }
        let held_bytes = fs::read(&held).unwrap();
        let out = f.run(&mut reentry(&f, "claude", &path));
        assert_refused(&f, &out);
        assert_eq!(fs::read(&held).unwrap(), held_bytes);
        assert!(
            fs::symlink_metadata(&path).is_ok(),
            "removed caller-owned context"
        );
        before.assert_unchanged();
    }
}

#[cfg(target_os = "macos")]
#[test]
fn injected_keychain_only_fallback_authenticates_and_preserves_child_exit_status() {
    for entry in ["claude", "launch"] {
        let f = leased();
        let keychain = f.keychain_login("Z");
        let credentials_before = fs::read(&keychain).unwrap();
        let before = Preserved::capture(&f);
        let out = f.run(f.launch(entry).env("CLAUDE_EXIT", "37"));
        assert_eq!(
            out.status.code(),
            Some(37),
            "Keychain storage alone must not disqualify Z: {out:?}"
        );
        assert_eq!(f.attempts(), ["Z"]);
        assert!(f.identity_requests().iter().any(|name| name == "Z"));
        assert_eq!(
            fs::read_to_string(f.records.join("env.Z")).unwrap(),
            format!("{}\nZ\n{}\n", f.dir("Z").display(), f.root.display())
        );
        assert!(
            !f.dir("Z").join(".credentials.json").exists(),
            "must not export Keychain credentials"
        );
        assert_eq!(fs::read(keychain).unwrap(), credentials_before);
        before.assert_unchanged();
    }
}

#[cfg(target_os = "macos")]
#[test]
fn injected_keychain_requires_identity_and_protected_storage_continuity() {
    for invalidation in [
        "account",
        "organisation",
        "unavailable-proof",
        "changed-at-discovery",
    ] {
        let f = leased();
        let store = f.keychain_login("Z");
        let before = Preserved::capture(&f);
        let mut cmd = f.launch("claude");
        match invalidation {
            "account" => f.set_remote_identity("Z", Some(("uuid-other", "org-team"))),
            "organisation" => f.set_remote_identity("Z", Some(("uuid-Z", "org-other"))),
            "unavailable-proof" => {
                let mut body: Value = serde_json::from_slice(&fs::read(&store).unwrap()).unwrap();
                body["entries"][f.dir("Z").to_str().unwrap()]["continuity"] = Value::Null;
                private(&store, body.to_string());
            }
            "changed-at-discovery" => {
                let mutation = f.home.join("keychain-change.sh");
                private(
                    &mutation,
                    "printf '%s' '{\"version\":1,\"entries\":{}}' > \"$CONTINUITY_STORE\"\n",
                );
                // Version discovery strips environment names containing KEY.
                // A fixture-only path alias lets this mutation run without
                // weakening the production environment scrubber.
                cmd.env("MUTATE_AT_DISCOVERY", mutation)
                    .env("CONTINUITY_STORE", &store);
            }
            _ => unreachable!(),
        }
        let out = f.run(&mut cmd);
        if invalidation == "changed-at-discovery" {
            assert!(
                f.records.join("invalidated").exists(),
                "initial Keychain selection must succeed: {out:?}"
            );
        }
        assert_refused(&f, &out);
        f.assert_recovery(&out);
        before.assert_unchanged();
    }
}

#[test]
fn refused_credentials_do_not_hide_a_usable_peer_or_authorise_organisation_crossing() {
    for same_org in [true, false] {
        let f = leased();
        f.add("W");
        f.seed_usage(&[
            ("X", 100.0, 100.0),
            ("Y", 100.0, 100.0),
            ("Z", 10.0, 10.0),
            ("W", 20.0, 20.0),
        ]);
        f.set_remote_identity("Z", None);
        if !same_org {
            f.set_organisation("W", "org-other");
        }
        let before = Preserved::capture(&f);
        let out = f.run(&mut f.launch("claude"));
        if same_org {
            f.assert_selected(&out, "W");
        } else {
            assert_refused(&f, &out);
            f.assert_recovery(&out);
        }
        before.assert_unchanged();
    }
}

#[test]
fn unavailable_private_handoff_refuses_with_recovery_before_creating_claude() {
    use std::os::unix::fs::symlink;
    for entry in ["claude", "launch"] {
        for supervise in [false, true] {
            let f = leased();
            let before = Preserved::capture(&f);
            symlink(f.home.join("runtime"), f.state().join("rerun")).unwrap();
            let mut cmd = if supervise {
                supervised(&f, entry)
            } else {
                f.launch(entry)
            };
            let out = f.run(&mut cmd);
            assert_refused(&f, &out);
            f.assert_recovery(&out);
            assert!(
                fs::symlink_metadata(f.state().join("rerun"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            before.assert_unchanged();
        }
    }
}

#[test]
fn context_intent_cannot_clear_a_fallback_descendants_restriction() {
    for origin in ["explicit", "inherited"] {
        let f = leased();
        let before = Preserved::capture(&f);
        let (path, mut context) = capture_context(&f, "claude");
        context["origin"] = json!(origin);
        context["policy_source"] = Value::Null;
        context["fallback"] = Value::Null;
        private(&path, context.to_string());
        let out = f.run(reentry(&f, "claude", &path).env("AMPLIHACK_CLAUDE_PROFILE_JOIN", "never"));
        assert_refused(&f, &out);
        let err = String::from_utf8_lossy(&out.stderr);
        for command in [
            "amplihack profile switch --none",
            "amplihack profile run <other>",
        ] {
            assert!(err.contains(command), "{err}");
        }
        before.assert_unchanged();
    }
}

#[cfg(target_os = "macos")]
#[test]
fn custom_keychain_namespace_does_not_send_credentials_to_another_identity_server() {
    let f = leased();
    f.keychain_login("Z");
    let before = Preserved::capture(&f);
    let out = f.run(f.launch("claude").env(
        "CLAUDE_CODE_CUSTOM_OAUTH_URL",
        "https://claude.fedstart.com",
    ));
    assert_refused(&f, &out);
    f.assert_recovery(&out);
    assert!(
        f.identity_requests().is_empty(),
        "custom namespace credentials reached the ordinary endpoint"
    );
    before.assert_unchanged();
}
