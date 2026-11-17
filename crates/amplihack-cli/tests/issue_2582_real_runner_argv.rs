//! Issue #2582, the acceptance condition (#2658's remaining one): no recipe
//! context value and no prompt text is on the command line of any process in
//! a live run's tree, through the pinned runner.
//!
//! This is the behaviour test the issue asks for, on real processes: not a
//! text scan of the source, and not a stub runner. It drives the real
//! `amplihack` binary against the runner built at the revision in
//! `claude-plugin/recipe-runner.rev` (`crate::pinned_runner`), over a tree
//! that holds every process kind the issue and its comments name:
//!
//! ```text
//! amplihack recipe run (outer)      --context-file <private file>
//!   recipe-runner-rs (outer)        --context-file <private file>
//!     sh (a bash step)
//!       amplihack recipe run (nested)   --context-file <private file>
//!         recipe-runner-rs (nested)     --context-file <private file>
//!           [a sub-recipe step, run by that runner]
//!             amplihack claude --        the agent wrapper
//!               claude                   a fake agent CLI of this test's own
//! ```
//!
//! The fake agent blocks on a FIFO, so the whole tree is live and complete
//! while the walk runs (`support::fifo_release`, whose rendezvous is the
//! FIFO open itself: no sleep, deadline, poll, timer or retry anywhere, and
//! no process is signalled). The walk is read-only over
//! `/proc/<pid>/task/<tid>/children` and `/proc/<pid>/cmdline`
//! (`support::proc_tree`).
//!
//! What it asserts, in order:
//! 1. the resolved runner's own `--capabilities` lists `context_file_v1`, so
//!    a run against an older runner fails here rather than passing for the
//!    wrong reason;
//! 2. the tree is at least as large as the diagram, holds two runners, two
//!    `recipe run` processes and the fake agent — an empty or truncated walk
//!    (a kernel without `CONFIG_PROC_CHILDREN`, an agent step that never
//!    ran) fails instead of passing vacuously;
//! 3. no argv element of any process in it contains any context value, any
//!    sentinel token, or `--set`;
//! 4. the agent did receive the task, on stdin or in a file — so (3) cannot
//!    be satisfied by the task never arriving;
//! 5. both context files are gone when the runs end, no run report holds a
//!    value, and neither stream of the run prints one.
//!
//! One value is excluded from (5) and only from (5): `task_description` is in
//! the run correlation record (the `amplihack.recipe.log_pointer` events, by
//! name, since #1606), so it reaches this run's own stderr and its 0600 log.
//! That is the record doing its job, on a channel no other account can read
//! and `pkill -f` cannot match. It is not what this issue is about, and this
//! test does not pretend otherwise.
//!
//! There is no skip path and no `#[ignore]`. If a host cannot resolve the
//! pinned runner or read `children`, that is a fault to fix on the host, not
//! a case to pass over: a skip here would go green against a runner with no
//! `context_file_v1` and prove nothing.

#![cfg(target_os = "linux")]

use crate::isolated_amplihack::HOME_GUARD_ENV;
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::{fifo_release, proc_tree};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The context this run carries. Each value has a unique 16-character token,
/// so a match on argv can only come from this test. `task_description` also
/// holds the two phrases whose pattern kills stopped live runs — `vitest`
/// (#2582) and `scripts/check full` (#2674) — because that is the shape of
/// the text that must never be matchable.
const VALUES: &[(&str, &str)] = &[
    (
        "task_description",
        "ZQXV2582REALWORD rebuild the widget: vitest hangs and scripts/check full is slow",
    ),
    (
        "pr_body",
        "ZQXV2582BODYWORD {\"json\": [1, 2]} $(touch pwned) `x`",
    ),
    ("note", "ZQXV2582NOTEWORD second line\nthird line"),
];

const TOKENS: &[&str] = &["ZQXV2582REALWORD", "ZQXV2582BODYWORD", "ZQXV2582NOTEWORD"];

/// The capability the pinned runner must report for the file transport.
const CONTEXT_FILE_CAPABILITY: &str = "context_file_v1";

/// The outer recipe: one bash step that runs a nested `amplihack recipe run`
/// in the foreground, so the nested run is part of this tree rather than a
/// reparented orphan. It reads the context from the environment mirror the
/// runner exports, checks the value arrived, and passes it on by file — never
/// as `-c key=value`, which would put it back on the nested run's argv.
const OUTER_RECIPE: &str = r#"name: "issue-2582-outer"
description: "a nested run and an agent step, live at the same time"
version: "1.0.0"
context:
  task_description: ""
  pr_body: ""
  note: ""
steps:
  - id: "nested-run"
    type: "bash"
    command: |
      set -eu
      printf '%s' {{task_description}} > "$AMPLIHACK_2582_RECORD_DIR/outer.task"
      case {{task_description}} in
        *ZQXV2582REALWORD*) ;;
        *) echo "the outer context did not reach the bash step" >&2; exit 1 ;;
      esac
      HOME="$AMPLIHACK_2582_HOME" \
      PATH="$AMPLIHACK_2582_PATH" \
      RECIPE_RUNNER_RS_PATH="$AMPLIHACK_2582_RUNNER" \
        "$AMPLIHACK_2582_AMPLIHACK" recipe run "$AMPLIHACK_2582_INNER_RECIPE" \
          --context-file "$AMPLIHACK_2582_INNER_CONTEXT" \
          -w "$AMPLIHACK_2582_PROJECT" \
          --no-detach
"#;

/// The nested run's recipe: a sub-recipe step, so the leaf runs inside the
/// nested runner rather than beside it.
const INNER_RECIPE: &str = r#"name: "issue-2582-inner"
description: "a sub-recipe step"
version: "1.0.0"
context:
  task_description: ""
  pr_body: ""
  note: ""
steps:
  - id: "leaf"
    type: "recipe"
    recipe: "issue-2582-leaf"
    context:
      task_description: "{{task_description}}"
      pr_body: "{{pr_body}}"
"#;

/// The leaf recipe: the agent step. Its prompt carries the task text, which
/// is what the runner must hand over by stdin or by file rather than on argv.
const LEAF_RECIPE: &str = r#"name: "issue-2582-leaf"
description: "one agent step"
version: "1.0.0"
context:
  task_description: ""
  pr_body: ""
steps:
  - id: "agent-step"
    type: "agent"
    agent: "amplihack:core:builder"
    prompt: |
      Record this task and stop. Change nothing.
      Task: {{task_description}}
      Body: {{pr_body}}
"#;

/// The agent an agent step names. Its text becomes the system prompt.
const AGENT_MD: &str = "---\nname: builder\ndescription: a fixture agent\n---\n\nRecord the task \
                        you are given and stop.\n";

/// A fake agent CLI. It answers the launcher's `--version` probe, records
/// what it was given, blocks on the FIFO so the tree stays live, then drains
/// its stdin and exits 0.
///
/// Draining stdin *after* the release matters twice: the runner treats an
/// agent that exits before its whole task is written as a failure, and a
/// writer blocked on a full pipe keeps the wrapper alive, which is what the
/// walk needs. Only the first run blocks, so a second invocation (a probe,
/// a retry) cannot wait for a release byte that has already been consumed.
const AGENT_STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' "$AMPLIHACK_2582_STUB_VERSION"
  exit 0
fi
rec="$AMPLIHACK_2582_RECORD_DIR"
cp "/proc/$$/cmdline" "$rec/agent.cmdline.bin" 2>/dev/null
: > "$rec/agent.argv.bin"
prev=
for a in "$@"; do
  printf '%s\0' "$a" >> "$rec/agent.argv.bin"
  case "$prev" in
    --*-file) [ -f "$a" ] && cp -- "$a" "$rec/agent.content.${prev#--}" ;;
  esac
  prev=$a
done
if [ ! -e "$rec/agent.blocked" ]; then
  : > "$rec/agent.blocked"
  read _ < "$AMPLIHACK_2582_FIFO"
fi
cat > "$rec/agent.stdin"
printf 'stub-ok\n'
exit 0
"#;

/// The repository root, two levels above this crate's manifest.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// The runner built at the pinned revision, checked to report the capability
/// this issue's fix depends on. No skip: a runner without it would take the
/// `--set` path and this test would assert nothing.
fn pinned_runner_with_the_capability() -> PathBuf {
    let runner = crate::pinned_runner::pinned_runner(&repo_root());
    let report = Command::new(&runner)
        .arg("--capabilities")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{} --capabilities: {e}", runner.display()));
    assert!(
        report.status.success(),
        "{} --capabilities failed: {}",
        runner.display(),
        String::from_utf8_lossy(&report.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap_or_else(|e| {
        panic!(
            "{} --capabilities is not JSON: {e}: {}",
            runner.display(),
            String::from_utf8_lossy(&report.stdout)
        )
    });
    let capabilities: Vec<&str> = json["capabilities"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        capabilities.contains(&CONTEXT_FILE_CAPABILITY),
        "the pinned runner at {} reports {capabilities:?}, without {CONTEXT_FILE_CAPABILITY}: \
         this test would prove nothing against it. Move the pin, or supply the pinned runner.",
        runner.display()
    );
    runner
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
    records: PathBuf,
    tools: PathBuf,
    work_root: PathBuf,
    runner: PathBuf,
    agent: PathBuf,
    outer_recipe: PathBuf,
    outer_context: PathBuf,
    fifo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let runner = pinned_runner_with_the_capability();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let records = root.join("records");
        let tools = root.join("tools");
        let work_root = root.join("work");
        let recipes = root.join("recipes");
        let agents = root.join(".claude/agents/amplihack/core");
        for d in [
            &home,
            &records,
            &tools,
            &work_root,
            &recipes,
            &agents,
            &home.join("profiles"),
        ] {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(agents.join("builder.md"), AGENT_MD).unwrap();

        // Both runs work in `project`, a real Git repository: since #2096 the
        // checkout discovery fails closed without one, and #2176's Git
        // recording otherwise ends the run with "required observer did not
        // finalize". It is its own directory rather than `root` because the
        // work root holds each run's private checkout, and a work root inside
        // the repository being isolated is refused ("unsupported local
        // input ... checkout/"). This fixture is about argv, not Git, so it
        // takes the shared helper's repository and nothing more.
        let project = root.join("project");
        crate::support::git_fixture::init_with_commit(&project);
        fs::set_permissions(&project, fs::Permissions::from_mode(0o700)).unwrap();

        let agent = tools.join("claude");
        crate::support::exec_stub::write_executable(&agent, AGENT_STUB, 0o700);

        let outer_recipe = recipes.join("issue-2582-outer.yaml");
        fs::write(&outer_recipe, OUTER_RECIPE).unwrap();
        fs::write(recipes.join("issue-2582-inner.yaml"), INNER_RECIPE).unwrap();
        fs::write(recipes.join("issue-2582-leaf.yaml"), LEAF_RECIPE).unwrap();

        let outer_context = root.join("outer-context.json");
        let inner_context = root.join("inner-context.json");
        let object: serde_json::Map<String, serde_json::Value> = VALUES
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect();
        let bytes = serde_json::to_vec(&object).unwrap();
        for path in [&outer_context, &inner_context] {
            fs::write(path, &bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        let fifo = records.join("agent-release.fifo");
        fifo_release::mkfifo(&fifo);

        Self {
            _tmp: tmp,
            root,
            project,
            home,
            records,
            tools,
            work_root,
            runner,
            agent,
            outer_recipe,
            outer_context,
            fifo,
        }
    }

    /// `amplihack recipe run <outer> --context-file <file> -w <root>`, with
    /// an isolated home and the fake agent behind the real wrapper.
    ///
    /// The run's own context reaches it by file, so no value is on this
    /// test's command line either and the root of the walk is clean.
    /// Everything the bash step and the fake agent need is passed under an
    /// `AMPLIHACK_2582_` name: the runner keeps `AMPLIHACK_*` when it bounds
    /// a step's environment, so these survive into both.
    fn command(&self) -> Command {
        let path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display());
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .arg(&self.outer_recipe)
            .arg("--context-file")
            .arg(&self.outer_context)
            .arg("-w")
            .arg(&self.project)
            .arg("--no-detach")
            .current_dir(&self.project)
            .env(HOME_GUARD_ENV, "1")
            .env("PATH", &path)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("AMPLIHACK_WORK_ROOT", &self.work_root)
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", &self.agent)
            // The runner starts an agent step through the wrapper, which it
            // looks for as plain `amplihack` on PATH unless named here. This
            // test's PATH holds only its own tools, so the real wrapper is
            // named by full path: the point is to keep the real
            // `amplihack claude` in the tree and read its argv, which is what
            // the 13 November comment on #2582 asks for.
            .env("AMPLIHACK_LAUNCHER_BINARY", amplihack_bin())
            // Read by the bash step and the fake agent.
            .env("AMPLIHACK_2582_AMPLIHACK", amplihack_bin())
            .env("AMPLIHACK_2582_RUNNER", &self.runner)
            .env("AMPLIHACK_2582_PROJECT", &self.project)
            .env("AMPLIHACK_2582_HOME", &self.home)
            .env("AMPLIHACK_2582_PATH", &path)
            .env("AMPLIHACK_2582_RECORD_DIR", &self.records)
            .env("AMPLIHACK_2582_FIFO", &self.fifo)
            .env("AMPLIHACK_2582_STUB_VERSION", "2.1.319 (Claude Code)")
            .env(
                "AMPLIHACK_2582_INNER_RECIPE",
                self.root.join("recipes").join("issue-2582-inner.yaml"),
            )
            .env(
                "AMPLIHACK_2582_INNER_CONTEXT",
                self.root.join("inner-context.json"),
            )
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn record(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.records.join(name)).ok()
    }

    /// Every `run.json` under `dir`, one per run in the tree.
    fn run_reports(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = entry.file_type().expect("file type");
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                out.extend(Self::run_reports(&path));
            } else if entry.file_name() == "run.json" {
                out.push(path);
            }
        }
        out
    }

    /// Every `<runtime>/context` folder left anywhere under the work root.
    fn leftover_context_dirs(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|k| k.is_dir()) {
                continue;
            }
            if entry.file_name() == "context" {
                out.push(path.clone());
            }
            out.extend(Self::leftover_context_dirs(&path));
        }
        out
    }
}

/// The needles a walk must not find: every token and every whole value.
fn needles() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = TOKENS.to_vec();
    all.extend(VALUES.iter().map(|(_, value)| *value));
    all
}

/// How many processes in `tree` have `argv` elements `a` then `b`, adjacent.
fn pairs(tree: &[proc_tree::Process], a: &str, b: &str) -> usize {
    tree.iter()
        .filter(|(_, argv)| {
            argv.windows(2)
                .any(|w| w[0].as_slice() == a.as_bytes() && w[1].as_slice() == b.as_bytes())
        })
        .count()
}

/// How many processes in `tree` name `path` as one argv element.
fn naming(tree: &[proc_tree::Process], path: &Path) -> usize {
    let wanted = path.as_os_str().as_bytes();
    tree.iter()
        .filter(|(_, argv)| argv.iter().any(|a| a.as_slice() == wanted))
        .count()
}

#[test]
fn no_context_value_or_prompt_text_on_any_argv_of_a_real_run() {
    let f = Fixture::new();
    let mut live = fifo_release::start(f.command(), &f.fifo, "amplihack recipe run");

    let tree = live.tree();
    let shown = fifo_release::show(&tree);

    // The walk must have reached the whole tree. Without these, an agent step
    // that never launched — or a kernel without CONFIG_PROC_CHILDREN, which
    // makes the walk return the root alone — would pass every assertion
    // below for the wrong reason.
    assert!(
        tree.len() >= 6,
        "the walk found {} processes, fewer than the run's own tree:\n{shown}",
        tree.len()
    );
    assert!(
        naming(&tree, &f.runner) >= 2,
        "expected the outer and the nested recipe-runner-rs:\n{shown}"
    );
    assert!(
        pairs(&tree, "recipe", "run") >= 2,
        "expected the outer and the nested `amplihack recipe run`:\n{shown}"
    );
    assert!(
        pairs(&tree, "claude", "--") >= 1,
        "the `amplihack claude --` wrapper is not in the tree, so its command \
         line — the one the 13 November comment on #2582 names — was never \
         read:\n{shown}"
    );
    assert!(
        naming(&tree, &f.agent) >= 1,
        "the agent step's own process is not in the tree:\n{shown}"
    );

    // The condition itself.
    let hits = proc_tree::matches(&tree, &needles());
    assert!(
        hits.is_empty(),
        "context or prompt text on argv: {hits:?}\ntree:\n{shown}"
    );
    let sets: Vec<(u32, String)> = proc_tree::flags(&tree, "--set");
    assert!(
        sets.is_empty(),
        "a --set argument survived on argv: {sets:?}\ntree:\n{shown}"
    );

    // Both runs must be carrying their context in a private file while they
    // run, so the absence above is the transport working and not an empty
    // context.
    let files = proc_tree::flags(&tree, "--context-file");
    assert!(
        files.len() >= 2,
        "expected --context-file on the outer and the nested runner: {files:?}\ntree:\n{shown}"
    );

    let output = live.finish();
    assert!(
        output.status.success(),
        "the run must succeed:\n{}",
        fifo_release::describe(&output)
    );

    // The task did reach the agent — on stdin or in a file it was handed, as
    // the channel reference says, and never on its argv.
    let argv = f
        .record("agent.argv.bin")
        .expect("the fake agent was never run");
    let argv_text = String::from_utf8_lossy(&argv).into_owned();
    for token in TOKENS {
        assert!(
            !argv_text.contains(token),
            "the agent's own argv holds {token}: {argv_text:?}"
        );
    }
    let mut delivered = f.record("agent.stdin").unwrap_or_default();
    for name in [
        "agent.content.system-prompt-file",
        "agent.content.prompt-file",
    ] {
        if let Some(extra) = f.record(name) {
            delivered.extend(extra);
        }
    }
    let delivered = String::from_utf8_lossy(&delivered).into_owned();
    assert!(
        delivered.contains("ZQXV2582REALWORD"),
        "the agent step never received the task, so the walk proved nothing: {delivered:?}"
    );

    // Nothing is left on disk: each run removes its context file and the
    // folder it made for it. One `run.json` per run in the tree.
    let leftovers = Fixture::leftover_context_dirs(&f.work_root);
    assert!(
        leftovers.is_empty(),
        "a context folder outlived its run: {leftovers:?}"
    );
    let reports = Fixture::run_reports(&f.work_root);
    assert!(
        reports.len() >= 2,
        "expected a run report for the outer and the nested run, so this check \
         cannot pass vacuously: {reports:?}"
    );
    for path in &reports {
        let text = String::from_utf8_lossy(&fs::read(path).unwrap_or_default()).into_owned();
        for token in TOKENS {
            assert!(
                !text.contains(token),
                "{} holds the context value {token}",
                path.display()
            );
        }
    }

    // The run's own streams. `task_description` is excluded on purpose: both
    // log-pointer events carry it by name, as the run correlation record has
    // since #1606, and that is the point of the record. It is this process's
    // own stderr and its 0600 log, neither of which another account can read
    // and neither of which `pkill -f` matches, so it is not this issue's
    // exposure. Every other value must be absent from both streams.
    let quiet: Vec<&str> = TOKENS
        .iter()
        .copied()
        .filter(|t| *t != "ZQXV2582REALWORD")
        .collect();
    for (name, stream) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let text = String::from_utf8_lossy(stream).into_owned();
        for token in &quiet {
            assert!(
                !text.contains(token),
                "the run printed the context value {token} on {name}: {text}"
            );
        }
    }
}
