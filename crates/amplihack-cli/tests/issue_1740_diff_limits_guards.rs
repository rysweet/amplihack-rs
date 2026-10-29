//! Issue #1740: the checks in `quick_change_diff_limits.sh` that fail closed
//! or stop the run: git configuration and hook drift (exit 4), an empty diff
//! (exit 3), bad arguments and a missing snapshot (exit 2), and the agent
//! commit, mode and symlink rules that a size limit must not hide.

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
