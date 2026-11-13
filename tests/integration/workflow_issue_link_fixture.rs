//! Private, synchronous fixtures for #2550; no credentials, host profiles or network.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

pub const BRANCH: &str = "feat/issue-2550-link-mode";
pub const ISSUE: &str = "2550";
pub const PR: &str = "https://github.com/owner/repo/pull/99";
pub const MARKER: &str = "<!-- merge-ready-report: v1 -->";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn recipe(name: &str) -> serde_yaml::Value {
    serde_yaml::from_str(
        &fs::read_to_string(root().join(format!("amplifier-bundle/recipes/{name}.yaml"))).unwrap(),
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

pub fn command(name: &str, id: &str) -> String {
    step(name, id)["command"].as_str().unwrap().to_owned()
}

pub fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn describe(out: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub struct Fixture {
    pub temp: TempDir,
    pub repo: PathBuf,
    pub bin: PathBuf,
    pub bundle: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        for dir in [
            "home/.claude",
            "home/.amplihack",
            "home/.codex",
            "profiles",
            "config",
            "cache",
            "private",
            "bin",
            "repo",
            "bundle/amplifier-bundle/tools",
        ] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        let f = Self {
            repo: base.join("worktree"),
            bin: base.join("bin"),
            bundle: base.join("bundle"),
            temp,
        };
        // The existing hook-repair helper needs modern Bash. macOS ships 3.2;
        // use the installed Homebrew shell without changing production scripts.
        let modern_bash = ["/opt/homebrew/bin/bash", "/usr/local/bin/bash"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .unwrap_or_else(|| f.lookup("bash"));
        std::os::unix::fs::symlink(modern_bash, f.bin.join("bash")).unwrap();
        let proc_root = base.join("private/fake-proc");
        fs::create_dir(&proc_root).unwrap();
        crate::isolated_amplihack::seed_proc_tree(&proc_root);
        // Resolve tools before installing shims, using the same isolated environment.
        let real_git = f.lookup("git");
        f.write("real-git", real_git.to_str().unwrap());
        executable(&f.bin.join("git"), GIT);
        executable(&f.bin.join("gh"), GH);
        // Production helpers may wrap service commands in timeout. This fixture
        // has synchronous fake services: pass through without starting a timer.
        executable(&f.bin.join("timeout"), "#!/bin/sh\nshift\nexec \"$@\"\n");
        for tool in ["sleep", "curl", "wget", "ssh"] {
            executable(
                &f.bin.join(tool),
                "#!/bin/sh\necho forbidden-operation >&2\nexit 97\n",
            );
        }
        // Use the Cargo-built CLI for policy and composition, never an installed CLI.
        executable(&f.bin.join("amplihack"), AMPLIHACK);
        for item in fs::read_dir(root().join("amplifier-bundle/tools")).unwrap() {
            let item = item.unwrap();
            if item.file_type().unwrap().is_file() {
                fs::copy(
                    item.path(),
                    f.bundle
                        .join("amplifier-bundle/tools")
                        .join(item.file_name()),
                )
                .unwrap();
            }
        }
        let main = base.join("repo");
        let origin = base.join("origin.git");
        f.git_at(&main, &["init", "-b", "main"]);
        f.git_at(&main, &["config", "user.name", "Issue Link Test"]);
        f.git_at(&main, &["config", "user.email", "tests@example.com"]);
        f.git_at(&main, &["config", "protocol.allow", "never"]);
        f.git_at(&main, &["config", "protocol.file.allow", "always"]);
        let hooks = base.join("hooks");
        fs::create_dir(&hooks).unwrap();
        for hook in ["pre-commit", "commit-msg", "pre-push"] {
            executable(
                &hooks.join(hook),
                &format!("#!/bin/sh\nprintf '%s\\n' '{hook}' >> \"$CASE_ROOT/hooks.log\"\n"),
            );
        }
        f.git_at(
            &main,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        fs::write(main.join("README.md"), "Fixture base\n").unwrap();
        f.git_at(&main, &["add", "."]);
        f.git_at(&main, &["commit", "-m", "fixture base"]);
        f.git_at(
            &main,
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        // Keep the observable remote GitHub-shaped; the shim routes transfers
        // exclusively to the private bare repository, without bypassing hooks.
        f.git_at(
            &main,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        f.git_at(&main, &["push", "-u", "origin", "main"]);
        f.git_at(&main, &["remote", "set-head", "origin", "main"]);
        f.git_at(
            &main,
            &["worktree", "add", "-b", BRANCH, f.repo.to_str().unwrap()],
        );
        f.git(&["push", "-u", "origin", BRANCH]);
        f.write("hooks.log", "");
        f.write("pr.json", "null");
        f.write(
            "issue.json",
            r#"{"title":"Honour the caller's issue linkage"}"#,
        );
        f
    }

    pub fn path(&self) -> PathBuf {
        self.temp.path().canonicalize().unwrap()
    }

    pub fn write(&self, name: &str, contents: &str) {
        let path = self.path().join(name);
        fs::write(&path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    pub fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path().join(name)).unwrap_or_default()
    }

    pub fn child(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let base = self.path();
        let mut cmd = crate::isolated_amplihack::isolated(program);
        amplihack_git::scrub(&mut cmd);
        cmd.env("HOME", base.join("home"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            .env("TMPDIR", &base)
            .env("AMPLIHACK_TEST_PROC_ROOT", base.join("private/fake-proc"))
            .env("GH_RETRY_TMPDIR", base.join("private"))
            .env("CLAUDE_CONFIG_DIR", base.join("home/.claude"))
            .env("CODEX_HOME", base.join("home/.codex"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", base.join("profiles"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Issue Link Test")
            .env("GIT_AUTHOR_EMAIL", "tests@example.com")
            .env("GIT_COMMITTER_NAME", "Issue Link Test")
            .env("GIT_COMMITTER_EMAIL", "tests@example.com")
            .env("CASE_ROOT", &base)
            .env("REAL_AMPLIHACK", env!("CARGO_BIN_EXE_amplihack"))
            .env("AMPLIHACK_HOME", &self.bundle)
            .env("REPO_PATH", base.join("repo"))
            .env("RECIPE_VAR_repo_path", base.join("repo"))
            .env(
                "RECIPE_VAR_worktree_setup",
                self.worktree_context().to_string(),
            )
            .env("RECIPE_VAR_task_description", "Honour caller issue linkage")
            .env("TASK_DESCRIPTION", "Honour caller issue linkage")
            .env("ISSUE_NUMBER", ISSUE)
            .env("REMOTE_HOST_TYPE", "github")
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("RECIPE_VAR_skip_pre_agent_validation", "true")
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .current_dir(if self.repo.is_dir() {
                self.repo.clone()
            } else {
                base
            });
        cmd
    }

    pub fn worktree_context(&self) -> serde_json::Value {
        serde_json::json!({"worktree_path": self.repo, "branch_name": BRANCH})
    }

    fn lookup(&self, tool: &str) -> PathBuf {
        let out = self
            .child("bash")
            .args(["-c", "command -v \"$1\"", "lookup", tool])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "missing prerequisite {tool}: {}",
            describe(&out)
        );
        PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
    }

    pub fn git_at(&self, cwd: &Path, args: &[&str]) -> String {
        let mut cmd = amplihack_git::command();
        // Reuse the fixture's cleared environment even for Rust-started Git.
        let environment = self.child("bash");
        cmd.env_clear();
        for (key, value) in environment.get_envs() {
            if let Some(value) = value {
                cmd.env(key, value);
            }
        }
        let out = cmd.current_dir(cwd).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", describe(&out));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.git_at(&self.repo, args)
    }

    pub fn change(&self, path: &str) {
        let file = self.repo.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "fixture change\n").unwrap();
    }

    pub fn commit_change(&self, path: &str) {
        self.change(path);
        self.git(&["add", "."]);
        self.git(&[
            "commit",
            "-m",
            "fixture substantive change\n\nTask: unrelated-trailer\nFixes #17",
        ]);
    }

    pub fn bash(&self, script: &str) -> Command {
        let mut cmd = self.child("bash");
        cmd.args(["-c", script]);
        cmd
    }

    pub fn publish(&self, mode: Option<&str>) -> Output {
        let mut cmd = self.bash(&command("workflow-publish", "step-16-create-draft-pr"));
        if let Some(mode) = mode {
            cmd.env("RECIPE_VAR_pr_issue_link", mode);
        }
        cmd.output().unwrap()
    }

    pub fn assert_published(&self, out: &Output, reference: &str) -> String {
        assert!(
            out.status.success(),
            "publication failed: {}",
            describe(out)
        );
        assert!(
            self.read("operations")
                .lines()
                .any(|s| s.starts_with("pr create ")),
            "must create a PR: {}",
            describe(out)
        );
        let body = self.read("body.md");
        assert_eq!(
            body.lines().filter(|l| *l == reference).count(),
            1,
            "{body}"
        );
        body
    }

    pub fn stdin(&self, args: &[&str], input: &str) -> Output {
        let mut cmd = self.child(env!("CARGO_BIN_EXE_amplihack"));
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

const GIT: &str = r#"#!/bin/bash
set -eu
real=$(cat "$CASE_ROOT/real-git")
case "${1:-}" in
  push|fetch|ls-remote)
    action=$1; shift
    # Pushing by path loses upstream inference, so keep the configured remote
    # and use a command-local URL rewrite instead.
    exec "$real" -c "url.$CASE_ROOT/origin.git.insteadOf=https://github.com/owner/repo.git" "$action" "$@" ;;
esac
exec "$real" "$@"
"#;

const AMPLIHACK: &str = r#"#!/bin/bash
set -eu
if [ "${1:-} ${2:-}" = 'workflow issue-keyword' ] && [ -f "$CASE_ROOT/selector-fails" ]; then
  printf 'Closes\n'; exit 71
fi
if [ "${1:-} ${2:-}" = 'workflow issue-keyword' ] && [ -f "$CASE_ROOT/selector-output" ]; then
  cat "$CASE_ROOT/selector-output"; exit 0
fi
exec "$REAL_AMPLIHACK" "$@"
"#;

const GH: &str = r#"#!/bin/bash
set -eu
printf '%s\n' "$*" >> "$CASE_ROOT/operations"
case "${1:-} ${2:-}" in
  'auth status') exit 0 ;;
  'repo view') printf 'main\n' ;;
  'issue view') cat "$CASE_ROOT/issue.json" ;;
  'pr list') jq 'if type == "object" then [.] else [] end' "$CASE_ROOT/pr.json" ;;
  'pr view')
    if [ -f "$CASE_ROOT/fetch-fails" ]; then echo 'read denied' >&2; exit 41; fi
    query=''
    while [ "$#" -gt 0 ]; do
      case "$1" in --jq|-q) query=$2; shift ;; esac
      shift
    done
    if [ -n "$query" ]; then jq -r "$query" "$CASE_ROOT/pr.json"; else cat "$CASE_ROOT/pr.json"; fi ;;
  'pr create'|'pr edit')
    action=$2; shift 2
    body=''; title='Honour caller issue linkage'
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --body-file) body=$(cat "$2"; printf x); body=${body%x}; shift ;;
        --body) echo 'body must use a file or stdin' >&2; exit 96 ;;
        --title) title=$2; shift ;;
      esac
      shift
    done
    [ -n "$body" ] || { echo 'missing body' >&2; exit 96; }
    printf '%s' "$body" > "$CASE_ROOT/body.md"
    if [ "$action" = create ]; then
      sha=$(git rev-parse HEAD)
      jq -n --arg body "$body" --arg title "$title" --arg sha "$sha" '{number:99,url:"https://github.com/owner/repo/pull/99",title:$title,body:$body,state:"OPEN",isDraft:true,mergedAt:null,createdAt:"2026-11-13T00:00:00Z",headRefName:"feat/issue-2550-link-mode",baseRefName:"main",headRefOid:$sha,headRepositoryOwner:{login:"owner"},headRepository:{name:"repo"},isCrossRepository:false,closingIssuesReferences:[]}' > "$CASE_ROOT/pr.json"
      printf 'https://github.com/owner/repo/pull/99\n'
    else
      jq --arg body "$body" '.body=$body' "$CASE_ROOT/pr.json" > "$CASE_ROOT/edited.json"
      mv "$CASE_ROOT/edited.json" "$CASE_ROOT/pr.json"
    fi ;;
  'pr ready')
    jq '.isDraft=false' "$CASE_ROOT/pr.json" > "$CASE_ROOT/ready.json"
    mv "$CASE_ROOT/ready.json" "$CASE_ROOT/pr.json" ;;
  'pr comment') exit 0 ;;
  *) echo "unexpected fake service operation: $*" >&2; exit 96 ;;
esac
"#;
