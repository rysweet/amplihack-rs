//! Issue #1955 at the process level: what the real `amplihack` binary prints
//! and exits with. In-process tests can't see stdout, and this is where the
//! false "Bundled framework source not found" message was reported.
//!
//! Each `git` here is a logging script that fails, and the build under test may
//! not fetch, so nothing reaches the network.

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("amplifier-bundle").is_dir() && dir.join("Cargo.toml").is_file())
        .expect("the workspace root")
        .to_path_buf()
}

/// Copies regular files and directories; symlinks aren't staged anyway.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A source tree with the real bundle, declaring `version`.
fn source_tree(root: &Path, version: &str) {
    copy_tree(
        &workspace_root().join("amplifier-bundle"),
        &root.join("amplifier-bundle"),
    );
    fs::write(root.join("CLAUDE.md"), "root\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"crates/amplihack-cli\"]\n\n\
             [workspace.package]\nversion = \"{version}\"\n"
        ),
    )
    .unwrap();
}

struct Host {
    temp: tempfile::TempDir,
    home: PathBuf,
    stubs: PathBuf,
}

impl Host {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let stubs = temp.path().join("stubs");
        fs::create_dir_all(&stubs).unwrap();
        let padding = "x".repeat(1100);
        write_executable(
            &stubs.join("amplihack-hooks"),
            format!("#!/bin/sh\nexit 0\n# {padding}\n"),
            0o755,
        );
        write_executable(
            &stubs.join("recipe-runner-rs"),
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{{\"schema_version\":1,\"version\":\"fixture\",\
                 \"capabilities\":[\"codex_exec\"]}}'\nexit 0\n# {padding}\n"
            ),
            0o755,
        );
        write_executable(
            &stubs.join("git"),
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
                temp.path().join("git.log").display()
            ),
            0o755,
        );
        Self { temp, home, stubs }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn git_log(&self) -> String {
        fs::read_to_string(self.path("git.log")).unwrap_or_default()
    }

    fn install(&self, args: &[&str], amplihack_home: Option<&Path>, cwd: &Path) -> Output {
        // `isolated` clears the environment and sets the test-home guard;
        // `HOME` is then pointed at this host's own temp home.
        let mut command = crate::isolated_amplihack::isolated(amplihack_bin());
        command
            .arg("install")
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.stubs.display()),
            )
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                self.stubs.join("amplihack-hooks"),
            )
            .env("RECIPE_RUNNER_RS_PATH", self.stubs.join("recipe-runner-rs"))
            .env("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1")
            .env("AMPLIHACK_SKIP_MMDC", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .stdin(std::process::Stdio::null());
        if let Some(path) = amplihack_home {
            command.env("AMPLIHACK_HOME", path);
        }
        command.output().unwrap()
    }
}

fn text(output: &Output) -> String {
    format!(
        "--- stdout\n{}\n--- stderr\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const SOURCE_ONLY_MESSAGE: &str = "Bundled framework source not found";

/// The older version a release or dev build of this tree would refuse.
fn older_version() -> String {
    let running = semver::Version::parse(amplihack_cli::VERSION).unwrap();
    format!(
        "{}.{}.{}",
        running.major.saturating_sub(1),
        running.minor,
        running.patch
    )
}

#[test]
fn force_refresh_with_a_same_version_amplihack_home_prints_no_false_message() {
    let host = Host::new();
    let tree = host.path("tag-worktree");
    source_tree(&tree, amplihack_cli::VERSION);
    fs::write(tree.join("amplifier-bundle/context/sentinel-1955.txt"), "y").unwrap();

    let output = host.install(&["--force-refresh"], Some(&tree), host.temp.path());

    let all = text(&output);
    assert!(output.status.success(), "{all}");
    assert!(!all.contains(SOURCE_ONLY_MESSAGE), "{all}");
    assert!(
        all.contains(&format!(
            "Using framework source {} (v{}, from AMPLIHACK_HOME)",
            tree.display(),
            amplihack_cli::VERSION
        )),
        "{all}"
    );
    assert_eq!(
        host.git_log().matches(" fetch ").count(),
        0,
        "no fetch was needed"
    );
    assert!(
        host.home
            .join(".amplihack/amplifier-bundle/context/sentinel-1955.txt")
            .is_file(),
        "{all}"
    );
    assert_eq!(
        fs::read_to_string(host.home.join(".amplihack/.installed-version"))
            .unwrap()
            .lines()
            .next(),
        Some(amplihack_cli::VERSION)
    );
}

#[test]
fn install_with_an_older_amplihack_home_exits_non_zero_and_stages_nothing() {
    let host = Host::new();
    let tree = host.path("old");
    source_tree(&tree, &older_version());

    let output = host.install(&[], Some(&tree), host.temp.path());

    let all = text(&output);
    assert!(!output.status.success(), "{all}");
    assert!(
        all.contains("refusing to stage the framework bundle"),
        "{all}"
    );
    assert!(
        all.contains(&format!("source version: v{}", older_version())),
        "{all}"
    );
    assert!(
        all.contains(&format!("install --local {}", tree.display())),
        "{all}"
    );
    assert!(
        !host.home.join(".amplihack/amplifier-bundle").exists(),
        "{all}"
    );
    assert!(
        !host.home.join(".local/bin/amplihack").exists(),
        "the refusal comes before binaries are deployed: {all}"
    );
}

#[test]
fn force_refresh_on_a_dev_build_with_an_older_home_never_goes_to_the_network() {
    let host = Host::new();
    let tree = host.path("old");
    source_tree(&tree, &older_version());

    let output = host.install(&["--force-refresh"], Some(&tree), host.temp.path());

    let all = text(&output);
    assert!(!output.status.success(), "{all}");
    assert!(all.contains("cannot fetch a release tag"), "{all}");
    assert!(all.contains("install --local"), "{all}");
    assert!(!all.contains(SOURCE_ONLY_MESSAGE), "{all}");
    let git = host.git_log();
    assert!(!git.contains("clone") && !git.contains("fetch"), "{git}");
    assert!(
        !host.home.join(".amplihack/amplifier-bundle").exists(),
        "{all}"
    );
}
