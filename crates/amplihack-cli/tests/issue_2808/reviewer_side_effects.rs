//! #2808: the reviewer now writes where it reads, so what it leaves behind must
//! be handled by one agentic step that reports and never ends the run, and the
//! evidence it is handled against must stay private to the operator.
//!
//! The assessor is a scripted agent here: these tests pin the contract around
//! it (the run continues, nothing is written outside the tree, nothing is
//! readable by others, review-owned keys cannot be injected), not model quality.
use super::fixture::{Fixture, files_under, private, succeeds, workspace};
use super::review_rounds::{Mutation, ROUNDS, run};
use crate::support::exec_stub::write_executable;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const OUTSIDE_BODY: &[u8] = b"outside the tree, never to be written 2808\n";

/// Acceptance criterion 3: a reviewer that edits a tracked file and creates an
/// untracked one is followed by the assessment, and the run continues.
#[test]
fn reviewer_edit_and_new_file_are_assessed_and_the_run_continues() {
    for spec in ROUNDS {
        let f = Fixture::new();
        let (result, calls) = run(&f, spec, Mutation::TrackedAndUntracked, None);
        assert!(
            result.success,
            "{}: a reviewer side effect must not end the run: {result:?}",
            spec.0
        );
        let calls = calls.lock().unwrap();
        assert!(calls.reviewed, "{}: reviewer did not run", spec.0);
        assert!(
            calls.assessed,
            "{}: no assessment followed the review",
            spec.0
        );
        // The scripted assessor reports and declines to act: nothing the
        // reviewer left is silently discarded.
        assert_eq!(
            fs::read(f.pr.join("pr-only")).unwrap(),
            b"reviewer tracked edit\n"
        );
        assert!(f.pr.join("reviewer-created").exists());
        // The drive clone is a different checkout and is never touched.
        assert!(!f.repo.join("reviewer-created").exists());
        f.assert_no_private_repositories();
    }
}

/// SR-8: a reviewer that swaps a tracked file for a symlink out of the tree
/// must not cause any write outside the tree, and must not leak the target's
/// contents into the pre-review evidence taken before it ran.
#[test]
fn symlink_replacement_never_writes_outside_the_tree_and_the_run_continues() {
    for spec in ROUNDS {
        let f = Fixture::new();
        let outside = f.temp.path().join("outside-target");
        fs::write(&outside, OUTSIDE_BODY).unwrap();
        let (result, calls) = run(&f, spec, Mutation::SymlinkOutside, None);
        assert!(result.success, "{}: {result:?}", spec.0);
        assert!(calls.lock().unwrap().assessed);
        assert_eq!(
            fs::read(&outside).unwrap(),
            OUTSIDE_BODY,
            "{}: a file outside the tree was modified",
            spec.0
        );
        assert!(
            fs::symlink_metadata(f.pr.join("pr-only"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{}: an uncertain change must be left in place",
            spec.0
        );
        for (path, bytes) in &calls.lock().unwrap().baseline {
            assert!(
                !bytes.windows(OUTSIDE_BODY.len()).any(|w| w == OUTSIDE_BODY),
                "{}: baseline {} captured content from outside the tree",
                spec.0,
                path.display()
            );
        }
    }
}

fn baseline(f: &Fixture) -> std::path::PathBuf {
    let tools = workspace().join("amplifier-bundle/tools");
    let out = f
        .command("bash")
        .current_dir(&f.pr)
        .env("REPO_PATH", &f.pr)
        .env("AUTODRIVE_STATE_DIR", &f.state)
        .arg(tools.join("autodrive_round_evidence.sh"))
        .arg("review-baseline")
        .output()
        .unwrap();
    succeeds(&out);
    Path::new(String::from_utf8(out.stdout).unwrap().trim()).to_path_buf()
}

/// SR-10: the baseline holds untracked file contents, so neither the
/// directory nor anything in it may be group- or world-accessible.
#[test]
fn baseline_directory_and_files_are_private() {
    let f = Fixture::new();
    fs::write(f.pr.join("untracked-secret"), b"token=abc\n").unwrap();
    let dir = baseline(&f);
    assert!(dir.starts_with(&f.state), "{}", dir.display());
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o077,
        0,
        "baseline directory is accessible to others"
    );
    let files = files_under(&dir);
    assert!(!files.is_empty());
    for file in files {
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o077,
            0,
            "{} is accessible to others",
            file.display()
        );
    }
    assert_eq!(
        fs::read_to_string(dir.join("collection-status"))
            .unwrap()
            .trim(),
        "complete"
    );
}

/// SR-8: a path with a symlinked parent marks the evidence incomplete and its
/// contents are not collected; collection reports the fact rather than dying.
#[test]
fn symlinked_parent_makes_the_baseline_incomplete_without_following_it() {
    let f = Fixture::new();
    let outside = f.temp.path().join("outside-dir");
    private(&outside);
    fs::write(outside.join("leaf"), OUTSIDE_BODY).unwrap();
    // A tracked `sub/leaf` whose parent was then swapped for a symlink.
    fs::create_dir_all(f.pr.join("sub")).unwrap();
    fs::write(f.pr.join("sub/leaf"), b"tracked leaf\n").unwrap();
    f.git(&f.pr, &["add", "sub/leaf"]);
    fs::remove_dir_all(f.pr.join("sub")).unwrap();
    std::os::unix::fs::symlink(&outside, f.pr.join("sub")).unwrap();
    let dir = baseline(&f);
    let status = fs::read_to_string(dir.join("collection-status")).unwrap();
    assert!(status.starts_with("incomplete"), "{status}");
    for file in files_under(&dir) {
        let bytes = fs::read(&file).unwrap_or_default();
        assert!(
            !bytes.windows(OUTSIDE_BODY.len()).any(|w| w == OUTSIDE_BODY),
            "{} followed the symlink",
            file.display()
        );
    }
}

/// SR-5/SR-6/SR-7 and #2814: some context keys belong to the review and are
/// never taken from the caller. #3129 replaced the Crusty loop, whose guard
/// refused such a key, with the review action (`autodrive_pr_goal.sh review`,
/// reached through the `autodrive_loop.sh --loop-name crusty` alias, the one
/// path that still takes a caller's `--context`). The action builds the
/// round's context itself: the reviewed head is the one it measured and the
/// launch tools are the launch bundle's, whatever the caller supplied, and the
/// round's own keys are never passed in. Forwarding the caller's other keys
/// went with the loop.
#[test]
fn the_review_never_takes_the_context_keys_it_owns_from_the_caller() {
    let tools = workspace().join("amplifier-bundle/tools");
    for key in [
        "reviewed_head_sha",
        "autodrive_launch_tools",
        "autodrive_completion_state",
        "pre_review_evidence",
        "review_change_assessment",
    ] {
        let f = Fixture::new();
        let task = f.temp.path().join("task.txt");
        fs::write(&task, "address the review\n").unwrap();
        // The round's `amplihack recipe run` records its arguments and fails,
        // so no review runs.
        let stub = f.temp.path().join("amplihack-stub");
        let argv = f.temp.path().join("round-argv");
        write_executable(
            &stub,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_OUT\"\nexit 97\n",
            0o700,
        );
        let out = f
            .command("bash")
            .current_dir(&f.pr)
            .env("AMPLIHACK_BIN", &stub)
            .env("ARGV_OUT", &argv)
            .arg(tools.join("autodrive_loop.sh"))
            .args(["--loop-name", "crusty"])
            .args(["--round-recipe", "autodrive-crusty-round"])
            .args(["--clean-token", "CLEAN"])
            .args(["--verdict-field", "crusty_verdict"])
            .arg("--repo")
            .arg(&f.pr)
            .arg("--state-dir")
            .arg(&f.state)
            .arg("--task-file")
            .arg(&task)
            .args(["--context", "pr_number=2808"])
            .arg("--context")
            .arg(format!("{key}=supplied-by-the-operator"))
            .output()
            .unwrap();
        let args = fs::read_to_string(&argv)
            .unwrap_or_else(|e| panic!("{key}: the review started no round ({e}): {out:?}"));
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(
            args[..3],
            [
                "recipe",
                "run",
                tools
                    .parent()
                    .unwrap()
                    .join("recipes/autodrive-crusty-round.yaml")
                    .to_str()
                    .unwrap()
            ],
            "{key}: the round recipe comes from the launch bundle: {out:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("supplied-by-the-operator")),
            "{key}: a caller's value reached the round: {args:?}"
        );
        let expected: Vec<String> = match key {
            "reviewed_head_sha" => vec![format!("reviewed_head_sha={}", f.observed_head)],
            "autodrive_launch_tools" => {
                vec![format!("autodrive_launch_tools={}", tools.display())]
            }
            _ => Vec::new(),
        };
        let given: Vec<&str> = args
            .iter()
            .copied()
            .filter(|a| a.starts_with(&format!("{key}=")))
            .collect();
        assert_eq!(given, expected, "{key} routing: {out:?}");
    }
}

/// SR-1: a bundle that lives inside the reviewed tree cannot supply the reviewer.
#[test]
fn a_round_recipe_inside_the_reviewed_tree_is_refused() {
    let f = Fixture::new();
    let tools = f.pr.join("amplifier-bundle/tools");
    fs::create_dir_all(tools.parent().unwrap()).unwrap();
    succeeds(
        &f.command("cp")
            .arg("-r")
            .arg(workspace().join("amplifier-bundle/tools"))
            .arg(&tools)
            .output()
            .unwrap(),
    );
    fs::create_dir_all(f.pr.join("amplifier-bundle/recipes")).unwrap();
    fs::write(
        f.pr.join("amplifier-bundle/recipes/autodrive-crusty-round.yaml"),
        "name: x\nversion: 1.0.0\nsteps: []\n",
    )
    .unwrap();
    let out = f
        .command("bash")
        .current_dir(&f.pr)
        .arg(tools.join("autodrive_worktree_guard.sh"))
        .args(["loop-preflight", "crusty", f.pr.to_str().unwrap()])
        .arg("autodrive-crusty-round")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
}
