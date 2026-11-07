//! Level 1 and level 2 through `multitask`: a workstream runs on the CLI of
//! the session that started it.
//!
//! Each test goes `amplihack multitask run` (or `orch run`, the route
//! smart-orchestrator's parallel step takes) → `bash run.sh` in a fresh clone
//! → `amplihack <cli>` or `amplihack recipe run` → fake CLI. The workstream
//! scripts call `amplihack` by name, so these tests put the binary under test
//! on `PATH` ([`Fixture::workstream_cmd`]).

use super::fixture::{Cli, Fixture};
use super::script;
use super::steps::real_runner;
use crate::support::amplihack_bin::amplihack_bin;
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

impl Fixture {
    /// The command for a `multitask` or `orch run` test: `amplihack` on
    /// `PATH` (the workstream scripts call it by name), a `TMPDIR` inside the
    /// fixture (multitask keeps its workstreams under it), and a committed git
    /// repository at `source` for the workstreams to clone.
    fn workstream_cmd<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let link = self.tools().join("amplihack");
        if !link.exists() {
            std::os::unix::fs::symlink(amplihack_bin(), &link).unwrap();
        }
        let source = self.source_repo();
        fs::create_dir_all(self.path("tmp")).unwrap();
        let mut c = self.cmd(args);
        c.env("TMPDIR", self.path("tmp"))
            .env("AMPLIHACK_SESSION_TREE_DIR", self.path("trees"))
            .env("AMPLIHACK_REPO_PATH", &source)
            .current_dir(&source);
        c
    }

    /// A git repository holding the probe recipe and the level 2 recipes,
    /// committed on `main`, created on first use.
    fn source_repo(&self) -> PathBuf {
        let dir = self.path("source");
        if dir.join(".git").exists() {
            return dir;
        }
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("probe.yaml"), script::PROBE_RECIPE).unwrap();
        for (name, text) in script::NESTED_RECIPES {
            fs::write(dir.join(name), text).unwrap();
        }
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["add", "-A"][..],
            &[
                "-c",
                "user.name=issue-1983",
                "-c",
                "user.email=issue-1983@example.invalid",
                "commit",
                "-q",
                "-m",
                "fixture",
            ][..],
        ] {
            let out = amplihack_git::command_in(&dir)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::null())
                .output()
                .expect("run git");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        }
        dir
    }

    /// Write a one-workstream `multitask` config whose workstream runs
    /// `recipe`, and return its path.
    fn workstream_config(&self, recipe: &str) -> PathBuf {
        let file = self.path("ws.json");
        let config = serde_json::json!([{
            "issue": 1983,
            "branch": "issue-1983-ws",
            "description": "issue 1983 workstream",
            "task": "issue 1983 workstream task",
            "recipe": recipe,
            "max_runtime": 110,
        }]);
        fs::write(&file, config.to_string()).unwrap();
        file
    }
}

/// Every record of the run: `cli` ran each time, and no record is missing or
/// extra.
fn assert_only(fx: &Fixture, cli: Cli, count: usize) {
    let records = fx.records();
    assert_eq!(records.len(), count, "{}", fx.logs());
    for record in &records {
        record.assert_cli(cli);
    }
}

/// Classic mode starts `amplihack <delegate>` directly. The delegate used to
/// default to `amplihack claude` whatever the session, and `amplihack codex`
/// was not a valid `AMPLIHACK_DELEGATE`.
fn classic_workstream_stays(cli: Cli) {
    let fx = Fixture::new();
    let config = fx.workstream_config("default-workflow");
    let mut cmd = fx.workstream_cmd(["multitask", "run"]);
    cmd.arg(&config).args(["--mode", "classic"]);
    fx.session(&mut cmd, cli);
    fx.run_ok(cmd);

    assert_only(&fx, cli, 1);
    // The launcher puts its own flags first; the prompt form comes right
    // before the prompt.
    let argv = fx.records()[0].get("argv").to_string();
    let verb = if cli == Cli::Codex { "exec" } else { "-p" };
    assert!(
        argv.contains(&format!(" {verb} @TASK.md "))
            || argv.starts_with(&format!("{verb} @TASK.md ")),
        "{} must get its own prompt form, got argv {argv:?}\n{}",
        cli.name(),
        fx.logs()
    );
}

#[test]
fn classic_workstream_from_a_codex_session_runs_codex() {
    classic_workstream_stays(Cli::Codex);
}

#[test]
fn classic_workstream_from_a_claude_session_runs_claude() {
    classic_workstream_stays(Cli::Claude);
}

/// `AMPLIHACK_DELEGATE='amplihack codex'` is an explicit choice, honoured even
/// inside a claude session.
#[test]
fn explicit_codex_delegate_is_honoured() {
    let fx = Fixture::new();
    let config = fx.workstream_config("default-workflow");
    let mut cmd = fx.workstream_cmd(["multitask", "run"]);
    cmd.arg(&config).args(["--mode", "classic"]);
    fx.session(&mut cmd, Cli::Claude);
    cmd.env("AMPLIHACK_DELEGATE", "amplihack codex");
    fx.run_ok(cmd);

    assert_only(&fx, Cli::Codex, 1);
}

/// What smart-orchestrator does to launch parallel workstreams:
/// `smart-classify-route.yaml` registers the session tree, and
/// `smart-execute-routing.yaml`'s `launch-parallel-round-1` bash step runs
/// `amplihack orch run` with its `tree_id` and `depth`. Without a registered
/// tree the workstream's nested `recipe run` refuses to start (issue #1326).
/// `WS_CONFIG` names the workstream config.
const LAUNCH_PARALLEL: &str = r#"set -e
info=$("$AMPLIHACK_UNDER_TEST" session-tree register issue1983-root --json)
field() { printf '%s' "$info" | "$AMPLIHACK_UNDER_TEST" orch helper extract-json | "$AMPLIHACK_UNDER_TEST" orch helper extract-field --field "$1" --default "$2"; }
tree=$(field tree_id '')
depth=$(field depth 0)
AMPLIHACK_TREE_ID="$tree" AMPLIHACK_SESSION_DEPTH="$depth" "$AMPLIHACK_UNDER_TEST" orch run -- "$WS_CONFIG"
"#;

/// An agent in a `cli` session launches parallel workstreams the way
/// smart-orchestrator does: test → `amplihack <cli>` → fake CLI →
/// [`LAUNCH_PARALLEL`] → `orch run` → multitask (recipe mode) → `bash run.sh`
/// → `launcher.sh` → `amplihack recipe run` → stub runner → three agent
/// steps → `amplihack <cli>` → fake CLI.
fn orch_run_workstream_stays(cli: Cli) {
    let fx = Fixture::new();
    let config = fx.workstream_config("probe.yaml");
    let mut cmd = match cli {
        Cli::Codex => fx.workstream_cmd(["codex", "--", "exec", "-"]),
        Cli::Claude => fx.workstream_cmd(["claude", "--", "-p", "go"]),
    };
    fx.session(&mut cmd, cli);
    cmd.env("WS_CONFIG", &config)
        .env("FAKE_THEN", LAUNCH_PARALLEL);
    fx.run_ok(cmd);

    assert_eq!(
        fx.runner_env(),
        (cli.name().to_string(), format!("session:{}", cli.name())),
        "{}",
        fx.logs()
    );
    // The top fake plus the stub runner's three agent steps, and nothing else.
    assert_eq!(fx.step_records().len(), 3, "{}", fx.logs());
    assert_only(&fx, cli, 4);
}

#[test]
fn orch_run_workstream_from_a_codex_session_runs_codex() {
    orch_run_workstream_stays(Cli::Codex);
}

#[test]
fn orch_run_workstream_from_a_claude_session_runs_claude() {
    orch_run_workstream_stays(Cli::Claude);
}

/// The whole parallel route of smart-orchestrator, under the real runner:
/// `recipe run` → runner → a bash step running [`LAUNCH_PARALLEL`] →
/// `orch run` → multitask → `bash run.sh` → `launcher.sh` →
/// `amplihack recipe run` → runner → the workstream recipe's agent step →
/// `amplihack <cli>` → fake CLI.
fn parallel_bash_step_stays(cli: Cli) {
    let Some(runner) = real_runner() else { return };
    let fx = Fixture::new();
    let config = fx.workstream_config("issue1983-subsub.yaml");
    let recipe = fx.path("issue1983-parallel.yaml");
    // The runner gives a bash step a private `TMPDIR`; the step puts the
    // fixture's back, so multitask keeps the workstream inside the fixture.
    let step = format!("export TMPDIR=\"$WS_TMPDIR\"\n{LAUNCH_PARALLEL}");
    let indented: String = step.lines().map(|line| format!("      {line}\n")).collect();
    std::fs::write(
        &recipe,
        format!(
            "name: issue1983-parallel\nversion: 1.0.0\nsteps:\n  - id: launch-parallel\n    type: bash\n    command: |\n{indented}"
        ),
    )
    .unwrap();
    let mut cmd = fx.workstream_cmd(["recipe", "run"]);
    cmd.arg(&recipe);
    fx.session(&mut cmd, cli);
    cmd.env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("WS_CONFIG", &config)
        .env("WS_TMPDIR", fx.path("tmp"))
        // The bash step already runs at depth 2, and the workstream's
        // `recipe run` one deeper; the default ceiling of 3 refuses it
        // (smart-orchestrator's `recursion_guard` then takes the single-session
        // route instead). The root seals a higher ceiling.
        .env("AMPLIHACK_MAX_DEPTH", "6");
    fx.run_ok(cmd);

    let records = fx.records();
    assert_eq!(
        records.len(),
        1,
        "the workstream's one agent step\n{}",
        fx.logs()
    );
    assert_eq!(
        records[0].get("prompt_tag"),
        "issue1983-level-subsub",
        "{}",
        fx.logs()
    );
    records[0].assert_cli(cli);
}

#[test]
fn parallel_bash_step_from_a_codex_session_runs_codex() {
    parallel_bash_step_stays(Cli::Codex);
}

#[test]
fn parallel_bash_step_from_a_claude_session_runs_claude() {
    parallel_bash_step_stays(Cli::Claude);
}
