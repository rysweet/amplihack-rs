//! Durable recorder contracts at the runner launch boundary.
//! Real shell/agent/hook descendants still require pinned-runner integration tests.
use super::*;

fn command_logs(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            command_logs(&entry.path(), found);
        } else if entry.file_name() == "commands.jsonl" {
            found.push(entry.path());
        }
    }
}

#[test]
fn absolute_git_add_has_durable_attributed_command_record() {
    let f = Fixture::new();
    succeeds(
        &f.command("recorded")
            .env("ISSUE_2096_STAGE", "1")
            .output()
            .unwrap(),
    );
    let mut logs = Vec::new();
    command_logs(&f.root.join("work/runs"), &mut logs);
    assert!(
        !logs.is_empty(),
        "required command recording was silently omitted"
    );
    let records: Vec<serde_json::Value> = logs
        .iter()
        .flat_map(|path| {
            assert!(!path.components().any(|c| c.as_os_str() == "runtime"));
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).expect("valid command JSONL"))
                .collect::<Vec<_>>()
        })
        .collect();
    let add = records
        .iter()
        .find(|r| {
            r["command"].as_array().is_some_and(|argv| {
                argv.iter().any(|a| a == "add") && argv.iter().any(|a| a == "run-output")
            })
        })
        .expect("absolute Git invocation must be observed, not merely parent launch argv");
    for field in [
        "schema_version",
        "run_id",
        "timestamp",
        "executable",
        "cwd",
        "index_path",
        "git_environment",
    ] {
        assert!(
            !add[field].is_null(),
            "missing required attribution field {field}: {add}"
        );
    }
    assert_eq!(
        add["cwd"].as_str().unwrap().as_bytes(),
        f.observed("recorded", "cwd")
    );
    assert_eq!(
        add["index_path"].as_str().unwrap(),
        String::from_utf8(f.observed("recorded", "index"))
            .unwrap()
            .trim()
    );
    assert!(!add["run_id"].as_str().unwrap().is_empty());
    assert!(
        add["executable"]
            .as_str()
            .is_some_and(|p| Path::new(p).is_absolute())
    );
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        assert!(
            add["git_environment"].get(key).is_some(),
            "absence of {key} must be explicit"
        );
    }
    use std::os::unix::fs::PermissionsExt;
    for path in logs {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o077,
            0,
            "sensitive command records must be owner-only"
        );
    }
}

#[test]
fn unavailable_durable_record_storage_blocks_runner_launch() {
    let f = Fixture::new();
    // A file is an event-independent, deterministic storage failure, even as root.
    let blocked = f.root.join("not-a-directory");
    fs::write(&blocked, "occupied").unwrap();
    let out = f
        .command("blocked")
        .env("AMPLIHACK_WORK_ROOT", blocked)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "mandatory recording cannot fall back to best effort"
    );
    assert!(
        !f.root.join("blocked/cwd").exists(),
        "runner executed without durable storage"
    );
}
