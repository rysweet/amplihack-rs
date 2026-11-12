//! Direct component contracts, separate from CLI launch-boundary tests.
use super::*;

#[allow(dead_code)]
#[path = "../src/commands/recipe/run/run_checkout.rs"]
mod implementation;

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
