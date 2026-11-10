//! Isolated publishing fixture: local refs, fake gh, no credentials or network.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

pub(super) const BRANCH: &str = "feat/issue-2306-publish";
pub(super) const ISSUE_TITLE: &str = "fix: recover draft publishing";

pub(super) fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub(super) fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub(super) struct PublishFixture {
    pub tmp: TempDir,
    pub repo: PathBuf,
    pub bin: PathBuf,
}

impl PublishFixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir(tmp.path().join("home")).unwrap();
        fs::create_dir(tmp.path().join("private")).unwrap();
        let fixture = Self { tmp, repo, bin };
        fixture.git(&["init", "-b", "main"]);
        fixture.git(&["config", "user.name", "Publish Test"]);
        fixture.git(&["config", "user.email", "publish@example.invalid"]);
        fixture.git(&["config", "core.hooksPath", "/dev/null"]);
        fs::write(fixture.repo.join("feature.txt"), "base\n").unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "base"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ]);
        fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        fixture.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        fixture.git(&["checkout", "-b", BRANCH]);
        fs::write(fixture.repo.join("feature.txt"), "feature\n").unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "fix: publish metadata"]);
        fixture.write(
            "issue.json",
            &serde_json::json!({"title": ISSUE_TITLE}).to_string(),
        );
        fixture.write("create.stderr", "");
        fixture.write("prs.json", "[]");
        fixture.write("collision.json", "[]");
        executable(&fixture.bin.join("gh"), GH);
        // Any accidental remote operation fails immediately; all refs are local.
        fixture.shim(
            "git",
            r#"
case "${1:-} ${2:-}" in
  "fetch "*|"push "*|"pull "*|"ls-remote "*|"remote set-head")
    echo 'test forbids Git network access' >&2; exit 97 ;;
esac
exec "$REAL_TOOL" "$@"
"#,
        );
        // A regression must not introduce a sleep or contact GitHub through curl.
        for name in ["sleep", "curl", "wget"] {
            executable(
                &fixture.bin.join(name),
                "#!/bin/bash\necho 'unexpected wait/network operation' >&2\nexit 97\n",
            );
        }
        fixture
    }

    pub fn write(&self, name: &str, value: &str) {
        fs::write(self.tmp.path().join(name), value).unwrap();
    }

    pub fn read(&self, name: &str) -> String {
        fs::read_to_string(self.tmp.path().join(name)).unwrap_or_default()
    }

    pub fn git(&self, args: &[&str]) {
        let result = amplihack_git::command_in(&self.repo)
            .args(args)
            .current_dir(&self.repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    /// Wrap a real PATH utility. These are fault injections, not BSD emulators.
    pub fn shim(&self, name: &str, body: &str) {
        let found = Command::new("bash")
            .args(["-c", "command -v \"$1\"", "shim", name])
            .output()
            .unwrap();
        assert!(found.status.success(), "missing test prerequisite {name}");
        let real = String::from_utf8(found.stdout).unwrap();
        // Paths come from command -v, never issue metadata.
        let quoted = real.trim().replace('\'', "'\\''");
        executable(
            &self.bin.join(name),
            &format!("#!/bin/bash\nREAL_TOOL='{quoted}'\n{body}"),
        );
    }

    pub fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        amplihack_git::scrub(&mut command);
        command
            .current_dir(&self.repo)
            .env_clear()
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("HOME", self.tmp.path().join("home"))
            .env("TMPDIR", self.tmp.path().join("private"))
            .env("GH_RETRY_TMPDIR", self.tmp.path().join("private"))
            .env("CASE_ROOT", self.tmp.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "true")
            .env("LC_ALL", "C")
            .env("REMOTE_HOST_TYPE", "github")
            .env("ISSUE_NUMBER", "2306")
            .env("AMPLIHACK_HOME", root())
            .env("RECIPE_VAR_task_description", "Add task title")
            .env(
                "RECIPE_VAR_worktree_setup",
                serde_json::json!({
                    "worktree_path": self.repo.canonicalize().unwrap()
                })
                .to_string(),
            )
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1");
        command
    }

    pub fn publish_command(&self) -> Command {
        let mut command = self.command("bash");
        command.arg("-c").arg(super::step_16_command());
        command
    }

    pub fn publish(&self) -> Output {
        self.publish_command().output().unwrap()
    }

    pub fn assert_created(&self, output: &Output) -> serde_json::Value {
        assert!(
            output.status.success(),
            "publish must recover successfully: {}",
            describe(output)
        );
        let result = json(output);
        assert_eq!(result["terminal_status"], "success");
        assert_eq!(result["state"], "FOLLOWUP_CREATED");
        assert_eq!(result["pr_url"], "https://github.com/owner/repo/pull/99");
        let args = fs::read(self.tmp.path().join("create.argv")).expect("must reach gh pr create");
        let args: Vec<_> = args.split(|b| *b == 0).collect();
        assert!(
            args.contains(&b"--draft".as_slice()),
            "recovery must create a draft"
        );
        assert!(args.contains(&b"--body-file".as_slice()));
        assert!(
            !args.contains(&b"--body".as_slice()),
            "body must stay out of argv"
        );
        assert!(
            !self.read("body.md").trim().is_empty(),
            "body must remain useful"
        );
        assert!(
            self.read("body.permissions").starts_with("-rw-------"),
            "body file must remain private"
        );
        assert!(
            !Path::new(&self.read("body.path")).exists(),
            "body file must be cleaned after creation"
        );
        assert_eq!(
            fs::read_dir(self.tmp.path().join("private"))
                .unwrap()
                .count(),
            0,
            "publishing must clean its private capture files"
        );
        result
    }

    pub fn inject_scope_failure(&self) {
        self.shim(
            "cut",
            r#"
if [ "${1:-}" = '-d/' ]; then
  printf 'injected scope derivation failure\n' >&2
  exit 71
fi
exec "$REAL_TOOL" "$@"
"#,
        );
    }

    pub fn pr(&self, state: &str, head: &str) -> serde_json::Value {
        let sha = amplihack_git::command_in(&self.repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(sha.status.success());
        serde_json::json!({
            "number": 99, "url": "https://github.com/owner/repo/pull/99",
            "title": "fix: recover draft publishing (#2306)",
            "body": "Fixes #2306", "state": state,
            "createdAt": "2026-11-10T00:00:00Z",
            "mergedAt": if state == "MERGED" { Some("2026-11-10T01:00:00Z") } else { None },
            "headRefName": head, "baseRefName": "main",
            "headRefOid": String::from_utf8(sha.stdout).unwrap().trim(),
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"}, "isCrossRepository": false
        })
    }
}

pub(super) fn describe(output: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(super) fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout must contain exactly one JSON result: {e}\n{}",
            describe(output)
        )
    })
}

const GH: &str = r#"#!/bin/bash
set -eu
printf '%s\n' "$*" >> "$CASE_ROOT/gh.log"
case "${1:-} ${2:-}" in
  'pr list')
    if [ "${LOOKUP_STATUS:-0}" != 0 ]; then
      echo 'authentication failed: distinctive lookup rejection' >&2
      exit "$LOOKUP_STATUS"
    fi
    case " $* " in
      *' --head '*)
        if [ -e "$CASE_ROOT/create.argv" ]; then
          if [ "${COLLISION_LOOKUP_FAIL:-0}" = 1 ]; then
            echo 'distinctive collision lookup rejection' >&2; exit 42
          fi
          cat "$CASE_ROOT/collision.json"
        else cat "$CASE_ROOT/prs.json"; fi ;;
      *) printf '[]\n' ;;
    esac ;;
  'repo view') printf 'main\n' ;;
  'issue view')
    : > "$CASE_ROOT/issue-looked-up"
    case "${ISSUE_MODE:-ok}" in
      failed) echo 'issue lookup denied' >&2; exit 41 ;;
      unavailable) echo 'gh issue view unavailable' >&2; exit 127 ;;
    esac
    cat "$CASE_ROOT/issue.json" ;;
  'pr create')
    printf '%s\0' "$@" >> "$CASE_ROOT/create.argv"
    shift 2
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --title) printf '%s' "$2" > "$CASE_ROOT/title"; shift ;;
        --body-file)
          cat "$2" > "$CASE_ROOT/body.md"
          ls -l "$2" > "$CASE_ROOT/body.permissions"
          printf '%s' "$2" > "$CASE_ROOT/body.path"; shift ;;
      esac
      shift
    done
    if [ "${CREATE_STATUS:-0}" != 0 ]; then
      cat "$CASE_ROOT/create.stderr" >&2; exit "$CREATE_STATUS"
    fi
    printf 'https://github.com/owner/repo/pull/99\n' ;;
  *) printf 'unexpected fake gh operation\n' >&2; exit 96 ;;
esac
"#;
