//! C3a: `tracking_issue_reference` says which issue the run is about and where
//! the number came from. The older functions are untouched.

#![cfg(target_os = "linux")]

use super::tools::{text, tools_dir};
use std::process::Command;

fn reference(task: &str, existing: &str) -> String {
    let script = format!(
        ". '{}/workflow_issue_tracking.sh'; tracking_issue_reference",
        tools_dir().display()
    );
    let out = Command::new("bash")
        .args(["-c", &script])
        .env("TASK_DESC", task)
        .env("EXISTING_ISSUE_NUMBER", existing)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn a_marked_reference_in_the_task_is_found_without_an_issue_number() {
    assert_eq!(
        reference("Continue the work. Fixes #463.", ""),
        "463 marked-reference"
    );
}

#[test]
fn a_bare_reference_is_found_and_named_as_bare() {
    assert_eq!(
        reference("Follow up on #12 please", ""),
        "12 bare-reference"
    );
}

#[test]
fn the_context_issue_number_wins_and_is_named_as_context() {
    assert_eq!(reference("Fixes #463", "9"), "9 context");
}

#[test]
fn a_task_with_no_reference_says_none() {
    assert_eq!(reference("Tidy the readme", ""), "none");
}

#[test]
fn a_reference_inside_a_code_span_is_not_taken() {
    assert_eq!(reference("Run `echo Fixes #77` and stop", ""), "none");
}
