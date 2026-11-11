//! Issue #1883: every agent step started by `amplihack recipe run` inherits
//! the parent session's effort and model, unless the step names its own.
//!
//! The real `amplihack` binary runs `recipe run` with a stub recipe runner
//! (through `RECIPE_RUNNER_RS_PATH`, as in `issue_1801_evaluator_retry`). The
//! stub runs two steps, each as
//! `$AMPLIHACK_UNDER_TEST <binary> --subprocess-safe -- -p "step N" [args]`,
//! the way `recipe-runner-rs` starts an agent step, and saves each step's
//! stderr. The fake agent records its argv, the NAMES of its environment
//! variables, and the values of only `CLAUDE_CODE_EFFORT_LEVEL`,
//! `ANTHROPIC_MODEL`, `AMPLIHACK_PARENT_*` and `HOME`. No secret value is
//! ever written.
//!
//! Isolation (#1699): `isolated(..)` clears the environment; HOME is a temp
//! dir; `AMPLIHACK_CLAUDE_PROFILE_ROOT` is never set; no rotator or tmux server
//! starts. `/proc` is a synthetic tree named through `AMPLIHACK_TEST_PROC_ROOT`,
//! so the developer's own `claude` session, which is usually an ancestor of
//! `cargo test`, is never read.
//!
//! Cases:
//! - (a) effort high and model X inherited: every step records both, on argv
//!   and in its environment.
//! - (b) a step with `--model Y` records Y, still `--effort high`, and
//!   `ANTHROPIC_MODEL=Y`.
//! - (c) nothing inherited: no `--effort`, no added variables, the default
//!   model argv and the #1421 notice unchanged.
//! - (d) copilot: every step receives the inherited effort as
//!   `--reasoning-effort`; an effort copilot does not list gives the
//!   "cannot be passed" line exactly once across the run and two steps.
//! - (e) children have no `AMPLIHACK_CLAUDE_PROFILE_ROOT` and HOME lies under
//!   the temp root.
//! - (f) Linux: a plain `claude` ancestor's argv is read through `/proc`; under
//!   an older amplihack session its model is not trusted (C2).
//! - (g) a Claude run with an inherited model whose second step starts
//!   `amplihack copilot`: that step receives the effort but no `--model`.
//! - (h) a Claude run with effort `max` whose second step starts
//!   `amplihack codex`: codex cannot take `max`, so that step prints the
//!   "cannot be passed" line for effort and for the model, once each.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The model amplihack passes when nothing else chooses one
/// (`command::DEFAULT_MODEL`).
const DEFAULT_MODEL: &str = "claude-opus-5[1m]";

const X: &str = "claude-opus-5-5";
const Y: &str = "claude-haiku-4-5";

/// The fake agent: answers `--version`; otherwise writes `call.<STEP>` and
/// exits 0. Values are recorded only for the allowlisted variables.
const FAKE_AGENT: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
{
  printf 'ARGV='
  for a in "$@"; do printf '%s|' "$a"; done
  printf '\n'
  printf 'NAMES='
  env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p' | sort -u | tr '\n' '|'
  printf '\n'
  printf 'CLAUDE_CODE_EFFORT_LEVEL=%s\n' "${CLAUDE_CODE_EFFORT_LEVEL-<unset>}"
  printf 'ANTHROPIC_MODEL=%s\n' "${ANTHROPIC_MODEL-<unset>}"
  printf 'AMPLIHACK_PARENT_EFFORT=%s\n' "${AMPLIHACK_PARENT_EFFORT-<unset>}"
  printf 'AMPLIHACK_PARENT_MODEL=%s\n' "${AMPLIHACK_PARENT_MODEL-<unset>}"
  printf 'AMPLIHACK_PARENT_MODEL_BINARY=%s\n' "${AMPLIHACK_PARENT_MODEL_BINARY-<unset>}"
  printf 'HOME=%s\n' "$HOME"
} > "$RECORD_DIR/call.$step"
if [ "${AMPLIHACK_TEST_NESTED_ONCE-}" = "$step" ]; then
  unset AMPLIHACK_TEST_NESTED_ONCE
  export RECORD_DIR="$RECORD_DIR/nested"
  exec "$AMPLIHACK_UNDER_TEST" recipe run "$AMPLIHACK_TEST_RECIPE" -w "$AMPLIHACK_TEST_WORK"
fi
echo "step $step done"
exit 0
"#;

/// The stub runner: two agent steps with the run's agent binary. Step 2 adds
/// `$STEP2_ARGS` (word-split on purpose: test values have no spaces), and
/// starts `$STEP2_BIN` instead when it is set, as a bash step running
/// `amplihack copilot -p …` does in quality-loop.yaml.
const STUB_RUNNER: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'recipe-runner-rs 0.0.0 (stub)'; exit 0 ;;
  --capabilities) printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec"]}'; exit 0 ;;
esac
export AMPLIHACK_NONINTERACTIVE=1
bin="${AMPLIHACK_AGENT_BINARY:-claude}"
STEP=1 "$AMPLIHACK_UNDER_TEST" "$bin" --subprocess-safe -- -p "step 1" \
  </dev/null >"$RECORD_DIR/step.1.out" 2>"$RECORD_DIR/step.1.err" || exit 1
STEP=2 "$AMPLIHACK_UNDER_TEST" "${STEP2_BIN:-$bin}" --subprocess-safe -- -p "step 2" $STEP2_ARGS \
  </dev/null >"$RECORD_DIR/step.2.out" 2>"$RECORD_DIR/step.2.err" || exit 1
printf '%s\n' '{"recipe_name":"inherit-1883","success":true,"step_results":[],"context":{}}'
"#;

const RECIPE: &str = r#"name: inherit-1883
description: Two agent steps for the issue 1883 inheritance test
version: 1.0.0
context: {}
steps:
  - id: step-01
    type: bash
    command: "true"
  - id: step-02
    type: bash
    command: "true"
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    records: PathBuf,
    work: PathBuf,
    tools: PathBuf,
    proc_root: PathBuf,
}

fn executable(path: &Path, body: &str) {
    crate::support::exec_stub::write_executable(path, body, 0o755);
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

/// One entry of the synthetic `/proc`, in the format `procfs::snapshot`
/// reads.
fn add_proc(root: &Path, pid: u32, ppid: u32, comm: &str, argv: &[&str]) {
    let d = root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("stat"),
        format!(
            "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 5 1000 100\n"
        ),
    )
    .unwrap();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    fs::write(
        d.join("status"),
        format!("Name:\t{comm}\nState:\tS (x)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
    )
    .unwrap();
    let cmdline: String = argv.iter().map(|a| format!("{a}\0")).collect();
    fs::write(d.join("cmdline"), cmdline).unwrap();
    fs::write(d.join("environ"), "PATH=/usr/bin\0").unwrap();
}

impl Fixture {
    /// `ancestor` is the argv of the process above `amplihack recipe run` in
    /// the synthetic `/proc`; empty means a shell, so no claude ancestor.
    fn new(ancestor: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let records = root.join("records");
        let work = root.join("work");
        let tools = root.join("tools");
        let proc_root = root.join("proc");
        for d in [&home, &records, &work, &tools, &proc_root] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(home.join(".claude")).unwrap();
        for name in ["claude", "copilot", "codex"] {
            executable(&tools.join(name), FAKE_AGENT);
        }
        executable(&tools.join("recipe-runner-rs"), STUB_RUNNER);
        fs::write(root.join("inherit-1883.yaml"), RECIPE).unwrap();
        add_proc(&proc_root, 10, 1, "bash", &["bash"]);
        if ancestor.is_empty() {
            add_proc(&proc_root, 50, 10, "bash", &["bash"]);
        } else {
            add_proc(&proc_root, 50, 10, "claude", ancestor);
        }
        add_proc(
            &proc_root,
            100,
            50,
            "amplihack",
            &["amplihack", "recipe", "run"],
        );
        std::os::unix::fs::symlink("100", proc_root.join("self")).unwrap();
        Self {
            _tmp: tmp,
            root,
            home,
            records,
            work,
            tools,
            proc_root,
        }
    }

    /// `amplihack recipe run <extra>` with the stub runner, isolated, with
    /// only the fixture's environment plus `env`.
    fn command(&self, agent: &str, env: &[(&str, &str)]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("TMPDIR", std::env::temp_dir())
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("AMPLIHACK_AGENT_BINARY", agent)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_COPILOT_BINARY_PATH", self.tools.join("copilot"))
            .env("AMPLIHACK_CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env("AMPLIHACK_TEST_RECIPE", self.root.join("inherit-1883.yaml"))
            .env("AMPLIHACK_TEST_WORK", &self.work)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_TEST_PROC_ROOT", &self.proc_root)
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
            .env_remove("CLAUDE_CONFIG_DIR");
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    fn run_with(&self, agent: &str, extra: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = self.command(agent, env);
        c.arg("recipe")
            .arg("run")
            .arg(self.root.join("inherit-1883.yaml"))
            .arg("-w")
            .arg(&self.work)
            .args(extra);
        let mut tries = 0;
        loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => return r.expect("run amplihack recipe run"),
            }
        }
    }

    fn run(&self, agent: &str, env: &[(&str, &str)]) -> Output {
        self.run_with(agent, &[], env)
    }

    /// The fake agent's record for `step`.
    fn call(&self, step: u32) -> BTreeMap<String, String> {
        let raw = fs::read_to_string(self.records.join(format!("call.{step}")))
            .unwrap_or_else(|e| panic!("step {step} did not reach the agent: {e}"));
        raw.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn step_stderr(&self, step: u32) -> String {
        fs::read_to_string(self.records.join(format!("step.{step}.err"))).unwrap_or_default()
    }
}

fn argv(call: &BTreeMap<String, String>) -> Vec<String> {
    call["ARGV"]
        .split('|')
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

fn names(call: &BTreeMap<String, String>) -> Vec<String> {
    call["NAMES"]
        .split('|')
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

/// The value after the last `flag` in `args`, or the `flag=` spelling.
fn flag(args: &[String], name: &str) -> Option<String> {
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            out = args.get(i + 1).cloned();
            i += 1;
        } else if let Some(v) = args[i].strip_prefix(&format!("{name}=")) {
            out = Some(v.to_string());
        }
        i += 1;
    }
    out
}

fn count(args: &[String], name: &str) -> usize {
    args.iter()
        .filter(|a| *a == name || a.starts_with(&format!("{name}=")))
        .count()
}

const PARENT_HIGH_X: [(&str, &str); 2] = [
    ("AMPLIHACK_PARENT_EFFORT", "high"),
    ("AMPLIHACK_PARENT_MODEL", X),
];

/// (a) Every step records the inherited effort and model on argv and in its
/// environment, and passes them on for a nested run (C1).
#[test]
fn a_every_step_inherits_effort_and_model() {
    let f = Fixture::new(&[]);
    let out = f.run("claude", &PARENT_HIGH_X);
    assert!(out.status.success(), "{}", describe(&out));
    for step in [1, 2] {
        let call = f.call(step);
        let args = argv(&call);
        assert_eq!(
            flag(&args, "--model").as_deref(),
            Some(X),
            "step {step}: {args:?}"
        );
        assert_eq!(
            flag(&args, "--effort").as_deref(),
            Some("high"),
            "step {step}: {args:?}"
        );
        assert_eq!(count(&args, "--model"), 1, "step {step}: {args:?}");
        assert_eq!(count(&args, "--effort"), 1, "step {step}: {args:?}");
        assert_eq!(call["CLAUDE_CODE_EFFORT_LEVEL"], "high", "step {step}");
        assert_eq!(call["ANTHROPIC_MODEL"], X, "step {step}");
        assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "high", "step {step}");
        assert_eq!(call["AMPLIHACK_PARENT_MODEL"], X, "step {step}");
        // D8: under a recipe run a step prints no per-step model notice.
        assert!(
            !f.step_stderr(step).contains("passing `--model"),
            "step {step}: {}",
            f.step_stderr(step)
        );
    }
    let stderr = text(&out.stderr);
    let summary = format!(
        "amplihack: steps inherit effort=high model={X} (from the parent amplihack launch)"
    );
    assert_eq!(
        stderr.lines().filter(|l| *l == summary).count(),
        1,
        "{}",
        describe(&out)
    );
}

/// (b) A step's own `--model` wins; the inherited effort still applies, and
/// the child holds only the step's model.
#[test]
fn b_a_step_own_model_wins_and_effort_is_still_inherited() {
    let f = Fixture::new(&[]);
    let mut env = PARENT_HIGH_X.to_vec();
    env.push(("STEP2_ARGS", "--model claude-haiku-4-5"));
    let out = f.run("claude", &env);
    assert!(out.status.success(), "{}", describe(&out));
    let call = f.call(2);
    let args = argv(&call);
    assert_eq!(flag(&args, "--model").as_deref(), Some(Y), "{args:?}");
    assert_eq!(count(&args, "--model"), 1, "{args:?}");
    assert!(!args.iter().any(|a| a.contains(X)), "{args:?}");
    assert_eq!(flag(&args, "--effort").as_deref(), Some("high"), "{args:?}");
    assert_eq!(call["ANTHROPIC_MODEL"], Y);
    assert_eq!(call["CLAUDE_CODE_EFFORT_LEVEL"], "high");
    assert_eq!(call["AMPLIHACK_PARENT_MODEL"], Y);
    // Step 1 is unaffected by step 2's choice.
    assert_eq!(f.call(1)["ANTHROPIC_MODEL"], X);
}

/// (c) F3: with nothing to inherit, argv, environment and stderr are as
/// before #1883.
#[test]
fn c_nothing_inherited_changes_nothing() {
    let f = Fixture::new(&[]);
    let out = f.run("claude", &[]);
    assert!(out.status.success(), "{}", describe(&out));
    let notice = format!(
        "amplihack: passing `--model {DEFAULT_MODEL}` to `claude` (from amplihack's built-in \
         default). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an empty value to let \
         claude choose its own default model."
    );
    for step in [1, 2] {
        let call = f.call(step);
        let args = argv(&call);
        assert_eq!(
            flag(&args, "--model").as_deref(),
            Some(DEFAULT_MODEL),
            "{args:?}"
        );
        assert_eq!(count(&args, "--effort"), 0, "step {step}: {args:?}");
        let names = names(&call);
        for v in [
            "CLAUDE_CODE_EFFORT_LEVEL",
            "ANTHROPIC_MODEL",
            "AMPLIHACK_PARENT_EFFORT",
            "AMPLIHACK_PARENT_MODEL",
        ] {
            assert!(!names.contains(&v.to_string()), "step {step} has {v}");
        }
        assert_eq!(
            f.step_stderr(step).lines().filter(|l| *l == notice).count(),
            1,
            "step {step}: {}",
            f.step_stderr(step)
        );
    }
    let stderr = text(&out.stderr);
    assert!(
        !stderr.contains("steps inherit") && !stderr.contains("cannot be passed"),
        "{}",
        describe(&out)
    );
}

/// (d) Copilot takes the inherited effort as `--reasoning-effort L` on every
/// step, with no Claude variables, and no "cannot be passed" line.
#[test]
fn d_copilot_steps_receive_reasoning_effort() {
    let f = Fixture::new(&[]);
    let out = f.run("copilot", &[("AMPLIHACK_PARENT_EFFORT", "high")]);
    assert!(out.status.success(), "{}", describe(&out));
    let all = format!(
        "{}\n{}\n{}",
        text(&out.stderr),
        f.step_stderr(1),
        f.step_stderr(2)
    );
    assert!(!all.contains("cannot be passed"), "{all}");
    for step in [1, 2] {
        let call = f.call(step);
        let args = argv(&call);
        assert_eq!(
            count(&args, "--reasoning-effort"),
            1,
            "step {step}: {args:?}"
        );
        assert_eq!(
            flag(&args, "--reasoning-effort").as_deref(),
            Some("high"),
            "step {step}: {args:?}"
        );
        assert_eq!(count(&args, "--effort"), 0, "step {step}: {args:?}");
        assert_eq!(call["CLAUDE_CODE_EFFORT_LEVEL"], "<unset>", "step {step}");
        assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "high", "step {step}");
    }
}

/// (d) D7: an effort copilot does not list is reported once per run, by
/// `recipe run`, and never by the steps.
#[test]
fn d_copilot_unlisted_effort_line_appears_exactly_once() {
    let f = Fixture::new(&[]);
    let out = f.run("copilot", &[("AMPLIHACK_PARENT_EFFORT", "ultra")]);
    assert!(out.status.success(), "{}", describe(&out));
    let line = "amplihack: effort 'ultra' cannot be passed to copilot; steps use copilot's default";
    let all = format!(
        "{}\n{}\n{}",
        text(&out.stderr),
        f.step_stderr(1),
        f.step_stderr(2)
    );
    assert_eq!(all.lines().filter(|l| *l == line).count(), 1, "{all}");
    for step in [1, 2] {
        let call = f.call(step);
        let args = argv(&call);
        assert_eq!(
            count(&args, "--reasoning-effort"),
            0,
            "step {step}: {args:?}"
        );
        assert_eq!(count(&args, "--effort"), 0, "step {step}: {args:?}");
        assert_eq!(call["CLAUDE_CODE_EFFORT_LEVEL"], "<unset>", "step {step}");
    }
}

/// (e) #1699: no child sees the real profile root or a real HOME.
#[test]
fn e_children_are_isolated() {
    let f = Fixture::new(&[]);
    let out = f.run("claude", &PARENT_HIGH_X);
    assert!(out.status.success(), "{}", describe(&out));
    for step in [1, 2] {
        let call = f.call(step);
        assert!(
            !names(&call).contains(&"AMPLIHACK_CLAUDE_PROFILE_ROOT".to_string()),
            "step {step} inherited AMPLIHACK_CLAUDE_PROFILE_ROOT"
        );
        assert!(
            Path::new(&call["HOME"]).starts_with(&f.root),
            "step {step} HOME={} is outside {}",
            call["HOME"],
            f.root.display()
        );
    }
}

/// A stale or tampered parent value in the caller's environment is checked,
/// warned about once without echoing it, and never reaches a step.
#[test]
fn an_invalid_parent_value_never_reaches_a_step() {
    let f = Fixture::new(&[]);
    let out = f.run(
        "claude",
        &[("AMPLIHACK_PARENT_MODEL", "-sk-ant-not-a-model")],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let stderr = text(&out.stderr);
    assert!(!stderr.contains("sk-ant"), "{stderr}");
    assert_eq!(
        stderr
            .lines()
            .filter(|l| *l
                == "amplihack: ignoring model from the parent amplihack launch: not a valid option value")
            .count(),
        1,
        "{stderr}"
    );
    for step in [1, 2] {
        let call = f.call(step);
        assert_eq!(
            flag(&argv(&call), "--model").as_deref(),
            Some(DEFAULT_MODEL)
        );
        assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "<unset>", "step {step}");
    }
}

/// `-c effort=` and `-c model=` set the run's values without changing the
/// session, and beat the parent launch's values.
#[test]
fn context_values_set_the_run() {
    let f = Fixture::new(&[]);
    let out = f.run_with(
        "claude",
        &["-c", "effort=medium", "-c", "model=claude-sonnet-4-5"],
        &PARENT_HIGH_X,
    );
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        text(&out.stderr).lines().any(|l| l
            == "amplihack: steps inherit effort=medium model=claude-sonnet-4-5 (from the recipe context)"),
        "{}",
        describe(&out)
    );
    for step in [1, 2] {
        let args = argv(&f.call(step));
        assert_eq!(
            flag(&args, "--effort").as_deref(),
            Some("medium"),
            "{args:?}"
        );
        assert_eq!(
            flag(&args, "--model").as_deref(),
            Some("claude-sonnet-4-5"),
            "{args:?}"
        );
    }
}

/// (f) D2: a plain `claude` session above the run is read through `/proc`.
#[cfg(target_os = "linux")]
#[test]
fn f_a_plain_claude_ancestor_is_inherited() {
    let f = Fixture::new(&["claude", "--effort", "high", "--model", X]);
    let out = f.run("claude", &[]);
    assert!(out.status.success(), "{}", describe(&out));
    for step in [1, 2] {
        let args = argv(&f.call(step));
        assert_eq!(flag(&args, "--effort").as_deref(), Some("high"), "{args:?}");
        assert_eq!(flag(&args, "--model").as_deref(), Some(X), "{args:?}");
    }
    assert!(
        text(&out.stderr).lines().any(|l| l
            == format!(
                "amplihack: steps inherit effort=high model={X} (from the parent claude process)"
            )),
        "{}",
        describe(&out)
    );
}

/// (f) C2: under an older amplihack session the ancestor's `--model` may be
/// amplihack's own default, so only its effort is taken.
#[cfg(target_os = "linux")]
#[test]
fn f_an_ancestor_model_is_not_trusted_inside_an_older_session() {
    let f = Fixture::new(&["claude", "--effort", "high", "--model", DEFAULT_MODEL]);
    let out = f.run("claude", &[("AMPLIHACK_SESSION_ID", "older-session")]);
    assert!(out.status.success(), "{}", describe(&out));
    for step in [1, 2] {
        let call = f.call(step);
        let args = argv(&call);
        assert_eq!(flag(&args, "--effort").as_deref(), Some("high"), "{args:?}");
        // The model is amplihack's own default again, chosen by the step, not
        // inherited: no ANTHROPIC_MODEL and no exported parent model.
        assert_eq!(flag(&args, "--model").as_deref(), Some(DEFAULT_MODEL));
        assert_eq!(call["ANTHROPIC_MODEL"], "<unset>", "step {step}");
        assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "<unset>", "step {step}");
    }
}

/// (g) Crusty review of #1892, round 2: a Claude run passes its model to the
/// runner, which every process under the run sees. A step that starts
/// `amplihack copilot` (quality-loop.yaml) must not receive that Claude
/// model id, because the run checked it for claude only. It still receives
/// the effort, which copilot lists.
#[test]
fn g_claude_run_model_does_not_reach_an_amplihack_copilot_step() {
    let f = Fixture::new(&[]);
    let out = f.run(
        "claude",
        &[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("AMPLIHACK_PARENT_MODEL", X),
            ("STEP2_BIN", "copilot"),
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let claude = argv(&f.call(1));
    assert_eq!(flag(&claude, "--model").as_deref(), Some(X), "{claude:?}");
    let call = f.call(2);
    let copilot = argv(&call);
    assert_eq!(count(&copilot, "--model"), 0, "{copilot:?}");
    assert!(!copilot.iter().any(|a| a.contains(X)), "{copilot:?}");
    assert_eq!(
        flag(&copilot, "--reasoning-effort").as_deref(),
        Some("high"),
        "{copilot:?}"
    );
    assert_eq!(call["ANTHROPIC_MODEL"], "<unset>");
    assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "<unset>");
    let line =
        format!("amplihack: model '{X}' cannot be passed to copilot; steps use copilot's default");
    assert_eq!(
        f.step_stderr(2).lines().filter(|l| *l == line).count(),
        1,
        "{}",
        f.step_stderr(2)
    );
}

/// (h) Crusty review of #1892, round 3: under a Claude run with effort `max`,
/// a bash step that starts `amplihack codex` cannot pass `max` to codex. The
/// run's own lines named only claude, so the codex launch prints the
/// "cannot be passed" line itself, once, and passes neither the effort nor
/// the Claude model id.
#[test]
fn h_claude_run_effort_codex_cannot_take_is_logged_by_the_codex_step() {
    let f = Fixture::new(&[]);
    let out = f.run(
        "claude",
        &[
            ("AMPLIHACK_PARENT_EFFORT", "max"),
            ("AMPLIHACK_PARENT_MODEL", X),
            ("STEP2_BIN", "codex"),
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let claude = argv(&f.call(1));
    assert_eq!(
        flag(&claude, "--effort").as_deref(),
        Some("max"),
        "{claude:?}"
    );
    let codex = argv(&f.call(2));
    assert_eq!(count(&codex, "--model"), 0, "{codex:?}");
    assert!(
        !codex
            .iter()
            .any(|a| a.contains("model_reasoning_effort") || a.contains(X)),
        "{codex:?}"
    );
    let stderr = f.step_stderr(2);
    for line in [
        "amplihack: effort 'max' cannot be passed to codex; steps use codex's default".to_string(),
        format!("amplihack: model '{X}' cannot be passed to codex; steps use codex's default"),
    ] {
        assert_eq!(
            stderr.lines().filter(|l| *l == line).count(),
            1,
            "{line}\n---\n{stderr}"
        );
    }
    // The run's own lines are about claude, which takes both values.
    assert!(
        !text(&out.stderr).contains("cannot be passed"),
        "{}",
        describe(&out)
    );
}

/// A real top-level launch exports both the model and the binary that selected
/// it. A recipe started by the agent then passes it to every same-binary step.
#[test]
fn codex_and_copilot_parent_models_reach_nested_recipe_steps() {
    for agent in ["codex", "copilot"] {
        let f = Fixture::new(&[]);
        fs::create_dir(f.records.join("nested")).unwrap();
        let out = f
            .command(
                agent,
                &[("STEP", "parent"), ("AMPLIHACK_TEST_NESTED_ONCE", "parent")],
            )
            .args([agent, "--subprocess-safe", "--", "--model", "gpt-5.1"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{agent}: {}", describe(&out));
        for step in [1, 2] {
            let raw = fs::read_to_string(f.records.join(format!("nested/call.{step}"))).unwrap();
            assert!(raw.contains("--model|gpt-5.1|"), "{agent}: {raw}");
            assert!(raw.contains("AMPLIHACK_PARENT_MODEL=gpt-5.1\n"), "{raw}");
            assert!(
                raw.contains(&format!("AMPLIHACK_PARENT_MODEL_BINARY={agent}\n")),
                "{raw}"
            );
            assert!(raw.contains("ANTHROPIC_MODEL=<unset>"), "{raw}");
        }
    }
}

/// Context-selected models survive a step starting a second recipe, and a
/// step's explicit model replaces the enclosing run's choice for its children.
#[test]
fn context_and_step_models_survive_subsequent_same_binary_runs() {
    for agent in ["codex", "copilot"] {
        for own in [false, true] {
            let f = Fixture::new(&[]);
            fs::create_dir(f.records.join("nested")).unwrap();
            if own {
                // Step 2 selects the model and starts the nested run. Its own
                // choice must win over the surrounding run's gpt-5.1.
                let out = f.run_with(
                    agent,
                    &["-c", "model=gpt-5.1"],
                    &[
                        ("AMPLIHACK_TEST_NESTED_ONCE", "2"),
                        ("STEP2_ARGS", "--model gpt-5.2"),
                    ],
                );
                assert!(out.status.success(), "{}", describe(&out));
            } else {
                // The nested recipe has no model context, so it must inherit
                // what the first run resolved rather than finding it again.
                let out = f.run_with(
                    agent,
                    &["-c", "model=gpt-5.1"],
                    &[("AMPLIHACK_TEST_NESTED_ONCE", "1")],
                );
                assert!(out.status.success(), "{}", describe(&out));
            }
            let model = if own { "gpt-5.2" } else { "gpt-5.1" };
            let raw = fs::read_to_string(f.records.join("nested/call.1")).unwrap();
            assert!(raw.contains(&format!("--model|{model}|")), "{agent}: {raw}");
            assert!(
                raw.contains(&format!("AMPLIHACK_PARENT_MODEL={model}\n")),
                "{raw}"
            );
        }
    }
}

#[test]
fn codex_quoted_toml_effort_survives_a_nested_recipe() {
    for (args, level) in [
        (vec!["-c", r#"model_reasoning_effort="high""#], "high"),
        (vec!["-c", "model_reasoning_effort='high'"], "high"),
        (vec!["-c", r#"model_reasoning_effort="h\u0069gh""#], "high"),
        (vec!["-c", "model_reasoning_effort = \"high\""], "high"),
        (
            vec!["--config", " model_reasoning_effort = 'high' "],
            "high",
        ),
        (vec!["--config= model_reasoning_effort = \"high\" "], "high"),
        (vec!["-c", " model_reasoning_effort = high "], "high"),
        (vec!["-c", r#"model_reasoning_effort="low""#], "low"),
    ] {
        let f = Fixture::new(&[]);
        fs::create_dir(f.records.join("nested")).unwrap();
        let out = f
            .command(
                "codex",
                &[
                    ("STEP", "parent"),
                    ("AMPLIHACK_TEST_NESTED_ONCE", "parent"),
                    ("AMPLIHACK_PARENT_EFFORT", "low"),
                ],
            )
            .args(["codex", "--subprocess-safe", "--"])
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", describe(&out));
        assert!(
            text(&out.stderr).contains(
                &format!("amplihack: steps inherit effort={level} model=unset (from the parent amplihack launch)")
            ),
            "{args:?}: {}",
            describe(&out)
        );
        assert!(
            !text(&out.stderr).contains("cannot be passed"),
            "{}",
            describe(&out)
        );
        let raw = fs::read_to_string(f.records.join("call.parent")).unwrap();
        assert!(
            raw.contains(&format!("AMPLIHACK_PARENT_EFFORT={level}\n")),
            "{raw}"
        );
        for step in [1, 2] {
            let raw = fs::read_to_string(f.records.join(format!("nested/call.{step}"))).unwrap();
            assert!(
                raw.contains(&format!("-c|model_reasoning_effort={level}|")),
                "{args:?}: {raw}"
            );
            assert!(
                raw.contains(&format!("AMPLIHACK_PARENT_EFFORT={level}\n")),
                "{raw}"
            );
        }
    }
}

/// Exercise the public command with the real runner, including a successful
/// bash launch control. A stub cannot detect the runner's YAML alias exports.
#[test]
fn real_runner_yaml_cannot_override_checked_parent_inheritance() {
    let runner = std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join("recipe-runner-rs"))
                    .find(|p| p.is_file())
            })
        })
        .expect("install the runner pinned in claude-plugin/recipe-runner.rev");
    let f = Fixture::new(&[]);
    let recipe = r#"name: inherit-1883
version: 1.0.0
context: {}
steps:
  - id: nested-launch
    type: bash
    command: |
      STEP=1 "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p control
"#;
    fs::write(f.root.join("inherit-1883.yaml"), recipe).unwrap();
    let run = || {
        f.command("claude", &PARENT_HIGH_X)
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .args(["recipe", "run"])
            .arg(f.root.join("inherit-1883.yaml"))
            .arg("-w")
            .arg(&f.work)
            .output()
            .unwrap()
    };
    let out = run();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.call(1)["AMPLIHACK_PARENT_EFFORT"], "high");
    assert_eq!(f.call(1)["AMPLIHACK_PARENT_MODEL"], X);
    fs::remove_file(f.records.join("call.1")).unwrap();
    for key in [
        "amplihack_parent_effort",
        "AMPLIHACK_PARENT_MODEL",
        "Amplihack_Parent_Model_Binary",
    ] {
        let yaml = recipe.replace("context: {}", &format!("context: {{{key}: low}}"));
        fs::write(f.root.join("inherit-1883.yaml"), yaml).unwrap();
        let out = run();
        assert!(!out.status.success(), "{key}: {}", describe(&out));
        assert!(
            text(&out.stderr).contains("namespace cannot be set from recipe context"),
            "{}",
            describe(&out)
        );
        assert!(
            !f.records.join("call.1").exists(),
            "reserved YAML reached a bash launch"
        );
    }
}

#[test]
fn codex_step_short_model_overrides_the_recipe_context_model() {
    let f = Fixture::new(&[]);
    let out = f.run_with(
        "codex",
        &["-c", "model=inherited-model"],
        &[("STEP2_ARGS", "-m step-model")],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let inherited_call = f.call(1);
    let inherited_args = argv(&inherited_call);
    assert!(
        inherited_args
            .windows(2)
            .any(|pair| pair == ["--model", "inherited-model"]),
        "{inherited_args:?}"
    );
    let explicit_call = f.call(2);
    let explicit_args = argv(&explicit_call);
    assert!(
        explicit_args
            .windows(2)
            .any(|pair| pair == ["-m", "step-model"]),
        "{explicit_args:?}"
    );
    assert_eq!(count(&explicit_args, "--model"), 0, "{explicit_args:?}");
    assert_eq!(count(&explicit_args, "-m"), 1, "{explicit_args:?}");
    assert_eq!(explicit_call["AMPLIHACK_PARENT_MODEL"], "step-model");
    assert_eq!(explicit_call["AMPLIHACK_PARENT_MODEL_BINARY"], "codex");
}

#[test]
fn codex_short_model_override_is_forwarded_without_a_duplicate_and_inherited_by_nested_run() {
    let f = Fixture::new(&[]);
    fs::create_dir(f.records.join("nested")).unwrap();
    let out = f
        .command(
            "codex",
            &[
                ("STEP", "0"),
                ("AMPLIHACK_TEST_NESTED_ONCE", "0"),
                ("AMPLIHACK_PARENT_MODEL", "parent-model"),
                ("AMPLIHACK_PARENT_MODEL_BINARY", "codex"),
            ],
        )
        .args(["codex", "--subprocess-safe", "--", "-m", "own-model"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let call = f.call(0);
    let args = argv(&call);
    assert!(
        args.windows(2).any(|pair| pair == ["-m", "own-model"]),
        "{args:?}"
    );
    assert!(!args.iter().any(|arg| arg == "--model"), "{args:?}");
    assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "own-model");
    for step in [1, 2] {
        let raw = fs::read_to_string(f.records.join(format!("nested/call.{step}"))).unwrap();
        assert!(raw.contains("--model|own-model|"), "{raw}");
        assert!(raw.contains("AMPLIHACK_PARENT_MODEL=own-model\n"), "{raw}");
    }
}

#[test]
fn codex_quoted_and_attached_effort_overrides_reach_nested_runs_without_quotes() {
    for nested_binary in ["codex", "claude"] {
        for extra in [
            vec!["-c", r#"model_reasoning_effort="low""#],
            vec!["-cmodel_reasoning_effort=low"],
            vec![r#"-c=model_reasoning_effort="low""#],
        ] {
            let f = Fixture::new(&[]);
            fs::create_dir(f.records.join("nested")).unwrap();
            let out = f
                .command(
                    "codex",
                    &[
                        ("STEP", "0"),
                        ("AMPLIHACK_TEST_NESTED_ONCE", "0"),
                        ("AMPLIHACK_PARENT_EFFORT", "high"),
                        ("STEP2_BIN", nested_binary),
                    ],
                )
                .args(["codex", "--subprocess-safe", "--"])
                .args(&extra)
                .output()
                .unwrap();
            assert!(out.status.success(), "{extra:?}: {}", describe(&out));
            let call = f.call(0);
            let args = argv(&call);
            assert_eq!(args, extra, "explicit config must be forwarded unchanged");
            assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "low", "{extra:?}");
            for step in [1, 2] {
                let raw =
                    fs::read_to_string(f.records.join(format!("nested/call.{step}"))).unwrap();
                let expected_effort = if step == 1 || nested_binary == "codex" {
                    "-c|model_reasoning_effort=low|"
                } else {
                    "--effort|low|"
                };
                assert!(
                    raw.contains(expected_effort),
                    "{nested_binary}, {extra:?}: {raw}"
                );
                if step == 2 && nested_binary == "claude" {
                    assert!(raw.contains("CLAUDE_CODE_EFFORT_LEVEL=low\n"), "{raw}");
                }
                assert!(
                    raw.contains("AMPLIHACK_PARENT_EFFORT=low\n"),
                    "{extra:?}: {raw}"
                );
                assert!(
                    !raw.contains("model_reasoning_effort=high"),
                    "{extra:?}: {raw}"
                );
            }
        }
    }
}

#[test]
fn system_prompt_that_looks_like_a_model_flag_keeps_inherited_model() {
    let f = Fixture::new(&[]);
    let out = f
        .command("claude", &[("AMPLIHACK_PARENT_MODEL", "sonnet")])
        .args([
            "claude",
            "--subprocess-safe",
            "--",
            "--system-prompt",
            "--model=haiku",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let call = f.call(0);
    let args = argv(&call);
    assert!(
        args.windows(2).any(|pair| pair == ["--model", "sonnet"]),
        "{args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--system-prompt", "--model=haiku"]),
        "{args:?}"
    );
    assert_eq!(call["ANTHROPIC_MODEL"], "sonnet");
    assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "sonnet");
}
