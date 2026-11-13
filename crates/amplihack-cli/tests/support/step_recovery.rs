//! Issue #2539 fixtures for `amplihack orch helper step-recovery`.
//!
//! Every test gets its own scratch tree with a private `HOME`, `TMPDIR`,
//! `XDG_STATE_HOME` and `AMPLIHACK_HOME`, and runs the real `amplihack`
//! binary through [`isolated`](crate::isolated_amplihack::isolated), which
//! clears the inherited environment (so no `RECIPE_VAR_*`, `REPO_PATH` or
//! `AMPLIHACK_STEP_RECOVERY_ACTIVE` from an enclosing run reaches it).
//!
//! The driver starts the recovery recipe with `amplihack recipe run
//! step-failure-recovery -c step_recovery_evidence=<path>`. Here the recipe
//! runner is a stub (`RECIPE_RUNNER_RS_PATH`) that plays the agent, because
//! the real runner is not installed everywhere these tests run. The stub:
//! - logs each call as `<n>\t<AMPLIHACK_STEP_RECOVERY_ACTIVE>\t<argv>` in
//!   `agent/calls`, so a test can tell that no agent was started;
//! - copies the evidence file it was given to `agent/evidence-<n>.json`;
//! - runs `agent/repair-<n>.sh` when present (the agent's local repair);
//! - prints `agent/result-<n>.json`, a runner result whose one step output
//!   ends with the scripted `{"step_recovery":…}` reply.
//!
//! `AMPLIHACK_AGENT_BINARY` is set to `claude` so the inherited agent choice
//! is the one tests name, as the design requires.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
pub use crate::support::step_recovery_check::{assert_valid_record, outcome};

pub const HANDOFF_EXIT: i32 = 81;

const STUB_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
case "$*" in --version) echo fixture; exit 0 ;; esac
d="$STEP_RECOVERY_AGENT_DIR"
n=$(( $(cat "$d/calls" 2>/dev/null | wc -l) + 1 ))
printf '%s\t%s\t%s\n' "$n" "${AMPLIHACK_STEP_RECOVERY_ACTIVE-}" "$*" >> "$d/calls"
for a in "$@"; do
  case "$a" in step_recovery_evidence=*) cp "${a#step_recovery_evidence=}" "$d/evidence-$n.json" ;; esac
done
[ -x "$d/repair-$n.sh" ] && "$d/repair-$n.sh"
if [ -f "$d/result-$n.json" ]; then cat "$d/result-$n.json"; else
  printf '%s\n' '{"recipe_name":"step-failure-recovery","success":true,"step_results":[{"step_id":"recover","status":"completed","output":""}],"context":{}}'
fi
"#;

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub tools: PathBuf,
    pub agent: PathBuf,
    pub work: PathBuf,
    /// The private recovery directory (0700) most tests pass as `--dir`.
    pub dir: PathBuf,
    pub state_file: PathBuf,
}

pub fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self {
            // The binary's test guard refuses a HOME outside the child's
            // TMPDIR, so HOME sits under the fixture's TMPDIR.
            home: root.join("tmp").join("home"),
            tools: root.join("tools"),
            agent: root.join("agent"),
            work: root.join("work"),
            dir: root.join("state").join("recovery"),
            state_file: root.join("state").join("run-state.json"),
            root,
            tmp,
        };
        for d in [&f.root.join("tmp"), &f.home, &f.tools, &f.agent, &f.work] {
            private_dir(d);
        }
        private_dir(&f.root.join("state"));
        private_dir(&f.dir);
        fs::write(&f.state_file, r#"{"phase":"merge","round":3}"#).unwrap();
        write_executable(&f.tools.join("recipe-runner-rs"), STUB_RUNNER, 0o755);
        let git = amplihack_git::command_in(&f.work)
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(git.success());
        f
    }

    /// Writes an executable tool into the fixture's `tools` directory.
    pub fn tool(&self, name: &str, body: &str) -> PathBuf {
        let path = self.tools.join(name);
        write_executable(&path, body, 0o755);
        path
    }

    /// Scripts the agent's `n`th reply. `reply` is the whole last line.
    pub fn reply(&self, n: usize, reply: &str) {
        let output = format!("Read the evidence file and decided.\n{reply}");
        let result = json!({
            "recipe_name": "step-failure-recovery",
            "success": true,
            "step_results": [{"step_id": "recover", "status": "completed", "output": output}],
            "context": {}
        });
        fs::write(
            self.agent.join(format!("result-{n}.json")),
            result.to_string(),
        )
        .unwrap();
    }

    /// Scripts the agent's `n`th reply as a `{"step_recovery": body}` line.
    pub fn choose(&self, n: usize, body: Value) {
        self.reply(n, &json!({ "step_recovery": body }).to_string());
    }

    /// A shell script the agent runs as its repair before its `n`th reply.
    pub fn repair(&self, n: usize, body: &str) {
        write_executable(&self.agent.join(format!("repair-{n}.sh")), body, 0o755);
    }

    /// Lines of `agent/calls`: one per recovery agent started.
    pub fn agent_calls(&self) -> Vec<String> {
        fs::read_to_string(self.agent.join("calls"))
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The evidence file the agent was given on its `n`th call.
    pub fn evidence(&self, n: usize) -> Value {
        let path = self.agent.join(format!("evidence-{n}.json"));
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("agent call {n} got no evidence file ({e})"));
        serde_json::from_str(&text).expect("evidence is JSON")
    }

    pub fn amplihack(&self) -> Command {
        // Under umask 077, as a private run is: the real recipe runner
        // refuses a group-writable directory in its recovery store, and the
        // host umask may be 002.
        let mut cmd = crate::isolated_amplihack::isolated("/bin/sh");
        cmd.args(["-c", "umask 077 && exec \"$0\" \"$@\""])
            .arg(amplihack_bin());
        let path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display());
        cmd.current_dir(&self.work)
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("TMPDIR", self.root.join("tmp"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("STEP_RECOVERY_AGENT_DIR", &self.agent)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env_remove("AMPLIHACK_STEP_RECOVERY_ACTIVE")
            .env_remove("REPO_PATH")
            .stdin(Stdio::null());
        cmd
    }

    /// `step-recovery run` with the fixture's dir, goal and state file, the
    /// bundle's recipes as `--recipe-dir`, `extra` flags, then `-- command`.
    pub fn run_cmd(&self, step_id: &str, extra: &[&str], command: &[&str]) -> Command {
        let mut cmd = self.amplihack();
        cmd.args([
            "orch",
            "helper",
            "step-recovery",
            "run",
            "--step-id",
            step_id,
        ])
        .arg("--dir")
        .arg(&self.dir)
        .args(["--goal", "Merge the fixture pull request"])
        .arg("--state-file")
        .arg(&self.state_file)
        .arg("--recipe-dir")
        .arg(bundle_recipes())
        .args(extra)
        .arg("--")
        .args(command);
        cmd
    }

    pub fn run(&self, step_id: &str, extra: &[&str], command: &[&str]) -> Output {
        self.run_cmd(step_id, extra, command).output().unwrap()
    }

    pub fn report(&self) -> Output {
        self.amplihack()
            .args(["orch", "helper", "step-recovery", "report", "--dir"])
            .arg(&self.dir)
            .output()
            .unwrap()
    }

    /// Every record in `<dir>/<file>.jsonl`, each checked by [`assert_valid_record`].
    pub fn records(&self, file: &str) -> Vec<Value> {
        let path = self.dir.join(format!("{file}.jsonl"));
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("no record file {} ({e})", path.display()));
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} must be 0600", path.display());
        text.lines()
            .map(|line| {
                let record: Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("record is not JSON ({e}): {line}"));
                assert_valid_record(&record);
                record
            })
            .collect()
    }

    /// Names of the files in the recovery directory.
    pub fn dir_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Everything written under the recovery directory and the agent's
    /// copies of its evidence, for secret-leak checks.
    pub fn all_written_text(&self) -> String {
        let mut text = String::new();
        for dir in [&self.dir, &self.agent] {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_file() {
                    text.push_str(&String::from_utf8_lossy(&fs::read(&path).unwrap()));
                }
            }
        }
        text
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn bundle_recipes() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes")
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn show(o: &Output) -> String {
    format!(
        "status: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        o.status.code(),
        text(&o.stdout),
        text(&o.stderr)
    )
}
