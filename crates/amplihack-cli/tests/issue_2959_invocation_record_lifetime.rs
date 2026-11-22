//! Issue #2959: a drive's invocation record lasts as long as the drive.
//!
//! The record (`autodrive-invocation-*/invocation.json`) was a loose folder
//! under `TMPDIR`. It now lives in the run's own durable folder, beside the
//! `autodrive-state` folder the runner binds, which cleanup does not sweep
//! while the run is live.
//!
//! A stand-in runner (through `RECIPE_RUNNER_RS_PATH`) stands in for the
//! drive. It reads the invocation path and state folder from its context
//! file, runs a nested `amplihack recipe run` that ends and runs its own
//! end-of-run cleanup (the nested run's `TMPDIR` is the shared one), and then
//! calls the same helper the finalize merge step calls:
//! `amplihack orch helper autodrive authorization`. The test asserts the
//! record sits in the durable run folder and the helper still reads it.
//!
//! Temporary 0700 homes and a stand-in runner only; no real login, tmux or
//! `~/.amplihack`. There is no sleep, timer or poll: the run ends when the
//! stand-in has finished.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PR: &str = "2959";

const RUNNER: &str = r#"#!/bin/sh
case "$1" in --capabilities) printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1","context_file_v1"]}'; exit 0 ;; esac
case "$1" in --version) echo 'recipe-runner-rs 0.0.0 (stub)'; exit 0 ;; esac
ok='{"recipe_name":"stub","success":true,"step_results":[],"context":{}}'
# The nested run reaches this same stub; it only has to end.
if [ -n "${STUB_NESTED:-}" ]; then printf '%s\n' "$ok"; exit 0; fi
rec="$RECORD_DIR"
prev=
for a in "$@"; do
  [ "$prev" = "--context-file" ] && ctx=$a
  prev=$a
done
inv=$(jq -r '.autodrive_invocation_file' "$ctx")
dir=$(jq -r '.autodrive_state_dir' "$ctx")
printf '%s' "$inv" > "$rec/inv.path"
printf '%s' "$dir" > "$rec/dir.path"
[ -f "$inv" ] && echo present > "$rec/before.state" || echo missing > "$rec/before.state"
# A nested recipe run ends, and runs its own end-of-run cleanup.
STUB_NESTED=1 "$AMPLIHACK_BIN" recipe run "$NESTED_RECIPE" -w "$WORK_REPO" > "$rec/nested.out" 2> "$rec/nested.err"
echo $? > "$rec/nested.rc"
[ -f "$inv" ] && echo present > "$rec/after.state" || echo missing > "$rec/after.state"
"$AMPLIHACK_BIN" orch helper autodrive authorization --invocation "$inv" --directory "$dir" --pr "$PR" \
  > "$rec/helper.out" 2> "$rec/helper.err"
echo $? > "$rec/helper.rc"
printf '%s\n' "$ok"
"#;

struct Run {
    out: Output,
    records: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Run {
    fn record(&self, name: &str) -> String {
        fs::read_to_string(self.records.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}\n{}", describe(&self.out)))
            .trim()
            .to_string()
    }
}

fn describe(o: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn drive() -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let [home, records, tools, work, repo, scratch] =
        ["home", "records", "tools", "work", "repo", "tmp"].map(|d| root.join(d));
    for d in [&home, &records, &tools, &work, &repo, &scratch] {
        private_dir(d);
    }
    // A trusted copy of the shipped bundle, not writable by group or others.
    let amplihack_home = home.join(".amplihack");
    private_dir(&amplihack_home);
    let copied = Command::new("cp")
        .arg("-R")
        .arg(repo_root().join("amplifier-bundle"))
        .arg(amplihack_home.join("amplifier-bundle"))
        .status()
        .unwrap();
    assert!(copied.success());
    let chmod = Command::new("chmod")
        .args(["-R", "go-w"])
        .arg(amplihack_home.join("amplifier-bundle"))
        .status()
        .unwrap();
    assert!(chmod.success());

    let runner = tools.join("recipe-runner-rs");
    fs::write(&runner, RUNNER).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
    let nested = repo.join("nested.yaml");
    fs::write(
        &nested,
        "name: nested-2959\ndescription: ends\nversion: 1.0.0\ncontext: {}\nsteps:\n  - id: end\n    type: bash\n    command: \"true\"\n",
    )
    .unwrap();
    let ctx = root.join("ctx.json");
    fs::write(
        &ctx,
        serde_json::json!({"no_merge": "false", "pr_number": PR}).to_string(),
    )
    .unwrap();
    fs::set_permissions(&ctx, fs::Permissions::from_mode(0o600)).unwrap();

    // `isolated` clears the environment, so no inherited session tree, depth,
    // login or profile variable reaches the stand-in drive: it is a root run
    // with only the isolation defaults and what is set below.
    let out = crate::isolated_amplihack::isolated(amplihack_bin())
        .args(["recipe", "run", "auto-drive-to-merge", "--context-file"])
        .arg(&ctx)
        .arg("-w")
        .arg(&repo)
        .current_dir(&repo)
        .env(
            "PATH",
            format!(
                "{}:{}",
                tools.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", &home)
        .env("TMPDIR", &scratch)
        .env("AMPLIHACK_HOME", &amplihack_home)
        .env("AMPLIHACK_BIN", amplihack_bin())
        .env("AMPLIHACK_WORK_ROOT", &work)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
        .env("RECORD_DIR", &records)
        .env("NESTED_RECIPE", &nested)
        .env("WORK_REPO", &repo)
        .env("PR", PR)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .env("AMPLIHACK_GOVERNOR", "off")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
        .stdin(Stdio::null())
        .output()
        .expect("run amplihack");
    Run {
        out,
        records,
        _tmp: tmp,
    }
}

/// The record sits in the run's durable folder, beside the state folder; the
/// finalize authorization read still succeeds after a nested recipe run has
/// ended and run its cleanup; and the record goes when its owner ends. On main
/// it sat in a loose `TMPDIR` folder, so the location assertion fails there.
#[test]
fn the_record_lives_in_the_durable_run_folder_for_the_whole_drive() {
    let run = drive();
    let inv = PathBuf::from(run.record("inv.path"));
    let dir = PathBuf::from(run.record("dir.path"));
    assert!(
        inv.parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("autodrive-invocation-"),
        "{}",
        inv.display()
    );
    assert_eq!(
        inv.parent().unwrap().parent(),
        dir.parent(),
        "the record sits in the same durable run folder as the state folder: {} vs {}",
        inv.display(),
        dir.display()
    );
    assert_eq!(run.record("before.state"), "present");
    assert_eq!(run.record("nested.rc"), "0", "{}", run.record("nested.err"));
    assert_eq!(
        run.record("after.state"),
        "present",
        "a nested run's cleanup must not remove the record"
    );
    assert_eq!(
        run.record("helper.rc"),
        "0",
        "the helper must read the record: {}",
        run.record("helper.err")
    );
    let answer: serde_json::Value = serde_json::from_str(&run.record("helper.out")).unwrap();
    assert_eq!(answer["allows_merge"], serde_json::json!(true), "{answer}");
    assert_eq!(answer["pr_selected"], serde_json::json!(true), "{answer}");
    assert!(
        !inv.exists(),
        "the record must not outlive its run: {}",
        inv.display()
    );
}
