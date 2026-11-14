//! Issue #1976 (decision 17.113): the caller context key `should_publish` is
//! removed. No recipe declares it, and a caller who still passes it gets an
//! error and exit 1 before any work, not a silent no-op.
//!
//! T1 (AC1) parses every bundled recipe. T2 (AC3) runs the real `amplihack`
//! binary with a temp `HOME`, a temp project, and fake `git`, `gh` and `tmux`
//! first on `PATH`, each appending its argv to a call log. A refused run must
//! leave every call log empty, start no runner and create no directory.
//!
//! Hermetic: `env_clear()`, a temp `HOME`; nothing under the developer's
//! `~/.claude` is read or written. No sleeps: every wait is the child's exit.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::wait::output_within;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn recipes_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle").join("recipes")
}

// -------------------------------------------------------------------------
// T1 (AC1): no recipe declares `should_publish` in its top-level context.
// -------------------------------------------------------------------------

#[test]
fn no_bundled_recipe_declares_should_publish_in_its_context() {
    let mut parsed = BTreeSet::new();
    let mut offenders = Vec::new();
    for entry in fs::read_dir(recipes_dir()).expect("read recipes dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if let Some(context) = doc.get("context").and_then(serde_yaml::Value::as_mapping)
            && context.contains_key(serde_yaml::Value::from("should_publish"))
        {
            offenders.push(name.clone());
        }
        parsed.insert(name);
    }
    for required in ["default-workflow", "workflow-publish", "workflow-finalize"] {
        assert!(parsed.contains(required), "{required}.yaml was not parsed");
    }
    assert!(
        offenders.is_empty(),
        "recipes still declaring the removed `should_publish` context key: {offenders:?}"
    );
}

#[test]
fn no_recipe_reads_the_removed_caller_key() {
    for entry in fs::read_dir(recipes_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for needle in ["{{should_publish}}", "RECIPE_VAR_should_publish"] {
            assert!(
                !text.contains(needle),
                "{} references the removed caller key via {needle}",
                path.display()
            );
        }
    }
}

/// R2: the internal `terminal_state.should_publish` stays: terminal-state
/// still emits it and workflow-publish still gates on it.
#[test]
fn the_internal_terminal_state_should_publish_is_kept() {
    let terminal = fs::read_to_string(recipes_dir().join("workflow-terminal-state.yaml")).unwrap();
    assert!(
        terminal.contains("should_publish"),
        "workflow-terminal-state.yaml must still emit should_publish"
    );
    let publish = fs::read_to_string(recipes_dir().join("workflow-publish.yaml")).unwrap();
    assert!(
        publish.matches("terminal_state.should_publish").count() >= 7,
        "workflow-publish.yaml must keep every terminal_state.should_publish gate"
    );
}

// -------------------------------------------------------------------------
// T2 (AC3): the CLI refuses the key from every source, before any work.
// -------------------------------------------------------------------------

/// Appends its argv to `$CALL_LOG_DIR/<name>.log`, then succeeds.
fn logging_stub(name: &str) -> String {
    format!(
        "#!/bin/sh\ncase \"$*\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\nprintf '%s\\n' \"$*\" >> \"$CALL_LOG_DIR/{name}.log\"\nexit 0\n"
    )
}

/// Records that the runner started, then reports success.
const STUB_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
printf '%s\n' "$*" >> "$CALL_LOG_DIR/runner.log"
printf '%s\n' '{"recipe_name":"issue-1976","success":true,"step_results":[],"context":{}}'
"#;

const LOCAL_RECIPE: &str = r#"name: issue-1976-local
description: no-op
version: 1.0.0
context: {}
steps:
  - id: noop
    type: bash
    command: "true"
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
    tools: PathBuf,
    logs: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let fx = Self {
            home: root.join("home"),
            project: root.join("project"),
            tools: root.join("tools"),
            logs: root.join("logs"),
            root,
            _tmp: tmp,
        };
        // `.amplihack` exists up front: startup may write a cache file in it,
        // which is not a run's work. Only new directories count.
        for d in [
            &fx.home.join(".amplihack"),
            &fx.project,
            &fx.tools,
            &fx.logs,
        ] {
            fs::create_dir_all(d).unwrap();
        }
        for name in ["git", "gh", "tmux"] {
            crate::support::exec_stub::write_executable(
                &fx.tools.join(name),
                logging_stub(name),
                0o755,
            );
        }
        crate::support::exec_stub::write_executable(
            &fx.tools.join("recipe-runner-rs"),
            STUB_RUNNER,
            0o755,
        );
        fs::write(fx.root.join("local.yaml"), LOCAL_RECIPE).unwrap();
        // Issue #2528: the checkout is skipped as an installed candidate, so
        // `recipe run` needs a bundle under $HOME.
        crate::support::installed_bundle::ensure(&fx.home.join(".amplihack"));
        fx
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .args(args)
            .arg("-w")
            .arg(&self.project)
            .current_dir(&self.project)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("CALL_LOG_DIR", &self.logs)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            // Bundled recipes resolve by name from this checkout.
            .env("AMPLIHACK_HOME", workspace_root())
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .stdin(Stdio::null());
        output_within("amplihack recipe run to exit", &mut c)
    }

    fn dirs(&self) -> BTreeSet<PathBuf> {
        let mut out = BTreeSet::new();
        for top in [&self.home, &self.project] {
            walk_dirs(top, &mut out);
        }
        out
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.logs.join(format!("{name}.log"))).unwrap_or_default()
    }

    /// Runs `args`, which pass the removed key, and asserts the refusal.
    fn assert_refused(&self, why: &str, args: &[&str]) -> String {
        let before = self.dirs();
        let out = self.run(args);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(1), "{why}: exit\nstderr:\n{stderr}");
        assert!(
            stderr.contains("context key 'should_publish' was removed by #1976"),
            "{why}: stderr must name the removed key\n{stderr}"
        );
        assert!(stderr.contains("removed"), "{why}: {stderr}");
        for name in ["git", "gh", "tmux", "runner"] {
            assert!(
                self.log(name).is_empty(),
                "{why}: {name} was called before the refusal:\n{}",
                self.log(name)
            );
        }
        let created: Vec<_> = self.dirs().difference(&before).cloned().collect();
        assert!(
            created.is_empty(),
            "{why}: a refused run created directories: {created:?}"
        );
        stderr
    }
}

fn walk_dirs(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            out.insert(path.clone());
            walk_dirs(&path, out);
        }
    }
}

#[test]
fn bundled_recipes_refuse_should_publish_from_c_for_every_value() {
    let fx = Fx::new();
    for recipe in ["default-workflow", "smart-orchestrator"] {
        for value in ["false", "true"] {
            let pair = format!("should_publish={value}");
            fx.assert_refused(
                &format!("{recipe} -c {pair}"),
                &[
                    recipe,
                    "-c",
                    "task_description=issue 1976 probe",
                    "-c",
                    &pair,
                ],
            );
        }
    }
}

#[test]
fn a_context_file_with_should_publish_is_refused() {
    let fx = Fx::new();
    let file = fx.root.join("ctx.json");
    fs::write(
        &file,
        r#"{"task_description":"issue 1976 probe","should_publish":"false"}"#,
    )
    .unwrap();
    fx.assert_refused(
        "--context-file",
        &["default-workflow", "--context-file", file.to_str().unwrap()],
    );
}

#[test]
fn a_detached_run_is_refused_before_the_pane_opens() {
    let fx = Fx::new();
    fx.assert_refused(
        "--detach",
        &[
            "smart-orchestrator",
            "--detach",
            "-c",
            "task_description=issue 1976 probe",
            "-c",
            "should_publish=false",
        ],
    );
}

#[test]
fn the_refusal_never_echoes_the_value() {
    let fx = Fx::new();
    let stderr = fx.assert_refused(
        "secret value",
        &[
            "default-workflow",
            "-c",
            "task_description=issue 1976 probe",
            "-c",
            "should_publish=SECRET1976VALUE",
        ],
    );
    assert!(!stderr.contains("SECRET1976VALUE"), "{stderr}");
}

#[test]
fn a_key_that_only_starts_with_should_publish_is_not_refused() {
    let fx = Fx::new();
    let recipe = fx.root.join("local.yaml");
    let out = fx.run(&[
        recipe.to_str().unwrap(),
        "-c",
        "should_publish_extra=false",
        "-c",
        "Should_Publish=false",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("was removed by #1976"),
        "only the exact key is removed:\n{stderr}"
    );
    assert!(
        !fx.log("runner").is_empty(),
        "the run must reach the runner\nstderr:\n{stderr}"
    );
}
