//! Isolated execution fixtures for the issue-reference policy acceptance tests.
//! All adapters are compiled Rust; no credentials, live hosting or disabled hooks.

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

pub const REPOSITORY: &str = "owner/repo";
pub const BRANCH: &str = "feat/issue-2550-fixture";

fn git_binary() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(|| {
        // Resolve Apple's developer-tool shim once before clearing the child
        // environment. Its cache updates can stall for each private HOME.
        #[cfg(target_os = "macos")]
        let binary = {
            let out = Command::new("/usr/bin/xcrun")
                .args(["--find", "git"])
                .output()
                .expect("xcrun must resolve the trusted Git executable");
            succeeds(&out);
            PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
        };
        #[cfg(not(target_os = "macos"))]
        let binary = PathBuf::from("/usr/bin/git");
        let binary = binary.canonicalize().unwrap();
        assert!(binary.is_file(), "Git executable is required");
        binary
    })
    .as_path()
}

pub fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_owned()
}

pub fn describe(out: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn succeeds(out: &Output) {
    assert!(out.status.success(), "{}", describe(out));
}

pub fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

pub fn recipe(name: &str) -> serde_yaml::Value {
    serde_yaml::from_str(
        &fs::read_to_string(source_root().join(format!("amplifier-bundle/recipes/{name}.yaml")))
            .unwrap(),
    )
    .unwrap()
}

pub fn step(name: &str, id: &str) -> serde_yaml::Value {
    recipe(name)["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing {name}/{id}"))
        .clone()
}

pub fn step_command(name: &str, id: &str) -> String {
    step(name, id)["command"]
        .as_str()
        .unwrap_or_else(|| panic!("{name}/{id} must be executable"))
        .to_owned()
}

fn adapter() -> &'static Path {
    static ADAPTER: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    extern "C" fn remove_adapter() {
        if let Some((directory, _)) = ADAPTER.get() {
            let _ = fs::remove_dir_all(directory.path());
        }
    }
    &ADAPTER
        .get_or_init(|| {
            let tmp = tempfile::Builder::new()
                .prefix("issue-2550-adapter-")
                .tempdir_in(super::scratch_base::scratch_base())
                .unwrap();
            // Resolve the toolchain before clearing HOME; rustup configuration is
            // not application/profile configuration and is never modified here.
            let sysroot = Command::new("rustc")
                .args(["--print", "sysroot"])
                .output()
                .expect("rustc is required to build the Rust adapters");
            succeeds(&sysroot);
            let compiler =
                Path::new(String::from_utf8(sysroot.stdout).unwrap().trim()).join("bin/rustc");
            let binary = tmp.path().join("adapter");
            let out = Command::new(compiler)
                .args(["--edition=2024", "--crate-name", "issue_2550_adapter"])
                .arg(source_root().join("crates/amplihack-cli/tests/support/issue_2550_adapter.rs"))
                .arg("-o")
                .arg(&binary)
                .env_clear()
                .env("HOME", tmp.path())
                .env("PATH", "/usr/bin:/bin")
                .output()
                .unwrap();
            succeeds(&out);
            // Statics do not run TempDir::drop when libtest exits. Every
            // adapter child has been reaped by then, so remove our own tree.
            // SAFETY: a C-ABI callback with process lifetime and no unwinding.
            assert_eq!(unsafe { libc::atexit(remove_adapter) }, 0);
            (tmp, binary)
        })
        .1
}

pub struct Fixture {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub main: PathBuf,
    pub worktree: PathBuf,
    pub bundle: PathBuf,
    pub bin: PathBuf,
    pub state: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("issue-2550-")
            .tempdir_in(super::scratch_base::scratch_base())
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let f = Self {
            main: root.join("main"),
            worktree: root.join("worktree"),
            bundle: root.join("home/.amplihack"),
            bin: root.join("bin"),
            state: root.join("state"),
            root,
            _tmp: tmp,
        };
        for d in [
            &f.main,
            &f.bin,
            &f.state,
            &f.root.join("tmp"),
            &f.root.join("home/.claude"),
            &f.root.join("home/.codex"),
            &f.root.join("profiles"),
            &f.bundle,
        ] {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for name in [
            "gh",
            "az",
            "amplihack",
            "pre-commit",
            "pre-push",
            "classifier",
            "checkpoint",
            "boundary",
            "timeout",
            "sleep",
            "curl",
            "wget",
            "claude",
            "codex",
            "copilot",
        ] {
            symlink(adapter(), f.bin.join(name)).unwrap();
        }
        // Shell helpers and direct fixture calls use the same trusted Git.
        symlink(git_binary(), f.bin.join("git")).unwrap();
        // Use the actual bundle helpers, but never a user-installed bundle.
        let tools = f.bundle.join("amplifier-bundle/tools");
        fs::create_dir_all(&tools).unwrap();
        for entry in fs::read_dir(source_root().join("amplifier-bundle/tools")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                super::exec_stub::copy_executable(&entry.path(), &tools.join(entry.file_name()));
            }
        }
        // Auto-drive pins a complete trusted launch bundle before resolving
        // nested recipes. Supply its real authority assets, never empty files.
        for relative in [
            "skills/merge-ready/SKILL.md",
            "skills/crusty-old-engineer/SKILL.md",
            "skills/merge-ready/pr-description-template.md",
            "skills/auto-drive-to-merge/evidence-contract.md",
            "recipes/loop-health-evaluator.yaml",
            "recipes/autodrive-finalize.yaml",
            "recipes/auto-drive-to-merge.yaml",
            "recipes/autodrive-build.yaml",
            "recipes/default-workflow.yaml",
            "recipes/workflow-publish.yaml",
        ] {
            let dest = f.bundle.join("amplifier-bundle").join(relative);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::copy(source_root().join("amplifier-bundle").join(relative), dest).unwrap();
        }
        succeeds(
            &f.command("/usr/bin/git")
                .current_dir(&f.main)
                .args(["init", "-b", "main"])
                .output()
                .unwrap(),
        );
        for args in [
            ["config", "user.name", "Policy Fixture"],
            ["config", "user.email", "policy@example.com"],
        ] {
            succeeds(
                &f.command("/usr/bin/git")
                    .current_dir(&f.main)
                    .args(args)
                    .output()
                    .unwrap(),
            );
        }
        fs::write(f.main.join("README.md"), "Fixture base\n").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "fixture base"]] {
            succeeds(
                &f.command("/usr/bin/git")
                    .current_dir(&f.main)
                    .args(args)
                    .output()
                    .unwrap(),
            );
        }
        let remote = f.root.join("remote.git");
        succeeds(
            &f.command("/usr/bin/git")
                .args(["init", "--bare"])
                .arg(&remote)
                .current_dir(&f.main)
                .output()
                .unwrap(),
        );
        let commands = [
            vec![
                "remote".into(),
                "add".into(),
                "origin".into(),
                "https://github.com/owner/repo.git".into(),
            ],
            vec![
                "config".into(),
                format!("url.{}.insteadOf", remote.display()),
                "https://github.com/owner/repo.git".into(),
            ],
            vec!["push".into(), "-u".into(), "origin".into(), "main".into()],
            vec![
                "symbolic-ref".into(),
                "refs/remotes/origin/HEAD".into(),
                "refs/remotes/origin/main".into(),
            ],
            vec![
                "worktree".into(),
                "add".into(),
                "-b".into(),
                BRANCH.into(),
                f.worktree.display().to_string(),
            ],
        ];
        for args in commands {
            succeeds(
                &f.command("/usr/bin/git")
                    .current_dir(&f.main)
                    .args(args)
                    .output()
                    .unwrap(),
            );
        }
        f.git(&["push", "-u", "origin", BRANCH]);
        let hooks = f.root.join("hooks");
        fs::create_dir(&hooks).unwrap();
        for name in ["pre-commit", "pre-push"] {
            symlink(adapter(), hooks.join(name)).unwrap();
        }
        f.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
        f
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut c = Command::new(program);
        amplihack_git::scrub(&mut c);
        c.env_clear()
            .current_dir(if self.worktree.is_dir() {
                &self.worktree
            } else {
                &self.main
            })
            .env(
                "PATH",
                format!(
                    "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                    self.bin.display()
                ),
            )
            .env("HOME", self.root.join("home"))
            // The HOME guard requires HOME beneath TMPDIR. The whole fixture
            // is private; hosting capture files have their own child directory.
            .env("TMPDIR", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("XDG_CACHE_HOME", self.root.join("home/.cache"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("home/.claude"))
            .env("CODEX_HOME", self.root.join("home/.codex"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.root.join("profiles"))
            .env("AMPLIHACK_HOME", &self.bundle)
            .env("AMPLIHACK_WORK_ROOT", self.root.join("runs"))
            .env("AMPLIHACK_TEST_HOME_GUARD", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("empty-gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("ISSUE_2550_STATE", &self.state)
            .env("ISSUE_2550_CLASSIFIER", self.bin.join("classifier"))
            .env("REPO_PATH", &self.main)
            .env("RECIPE_VAR_repo_path", &self.main)
            .env(
                "RECIPE_VAR_worktree_setup",
                serde_json::json!({
                "worktree_path": self.worktree, "branch_name": BRANCH,
                "base_ref": "origin/main" })
                .to_string(),
            )
            .env("REMOTE_HOST_TYPE", "github")
            .env("ISSUE_NUMBER", "42")
            .env("TASK_DESCRIPTION", "Fix reference selection")
            .env("RECIPE_VAR_task_description", "Fix reference selection")
            .env("RECIPE_VAR_base_branch", "main")
            .env(
                "RECIPE_VAR_version_bump",
                r#"{"change_classification":"PATCH"}"#,
            )
            .env("GH_RETRY_TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        c
    }

    pub fn cli(&self) -> Command {
        let mut c = crate::isolated_amplihack::isolated(super::amplihack_bin::amplihack_bin());
        let environment = self.command("/usr/bin/true");
        c.env_clear()
            .envs(
                environment
                    .get_envs()
                    .filter_map(|(k, v)| v.map(|v| (k, v))),
            )
            .current_dir(&self.worktree)
            .stdin(Stdio::null());
        c
    }

    pub fn workflow(&self, action: &str) -> Command {
        let mut c = self.cli();
        c.args(["workflow", "issue-reference", action]);
        c
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = self.command(git_binary()).args(args).output().unwrap();
        succeeds(&out);
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    pub fn change(&self, path: &str) {
        let path = self.worktree.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "Policy acceptance fixture\n").unwrap();
    }

    pub fn write(&self, name: &str, data: impl AsRef<[u8]>) {
        fs::write(self.state.join(name), data).unwrap();
    }

    pub fn read(&self, name: &str) -> String {
        fs::read_to_string(self.state.join(name)).unwrap_or_default()
    }

    pub fn bash(&self, command: &str) -> Command {
        // Resolve Bash from the deliberately constructed trusted PATH. On
        // macOS the production hook-repair helper requires current Bash.
        let mut c = self.command("bash");
        c.args(["-euo", "pipefail", "-c", command]);
        // Only workflow operations/hygiene run through the adapter; the policy
        // implementation itself always executes in the real current binary.
        c.env(
            "ISSUE_2550_REAL_AMPLIHACK",
            super::amplihack_bin::amplihack_bin(),
        );
        c
    }

    pub fn publication(&self, policy: Option<&str>) -> Output {
        let mut c = self.bash(&step_command("workflow-publish", "step-16-create-draft-pr"));
        if let Some(p) = policy {
            c.env("RECIPE_VAR_pr_issue_link", p);
        }
        c.output().unwrap()
    }

    pub fn commit(&self, policy: Option<&str>) -> Output {
        let mut c = self.bash(&step_command("workflow-publish", "step-15-commit-push"));
        if let Some(p) = policy {
            c.env("RECIPE_VAR_pr_issue_link", p);
        }
        c.output().unwrap()
    }

    pub fn edit(&self, baseline: &str, candidate: &str) -> Output {
        self.write("body", baseline);
        let mut c = self.edit_command(candidate);
        c.output().unwrap()
    }

    pub fn edit_command(&self, candidate: &str) -> Command {
        self.write("candidate.md", candidate);
        let mut c = self.workflow("edit-body");
        c.args(["--repo", REPOSITORY, "--pr", "99", "--candidate-file"])
            .arg(self.state.join("candidate.md"));
        c
    }
}

pub fn input(mut command: Command, bytes: &[u8]) -> Output {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Err(error) = child.stdin.take().unwrap().write_all(bytes) {
        // Validation may reject the invocation before consuming evidence.
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "stdin write: {error}"
        );
    }
    child.wait_with_output().unwrap()
}
