//! Issue #1700: the API-key docs say what the code does. Each test derives
//! the strings from the code where it can, so a change on one side fails
//! here instead of drifting.

use super::kind::{Evidence, Kind, guard, resolve_from};
use super::name::ProfileName;
use super::rotate::identify::LoginKind;
use super::usage_parse::classify_key_error;

const REFERENCE: &str = include_str!("../../../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../../../docs/howto/claude-profiles.md");
const ROTATION: &str = include_str!("../../../../../docs/howto/claude-profile-rotation.md");
const SKILL: &str =
    include_str!("../../../../../amplifier-bundle/skills/claude-profile-rotation/SKILL.md");

/// Every source the resolver can produce appears, backticked, in the
/// reference's detection table.
#[test]
fn every_detected_source_is_documented() {
    use LoginKind::*;
    let mut seen = std::collections::BTreeSet::new();
    for own in [OAuth, NoOAuth, Unreadable, Missing] {
        for home in [
            None,
            Some(OAuth),
            Some(NoOAuth),
            Some(Unreadable),
            Some(Missing),
        ] {
            for key in [false, true] {
                for over in [None, Some(Kind::Platform)] {
                    let r = resolve_from(&Evidence {
                        override_kind: over,
                        own_login: own,
                        home_login: home,
                        key_setting: key,
                    });
                    seen.insert(r.source.to_string());
                }
            }
        }
    }
    assert_eq!(seen.len(), 9, "8 detected sources and override: {seen:?}");
    for s in &seen {
        assert!(
            REFERENCE.contains(&format!("`{s}`")),
            "reference lacks `{s}`"
        );
    }
}

#[test]
fn both_labels_are_documented() {
    for k in [Kind::Subscription, Kind::Platform] {
        assert!(
            REFERENCE.contains(&format!("`{}`", k.label())),
            "{}",
            k.label()
        );
    }
}

#[test]
fn the_launch_guard_refusal_is_quoted_word_for_word() {
    let r = resolve_from(&Evidence {
        override_kind: None,
        own_login: LoginKind::Missing,
        home_login: None,
        key_setting: false,
    });
    let err = guard(&r, &ProfileName::parse("old-team").unwrap(), false).unwrap_err();
    let line = format!("error: {err}");
    assert!(REFERENCE.contains(&line), "reference lacks: {line}");
}

/// Each phrase in the reference's key-error table classifies to its row.
#[test]
fn the_key_error_table_is_the_classifiers_table() {
    let mut rows = 0;
    for line in REFERENCE.lines() {
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        let Some(reason) = cols
            .get(1)
            .and_then(|c| c.strip_prefix('`')?.strip_suffix('`'))
        else {
            continue;
        };
        // `key_check_refused <name>: <reason>` is a log line, not a reason.
        if !reason.starts_with("key_") || reason.contains(' ') || cols.len() < 4 {
            continue;
        }
        rows += 1;
        let phrases: Vec<&str> = cols[2].split('`').skip(1).step_by(2).collect();
        assert!(!phrases.is_empty(), "{line}");
        for p in phrases {
            // `429` is documented together with `rate limit`.
            let text = if p == "429" || p == "rate limit" {
                "429 rate limit".to_string()
            } else {
                p.to_string()
            };
            assert_eq!(
                classify_key_error(&text).map(|(r, _)| r),
                Some(reason),
                "{p} in row {reason}"
            );
        }
    }
    assert_eq!(rows, 3, "key_exhausted, key_revoked, key_rate_limited");
}

#[test]
fn the_documented_log_lines_and_errors_are_present() {
    for s in [
        "per-token <name>: no plan login below 100%; <gate>; marked overflow tier <N> via profile overflow add; <check>",
        "moved ~/.claude onto API key profile <name> (billed per token)",
        "key_check_refused <name>: <reason>",
        "ALERT: API key profile <name> failed its login check (<reason>); not moving onto it",
        "would_refuse <name>: <reason>",
        "reason=plan-room",
        "moved ~/.claude off API key profile <name> to <target>",
        "ALERT: the API key is blocked (<reason>) until <time>; API-key profiles are not used",
        "ALERT: the API key is blocked (<reason>) and no plan or credit profile can be used; staying on <name>",
        "ALERT: more than one API-key profile could be live in ~/.claude; no swap",
        "ALERT: swap refused: <name> has no oauth login and is not an API-key profile",
        "skip: API-key profile <name> is not marked; it is never used. To allow per-token billing, run: amplihack profile overflow add <name>",
        "platform block not recorded: <error kind>",
        "profile <name> is an API-key profile; the API-key tier is fixed at 10, so --tier cannot be set",
        "PlatformTierFixed",
        "api-key  api-key tier  not marked (not used)",
        "api-key  api-key tier  marked",
        "live: work is running on the API key (billed per token)",
        "api-key (billed per token)",
        "overflow clear --platform",
        "amplihack profile kind set <name> <subscription|api-key|auto>",
    ] {
        assert!(REFERENCE.contains(s), "reference lacks: {s}");
    }
}

#[test]
fn the_how_tos_and_the_skill_describe_api_key_profiles() {
    for (name, doc) in [
        ("profiles how-to", HOWTO),
        ("rotation how-to", ROTATION),
        ("skill", SKILL),
    ] {
        assert!(doc.contains("API-key profile"), "{name}");
        assert!(doc.contains("billed per token"), "{name}");
    }
    assert!(
        ROTATION.contains("overflow clear --platform"),
        "rotation how-to"
    );
}

/// The launch guard is compiled only on Unix, and the reference says so.
/// If the guard is ever built on other platforms, this fails until the
/// "API-key platform support" section is updated to match.
#[test]
fn the_reference_says_the_launch_guard_is_unix_only() {
    const MOD_RS: &str = include_str!("mod.rs");
    let calls = MOD_RS.matches("kind_guard(&store").count();
    let unix_calls = MOD_RS
        .matches("#[cfg(unix)]\n    kind_guard(&store")
        .count()
        + MOD_RS
            .matches("#[cfg(unix)]\n            kind_guard(&store")
            .count();
    assert!(calls > 0, "no kind_guard call sites found");
    assert_eq!(calls, unix_calls, "a kind_guard call is not Unix-only");
    assert!(MOD_RS.contains("#[cfg(unix)]\nfn kind_guard("));
    assert!(REFERENCE.contains("### API-key platform support"));
    assert!(REFERENCE.contains("[API-key platform support](#api-key-platform-support)"));
    assert!(REFERENCE.contains(
        "Kind detection, `profile kind`, the `SubscriptionNoLogin` refusal and the\n\
         rotator's swap check exist only on Unix."
    ));
}
