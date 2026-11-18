//! #2808 replaces #2096's private execution repositories with named checkouts.
//! Behavioral contracts only. Existing #2096 mechanism tests are removed with
//! their implementation in the next checkpoint, not in this tests-first step.
#![cfg(unix)]

#[path = "issue_2808/codex_review.rs"]
mod codex_review;
#[path = "issue_2808/support.rs"]
mod fixture;
#[path = "issue_2808/real_runner.rs"]
mod real_runner;
#[path = "issue_2808/review_rounds.rs"]
mod review_rounds;
#[path = "issue_2808/reviewer_side_effects.rs"]
mod reviewer_side_effects;
#[path = "issue_2808/round_records.rs"]
mod round_records;

use fixture::{Fixture, succeeds};
use std::fs;
use std::path::Path;

fn observed_path(f: &Fixture, name: &str) -> std::path::PathBuf {
    let bytes = f.observation(name);
    Path::new(String::from_utf8(bytes).unwrap().trim()).to_path_buf()
}

#[test]
fn explicit_named_checkout_is_runner_cwd_and_build_write_destination() {
    let f = Fixture::new();
    let out = f
        .recipe_command(&f.repo)
        .current_dir(f.temp.path())
        .env("WRITE_RESULT", "1")
        .output()
        .unwrap();
    succeeds(&out);
    assert_eq!(observed_path(&f, "startup"), f.repo);
    assert_eq!(observed_path(&f, "cwd"), f.repo);
    assert_eq!(
        fs::read(f.repo.join("build-output")).unwrap(),
        b"build output\n"
    );
    assert!(!f.pr.join("build-output").exists());
    f.assert_no_private_repositories();
}

#[test]
fn explicitly_selected_linked_worktree_keeps_its_repository_head_and_context_paths() {
    let f = Fixture::new();
    let before = f.git(&f.repo, &["worktree", "list", "--porcelain"]);
    succeeds(
        &f.recipe_command(&f.pr)
            .arg("-c")
            .arg(format!("repo_path={}", f.pr.display()))
            .output()
            .unwrap(),
    );
    assert_eq!(observed_path(&f, "cwd"), f.pr);
    assert_eq!(observed_path(&f, "common"), f.repo.join(".git"));
    assert_eq!(
        String::from_utf8(f.observation("head")).unwrap().trim(),
        f.observed_head
    );
    let context: serde_json::Value =
        serde_json::from_slice(&f.observation("context.json")).unwrap();
    assert_eq!(context["repo_path"], f.pr.to_str().unwrap());
    assert_eq!(f.git(&f.repo, &["worktree", "list", "--porcelain"]), before);
    f.assert_no_private_repositories();
}

#[test]
fn checkout_paths_in_task_data_are_delivered_without_rejection_or_rewriting() {
    let f = Fixture::new();
    let task = format!("Edit {} and inspect {}", f.pr.display(), f.repo.display());
    succeeds(
        &f.recipe_command(&f.pr)
            .arg("-c")
            .arg(format!("task_description={task}"))
            .output()
            .unwrap(),
    );
    let context: serde_json::Value =
        serde_json::from_slice(&f.observation("context.json")).unwrap();
    assert_eq!(context["task_description"], task);
    assert_eq!(observed_path(&f, "cwd"), f.pr);
}

#[test]
fn ignored_local_recipe_and_selected_subdirectory_are_used_in_place() {
    let f = Fixture::new();
    let subdir = f.pr.join("subdir");
    fs::create_dir(&subdir).unwrap();
    let recipe = f.pr.join("local-recipe.yaml");
    let content =
        b"name: local-build\nsteps:\n  - id: build\n    type: bash\n    command: 'true'\n";
    fs::write(f.repo.join(".git/info/exclude"), b"local-recipe.yaml\n").unwrap();
    fs::write(&recipe, content).unwrap();
    succeeds(&f.recipe_command_for(&subdir, &recipe).output().unwrap());
    assert_eq!(observed_path(&f, "recipe"), recipe);
    assert_eq!(observed_path(&f, "startup"), subdir);
    assert_eq!(observed_path(&f, "cwd"), subdir);
    assert_eq!(observed_path(&f, "toplevel"), f.pr);
    assert_eq!(fs::read(&recipe).unwrap(), content);
    f.assert_no_private_repositories();
}

#[test]
fn named_checkout_receives_config_credentials_identity_and_scrubbed_git_routing() {
    let f = Fixture::new();
    let config = f.home.join("gitconfig");
    fs::write(&config, "[fixture]\nkept = caller-configuration\n").unwrap();
    let out = f
        .recipe_command(&f.pr)
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GH_TOKEN", "fixture-credential-never-use-on-network")
        .env("GIT_DIR", f.repo.join(".git"))
        .env("GIT_WORK_TREE", &f.repo)
        .env("GIT_INDEX_FILE", f.repo.join(".git/index"))
        .env("GIT_COMMON_DIR", f.repo.join(".git"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.bare")
        .env("GIT_CONFIG_VALUE_0", "true")
        .env("GIT_CONFIG_PARAMETERS", "'core.bare=true'")
        .env("GIT_TRACE2_EVENT", f.temp.path().join("must-not-trace"))
        .env("GIT_TRACE2_REDACT", "0")
        .output()
        .unwrap();
    succeeds(&out);
    assert_eq!(observed_path(&f, "toplevel"), f.pr);
    assert!(
        !f.temp.path().join("observations/git-routing").exists(),
        "inherited Git routing or Trace2 reached the runner"
    );
    assert_eq!(f.observation("config"), b"caller-configuration\n");
    assert_eq!(
        f.observation("credential"),
        b"fixture-credential-never-use-on-network"
    );
    assert_eq!(f.observation("author"), b"Fixture Author");
    assert_eq!(
        fs::read(config).unwrap(),
        b"[fixture]\nkept = caller-configuration\n"
    );
}

#[test]
fn interrupted_run_keeps_uncommitted_unpushed_work_and_worktree_registration() {
    let f = Fixture::new();
    fs::write(f.pr.join("tracked"), b"prior staged work\n").unwrap();
    f.git(&f.pr, &["add", "tracked"]);
    fs::write(f.pr.join("tracked"), b"prior unstaged work\n").unwrap();
    fs::write(f.pr.join("prior-untracked"), b"prior untracked work\n").unwrap();
    // The runner commits only its own file and leaves the prior index layer.
    let out = f
        .recipe_command(&f.pr)
        .env("INTERRUPT_BUILD", "1")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "interruption must remain a failure: {out:?}"
    );
    assert_eq!(
        fs::read(f.pr.join("unfinished")).unwrap(),
        b"unfinished run work\n"
    );
    assert_eq!(
        fs::read(f.pr.join("tracked")).unwrap(),
        b"prior unstaged work\n"
    );
    assert_eq!(
        fs::read(f.pr.join("prior-untracked")).unwrap(),
        b"prior untracked work\n"
    );
    assert_eq!(f.git(&f.pr, &["show", ":tracked"]), b"prior staged work\n");
    assert_eq!(
        f.git(&f.pr, &["show", "HEAD:tracked"]),
        b"committed baseline\n"
    );
    assert_eq!(
        f.git(&f.pr, &["show", "HEAD:unpushed"]),
        b"unpushed run work\n"
    );
    assert_eq!(
        f.git_text(&f.pr, &["rev-list", "--count", "origin/feature/2808..HEAD"]),
        "1"
    );
    // Registration remains on the same branch; only its recorded HEAD moves.
    let after = f.git_text(&f.repo, &["worktree", "list", "--porcelain"]);
    assert!(after.contains(f.pr.to_str().unwrap()));
    assert!(after.contains("branch refs/heads/feature/2808"));
}

#[test]
fn retained_worktree_helper_accepts_registered_pr_and_rejects_wrong_identity() {
    let f = Fixture::new();
    let before = f.git(&f.repo, &["worktree", "list", "--porcelain"]);
    let out = f.helper(&f.pr, &f.observed_head, "feature/2808");
    succeeds(&out);
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["verified"], true);
    assert_eq!(result["worktree"], f.pr.to_str().unwrap());
    let main_head = f.git_text(&f.repo, &["rev-parse", "HEAD"]);
    for (path, head, branch) in [
        (f.pr.as_path(), main_head.as_str(), "feature/2808"),
        (f.pr.as_path(), f.observed_head.as_str(), "main"),
        (f.repo.as_path(), main_head.as_str(), "main"),
        (f.temp.path(), f.observed_head.as_str(), "feature/2808"),
    ] {
        let out = f.helper(path, head, branch);
        assert!(
            !out.status.success(),
            "accepted invalid worktree identity: {path:?} {head} {branch}: {out:?}"
        );
    }
    assert_eq!(f.git(&f.repo, &["worktree", "list", "--porcelain"]), before);
}

#[test]
fn ordinary_run_leaves_legacy_execution_recovery_data_untouched() {
    let f = Fixture::new();
    let legacy = f.work.join("exec/legacy-recovery");
    fixture::private(&legacy);
    let bare = legacy.join("repository.git");
    let checkout = legacy.join("checkout");
    f.git(
        f.temp.path(),
        &[
            "clone",
            "--bare",
            "-q",
            f.repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    f.git(
        &bare,
        &[
            "worktree",
            "add",
            "--detach",
            "-q",
            checkout.to_str().unwrap(),
            &f.observed_head,
        ],
    );
    fs::write(checkout.join("recoverable"), b"old unpushed work\n").unwrap();
    f.git(&checkout, &["add", "recoverable"]);
    f.git(&checkout, &["commit", "-qm", "old unpublished work"]);
    fs::write(checkout.join("recoverable"), b"old uncommitted work\n").unwrap();
    let head = f.git(&checkout, &["rev-parse", "HEAD"]);
    let status = f.git(&checkout, &["status", "--porcelain", "-z"]);
    let registration = f.git(&bare, &["worktree", "list", "--porcelain"]);
    succeeds(&f.recipe_command(&f.repo).output().unwrap());
    assert_eq!(f.git(&checkout, &["rev-parse", "HEAD"]), head);
    assert_eq!(f.git(&checkout, &["status", "--porcelain", "-z"]), status);
    assert_eq!(
        f.git(&bare, &["worktree", "list", "--porcelain"]),
        registration
    );
    assert_eq!(
        f.git(&checkout, &["show", "HEAD:recoverable"]),
        b"old unpushed work\n"
    );
    assert_eq!(
        fs::read(checkout.join("recoverable")).unwrap(),
        b"old uncommitted work\n"
    );
}

#[test]
fn repo_path_alone_selects_the_checkout_from_an_unrelated_launch_directory() {
    for relative in [false, true] {
        let f = Fixture::new();
        let setup = f.recipe_command(&f.pr);
        let mut args: Vec<_> = setup
            .get_args()
            .map(std::ffi::OsStr::to_os_string)
            .collect();
        args.truncate(args.len() - 2); // This case selects repo_path, without -w.
        let mut command = f.command(crate::support::amplihack_bin::amplihack_bin());
        for (key, value) in setup.get_envs() {
            if let Some(value) = value {
                command.env(key, value);
            } else {
                command.env_remove(key);
            }
        }
        let selected = if relative {
            "drive-clone/worktrees/pr-2808".into()
        } else {
            f.pr.to_string_lossy().into_owned()
        };
        succeeds(
            &command
                .current_dir(f.temp.path())
                .args(args)
                .args(["-c", &format!("repo_path={selected}")])
                .output()
                .unwrap(),
        );
        assert_eq!(observed_path(&f, "startup"), f.pr);
        assert_eq!(observed_path(&f, "cwd"), f.pr);
        let context: serde_json::Value =
            serde_json::from_slice(&f.observation("context.json")).unwrap();
        assert_eq!(context["repo_path"], f.pr.to_str().unwrap());
        f.assert_no_private_repositories();
    }
}
