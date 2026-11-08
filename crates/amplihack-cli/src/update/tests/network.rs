use super::super::network::{
    bounded_status_message, github_error_message, http_get_bounded, is_retryable_error,
    parse_latest_release, validate_bounded_download_url, validate_download_url,
};
use super::super::*;
use std::time::Duration;

#[test]
fn is_retryable_error_classifies_status_codes() {
    for status in [500u16, 502, 503, 504] {
        let err = ureq::Error::Status(status, ureq::Response::new(status, "err", "").unwrap());
        assert!(is_retryable_error(&err), "{status} should be retryable");
    }
    let err = ureq::Error::Status(429, ureq::Response::new(429, "Rate Limited", "").unwrap());
    assert!(is_retryable_error(&err), "429 should be retryable");
    for status in [400u16, 401, 403, 404, 422] {
        let err = ureq::Error::Status(
            status,
            ureq::Response::new(status, "client err", "").unwrap(),
        );
        assert!(
            !is_retryable_error(&err),
            "{status} should NOT be retryable"
        );
    }
}

#[test]
fn github_error_message_contains_actionable_advice() {
    let msg_403 = github_error_message(403, "https://api.github.com/test");
    assert!(msg_403.contains("rate limit"));
    assert!(msg_403.contains("AMPLIHACK_NO_UPDATE_CHECK"));

    let msg_429 = github_error_message(429, "https://api.github.com/test");
    assert!(msg_429.contains("rate limit"));
}

#[test]
fn github_error_message_5xx_indicates_transient() {
    for status in [500u16, 502, 503, 504] {
        let msg = github_error_message(status, "https://api.github.com/test");
        assert!(
            msg.contains("Transient server error"),
            "expected transient-error message for {status}, got: {msg}"
        );
        assert!(
            msg.contains("retried automatically"),
            "expected retry hint for {status}, got: {msg}"
        );
    }
}

#[test]
fn validate_download_url_accepts_allowed_hosts() {
    assert!(validate_download_url("https://api.github.com/repos/x/y/releases/latest").is_ok());
    assert!(validate_download_url("https://github.com/x/y/releases/download/v1/x.tar.gz").is_ok());
    assert!(validate_download_url("https://objects.githubusercontent.com/x/y.tar.gz").is_ok());
}

#[test]
fn validate_download_url_rejects_disallowed_hosts() {
    assert!(validate_download_url("https://example.com/evil.tar.gz").is_err());
    assert!(
        validate_download_url("http://api.github.com/repos/x/y").is_err(),
        "http:// (not https://) must be rejected"
    );
    assert!(
        validate_download_url("https://attacker.com/https://api.github.com/").is_err(),
        "URL that contains but does not start with an allowed prefix must be rejected"
    );
    assert!(
        validate_download_url("").is_err(),
        "empty URL must be rejected"
    );
}

#[test]
fn normalize_tag_strips_v_prefix() {
    assert_eq!(normalize_tag("v1.2.3").unwrap(), "1.2.3");
}

#[test]
fn normalize_tag_rejects_non_semver() {
    assert!(normalize_tag("snapshot-abcdef").is_err());
}

#[test]
fn is_newer_detects_version_bumps() {
    assert!(is_newer("0.1.0", "0.2.0").unwrap());
    assert!(!is_newer("0.2.0", "0.2.0").unwrap());
    assert!(!is_newer("0.2.1", "0.2.0").unwrap());
}

#[test]
fn cache_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cache");
    write_cache(&path, "1.2.3").unwrap();
    let (version, timestamp) = read_cache(&path).unwrap();
    assert_eq!(version, "1.2.3");
    assert!(timestamp > 0);
}

#[test]
fn cache_path_uses_home() {
    let temp = tempfile::tempdir().unwrap();
    let path = cache_path_from_home(temp.path());
    assert_eq!(
        path,
        temp.path().join(".config/amplihack/last_update_check")
    );
}

#[test]
fn parse_latest_release_selects_matching_asset() {
    let archive_name = expected_archive_name().unwrap();
    let checksum_name = format!("{archive_name}.sha256");
    let json = format!(
        r#"{{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {{"name": "wrong.tar.gz", "browser_download_url": "https://example.invalid/wrong"}},
                {{"name": "{archive_name}", "browser_download_url": "https://example.invalid/right"}},
                {{"name": "{checksum_name}", "browser_download_url": "https://example.invalid/right.sha256"}}
            ]
        }}"#
    );
    let release =
        parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
    assert_eq!(
        release,
        UpdateRelease {
            version: "0.2.0".to_string(),
            asset_url: "https://example.invalid/right".to_string(),
            checksum_url: Some("https://example.invalid/right.sha256".to_string()),
        }
    );
}

#[test]
fn parse_latest_release_no_checksum_asset() {
    let archive_name = expected_archive_name().unwrap();
    let json = format!(
        r#"{{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {{"name": "{archive_name}", "browser_download_url": "https://example.invalid/right"}}
            ]
        }}"#
    );
    let release =
        parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
    assert_eq!(release.checksum_url, None);
}

#[test]
fn fake_latest_version_env_returns_synthetic_release() {
    // We cannot directly call fetch_latest_release() because it is
    // pub(super) in a sibling module; the existing tests pattern (see
    // `parse_latest_release_selects_matching_asset`) tests at the parsing
    // layer. Here we assert the source-level invariant: the
    // synthetic-version short-circuit is wired into network.rs.
    let src = include_str!("network.rs");
    assert!(
        src.contains("AMPLIHACK_TEST_FAKE_LATEST_VERSION"),
        "fetch_latest_release MUST honor AMPLIHACK_TEST_FAKE_LATEST_VERSION; \
         the env-var name is missing from network.rs"
    );
    assert!(
        src.contains("is_empty"),
        "AMPLIHACK_TEST_FAKE_LATEST_VERSION MUST be guarded by !is_empty() so an \
         exported-but-empty env value falls through to the production path"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #1955 (SEC-1): the tag tarball download.
//
// GitHub redirects a tag tarball to codeload.github.com, so the allow-list of
// the bounded download has to name it, and that list has to hold for the final
// URL too. No test here reaches the network: each URL is refused first.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn bounded_download_allows_the_hosts_a_tag_tarball_goes_through() {
    for url in [
        "https://github.com/rysweet/amplihack-rs/archive/refs/tags/v1.2.99.tar.gz",
        "https://codeload.github.com/rysweet/amplihack-rs/tar.gz/refs/tags/v1.2.99",
    ] {
        validate_bounded_download_url(url).unwrap_or_else(|e| panic!("{url}: {e:#}"));
    }
}

#[test]
fn bounded_download_refuses_other_hosts_and_look_alikes() {
    for url in [
        "http://github.com/rysweet/amplihack-rs/archive/refs/tags/v1.2.99.tar.gz",
        "http://codeload.github.com/x.tar.gz",
        "https://codeload.github.com.evil.example/x.tar.gz",
        "https://github.com.evil.example/x.tar.gz",
        "https://github.com@evil.example/x.tar.gz",
        "https://evil.example/https://codeload.github.com/x.tar.gz",
        "",
    ] {
        assert!(
            validate_bounded_download_url(url).is_err(),
            "{url:?} must be refused"
        );
    }
}

#[test]
fn codeload_is_allowed_for_the_bounded_download_only() {
    assert!(
        validate_download_url("https://codeload.github.com/x.tar.gz").is_err(),
        "binary downloads must keep their shorter allow-list"
    );
}

#[test]
fn http_get_bounded_refuses_a_bad_url_without_a_request() {
    for url in [
        "http://github.com/x.tar.gz",
        "https://evil.example/x.tar.gz",
    ] {
        let err = http_get_bounded(url, Duration::from_secs(5))
            .expect_err("a URL outside the allow-list must be refused");
        assert!(
            format!("{err:#}").contains("allowed"),
            "the error must say the host isn't allowed: {err:#}"
        );
    }
}

#[test]
fn http_get_bounded_fails_at_once_when_the_budget_is_spent() {
    let started = std::time::Instant::now();
    let err = http_get_bounded(
        "https://github.com/rysweet/amplihack-rs/archive/refs/tags/v1.2.99.tar.gz",
        Duration::ZERO,
    )
    .expect_err("a spent budget must fail");
    assert!(started.elapsed() < Duration::from_secs(1), "{err:#}");
}

/// The update check's messages suggest `AMPLIHACK_NO_UPDATE_CHECK` and promise a
/// retry, and neither is true of a failure that reaches an install's warning.
#[test]
fn a_tag_download_status_message_gives_no_update_check_advice() {
    let url = "https://github.com/rysweet/amplihack-rs/archive/refs/tags/v1.2.99.tar.gz";
    for status in [403, 404, 429, 503] {
        assert_eq!(
            bounded_status_message(status, url),
            format!("HTTP {status} from {url}")
        );
    }
}

/// A redirect can't be driven without the network, so this pins the two calls
/// that make the final-URL rule hold. Enforcing the final host without adding
/// codeload.github.com would break every tarball fetch; the tests above cover
/// the list, and this covers its use on the final URL.
#[test]
fn http_get_bounded_enforces_https_and_checks_the_final_url() {
    let src = include_str!("../network.rs");
    let body = src
        .split_once("fn http_get_bounded(")
        .map(|(_, rest)| rest)
        .expect("http_get_bounded is in network.rs");
    assert!(
        src.contains(".https_only(true)"),
        "the bounded download must refuse an HTTP redirect"
    );
    assert!(
        body.contains(".get_url()"),
        "the bounded download must check the final URL before reading the body"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #625: skip-line literal contract.
//
// The skip-line wording is part of the contract — delegated agents grep
// for it. Guard against accidental rewording.
// ─────────────────────────────────────────────────────────────────────────────
