//! Uses the existing event-scripted fake; no timers or actual tmux server.
use super::*;
use amplihack_utils::rotator_status::swap_record::parse_swap;

fn affinity_record(outcome: &str) -> SwapRecord {
    let mut v = serde_json::to_value(fresh(1)).unwrap();
    v["from_org"] = json!("org-a");
    v["to_org"] = if outcome == "unresolved" {
        json!(null)
    } else {
        json!("org-b")
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

#[test]
fn durable_warning_survives_latest_same_org_swap_and_is_acknowledged() {
    use amplihack_utils::swap_warning_history::{Completion, WarningHistory};
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut history = WarningHistory::open(tmp.path()).unwrap();
    let session = history.register_current_process("s", "home").unwrap();
    for (tx, affinity, to) in [("cross", "crossing", "org-b"), ("same", "same", "org-a")] {
        let intent = history
            .prepare(&json!({"version":1,"transaction_id":tx,
            "producer":"rotator","reason":"threshold","host":"home",
            "from_login":"work","to_login":"spare", "from_org":"org-a",
            "to_org":to,"affinity":affinity}))
            .unwrap();
        history.complete(&intent, Completion::Committed).unwrap();
    }
    let mut d = Fake::new().then(Act::Swap(fresh(2)));
    d.warning_root = Some(tmp.path().to_owned());
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        2
    );
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("earlier thinking")
    );
    assert!(history.pending(&session).unwrap().is_empty());
}

#[test]
fn failed_tmux_delivery_falls_back_to_stderr() {
    use amplihack_utils::swap_warning_history::{Completion, WarningHistory};
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut history = WarningHistory::open(tmp.path()).unwrap();
    let session = history.register_current_process("s", "home").unwrap();
    let intent = history
        .prepare(&json!({"version":1,"transaction_id":"cross",
        "producer":"auto-switch","reason":"threshold","host":"home",
        "from_login":"work","to_login":"spare", "from_org":"org-a",
        "to_org":"org-b","affinity":"crossing"}))
        .unwrap();
    history.complete(&intent, Completion::Committed).unwrap();
    let mut d = Fake::with_pane().then(Act::Swap(fresh(2)));
    d.warning_root = Some(tmp.path().to_owned());
    d.fail_key = Some(3);
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut Vec::new(), &mut d),
        0
    );
    assert!(history.pending(&session).unwrap().is_empty());
}

#[test]
fn warning_published_during_the_event_wait_is_delivered_before_acknowledgement() {
    use amplihack_utils::swap_warning_history::WarningHistory;
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut history = WarningHistory::open(tmp.path()).unwrap();
    let session = history.register_current_process("s", "home").unwrap();
    let mut d = Fake::with_pane().then(Act::PublishWarningThenSwap(fresh(3)));
    d.warning_root = Some(tmp.path().to_owned());
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut Vec::new(), &mut d),
        0
    );
    assert!(d.keys.contains(&Key::LiteralCrossing));
    assert!(history.pending(&session).unwrap().is_empty());
}

#[test]
fn a_newly_registered_session_does_not_inherit_an_unscoped_latest_warning() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut d = Fake::new().then(Act::Swap(affinity_record("crossing")));
    d.warning_root = Some(tmp.path().to_owned());
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
fn unavailable_warning_registration_does_not_block_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let mut d = Fake::new().then(Act::Swap(fresh(2)));
    d.warning_root = Some(tmp.path().join("missing"));
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut Vec::new(), &mut d),
        2
    );
}

#[test]
fn rotator_down_delivers_scoped_warning_once_without_resume() {
    use amplihack_utils::swap_warning_history::{Completion, WarningHistory};
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut h = WarningHistory::open(tmp.path()).unwrap();
    let s = h.register_current_process("s", "home").unwrap();
    let other = h.register_current_process("other", "unrelated").unwrap();
    let intent = h
        .prepare(&json!({"version":1,"transaction_id":"auto",
        "producer":"auto-switch","reason":"threshold","host":"home",
        "from_login":"work","to_login":"spare","from_org":"a","to_org":"b",
        "affinity":"crossing"}))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    let mut d = Fake::new();
    d.rotator_down = true;
    d.warning_root = Some(tmp.path().to_owned());
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut FailedWarningWriter, &mut d),
        0
    );
    assert_eq!(h.pending(&s).unwrap().len(), 1);
    let mut out = Vec::new();
    assert_eq!(run_with_io(limit_payload().as_bytes(), &mut out, &mut d), 0);
    assert!(String::from_utf8(out).unwrap().contains("earlier thinking"));
    assert!(h.pending(&s).unwrap().is_empty());
    assert!(h.pending(&other).unwrap().is_empty());
    let mut out = Vec::new();
    assert_eq!(run_with_io(limit_payload().as_bytes(), &mut out, &mut d), 0);
    assert!(out.is_empty());
}

#[test]
fn unreadable_warning_journal_does_not_block_resume() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut d = Fake::new().then(Act::Swap(fresh(2)));
    d.warning_root = Some(tmp.path().to_owned());
    d.corrupt_warning_journal = true;
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut Vec::new(), &mut d),
        2
    );
}
