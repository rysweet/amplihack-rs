//! REST → GraphQL spellings (issue #2333, D4). Pure functions, so a REST read
//! and a GraphQL read of one PR give equal output apart from `source`.
//! Contract: docs/reference/autodrive-github-reads.md#how-rest-values-map-to-graphql-values.

use serde_json::Value;
use std::collections::BTreeMap;

/// `state` plus `merged_at` → `MERGED`, `CLOSED` or `OPEN`.
pub fn pr_state(state: &str, merged_at: &Value) -> &'static str {
    if !merged_at.is_null() {
        "MERGED"
    } else if state == "closed" {
        "CLOSED"
    } else {
        "OPEN"
    }
}

/// REST `mergeable` (true, false or null) → GraphQL `mergeable`.
pub fn mergeable(v: &Value) -> &'static str {
    match v.as_bool() {
        Some(true) => "MERGEABLE",
        Some(false) => "CONFLICTING",
        None => "UNKNOWN",
    }
}

/// REST `mergeable_state` → GraphQL `mergeStateStatus`. Anything unlisted,
/// including an upper-case spelling, is `UNKNOWN`.
pub fn merge_state(v: &str) -> &'static str {
    match v {
        "clean" => "CLEAN",
        "dirty" => "DIRTY",
        "behind" => "BEHIND",
        "blocked" => "BLOCKED",
        "unstable" => "UNSTABLE",
        "has_hooks" => "HAS_HOOKS",
        "draft" => "DRAFT",
        _ => "UNKNOWN",
    }
}

/// The latest `APPROVED`, `CHANGES_REQUESTED` or `DISMISSED` review per user,
/// in the order the reviews were given. `COMMENTED` and other states are
/// ignored. Reviews with no user login are ignored.
pub fn latest_per_user(reviews: &[Value]) -> BTreeMap<String, String> {
    let mut latest = BTreeMap::new();
    for r in reviews {
        let user = r["user"]["login"]
            .as_str()
            .or_else(|| r["author"]["login"].as_str());
        let state = r["state"].as_str().unwrap_or_default();
        if let Some(user) = user
            && matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED")
        {
            latest.insert(user.to_string(), state.to_string());
        }
    }
    latest
}

/// Counts of (approvals, changes requested) among each user's latest review.
pub fn review_counts(reviews: &[Value]) -> (u64, u64) {
    let latest = latest_per_user(reviews);
    let count = |s: &str| latest.values().filter(|v| v.as_str() == s).count() as u64;
    (count("APPROVED"), count("CHANGES_REQUESTED"))
}

/// GraphQL `reviewDecision` computed from `pulls/{n}/reviews`.
pub fn review_decision(reviews: &[Value], required: u64) -> &'static str {
    let (approvals, changes) = review_counts(reviews);
    if changes > 0 {
        "CHANGES_REQUESTED"
    } else if approvals >= required {
        "APPROVED"
    } else {
        "REVIEW_REQUIRED"
    }
}

/// A check run's bucket, as `gh pr checks` names it.
pub fn check_run_bucket(status: &str, conclusion: Option<&str>) -> &'static str {
    if status != "completed" {
        return "pending";
    }
    match conclusion {
        Some("success" | "neutral") => "pass",
        Some("skipped") => "skipping",
        _ => "fail",
    }
}

/// A legacy commit status context's bucket.
pub fn status_bucket(state: &str) -> &'static str {
    match state {
        "pending" => "pending",
        "success" => "pass",
        _ => "fail",
    }
}

/// A `gh pr checks` bucket, kept to the four the tools read. `cancel`
/// is a failure.
pub fn graphql_bucket(bucket: &str) -> &'static str {
    match bucket {
        "pass" => "pass",
        "pending" => "pending",
        "skipping" => "skipping",
        _ => "fail",
    }
}

/// A GraphQL `mergeStateStatus`, kept to the documented set.
pub fn graphql_merge_state(v: &str) -> &'static str {
    match v {
        "CLEAN" => "CLEAN",
        "DIRTY" => "DIRTY",
        "BEHIND" => "BEHIND",
        "BLOCKED" => "BLOCKED",
        "UNSTABLE" => "UNSTABLE",
        "HAS_HOOKS" => "HAS_HOOKS",
        "DRAFT" => "DRAFT",
        _ => "UNKNOWN",
    }
}

/// A GraphQL `mergeable`, kept to the documented set.
pub fn graphql_mergeable(v: &str) -> &'static str {
    match v {
        "MERGEABLE" => "MERGEABLE",
        "CONFLICTING" => "CONFLICTING",
        _ => "UNKNOWN",
    }
}

/// A GraphQL `state`, kept to the documented set.
pub fn graphql_state(v: &str) -> Option<&'static str> {
    match v {
        "OPEN" => Some("OPEN"),
        "CLOSED" => Some("CLOSED"),
        "MERGED" => Some("MERGED"),
        _ => None,
    }
}
