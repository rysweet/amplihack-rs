//! Issue #3162: an agent step whose prompt arrives on stdin takes operator
//! guidance, and a step that cannot take it says so to the sender.
//!
//! Real `amplihack` binary, real launcher, the fixture's pinned recipe-runner
//! stand-in. The runner starts the way the real one does,
//! `amplihack claude -- --system-prompt-file <file> -p --add-dir .` with the
//! prompt already on standard input, and blocks on a FIFO between its two
//! steps. The test sends guidance by session name while the runner is
//! blocked, then releases it. There is no sleep, poll or timer: the FIFO open
//! is the rendezvous and the test waits for the process to finish.
//!
//! - (a) guidance sent by session name mid-run is after the next step's own
//!   prompt, not in the step before, and `recipe guidance show` reports the
//!   entry as read;
//! - (c) a launch with no prompt on stdin (`/dev/null`) cannot take guidance:
//!   the drop is recorded in the run's guidance folder, and `recipe guide`
//!   and `recipe guidance show` report it, naming the tool and a reason
//!   code. The entry stays not yet read, and once the run has ended both
//!   readers say that it ended (PR #3190 review).
//! - (d) PR #3190 review: a pipe on stdin that closes empty, or with only
//!   blank lines, looks like a prompt to `fstat` but holds none. The block
//!   is not written (the agent gets exactly the caller's bytes, so the
//!   guidance never becomes its whole prompt), the launch records
//!   `no_prompt_on_stdin`, and the entry stays not yet read.
//!
//! The assertions compare values: positions, counts, JSON fields (among
//! them the documented launch-form label), the status value the sender is
//! given, and reason codes, which are also looked for on the step's own
//! stderr. None compares the wording of a message.

#![cfg(unix)]

use crate::support::fifo_release;
use crate::support::recipe_guide::{
    BEGIN, END, GuideFixture, drop_lines, show, status_lines, stderr, stdout, written_lines,
};
use std::fs;
use std::path::PathBuf;

const MARKER: &str = "MARKER-3162-place-and-launch-nothing-in-this-pass";
const SESSION: &str = "factory-fill-pass";

/// Step one, a block on `$RELEASE_FIFO`, then step two. `$STEP_TWO_STDIN`
/// is `pipe` (the prompt is already on stdin), `null` (`/dev/null`),
/// `empty` (a pipe that closes with nothing in it) or `blank` (a pipe that
/// holds only white space). Both steps use the form the real runner starts.
const TWO_STDIN_STEPS: &str = r#"
printf '%s\n' "agent: amplihack:core:builder" > "$RECORD_DIR/builder.md"
printf 'step one prompt\n' | STEP=s.1 "$AMP_BIN" claude --subprocess-safe -- \
  --system-prompt-file "$RECORD_DIR/builder.md" -p --add-dir . \
  > "$RECORD_DIR/out.s.1" 2> "$RECORD_DIR/err.s.1" || exit 11
read _ < "$RELEASE_FIFO"
step_two() {
  STEP=s.2 "$AMP_BIN" claude --subprocess-safe -- \
    --system-prompt-file "$RECORD_DIR/builder.md" -p --add-dir . \
    > "$RECORD_DIR/out.s.2" 2> "$RECORD_DIR/err.s.2"
}
case "$STEP_TWO_STDIN" in
  null) step_two < /dev/null || exit 12 ;;
  empty) : | step_two || exit 12 ;;
  blank) printf '\n  \t\n' | step_two || exit 12 ;;
  *) printf 'step two prompt\n' | step_two || exit 12 ;;
esac
"#;

struct Held {
    f: GuideFixture,
    live: fifo_release::Live,
    run_guidance: PathBuf,
}

/// Starts the two-step run and returns once it is blocked between its steps,
/// with a session log that names it.
fn start(step_two_stdin: &str) -> Held {
    let f = GuideFixture::new();
    let fifo = f.root.join("release.fifo");
    fifo_release::mkfifo(&fifo);
    let command = f.recipe_run_command(
        TWO_STDIN_STEPS,
        &[],
        &[
            ("RELEASE_FIFO", fifo.to_str().unwrap()),
            ("STEP_TWO_STDIN", step_two_stdin),
        ],
    );
    let live = fifo_release::start(command, &fifo, "amplihack recipe run");
    let corr = f.record("corr_id.run");
    f.session_log(SESSION, corr.trim());
    let run_guidance = f
        .runs()
        .join(f.record("run_id.run").trim())
        .join("guidance");
    Held {
        f,
        live,
        run_guidance,
    }
}

fn guidance_show_json(f: &GuideFixture) -> serde_json::Value {
    let o = f.run_with_stdin(
        f.amplihack(&["recipe", "guidance", "show", SESSION, "--json"]),
        None,
    );
    assert!(o.status.success(), "{}", show(&o));
    serde_json::from_str(stdout(&o).trim()).unwrap_or_else(|e| panic!("{e}: {}", show(&o)))
}

// ---------------------------------------------------------------------------
// (a) the prompt is already on stdin: the block follows it
// ---------------------------------------------------------------------------

#[test]
fn a_guidance_sent_by_session_name_follows_the_next_stdin_prompt() {
    let mut held = start("pipe");
    let o = held.f.guide_text(&[SESSION], MARKER);
    assert!(o.status.success(), "{}", show(&o));

    // The sender is told the truth: written, and no step has read it yet.
    let written = written_lines(&o);
    assert_eq!(written.len(), 1, "{}", show(&o));
    assert_eq!(written[0].0.parent().unwrap(), held.run_guidance);
    assert_eq!(status_lines(&o), ["appended; not yet read"], "{}", show(&o));
    assert!(drop_lines(&o).is_empty(), "{}", show(&o));

    let before = guidance_show_json(&held.f);
    assert_eq!(before["entries"][0]["read"], false, "{before}");

    let run = held.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));

    // Step one ran before the entry existed.
    assert_eq!(held.f.record("prompt.s.1"), "step one prompt\n");
    // Step two: its own prompt first, then exactly one block, nothing after.
    let p2 = held.f.record("prompt.s.2");
    assert!(p2.starts_with("step two prompt\n"), "{p2:?}");
    assert_eq!(p2.matches(BEGIN).count(), 1, "{p2:?}");
    assert_eq!(p2.matches(END).count(), 1, "{p2:?}");
    let own_prompt_end = p2.find("step two prompt\n").unwrap() + "step two prompt\n".len();
    assert!(p2.find(BEGIN).unwrap() >= own_prompt_end, "{p2:?}");
    assert!(p2.find(MARKER).unwrap() > p2.find(BEGIN).unwrap(), "{p2:?}");
    assert!(p2.trim_end().ends_with(END), "nothing after the block");

    // The guidance text is in the prompt only, never in amplihack's output.
    for text in [stderr(&run), held.f.record("err.s.2")] {
        assert!(!text.contains(MARKER), "{text}");
    }

    // The log names the entry as read, without its text, and the reader
    // reports it.
    let entry = written[0]
        .0
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let injected: Vec<_> = held
        .f
        .log(&held.run_guidance)
        .into_iter()
        .filter(|l| l["event"] == "guidance_injected")
        .collect();
    assert_eq!(injected.len(), 1, "{injected:?}");
    assert_eq!(injected[0]["entries"], 1);
    assert_eq!(injected[0]["entry_names"], serde_json::json!([entry]));
    let raw = fs::read_to_string(held.run_guidance.parent().unwrap().join("guidance.log")).unwrap();
    assert!(!raw.contains(MARKER), "{raw}");

    let after = guidance_show_json(&held.f);
    assert_eq!(after["schema_version"], 1);
    assert_eq!(after["entries"][0]["read"], true, "{after}");
    assert_eq!(after["drops"], serde_json::json!([]), "{after}");
}

// ---------------------------------------------------------------------------
// (c) nothing on stdin: the drop reaches the sender
// ---------------------------------------------------------------------------

#[test]
fn c_a_step_with_no_prompt_on_stdin_records_the_drop_for_the_sender() {
    let mut held = start("null");
    let o = held.f.guide_text(&[SESSION], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    let run = held.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));

    // Nothing was delivered, so nothing is claimed as read.
    assert!(
        !held.f.record("prompt.s.2").contains(MARKER),
        "undelivered guidance must not appear in the prompt"
    );
    let log = held.f.log(&held.run_guidance);
    assert!(
        log.iter().all(|l| l["event"] != "guidance_injected"),
        "{log:?}"
    );

    // The drop is in the run's guidance log: closed fields, no text.
    let drops: Vec<_> = log
        .iter()
        .filter(|l| l["event"] == "guidance_dropped")
        .collect();
    assert_eq!(drops.len(), 1, "{log:?}");
    let drop = drops[0];
    assert_eq!(drop["tool"], "claude");
    assert!(
        drop["reason"]
            .as_str()
            .unwrap()
            .starts_with("no_prompt_on_stdin"),
        "{drop}"
    );
    assert_eq!(
        drop["step_run_id"].as_str().unwrap(),
        held.f.record("corr_id.run").trim()
    );
    assert!(drop["launch_form"].as_str().is_some_and(|s| !s.is_empty()));
    let raw = fs::read_to_string(held.run_guidance.parent().unwrap().join("guidance.log")).unwrap();
    assert!(!raw.contains(MARKER), "{raw}");

    // The launcher also says it on the step's own stderr, with the code.
    assert!(
        held.f.record("err.s.2").contains("no_prompt_on_stdin"),
        "{}",
        held.f.record("err.s.2")
    );

    // `recipe guidance show` reports the drop and the unread entry.
    let shown = guidance_show_json(&held.f);
    assert_eq!(shown["entries"][0]["read"], false, "{shown}");
    let reported = shown["drops"].as_array().expect("drops is an array");
    assert_eq!(reported.len(), 1, "{shown}");
    assert_eq!(reported[0]["tool"], "claude");
    assert!(
        reported[0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("no_prompt_on_stdin"),
        "{shown}"
    );
    assert_eq!(
        reported[0]["step_run_id"].as_str().unwrap(),
        held.f.record("corr_id.run").trim()
    );

    // PR #3190 review: the run has ended, and the reader says so rather
    // than leave the entry only "not yet read".
    assert_eq!(
        shown["entries"][0]["run_ended"]["outcome"], "success",
        "{shown}"
    );

    // The next `recipe guide` to the same run tells its sender, by line,
    // that the run it named has already ended.
    let next = held.f.guide_text(&[SESSION], "a second note");
    assert!(next.status.success(), "{}", show(&next));
    let status = status_lines(&next);
    assert_eq!(status.len(), 1, "{}", show(&next));
    assert!(
        status[0].starts_with("appended; not yet read; the run ended at "),
        "{}",
        show(&next)
    );
    assert!(status[0].ends_with(" (success)"), "{}", show(&next));
    let lines = drop_lines(&next);
    assert_eq!(lines.len(), 1, "{}", show(&next));
    assert!(lines[0].contains("claude"), "{}", lines[0]);
    assert!(lines[0].contains("no_prompt_on_stdin"), "{}", lines[0]);
}

// ---------------------------------------------------------------------------
// (d) a pipe that gives no text: no block, and the drop reaches the sender
// ---------------------------------------------------------------------------

/// Runs step two with `stdin` (`empty` or `blank`) after guidance was sent,
/// and checks that the agent got exactly `given`, with no block, and that
/// the launch recorded `no_prompt_on_stdin` and nothing as read.
fn a_pipe_with_no_text_gets_no_block(stdin: &str, given: &str) {
    let mut held = start(stdin);
    let o = held.f.guide_text(&[SESSION], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    let run = held.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));

    // The agent got the caller's bytes and nothing else: the guidance is
    // not its whole prompt.
    assert_eq!(held.f.record("prompt.s.2"), given);

    // Nothing is claimed as read; the drop is recorded with its code.
    let log = held.f.log(&held.run_guidance);
    assert!(
        log.iter().all(|l| l["event"] != "guidance_injected"),
        "{log:?}"
    );
    let drops: Vec<_> = log
        .iter()
        .filter(|l| l["event"] == "guidance_dropped")
        .collect();
    assert_eq!(drops.len(), 1, "{log:?}");
    assert_eq!(drops[0]["tool"], "claude");
    assert_eq!(drops[0]["reason"], "no_prompt_on_stdin", "{}", drops[0]);
    assert_eq!(
        drops[0]["launch_form"], "claude -p, prompt expected on stdin",
        "{}",
        drops[0]
    );
    assert!(
        held.f.record("err.s.2").contains("no_prompt_on_stdin"),
        "{}",
        held.f.record("err.s.2")
    );

    // The reader says the same.
    let shown = guidance_show_json(&held.f);
    assert_eq!(shown["entries"][0]["read"], false, "{shown}");
    let reported = shown["drops"].as_array().expect("drops is an array");
    assert_eq!(reported.len(), 1, "{shown}");
    assert_eq!(reported[0]["reason"], "no_prompt_on_stdin", "{shown}");
}

#[test]
fn d_an_empty_pipe_on_stdin_gets_no_block_and_records_the_drop() {
    a_pipe_with_no_text_gets_no_block("empty", "");
}

#[test]
fn d_a_blank_pipe_on_stdin_gets_no_block_and_records_the_drop() {
    a_pipe_with_no_text_gets_no_block("blank", "\n  \t\n");
}
