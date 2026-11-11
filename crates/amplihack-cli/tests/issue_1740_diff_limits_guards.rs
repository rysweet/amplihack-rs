//! Issue #1740: the checks in `quick_change_diff_limits.sh` that fail closed
//! or stop the run: git configuration and hook drift (exit 4), an empty diff
//! (exit 3), bad arguments and a missing snapshot (exit 2), and the agent
//! commit, mode and symlink rules that a size limit must not hide. Also the
//! pinned-base cross-check in `quick_change_common.sh` (`qc_base`).

use crate::support::quick_change_repo::{Repo, git, handover, stderr, verdict};

#[test]
fn exec_bit_and_symlink_changes_hand_over() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    std::fs::set_permissions(
        r.wt.join("README.md"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    handover(&r, "mode-or-symlink");

    let r = Repo::new();
    std::os::unix::fs::symlink("README.md", r.wt.join("LINK.md")).unwrap();
    handover(&r, "mode-or-symlink");
}

#[test]
fn an_agent_commit_hands_over() {
    let r = Repo::new();
    r.write("README.md", "# demo, committed\n");
    git(&r.wt, &["commit", "-q", "-am", "agent"]);
    handover(&r, "agent-commit");
}

#[test]
fn empty_diff_stops_with_exit_3() {
    let r = Repo::new();
    let j = verdict(
        &r.measure(&[]),
        3,
        "QUICK_CHANGE_VERDICT=empty reason=empty-diff",
    );
    assert_eq!(j["verdict"], "empty");
}

#[test]
fn git_config_drift_stops_before_any_count() {
    let r = Repo::new();
    for i in 0..10 {
        r.write(&format!("docs/{i}.md"), "x\n");
    }
    git(
        &r.wt,
        &[
            "config",
            "url.https://evil.invalid/.insteadOf",
            "https://github.com/",
        ],
    );
    let j = verdict(
        &r.measure(&[]),
        4,
        "QUICK_CHANGE_VERDICT=stop reason=git-config-drift",
    );
    assert_eq!(j["verdict"], "stop");
    let text = j.to_string();
    assert!(
        !text.contains("evil.invalid"),
        "drift JSON must not print values: {text}"
    );
}

#[test]
fn hook_or_remote_drift_stops_even_with_an_empty_diff() {
    let r = Repo::new();
    crate::support::exec_stub::write_executable(
        &r.wt.join(".git/hooks/pre-push"),
        "#!/bin/sh\nexit 0\n",
        0o755,
    );
    verdict(
        &r.measure(&[]),
        4,
        "QUICK_CHANGE_VERDICT=stop reason=git-config-drift",
    );

    let r = Repo::new();
    r.write("README.md", "# changed\n");
    git(
        &r.wt,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            "/tmp/elsewhere.git",
        ],
    );
    verdict(
        &r.measure(&[]),
        4,
        "QUICK_CHANGE_VERDICT=stop reason=git-config-drift",
    );
}

#[test]
fn missing_snapshot_fails_closed() {
    let r = Repo::new();
    std::fs::remove_file(r.state.join("git-config.snapshot")).unwrap();
    r.write("README.md", "# changed\n");
    assert_eq!(r.measure(&[]).status.code(), Some(2));
}

#[test]
fn bad_arguments_are_faults() {
    let r = Repo::new();
    r.write("README.md", "# changed\n");
    let wt = r.wt.to_str().unwrap();
    let unrelated = "0123456789abcdef0123456789abcdef01234567";
    for args in [
        vec![wt, "--base", "HEAD~0"],
        vec![wt, "--base", unrelated],
        vec![wt, "--base", r.base.as_str(), "--max-files", "abc"],
        vec![wt, "--base", r.base.as_str(), "--max-lines", "1234567"],
        vec![],
    ] {
        let o = r.run(&args, true);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
    }
}

#[test]
fn global_proxy_or_clean_filter_is_drift() {
    for (key, value) in [
        ("http.proxy", "http://proxy.invalid:3128"),
        ("filter.x.clean", "exfiltrate-to-proxy.invalid"),
    ] {
        let r = Repo::new();
        r.write("README.md", "# changed\n");
        let global = r.tmp.path().join(".gitconfig");
        git(
            &r.wt,
            &["config", "--file", global.to_str().unwrap(), key, value],
        );
        let j = verdict(
            &r.measure(&[]),
            4,
            "QUICK_CHANGE_VERDICT=stop reason=git-config-drift",
        );
        let text = j.to_string();
        assert!(!text.contains(".invalid"), "{key}: values leaked: {text}");
    }
}

#[test]
fn unreadable_git_config_is_a_fault_in_both_modes() {
    let r = Repo::new();
    r.write("README.md", "# changed\n");
    std::fs::write(r.tmp.path().join(".gitconfig"), "[broken\n").unwrap();
    let o = r.measure(&[]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    let o = r.run(&["--snapshot", r.wt.to_str().unwrap()], true);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(o.stdout.is_empty(), "no partial snapshot on a fault");
}

/// Records `base` through `qc_write`, as quick-change-record-base does,
/// optionally overwrites the stored file, then runs `qc_base`.
fn qc_base(r: &Repo, tamper: Option<&str>, runner_value: &str) -> std::process::Output {
    let script = "printf '%s\\n' \"$PINNED\" | qc_write base\n\
                  [ -z \"${TAMPER:-}\" ] || printf '%s\\n' \"$TAMPER\" > \"$QUICK_CHANGE_STATE_DIR/base\"\n\
                  qc_base";
    r.common(
        script,
        &[
            ("PINNED", r.base.as_str()),
            ("TAMPER", tamper.unwrap_or("")),
            ("RECIPE_VAR_quick_change_base", runner_value),
        ],
    )
}

#[test]
fn qc_base_prints_the_base_when_both_records_agree() {
    let r = Repo::new();
    let o = qc_base(&r, None, &r.base);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout), r.base);
}

#[test]
fn qc_base_dies_when_the_stored_base_was_rewritten() {
    let r = Repo::new();
    r.write("README.md", "# hidden work\n");
    r.commit_all("hidden");
    let later = git(&r.wt, &["rev-parse", "HEAD"]);
    let o = qc_base(&r, Some(&later), &r.base);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
    assert!(
        stderr(&o).contains("differs from the recorded base"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn qc_base_dies_when_the_runner_value_is_missing() {
    let r = Repo::new();
    let o = qc_base(&r, None, "");
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
    assert!(
        stderr(&o).contains("runner did not provide quick_change_base"),
        "{}",
        stderr(&o)
    );
}
