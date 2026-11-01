//! Shared fixture for the #1762 tests.
//!
//! Each [`Fx`] owns a temp directory holding:
//! - `remotes/acme/widget.git`: the bare origin. The caller's `origin` is
//!   `https://github.com/acme/widget.git`, and every child gets
//!   `url.<tmp>/remotes/.insteadOf=https://github.com/`, so fetches and pushes
//!   stay on disk while the provider helpers still see a github.com origin;
//! - `caller/`: the caller's clone, on `main`, with `origin/HEAD` set;
//! - `bin/gh`: a fake `gh` that logs every call to `gh-calls.log`, answers
//!   `pr list` only for the head named in `list-head`, `pr view` from
//!   `view.json`, and exits 99 on any other call, `pr create` included;
//! - `home/`, `scratch/`, `runtime/`: HOME, TMPDIR and XDG_RUNTIME_DIR.
//!
//! Every child, `git` and `bash` alike, starts from `isolated(..)`, so it gets
//! a cleared environment with no credentials, no
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT` and no real HOME, and then only what
//! [`Fx::env`] adds. `AMPLIHACK_HOME` is this worktree, so the bundle tools are
//! read from the source tree and never written. `GIT_ALLOW_PROTOCOL=file` and
//! `GIT_TERMINAL_PROMPT=0` keep git off the network and away from prompts, and
//! stdin is `/dev/null`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use serde_yaml::Value as Yaml;
use tempfile::TempDir;

use crate::isolated_amplihack::isolated;

/// The target branch. It has a slash, so its aliases live under
/// `worktrees/feat/`, and no regular-expression metacharacters (D19).
pub const BRANCH: &str = "feat/issue-1762-fixture";
pub const ORIGIN_URL: &str = "https://github.com/acme/widget.git";
pub const PR_URL: &str = "https://github.com/acme/widget/pull/7";
pub const PLANNER: &str = "workflow_caller_branch_alias.sh";
pub const RESOLVER: &str = "workflow_remote_branch.sh";
pub const REUSE: &str = "workflow_alias_reuse.sh";

/// `<BRANCH>--wt-<n>`.
pub fn alias(n: u32) -> String {
    format!("{BRANCH}--wt-{n}")
}

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

pub fn tool_path(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn recipe_path(name: &str) -> PathBuf {
    workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml"))
}

/// The `field` (`command` or `prompt`) of step `id` in recipe `name`.
pub fn step_text(name: &str, id: &str, field: &str) -> String {
    let recipe: Yaml = serde_yaml::from_str(&read(&recipe_path(name)))
        .unwrap_or_else(|e| panic!("parse {name}.yaml: {e}"));
    recipe["steps"]
        .as_sequence()
        .expect("recipe must declare steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{name}.yaml has no step `{id}`"))[field]
        .as_str()
        .unwrap_or_else(|| panic!("{name}.yaml step `{id}` has no `{field}`"))
        .to_owned()
}

/// Lines of `text` that are not shell comments.
pub fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim_start().starts_with('#'))
}

pub fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    }
}

/// True when `call` passes `branch` as its `--head` (`--head B` or `--head=B`).
pub fn names_head(call: &str, branch: &str) -> bool {
    let words: Vec<&str> = call.split_whitespace().collect();
    words.windows(2).any(|w| w[0] == "--head" && w[1] == branch)
        || words.iter().any(|w| *w == format!("--head={branch}"))
}

/// One finished child.
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn describe(&self) -> String {
        format!(
            "exit={:?}\nstdout:\n{}\nstderr:\n{}",
            self.code, self.stdout, self.stderr
        )
    }

    /// The last stdout line that is a JSON object.
    pub fn json(&self) -> Value {
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .unwrap_or_else(|| panic!("no JSON line on stdout\n{}", self.describe()));
        serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("bad JSON {line:?}: {e}\n{}", self.describe()))
    }

    /// The first stderr line that contains every one of `parts`.
    pub fn stderr_line_with(&self, parts: &[&str]) -> Option<&str> {
        self.stderr
            .lines()
            .find(|l| parts.iter().all(|p| l.contains(p)))
    }
}

pub fn run(command: &mut Command) -> Run {
    let output = command.output().expect("spawn child");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

const GH_STUB: &str = r#"#!/usr/bin/env bash
# Fake gh for the #1762 tests: logs every call and answers only the reads the
# tests need. Anything else, `pr create` included, exits 99.
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/gh-calls.log"
head="" prev=""
for a in "$@"; do
  case "$a" in --head=*) head="${a#--head=}" ;; esac
  [ "$prev" != "--head" ] || head="$a"
  prev="$a"
done
case "${1:-} ${2:-}" in
  "pr list")
    if [ -f "$GH_FIXTURE_DIR/list.json" ] && [ -n "$head" ] \
        && [ "$head" = "$(cat "$GH_FIXTURE_DIR/list-head" 2>/dev/null)" ]; then
      cat "$GH_FIXTURE_DIR/list.json"
    else
      echo '[]'
    fi ;;
  "pr view")
    cat "$GH_FIXTURE_DIR/view.json" 2>/dev/null || { echo "fake gh: no canned pr view" >&2; exit 1; } ;;
  "auth status") exit 0 ;;
  *) echo "fake gh: unsupported call: $*" >&2; exit 99 ;;
esac
"#;

/// The parts of a checkout that #1762 must never change (R3, AC2).
#[derive(Debug, PartialEq, Eq)]
pub struct CallerState {
    head: String,
    head_sha: String,
    /// `refs/heads/*` with their commits, alias-shaped names left out.
    branches: String,
    stash: String,
    tracked_status: String,
    /// `branch.<BRANCH>.*`, matched as `^branch\.<BRANCH>\.` (D19).
    branch_config: String,
    index: Vec<u8>,
}

pub struct Fx {
    /// Keeps the temp directory alive for the fixture's lifetime.
    pub _tmp: TempDir,
    /// The temp directory as a physical path.
    pub root: PathBuf,
    /// The caller's clone, as a physical path.
    pub caller: PathBuf,
}

impl Fx {
    pub fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir under $TMPDIR");
        for dir in ["home", "scratch", "bin", "runtime", "remotes/acme"] {
            fs::create_dir_all(tmp.path().join(dir)).expect("fixture dir");
        }
        write_exec(&tmp.path().join("bin/gh"), GH_STUB);
        let root = fs::canonicalize(tmp.path()).expect("canonical tempdir");
        let fx = Self {
            caller: root.join("caller"),
            root,
            _tmp: tmp,
        };
        fx.git(
            &fx.root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                "remotes/acme/widget.git",
            ],
        );
        fx.git(&fx.root, &["init", "-q", "-b", "main", "caller"]);
        for (key, value) in [
            ("user.email", "fixture@example.com"),
            ("user.name", "Fixture"),
            ("commit.gpgsign", "false"),
        ] {
            fx.git(&fx.caller, &["config", key, value]);
        }
        fx.git(&fx.caller, &["remote", "add", "origin", ORIGIN_URL]);
        fx.write_in(&fx.caller, "README.md", "base\n");
        fx.git(&fx.caller, &["add", "README.md"]);
        fx.git(&fx.caller, &["commit", "-q", "-m", "base"]);
        fx.git(&fx.caller, &["push", "-q", "-u", "origin", "main"]);
        fx.git(&fx.caller, &["remote", "set-head", "origin", "main"]);
        fx
    }

    /// The environment every child gets on top of `isolated(..)`.
    pub fn env(&self, command: &mut Command) {
        let root = &self.root;
        command
            .env("HOME", root.join("home"))
            .env("TMPDIR", root.join("scratch"))
            .env("XDG_CONFIG_HOME", root.join("home/.config"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("GH_CONFIG_DIR", root.join("home/.config/gh"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.{}/.insteadOf", root.join("remotes").display()),
            )
            .env("GIT_CONFIG_VALUE_0", "https://github.com/")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("GH_FIXTURE_DIR", root)
            .env("AMPLIHACK_HOME", workspace_root())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    root.join("bin").display(),
                    crate::amplihack_path::path_with_amplihack_bin()
                ),
            )
            .stdin(Stdio::null());
    }

    /// `bash` from `isolated(..)` with [`Fx::env`], running in `cwd`.
    pub fn bash(&self, cwd: &Path) -> Command {
        let mut command = isolated("bash");
        self.env(&mut command);
        command.current_dir(cwd);
        command
    }

    pub fn git_output(&self, dir: &Path, args: &[&str]) -> Run {
        let mut command = isolated("git");
        self.env(&mut command);
        command.current_dir(dir).args(args);
        run(&mut command)
    }

    /// Run fixture git in `dir`; panics on failure. Returns stdout.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.git_output(dir, args);
        assert_eq!(
            out.code,
            Some(0),
            "fixture git {args:?} in {} failed\n{}",
            dir.display(),
            out.describe()
        );
        out.stdout
    }

    pub fn write_in(&self, dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir");
        }
        fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    pub fn commit_in(&self, dir: &Path, rel: &str, body: &str, message: &str) {
        self.write_in(dir, rel, body);
        self.git(dir, &["add", rel]);
        self.git(dir, &["commit", "-q", "-m", message]);
    }

    /// An unrelated repository at `<root>/<name>`, on `branch` with one
    /// commit and no origin: the target of an inherited GIT_DIR.
    pub fn other_repo(&self, name: &str, branch: &str) -> PathBuf {
        let dir = self.root.join(name);
        self.git(&self.root, &["init", "-q", "-b", branch, name]);
        for (key, value) in [
            ("user.email", "other@example.com"),
            ("user.name", "Other"),
            ("commit.gpgsign", "false"),
        ] {
            self.git(&dir, &["config", key, value]);
        }
        self.commit_in(&dir, "o.txt", "o\n", "other");
        dir
    }

    pub fn head(&self, dir: &Path) -> String {
        self.git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// Put the caller on BRANCH with one commit of its own, pushed with `-u`:
    /// the clean, pushed state the issue's first test case starts from.
    pub fn on_pushed_branch(&self) {
        self.git(&self.caller, &["switch", "-q", "-c", BRANCH]);
        self.commit_in(&self.caller, "feature.txt", "feature\n", "feature work");
        self.git(&self.caller, &["push", "-q", "-u", "origin", BRANCH]);
    }

    /// Create `alias` at `<caller>/worktrees/<alias>`, tracking
    /// `origin/<upstream>`, as step 4's create pass does. Returns its path.
    pub fn alias_worktree(&self, alias: &str, upstream: &str) -> PathBuf {
        let path = self.caller.join("worktrees").join(alias);
        self.git(
            &self.caller,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                alias,
                "--",
                path.to_str().expect("utf-8 path"),
                &format!("origin/{upstream}"),
            ],
        );
        path
    }

    pub fn config_get(&self, key: &str) -> String {
        self.git_output(&self.caller, &["config", "--get", key])
            .stdout
            .trim()
            .to_owned()
    }

    pub fn local_branch_exists(&self, name: &str) -> bool {
        self.git_output(
            &self.caller,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .code
            == Some(0)
    }

    /// Local branches whose name has an alias suffix.
    pub fn alias_branches(&self) -> Vec<String> {
        self.git(
            &self.caller,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
        )
        .lines()
        .filter(|l| l.contains("--wt-"))
        .map(str::to_owned)
        .collect()
    }

    pub fn origin_bare(&self) -> PathBuf {
        self.root.join("remotes/acme/widget.git")
    }

    /// Every ref on origin, as `<refname> <commit>` lines.
    pub fn origin_refs(&self) -> Vec<String> {
        self.git(
            &self.origin_bare(),
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    pub fn origin_sha(&self, branch: &str) -> Option<String> {
        let prefix = format!("refs/heads/{branch} ");
        self.origin_refs()
            .iter()
            .find_map(|l| l.strip_prefix(&prefix).map(str::to_owned))
    }

    pub fn assert_no_alias_on_origin(&self, why: &str) {
        let pushed: Vec<String> = self
            .origin_refs()
            .into_iter()
            .filter(|l| l.starts_with("refs/heads/") && l.contains("--wt-"))
            .collect();
        assert!(
            pushed.is_empty(),
            "{why}: an alias name must never become a branch on origin, got {pushed:?}"
        );
    }

    /// What #1762 must leave alone in the checkout at `dir`.
    pub fn caller_state(&self, dir: &Path) -> CallerState {
        let git = |args: &[&str]| self.git_output(dir, args).stdout;
        let branches = git(&[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
        ])
        .lines()
        .filter(|l| !l.contains("--wt-"))
        .collect::<Vec<_>>()
        .join("\n");
        let index_path = git(&["rev-parse", "--path-format=absolute", "--git-path", "index"]);
        CallerState {
            head: git(&["symbolic-ref", "-q", "HEAD"]),
            head_sha: git(&["rev-parse", "HEAD"]),
            branches,
            stash: git(&["stash", "list"]),
            tracked_status: git(&[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=no",
            ]),
            branch_config: git(&["config", "--get-regexp", &format!("^branch\\.{BRANCH}\\.")]),
            index: fs::read(index_path.trim()).unwrap_or_default(),
        }
    }

    /// Run workflow-worktree step 4 as default-workflow does, targeting
    /// `branch` with `existing_branch`. `extra` overrides any variable.
    pub fn step4(&self, branch: &str, extra: &[(&str, &str)]) -> Run {
        let mut command = self.bash(&self.caller);
        command
            .arg("-c")
            .arg(step_text(
                "workflow-worktree",
                "step-04-setup-worktree",
                "command",
            ))
            .env("REPO_PATH", &self.caller)
            .env("EXISTING_BRANCH", branch)
            .env("PR_NUMBER", "")
            .env("BRANCH_PREFIX", "feat")
            .env("ISSUE_NUMBER", "1762")
            .env(
                "TASK_DESCRIPTION",
                "continue the fixture branch for issue 1762",
            );
        for (key, value) in extra {
            command.env(key, value);
        }
        run(&mut command)
    }

    /// Run a bundle tool from the source tree with `args`, in `cwd`.
    pub fn tool(&self, name: &str, cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut command = self.bash(cwd);
        command.arg(tool_path(name)).args(args);
        for (key, value) in envs {
            command.env(key, value);
        }
        run(&mut command)
    }

    /// An `AMPLIHACK_HOME` holding a copy of this worktree's top-level tools;
    /// returns the home and its tools directory.
    fn bundle_copy(&self) -> (PathBuf, PathBuf) {
        let home = self.root.join("stub-bundle");
        let tools = home.join("amplifier-bundle/tools");
        fs::create_dir_all(&tools).expect("stub tools dir");
        let source = workspace_root().join("amplifier-bundle/tools");
        for entry in fs::read_dir(source).expect("read tools").flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "sh") {
                fs::copy(path, tools.join(entry.file_name())).expect("copy tool");
            }
        }
        (home, tools)
    }

    /// An `AMPLIHACK_HOME` whose tools are this worktree's, except that the
    /// planner is replaced by `planner_body`.
    pub fn bundle_with_planner(&self, planner_body: &str) -> PathBuf {
        let (home, tools) = self.bundle_copy();
        write_exec(&tools.join(PLANNER), planner_body);
        home
    }

    /// An `AMPLIHACK_HOME` whose tools are this worktree's, without `name`.
    pub fn bundle_without(&self, name: &str) -> PathBuf {
        let (home, tools) = self.bundle_copy();
        fs::remove_file(tools.join(name)).expect("remove tool from stub bundle");
        home
    }

    /// Canned answers for an open, green, same-repo PR #7 on BRANCH whose head
    /// is `head_sha`. `pr list` answers it only for `--head BRANCH`.
    pub fn answer_open_pr(&self, head_sha: &str) {
        let list = json!([{
            "number": 7, "url": PR_URL, "isCrossRepository": false, "headRefOid": head_sha
        }]);
        let view = json!({
            "number": 7, "url": PR_URL, "state": "OPEN", "mergeable": "MERGEABLE",
            "reviews": [], "headRefName": BRANCH, "headRefOid": head_sha,
            "isCrossRepository": false, "baseRefName": "main", "mergedAt": null,
            "statusCheckRollup": [{
                "__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS"
            }]
        });
        fs::write(self.root.join("list.json"), list.to_string()).expect("list.json");
        fs::write(self.root.join("list-head"), BRANCH).expect("list-head");
        fs::write(self.root.join("view.json"), view.to_string()).expect("view.json");
    }

    fn log_lines(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.root.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn gh_calls(&self) -> Vec<String> {
        self.log_lines("gh-calls.log")
    }

    pub fn scope_calls(&self) -> Vec<String> {
        self.log_lines("scope-calls.log")
    }

    pub fn assert_gh_never_names_alias(&self, run: &Run, why: &str) {
        let calls = self.gh_calls();
        assert!(
            calls.iter().all(|c| !c.contains("--wt-")),
            "{why}: no gh call may name the alias, got {calls:?}\n{}",
            run.describe()
        );
    }
}
