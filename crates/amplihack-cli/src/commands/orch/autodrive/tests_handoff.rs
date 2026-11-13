//! Behavioural TDD contracts for #2555. Registered by autodrive/tests.rs in
//! the library test harness, where private runner authority remains private.
//!
//! Production boundaries exercised by these tests:
//! - policy::resolve(auth, store, binding, hosting) persists a ResolvedPolicy;
//! - handoff::prepare(auth, store, policy, hosting) confirms an open, ready PR;
//! - handoff::complete(auth, store, policy, repo, accepted, hosting) replays
//!   production readiness and returns the existing helper-style snapshot envelope.
//!
//! The hosting fixture supplies facts and external effects, never a verdict.
//! These contracts run real production actions with controlled hosting facts.

#[path = "tests_hosting.rs"]
pub(super) mod hosting;

use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization,
    completion, handoff,
    hosting::{BaseConfig, CommentPage},
    outcome::reduce_outcome,
    policy::{self, DriveMode, ResolvedPolicy},
    records::*,
    report, review,
    state::EvidenceStore,
    validation::validate_candidate,
};
use hosting::{Call, Fault, Hosting, STEWARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Re-enter just this test in a fresh process. No process-global environment
/// changes occur beside other library tests, and even a faulty implementation
/// sees isolated homes, configuration and profile roots.
fn in_isolated_home(name: &str) -> bool {
    const MARKER: &str = "AMPLIHACK_ISSUE_2555_TEST_PARENT";
    if let Some(parent) = std::env::var_os(MARKER) {
        assert_eq!(
            parent,
            std::os::unix::process::parent_id().to_string().as_str()
        );
        return false;
    }
    let temp = tempfile::Builder::new()
        .prefix("issue-2555-home-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let home = temp.path().join("home");
    for relative in [
        ".claude",
        ".amplihack",
        ".codex",
        ".config",
        "profiles",
        "tmp",
    ] {
        fs::create_dir_all(home.join(relative)).unwrap();
    }
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink("/usr/bin/git", bin.join("git")).unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let output = Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("PATH", &bin)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("AMPLIHACK_HOME", home.join(".amplihack"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", home.join("profiles"))
        .env("TMPDIR", home.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .env("AMPLIHACK_TEST_HOME_GUARD", "1")
        .env(MARKER, std::process::id().to_string())
        .args([
            "--exact",
            &format!("{module}::{name}"),
            "--test-threads=1",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated {name}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "isolated test was not selected: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    true
}

struct Drive {
    _temp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    directory: PathBuf,
    auth: InvocationAuthorization,
    binding: Binding,
    candidate: CandidateRecord,
    host: Hosting,
    store: EvidenceStore,
}

fn replace(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::String(s) if s == old => *s = new.into(),
        Value::Array(a) => a.iter_mut().for_each(|v| replace(v, old, new)),
        Value::Object(o) => o.values_mut().for_each(|v| replace(v, old, new)),
        _ => (),
    }
}

impl Drive {
    fn new(mode: Option<&str>) -> Self {
        // Both supported hosts provide /var/tmp. Do not inherit a recipe's
        // TMPDIR, which may be inside a real framework or profile directory.
        let temp = tempfile::Builder::new()
            .prefix("issue-2555-")
            .tempdir_in("/var/tmp")
            .unwrap();
        assert!(
            std::env::var_os("AMPLIHACK_ISSUE_2555_TEST_PARENT").is_some(),
            "drive fixture requires an isolated child"
        );
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        for path in [".claude", ".amplihack", ".codex", ".config", "profiles"] {
            fs::create_dir_all(home.join(path)).unwrap();
        }
        let root = temp.path().canonicalize().unwrap();
        let main = root.join("main");
        fs::create_dir(&main).unwrap();
        let git = |repo: &Path, args: &[&str]| {
            let output = amplihack_git::command()
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(["--no-replace-objects", "-C"])
                .arg(repo)
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.test",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&main, &["init", "-b", "main"]);
        git(&main, &["commit", "--allow-empty", "-m", "base"]);
        let base = git(&main, &["rev-parse", "HEAD"]);
        git(&main, &["update-ref", "refs/remotes/origin/main", &base]);
        let repo = root.join("feature");
        git(
            &main,
            &["worktree", "add", "-b", "feature", repo.to_str().unwrap()],
        );
        fs::write(repo.join("code.rs"), "fn implemented() {}\n").unwrap();
        git(&repo, &["add", "code.rs"]);
        git(&repo, &["commit", "-m", "implementation"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let inputs = mode
            .map(|m| BTreeMap::from([("merge".into(), m.into())]))
            .unwrap_or_default();
        let mut auth = InvocationAuthorization::capture("auto-drive-to-merge", &inputs).unwrap();
        auth.bind_policy(format!("trusted-launch-bundle:sha256:{}", "a".repeat(64)));
        let directory = root.join("state");
        let store = EvidenceStore::create(&directory).unwrap();
        auth.bind_state_dir(directory.clone());
        let mut raw = super::candidate(&auth);
        replace(&mut raw, super::HEAD, &head);
        replace(&mut raw, super::BASE, &base);
        replace(&mut raw, "merge-1", "round-1");
        replace(
            &mut raw,
            "trusted-launch-bundle:fixture-v1",
            &format!("trusted-launch-bundle:sha256:{}", "a".repeat(64)),
        );
        let candidate: CandidateRecord = serde_json::from_value(raw).unwrap();
        let binding = candidate.binding.clone();
        let host = Hosting::new(binding.clone());
        Self {
            _temp: temp,
            home,
            repo,
            directory,
            auth,
            binding,
            candidate,
            host,
            store,
        }
    }

    fn resolve(&mut self) -> anyhow::Result<ResolvedPolicy> {
        policy::resolve(&self.auth, &self.store, &self.binding, &mut self.host)
    }

    fn prepare(&mut self, policy: &ResolvedPolicy) -> anyhow::Result<()> {
        handoff::prepare(&self.auth, &self.store, policy, &mut self.host).map(|_| ())
    }

    fn seal_ready(&mut self) -> PathBuf {
        self.store
            .write_new("qa.log", b"controlled Rust verification passed\n")
            .unwrap();
        self.candidate.criteria[0].evidence[0].reference = Some(EvidenceReference {
            file: "qa.log".into(),
            sha256: format!(
                "{:x}",
                Sha256::digest(b"controlled Rust verification passed\n")
            ),
        });
        review::verify(&self.repo, &self.candidate).unwrap();
        validate_candidate(&self.auth, &self.binding, &self.candidate).unwrap();
        self.store.snapshot_references(&self.candidate).unwrap();
        let accepted = self
            .store
            .snapshot(&serde_json::to_vec(&self.candidate).unwrap())
            .unwrap();
        self.store
            .write_new(
                "merge-ready-round-1.json",
                &serde_json::to_vec(&json!({
                    "merge_ready_verdict":"MERGE_READY", "accepted_evidence":accepted
                }))
                .unwrap(),
            )
            .unwrap();
        self.store
            .write_new(
                "merge-ready-result.json",
                &serde_json::to_vec(&json!({
                    "loop_result":"DONE", "round_label":"round-1", "round_verdict":"MERGE_READY"
                }))
                .unwrap(),
            )
            .unwrap();
        completion::record(&self.auth, &self.directory, "round-1").unwrap();
        accepted
    }

    fn complete(&mut self, policy: &ResolvedPolicy, accepted: &Path) -> anyhow::Result<Value> {
        let result = handoff::complete(
            &self.auth,
            &self.store,
            policy,
            &self.repo,
            accepted,
            &mut self.host,
        )?;
        Ok(serde_json::to_value(result)?)
    }

    fn ready(&mut self) -> (ResolvedPolicy, PathBuf) {
        let policy = self.resolve().unwrap();
        self.prepare(&policy).unwrap();
        let accepted = self.seal_ready();
        (policy, accepted)
    }

    fn report(&self, result: Value) -> Value {
        report::report(
            &self.auth,
            json!({
                "autodrive_state_dir":self.directory,
                "repo_path":self.repo,
                "autodrive_finalization":result,
            })
            .as_object()
            .unwrap(),
            Some("2329"),
            &self.repo,
        )
    }

    fn assert_completed(&self, result: &Value) {
        assert_eq!(result["outcome"]["action"], "handed_off", "{result}");
        assert_eq!(result["outcome"]["stop_reason"], "completed", "{result}");
        assert_eq!(result["outcome"]["readiness"], "merge_ready", "{result}");
        assert_eq!(result["outcome"]["platform_state"], "open", "{result}");
        assert_eq!(
            result["outcome"]["binding"],
            serde_json::to_value(&self.binding).unwrap()
        );
        assert_eq!(self.host.pr.state, PlatformState::Open);
        assert!(!self.host.pr.draft);
        assert!(!self.host.pr.auto_merge_enabled);
        assert_eq!(
            self.host
                .comments
                .values()
                .filter(|c| c.author == hosting::LOGIN && c.body == STEWARD)
                .count(),
            1
        );
        for key in ["completion_snapshot", "outcome_snapshot"] {
            let path = Path::new(
                result[key]
                    .as_str()
                    .unwrap_or_else(|| panic!("missing {key}: {result}")),
            );
            assert_eq!(path.parent(), Some(self.directory.as_path()));
            let bytes = self
                .store
                .read_private(path.file_name().unwrap().to_str().unwrap())
                .unwrap();
            let persisted: Value = serde_json::from_slice(&bytes).unwrap();
            if key == "outcome_snapshot" {
                assert_eq!(persisted, result["outcome"]);
            } else {
                assert_eq!(
                    persisted["binding"],
                    serde_json::to_value(&self.binding).unwrap()
                );
                assert_eq!(persisted["merge_requests"], 0);
                assert_eq!(persisted["scheduling_requests"], 0);
                let id = persisted["comment"]["id"]
                    .as_u64()
                    .expect("durable comment identity");
                assert_eq!(self.host.comments[&id].body, STEWARD);
                assert!(
                    persisted.get("readiness").is_some(),
                    "missing readiness evidence: {persisted}"
                );
            }
        }
        let report = self.report(result.clone());
        assert_eq!(report["action"], "handed_off", "{report}");
        assert_eq!(report["stop_reason"], "completed", "{report}");
    }
}

#[test]
fn mode_parser_is_trimmed_strict_and_independent_of_merge_method() {
    if in_isolated_home("mode_parser_is_trimmed_strict_and_independent_of_merge_method") {
        return;
    }
    for (text, expected) in [
        ("merge", DriveMode::Merge),
        (" handoff\n", DriveMode::Handoff),
    ] {
        assert_eq!(text.parse::<DriveMode>().unwrap(), expected);
    }
    for text in [
        "",
        " ",
        "MERGE",
        "Handoff",
        "squash",
        "handoff now",
        "true",
        "{{merge}}",
    ] {
        assert!(text.parse::<DriveMode>().is_err(), "accepted mode {text:?}");
    }
    for text in ["MERGE", "squash", "{{merge}}"] {
        assert!(
            InvocationAuthorization::capture(
                "auto-drive-to-merge",
                &BTreeMap::from([("merge".into(), text.into())])
            )
            .is_err(),
            "invalid explicit mode {text:?} reached a child"
        );
    }
}

#[test]
fn build_no_merge_cannot_suppress_outer_drive_and_handoff_cannot_authorise_merge() {
    if in_isolated_home(
        "build_no_merge_cannot_suppress_outer_drive_and_handoff_cannot_authorise_merge",
    ) {
        return;
    }
    for no_merge in ["false", "true"] {
        let auth = InvocationAuthorization::capture(
            "auto-drive-to-merge",
            &BTreeMap::from([
                ("no_merge".into(), no_merge.into()),
                ("merge".into(), "merge".into()),
            ]),
        )
        .unwrap();
        assert!(
            auth.allows_merge(),
            "build-only no_merge={no_merge} changed drive authority"
        );
    }
    let auth = InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([("merge".into(), "handoff".into())]),
    )
    .unwrap();
    assert!(!auth.allows_merge(), "handoff granted merge authority");
    assert!(auth.allows_followups());
}

#[test]
fn policy_uses_only_pinned_target_base_despite_fork_worktree_and_profile_configuration() {
    if in_isolated_home(
        "policy_uses_only_pinned_target_base_despite_fork_worktree_and_profile_configuration",
    ) {
        return;
    }
    let mut d = Drive::new(None);
    d.host.config = BaseConfig::File {
        bytes: b"merge = 'handoff'\nmerge_method = 'squash'\n".to_vec(),
        mode: "100644".into(),
    };
    for root in [&d.repo, &d.home] {
        fs::create_dir_all(root.join(".amplihack")).unwrap();
        fs::write(root.join(".amplihack/auto-drive.toml"), "merge = 'merge'\n").unwrap();
    }
    let p = d.resolve().unwrap();
    assert_eq!(p.mode, DriveMode::Handoff);
    assert!(d.host.calls.contains(&Call::BaseTip {
        repository: "example/project".into(),
        branch: "main".into()
    }));
    let reads: Vec<_> = d
        .host
        .calls
        .iter()
        .filter(|c| matches!(c, Call::Config { .. }))
        .collect();
    assert_eq!(
        reads,
        vec![&Call::Config {
            repository: "example/project".into(),
            commit: d.binding.base_sha.clone(),
            path: ".amplihack/auto-drive.toml".into(),
        }]
    );
    assert_ne!(d.binding.repository, d.binding.source_repository);
    let stored = serde_json::to_value(p).unwrap();
    assert_eq!(stored["binding"], serde_json::to_value(&d.binding).unwrap());
    assert_eq!(stored["source"], "base_configuration");
}

#[test]
fn absent_file_or_key_defaults_to_merge_and_empty_explicit_input_preserves_repository_mode() {
    if in_isolated_home(
        "absent_file_or_key_defaults_to_merge_and_empty_explicit_input_preserves_repository_mode",
    ) {
        return;
    }
    for config in [
        BaseConfig::Missing,
        BaseConfig::File {
            bytes: b"merge_method = 'squash'\n".to_vec(),
            mode: "100644".into(),
        },
    ] {
        let mut d = Drive::new(None);
        d.host.config = config;
        assert_eq!(d.resolve().unwrap().mode, DriveMode::Merge);
    }
    for explicit in [None, Some(""), Some(" \n\t")] {
        let mut d = Drive::new(explicit);
        d.host.config = BaseConfig::File {
            bytes: b"merge = ' handoff '\n".to_vec(),
            mode: "100644".into(),
        };
        assert_eq!(
            d.resolve().unwrap().mode,
            DriveMode::Handoff,
            "explicit={explicit:?}"
        );
    }
}

#[test]
fn valid_explicit_mode_skips_unreadable_or_malformed_repository_configuration() {
    if in_isolated_home(
        "valid_explicit_mode_skips_unreadable_or_malformed_repository_configuration",
    ) {
        return;
    }
    for (explicit, expected) in [
        ("merge", DriveMode::Merge),
        (" handoff ", DriveMode::Handoff),
    ] {
        let mut d = Drive::new(Some(explicit));
        d.host.config = BaseConfig::File {
            bytes: b"[invalid TOML".to_vec(),
            mode: "100644".into(),
        };
        d.host.fault = Some(Fault::Config);
        assert_eq!(d.resolve().unwrap().mode, expected);
        assert!(
            !d.host
                .calls
                .iter()
                .any(|c| matches!(c, Call::Config { .. }))
        );
        assert!(
            d.host.calls.contains(&Call::Observe),
            "explicit selection still binds the PR"
        );
    }
}

#[test]
fn applicable_invalid_configuration_and_unestablished_base_authority_fail_closed() {
    if in_isolated_home(
        "applicable_invalid_configuration_and_unestablished_base_authority_fail_closed",
    ) {
        return;
    }
    for bytes in [
        b"[broken".as_slice(),
        b"merge = 7",
        b"merge = true",
        b"merge = []",
        b"merge = ''",
        b"merge = 'HANDOFF'",
        b"merge = 'squash'",
        b"merge='merge'\nmerge='handoff'",
    ] {
        let mut d = Drive::new(None);
        d.host.config = BaseConfig::File {
            bytes: bytes.to_vec(),
            mode: "100644".into(),
        };
        assert!(d.resolve().is_err(), "accepted applicable config {bytes:?}");
        assert_eq!(d.host.creations(), 0);
    }
    let mut secret = Drive::new(None);
    secret.host.config = BaseConfig::File {
        bytes: b"token = PRIVATE_CONFIG_VALUE".to_vec(),
        mode: "100644".into(),
    };
    let diagnostic = format!("{:#}", secret.resolve().unwrap_err());
    assert!(diagnostic.contains("drive-policy-toml"));
    assert!(!diagnostic.contains("PRIVATE_CONFIG_VALUE"));
    for fault in [Fault::Observe, Fault::BaseTip, Fault::Config] {
        let mut d = Drive::new(None);
        d.host.fault = Some(fault);
        let error = d
            .resolve()
            .expect_err("unreadable authority became default merge");
        assert!(
            format!("{error:#}").contains("fixture-"),
            "lost diagnostic: {error:#}"
        );
    }
    for mode in ["120000", "040000", "160000"] {
        let mut d = Drive::new(None);
        d.host.config = BaseConfig::File {
            bytes: b"merge='handoff'".to_vec(),
            mode: mode.into(),
        };
        assert!(
            d.resolve().is_err(),
            "accepted unsuitable tree entry {mode}"
        );
    }
    for bytes in [
        vec![0xff],
        vec![b' '; crate::commands::orch::autodrive::validation::MAX_RECORD_BYTES + 1],
    ] {
        let mut d = Drive::new(None);
        d.host.config = BaseConfig::File {
            bytes,
            mode: "100644".into(),
        };
        assert!(
            d.resolve().is_err(),
            "accepted non-UTF-8 or unbounded configuration"
        );
    }
}

#[test]
fn preparation_converts_draft_and_confirms_status_without_claiming_readiness() {
    if in_isolated_home("preparation_converts_draft_and_confirms_status_without_claiming_readiness")
    {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    d.host.pr.draft = true;
    let p = d.resolve().unwrap();
    d.host.clear_calls();
    d.prepare(&p).unwrap();
    let ready = d
        .host
        .calls
        .iter()
        .position(|c| *c == Call::Ready)
        .expect("draft converted");
    assert!(
        d.host.calls[ready + 1..].contains(&Call::Observe),
        "ready conversion was not read back"
    );
    assert!(!d.host.pr.draft);
    assert_eq!(d.host.creations(), 0, "preparation claimed readiness");
    assert!(completion::verify(&d.auth, &d.directory, &d.candidate).is_err());
}

#[test]
fn handoff_completes_production_readiness_with_one_comment_and_durable_open_outcome() {
    if in_isolated_home(
        "handoff_completes_production_readiness_with_one_comment_and_durable_open_outcome",
    ) {
        return;
    }
    for draft in [false, true] {
        let mut d = Drive::new(Some("handoff"));
        d.host.pr.draft = draft;
        let (p, accepted) = d.ready();
        let result = d.complete(&p, &accepted).unwrap();
        d.assert_completed(&result);
        assert_eq!(d.host.creations(), 1);
        assert!(
            d.host
                .calls
                .iter()
                .any(|c| *c == Call::Create(STEWARD.into()))
        );
        // Reopening the actual store, rather than trusting helper stdout,
        // must retain a separately reportable successful handoff.
        let reopened = EvidenceStore::create(&d.directory).unwrap();
        let name = Path::new(result["outcome_snapshot"].as_str().unwrap())
            .file_name()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(
                &reopened.read_private(name.to_str().unwrap()).unwrap()
            )
            .unwrap(),
            result["outcome"]
        );
    }
}

#[test]
fn failed_readiness_or_missing_loop_receipt_never_posts_a_ready_comment() {
    if in_isolated_home("failed_readiness_or_missing_loop_receipt_never_posts_a_ready_comment") {
        return;
    }
    for failure in [
        "failed_criterion",
        "missing_receipt",
        "changed_artifact",
        "failed_evaluator",
    ] {
        let mut d = Drive::new(Some("handoff"));
        let (p, mut accepted) = d.ready();
        match failure {
            "failed_criterion" => {
                d.candidate.readiness = Readiness::Incomplete;
                d.candidate.blockers.push("required check failed".into());
                d.candidate.criteria[3].status = CriterionStatus::Failed;
                d.candidate.criteria[3]
                    .blockers
                    .push("required check failed".into());
                accepted = d
                    .store
                    .snapshot(&serde_json::to_vec(&d.candidate).unwrap())
                    .unwrap();
            }
            "missing_receipt" => fs::remove_file(
                d.directory
                    .join(format!("loop-{}-merge-ready.json", d.auth.run_id())),
            )
            .unwrap(),
            "changed_artifact" => fs::write(d.directory.join("qa.log"), b"altered").unwrap(),
            "failed_evaluator" => {
                fs::remove_file(
                    d.directory
                        .join(format!("loop-{}-merge-ready.json", d.auth.run_id())),
                )
                .unwrap();
                fs::remove_file(d.directory.join("merge-ready-result.json")).unwrap();
                d.store.write_new("merge-ready-result.json", br#"{"loop_result":"STUCK","round_label":"round-1","round_verdict":"MERGE_READY"}"#).unwrap();
                assert!(completion::record(&d.auth, &d.directory, "round-1").is_err());
            }
            _ => unreachable!(),
        }
        assert!(
            d.complete(&p, &accepted).is_err(),
            "completed after {failure}"
        );
        assert_eq!(d.host.creations(), 0, "claimed readiness after {failure}");
    }
}

#[test]
fn stale_or_non_open_platform_state_invalidates_preparation_and_completion() {
    if in_isolated_home("stale_or_non_open_platform_state_invalidates_preparation_and_completion") {
        return;
    }
    for phase in ["prepare", "complete"] {
        for change in [
            "closed",
            "merged",
            "draft",
            "auto_merge",
            "head",
            "base",
            "repository",
            "pr",
        ] {
            let mut d = Drive::new(Some("handoff"));
            let p = d.resolve().unwrap();
            let accepted = if phase == "complete" {
                d.prepare(&p).unwrap();
                Some(d.seal_ready())
            } else {
                None
            };
            match change {
                "closed" => d.host.pr.state = PlatformState::Closed,
                "merged" => d.host.pr.state = PlatformState::Merged,
                "draft" => {
                    d.host.pr.draft = true;
                    d.host.fault = Some(Fault::ReadyNotApplied);
                }
                "auto_merge" => d.host.pr.auto_merge_enabled = true,
                "head" => d.host.pr.binding.head_sha = super::OTHER.into(),
                "base" => {
                    d.host.pr.binding.base_sha = super::OTHER.into();
                    d.host.base_tip = super::OTHER.into();
                }
                "repository" => d.host.pr.binding.repository = "attacker/project".into(),
                "pr" => d.host.pr.binding.pr_number += 1,
                _ => unreachable!(),
            }
            let result = match accepted {
                Some(path) => d.complete(&p, &path).map(|_| ()),
                None => d.prepare(&p),
            };
            assert!(result.is_err(), "{phase} accepted changed {change}");
            assert_eq!(d.host.creations(), 0, "{phase}/{change} claimed readiness");
        }
    }
}

#[test]
fn completion_failures_preserve_diagnostics_instead_of_reporting_success() {
    if in_isolated_home("completion_failures_preserve_diagnostics_instead_of_reporting_success") {
        return;
    }
    for fault in [
        Fault::Ready,
        Fault::ReadyNotApplied,
        Fault::Observe,
        Fault::Login,
        Fault::Comments,
        Fault::Create,
        Fault::Comment,
    ] {
        let mut d = Drive::new(Some("handoff"));
        d.host.pr.draft = true;
        let p = d.resolve().unwrap();
        if matches!(
            fault,
            Fault::Ready | Fault::ReadyNotApplied | Fault::Observe
        ) {
            d.host.fault = Some(fault);
            assert!(d.prepare(&p).is_err(), "preparation swallowed {fault:?}");
        } else {
            d.prepare(&p).unwrap();
            let accepted = d.seal_ready();
            d.host.fault = Some(fault);
            let error = d
                .complete(&p, &accepted)
                .expect_err("completion swallowed hosting failure");
            assert!(
                format!("{error:#}").contains("fixture-"),
                "lost {fault:?}: {error:#}"
            );
        }
        let report = d.report(json!({"outcome":{"action":"handed_off","stop_reason":"completed"}}));
        assert_ne!(
            report["stop_reason"], "completed",
            "forged success after {fault:?}: {report}"
        );
    }
}

#[test]
fn reconciliation_enumerates_all_pages_and_reuses_only_authenticated_author_match() {
    if in_isolated_home(
        "reconciliation_enumerates_all_pages_and_reuses_only_authenticated_author_match",
    ) {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    let mut wrong_author = d.host.matching_comment(1);
    wrong_author.author = "untrusted-contributor".into();
    let matching = d.host.matching_comment(2);
    d.host.seed_comment(wrong_author.clone());
    d.host.seed_comment(matching.clone());
    d.host.pages.insert(
        None,
        CommentPage {
            comments: vec![wrong_author],
            next: Some("page-2".into()),
        },
    );
    d.host.pages.insert(
        Some("page-2".into()),
        CommentPage {
            comments: vec![matching],
            next: None,
        },
    );
    let result = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&result);
    assert_eq!(d.host.creations(), 0);
    assert!(
        d.host
            .calls
            .contains(&Call::Comments(Some("page-2".into())))
    );
    assert!(
        d.host.calls.contains(&Call::Comment(2)),
        "reused comment was not confirmed"
    );
}

#[test]
fn ambiguous_incomplete_or_cyclic_comment_enumeration_cannot_create_or_complete() {
    if in_isolated_home(
        "ambiguous_incomplete_or_cyclic_comment_enumeration_cannot_create_or_complete",
    ) {
        return;
    }
    for problem in [
        "duplicates",
        "unreadable",
        "later_page",
        "cycle",
        "wrong_pr",
    ] {
        let mut d = Drive::new(Some("handoff"));
        let (p, accepted) = d.ready();
        match problem {
            "duplicates" => {
                d.host.seed_comment(d.host.matching_comment(1));
                d.host.seed_comment(d.host.matching_comment(2));
            }
            "unreadable" => d.host.fault = Some(Fault::Comments),
            "later_page" => {
                d.host.pages.insert(
                    None,
                    CommentPage {
                        comments: vec![],
                        next: Some("unreadable".into()),
                    },
                );
                d.host.unreadable_cursor = Some("unreadable".into());
            }
            "cycle" => {
                d.host.pages.insert(
                    None,
                    CommentPage {
                        comments: vec![],
                        next: Some("repeat".into()),
                    },
                );
                d.host.pages.insert(
                    Some("repeat".into()),
                    CommentPage {
                        comments: vec![],
                        next: Some("repeat".into()),
                    },
                );
            }
            "wrong_pr" => {
                let mut comment = d.host.matching_comment(1);
                comment.pr_url.push('0');
                d.host.seed_comment(comment);
            }
            _ => unreachable!(),
        }
        assert!(d.complete(&p, &accepted).is_err(), "accepted {problem}");
        assert_eq!(
            d.host.creations(),
            0,
            "created before safe enumeration: {problem}"
        );
    }
}

#[test]
fn lost_creation_response_is_reconciled_on_resume_without_duplicate_comment() {
    if in_isolated_home("lost_creation_response_is_reconciled_on_resume_without_duplicate_comment")
    {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    d.host.fault = Some(Fault::LostCreateResponse);
    let error = d
        .complete(&p, &accepted)
        .expect_err("uncertain creation became successful handoff");
    assert!(format!("{error:#}").contains("LostCreateResponse"));
    assert_eq!(d.host.creations(), 1);
    assert_eq!(d.host.comments.len(), 1);
    d.host.fault = None;
    let result = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&result);
    assert_eq!(
        d.host.creations(),
        1,
        "repeated uncertain write without reconciliation"
    );
}

#[test]
fn old_comment_without_current_readiness_never_completes_resumed_drive() {
    if in_isolated_home("old_comment_without_current_readiness_never_completes_resumed_drive") {
        return;
    }
    let mut old = Drive::new(Some("handoff"));
    let (p, accepted) = old.ready();
    let result = old.complete(&p, &accepted).unwrap();
    old.assert_completed(&result);
    let mut resumed = Drive::new(Some("handoff"));
    let p = resumed.resolve().unwrap();
    resumed.prepare(&p).unwrap();
    resumed
        .host
        .seed_comment(resumed.host.matching_comment(100));
    let without_receipt = resumed
        .store
        .snapshot(&serde_json::to_vec(&resumed.candidate).unwrap())
        .unwrap();
    assert!(resumed.complete(&p, &without_receipt).is_err());
    assert!(
        resumed.complete(&p, &accepted).is_err(),
        "accepted another invocation's evidence"
    );
    assert_eq!(resumed.host.creations(), 0);
}

#[test]
fn disappearance_of_private_state_after_comment_confirmation_remains_a_failure() {
    if in_isolated_home(
        "disappearance_of_private_state_after_comment_confirmation_remains_a_failure",
    ) {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    d.host.remove_state_on_comment_read = Some(d.directory.clone());
    assert!(
        d.complete(&p, &accepted).is_err(),
        "publication failure became success"
    );
    assert_eq!(
        d.host.creations(),
        1,
        "test must reach the external write before failing"
    );
    assert_eq!(
        d.host.comments.len(),
        1,
        "partial external action must remain visible"
    );
    assert!(
        !d.directory.exists(),
        "publication failure was not exercised"
    );
}

#[test]
fn non_blocking_handoff_lock_refuses_overlapping_external_actions() {
    if in_isolated_home("non_blocking_handoff_lock_refuses_overlapping_external_actions") {
        return;
    }
    use fs2::FileExt;
    use std::os::unix::fs::OpenOptionsExt;
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(d.directory.join("handoff.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    d.host.clear_calls();
    assert!(
        d.complete(&p, &accepted).is_err(),
        "overlapping completion acquired held lock"
    );
    assert_eq!(d.host.creations(), 0);
    lock.unlock().unwrap();
    let result = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&result);
}

#[test]
fn final_readback_change_cannot_publish_success_after_partial_comment_write() {
    if in_isolated_home("final_readback_change_cannot_publish_success_after_partial_comment_write")
    {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    let mut changed = d.host.pr.clone();
    changed.binding.head_sha = super::OTHER.into();
    d.host.change_pr_after_create = Some(changed);
    assert!(
        d.complete(&p, &accepted).is_err(),
        "head change during completion was accepted"
    );
    assert_eq!(
        d.host.creations(),
        1,
        "must exercise final readback after external write"
    );
    assert_eq!(d.host.comments.len(), 1);
}

#[test]
fn merge_modes_keep_expected_head_protection_and_handoff_denies_forged_merge() {
    if in_isolated_home("merge_modes_keep_expected_head_protection_and_handoff_denies_forged_merge")
    {
        return;
    }
    for (mode, repository_mode) in [
        (None, None),
        (Some("merge"), None),
        (Some("handoff"), None),
        (None, Some("handoff")),
    ] {
        let mut d = Drive::new(mode);
        if repository_mode.is_some() {
            d.host.config = BaseConfig::File {
                bytes: b"merge='handoff'".to_vec(),
                mode: "100644".into(),
            };
        }
        let p = d.resolve().unwrap();
        let accepted = validate_candidate(&d.auth, &d.binding, &d.candidate).unwrap();
        let mut raw = super::finalization(&d.auth);
        raw["binding"] = serde_json::to_value(&d.binding).unwrap();
        raw["requested_action"] = json!("merge");
        raw["merge_requests"] = json!(1);
        raw["expected_head_sha"] = json!(d.binding.head_sha);
        raw["refreshed_head_sha"] = json!(d.binding.head_sha);
        raw["request_expected_head_sha"] = json!(d.binding.head_sha);
        raw["request_outcome"] = json!("accepted");
        raw["platform_state"] = json!("merged");
        let done: LoopCompletion = serde_json::from_value(super::completion()).unwrap();
        let observed = serde_json::from_value(raw.clone()).unwrap();
        let out = reduce_outcome(&d.auth, &accepted, &observed, &done).unwrap();
        if p.mode == DriveMode::Handoff {
            assert_eq!(out.action, Action::WorkflowViolation);
            assert_ne!(out.stop_reason, StopReason::Completed);
        } else {
            assert_eq!(out.action, Action::Merged, "{mode:?}");
            raw["request_expected_head_sha"] = Value::Null;
            let unsafe_merge = reduce_outcome(
                &d.auth,
                &accepted,
                &serde_json::from_value(raw).unwrap(),
                &done,
            )
            .unwrap();
            assert_eq!(unsafe_merge.action, Action::WorkflowViolation);
        }
    }
}

#[test]
fn dry_run_and_restricted_entrypoints_cannot_complete_handoff() {
    if in_isolated_home("dry_run_and_restricted_entrypoints_cannot_complete_handoff") {
        return;
    }
    for (entrypoint, dry_run) in [
        ("auto-drive-to-merge", true),
        ("auto-drive-light", false),
        ("autodrive-finalize", false),
    ] {
        let mut d = Drive::new(Some("handoff"));
        let auth = InvocationAuthorization::capture(
            entrypoint,
            &BTreeMap::from([
                ("merge".into(), "handoff".into()),
                ("autodrive_dry_run".into(), dry_run.to_string()),
            ]),
        );
        let Ok(auth) = auth else { continue };
        d.auth = auth;
        d.auth.bind_policy(d.binding.policy_provenance.clone());
        d.auth.bind_state_dir(d.directory.clone());
        d.binding.run_id = d.auth.run_id().into();
        d.host.pr.binding = d.binding.clone();
        let result = d.resolve().and_then(|p| d.prepare(&p));
        assert!(
            result.is_err(),
            "{entrypoint}/dry={dry_run} gained handoff authority"
        );
        assert_eq!(d.host.creations(), 0);
        assert!(!d.host.calls.contains(&Call::Ready));
    }
}

#[test]
fn handoff_reporting_requires_private_completion_and_rejects_altered_outcome() {
    if in_isolated_home("handoff_reporting_requires_private_completion_and_rejects_altered_outcome")
    {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    let result = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&result);
    let mut forged = result.clone();
    forged["outcome"]["binding"]["head_sha"] = json!(super::OTHER);
    assert_ne!(d.report(forged)["stop_reason"], "completed");
    fs::remove_file(result["completion_snapshot"].as_str().unwrap()).unwrap();
    assert_ne!(
        d.report(result)["stop_reason"],
        "completed",
        "comment alone established completion"
    );
}

#[test]
fn preparation_cannot_be_reused_with_a_different_private_policy_or_invocation() {
    if in_isolated_home(
        "preparation_cannot_be_reused_with_a_different_private_policy_or_invocation",
    ) {
        return;
    }
    let mut first = Drive::new(Some("handoff"));
    let (p, accepted) = first.ready();
    let mut second = Drive::new(Some("handoff"));
    let other = second.resolve().unwrap();
    assert!(
        first.complete(&other, &accepted).is_err(),
        "accepted foreign policy binding"
    );
    let mut unprepared = Drive::new(Some("handoff"));
    let own = unprepared.resolve().unwrap();
    let ready = unprepared.seal_ready();
    assert!(
        unprepared.complete(&own, &ready).is_err(),
        "skipped preparation evidence"
    );
    assert!(
        second.complete(&p, &accepted).is_err(),
        "accepted foreign invocation state"
    );
    let mut merge = Drive::new(Some("merge"));
    let mut forged = merge.resolve().unwrap();
    forged.mode = DriveMode::Handoff;
    assert!(
        merge.prepare(&forged).is_err(),
        "caller changed persisted drive mode"
    );
    let mut discovered = Drive::new(Some("handoff"));
    discovered.resolve().unwrap();
    let mut other_pr = discovered.binding.clone();
    other_pr.pr_number += 1;
    other_pr.pr_url = format!(
        "https://{}/{}/pull/{}",
        other_pr.host, other_pr.repository, other_pr.pr_number
    );
    discovered.host.pr.binding = other_pr.clone();
    assert!(
        policy::resolve(
            &discovered.auth,
            &discovered.store,
            &other_pr,
            &mut discovered.host
        )
        .is_err(),
        "policy resolution changed an implicitly selected PR within one invocation"
    );
    assert_eq!(discovered.host.creations(), 0);
}

#[test]
fn resolved_policy_must_remain_durable_before_handoff_can_complete() {
    if in_isolated_home("resolved_policy_must_remain_durable_before_handoff_can_complete") {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let p = d.resolve().unwrap();
    let policy_files: Vec<_> = fs::read_dir(&d.directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        !policy_files.is_empty(),
        "resolution did not persist policy authority"
    );
    d.prepare(&p).unwrap();
    let accepted = d.seal_ready();
    for path in policy_files {
        fs::remove_file(path).unwrap();
    }
    assert!(
        d.complete(&p, &accepted).is_err(),
        "in-memory policy replaced missing private authority"
    );
    assert_eq!(d.host.creations(), 0);
}

#[test]
fn handoff_policy_is_resolved_before_the_already_merged_shortcut() {
    if in_isolated_home("handoff_policy_is_resolved_before_the_already_merged_shortcut") {
        return;
    }
    for explicit in [true, false] {
        let mut d = Drive::new(if explicit { Some("handoff") } else { None });
        if !explicit {
            d.host.config = BaseConfig::File {
                bytes: b"merge='handoff'".to_vec(),
                mode: "100644".into(),
            };
        }
        d.resolve().unwrap();
        let context = json!({
            "autodrive_state_dir":d.directory,
            "build_result":{"merged_before_run":true,"pr_state":"MERGED",
                "entry_evidence":"Authenticated observation of a previously merged PR",
                "binding":d.binding}
        });
        let result = report::report(&d.auth, context.as_object().unwrap(), Some("2329"), &d.repo);
        assert_ne!(
            result["action"], "already_merged",
            "handoff accepted missing open PR: {result}"
        );
        assert_ne!(result["stop_reason"], "completed");
        assert_eq!(d.host.creations(), 0);
    }
}

#[test]
fn production_merge_dispatch_requires_current_merge_policy_and_readiness() {
    if in_isolated_home("production_merge_dispatch_requires_current_merge_policy_and_readiness") {
        return;
    }
    for mode in [None, Some("merge"), Some("handoff")] {
        let mut d = Drive::new(mode);
        let p = d.resolve().unwrap();
        let accepted = d.seal_ready();
        let result = crate::commands::orch::autodrive::helper::authorise_merge(
            &d.auth,
            &d.store,
            &d.repo,
            &accepted,
            &mut d.host,
        );
        if p.mode == DriveMode::Handoff {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap()["authorised"], true);
            d.host.pr.binding.head_sha = super::OTHER.into();
            assert!(
                crate::commands::orch::autodrive::helper::authorise_merge(
                    &d.auth,
                    &d.store,
                    &d.repo,
                    &accepted,
                    &mut d.host,
                )
                .is_err()
            );
        }
        assert_eq!(d.host.creations(), 0);
    }
}

#[test]
fn repeated_completion_reconciles_a_receipt_and_conflicting_receipts_fail_closed() {
    if in_isolated_home(
        "repeated_completion_reconciles_a_receipt_and_conflicting_receipts_fail_closed",
    ) {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let (p, accepted) = d.ready();
    let first = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&first);
    let second = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&second);
    assert_eq!(d.host.creations(), 1);
    let id = *d.host.comments.keys().next().unwrap();
    d.host.comments.get_mut(&id).unwrap().body = "Changed externally".into();
    assert!(d.complete(&p, &accepted).is_err());
    assert_eq!(d.host.creations(), 1);
}

#[test]
fn nested_round_acceptance_uses_parent_private_identity_without_granting_leaf_actions() {
    if in_isolated_home(
        "nested_round_acceptance_uses_parent_private_identity_without_granting_leaf_actions",
    ) {
        return;
    }
    let mut d = Drive::new(Some("handoff"));
    let p = d.resolve().unwrap();
    d.prepare(&p).unwrap();
    let launch = d._temp.path().join("launch");
    let private = EvidenceStore::create(&launch).unwrap();
    private
        .write_new("invocation.json", &serde_json::to_vec(&d.auth).unwrap())
        .unwrap();
    let parent = InvocationAuthorization::read_private(&launch.join("invocation.json")).unwrap();
    let mut leaf =
        InvocationAuthorization::capture("autodrive-merge-round", &BTreeMap::new()).unwrap();
    leaf.bind_state_dir(d.directory.clone());
    leaf.bind_policy(format!("trusted-launch-bundle:sha256:{}", "b".repeat(64)));
    assert_ne!(parent.run_id(), leaf.run_id());
    assert!(!leaf.allows_merge());
    assert!(!leaf.allows_drive());
    let record = d.directory.join("merge-ready-round-1.json");
    assert!(
        crate::commands::orch::autodrive::helper::accept_round(
            &leaf,
            &d.store,
            &d.repo,
            &record,
            &d.candidate,
        )
        .is_err()
    );
    assert!(!record.exists());
    let round = crate::commands::orch::autodrive::helper::accept_round(
        &parent,
        &d.store,
        &d.repo,
        &record,
        &d.candidate,
    )
    .unwrap();
    assert_eq!(round["merge_ready_verdict"], "MERGE_READY");
    d.store
        .write_new(
            "merge-ready-result.json",
            &serde_json::to_vec(&json!({
                "loop_result":"DONE", "round_label":"round-1", "round_verdict":"MERGE_READY"
            }))
            .unwrap(),
        )
        .unwrap();
    completion::record(&parent, &d.directory, "round-1").unwrap();
    let accepted = PathBuf::from(round["accepted_evidence"].as_str().unwrap());
    let result = d.complete(&p, &accepted).unwrap();
    d.assert_completed(&result);
    assert_eq!(d.host.creations(), 1);
}
