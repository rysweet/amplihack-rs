//! Issue #1570 finding 10 — `amplifier-bundle/tools/autodrive_loop.sh` passes
//! the task description to every round through a file.
//!
//! The autodrive loop recipes used to hand the task to the loop as
//! `--context "task_description=$TASK_DESCRIPTION"`, which the loop turned
//! into `-c task_description=...` on every round's `amplihack recipe run`.
//! Each round therefore carried the whole task in one argv element. The
//! recipe now writes the task to a file with `write_context_value` and names
//! it with `--task-file`. The loop copies it once to
//! `$STATE_DIR/task_description.txt` (0600, temporary file then `mv`), passes
//! `--context-from-file task_description=<file>` to each round, and removes
//! the copy when it exits. A missing or empty task file is exit 2.
//!
//! The loop is driven end to end with a stub `amplihack` first on `PATH` that
//! records every argv, copies the task file each round names, and plays the
//! evaluator: CONTINUE after round 1, STUCK after round 2. The loop therefore
//! runs exactly two rounds and exits 1.
//!
//! The task no longer passes through the environment, so the payload is
//! 200 KiB, above the 131,072-byte limit on a single argv or environment
//! string.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const ARGV_ELEMENT_MAX: usize = 131_072;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

const STUB: &str = r#"#!/usr/bin/env bash
set -u
# The loop pipes one `orch helper` call into another, so two stubs can run at
# once; take the call number under a lock so no two calls share a log file.
exec 9>"$STUB_STATE/calls.lock"
flock 9
n=$(cat "$STUB_STATE/calls" 2>/dev/null || echo 0)
n=$((n + 1))
printf '%s' "$n" > "$STUB_STATE/calls"
flock -u 9
exec 9>&-
printf '%s\0' "$@" > "$STUB_LOG/call-$(printf '%05d' "$n")"
case "${1:-} ${2:-}" in
  "recipe show") exit 0 ;;
  "orch helper")
    cat >/dev/null
    if [ "${3:-}" = "extract-field" ]; then
      while [ $# -gt 0 ]; do
        if [ "$1" = "--default" ]; then printf '%s' "${2:-}"; exit 0; fi
        shift
      done
    fi
    exit 0 ;;
  "recipe run")
    if [ "${3:-}" = "loop-health-evaluator" ]; then
      h=$(cat "$STUB_STATE/health" 2>/dev/null || echo 0)
      h=$((h + 1))
      printf '%s' "$h" > "$STUB_STATE/health"
      # The loop trusts only the runner's step-04 status block (#1512).
      if [ "$h" -lt 2 ]; then
        printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: CONTINUE\n'
        exit 0
      fi
      echo "LOOP_HEALTH: STUCK"; exit 1
    fi
    prev=
    for a in "$@"; do
      if [ "$prev" = "--context-from-file" ]; then
        f=${a#task_description=}
        cp -- "$f" "$STUB_STATE/task-seen-$n"
        if [ -n "$(find "$f" -prune -perm 600)" ]; then m=600; else m=other; fi
        printf '%s' "$m" > "$STUB_STATE/task-mode-$n"
      fi
      prev=$a
    done
    echo "round ran"; exit 0 ;;
esac
exit 0
"#;

fn task_payload() -> String {
    let unit = "Fix E2BIG: héllo € 😀 \"quoted\" 'single' $(touch pwned) `touch pwned2` %s -n\n";
    let mut s = String::new();
    while s.len() + unit.len() <= 200 * 1024 {
        s.push_str(unit);
    }
    s
}

struct Run {
    state_dir: PathBuf,
    stub_state: PathBuf,
    calls: Vec<Vec<String>>,
    status: std::process::ExitStatus,
    stderr: String,
    cwd: PathBuf,
    _tmp: tempfile::TempDir,
}

/// Run the loop with `task` written to the file named by `--task-file`.
fn drive_two_rounds(task: &str) -> Run {
    drive(Some(task))
}

/// Run the loop; `None` names a `--task-file` that does not exist.
fn drive(task: Option<&str>) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    let log = tmp.path().join("argv");
    let stub_state = tmp.path().join("stub-state");
    let state_dir = tmp.path().join("state");
    let repo = tmp.path().join("repo");
    let cwd = tmp.path().join("cwd");
    let task_file = tmp.path().join("task-in");
    if let Some(task) = task {
        std::fs::write(&task_file, task).unwrap();
    }
    for d in [&bin, &log, &stub_state, &repo, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    let stub = bin.join("amplihack");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let script = repo_root().join("amplifier-bundle/tools/autodrive_loop.sh");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    let out = Command::new("bash")
        .arg(&script)
        .args(["--loop-name", "crusty"])
        .args(["--round-recipe", "autodrive-crusty-round"])
        .args(["--clean-token", "CLEAN"])
        .args(["--verdict-field", "crusty_verdict"])
        .arg("--repo")
        .arg(&repo)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--task-file")
        .arg(&task_file)
        .args(["--context", "repo_path=."])
        .args(["--context", "pr_number=1570"])
        .env_clear()
        .env("PATH", path)
        .env("HOME", tmp.path())
        .env("STUB_LOG", &log)
        .env("STUB_STATE", &stub_state)
        .current_dir(&cwd)
        .output()
        .expect("spawn autodrive_loop.sh");

    let mut names: Vec<_> = std::fs::read_dir(&log)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    let calls = names
        .iter()
        .map(|p| {
            let raw = std::fs::read(p).unwrap();
            raw.split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .collect();
    Run {
        state_dir,
        stub_state,
        calls,
        status: out.status,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        cwd,
        _tmp: tmp,
    }
}

fn round_calls(run: &Run) -> Vec<&Vec<String>> {
    run.calls
        .iter()
        .filter(|c| {
            c.first().map(String::as_str) == Some("recipe")
                && c.get(1).map(String::as_str) == Some("run")
                && c.get(2).map(String::as_str) == Some("autodrive-crusty-round")
        })
        .collect()
}

/// The path given to `--context-from-file task_description=<path>`, in either
/// the two-argument or the `=` form.
fn task_file_arg(argv: &[String]) -> Option<String> {
    for (i, a) in argv.iter().enumerate() {
        let value = if a == "--context-from-file" {
            argv.get(i + 1).map(String::as_str)
        } else {
            a.strip_prefix("--context-from-file=")
        };
        if let Some(path) = value.and_then(|v| v.strip_prefix("task_description=")) {
            return Some(path.to_string());
        }
    }
    None
}

#[test]
fn task_description_is_copied_to_a_private_state_file_and_removed_at_exit() {
    let task = task_payload();
    let run = drive_two_rounds(&task);
    for round in 1..=2 {
        let seen = run
            .stub_state
            .join(format!("task-seen-{}", round_index(&run, round)));
        let got = std::fs::read(&seen).unwrap_or_else(|e| {
            panic!(
                "round {round}: the task file was not readable ({e}); stderr:\n{}",
                run.stderr
            )
        });
        assert_eq!(got.len(), task.len(), "round {round}: length must match");
        assert!(
            got == task.as_bytes(),
            "round {round}: task_description.txt must be byte-exact"
        );
        let mode_file = run
            .stub_state
            .join(format!("task-mode-{}", round_index(&run, round)));
        let mode = std::fs::read_to_string(mode_file).unwrap();
        assert_eq!(
            mode.trim(),
            "600",
            "round {round}: task_description.txt must be 0600"
        );
    }
    assert!(
        !run.state_dir.join("task_description.txt").exists(),
        "autodrive_loop.sh must remove its copy of the task when it exits"
    );
    assert!(!run.cwd.join("pwned").exists() && !run.cwd.join("pwned2").exists());
}

/// The stub's call number for the `round`-th round invocation.
fn round_index(run: &Run, round: usize) -> usize {
    let mut seen = 0;
    for (i, c) in run.calls.iter().enumerate() {
        if c.get(2).map(String::as_str) == Some("autodrive-crusty-round")
            && c.first().map(String::as_str) == Some("recipe")
        {
            seen += 1;
            if seen == round {
                return i + 1;
            }
        }
    }
    panic!("round {round} never ran; stderr:\n{}", run.stderr);
}

#[test]
fn a_missing_or_empty_task_file_is_exit_2_and_runs_no_round() {
    for (label, task) in [("missing", None), ("empty", Some(""))] {
        let run = drive(task);
        assert_eq!(
            run.status.code(),
            Some(2),
            "{label} --task-file must be exit 2; stderr:\n{}",
            run.stderr
        );
        assert!(
            run.stderr.contains("--task-file"),
            "{label}: the error must name --task-file; stderr:\n{}",
            run.stderr
        );
        assert!(
            round_calls(&run).is_empty(),
            "{label}: no round may run without a task"
        );
        assert!(!run.state_dir.join("task_description.txt").exists());
    }
}

#[test]
fn each_round_passes_the_task_by_file_and_never_in_argv() {
    let task = task_payload();
    let run = drive_two_rounds(&task);
    assert_eq!(
        run.status.code(),
        Some(1),
        "the stub evaluator says STUCK after round 2, so the loop exits 1; stderr:\n{}",
        run.stderr
    );
    let rounds = round_calls(&run);
    assert_eq!(
        rounds.len(),
        2,
        "exactly two rounds expected; stderr:\n{}",
        run.stderr
    );
    let expected = run.state_dir.join("task_description.txt");
    for (i, argv) in rounds.iter().enumerate() {
        let path = task_file_arg(argv).unwrap_or_else(|| {
            panic!(
                "round {}: missing --context-from-file task_description=<file>",
                i + 1
            )
        });
        assert_eq!(
            Path::new(&path).canonicalize().ok(),
            expected.canonicalize().ok(),
            "round {}: --context-from-file must name $STATE_DIR/task_description.txt",
            i + 1
        );
        assert!(
            !argv.iter().any(|a| a.contains("Fix E2BIG: héllo")),
            "round {}: the task text must not appear in argv",
            i + 1
        );
    }
    for argv in &run.calls {
        for a in argv {
            assert!(
                a.len() <= ARGV_ELEMENT_MAX,
                "an argv element of {} bytes exceeds {ARGV_ELEMENT_MAX}",
                a.len()
            );
        }
    }
}
