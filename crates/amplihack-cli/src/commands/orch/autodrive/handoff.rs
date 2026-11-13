//! Ready conversion, readiness replay and durable steward handoff.
use super::{
    authorization::InvocationAuthorization,
    completion, helper,
    hosting::*,
    policy::{self, DriveMode, ResolvedPolicy},
    records::*,
    state::EvidenceStore,
    validation::validate_candidate,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const STEWARD: &str = "This pull request is ready for its steward.";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    policy: ResolvedPolicy,
    observation: PrObservation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    schema_version: u32,
    binding: Binding,
    policy: ResolvedPolicy,
    preparation: Preparation,
    readiness: CandidateRecord,
    observation: PrObservation,
    comment: Comment,
    authenticated_author: String,
    merge_requests: u32,
    scheduling_requests: u32,
}
fn preparation_name(policy: &ResolvedPolicy) -> Result<String> {
    Ok(format!(
        "handoff-prepared-{:x}.json",
        Sha256::digest(serde_json::to_vec(policy)?)
    ))
}
fn receipt_name(auth: &InvocationAuthorization, b: &Binding) -> String {
    format!("handoff-comment-{}-{}.json", auth.run_id(), b.pr_number)
}
fn check_pr(b: &Binding, pr: &PrObservation, ready: bool) -> Result<()> {
    ensure!(
        policy::same_binding(b, &pr.binding),
        "handoff-pr-changed: refresh PR/head/base evidence"
    );
    ensure!(
        pr.state == PlatformState::Open,
        "handoff-pr-not-open: an open PR is required"
    );
    ensure!(
        !pr.auto_merge_enabled,
        "handoff-auto-merge-enabled: scheduled merging prevents handoff"
    );
    ensure!(
        !ready || !pr.draft,
        "handoff-pr-draft: ready status not confirmed"
    );
    Ok(())
}
fn observe(host: &mut impl PolicyHosting, b: &Binding, ready: bool) -> Result<PrObservation> {
    let pr = host.observe_pr(b).context("handoff-observe-failed")?;
    check_pr(b, &pr, ready)?;
    ensure!(
        host.base_tip(b).context("handoff-base-observe-failed")? == b.base_sha,
        "handoff-base-changed: resolve policy and refresh readiness"
    );
    Ok(pr)
}
fn attempt(store: &EvidenceStore, b: &Binding, operation: &str, status: &str) -> Result<()> {
    store.snapshot(&serde_json::to_vec(
        &json!({"binding":b,"operation":operation,"status":status}),
    )?)?;
    Ok(())
}
fn external<T>(
    store: &EvidenceStore,
    b: &Binding,
    operation: &str,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    attempt(store, b, operation, "started")?;
    match action() {
        Ok(value) => {
            attempt(store, b, operation, "response_received").with_context(|| {
                format!("handoff-observation-persistence-failed: {operation} may have occurred")
            })?;
            Ok(value)
        }
        Err(error) => {
            if let Err(storage) = attempt(store, b, operation, "uncertain") {
                return Err(error).context(format!("handoff-attempt-persistence-failed: {storage:#}; external action may have occurred"));
            }
            Err(error).context("handoff-external-action-unconfirmed: reconcile before repeating")
        }
    }
}
/// Preparation performs no readiness acceptance and posts no comment.
pub fn prepare(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    policy: &ResolvedPolicy,
    host: &mut impl HandoffHosting,
) -> Result<Value> {
    let _lock = store.handoff_lock()?;
    policy::require(auth, store, policy, DriveMode::Handoff)?;
    let b = &policy.binding;
    let pr = observe(host, b, false)?;
    if pr.draft {
        external(store, b, "mark_ready", || host.mark_ready(b))?;
    }
    let observation = observe(host, b, true)?;
    let prepared = Preparation {
        policy: policy.clone(),
        observation,
    };
    let name = preparation_name(policy)?;
    match store.read_optional(&name)? {
        Some(bytes) => ensure!(
            serde_json::from_slice::<Preparation>(&bytes)? == prepared,
            "handoff-preparation-conflict"
        ),
        None => store.write_new(&name, &serde_json::to_vec(&prepared)?)?,
    }
    Ok(json!({"prepared":true,"mode":"handoff","binding":b}))
}
fn preparation(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    policy: &ResolvedPolicy,
) -> Result<Preparation> {
    policy::require(auth, store, policy, DriveMode::Handoff)?;
    let prepared: Preparation = serde_json::from_slice(
        &store
            .read_private(&preparation_name(policy)?)
            .context("handoff-preparation-missing")?,
    )?;
    ensure!(
        prepared.policy == *policy,
        "handoff-preparation-policy-mismatch"
    );
    check_pr(&policy.binding, &prepared.observation, true)?;
    Ok(prepared)
}
fn check_comment(b: &Binding, comment: &Comment, login: &str) -> Result<()> {
    ensure!(
        comment.id > 0
            && comment.pr_url == b.pr_url
            && comment.author == login
            && comment.body == STEWARD,
        "handoff-comment-mismatch: comment identity, author or body differs"
    );
    Ok(())
}
fn reconcile(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    b: &Binding,
    host: &mut impl HandoffHosting,
    login: &str,
) -> Result<Comment> {
    let name = receipt_name(auth, b);
    let stored = store
        .read_optional(&name)?
        .map(|bytes| serde_json::from_slice::<Comment>(&bytes))
        .transpose()?;
    if let Some(receipt) = &stored {
        check_comment(b, receipt, login)?;
        let readback = host
            .read_comment(b, receipt.id)
            .context("handoff-comment-receipt-unreadable")?;
        ensure!(readback == *receipt, "handoff-comment-receipt-changed");
    }
    // Complete enumeration is required even with a receipt: duplicates or unreadable
    // later pages must remain failures. Cursor progress is finite, never timed.
    let mut cursor = None;
    let mut seen_pages = BTreeSet::new();
    let mut seen_comments = BTreeSet::new();
    let mut matching = Vec::new();
    loop {
        ensure!(
            seen_pages.len() < 10000 && seen_pages.insert(cursor.clone()),
            "handoff-comments-incomplete: cyclic or excessive pagination"
        );
        let page = host
            .list_comments(b, cursor.as_deref())
            .context("handoff-comments-unreadable")?;
        for comment in page.comments {
            ensure!(
                comment.pr_url == b.pr_url && comment.id > 0 && seen_comments.insert(comment.id),
                "handoff-comments-inconsistent: wrong PR or repeated identity"
            );
            if comment.author == login && comment.body == STEWARD {
                matching.push(comment);
            }
        }
        match page.next {
            Some(next) => {
                ensure!(!next.is_empty(), "handoff-comments-empty-cursor");
                cursor = Some(next);
            }
            None => break,
        }
    }
    ensure!(
        matching.len() <= 1,
        "handoff-comments-ambiguous: multiple steward comments"
    );
    let selected = match matching.pop() {
        Some(comment) => comment,
        None => {
            ensure!(stored.is_none(), "handoff-comment-receipt-not-listed");
            // Confirm readiness-related platform facts immediately before the write.
            observe(host, b, true)?;
            external(store, b, "create_steward_comment", || {
                host.create_comment(b, STEWARD)
            })?
        }
    };
    check_comment(b, &selected, login)?;
    if let Some(receipt) = &stored {
        ensure!(receipt == &selected, "handoff-comment-receipt-mismatch");
    }
    let confirmed = host
        .read_comment(b, selected.id)
        .context("handoff-comment-confirmation-failed")?;
    ensure!(
        confirmed == selected,
        "handoff-comment-confirmation-changed"
    );
    if stored.is_none() {
        store
            .write_new(&name, &serde_json::to_vec(&confirmed)?)
            .context("handoff-comment-receipt-persistence-failed: external comment exists")?;
    }
    Ok(confirmed)
}
fn readiness(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    repo: &Path,
    accepted: &Path,
    policy: &ResolvedPolicy,
) -> Result<CandidateRecord> {
    let candidate: CandidateRecord = serde_json::from_slice(&store.read_path(accepted)?)?;
    ensure!(
        policy::same_binding(&candidate.binding, &policy.binding),
        "handoff-readiness-binding-mismatch"
    );
    helper::local_binding(repo, &candidate.binding)?;
    super::review::verify(repo, &candidate)?;
    validate_candidate(auth, &candidate.binding, &candidate)?;
    ensure!(
        candidate.readiness == Readiness::MergeReady,
        "handoff-readiness-incomplete"
    );
    store.snapshot_references(&candidate)?;
    completion::verify(auth, store.root(), &candidate)?;
    super::followups::read(store, store.root(), candidate.binding.pr_number)?;
    Ok(candidate)
}
fn outcome(c: &Completion) -> Outcome {
    Outcome {
        schema_version: 1,
        binding: c.binding.clone(),
        readiness: Readiness::MergeReady,
        action: Action::HandedOff,
        platform_state: PlatformState::Open,
        stop_reason: StopReason::Completed,
        blockers: vec![],
        attempts: c.readiness.attempts.clone(),
        failure_cause: None,
    }
}
/// Replay production acceptance, reconcile external effects, then publish success.
pub fn complete(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    policy: &ResolvedPolicy,
    repo: &Path,
    accepted: &Path,
    host: &mut impl HandoffHosting,
) -> Result<Value> {
    let _lock = store.handoff_lock()?;
    let prepared = preparation(auth, store, policy)?;
    let candidate = readiness(auth, store, repo, accepted, policy)?;
    observe(host, &candidate.binding, true)?;
    let login = host
        .authenticated_login()
        .context("handoff-authenticated-author-unavailable")?;
    ensure!(
        !login.trim().is_empty(),
        "handoff-authenticated-author-empty"
    );
    let comment = reconcile(auth, store, &candidate.binding, host, &login)?;
    let observation = observe(host, &candidate.binding, true)?;
    // A partial external action remains visible if any subsequent validation or write fails.
    let refreshed = readiness(auth, store, repo, accepted, policy)?;
    ensure!(candidate == refreshed, "handoff-readiness-changed");
    let done = Completion {
        schema_version: 1,
        binding: candidate.binding.clone(),
        policy: policy.clone(),
        preparation: prepared,
        readiness: candidate,
        observation,
        comment,
        authenticated_author: login,
        merge_requests: 0,
        scheduling_requests: 0,
    };
    let completion_snapshot = store
        .snapshot(&serde_json::to_vec(&done)?)
        .context("handoff-completion-persistence-failed")?;
    let outcome = outcome(&done);
    let outcome_snapshot = store
        .snapshot(&serde_json::to_vec(&outcome)?)
        .context("handoff-outcome-persistence-failed")?;
    Ok(
        json!({"outcome":outcome,"completion_snapshot":completion_snapshot,"outcome_snapshot":outcome_snapshot,"accepted_snapshot":accepted,"repo":repo.canonicalize()?}),
    )
}
/// Runner authority replays private evidence independently of the helper's output.
pub(crate) fn validate_report(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    receipt: &Value,
    repo: &Path,
) -> Result<Outcome> {
    let path = |key: &str| -> Result<PathBuf> {
        Ok(PathBuf::from(receipt[key].as_str().with_context(|| {
            format!("handoff-report-missing: {key}")
        })?))
    };
    let done: Completion =
        serde_json::from_slice(&store.read_path(&path("completion_snapshot")?)?)?;
    ensure!(
        done.schema_version == 1 && done.merge_requests == 0 && done.scheduling_requests == 0,
        "handoff-report-contradictory-actions"
    );
    let prepared = preparation(auth, store, &done.policy)?;
    ensure!(
        prepared == done.preparation,
        "handoff-report-preparation-mismatch"
    );
    let current = readiness(auth, store, repo, &path("accepted_snapshot")?, &done.policy)?;
    ensure!(
        current == done.readiness && current.binding == done.binding,
        "handoff-report-readiness-mismatch"
    );
    check_pr(&done.binding, &done.observation, true)?;
    check_comment(&done.binding, &done.comment, &done.authenticated_author)?;
    let stored_comment: Comment =
        serde_json::from_slice(&store.read_private(&receipt_name(auth, &done.binding))?)?;
    ensure!(
        stored_comment == done.comment,
        "handoff-report-comment-mismatch"
    );
    let expected = outcome(&done);
    let stored: Outcome = serde_json::from_slice(&store.read_path(&path("outcome_snapshot")?)?)?;
    ensure!(
        expected == stored && serde_json::to_value(&expected)? == receipt["outcome"],
        "handoff-report-outcome-mismatch"
    );
    Ok(expected)
}
