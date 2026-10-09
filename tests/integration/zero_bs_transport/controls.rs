use super::*;

fn failure(f: &Fixture, prefix: &str, tool: &str, expected: i32) -> Output {
    let out = f.run_with(prefix, tool);
    assert_eq!(
        out.status.code(),
        Some(expected),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "incomplete scans cannot emit a success receipt"
    );
    out
}
#[test]
fn tracked_untracked_and_discovery_errors_keep_actual_exits_and_diagnostics() {
    for (tool, code) in [("tracked", 23), ("untracked", 24), ("discovery", 25)] {
        let f = Fixture::new();
        f.large();
        f.shim("git",r#"if [ "$1" = grep ] && [ "$FAIL_TOOL" = tracked ]; then echo tracked-operational-diagnostic >&2; exit 23; fi
if [ "$1" = ls-files ] && [ "$FAIL_TOOL" = discovery ]; then echo discovery-operational-diagnostic >&2; exit 25; fi
exec /usr/bin/git "$@""#);
        f.shim("grep",r#"if [ "$FAIL_TOOL" = untracked ]; then echo untracked-operational-diagnostic >&2; exit 24; fi
exec /usr/bin/grep "$@""#);
        failure(&f, "", tool, code);
        let paths: Vec<_> = fs::read_dir(f.temp.path().join("tmp"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect();
        assert_eq!(paths.len(), 1);
        let data = fs::read(paths[0].join("report")).unwrap();
        assert!(String::from_utf8_lossy(&data).contains(&format!("{tool}-operational-diagnostic")));
        assert!(String::from_utf8_lossy(&data).contains(&format!("exit {code}")));
    }
}
#[test]
fn allocation_payload_write_final_read_and_presentation_fail_loudly() {
    let f = Fixture::new();
    f.large();
    f.shim(
        "mktemp",
        r#"if [ "$FAIL_TOOL" = allocation ]; then exit 9; fi
exec /usr/bin/mktemp "$@""#,
    );
    f.shim("cat",r#"case "$FAIL_TOOL:$1" in read:*/report) exit 12;; presentation:*/summary.json) exit 14;; append:*/scan.*) exit 11;; esac
exec /usr/bin/cat "$@""#);
    f.shim("jq",r#"if [ "$FAIL_TOOL" = summary-write ] && [ "${2:-}" = --arg ]; then exit 15; fi
if [ "$FAIL_TOOL" = oversized ] && [ "${3:-}" = report_path ]; then /usr/bin/head -c 8193 /dev/zero; exit 0; fi
exec /usr/bin/jq "$@""#);
    for (name, code) in [
        ("allocation", 9),
        ("read", 12),
        ("presentation", 14),
        ("append", 11),
        ("summary-write", 15),
        ("oversized", 1),
    ] {
        failure(&f, "", name, code);
    }
    failure(&f, "printf() { return 13; }", "", 13);
    f.shim("chmod", "exit 16");
    failure(&f, "", "", 16);
}
#[test]
fn private_permissions_exist_before_scan_payload_writes() {
    let f = Fixture::new();
    f.large();
    f.shim(
        "git",
        r#"if [ "$1" = grep ]; then
raw=$(/usr/bin/readlink /proc/$$/fd/1)
[ "$(/usr/bin/stat -c %a "$raw")" = 600 ] || exit 98
[ "$(/usr/bin/stat -c %a "${raw%/*}")" = 700 ] || exit 98
[ "$(/usr/bin/stat -c %a "${raw%/*}/report")" = 600 ] || exit 98
printf checked-before-payload >> "$TMPDIR/permission-checks"
fi
exec /usr/bin/git "$@""#,
    );
    report(&receipt(&f.run()));
    let checks = fs::read_to_string(f.temp.path().join("tmp/permission-checks")).unwrap();
    assert_eq!(checks, "checked-before-payload".repeat(6));
}
#[test]
fn concurrent_invocations_retain_distinct_complete_artifacts() {
    use std::process::Stdio;
    let f = Fixture::new();
    f.large();
    let spawn = || {
        f.child(&f.shell)
            .args(["-c", step_command("workflow-pr-review", STEP)])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let a = spawn();
    let b = spawn();
    let a = receipt(&a.wait_with_output().unwrap());
    let b = receipt(&b.wait_with_output().unwrap());
    assert_ne!(a["report_path"], b["report_path"]);
    assert_eq!(report(&a), report(&b));
}
#[test]
fn no_match_missing_repository_and_worktree_fallback_remain_truthful() {
    let f = Fixture::new();
    f.write("clean.rs", b"fn main() {}\n");
    assert!(f.git(&["add", "."]).status.success());
    let j = receipt(&f.run());
    let data = report(&j);
    assert_eq!(j["status"], "NO_FINDINGS");
    assert_eq!(
        String::from_utf8_lossy(&data).matches("None found").count(),
        6
    );
    for category in j["categories"].as_array().unwrap() {
        assert_eq!(category["count"], 0);
        assert_eq!(category["result"], "NO_FINDINGS");
    }
    let out = f
        .child(&f.shell)
        .args(["-c", step_command("workflow-pr-review", STEP)])
        .env("WORKTREE_SETUP_WORKTREE_PATH", "/missing/worktree")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("falling back to REPO_PATH"));
    receipt(&out);
    fs::remove_dir_all(f.repo().join(".git")).unwrap();
    let out = failure(&f, "", "", 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("requires a git repo"));
}
#[test]
fn physical_root_is_required_and_supplied_artifacts_are_never_adopted() {
    let f = Fixture::new();
    f.write("clean.rs", b"fn main() {}\n");
    let foreign = f.temp.path().join("foreign");
    fs::write(&foreign, b"unchanged").unwrap();
    let out = f
        .child(&f.shell)
        .args(["-c", step_command("workflow-pr-review", STEP)])
        .env("ZERO_BS_REPORT_PATH", &foreign)
        .output()
        .unwrap();
    let j = receipt(&out);
    assert_ne!(j["report_path"], foreign.to_str().unwrap());
    assert_eq!(fs::read(&foreign).unwrap(), b"unchanged");
    let out = f
        .child(&f.shell)
        .args(["-c", step_command("workflow-pr-review", STEP)])
        .env("TMPDIR", "/tmp")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

#[test]
fn kernel_write_failure_cannot_report_scan_success() {
    let f = Fixture::new();
    let out = failure(
        &f,
        r#"printf() {
case "$(/usr/bin/readlink /proc/$$/fd/1)" in */report) builtin printf "$@" >/dev/full;; *) builtin printf "$@";; esac
}"#,
        "",
        1,
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("No space left on device"));
}
