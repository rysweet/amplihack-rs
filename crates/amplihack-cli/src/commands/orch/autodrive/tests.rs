//! #2329 and #2575: authorization, private state and run-report contracts.
//!
//! These are internal Rust APIs, not commands agents must use. No helper in
//! this suite contacts a hosting service or performs a merge. The parsed
//! readiness record and its reducer were retired in #2575; the report reads
//! only tool-written JSON (see report_v2_tests.rs).

use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization, report::report, state::EvidenceStore,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const HEAD: &str = concat!("11111111111111111111", "11111111111111111111");
const BASE: &str = concat!("22222222222222222222", "22222222222222222222");
const STATE_DIR_REFUSAL: &str = "autodrive_state_dir must be an absolute path";

fn authorization(no_merge: &str, dry_run: &str) -> InvocationAuthorization {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), no_merge.into()),
            ("autodrive_dry_run".into(), dry_run.into()),
        ]),
    )
    .expect("valid invocation")
}

fn binding(auth: &InvocationAuthorization) -> Value {
    json!({
        "run_id": auth.run_id(), "round_id": "merge-1",
        "platform": "github", "host": "github.example.test",
        "repository": "example/project", "source_repository": "example/fork",
        "pr_number": 2329,
        "pr_url": "https://github.example.test/example/project/pull/2329",
        "head_branch": "feature", "base_branch": "main",
        "head_sha": HEAD, "base_sha": BASE, "merge_base_sha": BASE,
        "policy_provenance": "trusted-launch-bundle:fixture-v1"
    })
}

fn capture_with_state_dir(value: &str) -> anyhow::Result<InvocationAuthorization> {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), "true".into()),
            ("autodrive_state_dir".into(), value.into()),
        ]),
    )
}

#[test]
fn invocation_defaults_distinguish_outer_composer_from_direct_consumers() {
    for (recipe, allowed) in [
        ("auto-drive-to-merge", true),
        ("auto-drive-light", false),
        ("autodrive-build", false),
        ("autodrive-crusty-loop", false),
        ("autodrive-merge-loop", false),
        ("autodrive-merge-round", false),
        ("autodrive-finalize", false),
    ] {
        let auth = InvocationAuthorization::capture(recipe, &BTreeMap::new()).unwrap();
        assert_eq!(auth.allows_merge(), allowed, "entrypoint {recipe}");
    }
    let direct = InvocationAuthorization::capture(
        "autodrive-finalize",
        &BTreeMap::from([("no_merge".into(), "false".into())]),
    )
    .unwrap();
    assert!(
        direct.allows_merge(),
        "explicit authorization can enable direct finalization"
    );
}

#[test]
fn malformed_flags_fail_before_child_execution_instead_of_granting_authority() {
    for key in ["no_merge", "autodrive_dry_run"] {
        for bad in [
            "",
            "0",
            "1",
            "yes",
            "FALSE",
            " true",
            "false\n",
            "null",
            "{{no_merge}}",
        ] {
            let result = InvocationAuthorization::capture(
                "auto-drive-to-merge",
                &BTreeMap::from([(key.into(), bad.into())]),
            );
            assert!(result.is_err(), "accepted {key}={bad:?}");
        }
    }
}

#[test]
fn no_merge_and_dry_run_are_independent_restrictions() {
    for (no_merge, dry_run, merge, followups) in [
        ("false", "false", true, true),
        ("true", "false", false, true),
        ("false", "true", false, false),
        ("true", "true", false, false),
    ] {
        let auth = authorization(no_merge, dry_run);
        assert_eq!(
            (auth.allows_merge(), auth.allows_followups()),
            (merge, followups)
        );
    }
}

#[test]
fn retired_environment_cleanup_respects_explicit_command_configuration() {
    use crate::commands::orch::autodrive::trusted;
    use std::{ffi::OsStr, process::Command};
    let mut command = Command::new("unused");
    command.env(
        "BASH_ENV",
        "/old/amplifier-bundle/tools/gh-merge-guard/bash-env",
    );
    command.env("AMPLIHACK_MERGE_GUARD_USER_BASH_ENV", "/user/config.sh");
    trusted::clean_retired_environment(&mut command).unwrap();
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "BASH_ENV")
            .unwrap()
            .1,
        Some(OsStr::new("/user/config.sh"))
    );
    command.env("BASH_ENV", "/explicit/config.sh");
    trusted::clean_retired_environment(&mut command).unwrap();
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "BASH_ENV")
            .unwrap()
            .1,
        Some(OsStr::new("/explicit/config.sh"))
    );
}

#[test]
fn explicit_state_directory_must_be_absolute_and_free_of_traversal() {
    for value in [
        "",
        "state",
        "./state",
        "../x",
        "/srv/autodrive/../escape",
        "/srv/auto\ndrive",
        "/srv/auto\u{1b}[0mdrive",
    ] {
        let error = format!(
            "{:#}",
            capture_with_state_dir(value).expect_err(&format!("accepted {value:?}"))
        );
        assert!(error.contains(STATE_DIR_REFUSAL), "{value:?}: {error}");
        // Control characters must not reach the operator's terminal raw.
        assert!(
            !error.contains('\n') && !error.contains('\u{1b}'),
            "{value:?}: refusal echoed raw control characters: {error:?}"
        );
    }
    let accepted = capture_with_state_dir("/srv/autodrive/state").unwrap();
    assert!(accepted.no_merge());
}

#[test]
fn bound_state_directory_is_serialised_and_survives_private_invocation_read() {
    let dir = tempfile::tempdir().unwrap();
    let store_root = dir.path().join("invocation");
    let store = EvidenceStore::create(&store_root).unwrap();
    let mut auth = authorization("true", "false");
    assert_eq!(
        auth.state_dir(),
        None,
        "capture must not invent a directory"
    );
    let assigned = dir.path().join("autodrive-state");
    auth.bind_state_dir(assigned.clone());
    auth.bind_policy(format!("trusted-launch-bundle:sha256:{}", "a".repeat(64)));
    assert_eq!(auth.state_dir(), Some(assigned.as_path()));
    let bytes = serde_json::to_vec(&auth).unwrap();
    let raw: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(raw["state_dir"], json!(assigned));
    store.write_new("invocation.json", &bytes).unwrap();
    let read = InvocationAuthorization::read_private(&store_root.join("invocation.json")).unwrap();
    assert_eq!(read.state_dir(), Some(assigned.as_path()));
    assert_eq!(read.run_id(), auth.run_id());
}

#[test]
fn each_resume_has_fresh_authority() {
    let earlier = authorization("false", "false");
    let resumed = authorization("true", "false");
    assert_ne!(earlier.run_id(), resumed.run_id());
}

#[cfg(unix)]
#[test]
fn state_is_private_and_existing_files_are_never_overwritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let store = EvidenceStore::create(&root).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    store.write_new("record.json", b"{}").unwrap();
    assert_eq!(
        std::fs::metadata(root.join("record.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        store.write_new("record.json", b"replaced").is_err(),
        "an existing state file is never overwritten"
    );
    assert_eq!(store.read_private("record.json").unwrap(), b"{}");
}

#[cfg(unix)]
#[test]
fn state_rejects_symlinks_traversal_and_unsafe_permissions() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("autodrive-state");
    let store = EvidenceStore::create(&root).unwrap();
    let outside = dir.path().join("secret.json");
    std::fs::write(&outside, b"{}").unwrap();
    symlink(&outside, root.join("linked.json")).unwrap();
    for path in [
        "../secret.json",
        outside.to_str().unwrap(),
        "linked.json",
        ".",
    ] {
        assert!(
            store.read_private(path).is_err(),
            "unsafe state path {path}"
        );
    }
    let unsafe_file = root.join("unsafe.json");
    std::fs::write(&unsafe_file, b"{}").unwrap();
    std::fs::set_permissions(&unsafe_file, std::fs::Permissions::from_mode(0o666)).unwrap();
    // #2546: a world-writable file is refused and left unchanged.
    assert!(
        store.read_private("unsafe.json").is_err(),
        "a world-writable state file is refused"
    );
    assert_eq!(mode(&unsafe_file), 0o666);
    // #2546: our own file with group bits only is repaired, not refused.
    std::fs::set_permissions(&unsafe_file, std::fs::Permissions::from_mode(0o664)).unwrap();
    store.read_private("unsafe.json").unwrap();
    assert_eq!(mode(&unsafe_file), 0o600);
    std::fs::hard_link(&unsafe_file, dir.path().join("second-name.json")).unwrap();
    assert!(
        store.read_private("unsafe.json").is_err(),
        "a hard-linked state file is still refused"
    );
    let alias = dir.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert!(EvidenceStore::create(&alias).is_err());
}

#[cfg(unix)]
#[test]
fn state_rejects_special_files_and_untrusted_existing_directory_modes() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("autodrive-state");
    let store = EvidenceStore::create(&root).unwrap();
    // A one-letter name keeps the socket path within the sun_path limit
    // under a long TMPDIR.
    let _socket = UnixListener::bind(root.join("s")).unwrap();
    assert!(store.read_private("s").is_err());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    // #2546: a world-writable state folder is refused and left unchanged.
    assert!(EvidenceStore::create(&root).is_err());
    assert_eq!(mode(&root), 0o777);
    // #2546 B1: the folder name allows no repair. This `autodrive-state`
    // folder is not directly under the auto-drive state root, so an existing
    // 0775 folder is refused and left as it was. Repair under the root is
    // covered by issue_2546_state_folder_0775_is_repaired_and_logged.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();
    let error = match EvidenceStore::create(&root) {
        Ok(_) => panic!("a 0775 autodrive-state folder outside the state root was accepted"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}")
            .contains("outside the auto-drive state tree has group or other access"),
        "{error:#}"
    );
    assert_eq!(mode(&root), 0o775);
}

#[cfg(unix)]
fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn already_merged_preparation_requires_fresh_bound_platform_confirmation() {
    let auth = authorization("true", "false");
    let mut context = json!({"build_result":{"merged_before_run":true,"pr_state":"MERGED","entry_evidence":"Observed merged=true on exact PR at invocation entry","binding":binding(&auth)}});
    let result = report(&auth, context.as_object().unwrap(), Some("2329"));
    assert_eq!(result["schema_version"], 2, "{result}");
    assert_eq!(result["merge"]["result"], "already-merged", "{result}");
    assert_eq!(result["merge"]["pr"], 2329, "{result}");
    assert_eq!(result["stop_reason"], "completed", "{result}");
    context["build_result"]["entry_evidence"] = json!("");
    let result = report(&auth, context.as_object().unwrap(), Some("2329"));
    assert_ne!(result["stop_reason"], "completed", "{result}");
    assert!(result["merge"].is_null(), "{result}");
    // Another invocation's preparation is not this run's confirmation.
    let other = authorization("true", "false");
    context["build_result"]["entry_evidence"] = json!("Observed merged=true");
    context["build_result"]["binding"] = binding(&other);
    let result = report(&auth, context.as_object().unwrap(), Some("2329"));
    assert_ne!(result["stop_reason"], "completed", "{result}");
}

#[test]
fn report_ignores_merge_output_when_the_state_directory_is_not_the_assigned_one() {
    let dir = tempfile::tempdir().unwrap();
    let assigned = dir.path().join("assigned");
    let mut auth = authorization("false", "false");
    auth.bind_state_dir(assigned.clone());
    let merged = json!({"result":"merged","pr":2329,"handoff":"false"}).to_string();
    let done = json!({"loop":"merge-ready","loop_result":"DONE"}).to_string();
    for (label, reported) in [
        ("matching", Some(json!(assigned))),
        ("empty", Some(json!(""))),
        ("absent", None),
    ] {
        let mut context = json!({"merge_loop_result": done, "finalize_merge": merged});
        if let Some(value) = reported {
            context["autodrive_state_dir"] = value;
        }
        let result = report(&auth, context.as_object().unwrap(), Some("2329"));
        assert_eq!(result["stop_reason"], "completed", "{label}: {result}");
    }
    for (label, reported) in [
        ("relative", json!("assigned")),
        (
            "parent component",
            json!(assigned.join("..").join("assigned")),
        ),
        ("sibling", json!(dir.path().join("other"))),
        ("object", json!({"path": assigned})),
        ("number", json!(7)),
    ] {
        let context = json!({"merge_loop_result": done, "finalize_merge": merged, "autodrive_state_dir": reported});
        let result = report(&auth, context.as_object().unwrap(), Some("2329"));
        assert_ne!(result["stop_reason"], "completed", "{label}: {result}");
        assert!(result["merge"].is_null(), "{label}: {result}");
        assert!(
            result["note"]
                .as_str()
                .is_some_and(|n| n.contains("runner-assigned state directory")),
            "{label}: {result}"
        );
    }
    let context = json!({"merge_loop_result": done, "finalize_merge": merged, "build_result": {"state_dir": "/elsewhere"}});
    let result = report(&auth, context.as_object().unwrap(), Some("2329"));
    assert_ne!(result["stop_reason"], "completed", "{result}");
}
