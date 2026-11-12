//! JSON-facing compatibility contracts; compile against the pre-fix parser.
use amplihack_utils::rotator_status::swap_record::{Judgement, judge, parse_swap};
use serde_json::{Value, json};
const NOW: i64 = 1_792_592_000;
fn legacy() -> Value {
    json!({"seq":1,"live_login":"spare","from_login":"work","swapped_at":NOW,
        "reading":{"session":10,"week":20,"measured_at":NOW}})
}
#[test]
fn legacy_records_stay_readable_without_inventing_affinity() {
    let r = parse_swap(&serde_json::to_vec(&legacy()).unwrap(), NOW).unwrap();
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("affinity").is_none_or(Value::is_null));
    assert_eq!(judge(None, &r, NOW, None), Judgement::Resume);
}
#[test]
fn explicit_affinity_survives_parsing_for_reporting() {
    for (from, to, outcome) in [
        (Some("org-a"), Some("org-a"), "same"),
        (Some("org-a"), Some("org-b"), "crossing"),
        (Some("org-a"), None, "unresolved"),
        (None, None, "unresolved"),
    ] {
        let mut v = legacy();
        v["from_org"] = json!(from);
        v["to_org"] = json!(to);
        v["affinity"] = json!(outcome);
        let r = parse_swap(&serde_json::to_vec(&v).unwrap(), NOW).unwrap();
        let roundtrip = serde_json::to_value(&r).unwrap();
        assert_eq!(roundtrip["affinity"], outcome);
        assert_eq!(roundtrip["from_org"], json!(from));
        assert_eq!(roundtrip["to_org"], json!(to));
        assert_eq!(
            judge(None, &r, NOW, None),
            Judgement::Resume,
            "affinity must not change resume authority"
        );
    }
}
#[test]
fn contradictory_affinity_is_not_accepted_as_committed_evidence() {
    for (from, to, outcome) in [
        (Some("org-a"), Some("org-b"), "same"),
        (None, None, "same"),
        (Some("org-a"), Some("org-a"), "crossing"),
        (None, Some("org-a"), "crossing"),
        (Some("org-a"), Some("org-b"), "unresolved"),
    ] {
        let mut v = legacy();
        v["from_org"] = json!(from);
        v["to_org"] = json!(to);
        v["affinity"] = json!(outcome);
        assert!(
            parse_swap(&serde_json::to_vec(&v).unwrap(), NOW).is_none(),
            "{v}"
        );
    }
}
#[test]
fn malformed_organisation_or_outcome_is_rejected() {
    for bad in [json!(""), json!("org\u{1b}[2J"), json!(123), json!([])] {
        let mut v = legacy();
        v["from_org"] = bad;
        v["to_org"] = json!("org-b");
        v["affinity"] = json!("crossing");
        assert!(
            parse_swap(&serde_json::to_vec(&v).unwrap(), NOW).is_none(),
            "{v}"
        );
    }
    let mut v = legacy();
    v["affinity"] = json!("future-unsupported-outcome");
    assert!(parse_swap(&serde_json::to_vec(&v).unwrap(), NOW).is_none());
}
