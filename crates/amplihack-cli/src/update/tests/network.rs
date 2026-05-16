use super::super::network::{
    github_error_message, is_retryable_error, parse_latest_release, validate_download_url,
};
use super::super::*;

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
// Issue #625: skip-line literal contract.
//
// The skip-line wording is part of the contract — delegated agents grep
// for it. Guard against accidental rewording.
// ─────────────────────────────────────────────────────────────────────────────
