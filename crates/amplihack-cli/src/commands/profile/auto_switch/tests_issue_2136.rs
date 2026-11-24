//! #2136: nested reruns exercise the production auto-switch selection boundary.
//! Temporary metadata only; no real profile root or Claude process.
use super::config::Config;
use super::rerun::nested_candidate;
use super::state::{Ledger, State};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn pick(same_reading: Option<f64>, exclude_same: bool, hosted: bool) -> Option<String> {
    pick_with_threshold(same_reading, exclude_same, hosted, 90)
}

fn pick_with_threshold(
    same_reading: Option<f64>,
    exclude_same: bool,
    hosted: bool,
    threshold: u8,
) -> Option<String> {
    pick_from(same_reading, 1.0, exclude_same, hosted, threshold, false)
}

/// The source `work` (organisation `org-a`) is a paid overflow login that
/// is not blocked; `same` (org-a) and `other` (org-b) are plan logins, and
/// `hosted` (org-b) is exhausted.
fn pick_from_overflow(same_reading: Option<f64>, other_reading: f64) -> Option<String> {
    pick_from(same_reading, other_reading, false, false, 90, true)
}

fn pick_from(
    same_reading: Option<f64>,
    other_reading: f64,
    exclude_same: bool,
    hosted: bool,
    threshold: u8,
    work_is_overflow: bool,
) -> Option<String> {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir(&fake_home).unwrap();
    let _env = crate::test_support::EnvGuard::set([
        ("HOME", fake_home.to_str().unwrap()),
        (
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
            tmp.path().to_str().unwrap(),
        ),
    ]);
    let _gate = crate::test_support::EnvGuard::unset([
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ]);
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let state = State::new(tmp.path()).unwrap();
    let profiles = vec![n("work"), n("same"), n("other"), n("hosted")];
    for (name, org) in [
        ("work", "org-a"),
        ("same", "org-a"),
        ("other", "org-b"),
        ("hosted", "org-b"),
    ] {
        fs::create_dir(tmp.path().join(name)).unwrap();
        fs::write(
            tmp.path().join(name).join(".credentials.json"),
            "synthetic-login",
        )
        .unwrap();
        state
            .write_sub_json(
                "expected",
                name,
                &serde_json::json!({
                    "account": format!("u-{name}"), "org": org
                }),
            )
            .unwrap();
        fs::write(
            tmp.path().join(name).join(".claude.json"),
            serde_json::json!({
                "oauthAccount": {"accountUuid": format!("u-{name}"), "organizationUuid": org}
            })
            .to_string(),
        )
        .unwrap();
    }
    let at = super::fsx::now();
    let reading = |used| Reading {
        five_hour: Some(Window {
            used_percentage: used,
            resets_at: at + 86400,
        }),
        seven_day: Some(Window {
            used_percentage: used,
            resets_at: at + 86400,
        }),
        observed_at: at,
        source: ReadingSource::Statusline,
    };
    let mut readings = BTreeMap::from([
        ("work".into(), reading(100.0)),
        ("other".into(), reading(other_reading)),
    ]);
    if let Some(used) = same_reading {
        readings.insert("same".into(), reading(used));
    }
    if work_is_overflow {
        // An unread plan login counts as capacity: read `hosted` (org-b) as
        // exhausted so only `same` and `other` are ways off overflow.
        readings.insert("hosted".into(), reading(100.0));
    }
    let cfg = Config {
        enabled: true,
        threshold,
        exclude: if exclude_same {
            vec![n("same")]
        } else {
            vec![]
        },
        overflow: if work_is_overflow {
            vec![crate::commands::profile::overflow::OverflowEntry::new(
                n("work"),
                1,
            )]
        } else {
            vec![]
        },
        kind: Default::default(),
    };
    let mut ledger = Ledger::default();
    if hosted {
        // Keep the hosted source distinct from the free target: the leased
        // source's own slot must remain ineligible under the existing rules.
        readings.insert("hosted".into(), reading(100.0));
        ledger.hosts.insert(
            "work".into(),
            super::state::HostEntry {
                live: "hosted".into(),
                swapped_at: at,
                pre_swap_fingerprint: None,
            },
        );
    }
    nested_candidate(
        &state,
        &ledger,
        &profiles,
        &readings,
        &cfg,
        "work",
        &["work".into()],
    )
    .map(|p| p.profile.to_string())
}

#[test]
fn same_org_capacity_beats_better_cross_org_headroom() {
    assert_eq!(pick(Some(40.0), false, false).as_deref(), Some("same"));
}
#[test]
fn unread_same_org_candidate_is_not_bypassed_by_known_cross_org_candidate() {
    assert_eq!(pick(None, false, false).as_deref(), Some("same"));
}
#[test]
fn exhausted_same_org_does_not_prevent_necessary_crossing() {
    assert_eq!(pick(Some(100.0), false, false).as_deref(), Some("other"));
}
#[test]
fn affinity_never_reenables_an_excluded_candidate() {
    assert_eq!(pick(Some(40.0), true, false).as_deref(), Some("other"));
}
#[test]
fn hosting_metadata_defines_source_org_instead_of_directory_name() {
    assert_eq!(pick(Some(40.0), false, true).as_deref(), Some("other"));
}

#[test]
fn issue_2136_same_org_room_above_threshold_does_not_hide_relief() {
    assert_eq!(pick(Some(95.0), false, false).as_deref(), Some("other"));
}

#[test]
fn configured_threshold_is_exclusive_for_same_org_relief() {
    for threshold in [50, 80, 90, 99] {
        assert_eq!(
            pick_with_threshold(Some(f64::from(threshold)), false, false, threshold).as_deref(),
            Some("other"),
            "at threshold {threshold}"
        );
        assert_eq!(
            pick_with_threshold(Some(f64::from(threshold) - 0.1), false, false, threshold)
                .as_deref(),
            Some("same"),
            "below threshold {threshold}"
        );
    }
}

/// Crusty round 7 on #2295: leaving a paid overflow login is cost relief.
/// With the source organisation recorded and no same-organisation plan login
/// with capacity, the session goes back to a plan login in another
/// organisation rather than paying until the overflow login is blocked.
#[test]
fn known_organisation_overflow_returns_to_a_cross_organisation_plan_login() {
    assert_eq!(
        pick_from_overflow(Some(100.0), 1.0).as_deref(),
        Some("other")
    );
}

/// The crossing is a last resort: a same-organisation plan login with
/// capacity is taken first, even with less headroom.
#[test]
fn overflow_exit_prefers_a_same_organisation_plan_login() {
    assert_eq!(pick_from_overflow(Some(40.0), 1.0).as_deref(), Some("same"));
}

/// The room fallback of an overflow exit may cross as well: a plan login
/// below 100% costs nothing, whatever its organisation.
#[test]
fn overflow_exit_room_fallback_may_cross() {
    assert_eq!(
        pick_from_overflow(Some(100.0), 95.0).as_deref(),
        Some("other")
    );
    // A same-organisation login with room still comes first.
    assert_eq!(
        pick_from_overflow(Some(95.0), 95.0).as_deref(),
        Some("same")
    );
}
