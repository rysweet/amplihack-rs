//! Issue #1552: `create-investigation-report` must actually write
//! `{{output_dir}}/investigation_report.md` (and fail if it cannot).

use serde_yaml::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn recipe_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes/investigation-report.yaml")
}

fn steps() -> Vec<Value> {
    let text = std::fs::read_to_string(recipe_path()).expect("read recipe");
    let doc: Value = serde_yaml::from_str(&text).expect("parse recipe");
    doc["steps"].as_sequence().expect("steps").clone()
}

fn step_index(steps: &[Value], id: &str) -> usize {
    steps
        .iter()
        .position(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing step {id}"))
}

fn command_of(id: &str) -> String {
    let s = steps();
    s[step_index(&s, id)]["command"]
        .as_str()
        .expect("command")
        .to_string()
}

/// Textually substitute template vars the way the runner does; any other
/// `{{...}}` placeholder becomes empty.
fn render(cmd: &str, synthesis: &str, out_dir: &str) -> String {
    let cmd = cmd
        .replace("{{synthesis}}", synthesis)
        .replace("{{output_dir}}", out_dir);
    let mut out = String::new();
    let mut rest = cmd.as_str();
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        match rest[i..].find("}}") {
            Some(j) => rest = &rest[i + j + 2..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn run(synthesis: &str, out_dir: &Path) -> std::process::Output {
    let script = render(
        &command_of("create-investigation-report"),
        synthesis,
        out_dir.to_str().unwrap(),
    );
    Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .expect("run bash")
}

#[test]
fn step_references_synthesis_and_checks_file() {
    let cmd = command_of("create-investigation-report");
    assert!(
        cmd.contains("{{synthesis}}"),
        "step must write the synthesis"
    );
    assert!(cmd.contains("set -e"), "step must fail fast");
    assert!(
        cmd.contains("-s "),
        "step must verify the file is non-empty"
    );
}

#[test]
fn final_output_comes_after_report_creation() {
    let s = steps();
    assert!(step_index(&s, "final-output") > step_index(&s, "create-investigation-report"));
}

#[test]
fn writes_report_file_with_synthesis() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let out = run("# Findings\nthe answer is 42", &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = std::fs::read_to_string(dir.join("investigation_report.md")).expect("report file");
    assert_eq!(body.trim_end(), "# Findings\nthe answer is 42");
}

#[test]
fn hostile_synthesis_is_written_literally() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let marker = tmp.path().join("pwned");
    let hostile = format!(
        "$(touch {m}) `touch {m}` \"; touch {m}; '",
        m = marker.display()
    );
    let out = run(&hostile, &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!marker.exists(), "synthesis was executed as shell");
    let body = std::fs::read_to_string(dir.join("investigation_report.md")).unwrap();
    assert_eq!(body.trim_end(), hostile);
}

#[test]
fn empty_synthesis_fails_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let out = run("", &dir);
    assert!(
        !out.status.success(),
        "empty report must not be reported as created"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("\"created\""),
        "must not claim success: {stdout}"
    );
}

#[test]
fn empty_output_dir_fails_nonzero() {
    let script = render(&command_of("create-investigation-report"), "x", "");
    let out = Command::new("bash").arg("-c").arg(script).output().unwrap();
    assert!(!out.status.success());
}
