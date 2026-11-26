//! Issues #3161 and #3162 through the runner built at the pinned revision
//! (`crate::pinned_runner`) and the real `amplihack` binary: a drive with an
//! explicit work root guided by pull request URL, a live drive that cannot
//! be reached, and a run that ends before any step reads its guidance.
//!
//! Each run is `amplihack recipe run <recipe> -c pr_url=<PR URL> -w
//! <project> --no-detach`, with its own state folder and an explicit work
//! root, neither of which is the sender's. Its standard error goes to
//! `<recipe-logs>/<session>.log`, as a detached session's does, so the
//! session pointer `recipe guide` reads is the one `recipe run` wrote, with
//! its own run id, run folder and pull request. Each agent step starts the
//! way the pinned runner starts every Claude step, `amplihack claude --
//! --system-prompt-file <file> -p --add-dir .`, with the step's prompt on
//! standard input. The agent CLI is a stub of this test that records its
//! standard input.
//!
//! The run blocks in a bash step on a FIFO while the test sends guidance.
//! The FIFO open is the rendezvous, so there is no sleep, poll or timer, and
//! the test waits for the run to finish.
//!
//! - (b) guidance sent by pull request URL reaches the drive's own run
//!   folder, and `recipe guidance show <PR URL>` under the drive's
//!   environment (the goal loop's read) lists that folder. The drive's next
//!   agent step takes one block after its own prompt. Once the drive has
//!   ended, its folder is no longer listed, and the sender's read reports the
//!   entry as read;
//! - (b2) a live drive whose run folder fails the trust rule: `recipe guide`
//!   exits 1, names the session and writes nothing;
//! - (e) guidance that arrives after the run's last agent step (the
//!   repo-f #396 case): the run ends with the entry unread,
//!   `recipe guidance show` reports it as not read with the run's end, and
//!   the next `recipe guide` to the session tells its sender that the run
//!   has ended.
//!
//! The pinned runner adds a fixed footer to every agent prompt, so a step it
//! starts always has text on standard input. The `no_prompt_on_stdin`,
//! `trailer_not_written` and Copilot drops are therefore covered with the
//! launcher run directly, in `issue_3162_stdin_prompt_guidance.rs` and
//! `issue_3162_copilot_stdin_launcher.rs`.
//!
//! The assertions compare paths, counts, exit codes, JSON fields, the
//! session name and the status value the sender is given. None compares the
//! wording of a message.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::fifo_release;
use crate::support::recipe_guide::{
    BEGIN, END, GuideFixture, PR_URL, private_dir, private_file, show, status_lines, stderr,
    stdout, written_lines,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

const MARKER: &str = "MARKER-3161-pinned-merge-only-on-a-conflict";
const SESSION: &str = "build-3143-review-and-merge-pinned";
const TURN_ONE: &str = "TURN-ONE-PROMPT read the pull request";
const TURN_TWO: &str = "TURN-TWO-PROMPT act on the reading";

/// Two turns of a drive with a pause between them.
const DRIVE: &str = r#"name: "issue-3161-drive"
description: "two agent turns with a pause between them, as a drive takes"
version: "1.0.0"
context:
  pr_url: ""
steps:
  - id: "turn-one"
    type: "agent"
    agent: "amplihack:core:builder"
    prompt: "TURN-ONE-PROMPT read the pull request"
  - id: "hold"
    type: "bash"
    command: |
      set -eu
      printf '%s\n' "$AMPLIHACK_RUN_DIR" > "$AMPLIHACK_3161_RECORD_DIR/run_dir"
      read _ < "$AMPLIHACK_3161_FIFO"
  - id: "turn-two"
    type: "agent"
    agent: "amplihack:core:builder"
    prompt: "TURN-TWO-PROMPT act on the reading"
"#;

/// One agent turn, then a pause, then the end: guidance sent in the pause
/// has no later agent step to read it.
const LAST_STEP: &str = r#"name: "issue-3161-last-step"
description: "an agent turn and then a pause at the end of the run"
version: "1.0.0"
context:
  pr_url: ""
steps:
  - id: "turn-one"
    type: "agent"
    agent: "amplihack:core:builder"
    prompt: "TURN-ONE-PROMPT read the pull request"
  - id: "hold"
    type: "bash"
    command: |
      set -eu
      printf '%s\n' "$AMPLIHACK_RUN_DIR" > "$AMPLIHACK_3161_RECORD_DIR/run_dir"
      read _ < "$AMPLIHACK_3161_FIFO"
"#;

/// The agent an agent step names. Its text becomes the system prompt.
const AGENT_MD: &str = "---\nname: builder\ndescription: a fixture agent\n---\n\nDo the step.\n";

/// The agent CLI. It answers the launcher's `--version` probe. Otherwise it
/// numbers itself, copies all of its standard input to `prompt.<n>`, and
/// exits 0.
const AGENT_STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo '2.1.319 (Claude Code)'
  exit 0
fi
rec="$AMPLIHACK_3161_RECORD_DIR"
n=$(( $(cat "$rec/agent.count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$rec/agent.count"
cat > "$rec/prompt.$n"
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

/// A run of `recipe` held in its `hold` step.
struct Held {
    f: GuideFixture,
    live: fifo_release::Live,
    records: PathBuf,
    drive_state: PathBuf,
    drive_work: PathBuf,
    run_dir: PathBuf,
}

/// Starts `recipe` (`DRIVE` or `LAST_STEP`) for `PR_URL` through the pinned
/// runner, and returns once it is blocked in `hold`.
fn start(recipe: &str) -> Held {
    let f = GuideFixture::new();
    let records = f.root.join("records-3161");
    let recipes = f.root.join("recipes-3161");
    let drive_state = f.root.join("drive-state");
    let drive_work = f.root.join("drive-elsewhere").join("work");
    for d in [
        &records,
        &recipes,
        &drive_state,
        drive_work.parent().unwrap(),
        &drive_work,
    ] {
        private_dir(d);
    }
    let recipe_file = recipes.join("recipe.yaml");
    private_file(&recipe_file, recipe);
    // The runner looks for agents under `$HOME/.amplihack/.claude/agents`.
    private_file(
        &f.home
            .join(".amplihack/.claude/agents/amplihack/core/builder.md"),
        AGENT_MD,
    );
    let agent = f.root.join("agent-3161").join("claude");
    private_dir(agent.parent().unwrap());
    crate::support::exec_stub::write_executable(&agent, AGENT_STUB, 0o700);
    let project = f.root.join("project");
    crate::support::git_fixture::init_with_commit(&project);
    let fifo = records.join("release.fifo");
    fifo_release::mkfifo(&fifo);

    let runner = crate::pinned_runner::pinned_runner(&repo_root());
    let context = format!("pr_url={PR_URL}");
    let mut c = f.amplihack(&["recipe", "run"]);
    c.arg(&recipe_file)
        .args(["-c", &context])
        .arg("-w")
        .arg(&project)
        .arg("--no-detach")
        .current_dir(&project)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &agent)
        // The runner starts an agent step through `amplihack`, which the
        // fixture's PATH does not hold: name the launcher.
        .env("AMPLIHACK_LAUNCHER_BINARY", amplihack_bin())
        // The drive's own state folder and explicit work root.
        .env("AMPLIHACK_STATE_DIR", &drive_state)
        .env("AMPLIHACK_WORK_ROOT", &drive_work)
        .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
        // `AMPLIHACK_*` names survive the runner's bound on a step's
        // environment, so the bash step and the stub read these.
        .env("AMPLIHACK_3161_RECORD_DIR", &records)
        .env("AMPLIHACK_3161_FIFO", &fifo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(f.session_log_sink(SESSION));
    let live = fifo_release::start(c, &fifo, "amplihack recipe run");

    let run_dir = PathBuf::from(read(&records, "run_dir").trim());
    assert!(
        run_dir.starts_with(&drive_work),
        "the run folder is under the drive's own work root: {}",
        run_dir.display()
    );
    assert_eq!(read(&records, "agent.count").trim(), "1");
    Held {
        f,
        live,
        records,
        drive_state,
        drive_work,
        run_dir,
    }
}

fn read(records: &Path, name: &str) -> String {
    fs::read_to_string(records.join(name)).unwrap_or_else(|e| panic!("record {name} missing: {e}"))
}

impl Held {
    fn run_guidance(&self) -> PathBuf {
        self.run_dir.join("guidance")
    }

    fn record(&self, name: &str) -> String {
        read(&self.records, name)
    }

    /// Releases the run, waits for it, and checks that it succeeded.
    fn finish(&mut self) -> Output {
        let run = self.live.finish();
        assert!(
            run.status.success(),
            "{}\nsession log:\n{}",
            fifo_release::describe(&run),
            self.f.session_log_text(SESSION)
        );
        run
    }

    /// `recipe guidance show <target> --json` with `env` added.
    fn show_json(&self, target: &str, env: &[(&str, &Path)]) -> serde_json::Value {
        let mut c = self
            .f
            .amplihack(&["recipe", "guidance", "show", target, "--json"]);
        for (k, v) in env {
            c.env(k, v);
        }
        c.env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS");
        let o = self.f.run_with_stdin(c, None);
        assert!(o.status.success(), "{}", show(&o));
        serde_json::from_str(stdout(&o).trim()).unwrap_or_else(|e| panic!("{e}: {}", show(&o)))
    }

    /// The goal loop's read: `recipe guidance show <PR URL> --json` with the
    /// drive's state folder and work root.
    fn goal_loop_show(&self) -> serde_json::Value {
        let explicit = PathBuf::from("1");
        self.show_json(
            PR_URL,
            &[
                ("AMPLIHACK_STATE_DIR", &self.drive_state),
                ("AMPLIHACK_WORK_ROOT", &self.drive_work),
                ("AMPLIHACK_WORK_ROOT_EXPLICIT", &explicit),
            ],
        )
    }
}

/// The folders a `show --json` document lists.
fn listed_folders(doc: &serde_json::Value) -> Vec<PathBuf> {
    doc["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| PathBuf::from(f["path"].as_str().unwrap()))
        .collect()
}

fn entry_name(path: &Path) -> String {
    path.file_name().unwrap().to_str().unwrap().to_string()
}

// ---------------------------------------------------------------------------
// (b) a drive with its own work root, guided by pull request URL
// ---------------------------------------------------------------------------

#[test]
fn b_pr_url_guidance_reaches_a_pinned_runner_drive_with_its_own_work_root() {
    let mut d = start(DRIVE);
    assert_ne!(d.drive_state, d.f.state);
    assert!(!d.run_dir.starts_with(&d.f.work), "{}", d.run_dir.display());

    let o = d.f.guide_text(&[PR_URL], MARKER);
    assert!(
        o.status.success(),
        "{}\nsession log:\n{}",
        show(&o),
        d.f.session_log_text(SESSION)
    );
    let written = written_lines(&o);
    let folders: Vec<PathBuf> = written
        .iter()
        .map(|(p, _, _)| p.parent().unwrap().to_path_buf())
        .collect();
    assert_eq!(written.len(), 2, "{}", show(&o));
    assert!(folders.contains(&d.run_guidance()), "{folders:?}");
    assert!(
        folders.contains(&d.f.pr_parent().join("guidance")),
        "{folders:?}"
    );
    assert_eq!(status_lines(&o), ["appended; not yet read"], "{}", show(&o));

    // The goal loop's read resolves the folder the sender's write used.
    let seen = d.goal_loop_show();
    assert!(listed_folders(&seen).contains(&d.run_guidance()), "{seen}");
    assert_eq!(seen["entries"].as_array().unwrap().len(), 1, "{seen}");
    assert_eq!(seen["entries"][0]["read"], false, "{seen}");

    d.finish();
    assert_eq!(d.record("agent.count").trim(), "2");

    // The first turn ran before the guidance existed.
    let p1 = d.record("prompt.1");
    assert!(p1.contains(TURN_ONE), "{p1:?}");
    assert!(!p1.contains(BEGIN) && !p1.contains(MARKER), "{p1:?}");

    // The next turn: its own prompt first, then exactly one block holding
    // the guidance, and nothing after it.
    let p2 = d.record("prompt.2");
    let own_end = p2.find(TURN_TWO).expect("the turn's own prompt") + TURN_TWO.len();
    assert_eq!(p2.matches(BEGIN).count(), 1, "{p2:?}");
    assert_eq!(p2.matches(END).count(), 1, "{p2:?}");
    assert_eq!(p2.matches(MARKER).count(), 1, "{p2:?}");
    let begin = p2.find(BEGIN).unwrap();
    assert!(begin >= own_end, "{p2:?}");
    assert!(p2.find(MARKER).unwrap() > begin, "{p2:?}");
    assert!(
        p2.trim_end().ends_with(END),
        "nothing after the block: {p2:?}"
    );

    // The run's log names the entry as read, once.
    let run_entry = written
        .iter()
        .find(|(p, _, _)| p.parent() == Some(d.run_guidance().as_path()))
        .map(|(p, _, _)| entry_name(p))
        .unwrap();
    let injected: Vec<_> =
        d.f.log(&d.run_guidance())
            .into_iter()
            .filter(|l| l["event"] == "guidance_injected")
            .collect();
    assert_eq!(injected.len(), 1, "{injected:?}");
    assert_eq!(injected[0]["entry_names"], serde_json::json!([run_entry]));

    // The drive has ended: its folder is no longer listed for the pull
    // request, so a later drive's turns are not given its guidance.
    let after = d.goal_loop_show();
    assert!(
        !listed_folders(&after).contains(&d.run_guidance()),
        "{after}"
    );

    // The sender's own read lists the entry once, from the pull request
    // folder, and reports it as read: the ended run's log proves the read.
    let sender = d.show_json(PR_URL, &[]);
    assert!(
        !listed_folders(&sender).contains(&d.run_guidance()),
        "{sender}"
    );
    assert_eq!(sender["entries"].as_array().unwrap().len(), 1, "{sender}");
    assert_eq!(
        PathBuf::from(sender["entries"][0]["folder"].as_str().unwrap()),
        d.f.pr_parent().join("guidance"),
        "{sender}"
    );
    assert_eq!(sender["entries"][0]["read"], true, "{sender}");
    assert_eq!(sender["drops"], serde_json::json!([]), "{sender}");
}

// ---------------------------------------------------------------------------
// (b2) a live drive that cannot be reached: refuse, name the session
// ---------------------------------------------------------------------------

#[test]
fn b2_an_unreachable_pinned_runner_drive_refuses_names_the_session_and_writes_nothing() {
    let mut d = start(DRIVE);
    // A group- and world-writable run folder fails the trust rule the run's
    // guidance folder is read under.
    let original = fs::metadata(&d.run_dir).unwrap().permissions().mode() & 0o7777;
    fs::set_permissions(&d.run_dir, fs::Permissions::from_mode(0o777)).unwrap();
    let o = d.f.guide_text(&[PR_URL], MARKER);
    fs::set_permissions(&d.run_dir, fs::Permissions::from_mode(original)).unwrap();

    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).trim().is_empty(), "{}", show(&o));
    assert!(stderr(&o).contains(SESSION), "{}", show(&o));
    assert!(!stderr(&o).contains(MARKER), "{}", show(&o));
    assert!(d.f.entries(&d.run_guidance()).is_empty());
    assert!(!d.f.pr_parent().exists(), "{}", d.f.pr_parent().display());

    d.finish();
    assert_eq!(d.record("agent.count").trim(), "2");
    let p2 = d.record("prompt.2");
    assert!(p2.contains(TURN_TWO), "{p2:?}");
    assert!(!p2.contains(BEGIN) && !p2.contains(MARKER), "{p2:?}");
}

// ---------------------------------------------------------------------------
// (e) guidance after the last agent step: the sender is told the run ended
// ---------------------------------------------------------------------------

#[test]
fn e_guidance_after_the_last_agent_step_is_reported_unread_with_the_runs_end() {
    let mut d = start(LAST_STEP);

    let o = d.f.guide_text(&[SESSION], MARKER);
    assert!(
        o.status.success(),
        "{}\nsession log:\n{}",
        show(&o),
        d.f.session_log_text(SESSION)
    );
    let written = written_lines(&o);
    assert_eq!(written.len(), 1, "{}", show(&o));
    assert_eq!(
        written[0].0.parent().unwrap(),
        d.run_guidance(),
        "{}",
        show(&o)
    );
    assert_eq!(status_lines(&o), ["appended; not yet read"], "{}", show(&o));

    d.finish();
    assert_eq!(d.record("agent.count").trim(), "1");
    assert!(!d.record("prompt.1").contains(MARKER));

    // No step read the entry, and none dropped it: there was no later step.
    let log = d.f.log(&d.run_guidance());
    assert!(
        log.iter()
            .all(|l| l["event"] != "guidance_injected" && l["event"] != "guidance_dropped"),
        "{log:?}"
    );

    // The sender's reader says the entry was not read and the run has ended.
    let shown = d.show_json(SESSION, &[]);
    assert_eq!(shown["entries"].as_array().unwrap().len(), 1, "{shown}");
    assert_eq!(
        shown["entries"][0]["name"],
        entry_name(&written[0].0).as_str(),
        "{shown}"
    );
    assert_eq!(shown["entries"][0]["read"], false, "{shown}");
    assert_eq!(
        shown["entries"][0]["run_ended"]["outcome"], "success",
        "{shown}"
    );
    let ended_at = shown["entries"][0]["run_ended"]["at"]
        .as_str()
        .unwrap_or_else(|| panic!("run_ended.at: {shown}"))
        .to_string();
    assert_eq!(shown["drops"], serde_json::json!([]), "{shown}");

    // The next `recipe guide` to the session gives its sender the same end.
    let next = d.f.guide_text(&[SESSION], "a second note");
    assert!(next.status.success(), "{}", show(&next));
    let status = status_lines(&next);
    assert_eq!(status.len(), 1, "{}", show(&next));
    assert!(
        status[0].starts_with("appended; not yet read; "),
        "{}",
        show(&next)
    );
    assert!(status[0].contains(&ended_at), "{}", show(&next));
    assert!(status[0].ends_with("(success)"), "{}", show(&next));
}
