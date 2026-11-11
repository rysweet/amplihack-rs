//! Issue #1740: `quick_change_diff_limits.sh` under different `awk`
//! implementations, and with an `awk` that fails.
//!
//! The public-interface and CLI rules read each changed `.rs` file's diff
//! with `awk`. mawk, gawk and busybox process `awk -v` escapes differently,
//! and some awks exit 1 on a pattern they cannot compile, which once read as
//! "no match" and let a CLI change proceed (PR #1758, round 2). Each test
//! puts a shim named `awk` first on `PATH`; none of them needs gawk or
//! busybox to be installed, and the per-implementation test runs every awk
//! it finds.

use std::path::{Path, PathBuf};

use crate::support::exec_stub::write_executable;
use crate::support::quick_change_repo::{Repo, stderr};

/// The first executable called `name` on the test's own `PATH`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// A directory holding an executable `awk` with `body` as its script.
fn awk_shim(r: &Repo, label: &str, body: &str) -> PathBuf {
    let dir = r.tmp.path().join(format!("shim-{label}"));
    std::fs::create_dir_all(&dir).unwrap();
    write_executable(&dir.join("awk"), format!("#!/bin/sh\n{body}\n"), 0o755);
    dir
}

/// Each installed awk, as the `exec` line of a shim. The awk the tests
/// would otherwise use is always included, so the list is never empty.
fn installed_awks() -> Vec<(String, String)> {
    let mut found = Vec::new();
    if let Some(awk) = find_on_path("awk") {
        let real = std::fs::canonicalize(&awk).unwrap_or(awk);
        found.push((
            format!("awk ({})", real.display()),
            format!("exec '{}' \"$@\"", real.display()),
        ));
    }
    for name in ["gawk", "mawk", "original-awk", "nawk"] {
        if let Some(p) = find_on_path(name) {
            found.push((name.to_string(), format!("exec '{}' \"$@\"", p.display())));
        }
    }
    if let Some(p) = find_on_path("busybox") {
        found.push((
            "busybox awk".to_string(),
            format!("exec '{}' awk \"$@\"", p.display()),
        ));
    }
    found
}

fn measure_line1(r: &Repo, dir: &Path) -> (Option<i32>, String, String) {
    let o = r.measure_with_path(dir);
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    (
        o.status.code(),
        out.lines().next().unwrap_or("").to_string(),
        stderr(&o),
    )
}

#[test]
fn cli_and_public_interface_rules_hold_under_every_installed_awk() {
    let cases = [
        (
            "fn private() {}\n\npub fn kept() {}\n#[arg(long)]\n",
            "QUICK_CHANGE_VERDICT=handover reason=cli-definition",
        ),
        (
            "fn private() {}\n\npub fn kept() {}\n#[command(name = \"x\")]\n",
            "QUICK_CHANGE_VERDICT=handover reason=cli-definition",
        ),
        (
            "fn private() {}\n\npub fn kept() {}\npub fn added() {}\n",
            "QUICK_CHANGE_VERDICT=handover reason=public-interface",
        ),
        (
            "fn private_renamed() {}\n\npub fn kept() {}\n",
            "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
        ),
    ];
    let awks = installed_awks();
    assert!(!awks.is_empty(), "no awk on PATH");
    for (label, exec) in &awks {
        eprintln!("checking the Rust rules under {label}");
        for (body, want) in cases {
            let r = Repo::new();
            r.write("src/lib.rs", body);
            let dir = awk_shim(&r, "impl", exec);
            let (code, line1, err) = measure_line1(&r, &dir);
            assert_eq!(code, Some(0), "{label}: {err}");
            assert_eq!(line1, want, "{label}, body {body:?}: {err}");
        }
    }
}

#[test]
fn a_failing_matcher_awk_is_a_fault_not_proceed() {
    // The matcher is the only awk call that sets QC_MATCH_RE, the variable
    // the pattern travels in (docs/reference/quick-change-recipe.md, "CLI
    // definitions"). Other awk calls reach the real awk, so each failure
    // below is seen by the matcher alone. Without the fault handling, each
    // of these change sets would proceed: the change is private.
    let real = find_on_path("awk").expect("awk on PATH");
    let failures = [
        ("bad-regex-exit-1", "echo \"awk: bad regex\" >&2; exit 1"),
        ("error-exit-2", "echo \"awk: fatal\" >&2; exit 2"),
        ("silent-exit-0", "exit 0"),
        ("wrong-output", "echo maybe; exit 0"),
    ];
    for (label, failure) in failures {
        let r = Repo::new();
        r.write(
            "src/lib.rs",
            "fn private_renamed() {}\n\npub fn kept() {}\n",
        );
        let body = format!(
            "if [ -n \"${{QC_MATCH_RE+set}}\" ]; then {failure}; fi\nexec '{}' \"$@\"",
            real.display()
        );
        let dir = awk_shim(&r, label, &body);
        let (code, line1, err) = measure_line1(&r, &dir);
        assert_eq!(code, Some(2), "{label}: line 1 {line1:?}, stderr {err}");
        assert!(
            line1.is_empty(),
            "{label}: no verdict on a fault, got {line1:?}"
        );
        assert!(err.contains("while matching a Rust file"), "{label}: {err}");
    }
}

#[test]
fn a_broken_awk_is_a_fault_in_both_modes() {
    // An awk that always fails leaves every snapshot checksum empty. Two
    // such snapshots would compare equal and hide drift, so the snapshot
    // itself faults, and the measuring run stops before any rule.
    let r = Repo::new();
    r.write(
        "src/lib.rs",
        "fn private_renamed() {}\n\npub fn kept() {}\n",
    );
    let dir = awk_shim(&r, "broken", "echo \"awk: broken\" >&2\nexit 2");
    let (code, line1, err) = measure_line1(&r, &dir);
    assert_eq!(code, Some(2), "line 1 {line1:?}, stderr {err}");
    assert!(line1.is_empty(), "no verdict on a fault, got {line1:?}");
    assert!(err.contains("without a checksum"), "{err}");

    let path = std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let o = crate::isolated_amplihack::isolated("bash")
        .arg(crate::support::quick_change_repo::tool())
        .args(["--snapshot", r.wt.to_str().unwrap()])
        .env("HOME", r.tmp.path())
        .env("PATH", path)
        .output()
        .expect("run --snapshot");
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(o.stdout.is_empty(), "no partial snapshot on a fault");
}

#[test]
fn the_old_backslash_pattern_is_not_used() {
    // gawk and busybox drop the backslash from `\[` in an `awk -v` value,
    // which leaves an unterminated bracket expression. The tool must pass
    // the pattern through the environment and spell `[` without one.
    let script = std::fs::read_to_string(crate::support::quick_change_repo::tool()).unwrap();
    assert!(
        script.contains("CLI_RE='#[[](arg|command|clap)'"),
        "CLI_RE must use a bracket expression for the literal ["
    );
    // The script's comments name `awk -v` to explain why it is not used, so
    // only command lines are checked.
    let offenders: Vec<&str> = script
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter(|line| line.contains("awk -v") || line.contains("awk' -v"))
        .collect();
    assert!(
        offenders.is_empty(),
        "no pattern may reach awk through -v: {offenders:?}"
    );
}
