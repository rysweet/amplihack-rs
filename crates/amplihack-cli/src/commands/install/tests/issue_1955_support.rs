//! Fixtures shared by the `issue_1955_*` tests.
//!
//! Every fixture passes the binary's version in explicitly, or derives it from
//! `crate::VERSION`, so the tests hold for any release.

use super::super::tag_fetch::TagFetcher;
use super::helpers::*;
use super::issue_1526_implicit_downgrade::IsolatedHome;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A `Cargo.toml` naming the amplihack workspace and declaring `version`.
pub(super) fn write_workspace_toml(root: &Path, version: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"crates/amplihack-cli\"]\n\n\
             [workspace.package]\nversion = \"{version}\"\n"
        ),
    )
    .unwrap();
}

/// A complete source tree at `root` declaring `version`.
pub(super) fn versioned_tree(root: &Path, version: &str) -> PathBuf {
    create_source_repo(root);
    write_workspace_toml(root, version);
    root.to_path_buf()
}

/// A source tree that declares no version and isn't a git repository.
pub(super) fn unversioned_tree(root: &Path) -> PathBuf {
    create_source_repo(root);
    root.to_path_buf()
}

/// Marks `path` as a repository root so a walk-up stops there.
pub(super) fn mark_repository(path: &Path) {
    fs::create_dir_all(path.join(".git")).unwrap();
}

/// A staged `~/.amplihack` with `stamp` (`None`: no stamp file).
pub(super) fn staged_install(home: &Path, stamp: Option<&str>) -> PathBuf {
    create_minimal_staged_assets(home);
    let staged = home.join(".amplihack");
    if let Some(stamp) = stamp {
        fs::write(staged.join(".installed-version"), stamp).unwrap();
    }
    staged
}

/// The compatibility check always passes, so ordering is what is under test.
pub(super) fn accept_all(_candidate: &Path) -> Result<(), String> {
    Ok(())
}

type ProduceTree = dyn Fn(&Path) -> anyhow::Result<PathBuf>;

/// A [`TagFetcher`] that records its calls and never touches the network.
pub(super) struct FakeFetcher {
    pub(super) calls: RefCell<Vec<(semver::Version, Duration)>>,
    produce: Box<ProduceTree>,
}

impl FakeFetcher {
    pub(super) fn new(produce: impl Fn(&Path) -> anyhow::Result<PathBuf> + 'static) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            produce: Box::new(produce),
        }
    }

    /// Fetching succeeds with a tree declaring `version`, with `sentinel` in a
    /// bundle file so a test can tell the fetched tree was staged.
    pub(super) fn serving(version: &str) -> Self {
        let version = version.to_string();
        Self::new(move |destination| {
            let root = destination.join(format!("amplihack-rs-{version}"));
            versioned_tree(&root, &version);
            fs::write(
                root.join("amplifier-bundle/context/fetched-sentinel.txt"),
                &version,
            )
            .unwrap();
            Ok(root)
        })
    }

    /// Fetching fails as the real fetcher does when the network is unreachable.
    pub(super) fn failing(reason: &str) -> Self {
        let reason = reason.to_string();
        Self::new(move |_| {
            Err(anyhow::Error::new(
                super::super::tag_fetch::SourceFetchFailed::Failed {
                    version: "fake".to_string(),
                    reason: reason.clone(),
                },
            ))
        })
    }

    pub(super) fn call_count(&self) -> usize {
        self.calls.borrow().len()
    }
}

impl TagFetcher for FakeFetcher {
    fn fetch(
        &self,
        version: &semver::Version,
        destination: &Path,
        budget: Duration,
    ) -> anyhow::Result<PathBuf> {
        self.calls.borrow_mut().push((version.clone(), budget));
        (self.produce)(destination)
    }
}

/// An isolated HOME with stub binaries, for tests that run a real install. The
/// `git` first on PATH always fails, so no path through the installer can reach
/// the network; the old `--force-refresh` route ends there instead of cloning.
pub(super) struct InstallEnv {
    _home: tempfile::TempDir,
    pub(super) outside: tempfile::TempDir,
    pub(super) home: PathBuf,
    pub(super) env: IsolatedHome,
}

impl InstallEnv {
    pub(super) fn new() -> Self {
        let home_dir = crate::test_support::tempdir_outside_framework_roots();
        let outside = crate::test_support::tempdir_outside_framework_roots();
        let home = home_dir.path().to_path_buf();
        let stub_bin = outside.path().join("stub_bin");
        create_exe_stub(&stub_bin, "python3");
        let hooks_stub = create_exe_stub(&stub_bin, "amplihack-hooks");
        let runner_stub = create_exe_stub(&stub_bin, "recipe-runner-rs");
        write_script(&stub_bin.join("git"), "#!/bin/sh\nexit 1\n");

        let mut env = IsolatedHome::installed(&home);
        env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        env.set("RECIPE_RUNNER_RS_PATH", &runner_stub);
        env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
        env.set("AMPLIHACK_SKIP_MMDC", "1");
        env.set("AMPLIHACK_AGENT_BINARY", "claude");
        env.unset("AMPLIHACK_CLAUDE_PROFILE_ROOT");
        let mut path_dirs = vec![stub_bin];
        path_dirs.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        env.set("PATH", std::env::join_paths(path_dirs).unwrap());
        Self {
            _home: home_dir,
            outside,
            home,
            env,
        }
    }

    /// A path outside HOME for a source tree.
    pub(super) fn source_path(&self, name: &str) -> PathBuf {
        self.outside.path().join(name)
    }

    pub(super) fn staged_bundle(&self) -> PathBuf {
        self.home.join(".amplihack/amplifier-bundle")
    }

    pub(super) fn staged_file(&self, rel: &str) -> PathBuf {
        self.staged_bundle().join(rel)
    }
}

/// Write an executable script.
pub(super) fn write_script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A version strictly above and one strictly below `crate::VERSION`.
pub(super) fn newer_than_binary() -> String {
    let running = semver::Version::parse(crate::VERSION).expect("crate::VERSION is semver");
    format!("{}.0.0", running.major + 1)
}

pub(super) fn older_than_binary() -> String {
    let running = semver::Version::parse(crate::VERSION).expect("crate::VERSION is semver");
    if running.major > 0 {
        format!("{}.0.0", running.major - 1)
    } else if running.minor > 0 {
        format!("0.{}.0", running.minor - 1)
    } else {
        "0.0.0-0".to_string()
    }
}
