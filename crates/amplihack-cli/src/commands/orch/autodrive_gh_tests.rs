//! Unit tests for `autodrive_gh` (issue #2333). Included from
//! `autodrive_gh.rs` with:
//!
//! ```ignore
//! #[cfg(test)]
//! #[path = "autodrive_gh_tests.rs"]
//! mod tests;
//! ```
//!
//! They pin the pure parts: the D4 REST → GraphQL mapping, rate-limit
//! classing (the same answer as `classify_gh_error` in
//! `amplifier-bundle/tools/workflow_gh_retry.sh` on every sample), the reset
//! bounds with an injected `now`, argument validation and excerpt redaction.

use super::ratelimit::{self, Class};
use super::rest_map;
use super::{excerpt, valid_branch, valid_pr, valid_repo, valid_sha};
use serde_json::{Value, json};

const NOW: i64 = 1_794_500_000;

// ── D4: head, base and state ─────────────────────────────────────────────────

#[test]
fn pr_state_maps_merged_closed_and_open() {
    assert_eq!(
        rest_map::pr_state("closed", &json!("2026-11-12T00:00:00Z")),
        "MERGED"
    );
    assert_eq!(
        rest_map::pr_state("open", &json!("2026-11-12T00:00:00Z")),
        "MERGED"
    );
    assert_eq!(rest_map::pr_state("closed", &Value::Null), "CLOSED");
    assert_eq!(rest_map::pr_state("open", &Value::Null), "OPEN");
}

#[test]
fn mergeable_maps_true_false_and_null() {
    assert_eq!(rest_map::mergeable(&json!(true)), "MERGEABLE");
    assert_eq!(rest_map::mergeable(&json!(false)), "CONFLICTING");
    assert_eq!(rest_map::mergeable(&Value::Null), "UNKNOWN");
    assert_eq!(rest_map::mergeable(&json!("yes")), "UNKNOWN");
}

#[test]
fn merge_state_maps_every_row_and_defaults_to_unknown() {
    for (rest, gql) in [
        ("clean", "CLEAN"),
        ("dirty", "DIRTY"),
        ("behind", "BEHIND"),
        ("blocked", "BLOCKED"),
        ("unstable", "UNSTABLE"),
        ("has_hooks", "HAS_HOOKS"),
        ("draft", "DRAFT"),
        ("unknown", "UNKNOWN"),
        ("", "UNKNOWN"),
        ("CLEAN", "UNKNOWN"),
        ("brand_new_state", "UNKNOWN"),
    ] {
        assert_eq!(rest_map::merge_state(rest), gql, "{rest}");
    }
}

// ── D4: review decision ──────────────────────────────────────────────────────

fn review(user: &str, state: &str) -> Value {
    json!({"user": {"login": user}, "state": state})
}

#[test]
fn review_counts_use_each_users_latest_counting_review() {
    let r = [
        review("a", "APPROVED"),
        review("a", "COMMENTED"), // ignored: a stays APPROVED
        review("b", "CHANGES_REQUESTED"),
        review("b", "DISMISSED"), // b's latest counting review
    ];
    assert_eq!(rest_map::review_counts(&r), (1, 0));
}

#[test]
fn a_latest_changes_requested_is_counted() {
    let r = [
        review("a", "APPROVED"),
        review("b", "APPROVED"),
        review("c", "CHANGES_REQUESTED"),
    ];
    assert_eq!(rest_map::review_counts(&r), (2, 1));
}

#[test]
fn a_later_approval_clears_the_same_users_changes_requested() {
    let r = [review("a", "CHANGES_REQUESTED"), review("a", "APPROVED")];
    assert_eq!(rest_map::review_counts(&r), (1, 0));
}

#[test]
fn no_reviews_or_only_comments_count_nothing() {
    assert_eq!(rest_map::review_counts(&[]), (0, 0));
    assert_eq!(rest_map::review_counts(&[review("a", "COMMENTED")]), (0, 0));
}

#[test]
fn one_user_approving_twice_counts_once() {
    let r = [review("a", "APPROVED"), review("a", "APPROVED")];
    assert_eq!(rest_map::review_counts(&r), (1, 0));
}

// ── D4: check buckets ────────────────────────────────────────────────────────

#[test]
fn check_runs_map_to_buckets() {
    for (status, conclusion, bucket) in [
        ("queued", None, "pending"),
        ("in_progress", None, "pending"),
        ("completed", Some("success"), "pass"),
        ("completed", Some("neutral"), "pass"),
        ("completed", Some("skipped"), "skipping"),
        ("completed", Some("failure"), "fail"),
        ("completed", Some("cancelled"), "fail"),
        ("completed", Some("timed_out"), "fail"),
        ("completed", Some("action_required"), "fail"),
        ("completed", Some("stale"), "fail"),
        ("completed", Some("something_new"), "fail"),
    ] {
        assert_eq!(
            rest_map::check_run_bucket(status, conclusion),
            bucket,
            "{status}/{conclusion:?}"
        );
    }
}

#[test]
fn legacy_status_contexts_map_to_buckets() {
    for (state, bucket) in [
        ("pending", "pending"),
        ("success", "pass"),
        ("failure", "fail"),
        ("error", "fail"),
    ] {
        assert_eq!(rest_map::status_bucket(state), bucket, "{state}");
    }
}

// ── Rate-limit classing: identical to classify_gh_error ─────────────────────

const SAMPLES: &[(&str, Class)] = &[
    ("HTTP 401: Bad credentials", Class::Auth),
    (
        "error: not logged in to any hosts; run gh auth login",
        Class::Auth,
    ),
    ("token has expired", Class::Auth),
    ("API rate limit exceeded for user", Class::RateLimit),
    (
        "GraphQL: You have exceeded a secondary rate limit.",
        Class::RateLimit,
    ),
    ("abuse detection mechanism triggered", Class::RateLimit),
    ("Retry-After: 60", Class::RateLimit),
    ("x-ratelimit-reset: 1794528000", Class::RateLimit),
    ("rate limit", Class::RateLimit),
    ("HTTP 429: Too Many Requests", Class::RateLimit),
    ("got 429 from server", Class::RateLimit),
    ("HTTP 403: Forbidden", Class::RateLimit),
    (
        "HTTP 403: Resource not accessible by integration",
        Class::Other,
    ),
    ("HTTP 403: must have admin rights", Class::Other),
    ("HTTP 502: Bad Gateway", Class::Transient),
    ("connection reset by peer", Class::Transient),
    ("i/o timeout", Class::Transient),
    ("unexpected EOF", Class::Transient),
    ("was submitted too quickly", Class::Other),
    ("HTTP 404: Not Found", Class::Other),
    ("port 4290 refused nothing", Class::Other),
    ("", Class::Other),
];

#[test]
fn classify_matches_every_sample() {
    for (text, class) in SAMPLES {
        assert_eq!(ratelimit::classify(text), *class, "{text:?}");
    }
}

#[test]
fn classify_agrees_with_the_shell_classifier() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/workflow_gh_retry.sh");
    let dir = tempfile::tempdir().unwrap();
    for (i, (text, _)) in SAMPLES.iter().enumerate() {
        let f = dir.path().join(format!("s{i}"));
        std::fs::write(&f, text).unwrap();
        let o = std::process::Command::new("bash")
            .args([
                "-c",
                "set +eu; . \"$1\" >/dev/null 2>&1; classify_gh_error \"$2\"",
            ])
            .arg("classify")
            .arg(&script)
            .arg(&f)
            .output()
            .unwrap();
        let shell = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let rust = match ratelimit::classify(text) {
            Class::Auth => "auth",
            Class::RateLimit => "rate_limit",
            Class::Transient => "transient",
            Class::Other => "other",
        };
        assert_eq!(rust, shell, "{text:?}");
    }
}

// ── Reset epoch ──────────────────────────────────────────────────────────────

#[test]
fn reset_comes_from_retry_after_or_x_ratelimit_reset() {
    assert_eq!(
        ratelimit::reset_from_text("Retry-After: 120", NOW),
        Some(NOW + 120)
    );
    assert_eq!(
        ratelimit::reset_from_text(&format!("X-RateLimit-Reset: {}", NOW + 900), NOW),
        Some(NOW + 900)
    );
    assert_eq!(
        ratelimit::reset_from_text("secondary rate limit", NOW),
        None
    );
}

#[test]
fn reset_outside_one_day_of_now_is_unknown() {
    assert_eq!(ratelimit::bounded(NOW + 86_400, NOW), Some(NOW + 86_400));
    assert_eq!(ratelimit::bounded(NOW + 86_401, NOW), None);
    assert_eq!(ratelimit::bounded(NOW - 86_401, NOW), None);
    assert_eq!(ratelimit::bounded(0, NOW), None);
    assert_eq!(
        ratelimit::reset_from_text("Retry-After: 99999999", NOW),
        None,
        "an absurd Retry-After is unknown"
    );
}

// ── Argument validation ──────────────────────────────────────────────────────

#[test]
fn pr_numbers_are_1_to_9999999999() {
    assert_eq!(valid_pr("1"), Some(1));
    assert_eq!(valid_pr("9999999999"), Some(9_999_999_999));
    for bad in ["0", "", "-1", "+1", "01", "1a", "10000000000", " 7", "7\n"] {
        assert_eq!(valid_pr(bad), None, "{bad:?}");
    }
}

#[test]
fn shas_are_40_lowercase_hex() {
    assert!(valid_sha(&"a".repeat(40)));
    for bad in ["", "abc", &"A".repeat(40), &"g".repeat(40), &"a".repeat(41)] {
        assert!(!valid_sha(bad), "{bad:?}");
    }
}

#[test]
fn branches_follow_the_branch_rules() {
    for ok in ["main", "feat/issue-2333-x", "quick/demo", "release-1.2"] {
        assert!(valid_branch(ok), "{ok}");
    }
    for bad in [
        "",
        "-x",
        "--upload-pack=x",
        "a..b",
        "a b",
        "a~1",
        "a^",
        "a:b",
        "a?",
        "a*",
        "a[",
        "a\\b",
        "/a",
        "a/",
        "a.lock",
        "a//b",
        "@",
        "a@{b",
        "a\nb",
    ] {
        assert!(!valid_branch(bad), "{bad:?}");
    }
}

#[test]
fn repos_are_owner_slash_name() {
    assert!(valid_repo("o/r"));
    assert!(valid_repo("my-org/my.repo_1"));
    for bad in ["", "o", "o/r/x", "../r", "o/..", "-o/r", "o r/x", "o/r\n"] {
        assert!(!valid_repo(bad), "{bad:?}");
    }
}

// ── Excerpts ─────────────────────────────────────────────────────────────────

#[test]
fn excerpt_keeps_one_short_printable_line() {
    let e = excerpt("first line\nsecond line");
    assert_eq!(e, "first line");
    let e = excerpt(&format!("{}\u{7}é", "x".repeat(500)));
    assert!(e.len() <= 200, "{}", e.len());
    assert!(e.bytes().all(|b| (0x20..0x7f).contains(&b)), "{e:?}");
}

#[test]
fn excerpt_redacts_token_values() {
    for secret in [
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        "github_pat_11ABCDEFG_abcdefghijklmnop",
        "Authorization: token sekrit123",
        "Bearer sekrit456",
    ] {
        let e = excerpt(&format!("HTTP 401: {secret} rejected"));
        for part in [
            "abcdefghijklmnopqrstuvwxyz0123456789",
            "11ABCDEFG_abcdefghijklmnop",
            "sekrit123",
            "sekrit456",
        ] {
            assert!(!e.contains(part), "{secret}: {e}");
        }
    }
}

#[test]
fn refusals_are_failures_that_waiting_will_not_clear() {
    for text in [
        "HTTP 401: Bad credentials (https://api.github.com/graphql)",
        "To get started with GitHub CLI, please run:  gh auth login",
        "HTTP 403: Resource not accessible by integration",
        "HTTP 404: Not Found (https://api.github.com/repos/o/r/pulls/7)",
        "GraphQL: Could not resolve to a Repository with the name 'o/r'.",
        "exit 128: remote: Repository not found.",
    ] {
        assert!(ratelimit::is_refusal(text), "{text:?}");
    }
    for text in [
        "GraphQL: You have exceeded a secondary rate limit.",
        "HTTP 403: API rate limit exceeded",
        "HTTP 403: Forbidden",
        "HTTP 502: Bad Gateway",
        "answer is not a list",
        "exit 128: fatal: couldn't find remote ref refs/pull/7/head",
        "",
    ] {
        assert!(!ratelimit::is_refusal(text), "{text:?}");
    }
}
