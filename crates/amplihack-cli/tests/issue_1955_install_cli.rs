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
        Self::in_temp(temp)
    }

    fn in_temp(temp: tempfile::TempDir) -> Self {
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
                 \"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}}'\nexit 0\n# {padding}\n"
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

fn native_release(host: &Host) -> PathBuf {
    let source = host.path("release-native.rs");
    let native = host.path("release-native");
    fs::write(&source, include_str!("fixtures/issue_2496_release.rs")).unwrap();
    let compiler =
        Path::new(env!("CARGO")).with_file_name(if cfg!(windows) { "rustc.exe" } else { "rustc" });
    let build = std::process::Command::new(compiler)
        .env("RELEASE_FIXTURE_DEFAULT_VERSION", amplihack_cli::VERSION)
        .args(["--edition=2024", "-o"])
        .arg(&native)
        .arg(&source)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", text(&build));
    native
}

fn available_release(host: &Host) {
    let native = native_release(host);
    let available = host
        .home
        .join(".amplihack/releases")
        .join(amplihack_cli::VERSION);
    fs::create_dir_all(available.join("bin")).unwrap();
    for name in ["amplihack", "amplihack-hooks"] {
        fs::copy(&native, available.join("bin").join(name)).unwrap();
    }
    fs::write(available.join(".complete"), b"").unwrap();
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
    available_release(&host);

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
    // Executables are already available. This test isolates the development
    // build's framework-fetch refusal rather than contacting a release API.
    available_release(&host);
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

/// Issue #2496 process contracts. Keep these in the existing consolidated
/// target: there is no additional test binary or executable shell fixture.
#[cfg(unix)]
mod issue_2496 {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;

    fn isolated_case(name: &str, child_marker: Option<&str>, body: impl FnOnce()) {
        if child_marker.is_some_and(|key| std::env::var_os(key).is_some()) {
            body();
            return;
        }
        crate::support::quiet_process::in_quiet_process(module_path!(), name, || {
            let temp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
            // Initialise the shared subprocess fixture only after TMPDIR is
            // private: recipe TMPDIRs can live beneath the real ~/.amplihack.
            crate::support::quiet_process::child_set_var("TMPDIR", temp.path());
            body();
        });
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Contents {
        Directory,
        File(Vec<u8>),
        Link(PathBuf),
    }

    /// Compare entry types, bytes, modes and literal link targets. Never
    /// descend through a link, even if its target is outside the fixture.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Contents)> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u32, Contents)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                let contents = if metadata.file_type().is_symlink() {
                    Contents::Link(fs::read_link(&path).unwrap())
                } else if metadata.is_dir() {
                    Contents::Directory
                } else {
                    assert!(metadata.is_file(), "unexpected fixture entry: {path:?}");
                    Contents::File(fs::read(&path).unwrap())
                };
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    (metadata.permissions().mode() & 0o7777, contents),
                );
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    walk(root, &path, out);
                }
            }
        }
        let mut result = BTreeMap::new();
        walk(root, root, &mut result);
        result
    }

    fn assert_preserved(home: &Path, before: &BTreeMap<PathBuf, (u32, Contents)>, output: &Output) {
        let after = snapshot(home);
        let changed: Vec<_> = before
            .keys()
            .chain(after.keys())
            .filter(|path| before.get(*path) != after.get(*path))
            .collect();
        assert!(
            changed.is_empty(),
            "installation changed at {changed:?}: {}",
            text(output)
        );
    }

    struct Fixture {
        host: Host,
        source: PathBuf,
        release: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let host = Host::in_temp(
                tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap(),
            );
            let source = host.path("release-source");
            source_tree(&source, amplihack_cli::VERSION);
            fs::write(
                source.join("amplifier-bundle/context/issue-2496.txt"),
                b"matching source\n",
            )
            .unwrap();
            let running = semver::Version::parse(amplihack_cli::VERSION).unwrap();
            let older = if running.major > 0 {
                format!("{}.0.0", running.major - 1)
            } else if running.minor > 0 {
                format!("0.{}.0", running.minor - 1)
            } else {
                "0.0.0-0".to_owned()
            };
            assert!(semver::Version::parse(&older).unwrap() < running);
            let amp = host.home.join(".amplihack");
            let release = amp.join("releases").join(&older);
            fs::create_dir_all(release.join("bin")).unwrap();
            copy_tree(
                &source.join("amplifier-bundle"),
                &release.join("amplifier-bundle"),
            );
            fs::write(
                release.join("amplifier-bundle/context/issue-2496.txt"),
                b"older release\n",
            )
            .unwrap();
            fs::write(release.join(".complete"), b"").unwrap();
            fs::write(
                release.join(".installed-by"),
                format!("format=1\nversion={older}\nexe=\npid=1\npost_update=0\n"),
            )
            .unwrap();
            fs::write(amp.join(".installed-version"), &older).unwrap();
            fs::set_permissions(
                amp.join(".installed-version"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            symlink(
                PathBuf::from("releases")
                    .join(&older)
                    .join("amplifier-bundle"),
                amp.join("amplifier-bundle"),
            )
            .unwrap();
            let bin = host.home.join(".local/bin");
            fs::create_dir_all(&bin).unwrap();
            for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
                // Existing executable fixture; ordering comes from the install
                // stamp and release metadata, never a repository version bump.
                fs::copy(
                    host.stubs.join("amplihack-hooks"),
                    release.join("bin").join(name),
                )
                .unwrap();
                symlink(release.join("bin").join(name), bin.join(name)).unwrap();
            }
            for (dir, file, bytes) in [
                (
                    ".claude",
                    "settings.json",
                    b"{\"model\":\"sentinel\"}\n".as_slice(),
                ),
                (
                    ".codex",
                    "config.toml",
                    b"model = \"sentinel\"\n".as_slice(),
                ),
                (
                    "profiles/private",
                    "sentinel",
                    b"private profile\n".as_slice(),
                ),
            ] {
                fs::create_dir_all(host.home.join(dir)).unwrap();
                fs::write(host.home.join(dir).join(file), bytes).unwrap();
            }
            Self {
                host,
                source,
                release,
            }
        }

        fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
            let mut command = crate::isolated_amplihack::isolated(executable);
            command
                .current_dir(self.host.temp.path())
                .env("HOME", &self.host.home)
                .env("TMPDIR", self.host.temp.path().parent().unwrap())
                .env("AMPLIHACK_HOME", &self.source)
                .env("CLAUDE_CONFIG_DIR", self.host.home.join(".claude"))
                .env("CODEX_HOME", self.host.home.join(".codex"))
                .env(
                    "AMPLIHACK_CLAUDE_PROFILE_ROOT",
                    self.host.home.join("profiles"),
                )
                .env("XDG_CONFIG_HOME", self.host.home.join(".config"))
                .env("XDG_CACHE_HOME", self.host.home.join(".cache"))
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.host.stubs.display()),
                )
                .env(
                    "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                    self.host.stubs.join("amplihack-hooks"),
                )
                .env(
                    "RECIPE_RUNNER_RS_PATH",
                    self.host.stubs.join("recipe-runner-rs"),
                )
                .env("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1")
                .env("AMPLIHACK_SKIP_MMDC", "1")
                .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
                .env("AMPLIHACK_NONINTERACTIVE", "1")
                .env("AMPLIHACK_AGENT_BINARY", "claude")
                // Origin regressions must exercise the safeguard itself.
                .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL");
            command
        }

        fn checkout(&self) -> Command {
            self.command(amplihack_bin())
        }

        fn acquisition_tools(&self, native: &Path) {
            // Avoid an unrelated memory-consent prompt/write on Linux, where
            // RAM discovery uses /proc even with the fully isolated PATH.
            fs::write(
                self.host.home.join(".amplihack/config.toml"),
                "[node]\nheap_consent = true\n",
            )
            .unwrap();
            for tool in [
                "npm",
                "uv",
                "node",
                "cargo",
                "curl",
                "wget",
                "codex-code-mode-host",
            ] {
                fs::hard_link(native, self.host.stubs.join(tool)).unwrap();
            }
        }

        fn startup(&self, provider: &str) -> Command {
            self.startup_from(amplihack_bin(), provider)
        }

        fn startup_from(&self, executable: impl AsRef<std::ffi::OsStr>, provider: &str) -> Command {
            let mut command = self.command(executable);
            command
                .args([provider, "--subprocess-safe", "--no-reflection"])
                // No host tool directories: even a regression can only run
                // our Rust tripwire, never a real installer or credential helper.
                .env("PATH", &self.host.stubs)
                .env("CARGO_HOME", self.host.home.join(".cargo"))
                .env("RUSTUP_HOME", self.host.home.join(".rustup"))
                .env("CARGO_TARGET_DIR", self.host.home.join("target"))
                .env("npm_config_cache", self.host.home.join(".npm"))
                .env("UV_CACHE_DIR", self.host.home.join(".cache/uv"))
                .env("UV_TOOL_DIR", self.host.home.join(".local/share/uv/tools"))
                .env("XDG_DATA_HOME", self.host.home.join(".local/share"))
                .env("ACQUISITION_TRIPWIRE", self.host.path("installer-ran"))
                .env("ACQUISITION_QUERY", self.host.path("registry-query"))
                .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1")
                .env("IS_SANDBOX", "1");
            command
        }

        fn assert_refused(&self, executable: &Path) {
            // Also observe redirected release roots and the offered source.
            let before = snapshot(self.host.temp.path());
            let output = self
                .command(executable)
                .args(["recipe", "list"])
                .output()
                .unwrap();
            assert_preserved(self.host.temp.path(), &before, &output);
            assert!(
                output.status.success(),
                "usable assets permit dispatch: {}",
                text(&output)
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("executable"), "{}", text(&output));
            assert!(
                stderr.contains("automatic installation"),
                "{}",
                text(&output)
            );
            let origin_lines: Vec<_> = stderr
                .lines()
                .filter(|line| line.contains("automatic installation"))
                .collect();
            assert_eq!(
                origin_lines.len(),
                1,
                "one refusal per process: {}",
                text(&output)
            );
            assert!(
                !origin_lines[0].chars().any(char::is_control),
                "{origin_lines:?}"
            );
            assert!(
                !stderr.contains("framework assets re-staged"),
                "{}",
                text(&output)
            );
        }
    }

    /// `docs/features/self-heal-asset-restage.md` promises that one run under
    /// `RUST_LOG=amplihack_cli=debug` reports which check refused, that the
    /// refusal line itself names only the rule, and that the opt-out
    /// suppresses both because eligibility is never assessed.
    #[test]
    fn the_refusal_cause_is_reported_only_under_rust_log_debug() {
        isolated_case(
            "the_refusal_cause_is_reported_only_under_rust_log_debug",
            None,
            || {
                const CAUSE: &str = "executable-outside-installed-release";
                let fixture = Fixture::new();
                let before = snapshot(&fixture.host.home);

                let run = |extra: &[(&str, &str)]| {
                    let mut command = fixture.checkout();
                    command.args(["recipe", "list"]);
                    for (key, value) in extra {
                        command.env(key, value);
                    }
                    let output = command.output().unwrap();
                    let combined = format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    (output, combined)
                };

                // Default verbosity: the rule, not the particular check.
                let (output, quiet) = run(&[]);
                assert!(
                    quiet.contains("prevents automatic installation"),
                    "{}",
                    text(&output)
                );
                assert!(
                    !quiet.contains(CAUSE),
                    "the specific cause belongs at debug level: {}",
                    text(&output)
                );

                // The documented diagnostic command.
                let (output, verbose) = run(&[("RUST_LOG", "amplihack_cli=debug")]);
                let causes: Vec<_> = verbose
                    .lines()
                    .filter(|line| line.contains(CAUSE))
                    .collect();
                assert_eq!(
                    causes.len(),
                    1,
                    "one cause record per run: {}",
                    text(&output)
                );
                assert!(
                    !causes[0].chars().any(char::is_control),
                    "the cause must be sanitised: {causes:?}"
                );

                // The opt-out short-circuits before the assessment, so it
                // reports the bypass diagnostic rather than an origin cause.
                let (output, opted_out) = run(&[
                    ("RUST_LOG", "amplihack_cli=debug"),
                    ("AMPLIHACK_SKIP_AUTO_INSTALL", "1"),
                ]);
                assert!(
                    !opted_out.contains(CAUSE),
                    "an opted-out run assesses nothing: {}",
                    text(&output)
                );
                assert!(
                    !opted_out.contains("prevents automatic installation"),
                    "an opted-out run reports no origin refusal: {}",
                    text(&output)
                );

                assert_preserved(&fixture.host.home, &before, &output);
            },
        );
    }

    /// Compile a std-only Rust executable, rather than a shell shim that
    /// could accidentally delegate to a host npm, uv or network downloader.
    fn acquisition_native(root: &Path) -> PathBuf {
        let source = root.join("client.rs");
        let binary = root.join("client");
        fs::write(&source, include_str!("fixtures/issue_2496_client.rs")).unwrap();
        let output = Command::new("rustc")
            .arg("--edition=2024")
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        // Establish that the native fixture is ready before the CLI's bounded
        // health probe. In particular, macOS may delay a newly linked image
        // in dyld before main, independently of the stub's version behavior.
        let output = Command::new(&binary).arg("--version").output().unwrap();
        assert!(output.status.success(), "{}", text(&output));
        binary
    }

    fn assert_origin_refusal(output: &Output) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<_> = stderr
            .lines()
            .filter(|line| line.contains("prevents automatic installation"))
            .collect();
        assert_eq!(lines.len(), 1, "one origin warning: {}", text(output));
        assert!(lines[0].contains("executable location"), "{}", text(output));
        assert!(!lines[0].chars().any(char::is_control), "{lines:?}");
        assert!(
            !stderr.contains("framework assets re-staged"),
            "{}",
            text(output)
        );
    }

    /// Where the startup actually reached client acquisition and failed, the
    /// reason is stated positively. The previous form asserted the absence of
    /// the caller's own `Installation failed:` context, which coupled this
    /// guard to an unrelated error contract; the tripwire assertions already
    /// carry the claim that no installer ran.
    fn assert_acquisition_refused(output: &Output) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("refused because of executable origin"),
            "{}",
            text(output)
        );
    }

    #[test]
    fn missing_client_startup_refuses_every_provider_before_acquisition() {
        isolated_case(
            "missing_client_startup_refuses_every_provider_before_acquisition",
            None,
            || {
                let tools = tempfile::tempdir().unwrap();
                let native = acquisition_native(tools.path());
                // RustyClawd uses Claude acquisition when no preferred native
                // client exists; test its public entrypoint as well.
                for provider in ["claude", "copilot", "codex", "amplifier", "rustyclawd"] {
                    let fixture = Fixture::new();
                    fixture.acquisition_tools(&native);
                    let before = snapshot(&fixture.host.home);
                    let output = fixture.startup(provider).output().unwrap();
                    assert!(
                        !output.status.success(),
                        "missing {provider}: {}",
                        text(&output)
                    );
                    assert_origin_refusal(&output);
                    assert_acquisition_refused(&output);
                    assert!(
                        !fixture.host.path("installer-ran").exists(),
                        "{}",
                        text(&output)
                    );
                    assert_preserved(&fixture.host.home, &before, &output);
                    assert!(!fixture.host.home.join(".npm-global").exists());
                }
            },
        );
    }

    #[test]
    fn usable_client_startup_from_checkout_does_not_require_acquisition() {
        isolated_case(
            "usable_client_startup_from_checkout_does_not_require_acquisition",
            None,
            || {
                let fixture = Fixture::new();
                let native = acquisition_native(fixture.host.temp.path());
                fixture.acquisition_tools(&native);
                fs::hard_link(&native, fixture.host.stubs.join("codex")).unwrap();
                let before = snapshot(&fixture.host.home);
                let output = fixture.startup("codex").output().unwrap();
                assert!(output.status.success(), "{}", text(&output));
                assert!(String::from_utf8_lossy(&output.stdout).contains("CLIENT_LAUNCHED"));
                assert!(!fixture.host.path("installer-ran").exists());
                assert!(!fixture.host.path("registry-query").exists());
                assert_preserved(&fixture.host.home, &before, &output);
            },
        );
    }

    /// Automatic upgrades are interactive-only. A PTY on stdin reaches the
    /// real startup decision while stdout/stderr remain captured. Keep the
    /// master alive until the child finishes so stdin remains a terminal.
    fn terminal_stdin(command: &mut Command) -> fs::File {
        use std::os::fd::FromRawFd;
        use std::process::Stdio;
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty initialises two owned descriptors; null arguments
        // request the default terminal settings and window size.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut::<libc::termios>(),
                    std::ptr::null_mut::<libc::winsize>(),
                )
            },
            0
        );
        // SAFETY: these descriptors were just opened, and each is owned once.
        let input = unsafe { fs::File::from_raw_fd(master) };
        let terminal = unsafe { fs::File::from_raw_fd(slave) };
        command
            .stdin(Stdio::from(terminal))
            .env_remove("AMPLIHACK_NONINTERACTIVE");
        input
    }

    #[test]
    fn upgrade_startup_refuses_acquisition_and_keeps_the_usable_client() {
        isolated_case(
            "upgrade_startup_refuses_acquisition_and_keeps_the_usable_client",
            None,
            || {
                let tools = tempfile::tempdir().unwrap();
                let native = acquisition_native(tools.path());
                for (provider, suppress) in ["claude", "copilot", "codex"]
                    .into_iter()
                    .flat_map(|provider| [false, true].map(|suppress| (provider, suppress)))
                {
                    let fixture = Fixture::new();
                    fixture.acquisition_tools(&native);
                    let bin = fixture.host.home.join(".npm-global/bin");
                    fs::create_dir_all(&bin).unwrap();
                    fs::hard_link(&native, bin.join(provider)).unwrap();
                    if provider == "codex" {
                        fs::hard_link(&native, bin.join("codex-code-mode-host")).unwrap();
                    }
                    let stale = match provider {
                        "claude" => "@anthropic-ai/.claude-code-sentinel",
                        "copilot" => "@github/.copilot-sentinel",
                        "codex" => "@openai/.codex-sentinel",
                        _ => unreachable!(),
                    };
                    let stale = fixture
                        .host
                        .home
                        .join(".npm-global/lib/node_modules")
                        .join(stale);
                    fs::create_dir_all(&stale).unwrap();
                    fs::write(stale.join("sentinel"), b"preserve pending npm contents").unwrap();
                    let cache = fixture.host.home.join(".npm/_cacache");
                    fs::create_dir_all(&cache).unwrap();
                    fs::write(cache.join("sentinel"), b"preserve npm cache").unwrap();
                    let eligible = fixture.release.join("bin/eligible-amplihack");
                    let mut command = if suppress {
                        fs::hard_link(amplihack_bin(), &eligible).unwrap();
                        let mut command = fixture.startup_from(&eligible, provider);
                        command.env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
                        command
                    } else {
                        fixture.startup(provider)
                    };
                    let before = snapshot(&fixture.host.home);
                    let _terminal = terminal_stdin(&mut command);
                    let output = command.output().unwrap();
                    assert!(output.status.success(), "{provider}: {}", text(&output));
                    assert!(String::from_utf8_lossy(&output.stdout).contains("CLIENT_LAUNCHED"));
                    if !suppress {
                        assert_origin_refusal(&output);
                    }
                    assert!(
                        !fixture.host.path("registry-query").exists(),
                        "refused startup queried npm: {}",
                        text(&output)
                    );
                    assert!(
                        !fixture.host.path("installer-ran").exists(),
                        "{}",
                        text(&output)
                    );
                    assert_preserved(&fixture.host.home, &before, &output);
                }
            },
        );
    }

    #[test]
    fn eligible_missing_client_startup_reaches_the_isolated_installer() {
        isolated_case(
            "eligible_missing_client_startup_reaches_the_isolated_installer",
            None,
            || {
                let fixture = Fixture::new();
                let native = acquisition_native(fixture.host.temp.path());
                fixture.acquisition_tools(&native);
                let executable = fixture.release.join("bin/eligible-amplihack");
                fs::hard_link(amplihack_bin(), &executable).unwrap();
                // Same argv/environment as checkout startup, from an eligible
                // executable instead. Codex resolution precedes asset repair.
                let output = fixture.startup_from(&executable, "codex").output().unwrap();
                assert!(
                    !output.status.success(),
                    "tripwire must fail: {}",
                    text(&output)
                );
                assert!(
                    fixture.host.path("installer-ran").exists(),
                    "{}",
                    text(&output)
                );
                assert!(
                    text(&output).contains("isolated installer tripwire"),
                    "{}",
                    text(&output)
                );
                assert!(!text(&output).contains("prevents automatic installation"));
            },
        );
    }

    #[test]
    fn checkout_self_heal_preserves_an_older_complete_release_and_links() {
        isolated_case(
            "checkout_self_heal_preserves_an_older_complete_release_and_links",
            None,
            || {
                let fixture = Fixture::new();
                fixture.assert_refused(&amplihack_bin());
            },
        );
    }

    #[test]
    fn opt_out_alone_preserves_installation_from_an_eligible_release() {
        isolated_case(
            "opt_out_alone_preserves_installation_from_an_eligible_release",
            None,
            || {
                let fixture = Fixture::new();
                let executable = fixture.release.join("bin/eligible-amplihack");
                fs::hard_link(amplihack_bin(), &executable).unwrap();
                let published = fixture.host.path("eligible-link");
                symlink(&executable, &published).unwrap();
                let before = snapshot(fixture.host.temp.path());

                let output = fixture
                    .command(&published)
                    .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                    .args(["recipe", "list"])
                    .output()
                    .unwrap();

                assert!(output.status.success(), "{}", text(&output));
                assert_preserved(fixture.host.temp.path(), &before, &output);
                assert!(
                    text(&output).contains("AMPLIHACK_SKIP_AUTO_INSTALL"),
                    "{}",
                    text(&output)
                );
                assert!(
                    !text(&output).contains("prevents automatic installation"),
                    "eligible origin must leave suppression as the sole refusal: {}",
                    text(&output)
                );
            },
        );
    }

    #[test]
    fn artifact_guard_invocation_preserves_an_older_complete_release_and_links() {
        isolated_case(
            "artifact_guard_invocation_preserves_an_older_complete_release_and_links",
            None,
            || {
                let fixture = Fixture::new();
                let repo = fixture.host.path("repo");
                fs::create_dir(&repo).unwrap();
                let mut git = crate::isolated_amplihack::isolated("/usr/bin/git");
                let init = git.args(["init", "--quiet"]).arg(&repo).output().unwrap();
                assert!(init.status.success(), "{}", text(&init));
                let before = snapshot(&fixture.host.home);

                // Same CLI argv as the configured hook, without the wrapper's opt-out:
                // wrapper propagation has an independent Rust-child regression below.
                let output = fixture
                    .checkout()
                    // Source-fetch probes use a rejecting git stub; the guard
                    // needs the real git to recognise this private worktree.
                    .env("PATH", "/usr/bin:/bin")
                    .args(["hygiene", "artifact-guard", "--repo"])
                    .arg(&repo)
                    .args(["--mode", "pre-commit"])
                    .output()
                    .unwrap();

                assert!(output.status.success(), "{}", text(&output));
                assert_preserved(&fixture.host.home, &before, &output);
            },
        );
    }

    #[test]
    fn release_names_and_canonical_path_escapes_do_not_authorise_a_checkout() {
        isolated_case(
            "release_names_and_canonical_path_escapes_do_not_authorise_a_checkout",
            None,
            || {
                for case in [
                    "prefix-sibling",
                    "override-release",
                    "control-characters",
                    "escaped-executable",
                    "redirected-releases",
                    "redirected-install-root",
                    "untrusted-releases",
                    "untrusted-release",
                    "incomplete",
                    "retired",
                ] {
                    let fixture = Fixture::new();
                    let amp = fixture.host.home.join(".amplihack");
                    let fake = match case {
                        "prefix-sibling" => amp.join("releases-checkout/current/bin/amplihack"),
                        "override-release" => fixture.source.join("releases/current/bin/amplihack"),
                        "control-characters" => amp.join("checkout\n\u{1b}[2J/bin/amplihack"),
                        "retired" => amp.join("releases/.retired/current/bin/amplihack"),
                        _ => amp.join("releases/current/bin/amplihack"),
                    };
                    if case == "redirected-releases" {
                        let releases = amp.join("releases");
                        let redirected = fixture.host.path("checkout-releases");
                        fs::rename(&releases, &redirected).unwrap();
                        symlink(&redirected, &releases).unwrap();
                    }
                    if case == "redirected-install-root" {
                        let redirected = fixture.host.path("redirected-install");
                        fs::rename(&amp, &redirected).unwrap();
                        symlink(&redirected, &amp).unwrap();
                    }
                    fs::create_dir_all(fake.parent().unwrap()).unwrap();
                    if case == "untrusted-releases" {
                        fs::set_permissions(
                            amp.join("releases"),
                            fs::Permissions::from_mode(0o777),
                        )
                        .unwrap();
                    }
                    if case == "escaped-executable" {
                        symlink(amplihack_bin(), &fake).unwrap();
                    } else {
                        fs::hard_link(amplihack_bin(), &fake).unwrap();
                    }
                    let candidate = fake.parent().unwrap().parent().unwrap();
                    copy_tree(
                        &fixture.source.join("amplifier-bundle"),
                        &candidate.join("amplifier-bundle"),
                    );
                    fs::write(
                        candidate.join(".installed-by"),
                        format!(
                            "format=1\nversion={}\nexe=\npid=1\npost_update=0\n",
                            amplihack_cli::VERSION
                        ),
                    )
                    .unwrap();
                    if case != "incomplete" {
                        fs::write(
                            fake.parent().unwrap().parent().unwrap().join(".complete"),
                            b"",
                        )
                        .unwrap();
                    }
                    if case == "untrusted-release" {
                        fs::set_permissions(
                            fake.parent().unwrap().parent().unwrap(),
                            fs::Permissions::from_mode(0o777),
                        )
                        .unwrap();
                    }
                    fixture.assert_refused(&fake);
                }
            },
        );
    }

    #[test]
    fn unresolved_executable_provenance_refuses_without_installation_writes() {
        isolated_case(
            "unresolved_executable_provenance_refuses_without_installation_writes",
            Some("AMPLIHACK_2496_UNLINKED_CHILD"),
            || {
                const CHILD: &str = "AMPLIHACK_2496_UNLINKED_CHILD";
                if let Some(home) = std::env::var_os(CHILD) {
                    let home = PathBuf::from(home);
                    assert_eq!(std::env::var_os("HOME"), Some(home.as_os_str().to_owned()));
                    // Removing this private hard link cannot remove or change the
                    // original harness. Canonical provenance is now unavailable.
                    let executable = std::env::current_exe().unwrap();
                    assert!(executable.starts_with(&home));
                    fs::remove_file(executable).unwrap();
                    let before = snapshot(&home);
                    let args = ["amplihack", "recipe", "list"].map(OsString::from);

                    amplihack_cli::self_heal::ensure_assets_match_binary_version(&args)
                        .expect("unknown provenance must refuse repair without blocking dispatch");

                    assert_eq!(snapshot(&home), before);
                    return;
                }
                let fixture = Fixture::new();
                let executable = fixture.release.join("bin/unlinked-test-child");
                fs::hard_link(std::env::current_exe().unwrap(), &executable).unwrap();
                let output = fixture.command(&executable)
            .args([
                "--exact",
                "issue_1955_install_cli::issue_2496::unresolved_executable_provenance_refuses_without_installation_writes",
                "--nocapture",
            ])
            .env(CHILD, &fixture.host.home)
            .output().unwrap();
                assert!(output.status.success(), "{}", text(&output));
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                    "{}",
                    text(&output)
                );
            },
        );
    }

    #[test]
    fn complete_installed_release_can_still_self_heal_through_a_published_link() {
        isolated_case(
            "complete_installed_release_can_still_self_heal_through_a_published_link",
            None,
            || {
                let fixture = Fixture::new();
                let release = fixture
                    .host
                    .home
                    .join(".amplihack/releases")
                    .join(amplihack_cli::VERSION);
                fs::create_dir_all(release.join("bin")).unwrap();
                copy_tree(
                    &fixture.source.join("amplifier-bundle"),
                    &release.join("amplifier-bundle"),
                );
                fs::write(release.join(".complete"), b"").unwrap();
                fs::write(
                    release.join(".installed-by"),
                    format!(
                        "format=1\nversion={}\nexe=\npid=1\npost_update=0\n",
                        amplihack_cli::VERSION
                    ),
                )
                .unwrap();
                let executable = release.join("bin/amplihack");
                fs::hard_link(amplihack_bin(), &executable).unwrap();
                fs::copy(
                    native_release(&fixture.host),
                    release.join("bin/amplihack-hooks"),
                )
                .unwrap();
                let published = fixture.host.home.join(".local/bin/amplihack");
                fs::remove_file(&published).unwrap();
                symlink(&executable, &published).unwrap();
                let before =
                    fs::read_link(fixture.host.home.join(".amplihack/amplifier-bundle")).unwrap();

                let output = fixture
                    .command(fixture.host.home.join(".local/bin/amplihack"))
                    .args(["recipe", "list"])
                    .output()
                    .unwrap();

                assert!(output.status.success(), "{}", text(&output));
                assert_eq!(
                    fs::read_to_string(fixture.host.home.join(".amplihack/.installed-version"))
                        .unwrap()
                        .lines()
                        .next(),
                    Some(amplihack_cli::VERSION),
                    "an eligible executable must retain supported implicit repair: {}",
                    text(&output)
                );
                assert_ne!(
                    fs::read_link(fixture.host.home.join(".amplihack/amplifier-bundle")).unwrap(),
                    before
                );
            },
        );
    }

    #[test]
    fn explicit_release_source_install_still_publishes_a_complete_release() {
        isolated_case(
            "explicit_release_source_install_still_publishes_a_complete_release",
            None,
            || {
                let fixture = Fixture::new();
                let native = native_release(&fixture.host);
                let available = fixture
                    .host
                    .home
                    .join(".amplihack/releases")
                    .join(amplihack_cli::VERSION);
                fs::create_dir_all(available.join("bin")).unwrap();
                for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
                    fs::copy(&native, available.join("bin").join(name)).unwrap();
                }
                fs::write(available.join(".complete"), b"").unwrap();
                fs::write(
                    available.join(".installed-by"),
                    format!(
                        "format=1\nversion={}\nexe=\npid=1\npost_update=0\n",
                        amplihack_cli::VERSION
                    ),
                )
                .unwrap();
                let old_release = snapshot(&fixture.release);
                let mut command = fixture.checkout();
                command
                    .args(["install", "--force-refresh"])
                    .env("RELEASE_FIXTURE_VERSION", amplihack_cli::VERSION);
                let output = command.output().unwrap();

                assert!(output.status.success(), "{}", text(&output));
                let bundle =
                    fs::canonicalize(fixture.host.home.join(".amplihack/amplifier-bundle"))
                        .unwrap();
                assert!(bundle.starts_with(
                    fs::canonicalize(fixture.host.home.join(".amplihack/releases")).unwrap()
                ));
                assert!(bundle.parent().unwrap().join(".complete").is_file());
                let bundle_link = fixture.host.home.join(".amplihack/amplifier-bundle");
                let active_bundle = bundle_link
                    .parent()
                    .unwrap()
                    .join(fs::read_link(&bundle_link).unwrap());
                let active = active_bundle.parent().unwrap();
                for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
                    let link = fixture.host.home.join(".local/bin").join(name);
                    assert_eq!(fs::read_link(&link).unwrap(), active.join("bin").join(name));
                    assert!(
                        fs::read(&link).unwrap() == fs::read(&native).unwrap(),
                        "{name} must publish release bytes instead of checkout or PATH bytes"
                    );
                }
                assert_preserved(&fixture.release, &old_release, &output);
                assert_eq!(
                    fs::read(bundle.join("context/issue-2496.txt")).unwrap(),
                    b"matching source\n"
                );
                assert_eq!(
                    fs::read_to_string(fixture.host.home.join(".amplihack/.installed-version"))
                        .unwrap()
                        .lines()
                        .next(),
                    Some(amplihack_cli::VERSION)
                );
            },
        );
    }

    #[test]
    fn hook_wrapper_unconditionally_passes_the_opt_out_to_its_rust_child() {
        isolated_case(
            "hook_wrapper_unconditionally_passes_the_opt_out_to_its_rust_child",
            Some("AMPLIHACK_2496_WRAPPER_CHILD"),
            || {
                const CHILD: &str = "AMPLIHACK_2496_WRAPPER_CHILD";
                if std::env::var_os(CHILD).is_some() {
                    assert_eq!(
                        std::env::var_os("AMPLIHACK_SKIP_AUTO_INSTALL"),
                        Some(OsString::from("1"))
                    );
                    return;
                }
                let temp =
                    tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
                let test_name = "issue_1955_install_cli::issue_2496::hook_wrapper_unconditionally_passes_the_opt_out_to_its_rust_child";
                for inherited in [None, Some("0")] {
                    let mut command = crate::isolated_amplihack::isolated("/bin/bash");
                    command
                        .arg(workspace_root().join("scripts/git-hook-cargo.sh"))
                        .arg(std::env::current_exe().unwrap())
                        .args(["--exact", test_name, "--nocapture"])
                        .env("HOME", temp.path())
                        .env("TMPDIR", temp.path())
                        .env("XDG_CACHE_HOME", temp.path().join("cache"))
                        .env("CARGO_TARGET_DIR", temp.path().join("target"))
                        .env("AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS", "0")
                        .env(CHILD, "1");
                    if let Some(value) = inherited {
                        command.env("AMPLIHACK_SKIP_AUTO_INSTALL", value);
                    } else {
                        command.env_remove("AMPLIHACK_SKIP_AUTO_INSTALL");
                    }
                    let output = command.output().unwrap();
                    assert!(
                        output.status.success(),
                        "wrapper child with {inherited:?}: {}",
                        text(&output)
                    );
                    assert!(
                        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                        "child must actually run: {}",
                        text(&output)
                    );
                }
            },
        );
    }

    #[test]
    fn ordinary_isolated_subprocesses_receive_the_opt_out() {
        isolated_case(
            "ordinary_isolated_subprocesses_receive_the_opt_out",
            Some("AMPLIHACK_2496_ORDINARY_CHILD"),
            || {
                const CHILD: &str = "AMPLIHACK_2496_ORDINARY_CHILD";
                if std::env::var_os(CHILD).is_some() {
                    assert_eq!(
                        std::env::var_os("AMPLIHACK_SKIP_AUTO_INSTALL"),
                        Some(OsString::from("1"))
                    );
                    return;
                }
                let mut command =
                    crate::isolated_amplihack::isolated(std::env::current_exe().unwrap());
                let output = command.args([
            "--exact",
            "issue_1955_install_cli::issue_2496::ordinary_isolated_subprocesses_receive_the_opt_out",
            "--nocapture",
        ]).env(CHILD, "1").output().unwrap();
                assert!(output.status.success(), "{}", text(&output));
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                    "{}",
                    text(&output)
                );
            },
        );
    }
}
