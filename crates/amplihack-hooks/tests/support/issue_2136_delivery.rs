//! Issue #2136: the organisation warning comes from the swap record that
//! woke the wait, and from nothing else. Uses the existing event-scripted
//! fake; no timers or actual tmux server. The real binary, spawned through
//! `/bin/sh -c` as Claude Code spawns it, is covered in `limit_resume_fs.rs`.
use super::*;
use amplihack_utils::rotator_status::swap_record::parse_swap;

fn affinity_record(outcome: &str) -> SwapRecord {
    let mut v = serde_json::to_value(fresh(1)).unwrap();
    v["from_org"] = json!("org-a");
    v["to_org"] = match outcome {
        "unresolved" => json!(null),
        "same" => json!("org-a"),
        _ => json!("org-b"),
    };
    v["affinity"] = json!(outcome);
    parse_swap(&serde_json::to_vec(&v).unwrap(), START + 600).unwrap()
}

#[test]
fn stderr_delivers_confirmed_crossing_warning_with_capacity_resume() {
    let mut d = Fake::new().then(Act::Swap(affinity_record("crossing")));
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        2
    );
    let text = String::from_utf8(stderr).unwrap();
    assert!(
        text.contains("earlier thinking") && text.contains("next turn"),
        "{text}"
    );
    assert!(text.contains("unavailable"), "{text}");
    assert!(!text.contains("conversation disappears"), "{text}");
}

#[test]
fn stderr_distinguishes_unresolved_risk_from_confirmed_crossing() {
    let mut d = Fake::new().then(Act::Swap(affinity_record("unresolved")));
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        2
    );
    let text = String::from_utf8(stderr).unwrap();
    assert!(
        text.contains("unresolved") && text.contains("earlier thinking"),
        "{text}"
    );
    assert!(text.contains("may") || text.contains("possible"), "{text}");
}

#[test]
fn tmux_resume_payload_delivers_the_same_thinking_warning() {
    let mut d = Fake::with_pane().then(Act::Swap(affinity_record("crossing")));
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        0
    );
    let pane = Pane::parse(Some(TMUX), Some("%5")).unwrap();
    let sent = d
        .keys
        .iter()
        .flat_map(|key| amplihack_hooks::limit_resume::keys::argv(&pane, *key))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        sent.contains("earlier thinking") && sent.contains("next turn"),
        "{sent}"
    );
    assert!(
        stderr.is_empty(),
        "tmux delivery must not also wake via exit 2"
    );
}

#[test]
fn a_legacy_swap_does_not_manufacture_thinking_loss() {
    let mut d = Fake::new().then(Act::Swap(fresh(1)));
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        2
    );
    assert_eq!(stderr, format!("{RESUME_MSG}\n").as_bytes());
}

struct FailedWarningWriter;
impl std::io::Write for FailedWarningWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("output unavailable"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn failed_warning_output_preserves_committed_exit_two() {
    let mut d = Fake::new().then(Act::Swap(affinity_record("crossing")));
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut FailedWarningWriter, &mut d),
        2
    );
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut Vec::new(), &mut d),
        0
    );
}

#[test]
fn a_same_organisation_swap_adds_no_warning() {
    let mut d = Fake::new().then(Act::Swap(affinity_record("same")));
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        2
    );
    assert_eq!(stderr, format!("{RESUME_MSG}\n").as_bytes());
}

#[test]
fn a_stop_that_does_not_resume_writes_no_warning() {
    // No per-session state: a warning reaches a session only with the
    // resume that follows the swap it describes.
    let mut d = Fake::new();
    d.rotator_down = true;
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        0
    );
    assert!(
        !String::from_utf8(stderr)
            .unwrap()
            .contains("earlier thinking")
    );
}

#[test]
fn each_resume_reads_only_its_own_swap_record() {
    let mut d = Fake::new().then(Act::Swap(affinity_record("crossing")));
    let mut first = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut first, &mut d),
        2
    );
    assert!(
        String::from_utf8(first)
            .unwrap()
            .contains("earlier thinking")
    );
    let mut d = Fake::new().then(Act::Swap(fresh(2)));
    let mut second = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut second, &mut d),
        2
    );
    assert_eq!(second, format!("{RESUME_MSG}\n").as_bytes());
}
