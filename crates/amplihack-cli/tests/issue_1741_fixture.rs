//! Real-runner fixtures for the #1741 red-phase contracts.
//!
//! Recovery YAML and result fields follow docs/reference/recipe-recovery.md
//! and recipe-result.md's proposed v1 contract. They intentionally require
//! upstream support: an older runner must fail, never skip these scenarios.
//! The step tool simulates an external service; it does not implement replay.
//! Its controls and execution trace live outside the observed Git repository.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub struct Fixture {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub work_root: PathBuf,
    pub recipe: PathBuf,
    pub runner: PathBuf,
    tool: PathBuf,
}

pub struct Attempt {
    pub output: Output,
    pub run_dirs: Vec<PathBuf>,
    capture_dir: PathBuf,
}

pub fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

impl Attempt {
    pub fn runner_result(&self) -> Value {
        // The CLI intentionally omits semantic context from its display.
        // Inspect the real runner's captured JSON instead of requiring the
        // CLI to expose private context as part of the recovery feature.
        let run_id = self.dir().file_name().unwrap().to_str().unwrap();
        let bytes = fs::read(self.capture_dir.join(format!("{run_id}.json")))
            .expect("real runner result captured by the transparent transport");
        serde_json::from_slice(&bytes).expect("real runner JSON")
    }

    pub fn result(&self) -> Value {
        serde_json::from_slice(&self.output.stdout)
            .unwrap_or_else(|e| panic!("expected recipe JSON: {e}\n{}", describe(&self.output)))
    }

    pub fn recovery(&self) -> Value {
        let result = self.result();
        assert!(
            result["recovery"].is_object(),
            "missing recovery decision: {result:#}"
        );
        result["recovery"].clone()
    }

    pub fn dir(&self) -> &Path {
        // Delegates can own disposable runtime scopes, but the root recipe
        // has exactly one durable launch directory.
        let roots: Vec<_> = self
            .run_dirs
            .iter()
            .filter(|p| {
                let Ok(bytes) = fs::read(p.join("run.json")) else {
                    return false;
                };
                let Ok(report) = serde_json::from_slice::<Value>(&bytes) else {
                    return false;
                };
                report["recipe"] == "smart-orchestrator"
            })
            .collect();
        assert_eq!(
            roots.len(),
            1,
            "one root scope per executing launch: {:?}",
            self.run_dirs
        );
        roots[0]
    }

    pub fn assert_fresh(&self, reason: &str) {
        let recovery = self.recovery();
        assert_eq!(recovery["mode"], "fresh", "{recovery:#}");
        assert_eq!(recovery["reason"], reason, "{recovery:#}");
        assert_eq!(recovery["restored_steps"], json!([]), "{recovery:#}");
        assert!(recovery["source_run_id"].is_null(), "{recovery:#}");
    }

    pub fn assert_resumed(&self, source: &Attempt, restart: &str) -> Value {
        let recovery = self.recovery();
        assert_eq!(recovery["mode"], "resumed", "{recovery:#}");
        assert_eq!(
            recovery["source_run_id"].as_str(),
            source.dir().file_name().unwrap().to_str(),
            "the immediate predecessor supplies recovery"
        );
        assert_eq!(recovery["restart"]["step_id"], restart, "{recovery:#}");
        assert!(
            recovery["source_sequence"].as_u64().is_some(),
            "{recovery:#}"
        );
        let log = String::from_utf8_lossy(&self.output.stderr);
        assert!(
            log.contains(recovery["source_run_id"].as_str().unwrap()),
            "{log}"
        );
        assert!(log.contains(restart), "restart step absent from log: {log}");
        recovery
    }
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let runner = std::env::var_os("RECIPE_RUNNER_RS_PATH")
            .or_else(|| std::env::var_os("RECIPE_RUNNER_QA_BINARY"))
            .map(PathBuf::from)
            .or_else(|| {
                std::env::split_paths(&std::env::var_os("PATH")?)
                    .map(|p| p.join("recipe-runner-rs"))
                    .find(|p| p.is_file())
            })
            .expect("install the pinned real runner or set RECIPE_RUNNER_RS_PATH; no stub/skip");
        let mut f = Self {
            repo: root.join("repo"),
            home: root.join("home"),
            work_root: root.join("state"),
            recipe: root.join("smart-orchestrator.yaml"),
            tool: root.join("step-tool"),
            runner: runner.canonicalize().expect("real runner path"),
            root,
            _tmp: tmp,
        };
        for dir in [&f.repo, &f.home, &f.work_root] {
            fs::create_dir(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let captures = f.root.join("runner-results");
        fs::create_dir(&captures).unwrap();
        fs::set_permissions(&captures, fs::Permissions::from_mode(0o700)).unwrap();
        let capture = f.root.join("capture-runner");
        crate::support::exec_stub::write_executable(
            &capture,
            format!(
                r#"#!/bin/sh
export RECIPE_RUNNER_NO_UPDATE_CHECK=1
case "$1" in
  --version|--capabilities) exec {} "$@" ;;
esac
umask 077
result={}/"${{AMPLIHACK_RUN_ID:?}}.json"
{} "$@" > "$result"
status=$?
cat "$result"
exit "$status"
"#,
                quote(&f.runner),
                quote(&captures),
                quote(&f.runner)
            ),
            0o700,
        );
        f.runner = capture;
        f.git(&["init", "--initial-branch=main"]);
        f.git(&["config", "user.name", "Recovery Test"]);
        f.git(&["config", "user.email", "recovery@example.invalid"]);
        f.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/example/recovery.git",
        ]);
        fs::write(f.repo.join("input.txt"), "initial\n").unwrap();
        f.git(&["add", "input.txt"]);
        f.git(&["commit", "-m", "fixture"]);
        let script = format!(
            r#"#!/bin/sh
set -eu
fixture_root={}
step=$1
printf '%s\n' "$step" >> "$fixture_root/events"
mode=success
if [ -f "$fixture_root/mode.$step" ]; then mode=$(cat "$fixture_root/mode.$step"); fi
case "$mode" in
  limit) printf '%s\n' "You've hit your session limit · resets 8:30am (America/Los_Angeles)" >&2; exit 1 ;;
  error) printf '%s\n' 'invalid task input' >&2; exit 1 ;;
esac
printf '%s\n' '{{"label":"saved-output","enabled":true,"count":7,"empty":null,"items":[1,"two"],"nested":{{"ok":false}}}}'
"#,
            quote(&f.root)
        );
        crate::support::exec_stub::write_executable(&f.tool, script, 0o700);
        f.write_recipe(&f.recipe, &f.workflow());
        f
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = amplihack_git::command()
            .args(args)
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", describe(&out));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    pub fn mode(&self, step: &str, mode: &str) {
        fs::write(self.root.join(format!("mode.{step}")), mode).unwrap();
    }

    pub fn step(&self, id: &str) -> Value {
        json!({
            "id": id, "type": "bash", "auto_stage": false,
            "command": format!("{} {id}", quote(&self.tool)),
            "parse_json": true, "parse_json_required": true, "output": id,
            "recovery": {"replay": "reuse"}
        })
    }

    pub fn document(&self, name: &str, steps: Vec<Value>) -> Value {
        let outputs: Vec<_> = steps.iter().map(|s| s["id"].as_str().unwrap()).collect();
        let mut context = vec![
            "issue_number",
            "issue_url",
            "repo_path",
            "task_description",
            "input",
        ];
        context.extend(steps.iter().filter_map(|s| s["output"].as_str()));
        json!({
            "name": name, "description": "Issue 1741 behavior fixture", "version": "1.0.0",
            "context": {},
            "recovery": {"version": 1, "persist": {"context": context, "outputs": outputs}},
            "steps": steps
        })
    }

    pub fn workflow(&self) -> Value {
        self.document(
            "smart-orchestrator",
            vec![
                self.step("classify"),
                self.step("design"),
                self.step("implement"),
            ],
        )
    }

    pub fn write_recipe(&self, path: &Path, recipe: &Value) {
        fs::write(path, serde_yaml::to_string(recipe).unwrap()).unwrap();
    }

    pub fn command(&self, recipe: &Path, repo: &Path, issue: &str, extra: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .arg(recipe)
            .args(["--no-detach", "--format", "json", "-w"])
            .arg(repo)
            .args([
                "-c",
                &format!("issue_number={issue}"),
                "-c",
                "task_description=recover issue",
            ])
            .arg("-c")
            .arg(format!("repo_path={}", repo.display()))
            .args(extra)
            .current_dir(repo)
            .env("HOME", &self.home)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env(
                "AMPLIHACK_HOME",
                crate::support::installed_bundle::home(self.home.join(".amplihack")),
            )
            .env("AMPLIHACK_WORK_ROOT", &self.work_root)
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0");
        c
    }

    pub fn run(&self, extra: &[&str]) -> Attempt {
        self.run_at(&self.recipe, &self.repo, "1741", extra)
    }

    pub fn run_at(&self, recipe: &Path, repo: &Path, issue: &str, extra: &[&str]) -> Attempt {
        let before = self.run_dirs();
        let output = self
            .command(recipe, repo, issue, extra)
            .output()
            .expect("execute amplihack");
        let run_dirs = self.run_dirs().difference(&before).cloned().collect();
        Attempt {
            output,
            run_dirs,
            capture_dir: self.root.join("runner-results"),
        }
    }

    pub fn run_dirs(&self) -> BTreeSet<PathBuf> {
        match fs::read_dir(self.work_root.join("runs")) {
            Ok(entries) => entries
                .map(|e| e.unwrap().path())
                .filter(|p| p.is_dir())
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(e) => panic!("list runs: {e}"),
        }
    }

    pub fn events(&self) -> Vec<String> {
        match fs::read_to_string(self.root.join("events")) {
            Ok(s) => s.lines().map(str::to_owned).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => panic!("read execution trace: {e}"),
        }
    }

    pub fn stop_at(&self, step: &str) -> Attempt {
        self.mode(step, "limit");
        let attempt = self.run(&[]);
        assert!(
            !attempt.output.status.success(),
            "{}",
            describe(&attempt.output)
        );
        assert!(
            String::from_utf8_lossy(&attempt.output.stderr).contains("usage_limit"),
            "fixture must reach a recognized usage limit: {}",
            describe(&attempt.output)
        );
        attempt
    }
}

/// Walk only ordinary files and directories. Tests use this to damage real
/// emitted history without assuming private record filenames or fabricating it.
pub fn files(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    for entry in fs::read_dir(root).unwrap_or_else(|e| panic!("{}: {e}", root.display())) {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            found.extend(files(&entry.path()));
        } else if kind.is_file() {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}
