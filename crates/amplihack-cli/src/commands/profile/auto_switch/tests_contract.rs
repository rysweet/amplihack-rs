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
    let e = ProfileError::AutoSwitchConfig("symlink".into());
    assert_eq!(
        e.hint(),
        "remove <root>/.auto-switch and run `amplihack profile auto-switch on` or `off`"
    );
    let e = ProfileError::AutoSwitchInvariant("ledger.json: unknown version".into());
    assert_eq!(
        e.hint(),
        "see the Recovery section of docs/howto/claude-profiles.md"
    );
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
