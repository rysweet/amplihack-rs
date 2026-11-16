//! Issue #2738 shared fixture: a real `amplihack recipe run` driven by a stub
//! recipe runner, used by the behaviour tests that outlive the Git-command
//! tracer removed in this change.
//!
//! The fixture builds a drive-shaped caller: a bare `origin.git` and a
//! working clone at `<root>/caller` that pushes `main` to it, so a test can
//! add a `worktrees/pr-N` worktree without inventing a second layout. Every
//! path is inside one `tempfile` root; nothing reads or writes the
//! developer's real `~/.claude` or `~/.amplihack`.
//!
//! The stub runner records where it was started, what Git sees there, and,
//! on request, runs Git from a directory that is not a repository (#2737),
//! lists the caller's worktrees, or stages and commits inside the run's own
//! checkout. It can also end on its own terms, with an exit code of the
//! test's choosing or with a report of its own failure, so a test can show
//! that the run's status is the runner's.
//!
//! Wired as a module of `crates/amplihack-cli/tests/main.rs`; the files that
//! use it are wired there too.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The stub runner. `--capabilities` answers the launch-time probe; otherwise
/// it enters the runner's `-C` directory and records what it finds there.
const STUB_RUNNER: &str = r#"#!/bin/sh
set -eu
if [ "${1:-}" = --capabilities ]; then
  printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}'
  exit 0
fi
out="$ISSUE_2738_OBS"
mkdir -p "$out"
printf '%s' "$(pwd -P)" > "$out/startup_cwd"
# The run's status is the runner's own: these two modes end the runner on its
# own terms, and nothing else may decide what the run reports (#2738).
if [ -n "${ISSUE_2738_RUNNER_EXIT:-}" ]; then
  printf 'the runner ended on its own terms\n' >&2
  exit "$ISSUE_2738_RUNNER_EXIT"
fi
if [ -n "${ISSUE_2738_RUNNER_REPORTS_FAILURE:-}" ]; then
  printf '%s\n' '{"recipe_name":"issue-2738","success":false,"step_results":[],"context":{}}'
  exit 0
fi
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    -C) dir="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cd "$dir"
printf '%s' "$(pwd -P)" > "$out/cwd"
printf '%s' "${AMPLIHACK_RUN_CHECKOUT:-}" > "$out/run_checkout"
git rev-parse HEAD > "$out/head"
git symbolic-ref -q HEAD > "$out/branch" || true
# Issue #2737: Git started outside any repository while the run is in flight.
if [ -n "${ISSUE_2738_OUTSIDE:-}" ]; then
  mkdir -p "$ISSUE_2738_OUTSIDE"
  status=0
  ( cd "$ISSUE_2738_OUTSIDE" && git ls-remote "$ISSUE_2738_REMOTE" ) \
    > "$out/ls_remote.out" 2> "$out/ls_remote.err" || status=$?
  printf '%s' "$status" > "$out/ls_remote.status"
  status=0
  ( cd "$ISSUE_2738_OUTSIDE" && git clone --quiet "$ISSUE_2738_REMOTE" clone ) \
    > "$out/clone.out" 2> "$out/clone.err" || status=$?
  printf '%s' "$status" > "$out/clone.status"
fi
# Issue #2290's own report: `git -C <main root> worktree list` stopped working
# under the observer in a drive-shaped clone.
if [ -n "${ISSUE_2738_WORKTREE_LIST:-}" ]; then
  status=0
  git -C "$ISSUE_2738_WORKTREE_LIST" worktree list --porcelain \
    > "$out/caller_worktrees" 2> "$out/caller_worktrees.err" || status=$?
  printf '%s' "$status" > "$out/caller_worktrees.status"
fi
if [ -n "${ISSUE_2738_STAGE:-}" ]; then
  printf 'private output\n' > run-output
  git add -- run-output
  git commit -qm "the run's own commit"
fi
# A commit whose pre-commit hook is itself a Git-running descendant. Carried
# over from the recording tests removed in #2738, which asserted through it
# that the caller's checkout is untouched by a hook-driven commit.
if [ -n "${ISSUE_2738_HOOK:-}" ]; then
  hooks="$out/hooks"
  mkdir -p "$hooks"
  cat > "$hooks/pre-commit" <<'HOOK'
#!/bin/sh
set -eu
printf 'hook descendant\n' > hook-output
git add -- hook-output
HOOK
  chmod 0755 "$hooks/pre-commit"
  git -c "core.hooksPath=$hooks" \
      -c user.name=Fixture -c user.email=fixture@example.invalid \
      commit --allow-empty -qm "hook coverage"
  git show HEAD:hook-output > "$out/hook_committed"
fi
printf '{"recipe_name":"issue-2738","success":true,"step_results":[],"context":{}}\n'
"#;

pub(crate) struct Fixture {
    _temp: tempfile::TempDir,
    /// The tempfile root every other path lives under.
    pub(crate) root: PathBuf,
    /// The caller's checkout: a clone of `origin`, never written by a run.
    pub(crate) repo: PathBuf,
    /// The bare repository `repo` pushes to, and a local remote for #2737.
    pub(crate) origin: PathBuf,
    home: PathBuf,
    runner: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repo = root.join("caller");
        let home = root.join("home");
        let origin = root.join("origin.git");
        for dir in [&repo, &home] {
            fs::create_dir_all(dir).unwrap();
        }
        let runner = root.join("recipe-runner-rs");
        crate::support::exec_stub::write_executable(&runner, STUB_RUNNER, 0o755);
        let f = Self {
            _temp: temp,
            root,
            repo,
            origin,
            home,
            runner,
        };
        f.git_in(
            &f.root,
            &["init", "--bare", "-q", "-b", "main", "origin.git"],
        );
        f.git(&["init", "-q", "-b", "main"]);
        f.git(&["config", "user.name", "Issue 2738 Fixture"]);
        f.git(&["config", "user.email", "fixture@example.invalid"]);
        f.git(&["config", "commit.gpgsign", "false"]);
        fs::write(f.repo.join("tracked"), b"base\n").unwrap();
        // The drive's own layout: pull-request worktrees live under
        // `worktrees/` and the clone ignores them, as this repository does.
        fs::write(f.repo.join(".gitignore"), b"worktrees/\n").unwrap();
        f.git(&["add", "tracked", ".gitignore"]);
        f.git(&["commit", "-qm", "fixture"]);
        f.git(&["remote", "add", "origin", f.origin.to_str().unwrap()]);
        f.git(&["push", "-q", "-u", "origin", "main"]);
        fs::write(
            f.root.join("recipe.yaml"),
            "name: issue-2738\ndescription: issue 2738 behaviour check\nversion: 1.0.0\n\
             context: {}\nsteps:\n  - id: stub\n    type: bash\n    command: \"true\"\n",
        )
        .unwrap();
        f
    }

    /// `worktrees/<name>` on its own branch, the shape an auto-drive leaves in
    /// a clone. Returns the worktree's path.
    pub(crate) fn add_worktree(&self, name: &str) -> PathBuf {
        let path = self.repo.join("worktrees").join(name);
        self.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("pr/{name}"),
            path.to_str().unwrap(),
        ]);
        path
    }

    /// A disposable home and no global or system Git configuration; the
    /// test's own `PATH`.
    fn environment(&self, c: &mut Command) {
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("TMPDIR", &self.root)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0");
    }

    pub(crate) fn git(&self, args: &[&str]) -> Vec<u8> {
        self.git_in(&self.repo, args)
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        let mut c = amplihack_git::command();
        self.environment(&mut c);
        let out = c.current_dir(dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// `amplihack recipe run <recipe.yaml> -w <dir>`, with the stub runner and
    /// the observations of this label.
    pub(crate) fn command_in(&self, label: &str, dir: &Path) -> Command {
        let mut c =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        self.environment(&mut c);
        c.env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_WORK_ROOT", self.root.join("work"))
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("ISSUE_2738_OBS", self.root.join(label))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .current_dir(dir)
            .args(["recipe", "run"])
            .arg(self.root.join("recipe.yaml"))
            .arg("-w")
            .arg(dir);
        c
    }

    pub(crate) fn command(&self, label: &str) -> Command {
        self.command_in(label, &self.repo)
    }

    /// Run `cmd` to completion under the suite's hang guard (issue #1973).
    ///
    /// A run that never ends is a real failure of this change, so it must
    /// fail naming itself rather than as an unattributed CI timeout.
    pub(crate) fn finish(label: &str, cmd: &mut Command) -> Output {
        crate::support::wait::output_within(&format!("the {label} recipe run to end"), cmd)
    }

    pub(crate) fn observed(&self, label: &str, name: &str) -> Vec<u8> {
        fs::read(self.root.join(label).join(name))
            .unwrap_or_else(|e| panic!("observation {label}/{name}: {e}"))
    }

    pub(crate) fn observed_str(&self, label: &str, name: &str) -> String {
        String::from_utf8(self.observed(label, name)).expect("UTF-8 observation")
    }

    /// The caller's index, `HEAD`, a tracked file and its Git configuration.
    pub(crate) fn snapshot(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            fs::read(self.repo.join(".git/index")).unwrap(),
            self.git(&["rev-parse", "HEAD"]),
            fs::read(self.repo.join("tracked")).unwrap(),
            fs::read(self.repo.join(".git/config")).unwrap(),
        )
    }

    pub(crate) fn worktrees(&self) -> String {
        String::from_utf8(self.git(&["worktree", "list", "--porcelain"])).unwrap()
    }
}

/// Fail with the run's status and both streams, so a failure names the cause
/// rather than only the exit code.
pub(crate) fn succeeds(out: &Output) {
    assert!(
        out.status.success(),
        "the recipe run did not finish: status {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The message #2737 ended every auto-drive with. Nothing in the tree may
/// produce it once the tracer is gone.
pub(crate) const ATTRIBUTION_MESSAGE: &str =
    "cannot attribute an observed Git command to its repository/index";
