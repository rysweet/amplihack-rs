//! Shared fixture for the issue #2535 `amplihack recipe guide` tests.
//!
//! Every run uses the real `amplihack` binary with a cleared environment and
//! a private temporary `HOME`, `AMPLIHACK_STATE_DIR` and work root
//! (`$HOME/.amplihack/work`). Nothing under the real `~/.amplihack` or
//! `~/.claude` is read or written; no credential file is read.
//!
//! Fakes, all shell scripts in the fixture's `tools/`:
//!
//! - `claude`: answers `--version`; otherwise copies all of stdin to
//!   `$RECORD_DIR/prompt.$STEP`, records `AMPLIHACK_RUN_GUIDANCE_DIRS` in
//!   `$RECORD_DIR/agentdirs.$STEP`, and prints `$AGENT_OUTPUT` (default
//!   `agent output`) on stdout.
//! - `codex`: answers `--version` and `plugin` probes; otherwise copies stdin
//!   to `$RECORD_DIR/prompt.$STEP` and writes the `--output-last-message` file.
//! - `recipe-runner-rs`: the script given to [`GuideFixture::recipe_run`]; it
//!   answers `--capabilities` and then runs the test's steps, which call the
//!   real launcher (`$AMP_BIN claude …` / `$AMP_BIN codex …`).
//! - `publish/gh` and `publish/git`: capture argv and any `--body-file`,
//!   `-F`/`--file` file into `$RECORD_DIR`. They are only on `PATH` for the
//!   commands a runner script prefixes with `PATH="$PUBLISH_TOOLS:$PATH"`.

#![cfg(unix)]
#![allow(dead_code)]

use crate::support::amplihack_bin::amplihack_bin;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const BEGIN: &str = "===== BEGIN OPERATOR INSTRUCTIONS FOR THIS RUN =====";
pub const END: &str = "===== END OPERATOR INSTRUCTIONS FOR THIS RUN =====";
pub const LIMIT: usize = 51_200;
pub const PR_URL: &str = "https://github.com/o/r/pull/7";
pub const PR_KEY: &str = "https___github.com_o_r_7";

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
cat > "$RECORD_DIR/prompt.$step"
printf '%s' "${AMPLIHACK_RUN_GUIDANCE_DIRS-<unset>}" > "$RECORD_DIR/agentdirs.$step"
printf '%s\n' "${AGENT_OUTPUT:-agent output}"
exit 0
"#;

const FAKE_CODEX: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'codex-cli 0.166.0'; exit 0;;
  plugin)
    case "$2" in
      list) echo '{"installed":[]}';;
      *) echo '{"ok":true}';;
    esac
    exit 0;;
esac
step="${STEP:-0}"
: > "$RECORD_DIR/codexargv.$step"
prev=
for arg in "$@"; do
  printf '%s\n' "$arg" >> "$RECORD_DIR/codexargv.$step"
  if [ "$prev" = "--output-last-message" ]; then
    printf 'fake codex final message\n' > "$arg"
  fi
  prev=$arg
done
cat > "$RECORD_DIR/prompt.$step"
exit 0
"#;

const FAKE_GH: &str = r#"#!/bin/sh
n=$(( $(cat "$RECORD_DIR/gh.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/gh.count"
: > "$RECORD_DIR/gh.argv.$n"
prev=
for a in "$@"; do
  printf '%s\n' "$a" >> "$RECORD_DIR/gh.argv.$n"
  if [ "$prev" = "--body-file" ]; then cp "$a" "$RECORD_DIR/gh.body.$n"; fi
  prev=$a
done
echo "https://github.com/o/r/pull/7"
"#;

const FAKE_GIT: &str = r#"#!/bin/sh
n=$(( $(cat "$RECORD_DIR/git.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/git.count"
: > "$RECORD_DIR/git.argv.$n"
prev=
for a in "$@"; do
  printf '%s\n' "$a" >> "$RECORD_DIR/git.argv.$n"
  case "$prev" in -F|--file) cp "$a" "$RECORD_DIR/git.msgfile.$n" ;; esac
  prev=$a
done
exit 0
"#;

/// The runner's answer to `--capabilities`, as in the #1570 tests.
pub const RUNNER_HEAD: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
. "$FIXTURE_DIR/launch.env"
tag="${RUN_TAG:-run}"
printf '%s\n' "$AMPLIHACK_RUN_ID" > "$RECORD_DIR/run_id.$tag"
printf '%s\n' "$AMPLIHACK_RECIPE_RUN_ID" > "$RECORD_DIR/corr_id.$tag"
printf '%s' "${AMPLIHACK_RUN_GUIDANCE_DIRS-<unset>}" > "$RECORD_DIR/dirs.$tag"
"#;

/// The runner's success line.
pub const RUNNER_TAIL: &str = r#"
printf '%s\n' '{"recipe_name":"issue-2535-guide","success":true,"step_results":[],"context":{}}'
"#;

const RECIPE: &str = r#"name: issue-2535-guide
description: Two agent steps for the recipe guide tests
version: 1.0.0
context: {}
steps:
  - id: step-one
    type: bash
    command: "true"
"#;

pub struct GuideFixture {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub state: PathBuf,
    pub work: PathBuf,
    pub recipe_logs: PathBuf,
    pub records: PathBuf,
    pub tools: PathBuf,
    pub publish: PathBuf,
    pub profiles: PathBuf,
    pub claude_config: PathBuf,
    pub codex_home: PathBuf,
    pub recipe: PathBuf,
}

pub fn private_dir(p: &Path) {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(p)
        .unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn private_file(p: &Path, body: impl AsRef<[u8]>) {
    if let Some(parent) = p.parent() {
        private_dir(parent);
    }
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn show(o: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn executable(path: &Path, body: &str) {
    crate::support::exec_stub::write_executable(path, body, 0o755);
}

impl GuideFixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let state = root.join("state");
        let work = home.join(".amplihack").join("work");
        let recipe_logs = home.join(".amplihack").join("recipe-logs");
        let records = root.join("records");
        let tools = root.join("tools");
        let publish = root.join("publish");
        let profiles = root.join("profiles");
        let claude_config = root.join("claude-config");
        let codex_home = home.join(".codex");
        // `$HOME/.amplihack` private before amplihack creates it: under a
        // umask of 002 it would be 0775, and the runner's recovery store
        // refuses a group-writable ancestor ("unsafe recovery ancestor").
        let dot_amplihack = home.join(".amplihack");
        for d in [
            &home,
            &dot_amplihack,
            &records,
            &tools,
            &publish,
            &profiles,
            &claude_config,
        ] {
            private_dir(d);
        }
        executable(&tools.join("claude"), FAKE_CLAUDE);
        executable(&tools.join("codex"), FAKE_CODEX);
        executable(&tools.join("codex-code-mode-host"), "#!/bin/sh\nexit 0\n");
        executable(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n");
        executable(&publish.join("gh"), FAKE_GH);
        executable(&publish.join("git"), FAKE_GIT);
        private_file(
            &codex_home.join("config.toml"),
            "sandbox_mode = \"danger-full-access\"\n",
        );
        let recipe = root.join("recipe.yaml");
        fs::write(&recipe, RECIPE).unwrap();
        let f = Self {
            _tmp: tmp,
            root,
            home,
            state,
            work,
            recipe_logs,
            records,
            tools,
            publish,
            profiles,
            claude_config,
            codex_home,
            recipe,
        };
        f.write_launch_env();
        f
    }

    /// Variables the runner script sources before it calls the launcher, so
    /// they reach the launcher whatever `recipe run` passes on.
    fn write_launch_env(&self) {
        let lines = [
            ("AMP_BIN", amplihack_bin().to_string_lossy().into_owned()),
            ("HOME", self.home.display().to_string()),
            ("AMPLIHACK_STATE_DIR", self.state.display().to_string()),
            ("RECORD_DIR", self.records.display().to_string()),
            ("PUBLISH_TOOLS", self.publish.display().to_string()),
            (
                "AMPLIHACK_CLAUDE_BINARY_PATH",
                self.tools.join("claude").display().to_string(),
            ),
            (
                "CODEX_BINARY_PATH",
                self.tools.join("codex").display().to_string(),
            ),
            ("CODEX_HOME", self.codex_home.display().to_string()),
            (
                "CLAUDE_CONFIG_DIR",
                self.claude_config.display().to_string(),
            ),
            (
                "AMPLIHACK_CLAUDE_PROFILE_ROOT",
                self.profiles.display().to_string(),
            ),
            ("AMPLIHACK_ROTATE_AUTOSTART", "0".into()),
            ("AMPLIHACK_GOVERNOR", "off".into()),
            ("AMPLIHACK_NONINTERACTIVE", "1".into()),
            ("AMPLIHACK_NO_FRESHNESS_CHECK", "1".into()),
            ("AMPLIHACK_NO_UPDATE_CHECK", "1".into()),
            ("AMPLIHACK_SKIP_AUTO_INSTALL", "1".into()),
            ("AMPLIHACK_SKIP_POWER_STEERING", "1".into()),
            ("AMPLIHACK_SKIP_REFLECTION", "1".into()),
            ("AMPLIHACK_DISABLE_BLARIFY", "1".into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ];
        let mut body = String::new();
        for (k, v) in lines {
            body.push_str(&format!("{k}='{}'\nexport {k}\n", v.replace('\'', "'\\''")));
        }
        private_file(&self.root.join("launch.env"), body);
    }

    fn path_var(&self) -> String {
        format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display())
    }

    /// An isolated `amplihack` command with the fixture's environment.
    pub fn amplihack(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.root)
            .env("PATH", self.path_var())
            .env("HOME", &self.home)
            .env("TMPDIR", std::env::temp_dir())
            .env("AMPLIHACK_STATE_DIR", &self.state)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("RECORD_DIR", &self.records)
            .env("FIXTURE_DIR", &self.root)
            .env("USER", "alice")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("CODEX_HOME", &self.codex_home)
            .env("CLAUDE_CONFIG_DIR", &self.claude_config)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }

    /// Runs `c`, writing `stdin` (or nothing, with stdin at /dev/null).
    pub fn run_with_stdin(&self, mut c: Command, stdin: Option<&[u8]>) -> Output {
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        match stdin {
            None => {
                c.stdin(Stdio::null());
                c.output().expect("run amplihack")
            }
            Some(bytes) => {
                c.stdin(Stdio::piped());
                let mut child = c.spawn().expect("start amplihack");
                {
                    let mut input = child.stdin.take().unwrap();
                    let _ = input.write_all(bytes);
                }
                child.wait_with_output().unwrap()
            }
        }
    }

    /// `amplihack recipe guide <args…>` with stdin at /dev/null or `stdin`.
    pub fn guide(&self, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let mut full = vec!["recipe", "guide"];
        full.extend_from_slice(args);
        self.run_with_stdin(self.amplihack(&full), stdin)
    }

    pub fn runs(&self) -> PathBuf {
        self.work.join("runs")
    }

    /// A run folder `<work>/runs/<folder_id>/` with a `run.json` naming
    /// `correlation_id` as its attempt, as `recipe run` writes it.
    pub fn make_run(&self, folder_id: &str, correlation_id: &str) -> PathBuf {
        let dir = self.runs().join(folder_id);
        private_dir(&self.work);
        private_dir(&self.runs());
        private_dir(&dir);
        let report = serde_json::json!({
            "schema_version": 1,
            "run_id": folder_id,
            "recipe": "smart-orchestrator",
            "pid": 0,
            "cwd": self.root.display().to_string(),
            "started_at": "2026-11-12T09:12:00Z",
            "outcome": "running",
            "attempts": [{"correlation_id": correlation_id}],
            "steps": [],
        });
        private_file(&dir.join("run.json"), report.to_string());
        dir
    }

    /// `<recipe-logs>/<session>.log` whose first pointer line names `run_id`.
    pub fn session_log(&self, session: &str, run_id: &str) -> PathBuf {
        let log = self.recipe_logs.join(format!("{session}.log"));
        let pointer = serde_json::json!({"run_id": run_id, "status": "started"});
        private_file(
            &log,
            format!("starting run\namplihack.recipe.log_pointer {pointer}\nmore output\n"),
        );
        log
    }

    pub fn pr_parent(&self) -> PathBuf {
        self.state.join("auto-drive").join(PR_KEY)
    }

    /// The `*.md` entries of a guidance folder, sorted.
    pub fn entries(&self, guidance: &Path) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(guidance) else {
            return Vec::new();
        };
        let mut v: Vec<PathBuf> = rd
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension() == Some(OsStr::new("md")))
            .collect();
        v.sort();
        v
    }

    /// The parsed lines of the `guidance.log` beside `guidance`.
    pub fn log(&self, guidance: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(guidance.parent().unwrap().join("guidance.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("guidance.log lines are JSON"))
            .collect()
    }

    pub fn record(&self, name: &str) -> String {
        fs::read_to_string(self.records.join(name))
            .unwrap_or_else(|e| panic!("record {name} missing: {e}"))
    }

    pub fn record_opt(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.records.join(name)).ok()
    }

    /// Every file under `records/` whose name starts with `prefix`, joined.
    pub fn records_starting(&self, prefix: &str) -> String {
        let mut out = String::new();
        let mut names: Vec<_> = fs::read_dir(&self.records)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(prefix))
            .collect();
        names.sort();
        for n in names {
            out.push_str(&self.record(&n));
            out.push('\n');
        }
        out
    }

    /// `amplihack recipe run <fixture recipe> -w <root> <args…>` with
    /// `runner_body` (after [`RUNNER_HEAD`], before [`RUNNER_TAIL`]) as the
    /// recipe runner and `extra_env` added.
    pub fn recipe_run(
        &self,
        runner_body: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> Output {
        let runner = self.tools.join("recipe-runner-rs");
        let _ = fs::remove_file(&runner);
        executable(&runner, &format!("{RUNNER_HEAD}{runner_body}{RUNNER_TAIL}"));
        let mut full = vec!["recipe", "run"];
        let recipe = self.recipe.to_string_lossy().into_owned();
        let root = self.root.to_string_lossy().into_owned();
        full.push(&recipe);
        full.extend_from_slice(args);
        full.extend_from_slice(&["-w", &root]);
        let mut c = self.amplihack(&full);
        c.env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude");
        for (k, v) in extra_env {
            c.env(k, v);
        }
        self.run_with_stdin(c, None)
    }

    /// `amplihack claude --subprocess-safe -- -p <prompt>` run directly, with
    /// `AMPLIHACK_RUN_GUIDANCE_DIRS` set to `dirs` when given.
    pub fn launch_claude(&self, step: &str, prompt: &str, dirs: Option<&OsStr>) -> Output {
        let mut c = self.amplihack(&["claude", "--subprocess-safe", "--", "-p", prompt]);
        c.env("STEP", step).env("AMPLIHACK_AGENT_BINARY", "claude");
        match dirs {
            Some(d) => c.env("AMPLIHACK_RUN_GUIDANCE_DIRS", d),
            None => c.env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS"),
        };
        self.run_with_stdin(c, None)
    }

    /// `amplihack codex -- <tail…>` run directly with `stdin`.
    pub fn launch_codex(
        &self,
        step: &str,
        tail: &[&str],
        stdin: &[u8],
        dirs: Option<&OsStr>,
    ) -> Output {
        let mut args = vec!["codex", "--"];
        args.extend_from_slice(tail);
        let mut c = self.amplihack(&args);
        c.env("STEP", step).env("AMPLIHACK_AGENT_BINARY", "codex");
        match dirs {
            Some(d) => c.env("AMPLIHACK_RUN_GUIDANCE_DIRS", d),
            None => c.env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS"),
        };
        self.run_with_stdin(c, Some(stdin))
    }
}

/// The one stdout line of a successful `recipe guide`:
/// `guidance appended: <path> (<n> bytes, run|pr)`. Returns (path, n, source).
pub fn parse_appended(o: &Output) -> (PathBuf, usize, String) {
    let out = stdout(o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line: {}", show(o));
    let re =
        regex::Regex::new(r"^guidance appended: (/.+\.md) \((\d+) bytes, (run|pr)\)$").unwrap();
    let caps = re
        .captures(lines[0])
        .unwrap_or_else(|| panic!("stdout line does not match the contract: {}", show(o)));
    (
        PathBuf::from(&caps[1]),
        caps[2].parse().unwrap(),
        caps[3].to_string(),
    )
}

/// A failure prints exactly one line on stderr.
pub fn one_line_error(o: &Output) -> String {
    let err = stderr(o);
    let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one stderr line: {}", show(o));
    lines[0].to_string()
}
