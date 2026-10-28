//! Issue #1710 (D3): the durable `<session>.run.json` a detached run leaves
//! in the log directory.
//!
//! Contract (`run_record`):
//! - `RunRecord { version, session, parent, started_at, recipe, issue_number,
//!   pr_number, repo, cwd, outcome }` and `Outcome { exit_code, class,
//!   finished_at }`, serialised with exactly those keys (no `run_id`).
//! - `build(session, parent, started_at, recipe, context, repo, cwd)` keeps
//!   only the numeric `issue_number` and `pr_number` context values.
//! - `record_path(log_dir, session)` is `<log_dir>/<session>.run.json`.
//! - `write_new` creates the file 0600, exclusively, never through a link.
//! - `write_outcome` writes `<path>.tmp.<pid>`, syncs and renames it over the
//!   record, keeping 0600; no temporary file is left behind.
//! - `read` returns `None` unless the file is a regular file owned by this
//!   user with no group or other bits, parses, and every field passes
//!   validation.
//! - `classify_tail(tail, exit_code)`: with `Some(code)`, the class of the
//!   last valid marker line, else `ok` for 0, else the failure-text class for
//!   a non-zero code, else `error`. With `None` (a run without a record),
//!   only a valid marker line gives a class.
//! - `repo_slug(origin_url)` is the GitHub `owner/name`, never the URL.
//! - `origin_repo(dir, timeout)` reads `origin` with a bounded git call.

use super::run::{FAILURE_CLASS_MARKER_PREFIX, FailureClass};
use super::run_record::{
    Outcome, RunRecord, build, classify_tail, origin_repo, read, read_tail, record_path, repo_slug,
    write_new, write_outcome,
};
use chrono::{TimeZone, Utc};
use serde_json::{Map, Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SESSION: &str = "amp-default-workflow-myapp-20261028-101500";
const USAGE_LIMIT_TEXT: &str = "Agent terminated early due to an API error: You've hit your session limit \u{b7} resets 9:20am (UTC)";

fn context(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect()
}

fn sample(cwd: &Path) -> RunRecord {
    build(
        SESSION,
        Some("session:rs-1760954100-48211".into()),
        Utc.with_ymd_and_hms(2026, 10, 28, 10, 15, 0).unwrap(),
        "default-workflow",
        &context(&[
            ("issue_number", "1710"),
            ("pr_number", "1715"),
            (
                "task_description",
                "SECRET TASK TEXT with a token ghp_abc123",
            ),
            ("repo_path", "/home/alice/src/myapp"),
        ]),
        Some("alice/myapp".into()),
        cwd,
    )
}

fn marker(class: &str) -> String {
    format!(
        "{FAILURE_CLASS_MARKER_PREFIX}{}",
        json!({"schema_version": 1, "class": class, "action": "terminal"})
    )
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

#[test]
fn build_keeps_only_the_two_numbers_from_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let rec = sample(dir.path());
    assert_eq!(rec.version, 1);
    assert_eq!(rec.session, SESSION);
    assert_eq!(rec.parent.as_deref(), Some("session:rs-1760954100-48211"));
    assert_eq!(rec.started_at, "2026-10-28T10:15:00Z");
    assert_eq!(rec.recipe, "default-workflow");
    assert_eq!(rec.issue_number, Some(1710));
    assert_eq!(rec.pr_number, Some(1715));
    assert_eq!(rec.repo.as_deref(), Some("alice/myapp"));
    assert_eq!(rec.cwd, dir.path());
    assert!(rec.outcome.is_none());

    let text = serde_json::to_string(&rec).unwrap();
    for secret in ["SECRET", "ghp_abc123", "task_description", "repo_path"] {
        assert!(!text.contains(secret), "{secret} leaked into {text}");
    }
}

#[test]
fn the_serialised_record_has_exactly_the_documented_keys() {
    let dir = tempfile::tempdir().unwrap();
    let value = serde_json::to_value(sample(dir.path())).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "cwd",
            "issue_number",
            "outcome",
            "parent",
            "pr_number",
            "recipe",
            "repo",
            "session",
            "started_at",
            "version"
        ]
    );
    assert!(value["outcome"].is_null());
}

#[test]
fn non_numeric_issue_and_pr_values_become_null() {
    let dir = tempfile::tempdir().unwrap();
    for bad in [
        "abc",
        "",
        "12; rm -rf /",
        "-3",
        "1.5",
        " 12",
        "99999999999999999999999",
    ] {
        let rec = build(
            SESSION,
            None,
            Utc::now(),
            "r",
            &context(&[("issue_number", bad), ("pr_number", bad)]),
            None,
            dir.path(),
        );
        assert_eq!(rec.issue_number, None, "{bad:?}");
        assert_eq!(rec.pr_number, None, "{bad:?}");
    }
}

#[test]
fn record_path_sits_beside_the_log() {
    assert_eq!(
        record_path(Path::new("/logs"), SESSION),
        PathBuf::from(format!("/logs/{SESSION}.run.json"))
    );
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[test]
fn write_new_creates_a_0600_file_that_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = record_path(dir.path(), SESSION);
    let rec = sample(dir.path());
    write_new(&path, &rec).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(read(&path), Some(rec));
}

#[test]
fn write_new_never_replaces_an_existing_file_or_follows_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = record_path(dir.path(), SESSION);
    fs::write(&path, "old").unwrap();
    assert!(write_new(&path, &sample(dir.path())).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "old");

    let target = dir.path().join("target");
    let link = record_path(dir.path(), "linked");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(write_new(&link, &sample(dir.path())).is_err());
    assert!(!target.exists());
}

#[test]
fn write_outcome_replaces_atomically_and_keeps_the_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = record_path(dir.path(), SESSION);
    let rec = sample(dir.path());
    write_new(&path, &rec).unwrap();
    let outcome = Outcome {
        exit_code: 0,
        class: "ok".into(),
        finished_at: "2026-10-28T12:20:41Z".into(),
    };
    write_outcome(&path, outcome.clone()).unwrap();

    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let back = read(&path).expect("record reads back");
    assert_eq!(back.outcome, Some(outcome));
    assert_eq!(back.session, rec.session);
    assert_eq!(back.started_at, rec.started_at);
    let leftovers: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn write_outcome_refuses_a_record_it_cannot_trust() {
    let dir = tempfile::tempdir().unwrap();
    let path = record_path(dir.path(), SESSION);
    write_new(&path, &sample(dir.path())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let outcome = Outcome {
        exit_code: 1,
        class: "error".into(),
        finished_at: "2026-10-28T12:00:00Z".into(),
    };
    assert!(write_outcome(&path, outcome.clone()).is_err());
    assert!(write_outcome(&dir.path().join("missing.run.json"), outcome).is_err());
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn read_refuses_loose_modes_and_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let path = record_path(dir.path(), SESSION);
    write_new(&path, &sample(dir.path())).unwrap();
    for mode in [0o640, 0o604, 0o660] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(read(&path), None, "mode {mode:o}");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let link = record_path(dir.path(), "link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert_eq!(read(&link), None);
    assert!(read(&path).is_some());
}

/// Writes `value` as a 0600 record and reads it back.
fn read_value(dir: &Path, value: &Value) -> Option<RunRecord> {
    let path = dir.join("x.run.json");
    let _ = fs::remove_file(&path);
    fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    read(&path)
}

#[test]
fn read_revalidates_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let good = serde_json::to_value(sample(dir.path())).unwrap();
    assert!(read_value(dir.path(), &good).is_some());

    let mutations: [(&str, Value); 8] = [
        ("session", json!("bad session; $(x)")),
        ("session", json!("x".repeat(200))),
        ("repo", json!("https://user:tok@github.com/o/n")),
        ("repo", json!("o/n/extra")),
        ("cwd", json!("relative/dir")),
        ("parent", json!("p".repeat(300))),
        ("parent", json!("run:\u{1b}[31mred")),
        ("version", json!(2)),
    ];
    for (key, bad) in mutations {
        let mut v = good.clone();
        v[key] = bad.clone();
        assert_eq!(read_value(dir.path(), &v), None, "{key} = {bad}");
    }
}

#[test]
fn read_accepts_only_allowlisted_outcome_classes() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = serde_json::to_value(sample(dir.path())).unwrap();
    for class in [
        "ok",
        "error",
        "usage_limit",
        "transient_transport",
        "environmental",
        "work",
        "indeterminate",
    ] {
        v["outcome"] =
            json!({"exit_code": 1, "class": class, "finished_at": "2026-10-28T12:00:00Z"});
        assert!(read_value(dir.path(), &v).is_some(), "{class}");
    }
    for class in ["pwned", "OK", "stopped on a usage limit\u{1b}[2J", ""] {
        v["outcome"] =
            json!({"exit_code": 1, "class": class, "finished_at": "2026-10-28T12:00:00Z"});
        assert_eq!(read_value(dir.path(), &v), None, "{class:?}");
    }
}

#[test]
fn read_of_garbage_or_a_missing_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read(&dir.path().join("missing.run.json")), None);
    let path = dir.path().join("g.run.json");
    fs::write(&path, "not json").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read(&path), None);
}

// ---------------------------------------------------------------------------
// Outcome class
// ---------------------------------------------------------------------------

#[test]
fn the_marker_class_names_match_failure_class() {
    // The allowlist must be FailureClass's own names.
    assert_eq!(FailureClass::UsageLimit.as_str(), "usage_limit");
    assert_eq!(
        classify_tail(&marker(FailureClass::Work.as_str()), Some(1)),
        Some("work")
    );
}

#[test]
fn a_marker_decides_whatever_the_exit_code() {
    let tail = format!("noise\n{}\nmore noise\n", marker("usage_limit"));
    assert_eq!(classify_tail(&tail, Some(0)), Some("usage_limit"));
    assert_eq!(classify_tail(&tail, Some(1)), Some("usage_limit"));
}

#[test]
fn the_last_marker_wins() {
    let tail = format!("{}\n{}\n", marker("transient_transport"), marker("work"));
    assert_eq!(classify_tail(&tail, Some(1)), Some("work"));
}

#[test]
fn a_forged_or_malformed_marker_is_ignored() {
    for line in [
        format!("{FAILURE_CLASS_MARKER_PREFIX}{}", json!({"class": "pwned"})),
        format!("{FAILURE_CLASS_MARKER_PREFIX}usage_limit"),
        format!("{FAILURE_CLASS_MARKER_PREFIX}{{not json"),
        format!("{FAILURE_CLASS_MARKER_PREFIX}{}", json!({"class": 7})),
    ] {
        assert_eq!(classify_tail(&line, Some(1)), Some("error"), "{line}");
        assert_eq!(classify_tail(&line, Some(0)), Some("ok"), "{line}");
        assert_eq!(classify_tail(&line, None), None, "{line}");
    }
}

#[test]
fn exit_zero_is_ok_even_when_the_log_mentions_a_usage_limit() {
    assert_eq!(classify_tail(USAGE_LIMIT_TEXT, Some(0)), Some("ok"));
}

#[test]
fn a_failed_run_takes_the_class_its_failure_text_matches() {
    assert_eq!(
        classify_tail(USAGE_LIMIT_TEXT, Some(1)),
        Some("usage_limit")
    );
}

#[test]
fn a_failed_run_with_nothing_recognisable_is_error() {
    assert_eq!(classify_tail("", Some(2)), Some("error"));
    assert_eq!(
        classify_tail("it broke somehow\n", Some(137)),
        Some("error")
    );
}

#[test]
fn without_an_exit_code_only_a_marker_gives_a_class() {
    assert_eq!(classify_tail(USAGE_LIMIT_TEXT, None), None);
    assert_eq!(classify_tail("", None), None);
    assert_eq!(
        classify_tail(&marker("usage_limit"), None),
        Some("usage_limit")
    );
}

#[test]
fn read_tail_returns_at_most_the_last_64_kib() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("x.log");
    let mut body = "A".repeat(200 * 1024);
    body.push_str("\nTHE END\n");
    fs::write(&log, &body).unwrap();
    let tail = read_tail(&log);
    assert!(tail.len() <= 64 * 1024, "{}", tail.len());
    assert!(tail.ends_with("THE END\n"));
    assert_eq!(read_tail(&dir.path().join("missing.log")), "");
}

#[test]
fn read_tail_does_not_follow_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("secret");
    fs::write(&target, "secret contents").unwrap();
    let link = dir.path().join("x.log");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(read_tail(&link), "");
}

// ---------------------------------------------------------------------------
// Origin
// ---------------------------------------------------------------------------

#[test]
fn repo_slug_keeps_owner_and_name_and_drops_credentials() {
    assert_eq!(
        repo_slug("https://user:tok3n@github.com/owner/name.git").as_deref(),
        Some("owner/name")
    );
    assert_eq!(
        repo_slug("git@github.com:rysweet/amplihack-rs.git").as_deref(),
        Some("rysweet/amplihack-rs")
    );
    assert_eq!(repo_slug("https://gitlab.com/o/n.git"), None);
    assert_eq!(repo_slug("not a url"), None);
}

#[test]
fn a_record_built_from_a_tokenised_origin_holds_no_credential() {
    let dir = tempfile::tempdir().unwrap();
    let rec = build(
        SESSION,
        None,
        Utc::now(),
        "r",
        &Map::new(),
        repo_slug("https://user:tok3n@github.com/owner/name.git"),
        dir.path(),
    );
    let path = record_path(dir.path(), SESSION);
    write_new(&path, &rec).unwrap();
    let on_disk = fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("tok3n"), "{on_disk}");
    assert!(!on_disk.contains('@'), "{on_disk}");
    assert!(on_disk.contains("owner/name"));
}

#[test]
fn origin_repo_reads_the_origin_of_a_checkout_within_its_timeout() {
    let have_git = amplihack_git::command()
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_git {
        eprintln!("skipping: git is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://user:tok3n@github.com/owner/name.git",
        ],
    ] {
        assert!(
            amplihack_git::command()
                .args(&args)
                .current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let start = Instant::now();
    assert_eq!(
        origin_repo(&repo, Duration::from_secs(3)).as_deref(),
        Some("owner/name")
    );
    assert!(start.elapsed() < Duration::from_secs(4));

    let plain = dir.path().join("plain");
    fs::create_dir(&plain).unwrap();
    assert_eq!(origin_repo(&plain, Duration::from_secs(3)), None);
}
