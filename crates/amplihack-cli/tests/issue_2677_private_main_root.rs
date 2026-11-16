//! Issue #2677: on 1.2.163 a smart-orchestrator run launched from a main
//! checkout with an issue number and a new branch stopped at
//! `step-04-setup-worktree` with "'<work root>/exec/<hash>' is not inside a
//! git work tree".
//!
//! The private checkout existed. Recipes are found first in the caller's own
//! `amplifier-bundle/`, so a caller older than #2290 supplied the old step-04,
//! which asks `workflow_worktree_root.sh base` about `root`'s answer. For a
//! private checkout that answer is `exec/<hash>`, the folder beside the bare
//! repository, and `base` stopped there. This launch shape (issue number,
//! explicit work root, run from the main checkout) must give step-04 a work
//! tree with the caller's `origin/*` refs, and one worktree base whichever way
//! it is asked.

use std::{fs, path::Path};

use super::{Fixture, succeeds};

#[test]
fn a_new_branch_run_with_an_issue_number_from_the_main_checkout_finds_its_worktree_base() {
    let f = Fixture::new();
    let upstream = f.root.join("upstream.git");
    f.git_in(
        &f.root,
        &["init", "-q", "--bare", upstream.to_str().unwrap()],
    );
    f.git(&["remote", "add", "origin", upstream.to_str().unwrap()]);
    f.git(&["push", "-q", "origin", "HEAD:refs/heads/main"]);
    f.git(&["fetch", "-q", "origin"]);

    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/workflow_worktree_root.sh")
        .canonicalize()
        .unwrap();
    let work_root = f.root.join("work");
    let out = f
        .command("issue-2677")
        .args(["-c", "repo_path=."])
        .args(["-c", "issue_number=2677"])
        .args(["-c", "force_single_workstream=true"])
        .env("ISSUE_2677_ROOT_HELPER", &helper)
        .env("ISSUE_2677_WORK_ROOT", &work_root)
        .output()
        .unwrap();
    succeeds(&out);

    let observed = |name: &str| String::from_utf8(f.observed("issue-2677", name)).unwrap();
    let cwd = observed("cwd");
    assert!(
        Path::new(&cwd).starts_with(work_root.join("exec")),
        "the run did not use its stable private checkout: {cwd}"
    );
    assert_eq!(
        observed("origin_refs").trim(),
        "refs/remotes/origin/main",
        "the private checkout lacks the caller's origin refs"
    );
    let main_root = observed("main_root");
    assert_ne!(main_root, cwd, "root of a private checkout is beside it");

    let from_checkout = observed("base_from_checkout");
    assert!(
        from_checkout.starts_with(work_root.join("worktrees").to_str().unwrap()),
        "worktree base is not under the work root: {from_checkout}"
    );
    assert_eq!(
        observed("base_from_main"),
        from_checkout,
        "`base` of root's answer failed or disagreed: {}",
        fs::read_to_string(f.root.join("issue-2677/base_from_main.err")).unwrap_or_default()
    );
}

fn helper() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/workflow_worktree_root.sh")
        .canonicalize()
        .unwrap()
}

/// Requirement 3 of #2677: when the private checkout cannot get the caller's
/// `origin/*` refs, the run stops with that plain reason before any step.
///
/// The refs are copied from the caller's own repository, not fetched from
/// the origin URL, so an unreachable URL alone does not fail (the recipe's
/// own `git fetch origin` fills the refs later). What can fail is the copy:
/// here `origin/main` names a commit whose blob is missing from the caller's
/// object store. `clone --bare` takes only branches and tags, so the clone
/// itself succeeds and the failure is the seeding step's.
#[test]
fn origin_refs_that_cannot_be_copied_fail_before_any_step() {
    let f = Fixture::new();
    let missing = f.root.join("no-such-upstream.git");
    f.git(&["remote", "add", "origin", missing.to_str().unwrap()]);

    // A blob only `origin/main` reaches, so deleting it leaves HEAD whole.
    let lost = f.root.join("lost-content");
    fs::write(&lost, b"issue 2677: only origin/main reaches this\n").unwrap();
    let blob = String::from_utf8(f.git(&["hash-object", "-w", lost.to_str().unwrap()]))
        .unwrap()
        .trim()
        .to_string();
    let tree_spec = format!("100644 blob {blob}\tlost\n");
    let mut mktree = amplihack_git::command();
    f.environment(&mut mktree);
    let mut child = mktree
        .current_dir(&f.repo)
        .arg("mktree")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(tree_spec.as_bytes())
        .unwrap();
    let tree = child.wait_with_output().unwrap();
    assert!(tree.status.success());
    let tree = String::from_utf8(tree.stdout).unwrap().trim().to_string();
    let commit = String::from_utf8(f.git(&["commit-tree", &tree, "-m", "origin only"]))
        .unwrap()
        .trim()
        .to_string();
    f.git(&["update-ref", "refs/remotes/origin/main", &commit]);
    let object = f
        .repo
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(&object).expect("the blob was written loose");

    let out = f
        .command("issue-2677-seed")
        .args(["-c", "repo_path=."])
        .args(["-c", "issue_number=2677"])
        .args(["-c", "force_single_workstream=true"])
        .env("ISSUE_2677_ROOT_HELPER", helper())
        .env("ISSUE_2677_WORK_ROOT", f.root.join("work"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the run must not start: {stderr}");
    assert!(
        stderr.contains("cannot seed origin refs"),
        "the plain reason is missing: {stderr}"
    );
    assert!(
        !f.root.join("issue-2677-seed").exists(),
        "a step ran before the checkout held the caller's origin refs"
    );
}

/// A bare repository and one linked checkout laid out like a run's private
/// checkout (`<storage>/repository.git`, `<storage>/checkout`).
fn private_layout(f: &Fixture) -> (std::path::PathBuf, std::path::PathBuf) {
    let storage = f.root.join("work/exec/0123abcd");
    fs::create_dir_all(&storage).unwrap();
    let repository = storage.join("repository.git");
    f.git_in(
        &f.root,
        &[
            "clone",
            "-q",
            "--bare",
            "--no-local",
            f.repo.to_str().unwrap(),
            repository.to_str().unwrap(),
        ],
    );
    let checkout = storage.join("checkout");
    f.git_in(
        &repository,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            checkout.to_str().unwrap(),
            "HEAD",
        ],
    );
    (storage, checkout)
}

fn ask(
    f: &Fixture,
    dir: &Path,
    run_checkout: Option<&Path>,
    args: &[&str],
) -> std::process::Output {
    let mut c = std::process::Command::new("bash");
    f.environment(&mut c);
    if let Some(checkout) = run_checkout {
        c.env("AMPLIHACK_RUN_CHECKOUT", checkout);
    }
    c.env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        .env("AMPLIHACK_WORK_ROOT", f.root.join("work"))
        .current_dir(dir)
        .arg(helper())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn root_answer_of_the_named_private_checkout_is_its_own_root() {
    let f = Fixture::new();
    let (storage, checkout) = private_layout(&f);
    let main = ask(&f, &checkout, None, &["root", "."]);
    assert!(main.status.success());
    assert_eq!(
        String::from_utf8(main.stdout).unwrap().trim(),
        storage.to_str().unwrap(),
        "root of a private checkout is the folder beside its bare repository"
    );

    for args in [["root", "."], ["base", "."]] {
        let from_main = ask(&f, &storage, Some(&checkout), &args);
        assert!(
            from_main.status.success(),
            "{args:?} of root's answer: {}",
            String::from_utf8_lossy(&from_main.stderr)
        );
        let from_checkout = ask(&f, &checkout, Some(&checkout), &args);
        assert_eq!(
            from_main.stdout, from_checkout.stdout,
            "{args:?} disagrees between the checkout and root's answer"
        );
    }
}

#[test]
fn unrelated_non_worktree_dir_still_fails() {
    let f = Fixture::new();
    let (storage, checkout) = private_layout(&f);
    let unrelated = f.root.join("work/exec/ffffffff");
    fs::create_dir_all(&unrelated).unwrap();

    for (dir, named) in [(&unrelated, Some(checkout.as_path())), (&storage, None)] {
        for verb in ["root", "base"] {
            let out = ask(&f, dir, named, &[verb, "."]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                !out.status.success(),
                "{verb} answered for {} with AMPLIHACK_RUN_CHECKOUT={named:?}: {}",
                dir.display(),
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(
                stderr.contains("is not inside a git work tree"),
                "{verb} in {}: {stderr}",
                dir.display()
            );
        }
    }
}
