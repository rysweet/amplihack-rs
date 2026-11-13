use super::{authorization::InvocationAuthorization, records::*, validation::AcceptedRecord};
use anyhow::{Result, ensure};

/// Reduce evidence and observations; command success alone never establishes a merge.
pub fn reduce_outcome(
    auth: &InvocationAuthorization,
    accepted: &AcceptedRecord,
    f: &FinalizationObservation,
    done: &LoopCompletion,
) -> Result<Outcome> {
    let r = accepted.record();
    ensure!(
        r.binding == f.binding && f.binding.run_id == auth.run_id(),
        "finalization identity mismatch"
    );
    let mut out = Outcome {
        schema_version: 1,
        binding: r.binding.clone(),
        readiness: r.readiness,
        action: Action::NotRequested,
        platform_state: f.platform_state,
        stop_reason: done.stop_reason,
        blockers: r.blockers.clone(),
        attempts: r.attempts.clone(),
        failure_cause: f.failure_cause.clone(),
    };
    out.attempts.extend(f.attempts.clone());
    let fresh = f.expected_head_sha == r.binding.head_sha
        && f.refreshed_head_sha == r.binding.head_sha
        && f.mutable_facts_refreshed
        && !f.confirmation_evidence.trim().is_empty();
    let converged = done.accepted_round_verdict == RoundVerdict::Ready
        && done.evaluator_verdict == EvaluatorVerdict::Done
        && done.stop_reason == StopReason::Completed;
    if !fresh
        || !converged
        || matches!(
            f.platform_state,
            PlatformState::Closed | PlatformState::Unknown
        )
    {
        out.readiness = Readiness::Incomplete;
    }
    let retried_unobserved = f.merge_requests > 1
        && f.attempts.iter().any(|a| {
            a.outcome == AttemptOutcome::Ambiguous
                && !a.recovery.as_ref().is_some_and(|r| {
                    r.observed_before_retry && r.complete && !r.evidence.trim().is_empty()
                })
        });
    let attempted = f.merge_requests > 0 || f.request_outcome != RequestOutcome::NotRequested;
    let violation = (f.platform_state_before_action == PlatformState::Merged
        && f.platform_state_at_entry != PlatformState::Merged)
        || (f.merge_requests == 0 && f.request_outcome != RequestOutcome::NotRequested)
        || (f.merge_requests > 0 && f.request_outcome == RequestOutcome::NotRequested)
        || (f.requested_action == RequestedAction::Merge && !auth.allows_merge())
        || (attempted
            && (!auth.allows_merge()
                || f.requested_action != RequestedAction::Merge
                || r.readiness != Readiness::MergeReady
                || !fresh
                || !converged
                || f.request_expected_head_sha.as_deref() != Some(r.binding.head_sha.as_str())))
        || retried_unobserved;
    out.action = if violation {
        Action::WorkflowViolation
    } else if f.platform_state == PlatformState::Merged
        && f.platform_state_at_entry == PlatformState::Merged
        && !attempted
        && !f.confirmation_evidence.trim().is_empty()
    {
        Action::AlreadyMerged
    } else if f.platform_state == PlatformState::Merged && !attempted {
        Action::WorkflowViolation
    } else if attempted {
        match f.request_outcome {
            RequestOutcome::Refused => Action::Refused,
            RequestOutcome::Failed => Action::Failed,
            _ if f.platform_state == PlatformState::Merged
                && !f.confirmation_evidence.trim().is_empty() =>
            {
                Action::Merged
            }
            RequestOutcome::Queued => Action::Pending,
            _ => Action::Unconfirmed,
        }
    } else {
        Action::NotRequested
    };
    if matches!(out.action, Action::Refused | Action::Failed) {
        ensure!(
            out.failure_cause
                .as_ref()
                .is_some_and(|c| !c.trim().is_empty()),
            "failed finalization must preserve its cause"
        );
    }
    let complete_action = matches!(out.action, Action::Merged | Action::AlreadyMerged)
        || (!auth.allows_merge()
            && out.action == Action::NotRequested
            && f.platform_state == PlatformState::Open);
    if out.stop_reason == StopReason::Completed
        && (out.readiness != Readiness::MergeReady || !complete_action)
    {
        out.stop_reason = StopReason::Blockers;
    }
    if out.action == Action::WorkflowViolation {
        out.stop_reason = StopReason::Blockers;
        out.blockers.push("finalization violated invocation authorization, freshness, or expected-head protection".into());
    }
    Ok(out)
}
