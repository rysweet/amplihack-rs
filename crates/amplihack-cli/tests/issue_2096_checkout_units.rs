//! Direct component contracts, separate from CLI launch-boundary tests.
use super::*;

#[allow(dead_code)]
#[path = "../src/commands/recipe/run/run_checkout.rs"]
pub(super) mod implementation;

fn isolated_body(name: &str, body: impl FnOnce(Fixture)) {
    crate::support::quiet_process::in_quiet_process(module_path!(), name, || {
        use crate::support::quiet_process::{child_remove_var, child_set_var};
        let f = Fixture::new();
        for key in amplihack_git::REPOSITORY_ENV_VARS {
            child_remove_var(key);
        }
        for (key, _) in std::env::vars_os() {
            let text = key.to_string_lossy();
            if text.starts_with("GIT_CONFIG_") || text == "AMPLIHACK_RUN_CHECKOUT" {
                child_remove_var(key);
            }
        }
        for (key, value) in [
            ("HOME", f.home.clone()),
            ("XDG_CONFIG_HOME", f.home.join("config")),
            ("CLAUDE_CONFIG_DIR", f.home.join(".claude")),
            ("CODEX_HOME", f.home.join(".codex")),
            ("AMPLIHACK_HOME", f.home.join(".amplihack")),
            ("AMPLIHACK_CLAUDE_PROFILE_ROOT", f.home.join("profiles")),
        ] {
            child_set_var(key, value);
        }
        child_set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        child_set_var("GIT_CONFIG_NOSYSTEM", "1");
        body(f);
    });
}

#[test]
fn prepare_preserves_independent_index_and_working_layers() {
    isolated_body(
        "prepare_preserves_independent_index_and_working_layers",
        |f| {
            fs::write(f.repo.join("tracked"), b"index\n").unwrap();
            f.git(&["add", "tracked"]);
            fs::write(f.repo.join("tracked"), b"working\n").unwrap();
            let scratch = f.root.join("scratch");
            fs::create_dir(&scratch).unwrap();
            let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
            assert_eq!(
                f.git_in(checkout.exec_dir(), &["show", ":tracked"]),
                b"index\n"
            );
            assert_eq!(
                fs::read(checkout.exec_dir().join("tracked")).unwrap(),
                b"working\n"
            );
        },
    );
}

#[test]
fn prepare_preserves_large_indexes_and_unreachable_duplicate_binary_blobs() {
    isolated_body(
        "prepare_preserves_large_indexes_and_unreachable_duplicate_binary_blobs",
        |f| {
            // Query input and output both exceed a pipe buffer. Most objects
            // arrive through the clone; the binary additions are unreachable.
            for i in 0..2048 {
                fs::write(f.repo.join(format!("committed-{i}")), format!("{i}\n")).unwrap();
            }
            f.git(&["add", "."]);
            f.git(&["commit", "-qm", "large index fixture"]);
            let binary = [0, 255, 10, 13].repeat(32768);
            fs::write(f.repo.join("addition-a"), &binary).unwrap();
            fs::write(f.repo.join("addition-b"), &binary).unwrap();
            f.git(&["add", "addition-a", "addition-b"]);
            fs::write(f.repo.join("addition-a"), b"working layer\n").unwrap();
            let entries = f.git(&["ls-files", "--stage", "-z"]);
            let before = f.snapshot();
            let scratch = f.root.join("scratch");
            fs::create_dir(&scratch).unwrap();
            let mut checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
            assert_eq!(
                f.git_in(checkout.exec_dir(), &["ls-files", "--stage", "-z"]),
                entries
            );
            for name in [":addition-a", ":addition-b"] {
                assert_eq!(f.git_in(checkout.exec_dir(), &["show", name]), binary);
            }
            assert_eq!(
                fs::read(checkout.exec_dir().join("addition-a")).unwrap(),
                b"working layer\n"
            );
            assert_eq!(
                fs::read(checkout.exec_dir().join("addition-b")).unwrap(),
                binary
            );
            assert_eq!(f.snapshot(), before);
            checkout.remove().unwrap();
        },
    );
}

#[test]
fn prepare_rejects_missing_staged_objects() {
    isolated_body("prepare_rejects_missing_staged_objects", |f| {
        fs::write(f.repo.join("addition"), b"unreachable staged bytes\n").unwrap();
        f.git(&["add", "addition"]);
        let oid = String::from_utf8(f.git(&["rev-parse", ":addition"]))
            .unwrap()
            .trim()
            .to_string();
        fs::remove_file(f.repo.join(".git/objects").join(&oid[..2]).join(&oid[2..])).unwrap();
        let scratch = f.root.join("scratch");
        fs::create_dir(&scratch).unwrap();
        let error = implementation::RunCheckout::prepare(&f.repo, &scratch, false)
            .expect_err("an unavailable staged object must block execution");
        assert!(format!("{error:#}").contains("cat-file"), "{error:#}");
    });
}

#[test]
fn prepare_rejects_staged_entries_that_name_tree_objects() {
    isolated_body(
        "prepare_rejects_staged_entries_that_name_tree_objects",
        |f| {
            let tree = String::from_utf8(f.git(&["rev-parse", "HEAD^{tree}"]))
                .unwrap()
                .trim()
                .to_string();
            f.git(&["update-index", "--cacheinfo", "100644", &tree, "tracked"]);
            let scratch = f.root.join("scratch");
            fs::create_dir(&scratch).unwrap();
            let error = implementation::RunCheckout::prepare(&f.repo, &scratch, false)
                .expect_err("a staged tree must not be accepted as an existing blob");
            assert!(
                format!("{error:#}").contains("unexpected isolated object query response"),
                "{error:#}"
            );
        },
    );
}

#[test]
fn prepare_preserves_an_empty_index_and_its_working_files() {
    isolated_body(
        "prepare_preserves_an_empty_index_and_its_working_files",
        |f| {
            f.git(&["rm", "--cached", "-r", "."]);
            let before = f.snapshot();
            let scratch = f.root.join("scratch");
            fs::create_dir(&scratch).unwrap();
            let mut checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
            assert!(
                f.git_in(checkout.exec_dir(), &["ls-files", "--stage"])
                    .is_empty()
            );
            assert_eq!(
                fs::read(checkout.exec_dir().join("tracked")).unwrap(),
                b"base\n"
            );
            assert_eq!(
                fs::read(checkout.exec_dir().join("sub/file")).unwrap(),
                b"sub\n"
            );
            assert_eq!(f.snapshot(), before);
            checkout.remove().unwrap();
        },
    );
}

#[test]
fn prepare_distinguishes_corrupt_metadata_from_absent_repository() {
    isolated_body(
        "prepare_distinguishes_corrupt_metadata_from_absent_repository",
        |f| {
            let plain = f.root.join("plain");
            let scratch = f.root.join("scratch");
            fs::create_dir(&plain).unwrap();
            fs::create_dir(&scratch).unwrap();
            let outside = implementation::RunCheckout::prepare(&plain, &scratch, false).unwrap();
            assert_eq!(outside.exec_dir(), plain);
            fs::write(plain.join(".git"), b"gitdir: /missing/issue-2096-unit\n").unwrap();
            assert!(
                implementation::RunCheckout::prepare(&plain, &scratch, false).is_err(),
                "a Git inspection error must never authorize caller execution"
            );
        },
    );
}

/// Issue #2346: a checkout's fresh execution storage is private storage
/// under the work root, so it is created 0700 whatever the umask. A
/// `tempfile` folder with no explicit mode is 0777 less the umask.
#[test]
fn fresh_execution_storage_is_0700() {
    isolated_body("fresh_execution_storage_is_0700", |f| {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: isolated_body runs this as the only test in its child process.
        unsafe {
            libc::umask(0o002);
        }
        let scratch = f.root.join("scratch");
        fs::create_dir(&scratch).unwrap();
        let mut checkout = implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
        let implementation::RunCheckout::Private { storage, root, .. } = &checkout else {
            panic!("a repository gets a private checkout");
        };
        let mode = fs::metadata(storage).unwrap().permissions().mode() & 0o7777;
        assert_eq!(format!("{mode:04o}"), "0700", "{}", storage.display());
        assert_eq!(
            fs::metadata(root).unwrap().permissions().mode() & 0o777,
            0o700,
            "the allocated checkout directory is private; tracked Git contents keep normal modes"
        );
        assert_eq!(
            fs::metadata(storage.join("owner.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        checkout.remove().unwrap();
    });
}

/// Issue #2346: the record a release leaves for a durable worktree is a
/// private file, 0600 whatever the umask, like every other file amplihack
/// writes for its own state.
#[test]
fn a_durable_dependency_record_is_0600() {
    isolated_body("a_durable_dependency_record_is_0600", |f| {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: isolated_body runs this as the only test in its child process.
        unsafe {
            libc::umask(0o002);
        }
        let scratch = f.root.join("scratch");
        fs::create_dir(&scratch).unwrap();
        let mut checkout = implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
        let (storage, repository) = match &checkout {
            implementation::RunCheckout::Private {
                storage,
                repository,
                ..
            } => (storage.clone(), repository.clone()),
            implementation::RunCheckout::Caller(_) => {
                panic!("a repository gets a private checkout")
            }
        };
        // A worktree a workflow made in the run's private repository, which
        // must outlive the run's own checkout.
        let durable = f.root.join("durable");
        f.git_in(
            &repository,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                durable.to_str().unwrap(),
            ],
        );
        checkout.remove().unwrap();
        let record = storage.join("durable-dependencies");
        let mode = fs::symlink_metadata(&record)
            .unwrap_or_else(|e| panic!("{}: {e}", record.display()))
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(format!("{mode:04o}"), "0600", "{}", record.display());
    });
}

#[test]
fn release_revalidates_directory_identity_before_removing_contents() {
    isolated_body(
        "release_revalidates_directory_identity_before_removing_contents",
        |f| {
            let scratch = f.root.join("scratch");
            fs::create_dir(&scratch).unwrap();
            let mut checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
            let destination = checkout.exec_dir().to_path_buf();
            fs::rename(&destination, f.root.join("parked")).unwrap();
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("sentinel"), b"not owned\n").unwrap();
            checkout
                .remove()
                .expect_err("substituted directory must refuse release");
            assert_eq!(
                fs::read(destination.join("sentinel")).unwrap(),
                b"not owned\n"
            );
        },
    );
}

/// S-IV4: `map_path` refuses `..` inside the caller's checkout as
/// `map_context` does. Joined onto the private root, `<caller>/x/../../y`
/// would resolve outside the checkout, beside `repository.git`.
#[test]
fn map_path_refuses_a_caller_path_that_climbs_out_with_parent_dirs() {
    isolated_body(
        "map_path_refuses_a_caller_path_that_climbs_out_with_parent_dirs",
        |f| {
            let scratch = f.root.join("scratch");
            fs::create_dir(&scratch).unwrap();
            let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch, false).unwrap();
            let error = checkout
                .map_path(&f.repo.join("x/../../y"))
                .expect_err("a path that climbs out of the checkout must be refused");
            assert!(format!("{error:#}").contains("contains '..'"), "{error:#}");
            assert_eq!(
                checkout.map_path(&f.repo.join("tracked")).unwrap(),
                checkout.exec_dir().join("tracked"),
                "a canonical path inside the checkout still maps"
            );
        },
    );
}

/// Index isolation between a linked worktree and the main checkout, kept from
/// the recording tests removed in #2738. The recipe runner is not involved:
/// this pins an observation about Git itself that candidate 1 of the #2096
/// investigation rested on, and `GIT_TRACE2_EVENT` is used here only as the
/// test's own instrument for reading which index each command ran against.
///
/// Observed with git 2.56 on 10 Nov 2026, a pre-commit hook's `git diff`,
/// `git checkout -- .` and `git apply` all ran with `GIT_INDEX_FILE` set by
/// git to the linked worktree's own index (`.git/worktrees/<name>/index`),
/// and the main checkout's index was byte-identical before and after. This
/// pins that synthetic observation, without ruling out the real hook chain
/// or identifying the historical writer. A regression names the commands
/// that ran with which index.
#[test]
fn a_precommit_stash_and_restore_in_a_worktree_leaves_the_main_index_alone() {
    use serde_json::Value;

    let f = Fixture::new();
    fs::write(f.repo.join("untracked-in-main"), "u").unwrap();
    let wt = f.root.join("wt");
    f.git(&["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
    let wt = fs::canonicalize(wt).unwrap();
    let wt_index = PathBuf::from(
        String::from_utf8(f.git_in(
            &wt,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        ))
        .unwrap()
        .trim(),
    );

    // pre-commit's staged_files_only: save unstaged changes as a patch,
    // check out the index, run hooks, apply the patch back.
    let hook = f.repo.join(".git/hooks/pre-commit");
    crate::support::exec_stub::write_executable(
        &hook,
        r#"#!/bin/sh
set -e
patch="$(git rev-parse --git-dir)/stash.patch"
if ! git diff --ignore-submodules --binary --exit-code --no-color --no-ext-diff > "$patch"; then
  git checkout -- .
  : > "$(git rev-parse --git-dir)/hook-stashed"
  git apply "$patch"
fi
rm -f "$patch"
"#,
        0o755,
    );

    fs::write(wt.join("staged"), "s").unwrap();
    f.git_in(&wt, &["add", "staged"]);
    fs::write(wt.join("tracked"), "base\nunstaged\n").unwrap();

    let main_index = f.repo.join(".git/index");
    let before = fs::read(&main_index).unwrap();
    let trace = f.root.join("replay-trace");
    fs::create_dir(&trace).unwrap();
    let mut c = amplihack_git::command();
    f.environment(&mut c);
    let mut target = trace.clone().into_os_string();
    target.push("/");
    let out = c
        .current_dir(&wt)
        .env("GIT_TRACE2_EVENT", target)
        .env(
            "GIT_TRACE2_ENV_VARS",
            "GIT_DIR,GIT_INDEX_FILE,GIT_WORK_TREE,PWD",
        )
        .args(["commit", "-qm", "worktree commit"])
        .output()
        .unwrap();
    succeeds(&out);
    let after = fs::read(&main_index).unwrap();

    // The hook did take the stash path.
    let wt_git_dir = wt_index.parent().unwrap();
    assert!(
        wt_git_dir.join("hook-stashed").exists(),
        "the hook never stashed"
    );
    assert_eq!(
        fs::read_to_string(wt.join("tracked")).unwrap(),
        "base\nunstaged\n",
        "the hook restored the unstaged change"
    );

    // Which index each of the hook's git commands ran against.
    let mut indexes: std::collections::BTreeMap<String, (Vec<String>, Option<String>)> =
        Default::default();
    for file in fs::read_dir(&trace).unwrap() {
        let text = fs::read_to_string(file.unwrap().path()).unwrap();
        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let sid = event["sid"].as_str().unwrap_or_default().to_string();
            match event["event"].as_str() {
                Some("start") => {
                    indexes.entry(sid).or_default().0 = event["argv"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                }
                Some("def_param") if event["param"] == "GIT_INDEX_FILE" => {
                    indexes.entry(sid).or_default().1 = event["value"].as_str().map(String::from);
                }
                _ => {}
            }
        }
    }
    let hook_commands: Vec<_> = indexes
        .values()
        .filter(|(argv, _)| argv.get(1).is_some_and(|s| s == "checkout" || s == "apply"))
        .collect();
    assert_eq!(hook_commands.len(), 2, "{indexes:#?}");
    for (argv, index) in &hook_commands {
        assert_eq!(
            index.as_deref(),
            wt_index.to_str(),
            "{argv:?} ran against an index other than the worktree's"
        );
    }

    assert!(
        before == after,
        "the main index changed; commands and their indexes: {indexes:#?}"
    );
}
