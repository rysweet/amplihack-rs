//! Wrapper-level behaviour after the Rust port (#1554): the shell tools call
//! `"$AMPLIHACK_BIN" internal workflow-checkpoint <action>`, keep its status,
//! and never run python3.
//!
//! Rows here drive the real scripts in `amplifier-bundle/tools` against local
//! bare remotes, with a `git` shim only where a broken remote is the point.

use std::fs;

use crate::workflow_checkpoint_support::*;

fn pr_head_sync(sb: &Sandbox, work: &std::path::Path, args: &[&str]) -> std::process::Output {
    sb.command("bash")
        .arg(tools_dir().join("workflow_pr_head_sync.sh"))
        .args(args)
        .current_dir(work)
        .output()
        .unwrap()
}

const EARLY: [&str; 2] = ["--strict-fetch", "--probe-origin-ref"];

// ------------------------------------------------ pr-head-sync status matrix

#[test]
fn early_sync_skips_only_when_a_valid_advertisement_lacks_the_task_ref() {
    let sb = Sandbox::new();
    let (work, bare) = repo_with_origin(&sb);
    let before = destination_snapshot(&sb, &bare);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("origin task ref is absent"),
        "{}",
        stderr(&out)
    );
    assert_eq!(before, destination_snapshot(&sb, &bare));
}

#[test]
fn early_sync_fetches_when_the_task_ref_is_present() {
    let sb = Sandbox::new();
    let (work, bare) = repo_with_origin(&sb);
    git(&sb, &work, &["push", "-q", "origin", "feat/x"]);
    let before = destination_snapshot(&sb, &bare);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(!stderr(&out).contains("origin task ref is absent"));
    assert_eq!(before, destination_snapshot(&sb, &bare));
}

#[test]
fn early_sync_stops_with_the_transport_status_when_git_exits_3_and_never_skips() {
    // 918c: the shell rejects any ls-remote status other than 0 and 2 before
    // parsing and exits with it, so a git that exits 3 is never "absent".
    let sb = Sandbox::new();
    let (work, bare) = repo_with_origin(&sb);
    let before = destination_snapshot(&sb, &bare);
    sb.git_shim(b"", 3);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 3, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("origin ref inspection failed (exit 3)"));
    assert!(!stderr(&out).contains("origin task ref is absent"));
    assert_eq!(before, destination_snapshot(&sb, &bare));
}

#[test]
fn early_sync_accepts_a_final_record_without_a_line_feed() {
    // D1: ORIGIN_REF_PY split records with bytes.splitlines(), so a final
    // record without LF is accepted; here it lacks the task ref.
    let sb = Sandbox::new();
    let (work, bare) = repo_with_origin(&sb);
    let before = destination_snapshot(&sb, &bare);
    sb.git_shim(format!("{}\trefs/heads/main", oid1('a')).as_bytes(), 0);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("origin task ref is absent"),
        "{}",
        stderr(&out)
    );
    assert_eq!(before, destination_snapshot(&sb, &bare));
}

#[test]
fn early_sync_stops_with_78_on_transport_0_and_a_malformed_body() {
    let sb = Sandbox::new();
    let (work, _) = repo_with_origin(&sb);
    sb.git_shim(b"<html>proxy error</html>\n", 0);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 78, "stderr: {}", stderr(&out));
}

#[test]
fn early_sync_passes_79_through_unchanged() {
    let sb = Sandbox::new();
    let (work, bare) = repo_with_origin(&sb);
    let before = destination_snapshot(&sb, &bare);
    sb.git_shim(format!("{}\trefs/heads/feat/x\n", oid1('a')).as_bytes(), 79);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 79, "stderr: {}", stderr(&out));
    assert!(!stderr(&out).contains("origin task ref is absent"));
    assert_eq!(before, destination_snapshot(&sb, &bare));
}

#[test]
fn early_sync_does_not_treat_a_prefix_or_case_variant_as_present() {
    // The skip needs the exact task ref to be absent; a sibling must not make
    // it look present (fetch), and must not be mistaken for it either.
    for sibling in ["feat/x2", "Feat/X", "feat"] {
        let sb = Sandbox::new();
        let (work, _) = repo_with_origin(&sb);
        git(
            &sb,
            &work,
            &[
                "push",
                "-q",
                "origin",
                &format!("HEAD:refs/heads/{sibling}"),
            ],
        );
        let out = pr_head_sync(&sb, &work, &EARLY);
        assert_eq!(status(&out), 0, "{sibling}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("origin task ref is absent"),
            "{sibling} was matched as the task ref"
        );
    }
}

#[test]
fn early_sync_ignores_the_ref_under_another_remote() {
    let sb = Sandbox::new();
    let (work, bare) = repo_with_origin(&sb);
    let other = sb.path().join("other.git");
    git(
        &sb,
        sb.path(),
        &["init", "--bare", "-q", other.to_str().unwrap()],
    );
    git(
        &sb,
        &work,
        &[
            "remote",
            "add",
            "upstream",
            &format!("file://{}", other.display()),
        ],
    );
    git(&sb, &work, &["push", "-q", "upstream", "feat/x"]);
    let before = destination_snapshot(&sb, &bare);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("origin task ref is absent"));
    assert_eq!(before, destination_snapshot(&sb, &bare));
}

// ---------------------------------------------------- binary resolution

#[test]
fn a_missing_amplihack_bin_stops_the_wrapper_and_never_skips_the_check() {
    let sb = Sandbox::new();
    let (work, _) = repo_with_origin(&sb);
    let out = sb
        .command("bash")
        .arg(tools_dir().join("workflow_pr_head_sync.sh"))
        .args(EARLY)
        .current_dir(&work)
        .env("AMPLIHACK_BIN", sb.path().join("no-such-amplihack"))
        .output()
        .unwrap();
    assert_ne!(status(&out), 0, "the wrapper ran without its engine");
    assert!(stderr(&out).contains("AMPLIHACK_BIN"), "{}", stderr(&out));
    assert!(!stderr(&out).contains("origin task ref is absent"));
}

// ------------------------------------------------------- TMPDIR (step20b)

#[test]
fn pr_head_sync_requires_an_assigned_tmpdir() {
    let sb = Sandbox::new();
    let (work, _) = repo_with_origin(&sb);
    let out = sb
        .command("bash")
        .arg(tools_dir().join("workflow_pr_head_sync.sh"))
        .arg("--strict-fetch")
        .current_dir(&work)
        .env_remove("TMPDIR")
        .output()
        .unwrap();
    assert_ne!(status(&out), 0);
    assert!(
        stderr(&out).contains("assigned TMPDIR required"),
        "{}",
        stderr(&out)
    );
}

// ------------------------------------------------------------ no python3

#[test]
fn cleanup_capture_skip_prints_the_python_bytes_without_python3() {
    let sb = Sandbox::new();
    let marker = sb.python3_marker_shim();
    let empty = sb.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    let out = sb
        .command("bash")
        .arg(tools_dir().join("workflow_cleanup_checkpoint.sh"))
        .args(["--scope-capture", "cleanup"])
        .current_dir(&empty)
        .env("AMPLIHACK_HOME", &empty)
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        out.stdout,
        b"{\"skipped\": true, \"admitted\": false, \"reason\": \"worktree_setup_path.sh not found\"}\n"
    );
    assert!(
        !marker.exists(),
        "python3 was called: {:?}",
        fs::read_to_string(&marker)
    );
}

#[test]
fn pr_head_sync_early_probe_runs_without_python3() {
    let sb = Sandbox::new();
    let marker = sb.python3_marker_shim();
    let (work, _) = repo_with_origin(&sb);
    let out = pr_head_sync(&sb, &work, &EARLY);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(!marker.exists(), "python3 was called");
}

#[test]
fn publish_checkpoint_admission_refusal_prints_commit_result_without_python3() {
    // An inherited selector is refused by `selectors`; the wrapper prints the
    // commit_result line via admission-refused-result and exits with the
    // refusal status, with python3 unavailable.
    let sb = Sandbox::new();
    let marker = sb.python3_marker_shim();
    let (work, _) = repo_with_origin(&sb);
    let out = sb
        .command("bash")
        .arg(tools_dir().join("workflow_publish_checkpoint.sh"))
        .current_dir(&work)
        .env("GIT_NAMESPACE", "x")
        .env("RECIPE_VAR_worktree_setup", r#"{"branch_name":"feat/x"}"#)
        .output()
        .unwrap();
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert_eq!(
        out.stdout,
        b"{\"pushed\": \"false\", \"sha\": \"\", \"branch\": \"feat/x\", \"reason\": \"scope-admission-refused\"}\n"
    );
    assert!(!marker.exists(), "python3 was called");
}

#[test]
fn stage_changes_tool_never_calls_python3_when_sourced() {
    let sb = Sandbox::new();
    let marker = sb.python3_marker_shim();
    let (work, _) = repo_with_origin(&sb);
    fs::write(work.join("a.txt"), "a\n").unwrap();
    let script = format!(
        ". '{}'; declare -F workflow_stage_strict >/dev/null",
        tools_dir().join("workflow_stage_changes.sh").display()
    );
    let out = sb
        .command("bash")
        .args(["-c", &script])
        .current_dir(&work)
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(!marker.exists(), "python3 was called");
}
