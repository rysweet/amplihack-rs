//! Issue #2787 TDD contracts. These execute Git, shipped tools and recipe
//! commands; the fake gh accepts ready only after inspecting the bare remote.
//! Shared with the standalone CI driver, with no new crate dependencies.

#[path = "workflow_finalize_publish_ready_fixture.rs"]
pub(crate) mod fixture;
use fixture::{BRANCH, Fixture, PR_URL, PUBLISH, READY, executable, show, success};
use std::fs;
use std::path::Path;

pub fn unpushed_commit(f: &Fixture) {
    f.audit(true);
    let local = f.head(&f.pr);
    assert_ne!(
        f.remote_head(),
        local,
        "fixture must start with an unpushed commit"
    );
    let initial = success(
        f.command("gh", &f.main)
            .args(["pr", "view", PR_URL, "--json", "headRefOid"])
            .output()
            .unwrap(),
    );
    assert!(
        initial.contains(&f.remote_head()),
        "stub must initially report the prior commit: {initial}"
    );
    assert!(
        !initial.contains(&local),
        "the unpushed commit must not be reported yet"
    );
    fs::write(f.root.join("events"), "").unwrap();
    success(f.publish());
    assert_eq!(
        f.head(&f.pr),
        local,
        "publication must preserve existing commits"
    );
    success(f.ready());
    f.assert_ready();
}

pub fn uncommitted_audit(f: &Fixture) {
    f.audit(false);
    let before = f.head(&f.pr);
    success(f.publish());
    assert_ne!(f.head(&f.pr), before, "audit edits need a cleanup commit");
    assert_eq!(
        f.git(&f.pr, &["log", "-1", "--format=%s"]).trim(),
        "final cleanup pass"
    );
    assert!(f.git(&f.pr, &["status", "--porcelain"]).is_empty());
    let events = fs::read_to_string(f.root.join("events")).unwrap();
    assert!(
        events.contains("commit-hook"),
        "commit hooks must execute: {events}"
    );
    assert!(
        events.contains("artifact-check"),
        "existing artifact checks must execute: {events}"
    );
    success(
        f.command("bash", &f.source)
            .args([
                "amplifier-bundle/tools/workflow_pr_ready.sh",
                "--base",
                "main",
            ])
            .output()
            .unwrap(),
    );
    f.assert_ready();
}

pub fn main_checkout_uses_pr_worktree(f: &Fixture) {
    let caller_head = f.head(&f.main);
    fs::write(f.main.join("caller-only.txt"), "leave this work in main\n").unwrap();
    f.audit(false);
    success(f.step(PUBLISH).output().unwrap());
    success(f.step(READY).output().unwrap());
    f.assert_ready();
    assert_eq!(f.head(&f.main), caller_head);
    assert_eq!(
        fs::read_to_string(f.main.join("caller-only.txt")).unwrap(),
        "leave this work in main\n"
    );
    assert_eq!(f.git(&f.pr, &["ls-files", "caller-only.txt"]), "");
}

pub fn alias_publishes_remote_branch(f: &Fixture) {
    let alias = format!("{BRANCH}--wt-1");
    f.git(&f.pr, &["branch", "-m", &alias]);
    f.audit(true);
    success(f.publish());
    success(f.ready());
    f.assert_ready();
    let refs = f.git(
        &f.remote,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    assert!(
        !refs.contains("--wt-"),
        "a local alias must not be pushed: {refs}"
    );
}

pub fn persistent_ancestor_enters_recovery(f: &Fixture) {
    f.audit(true);
    success(f.publish());
    fs::write(f.root.join("stale"), "").unwrap();
    let o = f
        .step(READY)
        .env("RECIPE_VAR_step_recovery_dir", f.root.join("recovery"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(81), "{}", show(&o));
    let evidence = fs::read_to_string(f.root.join("recovery-evidence.json"))
        .expect("the agent must receive validation evidence");
    assert!(evidence.contains("no_scoped_pr"), "{evidence}");
    assert!(
        evidence.contains(&f.head(&f.pr)),
        "expected commit missing: {evidence}"
    );
    assert!(
        evidence.contains(
            fs::read_to_string(f.root.join("prior-head"))
                .unwrap()
                .trim()
        ),
        "observed ancestor missing: {evidence}"
    );
    f.assert_no_mutation();
}

pub fn invalid_worktree_never_falls_back(f: &Fixture) {
    f.audit(true);
    // Make cwd independently valid so selecting it accidentally would mutate.
    f.git(
        &f.pr,
        &["push", "origin", &format!("HEAD:refs/heads/{BRANCH}")],
    );
    for context in [
        "".to_owned(),
        "not-json".to_owned(),
        "{}".to_owned(),
        format!("{{\"worktree_path\":\"{}/removed\"}}", f.root.display()),
    ] {
        for tool in ["workflow_finalize_publish.sh", "workflow_pr_ready.sh"] {
            let o = f
                .tool(tool, &f.pr)
                .env("RECIPE_VAR_worktree_setup", &context)
                .env("WORKTREE_SETUP_WORKTREE_PATH", &f.pr)
                .output()
                .unwrap();
            assert!(
                !o.status.success(),
                "{tool} accepted invalid context {context:?}: {}",
                show(&o)
            );
            f.assert_no_mutation();
        }
    }
}

pub fn missing_worktree_reaches_publication_recovery(f: &Fixture) {
    let o = f
        .step(PUBLISH)
        .env("RECIPE_VAR_worktree_setup", "{}")
        .env("RECIPE_VAR_step_recovery_dir", f.root.join("recovery"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(81), "{}", show(&o));
    let evidence = fs::read_to_string(f.root.join("recovery-evidence.json"))
        .expect("publication must permit agentic recovery");
    assert!(evidence.contains("worktree"), "{evidence}");
    f.assert_no_mutation();
}

pub fn missing_worktree_reaches_readiness_recovery(f: &Fixture) {
    let o = f
        .step(READY)
        .env("RECIPE_VAR_worktree_setup", "{}")
        .env("RECIPE_VAR_step_recovery_dir", f.root.join("recovery"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(81), "{}", show(&o));
    let evidence = fs::read_to_string(f.root.join("recovery-evidence.json")).unwrap();
    assert!(evidence.contains("worktree"), "{evidence}");
    f.assert_no_mutation();
}

pub fn push_output_redacts_credentials(f: &Fixture) {
    f.audit(true);
    executable(
        &f.remote.join("hooks/pre-receive"),
        "#!/bin/sh\necho 'http://secret-user:secret-pass@example.invalid/ https://secret-user:secret-pass@example.invalid/' >&2\nexit 1\n",
    );
    let o = f.publish();
    assert!(!o.status.success(), "{}", show(&o));
    let log = show(&o);
    assert!(log.contains("REDACTED"), "{log}");
    assert!(
        !log.contains("secret-user") && !log.contains("secret-pass"),
        "{log}"
    );
    f.assert_no_mutation();
}

pub fn artifact_failure_prevents_publication(f: &Fixture) {
    f.audit(false);
    let before = f.head(&f.pr);
    executable(
        &f.root.join("bin/amplihack"),
        "#!/bin/sh\necho 'artifact check failed' >&2\nexit 42\n",
    );
    let o = f.publish();
    assert!(!o.status.success(), "{}", show(&o));
    assert_eq!(f.head(&f.pr), before);
    assert_eq!(f.remote_head(), before);
    assert!(
        f.git(&f.pr, &["diff", "--cached", "--name-only"])
            .is_empty()
    );
    f.assert_no_mutation();
}

pub fn rejected_push_is_not_success(f: &Fixture) {
    // Prove the operation is runnable; a missing helper is not a push rejection.
    success(f.publish());
    f.audit(true);
    let prior = f.remote_head();
    executable(
        &f.remote.join("hooks/pre-receive"),
        "#!/bin/sh\necho 'fixture push rejected' >&2\nexit 1\n",
    );
    let o = f.publish();
    assert!(!o.status.success(), "{}", show(&o));
    assert_eq!(f.remote_head(), prior);
    f.assert_no_mutation();
}

pub fn publication_recovery_reexecutes_after_local_repair(f: &Fixture) {
    f.audit(true);
    executable(&f.remote.join("hooks/pre-receive"), "#!/bin/sh\nexit 1\n");
    executable(
        &f.root.join("repair"),
        "#!/bin/sh\nrm -- \"$FIXTURE/remotes/acme/widget.git/hooks/pre-receive\"\n",
    );
    success(
        f.step(PUBLISH)
            .env("RECIPE_VAR_step_recovery_dir", f.root.join("recovery"))
            .output()
            .unwrap(),
    );
    assert!(
        f.root.join("recovery-evidence.json").exists(),
        "repair must be agentic"
    );
    assert_eq!(
        f.remote_head(),
        f.head(&f.pr),
        "the original operation must publish after repair"
    );
    success(f.ready());
    f.assert_ready();
}

pub fn divergence_preserves_history(f: &Fixture) {
    success(f.publish());
    let peer = f.root.join("peer");
    f.git(
        &f.main,
        &[
            "worktree",
            "add",
            "-qb",
            "peer",
            peer.to_str().unwrap(),
            BRANCH,
        ],
    );
    fs::write(peer.join("remote-only.txt"), "remote work\n").unwrap();
    f.git(&peer, &["add", "."]);
    f.git(&peer, &["commit", "-qm", "remote contribution"]);
    f.git(
        &peer,
        &["push", "origin", &format!("HEAD:refs/heads/{BRANCH}")],
    );
    let remote = f.remote_head();
    f.audit(true);
    let local = f.head(&f.pr);
    let o = f.publish();
    assert!(
        !o.status.success(),
        "divergence must enter recovery without rewriting: {}",
        show(&o)
    );
    assert_eq!(
        f.head(&f.pr),
        local,
        "publication must not rebase local commits"
    );
    assert_eq!(f.remote_head(), remote);
    f.assert_no_mutation();
}

pub fn remote_fast_forward_keeps_audit(f: &Fixture) {
    let peer = f.root.join("peer");
    f.git(
        &f.main,
        &[
            "worktree",
            "add",
            "-qb",
            "peer",
            peer.to_str().unwrap(),
            BRANCH,
        ],
    );
    fs::write(peer.join("remote-only.txt"), "remote work\n").unwrap();
    f.git(&peer, &["add", "."]);
    f.git(&peer, &["commit", "-qm", "remote contribution"]);
    let remote = f.head(&peer);
    f.git(
        &peer,
        &["push", "origin", &format!("HEAD:refs/heads/{BRANCH}")],
    );
    // Ignore the fixture's setup push when checking operation order.
    fs::write(f.root.join("events"), "").unwrap();
    f.audit(false);
    success(f.publish());
    f.git(&f.pr, &["merge-base", "--is-ancestor", &remote, "HEAD"]);
    success(f.ready());
    f.assert_ready();
}

pub fn identity_and_secondary_filters_still_apply(f: &Fixture) {
    f.audit(true);
    success(f.publish());
    let original = fs::read(f.root.join("candidate.json")).unwrap();
    for filter in [
        ".number=8",
        ".url=\"https://github.com/other/widget/pull/7\"",
        ".headRefName=\"other-branch\"",
        ".baseRefName=\"dev\"",
        ".headRepositoryOwner.login=\"other\"",
        ".isCrossRepository=true",
        ".body=\"unrelated work\" | .title=\"Fix: unrelated\"",
        ".createdAt=\"2025-01-01T00:00:00Z\"",
        ".title=\"Other prefix #2787\"",
        "del(.headRefName)",
        "del(.headRepositoryOwner)",
        "del(.state)",
        "del(.isDraft)",
    ] {
        fs::write(f.root.join("candidate.json"), &original).unwrap();
        f.patch_candidate(filter);
        let o = f
            .tool("workflow_pr_ready.sh", &f.pr)
            .args(["--base", "main"])
            .env("EXPECTED_PR_TITLE_PREFIX", "Fix:")
            .env("WORKFLOW_STARTED_AT", "2026-11-18T01:23:00Z")
            .output()
            .unwrap();
        assert!(
            !o.status.success(),
            "scope mismatch {filter} accepted: {}",
            show(&o)
        );
        f.assert_no_mutation();
    }
}

pub fn empty_lookup_is_failure(f: &Fixture) {
    f.audit(true);
    success(f.publish());
    fs::write(f.root.join("empty"), "").unwrap();
    let o = f
        .tool("workflow_pr_ready.sh", &f.pr)
        .args(["--base", "main"])
        .env_remove("RECIPE_VAR_pr_number")
        .env_remove("RECIPE_VAR_pr_url")
        .output()
        .unwrap();
    assert!(
        !o.status.success(),
        "empty lookup must not imply readiness: {}",
        show(&o)
    );
    let stderr = String::from_utf8_lossy(&o.stderr).to_lowercase();
    assert!(
        stderr.contains("empty")
            || stderr.contains("no candidates")
            || stderr.contains("0 candidates"),
        "empty lookup needs distinct diagnostics: {}",
        show(&o)
    );
    f.assert_no_mutation();
}

pub fn diagnostics_report_all_exclusions(f: &Fixture) {
    f.audit(true);
    f.patch_candidate(
        r#".headRefName="wrong-head" | .baseRefName="wrong-base" |
        .title="wrong-prefix" | .body="unrelated" | .createdAt="2020-01-01T00:00:00Z""#,
    );
    let o = f
        .tool("workflow_pr_scope.sh", &f.pr)
        .args([
            "--repo",
            "acme/widget",
            "--head",
            BRANCH,
            "--base",
            "main",
            "--pr-number",
            "7",
            "--pr-url",
            PR_URL,
            "--head-sha",
            &f.head(&f.pr),
            "--issue",
            "2787",
            "--work-item",
            "2787",
            "--created-after",
            "2026-11-18T01:23:00Z",
            "--expected-pr-title-prefix",
            "Fix:",
            "--recipe-run-id",
            "run-2787",
            "--tree-id",
            "tree-2787",
            "--workstream-id",
            "stream-2787",
        ])
        .output()
        .unwrap();
    assert!(!o.status.success(), "{}", show(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        stdout.contains("no_scoped_pr"),
        "structured failure compatibility: {stdout}"
    );
    let result = f.root.join("scope-result.json");
    fs::write(&result, &o.stdout).unwrap();
    success(
        f.command("jq", &f.main)
            .args([
                "-se",
                "length == 1 and .[0].ok == false and .[0].reason == \"no_scoped_pr\"",
            ])
            .arg(result)
            .output()
            .unwrap(),
    );
    let stderr = String::from_utf8_lossy(&o.stderr);
    for value in [
        PR_URL,
        BRANCH,
        "wrong-head",
        "main",
        "wrong-base",
        &f.head(&f.pr),
        &f.remote_head(),
        "2787",
        "2026-11-18T01:23:00Z",
        "2020-01-01T00:00:00Z",
        "Fix:",
        "wrong-prefix",
        "run-2787",
        "tree-2787",
        "stream-2787",
    ] {
        assert!(
            stderr.contains(value),
            "missing exclusion detail {value:?}: {stderr}"
        );
    }
    for label in ["head", "base", "issue", "work", "creat", "title"] {
        assert!(
            stderr.to_lowercase().contains(label),
            "missing discriminator {label}: {stderr}"
        );
    }
    let reports = files_under(&f.root.join("runtime"));
    let report = reports
        .iter()
        .find(|p| {
            fs::read_to_string(p).is_ok_and(|s| {
                s.contains("wrong-head") && s.contains("wrong-base") && s.contains("run-2787")
            })
        })
        .unwrap_or_else(|| panic!("complete candidate exclusions must be persisted under runtime root; files={reports:?}; stderr={stderr}"));
    assert!(
        stderr.contains(report.to_str().unwrap()),
        "diagnostic artifact path missing: {stderr}"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(report).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

pub fn hostile_candidate_text_is_data(f: &Fixture) {
    f.audit(true);
    f.patch_candidate(r#".title="\u001b[31m$(touch injected) https://secret-user:secret-pass@example.invalid/" | .body="unrelated""#);
    let o = f
        .tool("workflow_pr_scope.sh", &f.pr)
        .args([
            "--repo",
            "acme/widget",
            "--head",
            BRANCH,
            "--base",
            "main",
            "--pr-number",
            "7",
            "--expected-pr-title-prefix",
            "Fix:",
            "--issue",
            "2787",
        ])
        .output()
        .unwrap();
    assert!(!o.status.success(), "{}", show(&o));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.to_lowercase().contains("untrusted"),
        "candidate text must be marked as evidence: {stderr}"
    );
    assert!(
        !stderr.contains('\u{1b}'),
        "raw terminal control in {stderr:?}"
    );
    assert!(!f.pr.join("injected").exists());
    for path in files_under(&f.root.join("runtime")) {
        let data = fs::read_to_string(path).unwrap_or_default();
        assert!(
            !data.contains("secret-user") && !data.contains("secret-pass"),
            "credential leaked: {data}"
        );
    }
}

fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap().map(Result::unwrap) {
        if entry.file_type().unwrap().is_dir() {
            out.extend(files_under(&entry.path()));
        } else if entry.file_type().unwrap().is_file() {
            out.push(entry.path());
        }
    }
    out
}

/// Execute the affected commands in their actual recipe order. Reading YAML
/// here is command loading, never a source-text assertion.
pub fn recipe_publication_precedes_readiness(f: &Fixture) {
    f.audit(false);
    let ids = success(f.command("awk", &f.main).arg(
        r#"/^[ ]*- id:/ && /step-20b-push-cleanup|step-21-pr-ready/ { x=$3; gsub(/"/, "", x); print x }"#)
        .arg(f.source.join("amplifier-bundle/recipes/workflow-finalize.yaml"))
        .output().unwrap());
    let mut executed = 0;
    for id in ids.lines() {
        success(f.step(id).output().unwrap());
        executed += 1;
    }
    assert_eq!(executed, 2, "both finalize commands must execute");
    f.assert_ready();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Fixture {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Fixture::new(&source, Path::new(env!("CARGO_BIN_EXE_amplihack")))
    }
    macro_rules! contract {
        ($($name:ident),+ $(,)?) => { $(#[test] fn $name() { super::$name(&fixture()); })+ };
    }
    contract!(
        missing_worktree_reaches_readiness_recovery,
        push_output_redacts_credentials,
        artifact_failure_prevents_publication,
        unpushed_commit,
        uncommitted_audit,
        main_checkout_uses_pr_worktree,
        alias_publishes_remote_branch,
        persistent_ancestor_enters_recovery,
        invalid_worktree_never_falls_back,
        missing_worktree_reaches_publication_recovery,
        rejected_push_is_not_success,
        publication_recovery_reexecutes_after_local_repair,
        divergence_preserves_history,
        remote_fast_forward_keeps_audit,
        identity_and_secondary_filters_still_apply,
        empty_lookup_is_failure,
        diagnostics_report_all_exclusions,
        hostile_candidate_text_is_data,
        recipe_publication_precedes_readiness
    );
}
