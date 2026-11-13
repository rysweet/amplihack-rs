//! Error contract for auto-switch (issue #1569): profile names go in the
//! message, hints are fixed text, and nothing secret is ever formatted.

use crate::commands::profile::ProfileError;

const STATUS_HINT: &str =
    "wait for that session to end, or run `amplihack profile auto-switch status`";

#[test]
fn leased_names_both_profiles_with_a_fixed_hint() {
    let e = ProfileError::Leased {
        profile: "personal".into(),
        host: "work".into(),
    };
    assert_eq!(
        e.to_string(),
        "profile \"personal\": its login is in use by a session in profile \"work\""
    );
    assert_eq!(e.hint(), STATUS_HINT);
}

#[test]
fn hosts_foreign_account_names_both_profiles_with_a_fixed_hint() {
    let e = ProfileError::HostsForeignAccount {
        profile: "work".into(),
        live: "personal".into(),
    };
    assert_eq!(
        e.to_string(),
        "profile \"work\": its directory holds the login of profile \"personal\" for a running session"
    );
    assert_eq!(e.hint(), STATUS_HINT);
}

#[test]
fn config_and_invariant_hints_are_fixed() {
    // Issue #2218: the generic hint never advises removing the file, whose
    // exclude list is in force in every case that reaches it.
    let e = ProfileError::AutoSwitchConfig("busy; retry".into());
    assert_eq!(
        e.hint(),
        "fix what the message names and retry; do not remove <root>/.auto-switch, which \
         holds the exclude list (see \"Settings file\" in docs/reference/profile-command.md)"
    );
    let e = ProfileError::AutoSwitchInvariant("ledger.json: unknown version".into());
    assert_eq!(
        e.hint(),
        "see the Recovery section of docs/howto/claude-profiles.md"
    );
}

/// Issue #2218: a corrupt file refused because removing it would drop its
/// exclude list never carries the generic "remove the file" hint.
#[test]
fn exclude_at_risk_hint_never_advises_removing_the_file() {
    let e = ProfileError::AutoSwitchExcludeAtRisk("x".into());
    assert_eq!(e.to_string(), "auto-switch settings: x");
    assert!(
        e.hint().starts_with("do not remove <root>/.auto-switch"),
        "{}",
        e.hint()
    );
    assert!(!e.hint().contains("and run"), "{}", e.hint());
}

#[test]
fn inherited_wrapper_keeps_the_lease_hint() {
    let e = ProfileError::Inherited {
        value: "\"X\"".into(),
        source: Box::new(ProfileError::HostsForeignAccount {
            profile: "X".into(),
            live: "Y".into(),
        }),
    };
    assert_eq!(e.hint(), STATUS_HINT);
}
