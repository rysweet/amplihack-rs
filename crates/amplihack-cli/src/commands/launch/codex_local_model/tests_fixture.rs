//! In-process fixture for the `codex_local_model` unit tests.
//!
//! Holds the crate's environment lock for its whole life, points `HOME` at
//! a fresh temporary directory, removes every variable the module reads
//! (so a local session that exported `AMPLIHACK_CODEX_LOCAL_MODEL` or
//! `AMPLIHACK_HOOK_TEXT` to `cargo test` changes nothing), and gives
//! the module a stub hooks binary through
//! `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`.
//!
//! [`LocalFixture::new`] asserts that `HOME` is inside its tempdir
//! before any test can call `prepare_launch` or `refresh`, so no test here
//! can touch a developer's real `~/.codex` or `~/.amplihack`.
#![allow(dead_code)]

use crate::test_support::{EnvGuard, env_lock};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

pub(super) const NAME: &str = "inkling";
pub(super) const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
pub(super) const HOOKS_OVERRIDE: &str = "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH";

pub(super) const PROFILE: &str = "model = \"inkling-2-mini\"\nmodel_provider = \"llamacpp-local\"\n\n[model_providers.llamacpp-local]\nbase_url = \"http://127.0.0.1:8080/v1\"\nwire_api = \"responses\"\n";

/// Variables the module reads, removed for every test and restored after.
const CLEARED: [&str; 7] = [
    "CODEX_HOME",
    super::LOCAL_MODEL_ENV,
    "AMPLIHACK_HOOK_TEXT",
    "AMPLIHACK_HOOK_PROTOCOL",
    "AMPLIHACK_USE_DOCKER",
    "AMPLIHACK_HOME",
    HOOKS_OVERRIDE,
];

pub(super) struct LocalFixture {
    dir: tempfile::TempDir,
    env: EnvGuard,
    // Dropped last: the environment is restored while the lock is held.
    _lock: MutexGuard<'static, ()>,
}

impl LocalFixture {
    pub(super) fn new() -> Self {
        let lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let dir = crate::test_support::tempdir_outside_framework_roots();
        let mut env = EnvGuard::capture(&[]);
        for name in CLEARED {
            env.clear(name);
        }
        env.assign("HOME", dir.path());
        let fixture = Self {
            dir,
            env,
            _lock: lock,
        };
        fixture.assert_home_is_private_to_the_test();

        let home = fixture.home().to_path_buf();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        for dir in [".amplihack", ".codex", "bin"] {
            fs::create_dir(home.join(dir)).unwrap();
            fs::set_permissions(home.join(dir), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(
            home.join(".codex").join(format!("{NAME}.config.toml")),
            PROFILE,
        )
        .unwrap();
        crate::test_support::write_executable_with_mode(
            &fixture.hooks_bin(),
            "#!/bin/sh\necho '{}'\n",
            0o700,
        );
        let mut fixture = fixture;
        let bin = fixture.hooks_bin();
        fixture.set(HOOKS_OVERRIDE, &bin);
        fixture
    }

    /// Verify that input and output paths are confined to this fixture.
    #[track_caller]
    pub(super) fn assert_home_is_private_to_the_test(&self) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        assert_eq!(
            home.as_deref(),
            Some(self.dir.path()),
            "HOME must be this test's tempdir before the module runs"
        );
    }

    pub(super) fn home(&self) -> &Path {
        self.dir.path()
    }

    pub(super) fn user_codex(&self) -> PathBuf {
        self.home().join(".codex")
    }

    pub(super) fn codex_local(&self) -> PathBuf {
        self.home().join(".amplihack/codex-local")
    }

    pub(super) fn generated(&self, name: &str) -> PathBuf {
        self.codex_local().join(name)
    }

    pub(super) fn hooks_bin(&self) -> PathBuf {
        self.home().join("bin/amplihack-hooks")
    }

    pub(super) fn write_profile(&self, name: &str, text: &str) {
        fs::write(self.user_codex().join(format!("{name}.config.toml")), text).unwrap();
    }

    pub(super) fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
        assert_ne!(key, "HOME", "HOME belongs to the fixture");
        self.env.assign(key, value);
    }

    pub(super) fn unset(&mut self, key: &str) {
        self.env.clear(key);
    }

    /// The selection by variable.
    pub(super) fn select(&mut self, name: &str) {
        self.set(super::LOCAL_MODEL_ENV, name);
    }
}

pub(super) fn no_conflicts() -> super::Conflicts {
    super::Conflicts {
        docker: None,
        auto: false,
    }
}

pub(super) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}
