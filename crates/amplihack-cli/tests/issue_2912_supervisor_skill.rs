//! Issue #2912: the step that reads what a standing session leaves on its
//! record is the bundled `supervise-agent-sessions` skill.
//!
//! The wrapper relaunches every exit its owner did not cause, with no count
//! and no brake, and leaves two things to the supervisor agent: whether a
//! session that keeps exiting is a crash loop, and why a session it could
//! not relaunch has not come back. Both rest on a reader of the record. These
//! tests hold that reader to what is written: the skill must have a row for
//! every form of alert, run only commands amplihack has, and be named by the
//! guides as the reader. The alerts themselves come from real wrapper runs in
//! `issue_2912_standing_relaunch.rs`, which checks each against
//! [`skill_row_for`].

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::{Fx, expect_code, stdout};
use serde_yaml::Value;

const SKILL_NAME: &str = "supervise-agent-sessions";
const SKILL: &str =
    include_str!("../../../amplifier-bundle/skills/supervise-agent-sessions/SKILL.md");
const KEEP_RUNNING: &str = include_str!("../../../docs/howto/keep-sessions-running.md");
const SUPERVISE: &str = include_str!("../../../docs/howto/supervise-agent-sessions.md");

/// The first column of the skill's table of alert forms: the text an alert
/// begins with, one per row.
fn alert_forms() -> Vec<&'static str> {
    let start = SKILL
        .find("| Alert begins with |")
        .expect("the skill has a table of alert forms");
    SKILL[start..]
        .lines()
        .skip(2)
        .map(str::trim)
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| l.strip_prefix("| `")?.split('`').next())
        .collect()
}

/// The row of the skill's table of alert forms that `alert` begins with, if
/// any. A wrapper or supervisor alert with no row is one the supervisor
/// agent has no instruction for.
pub fn skill_row_for(alert: &str) -> Option<&'static str> {
    alert_forms().into_iter().find(|f| alert.starts_with(f))
}

/// The evidence column of the row of the skill's "Judge the relaunches"
/// table whose decision is to stop the session: what the supervisor agent
/// must see before it stops a crash loop.
pub fn stop_evidence() -> &'static str {
    let start = SKILL
        .find("| What the evidence shows |")
        .expect("the skill has a table for judging relaunches");
    SKILL[start..]
        .lines()
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .find(|l| l.contains("(#stop-a-crash-loop)"))
        .and_then(|l| l.split('|').nth(1))
        .map(str::trim)
        .expect("a row of the table stops a crash loop")
}

/// The command the skill's "Stop a crash loop" section runs, as arguments
/// to `amplihack`, with `NAME` replaced by `name`.
pub fn stop_command(name: &str) -> Vec<String> {
    let start = SKILL
        .find("## Stop a crash loop")
        .expect("the skill says how to stop a crash loop");
    let block = SKILL[start..]
        .split("```bash\n")
        .nth(1)
        .and_then(|b| b.split("```").next())
        .expect("the section has a bash block");
    let line = block
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("amplihack "))
        .expect("the block runs an amplihack command");
    line.split_whitespace()
        .skip(1)
        .map(|w| if w == "NAME" { name } else { w }.to_string())
        .collect()
}

#[test]
fn the_skill_is_named_after_its_directory_and_runs_on_every_supervisor_pass() {
    let after = SKILL.strip_prefix("---\n").expect("frontmatter at byte 0");
    let end = after.find("\n---").expect("closed frontmatter");
    let fm: Value = serde_yaml::from_str(&after[..end]).expect("YAML frontmatter");
    assert_eq!(fm["name"].as_str(), Some(SKILL_NAME));
    let desc = fm["description"].as_str().expect("a description");
    for phrase in ["every supervisor pass", "amplihack agent status", "ALERT"] {
        assert!(desc.contains(phrase), "{phrase:?} not in {desc:?}");
    }
}

/// Four forms, one per kind of alert: the pane's record of a relaunch, its
/// two refusals that leave the session `active`, and the supervisor
/// service's own stop. The restart-limit text is the one
/// `issue_1760_agent_supervise_limits.rs` asserts the service writes.
#[test]
fn the_skill_has_a_row_for_every_kind_of_alert() {
    assert_eq!(alert_forms().len(), 4, "{:?}", alert_forms());
    assert_eq!(
        skill_row_for("stopped: restart limit (3 restarts in the last hour)"),
        Some("stopped: restart limit")
    );
    assert_eq!(skill_row_for("an alert nobody writes"), None);
}

/// The two endings a relaunch record can have, as the wrapper writes them,
/// each with what the skill says to do.
#[test]
fn the_skill_reads_both_endings_of_a_relaunch_record() {
    for ending in [
        "`the model replied during the run`",
        "without a reply from the model in its",
    ] {
        assert!(SKILL.contains(ending), "{ending:?}");
    }
    assert!(SKILL.contains("amplihack agent unregister NAME"));
    assert!(SKILL.contains("skipped: tmux session exists"));
}

/// Every amplihack command the skill tells the agent to run exists, with
/// the option it uses.
#[test]
fn every_amplihack_command_the_skill_runs_exists() {
    let fx = Fx::new();
    for (cmd, needs) in [
        (&["agent", "status"][..], "--json"),
        (&["agent", "unregister"][..], ""),
        (&["agent", "supervise"][..], "--once"),
        (&["claude"][..], "--supervise"),
        (&["profile", "usage"][..], ""),
    ] {
        let words = format!("amplihack {}", cmd.join(" "));
        assert!(SKILL.contains(&words), "{words:?} is not in the skill");
        let mut args = cmd.to_vec();
        args.push("--help");
        let out = fx.run(&args);
        expect_code(&out, 0);
        assert!(stdout(&out).contains(needs), "{words} --help: {needs:?}");
    }
}

/// The guides name the skill as the reader, so the next person to ask
/// "what notices a crash loop" finds it.
#[test]
fn the_guides_name_the_skill_as_the_reader_of_the_record() {
    assert!(KEEP_RUNNING.contains("### Who reads the record"));
    for (guide, text) in [
        ("keep-sessions-running.md", KEEP_RUNNING),
        ("supervise-agent-sessions.md", SUPERVISE),
    ] {
        assert!(
            text.contains("`supervise-agent-sessions`"),
            "{guide} does not name the skill"
        );
        assert!(text.contains("#who-reads-the-record"), "{guide}");
    }
}
