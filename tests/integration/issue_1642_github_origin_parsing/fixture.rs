//! Shared fixture for the issue #1642 origin-parsing tests.
//!
//! Each test gets a private temp directory (under `$TMPDIR`, which tempfile
//! honours) holding:
//! - `repo/`: a real git repo with `main` and `feat/x` (one commit ahead),
//!   `refs/remotes/origin/{main,HEAD}`, and `remote.origin.url` set to the URL
//!   under test through argv, never through a shell-interpolated literal;
//! - `bin/gh`: a stub that appends its argv to `gh-calls.log`, answers
//!   `auth status` with 0 and `pr list` with `[]`, and fails everything else;
//! - `home/`, `scratch/`: private HOME and TMPDIR for the child.
//!
//! Children run with `env_clear()`, so GH_HOST, GH_REPO (which would bypass
//! `--repo`), GH_TOKEN and every WORKFLOW_*_HELPER override from an enclosing
//! run are absent: the helper is found through SCRIPT_DIR / BASH_SOURCE.
//! GIT_ALLOW_PROTOCOL=file keeps git off the network even for a hostile origin.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

pub const BRANCH: &str = "feat/x";

/// The four URLs from the issue plus the first-vs-last `@github.com/` case.
/// git contacts evil.example (or github.com as user `a`) for each of them, so
/// none may be read as a github.com repository.
pub const HOSTILE_ORIGINS: &[&str] = &[
    "https://evil.example/x@github.com/a/b",
    "https://evil.example#@github.com/o/r",
    "https://evil.example?@github.com/o/r",
    r"https://evil.example\@github.com/o/r",
    "https://a@github.com/x@github.com/o/r",
    // Carries the dummy token, so P3 is checked on a rejected origin too.
    r"https://x:tok@evil.example\@github.com/o/r",
];

/// Real origin forms; every one must keep parsing to `o/r`.
pub const REAL_ORIGINS: &[&str] = &[
    "https://github.com/o/r",
    "https://github.com/o/r.git",
    "http://github.com/o/r",
    "git@github.com:o/r",
    "git@github.com:o/r.git",
    "ssh://git@github.com/o/r",
    "https://tok@github.com/o/r",
    "https://x:tok@github.com/o/r",
    "http://x:tok@github.com/o/r",
];

const GH_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_STUB_LOG"
case "${1:-} ${2:-}" in
  "auth status") exit 0 ;;
  "pr list") printf '[]\n'; exit 0 ;;
esac
echo "gh stub: unsupported call" >&2
exit 1
"#;

pub fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

pub fn tools_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools")
}

pub fn tool_path(name: &str) -> PathBuf {
    tools_dir().join(name)
}

pub fn helper_path() -> PathBuf {
    tool_path("workflow_github_repo.sh")
}

pub fn write_executable(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    fs::write(path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
    }
}

/// Exit status and both streams of one child run.
pub struct Outcome {
    pub output: Output,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn from(output: Output) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output,
        }
    }

    pub fn success(&self) -> bool {
        self.output.status.success()
    }

    /// The last stdout line that parses as a JSON object, if any.
    pub fn json(&self) -> Option<serde_json::Value> {
        self.stdout
            .lines()
            .rev()
            .filter(|line| line.trim_start().starts_with('{'))
            .find_map(|line| serde_json::from_str(line).ok())
    }

    /// P3: the first dummy-token fragment either stream contains, if any.
    pub fn leaked_token(&self) -> Option<&'static str> {
        [":tok@", "tok@github", "tok@evil", "ghp_FAKE1642"]
            .into_iter()
            .find(|leak| self.stdout.contains(leak) || self.stderr.contains(leak))
    }

    pub fn describe(&self) -> String {
        format!(
            "exit={:?}\nstdout:\n{}\nstderr:\n{}",
            self.output.status.code(),
            self.stdout,
            self.stderr
        )
    }
}

pub struct Fixture {
    pub tmp: TempDir,
    pub repo: PathBuf,
}

impl Fixture {
    /// A repo on `feat/x` whose `remote.origin.url` is exactly `origin`.
    pub fn with_origin(origin: &str) -> Self {
        let tmp = TempDir::new().expect("tempdir under $TMPDIR");
        let repo = tmp.path().join("repo");
        for dir in ["repo", "home/.config/gh", "scratch", "bin"] {
            fs::create_dir_all(tmp.path().join(dir)).expect("fixture dir");
        }
        write_executable(&tmp.path().join("bin/gh"), GH_STUB);
        let fixture = Self { tmp, repo };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.email", "fixture@example.com"]);
        fixture.git(&["config", "user.name", "Fixture"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fs::write(fixture.repo.join("README.md"), "base\n").expect("write README");
        fixture.git(&["add", "README.md"]);
        fixture.git(&["commit", "-q", "-m", "base"]);
        fixture.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        fixture.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        fixture.git(&["checkout", "-q", "-b", BRANCH]);
        fs::write(fixture.repo.join("feature.txt"), "feature\n").expect("write feature");
        fixture.git(&["add", "feature.txt"]);
        fixture.git(&["commit", "-q", "-m", "feat: change"]);
        fixture.set_origin(origin);
        fixture
    }

    /// Set the origin through argv and prove git stored it byte for byte
    /// (in particular exactly one backslash for the `\@` case).
    pub fn set_origin(&self, origin: &str) {
        self.git(&["config", "remote.origin.url", origin]);
        let stored = self.git(&["config", "--get", "remote.origin.url"]);
        assert_eq!(
            stored.trim_end_matches('\n'),
            origin,
            "git must store the origin unchanged"
        );
        assert_eq!(
            stored.matches('\\').count(),
            origin.matches('\\').count(),
            "backslash count must survive git config"
        );
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = amplihack_git::command_in(&self.repo)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    pub fn gh_log(&self) -> PathBuf {
        self.path("gh-calls.log")
    }

    /// Every logged gh call except `auth status`, which reads no repository.
    pub fn repo_gh_calls(&self) -> Vec<String> {
        fs::read_to_string(self.gh_log())
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with("auth status") && !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn all_gh_calls(&self) -> String {
        fs::read_to_string(self.gh_log()).unwrap_or_default()
    }

    /// A hermetic `program` command running in the repo.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let parent_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        let mut command = Command::new(program);
        command
            .env_clear()
            .current_dir(&self.repo)
            .env(
                "PATH",
                format!("{}:{parent_path}", self.path("bin").display()),
            )
            .env("HOME", self.path("home"))
            .env("TMPDIR", self.path("scratch"))
            .env("LANG", "C")
            .env("XDG_CONFIG_HOME", self.path("home/.config"))
            .env("GH_CONFIG_DIR", self.path("home/.config/gh"))
            .env("GH_TOKEN", "")
            .env("GH_STUB_LOG", self.gh_log())
            .env("GH_RETRY_TMPDIR", self.path("scratch"))
            .env("GH_RETRY_MAX_TRANSIENT", "1")
            .env("GH_RETRY_MAX_RL_WINDOWS", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file");
        command
    }

    pub fn run(&self, mut command: Command) -> Outcome {
        Outcome::from(command.output().expect("spawn child"))
    }
}
