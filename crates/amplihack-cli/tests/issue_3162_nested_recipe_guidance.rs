//! Issue #3162, the deputy's real case of 24 November: guidance sent by
//! session name to a smart-orchestrator run must reach the next agent step of
//! the default-workflow recipe that run is executing, not only an agent step
//! of the outer recipe.
//!
//! Real `amplihack` binary and the runner built at the pinned revision
//! (`crate::pinned_runner`). The recipes have the shape of the real ones:
//!
//! ```text
//! issue-3162-orchestrator          (smart-orchestrator)
//!   routing   type: recipe  ->  issue-3162-routing      (smart-execute-routing)
//!     workflow  type: recipe  ->  issue-3162-workflow   (default-workflow)
//!       design   type: agent   the first agent step
//!       hold     type: bash    blocks on a FIFO
//!       build    type: agent   the next agent step
//! ```
//!
//! The runner runs both sub-recipes itself, so there is one `recipe run` and
//! one run folder, as in the real case. Each agent step starts the way the
//! pinned runner starts every Claude step:
//! `amplihack claude -- --system-prompt-file <file> -p --add-dir .`, with the
//! step's prompt on standard input. The agent CLI is a stub of this test that
//! records its standard input and the folder list it was given.
//!
//! The test sends guidance by session name while the run is blocked in
//! `hold`, then releases it. The FIFO open is the rendezvous, so there is no
//! sleep, poll or timer, and the test waits for the run to finish.
//!
//! It checks values only: the folder written, the status line, where the
//! block sits in each step's prompt, the folder list each agent step was
//! given, and the `guidance_injected` record. None of it compares prose.
//!
//! What the case's own files show (PR #3190 review). The run folder
//! (`runs/1e043186-…`) and its recipe log were read, read-only, on the host
//! that ran them. `guidance.log` holds three `guidance_appended` records
//! (18:19:12Z, 18:20:48Z, 18:27:00Z) and no `guidance_injected` record. Every
//! agent step of the run, before and after those times, was launched as
//! `amplihack:claude`. The runner deletes a step's standard error when the
//! step succeeds, and every step after 18:27Z succeeded but the last:
//! `step-08-implement`, cancelled by signal 15 at 18:59:41Z, whose kept
//! standard error holds the launcher's line "operator guidance not added:
//! this claude launch has no prompt it can follow". That line is printed by
//! the launcher of 1.2.215 to 1.2.219 for the runner's `claude -- … -p` form
//! with the prompt on standard input. The cause is therefore read from the
//! case's files, not only inferred: the launch dropped the block, which is
//! what this test reproduces with the launcher of those releases.
//!
//! Mixed versions. Two further tests start the run with an older release's
//! `amplihack` (`recipe run`), then send and read guidance with the binary
//! under test, as happens when a release is installed while a run is going:
//!
//! - the older release's launcher still starts the steps: the entry is
//!   reported as not yet read, never as read, and once the run has ended the
//!   reader says so;
//! - this release's launcher starts the steps (the runner starts each step
//!   through `amplihack` on `PATH`, which the new release replaces): the
//!   next agent step takes the block and the entry is reported as read.
//!
//! They need an older release binary, which CI does not have, so they are
//! marked `#[ignore]` and run by hand before a release:
//!
//! ```console
//! $ AMPLIHACK_3162_OLD_AMPLIHACK=~/.amplihack/releases/1.2.215/bin/amplihack \
//!     cargo test -p amplihack-cli --test cli_integration -- --ignored issue_3162_nested_recipe_guidance::mixed_
//! ```
//!
//! Run explicitly without the variable, they fail and say what is missing.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::fifo_release;
use crate::support::recipe_guide::{
    BEGIN, END, GuideFixture, drop_lines, private_dir, private_file, show, status_lines, stdout,
    written_lines,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MARKER: &str = "MARKER-3162-nested-use-the-revised-design";
const SESSION: &str = "amp-smart-orchestrator-build-fixture-1487";
const DESIGN_PROMPT: &str = "DESIGN-STEP-PROMPT write the design";
const BUILD_PROMPT: &str = "BUILD-STEP-PROMPT build from the design";

const ORCHESTRATOR: &str = r#"name: "issue-3162-orchestrator"
description: "an outer recipe that routes to a workflow, as smart-orchestrator does"
version: "1.0.0"
context: {}
steps:
  - id: "routing"
    type: "recipe"
    recipe: "issue-3162-routing"
"#;

const ROUTING: &str = r#"name: "issue-3162-routing"
description: "runs the workflow, as smart-execute-routing runs default-workflow"
version: "1.0.0"
context: {}
steps:
  - id: "workflow"
    type: "recipe"
    recipe: "issue-3162-workflow"
"#;

/// The nested workflow. `hold` records the run folder, then blocks until
/// the test has sent its guidance.
const WORKFLOW: &str = r#"name: "issue-3162-workflow"
description: "two agent steps with a pause between them, as in default-workflow"
version: "1.0.0"
context: {}
steps:
  - id: "design"
    type: "agent"
    agent: "amplihack:core:builder"
    prompt: "DESIGN-STEP-PROMPT write the design"
  - id: "hold"
    type: "bash"
    command: |
      set -eu
      printf '%s\n' "$AMPLIHACK_RUN_DIR" > "$AMPLIHACK_3162_RECORD_DIR/run_dir"
      read _ < "$AMPLIHACK_3162_FIFO"
  - id: "build"
    type: "agent"
    agent: "amplihack:core:builder"
    prompt: "BUILD-STEP-PROMPT build from the design"
"#;

/// The agent an agent step names. Its text becomes the system prompt.
const AGENT_MD: &str = "---\nname: builder\ndescription: a fixture agent\n---\n\nDo the step.\n";

/// The agent CLI. It answers the launcher's `--version` probe. Otherwise it
/// numbers itself, copies all of its standard input to `prompt.<n>` and the
/// folder list it was given to `dirs.<n>`, and exits 0.
const AGENT_STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo '2.1.319 (Claude Code)'
  exit 0
fi
rec="$AMPLIHACK_3162_RECORD_DIR"
n=$(( $(cat "$rec/agent.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$rec/agent.count"
cat > "$rec/prompt.$n"
printf '%s' "${AMPLIHACK_RUN_GUIDANCE_DIRS-<unset>}" > "$rec/dirs.$n"
echo 'stub-ok'
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

struct Nested {
    f: GuideFixture,
    records: PathBuf,
    recipes: PathBuf,
    project: PathBuf,
    agent: PathBuf,
    fifo: PathBuf,
}

impl Nested {
    fn new() -> Self {
        let f = GuideFixture::new();
        let records = f.root.join("records-3162");
        let recipes = f.root.join("recipes-3162");
        private_dir(&records);
        private_dir(&recipes);
        for (name, body) in [
            ("issue-3162-orchestrator", ORCHESTRATOR),
            ("issue-3162-routing", ROUTING),
            ("issue-3162-workflow", WORKFLOW),
        ] {
            private_file(&recipes.join(format!("{name}.yaml")), body);
        }
        // The runner looks for agents under `$HOME/.amplihack/.claude/agents`.
        private_file(
            &f.home
                .join(".amplihack/.claude/agents/amplihack/core/builder.md"),
            AGENT_MD,
        );
        let agent = f.root.join("agent-3162").join("claude");
        private_dir(agent.parent().unwrap());
        crate::support::exec_stub::write_executable(&agent, AGENT_STUB, 0o700);
        let project = f.root.join("project");
        crate::support::git_fixture::init_with_commit(&project);
        let fifo = records.join("release.fifo");
        fifo_release::mkfifo(&fifo);
        Self {
            f,
            records,
            recipes,
            project,
            agent,
            fifo,
        }
    }

    /// `amplihack recipe run <orchestrator> -w <project> --no-detach` through
    /// the pinned runner, with the real launcher and the stub agent CLI.
    fn command(&self) -> Command {
        self.command_with(&amplihack_bin(), &amplihack_bin())
    }

    /// [`Self::command`] with `recipe run` started by `recipe_run` and each
    /// agent step started through `launcher`.
    fn command_with(&self, recipe_run: &Path, launcher: &Path) -> Command {
        let runner = crate::pinned_runner::pinned_runner(&repo_root());
        let mut c = self.f.amplihack_at(recipe_run, &["recipe", "run"]);
        c.arg(self.recipes.join("issue-3162-orchestrator.yaml"))
            .arg("-w")
            .arg(&self.project)
            .arg("--no-detach")
            .current_dir(&self.project)
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", &self.agent)
            // The runner starts an agent step through `amplihack`, which the
            // fixture's PATH does not hold: name the launcher.
            .env("AMPLIHACK_LAUNCHER_BINARY", launcher)
            // `AMPLIHACK_*` names survive the runner's bound on a step's
            // environment, so the bash step and the stub read these.
            .env("AMPLIHACK_3162_RECORD_DIR", &self.records)
            .env("AMPLIHACK_3162_FIFO", &self.fifo)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // The session log a detached run has: the pointer `recipe guide`
            // reads is the one this `recipe run` writes, with the outer run's
            // own run id and folder.
            .stderr(self.f.session_log_sink(SESSION));
        c
    }

    fn record(&self, name: &str) -> String {
        fs::read_to_string(self.records.join(name))
            .unwrap_or_else(|e| panic!("record {name} missing: {e}"))
    }

    /// The run's standard error, which is its session log, for failure
    /// messages.
    fn session_log(&self) -> String {
        format!("session log:\n{}", self.f.session_log_text(SESSION))
    }

    fn guidance_show_json(&self) -> serde_json::Value {
        let o = self.f.run_with_stdin(
            self.f
                .amplihack(&["recipe", "guidance", "show", SESSION, "--json"]),
            None,
        );
        assert!(o.status.success(), "{}", show(&o));
        serde_json::from_str(stdout(&o).trim()).unwrap_or_else(|e| panic!("{e}: {}", show(&o)))
    }
}

/// The folders a launch was given, in order.
fn listed(value: &str) -> Vec<PathBuf> {
    std::env::split_paths(value).collect()
}

#[test]
fn guidance_sent_by_session_name_reaches_the_next_agent_step_of_a_nested_recipe() {
    let n = Nested::new();
    let mut live = fifo_release::start(n.command(), &n.fifo, "amplihack recipe run");

    // The run is blocked in the nested workflow's `hold` step. Its session
    // log holds the early pointer the outer `recipe run` wrote, so the
    // sender finds the run the way it finds a detached one.
    let run_dir = PathBuf::from(n.record("run_dir").trim());
    let run_guidance = run_dir.join("guidance");

    // The first agent step ran before the guidance existed.
    assert_eq!(n.record("agent.count").trim(), "1");

    let o = n.f.guide_text(&[SESSION], MARKER);
    assert!(o.status.success(), "{}\n{}", show(&o), n.session_log());
    let written = written_lines(&o);
    assert_eq!(written.len(), 1, "{}", show(&o));
    assert_eq!(written[0].0.parent().unwrap(), run_guidance, "{}", show(&o));
    assert_eq!(status_lines(&o), ["appended; not yet read"], "{}", show(&o));
    assert!(drop_lines(&o).is_empty(), "{}", show(&o));

    let run = live.finish();
    let described = format!("{}\n{}", fifo_release::describe(&run), n.session_log());
    assert!(run.status.success(), "{described}");
    assert_eq!(n.record("agent.count").trim(), "2", "{described}");

    // Both agent steps of the nested workflow were given the run's own
    // guidance folder, first in the list.
    for step in ["1", "2"] {
        let dirs = listed(&n.record(&format!("dirs.{step}")));
        assert_eq!(dirs.first(), Some(&run_guidance), "step {step}: {dirs:?}");
    }

    // The design step: its own prompt, and no block.
    let p1 = n.record("prompt.1");
    assert!(p1.contains(DESIGN_PROMPT), "{p1:?}");
    assert!(!p1.contains(BEGIN) && !p1.contains(MARKER), "{p1:?}");

    // The build step: its own prompt first, then exactly one block holding
    // the guidance, and nothing after it.
    let p2 = n.record("prompt.2");
    let own_end = p2.find(BUILD_PROMPT).expect("the build step's own prompt") + BUILD_PROMPT.len();
    assert_eq!(p2.matches(BEGIN).count(), 1, "{p2:?}");
    assert_eq!(p2.matches(END).count(), 1, "{p2:?}");
    let begin = p2.find(BEGIN).unwrap();
    assert!(begin >= own_end, "{p2:?}");
    assert!(p2.find(MARKER).unwrap() > begin, "{p2:?}");
    assert!(
        p2.trim_end().ends_with(END),
        "nothing after the block: {p2:?}"
    );

    // The run's log names the entry as read, once, without its text.
    let entry = written[0]
        .0
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let injected: Vec<_> =
        n.f.log(&run_guidance)
            .into_iter()
            .filter(|l| l["event"] == "guidance_injected")
            .collect();
    assert_eq!(injected.len(), 1, "{injected:?}");
    assert_eq!(injected[0]["entry_names"], serde_json::json!([entry]));
    let raw = fs::read_to_string(run_dir.join("guidance.log")).unwrap();
    assert!(!raw.contains(MARKER), "{raw}");

    // The sender's reader now reports the entry as read, and no drop.
    let shown = n.guidance_show_json();
    assert_eq!(shown["entries"][0]["read"], true, "{shown}");
    assert_eq!(shown["drops"], serde_json::json!([]), "{shown}");
}

// ---------------------------------------------------------------------------
// Mixed versions (PR #3190 review): a run started by an older release
// ---------------------------------------------------------------------------

/// The older release's `amplihack`, from `AMPLIHACK_3162_OLD_AMPLIHACK`.
fn old_release() -> PathBuf {
    let path = std::env::var_os("AMPLIHACK_3162_OLD_AMPLIHACK")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "set AMPLIHACK_3162_OLD_AMPLIHACK to an older release's amplihack, \
                 such as ~/.amplihack/releases/1.2.215/bin/amplihack"
            )
        });
    assert!(path.is_file(), "{} is not a file", path.display());
    path
}

/// Starts the run with `recipe run` from the older release and steps
/// launched by `launcher`, sends guidance with this binary while the nested
/// workflow is held, releases it and waits for the run. Returns the run's
/// guidance folder and the entry name.
fn run_started_by_the_old_release(n: &Nested, launcher: &Path) -> (PathBuf, String) {
    let old = old_release();
    let mut live = fifo_release::start(
        n.command_with(&old, launcher),
        &n.fifo,
        "older amplihack recipe run",
    );
    let run_dir = PathBuf::from(n.record("run_dir").trim());
    let run_guidance = run_dir.join("guidance");
    assert_eq!(n.record("agent.count").trim(), "1");

    // This binary finds the older run by the pointer the older `recipe run`
    // wrote, and writes to its run folder.
    let o = n.f.guide_text(&[SESSION], MARKER);
    assert!(o.status.success(), "{}\n{}", show(&o), n.session_log());
    let written = written_lines(&o);
    assert_eq!(written.len(), 1, "{}", show(&o));
    assert_eq!(written[0].0.parent().unwrap(), run_guidance, "{}", show(&o));
    assert_eq!(status_lines(&o), ["appended; not yet read"], "{}", show(&o));

    let run = live.finish();
    let described = format!("{}\n{}", fifo_release::describe(&run), n.session_log());
    assert!(run.status.success(), "{described}");
    assert_eq!(n.record("agent.count").trim(), "2", "{described}");
    let entry = written[0]
        .0
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    (run_guidance, entry)
}

#[test]
#[ignore = "needs an older release binary in AMPLIHACK_3162_OLD_AMPLIHACK; run by hand before a release"]
fn mixed_a_run_and_launcher_of_an_older_release_are_read_as_not_yet_read() {
    let n = Nested::new();
    let old = old_release();
    let (run_guidance, entry) = run_started_by_the_old_release(&n, &old);

    // The older launcher did not put the block after a stdin prompt.
    let p2 = n.record("prompt.2");
    assert!(p2.contains(BUILD_PROMPT), "{p2:?}");
    assert!(!p2.contains(MARKER), "{p2:?}");

    // It wrote no record naming the entry, so this binary never reports it
    // as read: the read state understates, it does not overstate.
    let log = n.f.log(&run_guidance);
    assert!(
        log.iter().all(|l| {
            l["event"] != "guidance_injected"
                || !l["entry_names"]
                    .as_array()
                    .is_some_and(|names| names.iter().any(|v| v == entry.as_str()))
        }),
        "{log:?}"
    );
    let shown = n.guidance_show_json();
    assert_eq!(shown["entries"][0]["name"], entry.as_str(), "{shown}");
    assert_eq!(shown["entries"][0]["read"], false, "{shown}");
    // The older release recorded its end in `run.json`; the reader says the
    // run ended before any step read the entry.
    assert!(
        shown["entries"][0]["run_ended"]["at"].is_string(),
        "{shown}"
    );
}

#[test]
#[ignore = "needs an older release binary in AMPLIHACK_3162_OLD_AMPLIHACK; run by hand before a release"]
fn mixed_a_run_of_an_older_release_takes_guidance_through_this_launcher() {
    let n = Nested::new();
    let (run_guidance, entry) = run_started_by_the_old_release(&n, &amplihack_bin());

    // The next agent step: its own prompt, then exactly one block.
    let p2 = n.record("prompt.2");
    let own_end = p2.find(BUILD_PROMPT).expect("the build step's own prompt") + BUILD_PROMPT.len();
    assert_eq!(p2.matches(BEGIN).count(), 1, "{p2:?}");
    assert!(p2.find(BEGIN).unwrap() >= own_end, "{p2:?}");
    assert!(p2.find(MARKER).unwrap() > p2.find(BEGIN).unwrap(), "{p2:?}");

    // The older run's folder list reached this launcher, which recorded the
    // read in the older run's log, and this binary reports it.
    let injected: Vec<_> =
        n.f.log(&run_guidance)
            .into_iter()
            .filter(|l| l["event"] == "guidance_injected")
            .collect();
    assert_eq!(injected.len(), 1, "{injected:?}");
    assert_eq!(injected[0]["entry_names"], serde_json::json!([entry]));
    let shown = n.guidance_show_json();
    assert_eq!(shown["entries"][0]["read"], true, "{shown}");
    assert_eq!(shown["drops"], serde_json::json!([]), "{shown}");
}
