//! Issue #3286: a failed trusted-bundle check is classed by its type as
//! `environmental` and not retryable, rather than left `unknown`.
//!
//! A clone made under umask 0002 leaves its bundle group-writable. The check
//! then fails the same way on every attempt, so a supervising loop must be
//! able to tell it from a transient fault. The class is decided by the
//! error's type, so text that merely reads the same is not classed this way.

use super::failure_class::{
    FAILURE_CLASS_MARKER_PREFIX, FailureClass, UNTRUSTED_BUNDLE_SIGNAL, classify_error, marker_line,
};
use crate::commands::orch::autodrive::trusted::pin_bundle;
use serde_json::Value as JsonValue;
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// The error `pin_bundle` returns for a group-writable bundle, wrapped in a
/// context layer as the launch path may wrap it.
fn group_writable_bundle_error() -> (tempfile::TempDir, anyhow::Error) {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = tmp.path().join("amplifier-bundle");
    fs::create_dir_all(&bundle).unwrap();
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o775)).unwrap();
    let error = pin_bundle(tmp.path())
        .expect_err("a group-writable bundle fails the trust check")
        .context("the auto-drive launch failed");
    (tmp, error)
}

#[test]
fn untrusted_bundle_is_environmental_and_not_retryable() {
    let (_tmp, error) = group_writable_bundle_error();
    let verdict = classify_error(&error);
    assert_eq!(verdict.class, FailureClass::Environmental);
    assert!(!verdict.class.is_mechanically_retryable());
    assert!(!verdict.is_structural_refusal());
}

#[test]
fn untrusted_bundle_marker_says_not_retryable() {
    let (_tmp, error) = group_writable_bundle_error();
    let line = marker_line(&classify_error(&error).to_json(1, "terminal"));
    let payload: JsonValue = serde_json::from_str(
        line.strip_prefix(FAILURE_CLASS_MARKER_PREFIX)
            .expect("the marker starts with its prefix"),
    )
    .expect("the marker carries one JSON object");
    assert_eq!(payload["class"], "environmental");
    assert_eq!(payload["retryable"], false);
    assert_eq!(payload["structural_refusal"], false);
}

#[test]
fn text_alone_does_not_class_a_failure_as_an_untrusted_bundle() {
    let (_tmp, error) = group_writable_bundle_error();
    let copy = anyhow::anyhow!("{error:#}");
    assert_ne!(classify_error(&copy).signal, Some(UNTRUSTED_BUNDLE_SIGNAL));
    assert_eq!(classify_error(&error).signal, Some(UNTRUSTED_BUNDLE_SIGNAL));
}
