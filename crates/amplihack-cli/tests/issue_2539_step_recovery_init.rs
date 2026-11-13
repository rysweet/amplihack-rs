//! Issue #2539: `step-recovery init`, and the refusal of unsafe recovery
//! directories, record files and step ids. Unsafe paths are refused with
//! exit 2 and never repaired with `chmod`.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use crate::support::step_recovery::{Fixture, private_dir, show, text};

fn record(f: &Fixture, step: &str, reason: &str) -> std::process::Output {
    f.amplihack()
        .args(["orch", "helper", "step-recovery", "record", "--dir"])
        .arg(&f.dir)
        .args([
            "--step-id",
            step,
            "--reason",
            reason,
            "--owner",
            "run-requester",
            "--needs",
            "rerun finalization",
        ])
        .output()
        .unwrap()
}

#[test]
fn run_refuses_an_unsafe_directory_a_relative_dir_and_bad_step_ids() {
    let f = Fixture::new();
    let tool = f.tool("fails", "#!/bin/sh\nexit 1\n");
    let tool = tool.to_str().unwrap();

    fs::set_permissions(&f.dir, fs::Permissions::from_mode(0o755)).unwrap();
    let o = f.run("step-03-finalize", &[], &[tool]);
    assert_eq!(
        o.status.code(),
        Some(2),
        "group/other bits on the dir:\n{}",
        show(&o)
    );
    assert_eq!(f.report().status.code(), Some(2), "report refuses it too");
    fs::set_permissions(&f.dir, fs::Permissions::from_mode(0o700)).unwrap();

    let long = "x".repeat(129);
    for bad in [
        "step:alt:x",
        "x__alt__y",
        "..",
        ".",
        "",
        "has space",
        long.as_str(),
    ] {
        let o = f.run(bad, &[], &[tool]);
        assert_eq!(o.status.code(), Some(2), "step id {bad:?}:\n{}", show(&o));
    }
    let o = f
        .amplihack()
        .args([
            "orch",
            "helper",
            "step-recovery",
            "run",
            "--step-id",
            "s",
            "--dir",
            "relative/dir",
            "--goal",
            "g",
            "--",
            tool,
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "relative --dir:\n{}", show(&o));
    assert!(
        f.agent_calls().is_empty(),
        "no refused call reaches an agent"
    );
}

#[test]
fn a_record_file_with_open_permissions_stops_run_with_exit_2() {
    let f = Fixture::new();
    assert_eq!(record(&f, "step-03-finalize", "x").status.code(), Some(0));
    fs::set_permissions(
        f.dir.join("step-03-finalize.jsonl"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let tool = f.tool("fails", "#!/bin/sh\nexit 1\n");
    let o = f.run("step-03-finalize", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
}

#[test]
fn record_refuses_anything_but_a_complete_handoff() {
    let f = Fixture::new();
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "record", "--dir"])
        .arg(&f.dir)
        .args([
            "--step-id",
            "s",
            "--reason",
            "x",
            "--owner",
            "run-requester",
            "--needs",
            "",
        ])
        .output()
        .unwrap();
    assert_ne!(o.status.code(), Some(0), "empty needs:\n{}", show(&o));
    assert!(!f.dir.join("s.jsonl").exists());
}

#[test]
fn init_creates_a_private_directory_under_the_state_home() {
    let f = Fixture::new();
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "init"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let path = std::path::PathBuf::from(text(&o.stdout).trim());
    let parent = f.root.join("state/amplihack/step-recovery");
    assert_eq!(path.parent(), Some(parent.as_path()), "{}", path.display());
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit()),
        "{name}"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let again = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "init"])
        .output()
        .unwrap();
    assert_ne!(
        text(&again.stdout),
        text(&o.stdout),
        "one fresh directory per run"
    );
}

#[test]
fn init_ignores_a_relative_xdg_state_home() {
    let f = Fixture::new();
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "init"])
        .env("XDG_STATE_HOME", "relative-state")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let path = text(&o.stdout).trim().to_string();
    let expected = f.home.join(".local/state/amplihack/step-recovery");
    assert!(path.starts_with(expected.to_str().unwrap()), "{path}");
    assert!(!f.work.join("relative-state").exists());
}

#[test]
fn init_with_dir_checks_the_directory_and_prints_it_or_disables_recovery() {
    let f = Fixture::new();
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "init", "--dir"])
        .arg(&f.dir)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(text(&o.stdout).trim(), f.dir.to_str().unwrap());

    let open = f.root.join("open");
    private_dir(&open);
    fs::set_permissions(&open, fs::Permissions::from_mode(0o750)).unwrap();
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "init", "--dir"])
        .arg(&open)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "init never stops the run:\n{}",
        show(&o)
    );
    assert_eq!(text(&o.stdout), "\n", "an empty value disables recovery");
    assert!(
        text(&o.stderr).contains("[step-recovery] WARNING: recovery disabled:"),
        "{}",
        show(&o)
    );
    assert_eq!(
        fs::metadata(&open).unwrap().permissions().mode() & 0o777,
        0o750,
        "never chmod-fixed"
    );
}
