//! Issue #1932 merged with #2136: the pane-ownership check runs before every
//! key on the affinity path too. Once a swap record carries an affinity
//! warning, `run_started` sends keys through `send_resume_affinity`, so the
//! check has to live there and not only in `send_resume`. Uses the
//! event-scripted fake; no timers and no tmux server.
use super::*;
use amplihack_utils::rotator_status::swap_record::parse_swap;

/// A swap record whose organisation affinity is a confirmed crossing.
fn crossing_record() -> SwapRecord {
    let mut v = serde_json::to_value(fresh(1)).unwrap();
    v["from_org"] = json!("org-a");
    v["to_org"] = json!("org-b");
    v["affinity"] = json!("crossing");
    parse_swap(&serde_json::to_vec(&v).unwrap(), START + 600).unwrap()
}

/// `RESUME_KEYS` with `continue` replaced by the crossing warning, which is
/// what the affinity path types.
fn crossing_keys() -> Vec<Key> {
    RESUME_KEYS
        .iter()
        .map(|k| match k {
            Key::LiteralContinue => Key::LiteralCrossing,
            other => *other,
        })
        .collect()
}

#[test]
fn affinity_delivery_without_pane_change_types_the_crossing_warning() {
    let mut d = Fake::with_pane().then(Act::Swap(crossing_record()));
    let mut stderr = Vec::new();
    assert_eq!(
        run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
        0
    );
    assert!(stderr.is_empty(), "tmux delivery must not also exit 2");
    assert_eq!(d.keys, crossing_keys());
    assert_eq!(d.last_log().outcome, Outcome::ResumedKeys);
}

#[test]
fn losing_pane_ownership_on_the_affinity_path_stops_before_the_next_key() {
    let expected = crossing_keys();
    // Call one authorizes the path; each later call authorizes one key.
    for sent in 0..RESUME_KEYS.len() {
        let mut d = Fake::with_pane().then(Act::Swap(crossing_record()));
        d.lose_pane_on_call = Some(sent as u32 + 2);
        let mut stderr = Vec::new();
        assert_eq!(
            run_with_io(limit_payload().as_bytes(), &mut stderr, &mut d),
            0
        );
        assert!(stderr.is_empty(), "ownership loss must not select exit two");
        assert_eq!(
            d.keys,
            expected[..sent],
            "only keys typed before the change"
        );
        assert_eq!(d.key_attempts, sent, "no key reaches the changed pane");
        assert_eq!(d.guard.events.len(), 1, "committed authorization remains");
        assert_eq!(d.last_log().outcome, Outcome::ResumeKeysFailed);
        assert_eq!(d.last_log().keys_sent, Some(sent as u8));
        assert_eq!(d.last_log().detail.as_deref(), Some("PermissionDenied"));
        let lines = d.diagnostics.concat();
        assert!(
            lines.contains("stage=continuation-result disposition=resume-keys-failed"),
            "{lines}"
        );
        assert!(
            !lines.contains("disposition=resumed-keys"),
            "no success result after an ownership change: {lines}"
        );
    }
}
