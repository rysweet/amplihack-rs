//! Issue #2535: guidance never appears in published text.
//!
//! - (d) a run with guidance present builds a PR title, a PR body file and a
//!   commit message from an agent step's output and "publishes" them through
//!   fake `gh` and `git`. The marker reaches the agent's prompt and nothing
//!   else: not the `gh` argv, the `--body-file`, the `git commit` argv or
//!   message file, `run.json`, `guidance.log`, or anything amplihack prints.
//! - (u5) the auto-drive merge gate's private-state check still accepts a
//!   pull request state folder that holds `guidance/` and `guidance.log`.
//!
//! The fake agent echoes only its step output, as a real step's output is
//! what publish steps read. A model that copies the instruction into its own
//! output cannot be prevented by this mechanism; that is documented, and not
//! what this test covers.

#![cfg(unix)]

use crate::support::recipe_guide::{
    GuideFixture, PR_URL, private_dir, private_file, show, stderr, stdout,
};
use std::fs;

const MARKER: &str = "MARKER-2535-never-publish-this";

/// One agent step whose output becomes the title, body and commit message,
/// then the publish commands with the fake `gh` and `git` first on PATH.
/// Their stdout goes to stderr: the runner's stdout must hold only its JSON
/// result, which `recipe run` parses.
const PUBLISH_RUNNER: &str = r#"
out=$(STEP=d.1 AGENT_OUTPUT="Fix retry backoff" "$AMP_BIN" claude --subprocess-safe -- \
  -p "write a pull request title" 2> "$RECORD_DIR/err.d.1") || exit 11
printf '%s\n' "$out" > "$RECORD_DIR/body.md"
printf '%s\n' "$out" > "$RECORD_DIR/commit-msg.txt"
PATH="$PUBLISH_TOOLS:$PATH" gh pr create --title "$out" --body-file "$RECORD_DIR/body.md" >&2 || exit 12
PATH="$PUBLISH_TOOLS:$PATH" gh pr comment 7 --body "$out" >&2 || exit 13
PATH="$PUBLISH_TOOLS:$PATH" git commit -m "$out" >&2 || exit 14
PATH="$PUBLISH_TOOLS:$PATH" git commit -F "$RECORD_DIR/commit-msg.txt" >&2 || exit 15
"#;

#[test]
fn d_guidance_reaches_the_agent_prompt_but_no_published_text() {
    let f = GuideFixture::new();
    let g = f.guide_text(&[PR_URL], MARKER);
    assert!(g.status.success(), "{}", show(&g));

    let ctx = format!("pr_url={PR_URL}");
    let o = f.recipe_run(PUBLISH_RUNNER, &["-c", &ctx], &[("RUN_TAG", "d")]);
    assert!(o.status.success(), "{}", show(&o));

    // Positive control: the agent did receive the guidance.
    let prompt = f.record("prompt.d.1");
    assert!(
        prompt.contains(MARKER),
        "the agent prompt lacks the guidance:\n{prompt}"
    );

    // The publish commands ran.
    assert_eq!(f.record("gh.count").trim(), "2");
    assert_eq!(f.record("git.count").trim(), "2");
    assert!(f.record("gh.argv.1").contains("Fix retry backoff"));

    // Nothing published holds the marker.
    for (what, text) in [
        ("gh argv", f.records_starting("gh.argv.")),
        ("gh --body-file", f.records_starting("gh.body.")),
        ("git argv", f.records_starting("git.argv.")),
        ("git -F file", f.records_starting("git.msgfile.")),
        ("body.md", f.record("body.md")),
        ("commit-msg.txt", f.record("commit-msg.txt")),
    ] {
        assert!(
            !text.contains(MARKER),
            "guidance leaked into {what}:\n{text}"
        );
        assert!(
            !text.contains("OPERATOR INSTRUCTIONS"),
            "the block leaked into {what}:\n{text}"
        );
    }

    // Nor any record amplihack writes, nor anything it prints.
    let run = f.runs().join(f.record("run_id.d").trim());
    for file in ["run.json", "guidance.log"] {
        if let Ok(text) = fs::read_to_string(run.join(file)) {
            assert!(!text.contains(MARKER), "{file} holds guidance");
        }
    }
    let pr_log = fs::read_to_string(f.pr_parent().join("guidance.log")).unwrap();
    assert!(!pr_log.contains(MARKER));
    assert!(
        !stdout(&o).contains(MARKER),
        "recipe run stdout holds guidance"
    );
    assert!(
        !stderr(&o).contains(MARKER),
        "recipe run stderr holds guidance"
    );
    assert!(
        !f.record("err.d.1").contains(MARKER),
        "launcher stderr holds guidance"
    );
}

// ---------------------------------------------------------------------------
// (u5) the merge gate's private-state check with guidance present
// ---------------------------------------------------------------------------

// The #2336 merge-gate fixture, loaded once by that test module.
use crate::issue_2336_autodrive_completion::fixture as drive;

#[test]
fn u5_merge_gate_private_state_check_accepts_guidance_and_its_log() {
    let d = drive::Drive::new();
    d.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);

    // The layout `recipe guide` writes into an https-origin drive's state
    // folder: guidance/ at 0700, entries and guidance.log at 0600.
    let guidance = d.state.join("guidance");
    private_dir(&guidance);
    private_file(
        &guidance.join("20261112T091405.120458911Z-48211-0.md"),
        "timestamp: 2026-11-12T09:14:05.120458911Z\ncaller: alice\n\nKeep the API.",
    );
    private_file(
        &d.state.join("guidance.log"),
        "{\"event\":\"guidance_appended\",\"ts\":\"2026-11-12T09:14:05.120458911Z\",\"caller\":\"alice\",\"bytes\":13,\"entry\":\"20261112T091405.120458911Z-48211-0.md\"}\n",
    );

    d.record(1);
    let p = d.prepare(1, "");
    d.decide(&p, "complete");
    drive::assert_complete(&d, &d.dispatch(&p, 0));
}
