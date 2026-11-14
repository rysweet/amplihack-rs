//! Rate-limit classing and the reset time (issue #2333, D3).
//!
//! `classify` is a copy of `classify_gh_error` in
//! `amplifier-bundle/tools/workflow_gh_retry.sh`: the same rules in the same
//! order, matched per line and ignoring case. A unit test runs both on the
//! same samples. Nothing here sleeps or reads the clock; `now` is passed in.

use regex::Regex;
use std::sync::LazyLock;

/// One day: a reset further than this from now is treated as unknown.
const DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Auth,
    RateLimit,
    Transient,
    Other,
}

fn re(p: &str) -> Regex {
    Regex::new(&format!("(?im){p}")).expect("static pattern")
}

static AUTH: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"bad credentials|HTTP 401|(^|[^0-9])401([^0-9]|$)|unauthorized|authentication failed|requires authentication|gh auth login|not logged in|token .*(expired|invalid)|invalid .*token",
    )
});
static LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"api rate limit exceeded|rate limit already exceeded|secondary rate limit|abuse detection|retry-after|x-ratelimit-reset|rate limit|HTTP 429|(^|[^0-9])429([^0-9]|$)",
    )
});
static FORBIDDEN: LazyLock<Regex> = LazyLock::new(|| re(r"HTTP 403|(^|[^0-9])403([^0-9]|$)"));
static PERMISSION: LazyLock<Regex> = LazyLock::new(|| {
    re(r"resource not accessible|must have admin|not authorized|insufficient|permission")
});
static TRANSIENT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"HTTP 5[0-9][0-9]|(^|[^0-9])(500|502|503|504)([^0-9]|$)|timed out|timeout|temporar|connection reset|connection refused|TLS handshake|network|server error|i/o timeout|no such host|EOF",
    )
});
static RETRY_AFTER: LazyLock<Regex> = LazyLock::new(|| re(r"retry-after:?\s*([0-9]{1,12})"));
static RESET: LazyLock<Regex> = LazyLock::new(|| re(r"x-ratelimit-reset:?\s*([0-9]{1,12})"));

/// The class of one failed call's stderr; the first rule that matches wins.
pub fn classify(text: &str) -> Class {
    if text.is_empty() {
        return Class::Other;
    }
    if AUTH.is_match(text) {
        Class::Auth
    } else if LIMIT.is_match(text) {
        Class::RateLimit
    } else if FORBIDDEN.is_match(text) {
        if PERMISSION.is_match(text) {
            Class::Other
        } else {
            Class::RateLimit
        }
    } else if TRANSIENT.is_match(text) {
        Class::Transient
    } else {
        Class::Other
    }
}

/// `epoch` when it is within one day of `now`, else unknown.
pub fn bounded(epoch: i64, now: i64) -> Option<i64> {
    ((epoch - now).abs() <= DAY).then_some(epoch)
}

/// The reset from a `Retry-After` (seconds from now) or `X-RateLimit-Reset`
/// (epoch) value in a refused call's text. Only the number is kept.
pub fn reset_from_text(text: &str, now: i64) -> Option<i64> {
    let num = |r: &Regex| {
        r.captures(text)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<i64>().ok())
    };
    if let Some(secs) = num(&RETRY_AFTER) {
        return bounded(now.saturating_add(secs), now);
    }
    num(&RESET).and_then(|e| bounded(e, now))
}

static NOT_FOUND: LazyLock<Regex> = LazyLock::new(|| {
    re(r"HTTP 404|(^|[^0-9])404([^0-9]|$)|not found|could not resolve to a|no pull requests? found")
});

/// True when a failed call will not clear by waiting: bad or missing
/// credentials, a refused permission, or a repository or pull request that
/// does not exist. A rate limit or a transient fault is never a refusal.
pub fn is_refusal(text: &str) -> bool {
    match classify(text) {
        Class::Auth => true,
        Class::RateLimit | Class::Transient => false,
        Class::Other => {
            (FORBIDDEN.is_match(text) && PERMISSION.is_match(text)) || NOT_FOUND.is_match(text)
        }
    }
}

static REQUEST_URL: LazyLock<Regex> = LazyLock::new(|| re(r"\(?https?://[^\s)]*\)?"));
static GRAPHQL_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"number of [0-9]+"));

/// `text` without the parts gh copies from the request: the URL it appends
/// (`HTTP 404: Not Found (https://api.github.com/repos/o/r/pulls/429)`) and
/// GraphQL's `number of <n>`. A PR number or a SHA there must not match the
/// bare 401, 403 or 429 rules. Used before [`classify`] and [`is_refusal`];
/// [`reset_from_text`] still reads the whole text.
pub fn without_request(text: &str) -> String {
    let t = REQUEST_URL.replace_all(text, "");
    GRAPHQL_NUMBER.replace_all(&t, "number").into_owned()
}
