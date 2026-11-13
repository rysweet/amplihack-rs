//! Publication must retain collection evidence before legacy display rules.
use super::*;
use crate::commands::profile::usage_request::{Limits, parse_limits};
use amplihack_utils::usage_observation::SessionWindow;
use serde_json::json;
const NOW: i64 = 1_790_000_000;

#[test]
fn a_missing_structured_session_is_unknown_but_explicit_null_confirms_absence() {
    for (session, expected) in [
        (None, SessionWindow::Unknown),
        (Some(json!(null)), SessionWindow::Absent),
    ] {
        let mut payload = json!({"seven_day":{"utilization":40}});
        if let Some(session) = session {
            payload["five_hour"] = session;
        }
        let Limits::Report { report, .. } = parse_limits(&payload, NOW) else {
            panic!("weekly report");
        };
        assert_eq!(report.observation.session_window, expected);
        assert_eq!(report.observation.session_used, None);
        assert_eq!(report.observation.week_used, Some(40.0));
    }
}

#[test]
fn a_measured_zero_is_retained_when_legacy_display_hides_its_session_window() {
    let Limits::Report { report, .. } = parse_limits(
        &json!({
            "five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":40}
        }),
        NOW,
    ) else {
        panic!("complete reading");
    };
    assert!(report.session.is_none(), "existing display compatibility");
    assert_eq!(report.observation.session_window, SessionWindow::Present);
    assert_eq!(report.observation.session_used, Some(0.0));
    assert_eq!(report.observation.session_reset_at, None);
}

#[test]
fn text_omission_cannot_establish_an_absent_session_limit_for_publication() {
    let week_only = parse_usage("Current week (all models): 40% used", "", NOW);
    assert_eq!(week_only.observation.session_window, SessionWindow::Unknown);
    let zero = parse_usage(
        "Current session: 0% used\nCurrent week (all models): 40% used",
        "",
        NOW,
    );
    assert_eq!(zero.observation.session_window, SessionWindow::Present);
    assert_eq!(zero.observation.session_used, Some(0.0));
}

#[test]
fn malformed_structured_resets_cannot_become_authoritative_unknown_resets() {
    let report = match super::super::usage_request::parse_limits(
        &serde_json::json!({
            "five_hour":{"utilization":10,"resets_at":"invalid"},
            "seven_day":{"utilization":20}, "extra_usage":{"is_enabled":false}
        }),
        1_790_000_000,
    ) {
        super::super::usage_request::Limits::Report { report, .. } => report,
        _ => panic!("legacy display still accepts the percentages"),
    };
    assert_eq!(report.observation, Default::default());
}
