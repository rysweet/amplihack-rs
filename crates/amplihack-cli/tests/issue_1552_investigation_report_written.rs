//! Issue #1552: report creation must write literal synthesis and reject bad
//! output paths without interpreting recipe context as shell source.

use serde_yaml::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn recipe_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes/investigation-report.yaml")
}

fn steps() -> Vec<Value> {
    let text = std::fs::read_to_string(recipe_path()).expect("read recipe");
    let doc: Value = serde_yaml::from_str(&text).expect("parse recipe");
    doc["steps"].as_sequence().expect("steps").clone()
}

fn step(id: &str) -> Value {
    steps()
        .into_iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing step {id}"))
}

fn command_of(id: &str) -> String {
    step(id)["command"].as_str().expect("command").to_string()
}

fn run_create(synthesis: &str, out_dir: &Path) -> Output {
    Command::new("bash")
        .arg("-c")
        .arg(command_of("create-investigation-report"))
        .env("OUTPUT_DIR", out_dir)
        .env("SYNTHESIS", synthesis)
        .output()
        .expect("run bash")
}

#[test]
fn recipe_shell_steps_do_not_interpolate_context_into_commands() {
    for id in [
        "update-discoveries",
        "update-patterns",
        "create-investigation-report",
        "efficiency-report",
        "final-output",
    ] {
        let command = command_of(id);
        assert!(
            !command.contains("{{"),
            "{id} must read context from runner exports, not shell templates"
        );
    }
}

#[test]
fn final_output_follows_report_creation() {
    let s = steps();
    let index = |id: &str| {
        s.iter()
            .position(|step| step["id"].as_str() == Some(id))
            .unwrap()
    };
    assert!(index("final-output") > index("create-investigation-report"));
}

#[test]
fn writes_report_file_with_synthesis() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let out = run_create("# Findings\nthe answer is 42", &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = std::fs::read_to_string(dir.join("investigation_report.md")).expect("report file");
    assert_eq!(body.trim_end(), "# Findings\nthe answer is 42");
    let response: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON status");
    assert_eq!(response["status"], "created");
}

#[test]
fn hostile_output_dir_is_literal_and_never_runs_command_substitution() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("pwned");
    let dir = tmp.path().join(format!("$(touch {})", marker.display()));
    let out = run_create("safe synthesis", &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!marker.exists(), "output_dir was executed as shell source");
    assert!(dir.join("investigation_report.md").is_file());

    let update = Command::new("bash")
        .arg("-c")
        .arg(command_of("update-discoveries"))
        .env("OUTPUT_DIR", &dir)
        .output()
        .expect("run update-discoveries");
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    assert!(!marker.exists(), "update-discoveries executed output_dir");
}

#[test]
fn fixed_heredoc_delimiter_and_following_shell_code_remain_literal() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let marker = tmp.path().join("pwned");
    let hostile = format!(
        "before\nAMPLIHACK_INVESTIGATION_REPORT_EOF\n$(touch {})\nafter",
        marker.display()
    );
    let out = run_create(&hostile, &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!marker.exists(), "synthesis was executed as shell source");
    assert_eq!(
        std::fs::read_to_string(dir.join("investigation_report.md"))
            .unwrap()
            .trim_end(),
        hostile
    );
}

#[test]
fn recipe_var_prefixed_runner_exports_are_supported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let out = Command::new("bash")
        .arg("-c")
        .arg(command_of("create-investigation-report"))
        .env_remove("OUTPUT_DIR")
        .env_remove("SYNTHESIS")
        .env("RECIPE_VAR_output_dir", &dir)
        .env("RECIPE_VAR_synthesis", "from prefixed exports")
        .output()
        .expect("run bash");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("investigation_report.md"))
            .unwrap()
            .trim_end(),
        "from prefixed exports"
    );
}

#[test]
fn empty_synthesis_fails_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let out = run_create("", &dir);
    assert!(
        !out.status.success(),
        "empty report must not be reported as created"
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("created"));
}

#[test]
fn empty_synthesis_does_not_replace_an_existing_report() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    std::fs::create_dir_all(&dir).unwrap();
    let report = dir.join("investigation_report.md");
    std::fs::write(&report, "previous successful report\n").unwrap();

    let out = run_create(" \n  \n", &dir);
    assert!(!out.status.success());
    assert_eq!(
        std::fs::read_to_string(report).unwrap(),
        "previous successful report\n"
    );
}

#[test]
fn empty_output_dir_fails_nonzero() {
    let out = Command::new("bash")
        .arg("-c")
        .arg(command_of("create-investigation-report"))
        .env("OUTPUT_DIR", "")
        .env("SYNTHESIS", "x")
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[cfg(unix)]
#[test]
fn refuses_symlink_report_target_without_changing_target() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    std::fs::create_dir_all(&dir).unwrap();
    let target = tmp.path().join("protected.md");
    std::fs::write(&target, "keep this content").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("investigation_report.md")).unwrap();

    let out = run_create("replacement", &dir);
    assert!(!out.status.success());
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "keep this content"
    );
}

#[test]
fn refuses_directory_report_target_without_adding_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let report = dir.join("investigation_report.md");
    std::fs::create_dir_all(&report).unwrap();
    let out = run_create("replacement", &dir);
    assert!(!out.status.success());
    assert!(std::fs::read_dir(&report).unwrap().next().is_none());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn efficiency_step_reads_start_time_from_nested_tracking_json() {
    let tracking = r#"{"start_time":"2030-01-02T03:04:05Z","message_count":1}"#;
    let out = Command::new("bash")
        .arg("-c")
        .arg(command_of("efficiency-report"))
        .env("RECIPE_VAR_tracking", tracking)
        .output()
        .expect("run efficiency-report");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("efficiency JSON");
    assert_eq!(json["start_time"], "2030-01-02T03:04:05Z");
}

/// Exercise one exact shipped step through recipe-runner-rs when available.
/// CI images that do not install the runner still cover this command directly
/// above; local Gadugi/manual QA can run the runner-backed case as a black box.
#[test]
fn recipe_runner_exports_context_and_executes_shipped_report_step() {
    let explicit_runner = std::env::var_os("RECIPE_RUNNER_QA_BINARY").map(PathBuf::from);
    let runner = explicit_runner
        .clone()
        .unwrap_or_else(|| PathBuf::from("recipe-runner-rs"));
    if Command::new(&runner).arg("--version").output().is_err() {
        if explicit_runner.is_some()
            || std::env::var_os("AMPLIHACK_1552_REQUIRE_RECIPE_RUNNER").is_some()
        {
            panic!("recipe-runner-rs is required for the Gadugi runner scenario");
        }
        eprintln!("recipe-runner-rs unavailable; skipping runner-backed scenario");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("runner out");
    let marker = tmp.path().join("runner-pwned");
    let synthesis = format!(
        "AMPLIHACK_INVESTIGATION_REPORT_EOF\n$(touch {})\nliteral",
        marker.display()
    );
    let command = command_of("create-investigation-report");
    let command_block = command
        .lines()
        .map(|line| format!("      {line}\n"))
        .collect::<String>();
    let recipe = format!(
        "name: issue-1552-runner-regression\ndescription: runner regression\nversion: 1.0.0\ncontext: {{}}\nsteps:\n  - id: create-report\n    type: bash\n    command: |\n{command_block}    output: report_creation\n    parse_json: true\n"
    );
    let recipe_path = tmp.path().join("regression.yaml");
    std::fs::write(&recipe_path, recipe).unwrap();
    let output = Command::new(&runner)
        .arg(&recipe_path)
        .arg("--working-dir")
        .arg(tmp.path())
        .arg("--no-auto-stage")
        .arg("--set")
        .arg(format!("output_dir={}", dir.display()))
        .arg("--set")
        .arg(format!("synthesis={synthesis}"))
        .output()
        .expect("run recipe-runner-rs");
    assert!(
        output.status.success(),
        "runner failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !marker.exists(),
        "runner context was executed as shell source"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("investigation_report.md"))
            .unwrap()
            .trim_end(),
        synthesis
    );
}
