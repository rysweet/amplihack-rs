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
//! - (d) copilot: every step receives an effort chosen for copilot (a copilot
//!   launch's, or the run's `-c effort=`) as `--reasoning-effort`; an effort
//!   chosen for claude, from a parent launch, the `claude` ancestor or
//!   `CLAUDE_CODE_EFFORT_LEVEL`, is not passed, and neither is an effort
//!   copilot does not list; each gives the "cannot be passed" line exactly
//!   once across the run and two steps.
//! - (e) children have no `AMPLIHACK_CLAUDE_PROFILE_ROOT` and HOME lies under
//!   the temp root.
//! - (f) Linux: a plain `claude` ancestor's argv is read through `/proc`; under
//!   an older amplihack session its model is not trusted (C2).
//! - (g) a Claude run with an inherited effort and model whose second step
//!   starts `amplihack copilot`: that step receives neither, and says so.
//! - (h) a Claude run with effort `max` whose second step starts
//!   `amplihack codex`: codex cannot take `max`, so that step prints the
//!   "cannot be passed" line for effort and for the model, once each.
//! - (i) Linux, with tmux: a private tmux server started inside an amplihack
//!   session at `claude-haiku-4-5`/`low` keeps those values; a run started in
//!   it with `"$(amplihack agent-binary --shell) …"` runs every step with the
//!   caller's model and effort instead, or with none when the caller has none.

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

/// The fake agent: answers `--version` and `plugin`; otherwise writes
/// `call.<STEP>` and exits 0. Values are recorded only for the allowlisted
/// variables. As `codex` it reports a version the Codex health gate verifies
/// (with `codex-code-mode-host` beside it, as in `issue_1985`'s fixture); as
/// anything else, a Claude Code new enough for `--system-prompt-file` (#2030).
const FAKE_AGENT: &str = r#"#!/bin/sh
case "$1" in
  --version)
    case "${0##*/}" in
      codex) echo 'codex-cli 0.166.0' ;;
      *) echo '2.1.322 (Claude Code)' ;;
    esac
    exit 0 ;;
  plugin)
    case "$2" in
      list) echo '{"installed":[]}' ;;
      *) echo '{"ok":true}' ;;
    esac
    exit 0 ;;
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
  printf 'AMPLIHACK_PARENT_EFFORT_BINARY=%s\n' "${AMPLIHACK_PARENT_EFFORT_BINARY-<unset>}"
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
  --capabilities) printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}'; exit 0 ;;
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
        // The Codex health gate requires this helper beside codex 0.147-0.166.
        executable(&tools.join("codex-code-mode-host"), "#!/bin/sh\nexit 0\n");
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
        // No ETXTBSY retry: the command is `amplihack_bin()`, and every stub
        // this fixture execs is written by `exec_stub::write_executable`, so
        // this process never holds an executable open for writing.
        c.output().expect("run amplihack recipe run")
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

/// (d) Copilot takes an effort chosen for copilot as `--reasoning-effort L`
/// on every step, with no Claude variables, and no "cannot be passed" line:
/// one a copilot launch exported, or the run's own `-c effort=`.
#[test]
fn d_copilot_steps_receive_reasoning_effort() {
    let from_copilot_launch: (&[&str], &[(&str, &str)]) = (
        &[],
        &[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "copilot"),
        ],
    );
    // The run's context outranks a Claude session's effort, and is given to
    // this copilot run.
    let from_context: (&[&str], &[(&str, &str)]) = (
        &["-c", "effort=high"],
        &[
            ("AMPLIHACK_PARENT_EFFORT", "max"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "claude"),
        ],
    );
    for (extra, env) in [from_copilot_launch, from_context] {
        let f = Fixture::new(&[]);
        let out = f.run_with("copilot", extra, env);
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
            assert_eq!(
                call["AMPLIHACK_PARENT_EFFORT_BINARY"], "copilot",
                "step {step}"
            );
        }
    }
}

/// (d) Crusty review of #1892, round 7: which levels copilot accepts depends
/// on its model, and copilot exits 1 at startup on a level it refuses. An
/// effort chosen for claude says nothing about copilot's model, so a copilot
/// run does not pass it: not from a Claude launch (`AMPLIHACK_PARENT_EFFORT`
/// with claude or no provenance), not from the `claude` ancestor process, and
/// not from `CLAUDE_CODE_EFFORT_LEVEL`, with or without `-c model=<gpt id>`.
/// The run says so once and every step starts without `--reasoning-effort`.
#[test]
fn d_copilot_run_does_not_pass_an_effort_chosen_for_claude() {
    let ancestor = ["claude", "--effort", "max"];
    // (ancestor argv, environment, extra `recipe run` arguments)
    type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)], &'a [&'a str]);
    let cases: [Case; 5] = [
        (&[], &[("AMPLIHACK_PARENT_EFFORT", "max")], &[]),
        (
            &[],
            &[
                ("AMPLIHACK_PARENT_EFFORT", "max"),
                ("AMPLIHACK_PARENT_EFFORT_BINARY", "claude"),
            ],
            &[],
        ),
        (&ancestor, &[], &[]),
        (&ancestor, &[], &["-c", "model=gpt-5.1"]),
        (&[], &[("CLAUDE_CODE_EFFORT_LEVEL", "max")], &[]),
    ];
    let line = "amplihack: effort 'max' cannot be passed to copilot; steps use copilot's default";
    for (ancestor, env, extra) in cases {
        let f = Fixture::new(ancestor);
        let out = f.run_with("copilot", extra, env);
        assert!(out.status.success(), "{}", describe(&out));
        let all = format!(
            "{}\n{}\n{}",
            text(&out.stderr),
            f.step_stderr(1),
            f.step_stderr(2)
        );
        assert_eq!(
            all.lines().filter(|l| *l == line).count(),
            1,
            "{ancestor:?} {env:?} {extra:?}: {all}"
        );
        assert!(
            !text(&out.stderr).contains("steps inherit effort=max"),
            "{}",
            describe(&out)
        );
        for step in [1, 2] {
            let call = f.call(step);
            let args = argv(&call);
            assert_eq!(
                count(&args, "--reasoning-effort"),
                0,
                "{ancestor:?} {env:?} {extra:?} step {step}: {args:?}"
            );
            assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "<unset>", "step {step}");
            assert_eq!(
                call["AMPLIHACK_PARENT_EFFORT_BINARY"], "<unset>",
                "step {step}"
            );
        }
    }
}

/// (d) D7: an effort copilot does not list is reported once per run, by
/// `recipe run`, and never by the steps.
#[test]
fn d_copilot_unlisted_effort_line_appears_exactly_once() {
    let f = Fixture::new(&[]);
    let out = f.run(
        "copilot",
        &[
            ("AMPLIHACK_PARENT_EFFORT", "ultra"),
            ("AMPLIHACK_PARENT_EFFORT_BINARY", "copilot"),
        ],
    );
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
/// model id, because the run checked it for claude only. Round 7: nor the
/// effort, which the run chose for claude (`AMPLIHACK_PARENT_EFFORT_BINARY`
/// is `claude`); a Claude `max` could make copilot exit 1 on its own model.
/// The copilot launch says so, once for each.
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
    assert_eq!(f.call(1)["AMPLIHACK_PARENT_EFFORT_BINARY"], "claude");
    let call = f.call(2);
    let copilot = argv(&call);
    assert_eq!(count(&copilot, "--model"), 0, "{copilot:?}");
    assert!(!copilot.iter().any(|a| a.contains(X)), "{copilot:?}");
    assert_eq!(count(&copilot, "--reasoning-effort"), 0, "{copilot:?}");
    assert_eq!(call["ANTHROPIC_MODEL"], "<unset>");
    assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "<unset>");
    assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "<unset>");
    for line in [
        "amplihack: effort 'high' cannot be passed to copilot; steps use copilot's default"
            .to_string(),
        format!("amplihack: model '{X}' cannot be passed to copilot; steps use copilot's default"),
    ] {
        assert_eq!(
            f.step_stderr(2).lines().filter(|l| *l == line).count(),
            1,
            "{line}\n---\n{}",
            f.step_stderr(2)
        );
    }
    // The run's own lines are about claude, which takes both values.
    assert!(
        !text(&out.stderr).contains("cannot be passed"),
        "{}",
        describe(&out)
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

/// A multi-line JSON recipe whose bash step holds a UTF-16 surrogate-pair
/// escape (`\uD83D\uDE00`, U+1F600) on a later line. YAML refuses that
/// escape, so only the JSON fallback of `parse_recipe_text` reads it (#1964,
/// docs/reference/recipe-cli-reference.md, "Recipe text encoding"). The step
/// starts the fake agent as step 1. `{CONTEXT}` is replaced with the
/// recipe's `context` object.
const JSON_RECIPE: &str = r#"{"name": "inherit-1883-json", "version": "1.0.0",
 "context": {CONTEXT},
 "steps": [{"id": "nested-launch", "type": "bash",
   "command": "echo \uD83D\uDE00 >/dev/null && STEP=1 \"$AMPLIHACK_UNDER_TEST\" claude --subprocess-safe -- -p control"}]}
"#;

/// The recipe runner the CI installs from claude-plugin/recipe-runner.rev.
fn real_runner() -> PathBuf {
    std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join("recipe-runner-rs"))
                    .find(|p| p.is_file())
            })
        })
        .expect("install the runner pinned in claude-plugin/recipe-runner.rev")
}

/// Crusty review of #1892, round 7: `recipe run` once re-read the recipe
/// file with a YAML-only parser to look for reserved context keys, and so
/// refused a JSON recipe with a surrogate pair that `recipe validate` and the
/// runner accept (`found invalid Unicode character escape code`). The file is
/// read once, by `parse_recipe_text`; its `context:` reaches the run through
/// the merged context map. With the stub runner: `recipe validate` and
/// `recipe run` both accept the recipe, and its `effort` reaches every step.
#[test]
fn a_json_recipe_with_a_surrogate_pair_runs_and_its_context_is_inherited() {
    let f = Fixture::new(&[]);
    let path = f.root.join("inherit-1883.json");
    fs::write(
        &path,
        JSON_RECIPE.replace("{CONTEXT}", r#"{"effort": "low"}"#),
    )
    .unwrap();
    let validate = f
        .command("claude", &[])
        .args(["recipe", "validate"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(validate.status.success(), "{}", describe(&validate));
    let out = f
        .command("claude", &[])
        .args(["recipe", "run"])
        .arg(&path)
        .arg("-w")
        .arg(&f.work)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!text(&out.stderr).contains("Unicode"), "{}", describe(&out));
    assert!(
        text(&out.stderr)
            .contains("amplihack: steps inherit effort=low model=unset (from the recipe context)"),
        "{}",
        describe(&out)
    );
    for step in [1, 2] {
        let call = f.call(step);
        assert_eq!(
            flag(&argv(&call), "--effort").as_deref(),
            Some("low"),
            "step {step}"
        );
        assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "low", "step {step}");
    }
}

/// S6 through the command, for a YAML and a JSON recipe: an `AMPLIHACK_*`
/// key in the `context:` of the recipe the run starts is refused before the
/// runner starts, on a dry run too. The stub runner launches its steps even
/// on a dry run, so a step record would show the check was missed.
#[test]
fn a_reserved_key_in_the_recipe_context_is_refused_before_the_runner_starts() {
    let f = Fixture::new(&[]);
    for key in [
        "amplihack_parent_effort",
        "AMPLIHACK_PARENT_MODEL",
        "Amplihack_Parent_Model_Binary",
        "AMPLIHACK_PARENT_EFFORT_BINARY",
    ] {
        let yaml = RECIPE.replace("context: {}", &format!("context: {{{key}: low}}"));
        let json = JSON_RECIPE.replace("{CONTEXT}", &format!(r#"{{"{key}": "low"}}"#));
        for (file, body) in [("reserved.yaml", yaml), ("reserved.json", json)] {
            let path = f.root.join(file);
            fs::write(&path, body).unwrap();
            for dry_run in [false, true] {
                let mut c = f.command("claude", &[]);
                c.args(["recipe", "run"]).arg(&path).arg("-w").arg(&f.work);
                if dry_run {
                    c.arg("--dry-run");
                }
                let out = c.output().unwrap();
                let what = format!("{key} in {file}, dry run {dry_run}");
                assert!(!out.status.success(), "{what}: {}", describe(&out));
                assert!(
                    text(&out.stderr).contains("namespace cannot be set from recipe context"),
                    "{what}: {}",
                    describe(&out)
                );
                assert!(
                    !f.records.join("call.1").exists(),
                    "{what}: a reserved key reached a step"
                );
            }
        }
    }
}

/// Crusty's reproduction with the real runner: a multi-line JSON recipe with
/// a surrogate pair runs, and its bash step's launch inherits the recipe
/// context's effort.
#[test]
fn real_runner_runs_a_multi_line_json_recipe_with_a_surrogate_pair() {
    let runner = real_runner();
    let f = Fixture::new(&[]);
    let path = f.root.join("inherit-1883.json");
    fs::write(
        &path,
        JSON_RECIPE.replace("{CONTEXT}", r#"{"effort": "low"}"#),
    )
    .unwrap();
    let out = f
        .command("claude", &[])
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .args(["recipe", "run"])
        .arg(&path)
        .arg("-w")
        .arg(&f.work)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let call = f.call(1);
    assert_eq!(flag(&argv(&call), "--effort").as_deref(), Some("low"));
    assert_eq!(call["AMPLIHACK_PARENT_EFFORT"], "low");
}

/// Exercise the public command with the real runner, including a successful
/// bash launch control. A stub cannot detect the runner's YAML alias exports.
#[test]
fn real_runner_yaml_cannot_override_checked_parent_inheritance() {
    let runner = real_runner();
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

/// A `--system-prompt` value that looks like `--model=…` never becomes the
/// step's own model. Since #2030 the text reaches Claude as a
/// `--system-prompt-file` path, never on argv, so the agent sees the
/// inherited `--model` and the file flag, and no `--model=haiku` anywhere.
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
    assert!(args.iter().any(|a| a == "--system-prompt-file"), "{args:?}");
    assert!(
        !args.iter().any(|a| a == "--model=haiku" || a == "haiku"),
        "{args:?}"
    );
    assert_eq!(call["ANTHROPIC_MODEL"], "sonnet");
    assert_eq!(call["AMPLIHACK_PARENT_MODEL"], "sonnet");
}

// ---------------------------------------------------------------------------
// (i) The tmux hand-off (crusty review of #1892, round 6)
// ---------------------------------------------------------------------------

/// Kills the private tmux server on every way out of the test.
#[cfg(target_os = "linux")]
struct TmuxServer {
    dir: PathBuf,
}

#[cfg(target_os = "linux")]
impl TmuxServer {
    fn tmux(&self) -> Command {
        let mut c = Command::new("tmux");
        c.env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("TMUX_TMPDIR", &self.dir)
            .stdin(std::process::Stdio::null());
        c
    }
}

#[cfg(target_os = "linux")]
impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = self.tmux().arg("kill-server").output();
    }
}

/// `cmd`'s environment as it would start: `isolated()` clears it, so every
/// variable the child gets is listed.
#[cfg(target_os = "linux")]
fn env_of(cmd: &Command) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    cmd.get_envs()
        .filter_map(|(k, v)| Some((k.to_os_string(), v?.to_os_string())))
        .collect()
}

/// The values the session that first started the tmux server ran with.
#[cfg(target_os = "linux")]
const SERVER_STARTER: [(&str, &str); 5] = [
    ("AMPLIHACK_PARENT_EFFORT", "low"),
    ("AMPLIHACK_PARENT_MODEL", Y),
    ("AMPLIHACK_PARENT_MODEL_BINARY", "claude"),
    ("CLAUDE_CODE_EFFORT_LEVEL", "low"),
    ("ANTHROPIC_MODEL", Y),
];

/// The values of the session that starts the run, later.
#[cfg(target_os = "linux")]
const CALLER: [(&str, &str); 5] = [
    ("AMPLIHACK_PARENT_EFFORT", "xhigh"),
    ("AMPLIHACK_PARENT_MODEL", X),
    ("AMPLIHACK_PARENT_MODEL_BINARY", "claude"),
    ("CLAUDE_CODE_EFFORT_LEVEL", "xhigh"),
    ("ANTHROPIC_MODEL", X),
];

/// (i) `tmux new-session` gives its command the server's global environment,
/// copied from whatever started the server (tmux(1), GLOBAL AND SESSION
/// ENVIRONMENT), not the caller's. A server first started inside an amplihack
/// session at `claude-haiku-4-5`/`low` therefore hands those values to every
/// later session, where `recipe run` ranks them above all but its own
/// context. The control run shows that; the documented hand-off,
/// `tmux new-session -d "$(amplihack agent-binary --shell) amplihack recipe
/// run …"` in the caller's shell, replaces them with the caller's, or removes
/// them when the caller has none.
#[cfg(target_os = "linux")]
#[test]
fn i_a_tmux_hand_off_carries_the_callers_effort_and_model_not_the_servers() {
    if !Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipping: tmux is not installed");
        return;
    }
    let f = Fixture::new(&[]);
    let (dir, _socket_tmp) = crate::support::socket_dir::tmux_socket_dir(&f.root);
    fs::create_dir_all(&dir).unwrap();
    if !crate::support::socket_dir::tmux_socket_fits(&dir) {
        eprintln!("{}", crate::support::socket_dir::skip_message(&dir));
        return;
    }
    let server = TmuxServer { dir };

    // The server's starter: an amplihack session at haiku/low, as a step of
    // another run or another drive's agent would be.
    let starter = f.command("claude", &SERVER_STARTER);
    let started = server
        .tmux()
        .envs(env_of(&starter))
        .env("TMUX_TMPDIR", &server.dir)
        .args(["new-session", "-d", "-s", "host"])
        .output()
        .unwrap();
    assert!(started.status.success(), "{}", describe(&started));

    let amp = amplihack_bin();
    let recipe = f.root.join("inherit-1883.yaml");
    // `case` names the session and its output files; `prefix` is put in
    // front of the far side's command by the caller's own shell, as the
    // documented hand-off does.
    let start_run = |case: &str, caller: &[(&str, &str)], prefix: &str| -> String {
        let done = f.root.join(format!("{case}.done"));
        let err = f.root.join(format!("{case}.err"));
        let far = format!(
            "'{}' recipe run '{}' -w '{}' >'{}' 2>'{}'; touch '{}'",
            amp.display(),
            recipe.display(),
            f.work.display(),
            f.root.join(format!("{case}.out")).display(),
            err.display(),
            done.display()
        );
        let script = format!("tmux new-session -d -s {case} \"{prefix}$FAR\"");
        let out = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .envs(env_of(&f.command("claude", caller)))
            .env("TMUX_TMPDIR", &server.dir)
            .env("AMP", &amp)
            .env("WORK", &f.work)
            .env("FAR", &far)
            .output()
            .unwrap();
        assert!(out.status.success(), "{case}: {}", describe(&out));
        crate::support::wait::wait_until(&format!("the {case} run to finish"), || done.exists());
        for name in ["call.1", "call.2"] {
            assert!(f.records.join(name).is_file(), "{case}: no {name}");
        }
        fs::read_to_string(err).unwrap()
    };
    let take_call = |step: u32| {
        let call = f.call(step);
        fs::remove_file(f.records.join(format!("call.{step}"))).unwrap();
        call
    };
    let hand_off = "$(\"$AMP\" agent-binary --shell -w \"$WORK\") ";

    // Control: a plain `tmux new-session` gives the run the server's values,
    // from the caller holding others. This is the leak.
    let stderr = start_run("control", &CALLER, "");
    assert!(
        stderr.contains(&format!(
            "amplihack: steps inherit effort=low model={Y} (from the parent amplihack launch)"
        )),
        "{stderr}"
    );
    for step in [1, 2] {
        let args = argv(&take_call(step));
        assert_eq!(flag(&args, "--model").as_deref(), Some(Y), "{args:?}");
        assert_eq!(flag(&args, "--effort").as_deref(), Some("low"), "{args:?}");
    }

    // The hand-off: every step runs the caller's model at the caller's effort.
    let stderr = start_run("handoff", &CALLER, hand_off);
    assert!(
        stderr.contains(&format!(
            "amplihack: steps inherit effort=xhigh model={X} (from the parent amplihack launch)"
        )),
        "{stderr}"
    );
    for step in [1, 2] {
        let call = take_call(step);
        let args = argv(&call);
        assert_eq!(flag(&args, "--model").as_deref(), Some(X), "{args:?}");
        assert_eq!(
            flag(&args, "--effort").as_deref(),
            Some("xhigh"),
            "{args:?}"
        );
        assert_eq!(call["CLAUDE_CODE_EFFORT_LEVEL"], "xhigh", "step {step}");
        assert_eq!(call["ANTHROPIC_MODEL"], X, "step {step}");
        assert_eq!(call["AMPLIHACK_PARENT_MODEL"], X, "step {step}");
        assert!(!format!("{call:?}").contains(Y), "step {step}: {call:?}");
    }

    // A caller with no values (a plain shell, cron): the run inherits
    // nothing, as it would started there with `setsid`, and the steps get
    // amplihack's default model and no effort.
    let stderr = start_run("plain", &[], hand_off);
    assert!(!stderr.contains("steps inherit"), "{stderr}");
    for step in [1, 2] {
        let call = take_call(step);
        let args = argv(&call);
        assert_eq!(
            flag(&args, "--model").as_deref(),
            Some(DEFAULT_MODEL),
            "{args:?}"
        );
        assert_eq!(count(&args, "--effort"), 0, "{args:?}");
        for v in [
            "CLAUDE_CODE_EFFORT_LEVEL",
            "ANTHROPIC_MODEL",
            "AMPLIHACK_PARENT_EFFORT",
            "AMPLIHACK_PARENT_MODEL",
            "AMPLIHACK_PARENT_MODEL_BINARY",
        ] {
            assert!(
                !names(&call).contains(&v.to_string()),
                "step {step} has {v}"
            );
        }
    }
    drop(server);
}
