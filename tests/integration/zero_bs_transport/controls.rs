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
    f.shim("jq",r#"if [ "$FAIL_TOOL" = summary-write ] && [ "${3:-}" = report_path ]; then exit 15; fi
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
    failure(
        &f,
        r#"printf() { if [ "$1" = '%s\n' ]; then return 13; fi; builtin printf "$@"; }"#,
        "",
        13,
    );
    f.shim("chmod", "exit 16");
    failure(&f, "", "", 16);
}
#[test]
#[cfg(target_os = "linux")]
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
    for mask in ["000", "002", "022", "077"] {
        report(&f, &receipt(&f.run_with(&format!("umask {mask}"), "")));
    }
    let checks = fs::read_to_string(f.temp.path().join("tmp/permission-checks")).unwrap();
    assert_eq!(checks, "checked-before-payload".repeat(24));
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
    assert_eq!(report(&f, &a), report(&f, &b));
}
#[test]
fn no_match_missing_repository_and_worktree_fallback_remain_truthful() {
    let f = Fixture::new();
    f.write("clean.rs", b"fn main() {}\n");
    assert!(f.git(&["add", "."]).status.success());
    let j = receipt(&f.run());
    let data = report(&f, &j);
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
        .env(
            "RECIPE_VAR_worktree_setup__worktree_path",
            "/missing/worktree",
        )
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
    report(&f, &j);
    let alias = f.temp.path().join("root-alias");
    std::os::unix::fs::symlink(f.temp.path().join("tmp"), &alias).unwrap();
    for selected in [
        PathBuf::from("/tmp"),
        alias,
        f.temp.path().join("space : $(touch PWNED)`touch PWNED`\n"),
    ] {
        if !selected.exists() {
            fs::create_dir(&selected).unwrap();
        }
        let out = f
            .child(&f.shell)
            .args(["-c", step_command("workflow-pr-review", STEP)])
            .env("TMPDIR", &selected)
            .env("ZERO_BS_REPORT_PATH", &foreign)
            .output()
            .unwrap();
        let j = receipt(&out);
        report_in_root(&f, &selected, &j);
        assert_eq!(fs::read(&foreign).unwrap(), b"unchanged");
        fs::remove_dir_all(
            PathBuf::from(j["report_path"].as_str().unwrap())
                .parent()
                .unwrap(),
        )
        .unwrap();
    }
    let out = f
        .child(&f.shell)
        .args(["-c", step_command("workflow-pr-review", STEP)])
        .env_remove("TMPDIR")
        .output()
        .unwrap();
    let j = receipt(&out);
    report_in_root(&f, std::path::Path::new("/tmp"), &j);
    fs::remove_dir_all(
        PathBuf::from(j["report_path"].as_str().unwrap())
            .parent()
            .unwrap(),
    )
    .unwrap();
    let locked = f.temp.path().join("unusable");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let out = f
        .child(&f.shell)
        .args(["-c", step_command("workflow-pr-review", STEP)])
        .env("TMPDIR", &locked)
        .output()
        .unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    // Root can bypass Unix access permissions; ordinary callers must fail.
    if fs::metadata(f.temp.path()).unwrap().uid() != 0 {
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }
    let file = f.temp.path().join("not-directory");
    fs::write(&file, b"root sentinel").unwrap();
    for selected in [PathBuf::new(), f.temp.path().join("missing"), file.clone()] {
        let out = f
            .child(&f.shell)
            .args(["-c", step_command("workflow-pr-review", STEP)])
            .env("TMPDIR", selected)
            .env("ZERO_BS_REPORT_PATH", &foreign)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert_eq!(fs::read(&foreign).unwrap(), b"unchanged");
    }
    assert_eq!(fs::read(file).unwrap(), b"root sentinel");
    assert!(!f.repo().join("PWNED").exists());
}

#[test]
#[cfg(target_os = "linux")]
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

#[test]
fn known_records_have_independent_filename_line_and_content_framing() {
    let f = Fixture::new();
    f.write(
        "tracked: space\n.rs",
        b"clean\nTODO tracked\ntodo!(); // TODO overlap\n",
    );
    assert!(f.git(&["add", "."]).status.success());
    f.write(
        "-untracked: space\n.md",
        b"clean\nTODO \xff\x00 FINAL_MARKER\n",
    );
    let tracked = b"tracked: space\n.rs\x002\x00TODO tracked\ntracked: space\n.rs\x003\x00todo!(); // TODO overlap\n";
    let stub = b"tracked: space\n.rs\x003\x00todo!(); // TODO overlap\n";
    let untracked = b"-untracked: space\n.md\x002:TODO \xff\x00 FINAL_MARKER\n";
    assert_eq!(expected(&f, 0, true), tracked);
    assert_eq!(expected(&f, 0, false), untracked);
    let j = receipt(&f.run());
    let data = report(&f, &j);
    let mut full = b"=== ZERO-BS VERIFICATION ===\nFormat: NUL-delimited filenames; tracked line numbers also NUL-delimited. Complete matching lines follow.\n".to_vec();
    for (i, label) in LABELS.iter().enumerate() {
        full.extend_from_slice(format!("\n--- {label} ---\n").as_bytes());
        if i == 0 {
            full.extend_from_slice(tracked);
            full.extend_from_slice(untracked);
        } else if i == 1 {
            full.extend_from_slice(stub);
        } else {
            full.extend_from_slice(b"None found\n");
        }
        assert_eq!(
            j["categories"][i]["tracked_count"],
            if i == 0 {
                2
            } else if i == 1 {
                1
            } else {
                0
            }
        );
        assert_eq!(
            j["categories"][i]["untracked_count"],
            if i == 0 { 1 } else { 0 }
        );
    }
    full.extend_from_slice(b"\n=== Zero-BS Verification Complete ===\n");
    assert_eq!(data, full);
}
