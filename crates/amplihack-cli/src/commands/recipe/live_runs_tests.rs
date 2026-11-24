//! Issue #3161: the pull request fields in a session pointer, and the shared
//! resolver that turns them into one pull request.
//!
//! Contract:
//! - `guide_records::Pointer` gains `pr_url` and `pr_number`, both
//!   `Option<String>`, read from the pointer line as `run_id` is;
//! - `crate::run_guidance::env::pr_ref_from_context(&BTreeMap<String,String>,
//!   &Path) -> Option<PrRef>` is the one resolver. Owner and repo compare
//!   case-insensitively in `PrRef::same_pr`. A number-only context resolves
//!   against the origin of the working folder; with none, it is `None`.
//!
//! Liveness, reachability and the refusal are covered through the real binary
//! in `tests/issue_3161_pr_url_live_run.rs`, where a process record and a
//! start time exist.

use super::guide_records::{LOG_POINTER_PREFIX, scan};
use crate::run_guidance::env::pr_ref_from_context;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn ctx(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn private_log_dir(root: &Path) -> std::path::PathBuf {
    let dir = root.join("recipe-logs");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn write_log(dir: &Path, session: &str, pointer: serde_json::Value) {
    let path = dir.join(format!("{session}.log"));
    fs::write(&path, format!("{LOG_POINTER_PREFIX}{pointer}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_pointer_carries_the_pull_request_url_and_number() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_log_dir(&tmp.path().canonicalize().unwrap());
    write_log(
        &dir,
        "with-pr",
        serde_json::json!({
            "run_id": "c-1",
            "pr_url": "https://github.com/o/r/pull/7",
            "pr_number": "7",
        }),
    );
    write_log(&dir, "without-pr", serde_json::json!({"run_id": "c-2"}));
    let scanned = scan(&dir);
    let by_run = |id: &str| {
        scanned
            .records
            .iter()
            .find(|p| p.run_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no record for {id}"))
    };
    assert_eq!(
        by_run("c-1").pr_url.as_deref(),
        Some("https://github.com/o/r/pull/7")
    );
    assert_eq!(by_run("c-1").pr_number.as_deref(), Some("7"));
    // A pointer from 1.2.215 has neither, and is still read.
    assert_eq!(by_run("c-2").pr_url, None);
    assert_eq!(by_run("c-2").pr_number, None);
}

#[test]
fn the_resolver_takes_a_valid_url_and_refuses_an_invalid_one() {
    let wd = tempfile::tempdir().unwrap();
    let pr = pr_ref_from_context(
        &ctx(&[("pr_url", "https://github.com/o/r/pull/7")]),
        wd.path(),
    )
    .expect("a valid URL resolves");
    assert_eq!(
        (pr.owner.as_str(), pr.repo.as_str(), pr.number),
        ("o", "r", 7)
    );
    for bad in [
        "https://example.com/o/r/pull/7",
        "https://github.com/o/r/pull/0",
        "https://github.com/o/r/issues/7",
        "not a url",
    ] {
        assert!(
            pr_ref_from_context(&ctx(&[("pr_url", bad)]), wd.path()).is_none(),
            "{bad}"
        );
    }
}

#[test]
fn a_bare_number_with_no_origin_does_not_match_any_pull_request() {
    let wd = tempfile::tempdir().unwrap();
    assert!(pr_ref_from_context(&ctx(&[("pr_number", "7")]), wd.path()).is_none());
}

#[test]
fn a_context_with_no_pull_request_resolves_to_none() {
    let wd = tempfile::tempdir().unwrap();
    assert!(pr_ref_from_context(&ctx(&[("task", "x")]), wd.path()).is_none());
}
