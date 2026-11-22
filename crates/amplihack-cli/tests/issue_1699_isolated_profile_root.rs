//! Issue #1699: a child started through `isolated(..)` must never see the
//! developer's `AMPLIHACK_CLAUDE_PROFILE_ROOT`, or an `amplihack install` under
//! a temp HOME would refresh the real profiles' settings.json.
//!
//! `env_clear()` is the main control; `isolated(..)` also removes
//! `PROFILE_ROOT_ENV` explicitly after copying the kept variables, so a future
//! change to the kept list cannot let it through. After `env_clear()`, std's
//! `env_remove` deletes the key rather than recording `(key, None)`, so the
//! environment test only catches a kept list that copies the variable; it
//! still passes with the `env_remove` line deleted. The source assertion pins
//! that line. Nothing is spawned.

use crate::isolated_amplihack::{PROFILE_ROOT_ENV, isolated};
use crate::support::quiet_process::{child_set_var, in_quiet_process};
use std::ffi::OsStr;
use std::process::Command;

fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

#[test]
fn isolated_never_passes_the_profile_root() {
    in_quiet_process(
        module_path!(),
        "isolated_never_passes_the_profile_root",
        || {
            child_set_var(PROFILE_ROOT_ENV, "/home/someone/.claude-profiles");
            let cmd = isolated("x");
            assert!(
                matches!(env_of(&cmd, PROFILE_ROOT_ENV), None | Some(None)),
                "isolated() must never pass {PROFILE_ROOT_ENV} to the child"
            );
        },
    );
}

#[test]
fn profile_root_env_name_matches_the_binary() {
    assert_eq!(PROFILE_ROOT_ENV, "AMPLIHACK_CLAUDE_PROFILE_ROOT");
    let source = include_str!("../../amplihack-utils/src/rotator_status/root.rs");
    assert!(
        source.contains(&format!("\"{PROFILE_ROOT_ENV}\"")),
        "the binary reads its profile root from {PROFILE_ROOT_ENV}"
    );
}

#[test]
fn isolated_removes_the_profile_root_after_the_kept_vars() {
    let source = include_str!("../../../tests/support/isolated_amplihack.rs");
    let kept_loop = source
        .find("for (key, value) in std::env::vars_os()")
        .expect("isolated(..) copies the kept variables in a loop");
    let removal = source
        .find("command.env_remove(PROFILE_ROOT_ENV);")
        .expect("isolated(..) must remove PROFILE_ROOT_ENV explicitly");
    assert!(
        removal > kept_loop,
        "isolated(..) must remove {PROFILE_ROOT_ENV} after copying the kept variables"
    );
}

/// An integration test that calls `run_install` in process is outside both
/// `isolated(..)` and the library's `cfg!(test)` gate, so the refresh runs
/// against whatever root it inherited. Each such file must remove the
/// variable itself.
#[test]
fn in_process_installs_remove_the_profile_root() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        // This file names the patterns it searches for.
        if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with(file!()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        if source.contains("run_install(None") || source.contains("run_install(Some") {
            let removal = format!("child_remove_var(\"{PROFILE_ROOT_ENV}\")");
            if !source.contains(&removal) {
                offenders.push(path);
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these tests call run_install in process without removing \
         {PROFILE_ROOT_ENV}: {offenders:?}"
    );
}

// ── PrivateEnv: a fixture root of the test's own (issue #2931, GUARD and
// BSDGROUP) ────────────────────────────────────────────────────────────────
//
// A fixture that sets `HOME` by hand to a folder it made leaves the profile
// root, the process table and `TMPDIR` to whatever the test process had, and
// on BSD file systems (macOS) a folder made under a parent with a foreign
// group inherits that group. `private_env_in(base)` makes one root with the
// group set before anything is created in it, and `command(..)` builds on
// `isolated(..)` so that every one of those variables points inside it.

mod private_env {
    use super::{env_of, in_quiet_process};
    use crate::isolated_amplihack::{PROC_ROOT_ENV, PROFILE_ROOT_ENV, private_env_in};
    use crate::support::quiet_process::child_set_var;
    use std::ffi::OsStr;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn egid() -> u32 {
        // SAFETY: getegid has no preconditions.
        unsafe { libc::getegid() }
    }

    fn value(cmd: &std::process::Command, key: &str) -> std::path::PathBuf {
        match env_of(cmd, key) {
            Some(Some(v)) => Path::new(v).to_path_buf(),
            other => panic!("{key} is not set on the command: {other:?}"),
        }
    }

    #[test]
    fn the_root_is_private_canonical_and_in_the_effective_group() {
        let base = tempfile::tempdir().unwrap();
        let env = private_env_in(base.path());
        let root = env.home().parent().expect("home is inside the root");
        let meta = std::fs::metadata(root).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o700, "{}", root.display());
        assert_eq!(
            meta.gid(),
            egid(),
            "the group is set before any entry is made"
        );
        assert_eq!(root, root.canonicalize().unwrap(), "the root is canonical");
        assert!(root.starts_with(base.path().canonicalize().unwrap()));
    }

    #[test]
    fn what_is_made_inside_the_root_has_the_effective_group() {
        let base = tempfile::tempdir().unwrap();
        let env = private_env_in(base.path());
        let made = env.home().join("made-by-the-test");
        std::fs::write(&made, b"x").unwrap();
        assert_eq!(std::fs::metadata(env.home()).unwrap().gid(), egid());
        assert_eq!(std::fs::metadata(&made).unwrap().gid(), egid());
    }

    #[test]
    fn two_roots_made_under_one_base_differ() {
        let base = tempfile::tempdir().unwrap();
        let (a, b) = (private_env_in(base.path()), private_env_in(base.path()));
        assert_ne!(a.home(), b.home());
    }

    #[test]
    fn the_command_points_every_user_state_variable_inside_the_root() {
        let base = tempfile::tempdir().unwrap();
        let env = private_env_in(base.path());
        let root = env.home().parent().unwrap().to_path_buf();
        let cmd = env.command("x");
        assert_eq!(value(&cmd, "HOME"), root.join("home"));
        assert_eq!(value(&cmd, "TMPDIR"), root);
        assert_eq!(value(&cmd, PROFILE_ROOT_ENV), root.join("profiles"));
        assert!(value(&cmd, PROC_ROOT_ENV).starts_with(&root));
        for var in ["CLAUDE_CONFIG_DIR", "AMPLIHACK_HOME"] {
            assert!(value(&cmd, var).starts_with(&root), "{var}");
        }
        assert!(root.join("home").is_dir());
        assert!(
            value(&cmd, PROC_ROOT_ENV).is_dir(),
            "the process table is seeded, so the binary finds it"
        );
    }

    #[test]
    fn the_command_keeps_the_isolation_of_isolated() {
        let base = tempfile::tempdir().unwrap();
        let env = private_env_in(base.path());
        let cmd = env.command("x");
        assert_eq!(
            env_of(&cmd, crate::isolated_amplihack::HOME_GUARD_ENV),
            Some(Some(OsStr::new("1")))
        );
        assert_eq!(
            env_of(&cmd, crate::isolated_amplihack::ROTATE_AUTOSTART_ENV),
            Some(Some(OsStr::new("0")))
        );
    }

    /// The child's environment is cleared first, so credentials in the test
    /// process stay there.
    #[test]
    fn the_command_carries_no_credential_from_the_test_process() {
        const CREDENTIALS: [&str; 8] = [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "COPILOT_GITHUB_TOKEN",
            "SSH_AUTH_SOCK",
            "AWS_SECRET_ACCESS_KEY",
            "AZURE_CLIENT_SECRET",
        ];
        in_quiet_process(
            module_path!(),
            "the_command_carries_no_credential_from_the_test_process",
            || {
                for name in CREDENTIALS {
                    child_set_var(name, "credential-canary");
                }
                let base = tempfile::tempdir().unwrap();
                let cmd = private_env_in(base.path()).command("x");
                for name in CREDENTIALS {
                    assert!(
                        matches!(env_of(&cmd, name), None | Some(None)),
                        "{name} reached the child"
                    );
                }
                for (key, value) in cmd.get_envs() {
                    assert_ne!(
                        value,
                        Some(OsStr::new("credential-canary")),
                        "{key:?} carries a credential"
                    );
                }
            },
        );
    }
}
