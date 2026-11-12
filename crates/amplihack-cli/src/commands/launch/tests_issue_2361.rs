//! Issue #2361: the environment a recipe step gets from
//! `recipe run -w <checkout>` keeps the checkout as the bundle root, but no
//! runtime state or contract file is written into it.
//!
//! This exercises the Rust components a recipe step uses, without starting
//! an agent: `EnvBuilder::with_amplihack_home_from(<-w checkout>)` builds the
//! step environment; `amplihack_state_root()` / `session_state_dir()` choose
//! where hooks write; `contract_home()` and `ensure_contract_file()` choose
//! and write `private/system-prompt-append.md`.
//!
//! The test lives under `commands::launch` rather than `env_builder`
//! because `contract_home` is private to the launch module.
//!
//! The whole body holds `home_env_lock()` (the same mutex as
//! `cwd_env_lock()`), and a drop guard restores `HOME`, `AMPLIHACK_HOME` and
//! the other changed variables even when an assertion panics, so the real
//! `~/.amplihack` is never read or written.

use super::system_prompt_append::{CONTRACT_RELATIVE_PATH, contract_home, ensure_contract_file};
use crate::env_builder::EnvBuilder;
use crate::test_support::home_env_lock;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// Sets or removes variables for one test and restores every previous value
/// on drop, including on panic.
struct EnvRestore {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvRestore {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    fn remember(&mut self, key: &'static str) {
        if !self.saved.iter().any(|(k, _)| *k == key) {
            self.saved.push((key, std::env::var_os(key)));
        }
    }

    fn set(&mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        self.remember(key);
        // SAFETY: edition 2024 requires unsafe; the caller holds env_lock().
        unsafe { std::env::set_var(key, value) };
    }

    fn remove(&mut self, key: &'static str) {
        self.remember(key);
        // SAFETY: as above.
        unsafe { std::env::remove_var(key) };
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            // SAFETY: as above; the lock guard is declared before this guard
            // and so is dropped after it.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=issue-2361",
            "-c",
            "user.email=issue-2361@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A temporary `-w` checkout holding `amplifier-bundle/`, and a linked
/// worktree of it.
fn checkout_with_worktree(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("checkout");
    let wt = root.join("step-worktree");
    fs::create_dir_all(main.join("amplifier-bundle")).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    fs::write(main.join("README.md"), "base\n").unwrap();
    fs::write(main.join("amplifier-bundle").join("BUNDLE.md"), "bundle\n").unwrap();
    git(&main, &["add", "README.md", "amplifier-bundle/BUNDLE.md"]);
    git(&main, &["commit", "-q", "-m", "base"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "step-branch",
            wt.to_str().unwrap(),
        ],
    );
    (main, wt)
}

fn snapshot(checkout: &Path) -> (String, String) {
    (
        git(checkout, &["ls-files", "--stage"]),
        git(
            checkout,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=all",
            ],
        ),
    )
}

fn assert_untouched(checkout: &Path, before: &(String, String)) {
    assert_eq!(
        &snapshot(checkout),
        before,
        "the recipe-step writers changed the checkout {}",
        checkout.display()
    );
    for dir in ["runtime", "private"] {
        assert!(
            !checkout.join(dir).exists(),
            "{dir}/ was created in the checkout {}",
            checkout.display()
        );
    }
}

#[test]
fn recipe_step_environment_keeps_state_and_contract_out_of_the_checkout() {
    let _lock = home_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut env = EnvRestore::new();

    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (checkout, worktree) = checkout_with_worktree(tmp.path());
    let checkout = checkout.canonicalize().unwrap();
    let before = snapshot(&checkout);

    env.set("HOME", &home);
    env.remove("AMPLIHACK_HOME");
    env.remove("REPO_PATH");

    // `recipe run -w <checkout>`: the step environment names the checkout as
    // AMPLIHACK_HOME, because it holds amplifier-bundle/. Asset lookup is
    // unchanged by the fix.
    let step_env = EnvBuilder::new()
        .with_amplihack_home_from(&checkout)
        .build();
    let amplihack_home = step_env
        .get("AMPLIHACK_HOME")
        .expect("the step environment sets AMPLIHACK_HOME");
    assert_eq!(
        Path::new(amplihack_home),
        checkout,
        "AMPLIHACK_HOME stays the -w checkout for bundle lookup"
    );

    // The step process inherits that environment.
    env.set("AMPLIHACK_HOME", amplihack_home);
    let _cwd = crate::test_support::CwdGuard::set(&worktree).unwrap();

    let expected_state_root = home.join(".amplihack");
    assert_eq!(
        amplihack_types::amplihack_state_root(),
        Some(expected_state_root.clone()),
        "hook state must go to $HOME/.amplihack, not the -w checkout"
    );
    let contract_dir = contract_home().expect("contract home resolves");
    assert_eq!(
        contract_dir, expected_state_root,
        "the contract file must go under $HOME/.amplihack, not the -w checkout"
    );

    // Write through both resolved locations, as a step would.
    let session_dir = amplihack_types::session_state_dir(Some("test-session"))
        .expect("a session state directory resolves");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(session_dir.join("probe.json"), "{}\n").unwrap();
    let contract = ensure_contract_file(&contract_dir).expect("contract file is written");
    assert_eq!(contract, contract_dir.join(CONTRACT_RELATIVE_PATH));

    assert!(session_dir.starts_with(&expected_state_root));
    assert!(contract.starts_with(&expected_state_root));
    assert_untouched(&checkout, &before);
}
