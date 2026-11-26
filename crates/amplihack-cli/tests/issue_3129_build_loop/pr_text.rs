//! B1 (bb:0's second addition): after a pull request opens, its title and
//! body are read back and restored at most once. `pr_text_apply` in
//! `workflow_pr_text.sh` runs against a stand-in gh that keeps the live text
//! in files, so an edit changes what the next read returns.

#![cfg(target_os = "linux")]

use super::tools::{text, tools_dir};
use crate::support::exec_stub::write_executable;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `pr view --json title,body` answers from `live-title` and `live-body`.
/// `pr edit` writes the new text there, unless `edit-ignored` exists (GitHub
/// rewrote it again) or `edit-fails` exists. `view-fails` fails every read.
const GH: &str = r##"#!/usr/bin/env bash
G="$GH_STATE"
printf '%s\n' "$*" >> "$G/calls.log"
case "${1:-} ${2:-}" in
  "pr view")
    [ -e "$G/view-fails" ] && { echo "HTTP 502" >&2; exit 1; }
    jq -cn --rawfile t "$G/live-title" --rawfile b "$G/live-body" '{title:$t, body:$b}' ;;
  "pr edit")
    [ -e "$G/edit-fails" ] && { echo "HTTP 422" >&2; exit 1; }
    [ -e "$G/edit-ignored" ] && exit 0
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      [ "$prev" = --body-file ] && cp -- "$a" "$G/live-body"
      prev="$a"
    done ;;
  *) echo "stand-in gh: unexpected call: $*" >&2; exit 1 ;;
esac
"##;

struct Fixture {
    _tmp: tempfile::TempDir,
    gh: PathBuf,
    bin: PathBuf,
    title: PathBuf,
    body: PathBuf,
}

fn fixture(intended: (&str, &str), live: (&str, &str)) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let gh = base.join("gh");
    let bin = base.join("bin");
    fs::create_dir_all(&gh).unwrap();
    fs::create_dir_all(&bin).unwrap();
    write_executable(&bin.join("gh"), GH, 0o755);
    let title = base.join("title");
    let body = base.join("body");
    fs::write(&title, intended.0).unwrap();
    fs::write(&body, intended.1).unwrap();
    fs::write(gh.join("live-title"), live.0).unwrap();
    fs::write(gh.join("live-body"), live.1).unwrap();
    Fixture {
        _tmp: tmp,
        gh,
        bin,
        title,
        body,
    }
}

fn apply(f: &Fixture) -> (Option<i32>, Value) {
    let script = format!(
        ". '{}/workflow_pr_text.sh'; pr_text_apply 7 \"$1\" \"$2\"",
        tools_dir().display()
    );
    let out = Command::new("bash")
        .args(["-c", &script, "_"])
        .arg(&f.title)
        .arg(&f.body)
        .env("PATH", format!("{}:/usr/bin:/bin", f.bin.display()))
        .env("GH_STATE", &f.gh)
        .env("TMPDIR", f.gh.parent().unwrap())
        .output()
        .unwrap();
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let v = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(&out)));
    (out.status.code(), v)
}

fn edits(gh: &Path) -> usize {
    fs::read_to_string(gh.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("pr edit"))
        .count()
}

#[test]
fn text_that_already_matches_is_left_alone() {
    let f = fixture(("Title", "Body\nFixes #1\n"), ("Title", "Body\nFixes #1\n"));
    let (code, v) = apply(&f);
    assert_eq!(code, Some(0));
    assert_eq!(v["result"], "same");
    assert_eq!(edits(&f.gh), 0);
}

#[test]
fn crlf_and_trailing_whitespace_are_not_differences() {
    let f = fixture(
        ("Title", "Line one\nLine two\n"),
        ("Title ", "Line one\r\nLine two  \n\n"),
    );
    let (_, v) = apply(&f);
    assert_eq!(v["result"], "same", "{v}");
    assert_eq!(edits(&f.gh), 0);
}

#[test]
fn an_altered_body_is_restored_with_one_edit() {
    let f = fixture(
        ("Title", "Task: T-1\nRefs #3129\n"),
        ("Title", "A generated summary\n"),
    );
    let (_, v) = apply(&f);
    assert_eq!(v["result"], "corrected", "{v}");
    assert_eq!(edits(&f.gh), 1);
    assert_eq!(
        fs::read_to_string(f.gh.join("live-body")).unwrap(),
        "Task: T-1\nRefs #3129\n"
    );
}

#[test]
fn a_difference_that_remains_is_reported_after_one_edit_only() {
    let f = fixture(("Title", "Mine\n"), ("Other title", "Theirs\n"));
    fs::write(f.gh.join("edit-ignored"), "").unwrap();
    let (code, v) = apply(&f);
    assert_eq!(code, Some(0), "a difference is evidence, never a failure");
    assert_eq!(v["result"], "differs");
    assert_eq!(v["differs"], serde_json::json!(["title", "body"]));
    assert_eq!(edits(&f.gh), 1);
}

#[test]
fn a_read_that_fails_is_unread_and_nothing_is_edited() {
    let f = fixture(("Title", "Body"), ("Title", "Other"));
    fs::write(f.gh.join("view-fails"), "").unwrap();
    let (code, v) = apply(&f);
    assert_eq!(code, Some(0));
    assert_eq!(v["result"], "unread");
    assert_eq!(edits(&f.gh), 0);
}
