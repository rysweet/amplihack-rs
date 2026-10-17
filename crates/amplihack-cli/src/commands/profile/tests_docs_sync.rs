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
fn both_docs_describe_profile_usage() {
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        assert!(
            doc.contains("amplihack profile usage"),
            "{name} doc must mention `amplihack profile usage`"
        );
    }
}

/// The phrase table in the reference is the code's table, word for word.
#[test]
fn reference_lists_the_usage_phrases_the_parser_matches() {
    use super::usage_parse::{NOT_LOGGED_IN, SPEND_LIMIT};
    for (state, phrases) in [
        ("`not_logged_in`", NOT_LOGGED_IN),
        ("`spend_limit`", SPEND_LIMIT),
    ] {
        let row = REFERENCE
            .lines()
            .find(|l| l.starts_with(&format!("| {state} | `")))
            .unwrap_or_else(|| panic!("reference phrase row for {state}"));
        let documented = backticked(row.split('|').nth(2).unwrap());
        assert_eq!(documented, phrases, "{state} phrases differ from the code");
    }
}

#[test]
fn reference_lists_every_usage_error_reason() {
    use super::usage_cmd::ErrorReason;
    for reason in [
        ErrorReason::ProfileRefused,
        ErrorReason::StartFailed,
        ErrorReason::ReadFailed,
        ErrorReason::WaitFailed,
    ] {
        assert!(
            REFERENCE.contains(&format!("| `{}` |", reason.as_str())),
            "reference is missing error reason {:?}",
            reason.as_str()
        );
    }
}

#[test]
fn docs_state_spawn_time_revalidation_is_not_atomic() {
    for doc in [REFERENCE, HOWTO] {
        let d = doc.to_lowercase();
        assert!(d.contains("just before spawn") || d.contains("immediately before spawn"));
        assert!(d.contains("not atomic"), "docs must state the race remains");
    }
}

/// The `~/.claude` mark, its JSON field and its warnings are documented
/// word for word.
#[test]
fn docs_describe_logins_live_in_default_dir() {
    use super::default_live::{
        AMBIGUOUS_WARNING, LABEL, LEDGER_WARNING, LIST_WARNING, OWN_LOGIN_WARNING,
    };
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        for text in [
            format!("live in {LABEL}"),
            "\"live_in\"".to_string(),
            ".rotator/ledger.json".to_string(),
        ] {
            assert!(doc.contains(&text), "{name} doc is missing {text}");
        }
    }
    for w in [
        LEDGER_WARNING,
        AMBIGUOUS_WARNING,
        OWN_LOGIN_WARNING,
        LIST_WARNING,
    ] {
        assert!(REFERENCE.contains(w), "reference is missing {w:?}");
    }
    assert!(REFERENCE.contains("| `live_in` | string or `null` |"));
}
