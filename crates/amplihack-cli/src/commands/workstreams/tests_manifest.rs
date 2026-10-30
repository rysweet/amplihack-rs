//! Issue #1710 (R4): the workstream manifest.
//!
//! Contract (`manifest`):
//! - `parse(text) -> Result<Vec<ManifestRow>, ManifestError>`: three
//!   tab-separated fields `label issue pr`; `-` is empty; a number may carry
//!   a leading `#`; a row needs an issue or a PR; `#` lines and blank lines
//!   are skipped. `ManifestError { line, reason }` with a 1-based line.
//! - `load(path, explicit) -> anyhow::Result<Vec<ManifestRow>>`: a missing
//!   default is empty; a default that fails the file check is empty (with a
//!   warning on stderr); an explicit path that is missing or fails the check
//!   is an error; a malformed line is an error naming `<path>:<line>`. The
//!   check: regular file, owned by this user, mode `& 0o022 == 0`, at most
//!   64 KiB.
//! - `matches(row, issue, pr)`: same issue, or same PR when both have one.

use super::manifest::{ManifestRow, load, matches, parse};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn row(label: Option<&str>, issue: Option<u64>, pr: Option<u64>) -> ManifestRow {
    ManifestRow {
        label: label.map(str::to_string),
        issue,
        pr,
    }
}

fn write(path: &Path, body: &str, mode: u32) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn the_documented_example_parses() {
    let text = "# label\tissue\tpr\n\
                workstream-status skill\t1710\t-\n\
                rotator manual login\t#1693\t#1702\n\
                \n\
                docs sweep\t1720\t-\n";
    assert_eq!(
        parse(text).unwrap(),
        vec![
            row(Some("workstream-status skill"), Some(1710), None),
            row(Some("rotator manual login"), Some(1693), Some(1702)),
            row(Some("docs sweep"), Some(1720), None),
        ]
    );
}

#[test]
fn a_dash_label_uses_the_default_label_and_a_pr_alone_is_enough() {
    assert_eq!(
        parse("-\t-\t42\n").unwrap(),
        vec![row(None, None, Some(42))]
    );
}

#[test]
fn an_empty_file_or_only_comments_gives_no_rows() {
    assert!(parse("").unwrap().is_empty());
    assert!(parse("# a\n\n   \n# b\n").unwrap().is_empty());
}

#[test]
fn a_crlf_line_ending_is_tolerated() {
    assert_eq!(
        parse("x\t1\t-\r\n").unwrap(),
        vec![row(Some("x"), Some(1), None)]
    );
}

#[test]
fn errors_name_the_line_and_the_problem() {
    let cases = [
        (
            "# c\nok\t1\t-\nbad\t2\n",
            3,
            "expected 3 tab-separated fields, found 2",
        ),
        (
            "a\t1\t2\t3\n",
            1,
            "expected 3 tab-separated fields, found 4",
        ),
        ("a\t-\t-\n", 1, "issue or a PR"),
        ("a\tabc\t-\n", 1, "issue"),
        ("a\t1\tx7\n", 1, "PR"),
        ("a\t-1\t-\n", 1, "issue"),
        ("a\t##5\t-\n", 1, "issue"),
    ];
    for (text, line, reason) in cases {
        let err = parse(text).expect_err(text);
        assert_eq!(err.line, line, "{text:?}");
        assert!(err.reason.contains(reason), "{text:?}: {}", err.reason);
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

#[test]
fn a_missing_default_manifest_is_ignored_but_a_missing_explicit_one_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workstreams.tsv");
    assert!(load(&path, false).unwrap().is_empty());
    assert!(load(&path, true).is_err());
}

#[test]
fn a_world_readable_manifest_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.tsv");
    write(&path, "x\t1\t-\n", 0o644);
    assert_eq!(
        load(&path, true).unwrap(),
        vec![row(Some("x"), Some(1), None)]
    );
}

#[test]
fn a_group_or_world_writable_manifest_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.tsv");
    for mode in [0o664, 0o646, 0o666] {
        write(&path, "x\t1\t-\n", mode);
        assert!(load(&path, true).is_err(), "explicit {mode:o}");
        assert!(load(&path, false).unwrap().is_empty(), "default {mode:o}");
    }
}

#[test]
fn a_symlinked_or_non_regular_manifest_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.tsv");
    write(&real, "x\t1\t-\n", 0o600);
    let link = dir.path().join("link.tsv");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(load(&link, true).is_err());
    assert!(load(&link, false).unwrap().is_empty());

    let sub = dir.path().join("dir.tsv");
    fs::create_dir(&sub).unwrap();
    assert!(load(&sub, true).is_err());
}

#[test]
fn a_manifest_over_64_kib_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.tsv");
    let mut body = String::new();
    while body.len() <= 64 * 1024 {
        body.push_str("label\t1\t-\n");
    }
    write(&path, &body, 0o600);
    assert!(load(&path, true).is_err());
}

#[test]
fn a_malformed_line_is_an_error_in_either_file_naming_path_and_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.tsv");
    write(&path, "ok\t1\t-\n# c\nbad\t2\n", 0o600);
    for explicit in [true, false] {
        let msg = format!("{:#}", load(&path, explicit).expect_err("malformed"));
        assert!(
            msg.contains(&format!(
                "{}:3: expected 3 tab-separated fields, found 2",
                path.display()
            )),
            "{msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

#[test]
fn rows_match_by_issue_or_by_pr_when_both_have_one() {
    let by_issue = row(Some("a"), Some(1710), None);
    assert!(matches(&by_issue, Some(1710), None));
    assert!(matches(&by_issue, Some(1710), Some(99)));
    assert!(!matches(&by_issue, Some(1711), None));
    assert!(!matches(&by_issue, None, None));

    let by_pr = row(Some("b"), None, Some(1702));
    assert!(matches(&by_pr, None, Some(1702)));
    assert!(matches(&by_pr, Some(5), Some(1702)));
    assert!(!matches(&by_pr, Some(5), None));
    assert!(!matches(&by_pr, None, Some(1703)));
}
