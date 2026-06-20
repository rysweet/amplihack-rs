use super::launcher::{
    VALID_DELEGATES, write_classic_launcher, write_executable_script, write_recipe_launcher,
};
use super::models::Workstream;
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn test_valid_delegates() {
    assert!(VALID_DELEGATES.contains(&"amplihack claude"));
    assert!(VALID_DELEGATES.contains(&"amplihack copilot"));
    assert!(VALID_DELEGATES.contains(&"amplihack amplifier"));
    assert!(!VALID_DELEGATES.contains(&"rm -rf /"));
}

#[test]
fn executable_script_writer_normalizes_crlf_and_lone_cr_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let crlf_script = dir.path().join("crlf-run.sh");
    let lone_cr_script = dir.path().join("lone-cr-run.sh");

    write_executable_script(
        &crlf_script,
        "#!/usr/bin/env bash\r\nset -euo pipefail\r\necho crlf\r\n",
    )
    .unwrap();
    write_executable_script(
        &lone_cr_script,
        "#!/usr/bin/env bash\rset -euo pipefail\recho lone-cr\r",
    )
    .unwrap();

    for script in [&crlf_script, &lone_cr_script] {
        assert_script_is_lf_only_and_bash_valid(script);
    }
}

#[test]
fn recipe_launcher_scripts_are_lf_only_and_bash_valid() {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = test_workstream(base.path(), state.path());
    fs::create_dir_all(&ws.work_dir).unwrap();

    write_recipe_launcher(&ws, "amplihack copilot").unwrap();

    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("launcher.sh"));
    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
}

#[test]
fn classic_launcher_script_is_lf_only_and_bash_valid() {
    let base = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = test_workstream(base.path(), state.path());
    fs::create_dir_all(&ws.work_dir).unwrap();

    write_classic_launcher(&ws, "amplihack copilot").unwrap();

    assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
}

fn test_workstream(base_dir: &Path, state_dir: &Path) -> Workstream {
    Workstream::new(
        792,
        "feature/line-endings".to_string(),
        "Fix line endings".to_string(),
        "Normalize generated executable scripts".to_string(),
        "default-workflow".to_string(),
        base_dir,
        state_dir,
    )
}

fn assert_script_is_lf_only_and_bash_valid(script: &Path) {
    let bytes = fs::read(script).unwrap();
    assert!(
        !bytes.contains(&b'\r'),
        "{} contains carriage returns and will fail under bash",
        script.display()
    );

    let output = Command::new("bash")
        .arg("-n")
        .arg(script)
        .output()
        .unwrap_or_else(|err| panic!("failed to run bash -n for {}: {err}", script.display()));

    assert!(
        output.status.success(),
        "bash -n rejected {}:\nstdout:\n{}\nstderr:\n{}",
        script.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
