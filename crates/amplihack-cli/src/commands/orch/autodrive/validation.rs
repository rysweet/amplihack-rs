use super::{authorization::InvocationAuthorization, records::*};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
pub const CRITERIA: [&str; 13] = [
    "qa",
    "documentation",
    "audit_convergence",
    "checks",
    "metadata",
    "approvals",
    "conflicts",
    "protection",
    "linked_issues",
    "conversations",
    "final_verification",
    "description_evidence",
    "scope",
];

/// A record accepted against this invocation's identity. Construction is private.
#[derive(Debug, Clone)]
pub struct AcceptedRecord(pub(super) CandidateRecord);
impl AcceptedRecord {
    pub fn record(&self) -> &CandidateRecord {
        &self.0
    }
}

pub fn validate_candidate(
    auth: &InvocationAuthorization,
    expected: &Binding,
    record: &CandidateRecord,
) -> Result<AcceptedRecord> {
    ensure!(
        serde_json::to_vec(record)?.len() <= MAX_RECORD_BYTES,
        "evidence record exceeds size limit"
    );
    ensure!(
        record.schema_version == 1,
        "unsupported evidence schema_version"
    );
    ensure!(
        record.binding == *expected && expected.run_id == auth.run_id(),
        "evidence identity differs from current invocation"
    );
    validate_binding(expected)?;
    auth.validate_selection(expected)?;
    if let Some(policy) = auth.policy_provenance() {
        ensure!(
            expected.policy_provenance == policy,
            "evidence policy differs from runner-pinned launch bundle"
        );
    }
    ensure!(
        [40, 64].contains(&record.reviewed_head_sha.len())
            && record
                .reviewed_head_sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            && record.verified_head_sha == expected.head_sha
            && (record.reviewed_head_sha == expected.head_sha
                || record
                    .review_range
                    .as_ref()
                    .is_some_and(|range| range.reviewed_head_sha == record.reviewed_head_sha
                        && range.verified_head_sha == expected.head_sha
                        && range.base_sha == expected.base_sha
                        && !range.observation.trim().is_empty())),
        "stale verification or crusty review head; re-review required"
    );
    ensure!(
        record.criteria.len() == CRITERIA.len(),
        "all 13 criteria are required exactly once"
    );
    let mut seen = BTreeSet::new();
    for c in &record.criteria {
        ensure!(
            CRITERIA.contains(&c.id.as_str()) && seen.insert(&c.id),
            "unknown or duplicate criterion {}",
            c.id
        );
        ensure!(
            c.binding == *expected,
            "criterion {} has stale or mismatched binding",
            c.id
        );
        let satisfied = match c.status {
            CriterionStatus::Verified => true,
            // Documentation allows no user-facing change; QA and access failures never waive policy.
            CriterionStatus::NotApplicable => {
                ensure!(
                    matches!(c.id.as_str(), "documentation" | "linked_issues")
                        && c.not_applicable_reason
                            .as_ref()
                            .is_some_and(|s| !s.trim().is_empty()),
                    "criterion {} lacks permitted non-applicability",
                    c.id
                );
                true
            }
            _ => false,
        };
        if satisfied {
            ensure!(
                !c.evidence.is_empty()
                    && c.evidence
                        .iter()
                        .all(|e| !e.source.trim().is_empty() && !e.observation.trim().is_empty()),
                "criterion {} lacks concrete observations",
                c.id
            );
            ensure!(
                c.blockers.is_empty(),
                "criterion {} contradicts its blockers",
                c.id
            );
        }
        if record.readiness == Readiness::MergeReady {
            ensure!(satisfied, "criterion {} is not verified", c.id);
        } else if !satisfied {
            ensure!(
                !c.blockers.is_empty(),
                "incomplete criterion {} must name its cause",
                c.id
            );
        }
    }
    if record.readiness == Readiness::MergeReady {
        ensure!(record.blockers.is_empty(), "readiness contradicts blockers");
        for a in &record.attempts {
            if a.outcome != AttemptOutcome::Succeeded {
                ensure!(
                    a.recovery.as_ref().is_some_and(|r| r.complete
                        && !r.interface.trim().is_empty()
                        && !r.evidence.trim().is_empty()),
                    "unrecovered failure: {}",
                    a.cause
                );
            }
        }
    } else {
        ensure!(
            !record.blockers.is_empty(),
            "incomplete readiness must name blockers"
        );
    }
    Ok(AcceptedRecord(record.clone()))
}
pub fn validate_binding(b: &Binding) -> Result<()> {
    for sha in [&b.head_sha, &b.base_sha, &b.merge_base_sha] {
        ensure!(
            [40, 64].contains(&sha.len()) && sha.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid revision in evidence binding"
        );
    }
    ensure!(
        b.pr_number > 0 && b.platform == "github",
        "unsupported platform or invalid PR number"
    );
    for value in [
        &b.run_id,
        &b.round_id,
        &b.host,
        &b.repository,
        &b.source_repository,
        &b.head_branch,
        &b.base_branch,
        &b.policy_provenance,
    ] {
        ensure!(
            !value.trim().is_empty() && !value.chars().any(char::is_control),
            "invalid empty or control-bearing identity"
        );
    }
    ensure!(
        !b.host.contains(['/', '@', ':'])
            && b.repository.split('/').count() == 2
            && b.source_repository.split('/').count() == 2,
        "invalid repository or host"
    );
    ensure!(
        b.pr_url == format!("https://{}/{}/pull/{}", b.host, b.repository, b.pr_number),
        "PR URL does not match identity"
    );
    Ok(())
}

/// #2556: a string the writer could not observe. The reader turns `null` and
/// a missing key into `""`; the writer contract spells the same thing
/// `"unknown"`.
pub fn is_unknown(value: &str) -> bool {
    value.trim().is_empty() || value == "unknown"
}

/// #2556: name every unknown value in the null-tolerant string fields as an
/// `unknown-field:<path>` blocker, sorted and de-duplicated. Paths use array
/// positions; a criterion is named by its id only when that id is one of
/// [`CRITERIA`], so no record text reaches a blocker. A succeeded attempt has
/// no cause to report, so its empty cause is not a blocker.
pub fn unknown_fields(record: &CandidateRecord) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut check = |path: String, value: &str| {
        if is_unknown(value) {
            found.insert(format!("unknown-field:{path}"));
        }
    };
    binding_unknowns(&mut check, "binding", &record.binding);
    check("reviewed_head_sha".into(), &record.reviewed_head_sha);
    check("verified_head_sha".into(), &record.verified_head_sha);
    if let Some(range) = &record.review_range {
        check(
            "review_range.reviewed_head_sha".into(),
            &range.reviewed_head_sha,
        );
        check(
            "review_range.verified_head_sha".into(),
            &range.verified_head_sha,
        );
        check("review_range.base_sha".into(), &range.base_sha);
        check("review_range.observation".into(), &range.observation);
    }
    for (i, attempt) in record.attempts.iter().enumerate() {
        check(format!("attempts[{i}].operation"), &attempt.operation);
        if attempt.outcome != AttemptOutcome::Succeeded {
            check(format!("attempts[{i}].cause"), &attempt.cause);
        }
        if let Some(recovery) = &attempt.recovery {
            check(
                format!("attempts[{i}].recovery.interface"),
                &recovery.interface,
            );
            check(
                format!("attempts[{i}].recovery.evidence"),
                &recovery.evidence,
            );
        }
    }
    for (i, criterion) in record.criteria.iter().enumerate() {
        let label = if CRITERIA.contains(&criterion.id.as_str()) {
            criterion.id.clone()
        } else {
            i.to_string()
        };
        let prefix = format!("criteria[{label}]");
        binding_unknowns(&mut check, &format!("{prefix}.binding"), &criterion.binding);
        for (j, evidence) in criterion.evidence.iter().enumerate() {
            check(format!("{prefix}.evidence[{j}].source"), &evidence.source);
            check(
                format!("{prefix}.evidence[{j}].observation"),
                &evidence.observation,
            );
        }
    }
    found.into_iter().collect()
}

fn binding_unknowns(check: &mut impl FnMut(String, &str), prefix: &str, b: &Binding) {
    for (name, value) in [
        ("run_id", &b.run_id),
        ("round_id", &b.round_id),
        ("platform", &b.platform),
        ("host", &b.host),
        ("repository", &b.repository),
        ("source_repository", &b.source_repository),
        ("pr_url", &b.pr_url),
        ("head_branch", &b.head_branch),
        ("base_branch", &b.base_branch),
        ("head_sha", &b.head_sha),
        ("base_sha", &b.base_sha),
        ("merge_base_sha", &b.merge_base_sha),
        ("policy_provenance", &b.policy_provenance),
    ] {
        check(format!("{prefix}.{name}"), value);
    }
}
