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
