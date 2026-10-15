use super::store::SHARED_SETTINGS_KEYS;

const REFERENCE: &str = include_str!("../../../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../../../docs/howto/claude-profiles.md");

fn backticked(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

#[test]
fn constant_has_the_twelve_documented_keys() {
    assert_eq!(SHARED_SETTINGS_KEYS.len(), 12);
}

#[test]
fn both_docs_list_every_allowlisted_key() {
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        for key in SHARED_SETTINGS_KEYS {
            assert!(
                doc.contains(&format!("`{key}`")),
                "{name} doc is missing allowlisted key `{key}`"
            );
        }
    }
}

#[test]
fn howto_settings_row_lists_no_unknown_keys() {
    let row = HOWTO
        .lines()
        .find(|l| l.contains("settings.json` (filtered)"))
        .expect("howto settings row");
    let cols: Vec<&str> = row.split('|').collect();
    let keys = backticked(cols[2]);
    assert_eq!(keys.len(), SHARED_SETTINGS_KEYS.len(), "{keys:?}");
    for k in keys {
        assert!(SHARED_SETTINGS_KEYS.contains(&k), "unknown key `{k}`");
    }
}

#[test]
fn docs_state_spawn_time_revalidation_without_atomicity_claim() {
    for doc in [REFERENCE, HOWTO] {
        let d = doc.to_lowercase();
        assert!(d.contains("just before spawn") || d.contains("immediately before spawn"));
    }
    assert!(!HOWTO.to_lowercase().contains("atomically"));
}
