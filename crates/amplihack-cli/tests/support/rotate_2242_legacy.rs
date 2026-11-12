//! Frozen pre-#2242 journal reader. New active custody must refuse here.
//! This deliberately does not import the evolving production journal type.
use serde::Deserialize;

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PriorJournal {
    version: u32,
    from: String,
    to: String,
    adopt: bool,
    phase: String,
    home: String,
    live_ino: u64,
    target_ino: u64,
    home_lock_ino: Option<u64>,
    to_lock_ino: Option<u64>,
    started_at: i64,
    pid: u32,
    identity: Option<serde_json::Value>,
}

pub fn accepts(raw: &[u8]) -> bool {
    serde_json::from_slice::<PriorJournal>(raw).is_ok_and(|j| {
        j.version == 1
            && matches!(
                j.phase.as_str(),
                "checking" | "j0" | "s1" | "s2" | "s3" | "s4" | "inconsistent"
            )
    })
}

#[test]
fn issue_2242_frozen_reader_accepts_the_previous_journal_contract() {
    let old = serde_json::json!({"version":1,"from":"work","to":"spare","adopt":false,
        "phase":"checking","home":"/synthetic-home","live_ino":1,"target_ino":2,
        "started_at":1,"pid":1});
    assert!(accepts(&serde_json::to_vec(&old).unwrap()));
}
