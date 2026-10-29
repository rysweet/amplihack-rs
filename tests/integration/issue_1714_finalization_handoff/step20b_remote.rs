//! Fixture for `step20b.rs`: a linked worktree whose branch on a private
//! bare remote has been fast-forwarded (main merged in) from another clone,
//! and a runner for workflow-finalize bash steps exactly as the YAML holds them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use super::fixture::*;
use crate::artifact_guard_workflow::worktree_setup_json;

pub const BR: &str = "feat/sync";

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = amplihack_git::command_in(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub fn commit(dir: &Path, file: &str, body: &str, msg: &str) {
    fs::write(dir.join(file), body).expect("write");
    git(dir, &["add", file]);
    git(dir, &["commit", "-qm", msg]);
}

pub fn finalize_command(id: &str) -> String {
    step(&recipe_yaml("workflow-finalize"), id)["command"]
        .as_str()
        .unwrap_or_else(|| panic!("{id} is a bash step with a command"))
        .to_owned()
}

#[derive(Clone, Copy, Debug)]
pub enum Tracking {
    /// `git push -u`: upstream set, origin/<branch> at the local HEAD.
    Upstream,
    /// Pushed without `-u`: origin/<branch> exists, no upstream.
    RefOnly,
    /// Pushed from elsewhere: neither an upstream nor origin/<branch>.
    Nothing,
}

pub struct Remote {
    _tmp: TempDir,
    pub root: PathBuf,
    pub wt: PathBuf,
    pub local: String,
    pub pr_head: String,
}

impl Remote {
    /// main and <branch> (L) pushed; then, from another clone, main moves
    /// on and is merged into <branch> (M, a fast-forward of L) and pushed.
    /// The run's worktree is at L and has never seen M.
    pub fn new(tracking: Tracking) -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = fs::canonicalize(tmp.path()).expect("canonicalize");
        let bare = root.join("origin.git");
        git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);

        let launch = root.join("launch");
        fs::create_dir_all(&launch).expect("mkdir");
        git(&launch, &["init", "-q", "-b", "main"]);
        fs::write(launch.join(".gitignore"), "worktrees/\n.claude/runtime/\n").expect("write");
        commit(&launch, "README.md", "base\n", "base");
        git(&launch, &["add", ".gitignore"]);
        git(&launch, &["commit", "-qm", "ignore"]);
        git(
            &launch,
            &["remote", "add", "origin", &bare.to_string_lossy()],
        );
        git(&launch, &["push", "-q", "-u", "origin", "main"]);

        let wt = launch.join("worktrees/sync");
        git(
            &launch,
            &["worktree", "add", "-q", "-b", BR, &wt.to_string_lossy()],
        );
        commit(&wt, "work.txt", "run work\n", "run work");
        let local = git(&wt, &["rev-parse", "HEAD"]);
        match tracking {
            Tracking::Upstream => git(&wt, &["push", "-q", "-u", "origin", BR]),
            Tracking::RefOnly => git(&wt, &["push", "-q", "origin", BR]),
            Tracking::Nothing => {
                git(&wt, &["push", "-q", "origin", BR]);
                git(
                    &wt,
                    &["update-ref", "-d", &format!("refs/remotes/origin/{BR}")],
                )
            }
        };

        let other = root.join("other");
        git(
            &root,
            &[
                "clone",
                "-q",
                &bare.to_string_lossy(),
                &other.to_string_lossy(),
            ],
        );
        commit(&other, "main.txt", "main moved on\n", "main moves on");
        git(&other, &["push", "-q", "origin", "main"]);
        git(&other, &["checkout", "-q", BR]);
        git(
            &other,
            &["merge", "-q", "--no-ff", "-m", "merge main", "main"],
        );
        git(&other, &["push", "-q", "origin", BR]);
        let pr_head = git(&other, &["rev-parse", "HEAD"]);

        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("mkdir bin");
        fs::create_dir_all(root.join("home")).expect("mkdir home");
        let stub = bin.join("amplihack");
        fs::write(&stub, "#!/usr/bin/env bash\nexit 0\n").expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        assert_ne!(local, pr_head);
        Self {
            _tmp: tmp,
            root,
            wt,
            local,
            pr_head,
        }
    }

    /// Run one workflow-finalize step from its YAML, as the runner would.
    pub fn run(&self, id: &str) -> std::process::Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        );
        Command::new("bash")
            .arg("-c")
            .arg(finalize_command(id))
            .current_dir(self.root.join("launch"))
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env("AMPLIHACK_GIT_AUTHOR_NAME", "workflow")
            .env("AMPLIHACK_GIT_AUTHOR_EMAIL", "workflow@example.com")
            .env("AMPLIHACK_GIT_COMMITTER_NAME", "workflow")
            .env("AMPLIHACK_GIT_COMMITTER_EMAIL", "workflow@example.com")
            .env("REPO_PATH", self.root.join("launch"))
            .env("AMPLIHACK_HOME", workspace_root())
            .env("RECIPE_VAR_worktree_setup", worktree_setup_json(&self.wt))
            .output()
            .expect("spawn bash")
    }

    pub fn head(&self) -> String {
        git(&self.wt, &["rev-parse", "HEAD"])
    }

    pub fn remote_head(&self) -> String {
        git(
            &self.root.join("origin.git"),
            &["rev-parse", &format!("refs/heads/{BR}")],
        )
    }

    pub fn upstream(&self) -> String {
        git(&self.wt, &["rev-parse", "--abbrev-ref", "@{u}"])
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        amplihack_git::command_in(&self.wt)
            .args(["merge-base", "--is-ancestor", a, b])
            .status()
            .expect("merge-base")
            .success()
    }

    /// Stand-in for step-20 / step-20c: an edit left uncommitted.
    pub fn agent_edit(&self) {
        fs::write(self.wt.join("audit.txt"), "audit fix\n").expect("write");
    }
}

pub fn describe(id: &str, out: &std::process::Output) -> String {
    format!(
        "{id}: exit {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn assert_ok(id: &str, out: &std::process::Output) {
    assert!(out.status.success(), "{}", describe(id, out));
}
