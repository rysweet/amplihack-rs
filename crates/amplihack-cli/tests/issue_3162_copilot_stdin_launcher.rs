//! PR #3190 review: the launcher's half of the Copilot stdin route, checked
//! in CI.
//!
//! `amplihack copilot` with no `-p` or `-i` and a prompt piped on stdin takes
//! the `StdinTrailer` route. Two things must hold for the guidance to reach
//! the model:
//!
//! 1. amplihack hands Copilot the caller's prompt and then the block, and
//!    records the entry as read only when the block was written in full.
//! 2. Copilot takes the whole of its stdin as its prompt, block included.
//!
//! (2) is Copilot's behaviour. It needs a signed-in Copilot and a model call,
//! so it is checked by hand in `issue_3162_copilot_stdin_live.rs` and tracked
//! for an automated form in issue #3225. (1) is amplihack's, and is checked
//! here on every CI run with the real launcher and a stand-in Copilot that
//! copies its stdin to a file:
//!
//! - (a) a prompt on stdin: the stand-in gets the prompt, then the block once,
//!   and nothing after it; one `guidance_injected` record names the entry, no
//!   drop is recorded, and `recipe guidance show` reports the entry as read;
//! - (b) a stand-in that reads its first line and closes stdin before the
//!   caller's stdin ends: the block is never written, so the launch records a
//!   `trailer_not_written` drop and no `guidance_injected`, and the entry
//!   stays not yet read;
//! - (c) nothing on stdin (`/dev/null`): the launch records a
//!   `no_prompt_on_stdin` drop and the stand-in gets no block.
//!
//! The assertions compare values (positions, counts, JSON fields and reason
//! codes). None compares prose.

#![cfg(unix)]

use crate::support::recipe_guide::{
    BEGIN, END, GuideFixture, private_dir, show, stderr, written_lines,
};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const MARKER: &str = "MARKER-3162-copilot-stdin-launcher";
const PROMPT: &str = "copilot step prompt\n";

/// Answers `--version`; otherwise records its argv and copies all of stdin
/// to `$RECORD_DIR/prompt.$STEP`. With `COPILOT_CLOSE_STDIN` set, it reads
/// one line, records it, closes stdin and exits without reaching EOF.
const FAKE_COPILOT: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '1.0.115'; exit 0 ;;
esac
step="${STEP:-0}"
: > "$RECORD_DIR/copilotargv.$step"
for a in "$@"; do printf '%s\n' "$a" >> "$RECORD_DIR/copilotargv.$step"; done
if [ -n "$COPILOT_CLOSE_STDIN" ]; then
  IFS= read -r first || true
  printf '%s\n' "$first" > "$RECORD_DIR/prompt.$step"
  exec 0<&-
  exit 0
fi
cat > "$RECORD_DIR/prompt.$step"
exit 0
"#;

/// A fixture, a run folder holding one entry written by the real
/// `recipe guide`, the folder's `guidance/` path and the entry's file name.
struct Setup {
    f: GuideFixture,
    guidance: PathBuf,
    entry: String,
    copilot: PathBuf,
}

fn setup(run: &str) -> Setup {
    let f = GuideFixture::new();
    let folder = f.make_run(run, &format!("c-{run}"));
    let o = f.guide_text(&[run], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    let entry = written_lines(&o)[0]
        .0
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    // The stand-in is outside the fixture's `tools/`, so no other fixture
    // test finds a `copilot` on its `PATH`.
    let standin = f.root.join("copilot-standin");
    private_dir(&standin);
    let copilot = standin.join("copilot");
    crate::support::exec_stub::write_executable(&copilot, FAKE_COPILOT, 0o755);
    Setup {
        f,
        guidance: folder.join("guidance"),
        entry,
        copilot,
    }
}

/// `amplihack copilot` in the form the route covers: no `-p`, no `-i`.
fn copilot_launch(s: &Setup, step: &str) -> Command {
    let mut c =
        s.f.amplihack(&["copilot", "--subprocess-safe", "--", "--allow-all-tools"]);
    c.env("STEP", step)
        .env("AMPLIHACK_COPILOT_BINARY_PATH", &s.copilot)
        .env("AMPLIHACK_AGENT_BINARY", "copilot")
        .env("AMPLIHACK_RUN_GUIDANCE_DIRS", &s.guidance)
        .env("AMPLIHACK_RECIPE_RUN_ID", format!("c-{step}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn events(log: &[serde_json::Value]) -> Vec<String> {
    log.iter()
        .map(|l| l["event"].as_str().unwrap().to_string())
        .collect()
}

/// `recipe guidance show <run> --json`.
fn guidance_show_json(f: &GuideFixture, run: &str) -> serde_json::Value {
    let o = f.run_with_stdin(
        f.amplihack(&["recipe", "guidance", "show", run, "--json"]),
        None,
    );
    assert!(o.status.success(), "{}", show(&o));
    serde_json::from_str(String::from_utf8_lossy(&o.stdout).trim())
        .unwrap_or_else(|e| panic!("{e}: {}", show(&o)))
}

fn assert_stand_in_ran(f: &GuideFixture, step: &str, o: &Output) {
    let argv = f.record_opt(&format!("copilotargv.{step}"));
    assert!(
        argv.is_some(),
        "the stand-in Copilot was not started\n{}",
        show(o)
    );
    let argv = argv.unwrap();
    // Nothing about guidance reaches argv.
    assert!(
        !argv.contains(MARKER) && !argv.contains("OPERATOR"),
        "{argv}"
    );
    assert!(
        !argv.lines().any(|a| a == "-p" || a == "-i"),
        "the route under test has no -p or -i\n{argv}"
    );
}

/// The `read` field of the one entry named `entry`.
fn entry_read(shown: &serde_json::Value, entry: &str) -> bool {
    let entries = shown["entries"].as_array().expect("entries is an array");
    let found: Vec<_> = entries.iter().filter(|e| e["name"] == entry).collect();
    assert_eq!(found.len(), 1, "one entry named {entry}: {shown}");
    found[0]["read"].as_bool().expect("read is a boolean")
}

// ---------------------------------------------------------------------------
// (a) a prompt on stdin: the block follows it, and the entry is read
// ---------------------------------------------------------------------------

#[test]
fn a_copilot_launch_with_a_prompt_on_stdin_gets_the_block_after_it() {
    let s = setup("r-copilot-a");
    let o =
        s.f.run_with_stdin(copilot_launch(&s, "a"), Some(PROMPT.as_bytes()));
    assert!(o.status.success(), "{}", show(&o));
    assert_stand_in_ran(&s.f, "a", &o);

    // The caller's prompt first, then exactly one block, nothing after.
    let p = s.f.record("prompt.a");
    assert!(p.starts_with(PROMPT), "{p:?}");
    assert_eq!(p.matches(BEGIN).count(), 1, "{p:?}");
    assert_eq!(p.matches(END).count(), 1, "{p:?}");
    assert!(p.find(BEGIN).unwrap() >= PROMPT.len(), "{p:?}");
    assert!(p.find(MARKER).unwrap() > p.find(BEGIN).unwrap(), "{p:?}");
    assert!(
        p.trim_end().ends_with(END),
        "nothing after the block\n{p:?}"
    );

    // The guidance text is in the prompt only, never in amplihack's output.
    assert!(!stderr(&o).contains(MARKER), "{}", show(&o));
    assert_eq!(
        stderr(&o)
            .matches("operator guidance added to this step's prompt")
            .count(),
        1,
        "{}",
        show(&o)
    );

    let log = s.f.log(&s.guidance);
    assert_eq!(
        events(&log),
        ["guidance_appended", "guidance_injected"],
        "{log:?}"
    );
    assert_eq!(log[1]["entries"], 1, "{log:?}");
    assert_eq!(
        log[1]["entry_names"],
        serde_json::json!([s.entry]),
        "{log:?}"
    );
    assert_eq!(log[1]["step_run_id"], "c-a", "{log:?}");

    let shown = guidance_show_json(&s.f, "r-copilot-a");
    assert!(entry_read(&shown, &s.entry), "{shown}");
    assert_eq!(shown["drops"], serde_json::json!([]), "{shown}");
}

// ---------------------------------------------------------------------------
// (b) Copilot stops reading before the block: nothing is claimed as read
// ---------------------------------------------------------------------------

// The test holds amplihack's stdin open until amplihack exits, so the
// forwarder is still waiting on it when the stand-in has closed its stdin and
// exited, and the block is never written. No timer is involved. The caller
// wrote a prompt line first and the stand-in read it, so the step had a
// prompt and the drop is `trailer_not_written`, not `no_prompt_on_stdin`.
#[test]
fn b_a_copilot_that_closes_stdin_early_is_a_recorded_drop() {
    let s = setup("r-copilot-b");
    let mut c = copilot_launch(&s, "b");
    c.env("COPILOT_CLOSE_STDIN", "1").stdin(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack copilot");
    // Kept open until amplihack has exited.
    let mut held_stdin = child.stdin.take().unwrap();
    held_stdin
        .write_all(PROMPT.as_bytes())
        .expect("write the prompt");
    let o = child.wait_with_output().unwrap();
    drop(held_stdin);
    assert!(o.status.success(), "{}", show(&o));
    assert_stand_in_ran(&s.f, "b", &o);
    assert_eq!(s.f.record("prompt.b"), PROMPT, "{}", show(&o));
    assert!(
        !stderr(&o).contains("operator guidance added"),
        "no notice for undelivered guidance\n{}",
        show(&o)
    );

    let log = s.f.log(&s.guidance);
    assert_eq!(
        events(&log),
        ["guidance_appended", "guidance_dropped"],
        "no guidance_injected record: {log:?}"
    );
    assert_eq!(log[1]["reason"], "trailer_not_written", "{log:?}");
    assert_eq!(log[1]["tool"], "copilot", "{log:?}");
    assert_eq!(
        log[1]["launch_form"], "copilot, prompt expected on stdin",
        "{log:?}"
    );
    assert!(stderr(&o).contains("trailer_not_written"), "{}", show(&o));

    let shown = guidance_show_json(&s.f, "r-copilot-b");
    assert!(!entry_read(&shown, &s.entry), "{shown}");
    let drops = shown["drops"].as_array().expect("drops is an array");
    assert_eq!(drops.len(), 1, "{shown}");
    assert_eq!(drops[0]["reason"], "trailer_not_written", "{shown}");
}

// ---------------------------------------------------------------------------
// (c) nothing on stdin: a recorded drop, and no block
// ---------------------------------------------------------------------------

#[test]
fn c_a_copilot_launch_with_nothing_on_stdin_is_a_recorded_drop() {
    let s = setup("r-copilot-c");
    let o = s.f.run_with_stdin(copilot_launch(&s, "c"), None);
    assert!(o.status.success(), "{}", show(&o));
    assert_stand_in_ran(&s.f, "c", &o);
    let p = s.f.record("prompt.c");
    assert!(!p.contains(BEGIN) && !p.contains(MARKER), "{p:?}");

    let log = s.f.log(&s.guidance);
    assert_eq!(
        events(&log),
        ["guidance_appended", "guidance_dropped"],
        "{log:?}"
    );
    assert_eq!(log[1]["reason"], "no_prompt_on_stdin", "{log:?}");
    assert_eq!(log[1]["tool"], "copilot", "{log:?}");

    let shown = guidance_show_json(&s.f, "r-copilot-c");
    assert!(!entry_read(&shown, &s.entry), "{shown}");
}
