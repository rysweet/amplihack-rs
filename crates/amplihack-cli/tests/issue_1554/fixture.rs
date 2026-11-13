//! Real Git fixtures for #1554. No ignore rules, host profiles or network.
//!
//! The recipe contexts below are requirements and the reviewed source plan,
//! not a candidate inventory masquerading as authority. Production must bind
//! them to its established task scope before any checkpoint or publication.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const ALLOWED: &[&str] = &["src/allowed.rs", "src/new.rs", "src/deleted.rs"];
pub const RUNTIME: &[&str] = &[
    ".amplihack/graph_db",
    ".amplihack/kuzu_db/data",
    ".amplihack/indexes/code.idx",
    ".amplihack/index.scip",
    ".amplihack/index.scip.backup",
    ".amplihack/indexing.pid",
    ".amplihack/blarify.json",
    ".amplihack/locks/index.lock",
    ".claude/runtime/launcher_context.json",
    ".claude/runtime/sessions.jsonl",
    ".claude/runtime/sessions.jsonl.lock",
    ".claude/runtime/locks/.workflow_active",
    ".claude/runtime/metrics/post_tool_use_metrics.jsonl",
    ".claude/runtime/classification_state/task.json",
    "worktrees/scratch/runtime.txt",
];

/// Run every regression in a quiet child before allocating fixtures. Recipe
/// sessions can inherit TMPDIR beneath a real profile; these tests must not.
pub fn sandbox(module: &str, name: &str, body: impl FnOnce()) {
    crate::support::quiet_process::in_quiet_process(module, name, || {
        use crate::support::quiet_process::child_set_var;
        let base = Path::new("/var/tmp")
            .canonicalize()
            .expect("private test scratch base");
        let profile = tempfile::Builder::new()
            .prefix("issue-1554-profile-")
            .tempdir_in(&base)
            .unwrap();
        child_set_var("TMPDIR", &base);
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "AMPLIHACK_HOME",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
        ] {
            let path = profile.path().join(key);
            fs::create_dir(&path).unwrap();
            child_set_var(key, path);
        }
        body();
    });
}

pub fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

pub fn executable(path: impl AsRef<Path>, body: &str) {
    fs::create_dir_all(path.as_ref().parent().unwrap()).unwrap();
    crate::support::exec_stub::write_executable(path.as_ref(), body, 0o755);
}

pub fn describe(out: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn success(out: &Output) {
    assert!(out.status.success(), "{}", describe(out));
}

pub fn recipe(name: &str) -> serde_yaml::Value {
    let path = source_root().join(format!("amplifier-bundle/recipes/{name}.yaml"));
    serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
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

pub struct Repo {
    pub temp: tempfile::TempDir,
    pub root: PathBuf,
    pub main: PathBuf,
    pub work: PathBuf,
    pub remote: PathBuf,
    pub home: PathBuf,
    pub bin: PathBuf,
    pub base: String,
}

impl Repo {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let main = root.join("main");
        let work = root.join("task");
        let remote = root.join("origin.git");
        let home = root.join("profile");
        let bin = root.join("bin");
        for p in [&main, &home, &bin, &root.join("empty-template")] {
            fs::create_dir_all(p).unwrap();
        }
        let mut r = Self {
            temp,
            root,
            main,
            work,
            remote,
            home,
            bin,
            base: String::new(),
        };
        r.git(&r.main, &["init", "-q", "-b", "main"]);
        write(
            r.main.join(".git/info/exclude"),
            "# disposable clone exclusions\n",
        );
        r.git(&r.main, &["config", "user.name", "Issue 1554"]);
        r.git(
            &r.main,
            &["config", "user.email", "issue1554@example.invalid"],
        );
        r.git(&r.main, &["config", "commit.gpgsign", "false"]);
        for p in [
            "README.md",
            "user.txt",
            "src/allowed.rs",
            "src/deleted.rs",
            "unrelated.txt",
        ] {
            write(r.main.join(p), format!("initial {p}\n"));
            r.git(&r.main, &["add", "--", p]);
        }
        r.git(&r.main, &["commit", "-qm", "initial source manifest"]);
        r.base = r.oid(&r.main, "HEAD");
        r.git(
            &r.root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                r.remote.to_str().unwrap(),
            ],
        );
        r.git(
            &r.main,
            &["remote", "add", "origin", r.remote.to_str().unwrap()],
        );
        r.git(&r.main, &["push", "-q", "-u", "origin", "main"]);
        r.git(
            &r.main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/1554",
                r.work.to_str().unwrap(),
            ],
        );
        r.git(&r.work, &["push", "-q", "-u", "origin", "HEAD"]);
        // Any accidental GitHub access fails immediately instead of using a
        // host login. All intended remotes above are local bare repositories.
        executable(
            r.bin.join("gh"),
            "#!/bin/sh\necho 'unexpected GitHub access' >&2\nexit 97\n",
        );
        r.install_binary();
        r
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = crate::isolated_amplihack::isolated(program);
        c.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("xdg"))
            .env("CLAUDE_CONFIG_DIR", self.home.join("claude"))
            .env("CODEX_HOME", self.home.join("codex"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("AMPLIHACK_HOME", source_root())
            .env("AMPLIHACK_RUNTIME_DIR", self.root.join("external-runtime"))
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TEMPLATE_DIR", self.root.join("empty-template"))
            .env(
                "PATH",
                std::env::join_paths(std::iter::once(self.bin.clone()).chain(
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                ))
                .unwrap(),
            );
        c
    }

    pub fn git_output(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut c = amplihack_git::command_in(cwd);
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("xdg"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TEMPLATE_DIR", self.root.join("empty-template"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args);
        c.output().unwrap()
    }

    pub fn git(&self, cwd: &Path, args: &[&str]) -> Vec<u8> {
        let out = self.git_output(cwd, args);
        success(&out);
        out.stdout
    }

    pub fn oid(&self, cwd: &Path, rev: &str) -> String {
        String::from_utf8(self.git(cwd, &["rev-parse", rev]))
            .unwrap()
            .trim()
            .to_owned()
    }

    pub fn staged(&self) -> Vec<u8> {
        self.git(&self.work, &["ls-files", "--stage", "-z"])
    }

    pub fn staged_patch(&self) -> Vec<u8> {
        self.git(
            &self.work,
            &[
                "diff",
                "--cached",
                "--binary",
                "--full-index",
                "--no-ext-diff",
            ],
        )
    }

    pub fn inventory(&self, cwd: &Path) -> Vec<u8> {
        // Deliberately no user or XDG configuration, as in mistt-infra's
        // manifest check. A global ignore must not make this test pass.
        let out = amplihack_git::command_in(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()
            .unwrap();
        success(&out);
        out.stdout
    }

    pub fn shell(&self, script: &str) -> Command {
        let mut c = self.command("bash");
        c.args(["-c", script, "issue-1554"]);
        c
    }

    pub fn prepare(&self, cwd: &Path) -> Output {
        self.shell("set -euo pipefail; . \"$1\"; preflight_known_workflow_runtime_artifacts .")
            .arg(source_root().join("amplifier-bundle/tools/workflow_runtime_artifacts.sh"))
            .current_dir(cwd)
            .output()
            .unwrap()
    }

    pub fn install_binary(&self) {
        let binary = crate::support::amplihack_bin::amplihack_bin();
        let destination = self.bin.join("amplihack");
        if !destination.exists() {
            std::os::unix::fs::symlink(binary, destination).unwrap();
        }
    }

    pub fn step_command(&self, recipe_name: &str, id: &str) -> Command {
        let s = step(recipe_name, id);
        let script = s["command"].as_str().expect("bash step");
        let mut c = self.shell(script);
        let setup =
            serde_json::json!({"worktree_path": self.work, "branch_name": "feat/1554"}).to_string();
        let requirements = serde_json::json!({"version": 1, "task": "1554-fixture", "permitted_source_paths": ALLOWED});
        let design =
            serde_json::json!({"version": 1, "task": "1554-fixture", "files_to_change": ALLOWED});
        c.env("REPO_PATH", &self.main)
            .env("RECIPE_VAR_repo_path", &self.main)
            .env("RECIPE_VAR_worktree_setup", setup)
            .env("RECIPE_VAR_final_requirements", requirements.to_string())
            .env("RECIPE_VAR_requirements", requirements.to_string())
            .env("RECIPE_VAR_design_spec", design.to_string())
            .env(
                "RECIPE_VAR_ops_modified_files",
                serde_json::to_string(ALLOWED).unwrap(),
            )
            .env("TASK_DESCRIPTION", "Update only the reviewed source files")
            .env("ISSUE_NUMBER", "1554")
            .env("RECIPE_VAR_issue_number", "1554")
            .env("SKIP_PRE_AGENT_VALIDATION", "true")
            .env(
                "RECIPE_VAR_task_description",
                "Update only the reviewed source files",
            )
            .env("RECIPE_VAR_base_branch", "main");
        c
    }

    pub fn commit_file(&self, cwd: &Path, path: &str, bytes: &str) -> String {
        write(cwd.join(path), bytes);
        self.git(cwd, &["--literal-pathspecs", "add", "--", path]);
        self.git(cwd, &["commit", "-qm", "fixture source change"]);
        self.oid(cwd, "HEAD")
    }

    pub fn hook(&self, name: &str, body: &str) {
        executable(self.main.join(".git/hooks").join(name), body);
    }

    pub fn protect_user_blob(&self) -> Vec<u8> {
        write(
            self.work.join("user.txt"),
            b"protected staged bytes\0\xff\n",
        );
        self.git(&self.work, &["add", "--", "user.txt"]);
        write(
            self.work.join("user.txt"),
            b"different unstaged working-tree bytes\n",
        );
        self.git(&self.work, &["show", ":user.txt"])
    }
}
