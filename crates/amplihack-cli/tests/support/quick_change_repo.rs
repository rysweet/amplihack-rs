//! Issue #1740: the temporary repository and tool runner shared by
//! `issue_1740_diff_limits` and `issue_1740_diff_limits_guards`, which test
//! `amplifier-bundle/tools/quick_change_diff_limits.sh`, and by the tests
//! that source `quick_change_common.sh`.
//!
//! Each test builds a temporary repository (with a bare `origin`), pins the
//! base, makes the change an agent would make, and runs the real tool through
//! `bash` started by [`crate::isolated_amplihack::isolated`] with a temp
//! `HOME`. No agent, `gh` or network is involved.
//!
//! Snapshot contract used here (the documented tool usage plus one mode):
//! `quick_change_diff_limits.sh --snapshot <worktree>` prints the git
//! configuration and hooks snapshot on stdout; the measuring run reads it
//! from `$QUICK_CHANGE_STATE_DIR/git-config.snapshot` and exits 2 when that
//! variable is set and the file is missing.

use std::path::{Path, PathBuf};
use std::process::Output;

use tempfile::TempDir;

pub fn tool() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools/quick_change_diff_limits.sh")
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath="])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

pub struct Repo {
    pub tmp: TempDir,
    pub wt: PathBuf,
    pub state: PathBuf,
    pub base: String,
}

impl Repo {
    /// A clone of a bare origin holding a README, a lockfile, a manifest and
    /// a small Rust file; the base is pinned and the snapshot recorded.
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let wt = tmp.path().join("wt");
        let state = tmp.path().join("state");
        std::fs::create_dir(&state).unwrap();
        git(
            tmp.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                wt.to_str().unwrap(),
            ],
        );
        git(&wt, &["checkout", "-q", "-b", "main"]);
        let mut repo = Repo {
            tmp,
            wt,
            state,
            base: String::new(),
        };
        repo.write("README.md", "# demo\n");
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        );
        repo.write("Cargo.lock", &"# lock\n".repeat(10));
        repo.write("src/lib.rs", "fn private() {}\n\npub fn kept() {}\n");
        repo.write(".gitignore", "target/\n");
        git(&repo.wt, &["add", "-A"]);
        git(&repo.wt, &["commit", "-q", "-m", "base"]);
        git(&repo.wt, &["push", "-q", "-u", "origin", "main"]);
        repo.base = git(&repo.wt, &["rev-parse", "HEAD"]);
        let snap = repo.run(&["--snapshot", repo.wt.to_str().unwrap()], true);
        assert!(
            snap.status.success(),
            "--snapshot failed: {}",
            stderr(&snap)
        );
        std::fs::write(repo.state.join("git-config.snapshot"), &snap.stdout).unwrap();
        repo
    }

    pub fn write(&self, rel: &str, body: &str) {
        let p = self.wt.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    pub fn run(&self, args: &[&str], with_state: bool) -> Output {
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.arg(tool()).args(args).env("HOME", self.tmp.path());
        if with_state {
            c.env("QUICK_CHANGE_STATE_DIR", &self.state);
        }
        c.output().expect("run quick_change_diff_limits.sh")
    }

    pub fn measure(&self, extra: &[&str]) -> Output {
        let mut args = vec![self.wt.to_str().unwrap(), "--base", self.base.as_str()];
        args.extend_from_slice(extra);
        self.run(&args, true)
    }

    /// [`Repo::measure`] with `dir` first on `PATH`, so a shim there (an
    /// `awk`, say) replaces the real program for the tool.
    pub fn measure_with_path(&self, dir: &Path) -> Output {
        let path = std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.arg(tool())
            .args([self.wt.to_str().unwrap(), "--base", self.base.as_str()])
            .env("HOME", self.tmp.path())
            .env("QUICK_CHANGE_STATE_DIR", &self.state)
            .env("PATH", path);
        c.output().expect("run quick_change_diff_limits.sh")
    }

    /// Commits everything in the worktree, as the recipe's commit step and
    /// hook repair would.
    pub fn commit_all(&self, message: &str) {
        git(&self.wt, &["add", "-A"]);
        git(&self.wt, &["commit", "-q", "-m", message]);
    }

    /// Runs `script` in bash after sourcing the real `quick_change_common.sh`
    /// with this repository as the run's worktree. `$1` in the script is the
    /// common file; `vars` are extra environment variables.
    pub fn common(&self, script: &str, vars: &[(&str, &str)]) -> Output {
        let common = tool().with_file_name("quick_change_common.sh");
        let wt = std::fs::canonicalize(&self.wt).unwrap();
        let setup = serde_json::json!({ "worktree_path": wt }).to_string();
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.arg("-c")
            .arg(format!(". \"$1\"\n{script}"))
            .arg("bash")
            .arg(common)
            .env("HOME", self.tmp.path())
            .env("RECIPE_VAR_worktree_setup", setup)
            .env("QC_STEP", "test-step");
        for (k, v) in vars {
            c.env(k, v);
        }
        c.output().expect("run quick_change_common.sh")
    }
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Asserts exit code and line 1, and returns the JSON on line 2.
pub fn verdict(o: &Output, code: i32, line1: &str) -> serde_json::Value {
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(code),
        "stdout:\n{out}\nstderr:\n{}",
        stderr(o)
    );
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some(line1), "stdout:\n{out}");
    serde_json::from_str(lines.next().expect("JSON on line 2")).expect("line 2 is JSON")
}

pub fn handover(r: &Repo, reason: &str) {
    verdict(
        &r.measure(&[]),
        0,
        &format!("QUICK_CHANGE_VERDICT=handover reason={reason}"),
    );
}
