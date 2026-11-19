//! Hermetic process and Git fixtures; no retired isolation implementation imports.
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;

pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn succeeds(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub home: PathBuf,
    pub repo: PathBuf,
    pub pr: PathBuf,
    pub state: PathBuf,
    pub work: PathBuf,
    pub bin: PathBuf,
    pub observed_head: String,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut f = Self {
            home: root.join("home"),
            repo: root.join("drive-clone"),
            pr: root.join("drive-clone/worktrees/pr-2808"),
            state: root.join("state"),
            work: root.join("work"),
            bin: root.join("bin"),
            observed_head: String::new(),
            temp,
        };
        for path in [&f.home, &f.state, &f.work, &f.bin] {
            private(path);
        }
        for name in [".claude", ".codex", ".amplihack", ".config"] {
            private(&f.home.join(name));
        }
        // An allowlist prevents any accidental real hosting/agent invocation.
        for tool in [
            "sh",
            "bash",
            "git",
            "cat",
            "cp",
            "mv",
            "rm",
            "mkdir",
            "chmod",
            "dirname",
            "basename",
            "mktemp",
            "readlink",
            "realpath",
            "find",
            "stat",
            "tar",
            "jq",
            "awk",
            "sed",
            "tr",
            "tail",
            "head",
            "sort",
            "cut",
            "wc",
            "sha256sum",
            "uname",
            "env",
            "touch",
            "date",
            "tee",
            "grep",
            "id",
        ] {
            let found = Command::new("/bin/sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            assert!(found.status.success(), "missing fixture utility {tool}");
            std::os::unix::fs::symlink(
                String::from_utf8(found.stdout).unwrap().trim(),
                f.bin.join(tool),
            )
            .unwrap();
        }
        write_executable(
            &f.bin.join("gh"),
            "#!/bin/sh\necho 'unexpected hosting request in local fixture' >&2\nexit 91\n",
            0o700,
        );
        std::os::unix::fs::symlink(amplihack_bin(), f.bin.join("amplihack")).unwrap();
        let origin = root.join("origin.git");
        f.git(&root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        f.git(
            &root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                f.repo.to_str().unwrap(),
            ],
        );
        f.git(&f.repo, &["checkout", "-qb", "main"]);
        fs::write(f.repo.join("tracked"), b"committed baseline\n").unwrap();
        fs::write(f.repo.join(".gitignore"), b"worktrees/\n").unwrap();
        f.git(&f.repo, &["add", "tracked", ".gitignore"]);
        f.git(&f.repo, &["commit", "-qm", "baseline"]);
        f.git(&f.repo, &["push", "-q", "-u", "origin", "main"]);
        f.git(
            &f.repo,
            &[
                "worktree",
                "add",
                "-qb",
                "feature/2808",
                f.pr.to_str().unwrap(),
            ],
        );
        fs::write(f.pr.join("pr-only"), b"observed PR revision\n").unwrap();
        f.git(&f.pr, &["add", "pr-only"]);
        f.git(&f.pr, &["commit", "-qm", "PR revision"]);
        f.git(&f.pr, &["push", "-q", "-u", "origin", "feature/2808"]);
        f.observed_head = f.git_text(&f.pr, &["rev-parse", "HEAD"]);
        f
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut c = crate::isolated_amplihack::isolated(program);
        c.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("AMPLIHACK_WORK_ROOT", &self.work)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", "Fixture Author")
            .env("GIT_AUTHOR_EMAIL", "author@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture Committer")
            .env("GIT_COMMITTER_EMAIL", "committer@example.invalid");
        c
    }

    pub fn git(&self, cwd: &Path, args: &[&str]) -> Vec<u8> {
        let mut c = self.command("git");
        amplihack_git::scrub(&mut c);
        let out = c.current_dir(cwd).args(args).output().unwrap();
        succeeds(&out);
        out.stdout
    }

    pub fn git_text(&self, cwd: &Path, args: &[&str]) -> String {
        String::from_utf8(self.git(cwd, args))
            .unwrap()
            .trim()
            .to_owned()
    }

    pub fn helper(&self, repo: &Path, head: &str, branch: &str) -> Output {
        self.command(amplihack_bin())
            .args(["orch", "helper", "autodrive", "worktree", "--repo"])
            .arg(repo)
            .args(["--head", head, "--branch", branch])
            .output()
            .unwrap()
    }

    pub fn recipe_command(&self, checkout: &Path) -> Command {
        let recipe = self.temp.path().join("build.yaml");
        fs::write(&recipe, "name: named-build\nversion: 1.0.0\nsteps:\n  - id: build\n    type: bash\n    command: 'true'\n").unwrap();
        self.recipe_command_for(checkout, &recipe)
    }

    pub fn recipe_command_for(&self, checkout: &Path, recipe: &Path) -> Command {
        let runner = self.bin.join("recipe-runner-rs");
        write_executable(&runner, RECORDING_RUNNER, 0o700);
        let observations = self.temp.path().join("observations");
        private(&observations);
        let mut c = self.command(amplihack_bin());
        c.env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("OBSERVATIONS", observations)
            .args(["recipe", "run"])
            .arg(recipe)
            .args(["--no-detach", "-w"])
            .arg(checkout);
        c
    }

    pub fn observation(&self, name: &str) -> Vec<u8> {
        fs::read(self.temp.path().join("observations").join(name)).unwrap()
    }

    pub fn assert_no_private_repositories(&self) {
        for file in files_under(&self.work) {
            assert!(
                !file.components().any(|part| matches!(
                    part.as_os_str().to_str(),
                    Some(
                        "repository.git"
                            | "checkout.exec-owner.json"
                            | "checkout.review-owner.json"
                    )
                )),
                "retired execution/review resource: {}",
                file.display()
            );
        }
        assert!(
            !self.work.join("exec").exists(),
            "run allocated private execution storage"
        );
    }
}

pub fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                files.extend(files_under(&path));
            } else {
                files.push(path);
            }
        }
    }
    files
}

// A runner boundary double, not proof of the pinned runner's execution.
const RECORDING_RUNNER: &str = r#"#!/bin/bash
set -eu
if [ "${1:-}" = --capabilities ]; then
  printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1","context_file_v1"]}'
  exit 0
fi
pwd -P > "$OBSERVATIONS/startup"
recipe="$1"
printf '%s' "$recipe" > "$OBSERVATIONS/recipe"
while [ $# -gt 0 ]; do
  case "$1" in
    -C|--working-dir) cd -- "$2"; shift 2 ;;
    --context-file) cp -- "$2" "$OBSERVATIONS/context.json"; shift 2 ;;
    *) shift ;;
  esac
done
pwd -P > "$OBSERVATIONS/cwd"
git rev-parse --show-toplevel > "$OBSERVATIONS/toplevel"
git rev-parse --path-format=absolute --git-common-dir > "$OBSERVATIONS/common"
git rev-parse HEAD > "$OBSERVATIONS/head"
git ls-files --stage -z > "$OBSERVATIONS/index-entries"
git config --get fixture.kept > "$OBSERVATIONS/config" || :
printf '%s' "${GH_TOKEN:-}" > "$OBSERVATIONS/credential"
printf '%s' "${GIT_AUTHOR_NAME:-}" > "$OBSERVATIONS/author"
for key in GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_CONFIG_COUNT GIT_CONFIG_PARAMETERS GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0 GIT_TRACE2 GIT_TRACE2_EVENT GIT_TRACE2_REDACT; do
  if [ "${!key+x}" ]; then printf '%s\n' "$key" >> "$OBSERVATIONS/git-routing"; fi
done
if [ "${WRITE_RESULT:-}" = 1 ]; then
  printf 'build output\n' > build-output
fi
if [ "${INTERRUPT_BUILD:-}" = 1 ]; then
  printf 'unpushed run work\n' > unpushed
  git add -- unpushed
  git commit -qm 'unpublished run work' -- unpushed
  printf 'unfinished run work\n' > unfinished
  exit 79
fi
printf '%s\n' '{"recipe_name":"named-build","success":true,"step_results":[],"context":{}}'
"#;
