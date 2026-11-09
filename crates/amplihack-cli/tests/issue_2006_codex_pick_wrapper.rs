//! Issue #2006: `scripts/codex-pick` is a thin POSIX `sh` wrapper that runs
//! `exec amplihack account pick --cli codex "$@"` with
//! `AMPLIHACK_ACCOUNT_PICK_CALLER=codex-pick`
//! (docs/reference/account-pick.md, "The codex-pick wrapper").
//!
//! A fake `amplihack` on `PATH` records each argument on its own line and
//! the caller variable, so quoting is checked exactly.

#![cfg(unix)]

use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FAKE_AMPLIHACK: &str = r#"#!/bin/sh
: > "$RECORD/args"
for a in "$@"; do printf '%s\n' "$a" >> "$RECORD/args"; done
printf '%s\n' "${AMPLIHACK_ACCOUNT_PICK_CALLER-<unset>}" > "$RECORD/caller"
exit "${FAKE_EXIT:-0}"
"#;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/codex-pick")
}

struct Wx {
    _tmp: tempfile::TempDir,
    bin: PathBuf,
    record: PathBuf,
}

impl Wx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let record = tmp.path().join("record");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&record).unwrap();
        write_executable(&bin.join("amplihack"), FAKE_AMPLIHACK, 0o755);
        Self {
            _tmp: tmp,
            bin,
            record,
        }
    }

    fn run(&self, args: &[&str], exit: u8) -> std::process::Output {
        Command::new("/bin/sh")
            .arg(script())
            .args(args)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("RECORD", &self.record)
            .env("FAKE_EXIT", exit.to_string())
            .output()
            .unwrap()
    }

    fn args(&self) -> Vec<String> {
        fs::read_to_string(self.record.join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn the_wrapper_exists_and_is_posix_sh_without_eval() {
    let body = fs::read_to_string(script()).expect("scripts/codex-pick exists");
    let first = body.lines().next().unwrap_or_default();
    assert!(
        first == "#!/bin/sh" || first == "#!/usr/bin/env sh",
        "not a POSIX sh script: {first}"
    );
    assert!(body.contains("set -eu"), "{body}");
    assert!(
        body.contains(r#"exec amplihack account pick --cli codex "$@""#),
        "{body}"
    );
    assert!(!body.contains("eval"), "the wrapper must not eval");
    assert!(!body.contains("python"), "the wrapper must not need python");
    assert!(!body.contains("jq"), "the wrapper must not need jq");
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(script()).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0, "scripts/codex-pick is not executable");
}

#[test]
fn floor_and_list_pass_through_unchanged() {
    let w = Wx::new();
    let out = w.run(&["--floor", "10", "--list"], 0);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        w.args(),
        [
            "account", "pick", "--cli", "codex", "--floor", "10", "--list"
        ]
    );
    assert_eq!(
        fs::read_to_string(w.record.join("caller"))
            .unwrap()
            .trim_end(),
        "codex-pick"
    );
}

#[test]
fn arguments_keep_their_spaces_and_metacharacters() {
    let w = Wx::new();
    let out = w.run(&["a b", "$(touch pwned)", "*", ""], 0);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        w.args(),
        [
            "account",
            "pick",
            "--cli",
            "codex",
            "a b",
            "$(touch pwned)",
            "*",
            ""
        ]
    );
    assert!(!Path::new("pwned").exists());
}

#[test]
fn the_exit_status_is_amplihacks() {
    let w = Wx::new();
    assert_eq!(w.run(&[], 1).status.code(), Some(1));
    assert_eq!(w.run(&[], 2).status.code(), Some(2));
}
