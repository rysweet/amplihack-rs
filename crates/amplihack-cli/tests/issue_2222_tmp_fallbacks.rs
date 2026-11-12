//! Issue #2222 (D5): the two audited recipe steps that wrote to, or cleaned,
//! a bare `/tmp/...` path fall back through `${TMPDIR:-/tmp}` instead.
//!
//! - `smart-execute-routing.yaml` `cleanup-helper` removes the
//!   `smart-orch-ws-*.json` files that `orch.rs` creates with
//!   `std::env::temp_dir()`, which honours `TMPDIR`.
//! - `quality-audit-cycle.yaml` `merge-validations` keeps an unparseable
//!   validator's raw output under `${TMPDIR:-/tmp}/cycle_N` when the caller
//!   sets `output_dir` to an empty string; an explicit `output_dir` still wins.
//!
//! Both steps run from the shipped YAML with `TMPDIR` set to a temporary
//! directory. Nothing here creates files in the real `/tmp`; the one case
//! that could (the unfixed quality-audit fallback) uses a unique cycle name
//! and removes only that directory.

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support::auto_drive_light::{jq_available, show, step_command};

/// Guards `/tmp/cycle_<unique>`. This is the one bare `/tmp` path the tests
/// name, and only to check it stays absent, proving the fallback moved (D7).
/// It must be absent before the step runs, so any directory found there on
/// drop was created by the step under test and is removed.
struct StrayCycleDir(PathBuf);

impl StrayCycleDir {
    fn new(path: PathBuf) -> Self {
        assert!(
            !path.exists(),
            "{} already exists; refusing to run against another run's directory",
            path.display()
        );
        Self(path)
    }
}

impl Drop for StrayCycleDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_step(command: &str, tmpdir: &Path, cwd: &Path) -> Output {
    crate::isolated_amplihack::isolated("bash")
        .args(["-c", command])
        .current_dir(cwd)
        .env("TMPDIR", tmpdir)
        .env("HOME", cwd)
        .output()
        .expect("run recipe step")
}

#[test]
fn smart_execute_cleanup_removes_workstream_files_from_tmpdir() {
    let tmp = tempfile::tempdir().unwrap();
    let tmpdir = tmp.path().join("tmpdir");
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&tmpdir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    for name in ["smart-orch-ws-a1b2c3d4.json", "smart-orch-ws-e5f6a7b8.json"] {
        std::fs::write(tmpdir.join(name), "[]\n").unwrap();
    }
    std::fs::write(tmpdir.join("unrelated.json"), "{}\n").unwrap();
    let command = step_command("smart-execute-routing", "cleanup-helper");
    assert!(
        !command.contains("rm -f /tmp/"),
        "cleanup-helper must not clean a bare /tmp path:\n{command}"
    );
    let out = run_step(&command, &tmpdir, &cwd);
    assert!(out.status.success(), "{}", show(&out));
    let left: Vec<String> = std::fs::read_dir(&tmpdir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        left,
        vec!["unrelated.json".to_string()],
        "cleanup must remove the writer's files from $TMPDIR and nothing else"
    );
}

/// One parsed validator and one unparseable one, so the merge continues and
/// the unparseable output is preserved.
fn merge_validations(output_dir: &str, cycle: &str) -> String {
    const CONFIRM: &str = r#"{"validated":[{"finding_id":1,"verdict":"confirmed","new_severity":"high","reasoning":"r"}],"confirmed_count":1,"false_positive_count":0}"#;
    step_command("quality-audit-cycle", "merge-validations")
        .replace("{{validation_agent_1}}", CONFIRM)
        .replace("{{validation_agent_2}}", "not json at all <<unparseable>>")
        .replace("{{validation_agent_3}}", CONFIRM)
        .replace("{{validation_threshold}}", "2")
        .replace("{{cycle_number}}", cycle)
        .replace("{{output_dir}}", output_dir)
}

#[test]
fn quality_audit_empty_output_dir_keeps_raw_output_under_tmpdir() {
    if !jq_available() {
        eprintln!("SKIP: merge-validations requires jq");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let tmpdir = tmp.path().join("tmpdir");
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&tmpdir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    // Unique per test run, so the unfixed fallback cannot meet another run's
    // directory in /tmp, and the guard removes only this one.
    let cycle = format!("2222-{}", tmp.path().file_name().unwrap().to_string_lossy());
    let _stray = StrayCycleDir::new(PathBuf::from(format!("/tmp/cycle_{cycle}")));
    let out = run_step(&merge_validations("", &cycle), &tmpdir, &cwd);
    assert!(out.status.success(), "{}", show(&out));
    let artifact = tmpdir
        .join(format!("cycle_{cycle}"))
        .join("validator_v2_raw.txt");
    assert!(
        artifact.is_file(),
        "with output_dir empty, raw output belongs under $TMPDIR\n{}",
        show(&out)
    );
    assert!(
        std::fs::read_to_string(&artifact)
            .unwrap()
            .contains("<<unparseable>>")
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(&artifact.display().to_string()),
        "the warning must name the preserved path\n{}",
        show(&out)
    );
    assert!(
        !Path::new(&format!("/tmp/cycle_{cycle}")).exists(),
        "nothing may be written to the bare /tmp fallback"
    );
}

#[test]
fn quality_audit_explicit_output_dir_still_wins_over_tmpdir() {
    if !jq_available() {
        eprintln!("SKIP: merge-validations requires jq");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let tmpdir = tmp.path().join("tmpdir");
    let out_dir = tmp.path().join("explicit out");
    let cwd = tmp.path().join("cwd");
    for d in [&tmpdir, &out_dir, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    let out = run_step(
        &merge_validations(&out_dir.to_string_lossy(), "4"),
        &tmpdir,
        &cwd,
    );
    assert!(out.status.success(), "{}", show(&out));
    assert!(
        out_dir.join("cycle_4/validator_v2_raw.txt").is_file(),
        "an explicit output_dir must keep the artifact\n{}",
        show(&out)
    );
    assert!(
        !tmpdir.join("cycle_4").exists(),
        "an explicit output_dir is never replaced by $TMPDIR"
    );
}
