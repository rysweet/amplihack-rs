//! Real Git boundaries and isolated profiles for resolver integration tests.
//!
//! Callers of the isolated repository hold their environment mutex throughout.
#![allow(dead_code)]

/// Pure layer tests need a walk-up stop, without consulting profile state.
pub fn bounded_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir_in("/var/tmp").unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

pub struct BoundedRepo {
    repo: tempfile::TempDir,
    _profile: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl BoundedRepo {
    pub fn path(&self) -> &std::path::Path {
        self.repo.path()
    }
}

impl Drop for BoundedRepo {
    fn drop(&mut self) {
        for (key, value) in self.previous.iter().rev() {
            // SAFETY: callers serialise environment access for this lifetime.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

pub fn isolated_bounded_tempdir() -> BoundedRepo {
    let profile = tempfile::tempdir_in("/var/tmp").unwrap();
    let mut previous = Vec::new();
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "AMPLIHACK_HOME",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
        "AMPLIHACK_RUNTIME_DIR",
        "TMPDIR",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_NOSYSTEM",
    ] {
        previous.push((key, std::env::var_os(key)));
        let value = match key {
            "GIT_CONFIG_GLOBAL" => std::path::PathBuf::from("/dev/null"),
            "GIT_CONFIG_NOSYSTEM" => std::path::PathBuf::from("1"),
            "TMPDIR" => profile.path().to_owned(),
            _ => {
                let path = profile.path().join(key);
                std::fs::create_dir(&path).unwrap();
                path
            }
        };
        // SAFETY: callers hold the environment mutex before fixture creation.
        unsafe { std::env::set_var(key, value) };
    }
    let repo = tempfile::tempdir().unwrap();
    assert!(
        amplihack_git::command_in(repo.path())
            .args(["init", "--quiet", "--template="])
            .status()
            .unwrap()
            .success()
    );
    BoundedRepo {
        repo,
        _profile: profile,
        previous,
    }
}
