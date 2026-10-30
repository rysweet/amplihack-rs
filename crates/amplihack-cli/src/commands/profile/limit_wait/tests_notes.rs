//! Issue #1761: the plain lines written after a cap ends the waiting, and
//! the check on `login=`, which prints a profile name only when it passes
//! the profile-name rule (SR-6). No plain line holds a runner usage-limit
//! marker, so the runner still reports Claude's own message.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_notes;`.

use super::events::{Event, budget_spent_note, line, step_cap_note};
use crate::commands::recipe::run::limit_patterns;

const LA: chrono_tz::Tz = chrono_tz::America::Los_Angeles;

fn login_line(login: &str) -> String {
    line(
        &Event::Resume {
            attempt: 2,
            step: 1,
            waited: 30,
            login: Some(login.to_string()),
        },
        &LA,
    )
}

#[test]
fn the_budget_line_names_the_setting_its_value_and_the_runs_scope() {
    assert_eq!(
        budget_spent_note(21_600, false),
        "amplihack: this recipe run's usage-limit wait budget \
         (recipe.usage_limit_wait_budget_secs = 21600 s) is spent; not waiting again"
    );
    assert_eq!(
        budget_spent_note(u64::MAX, false),
        format!(
            "amplihack: this recipe run's usage-limit wait budget \
             (recipe.usage_limit_wait_budget_secs = {} s) is spent; not waiting again",
            u64::MAX
        )
    );
}

#[test]
fn in_the_per_step_fallback_the_budget_line_names_the_step() {
    assert_eq!(
        budget_spent_note(600, true),
        "amplihack: this step's usage-limit wait budget \
         (recipe.usage_limit_wait_budget_secs = 600 s) is spent; not waiting again"
    );
}

#[test]
fn the_cap_line_names_the_setting_and_its_value() {
    assert_eq!(
        step_cap_note(5),
        "amplihack: this step's usage-limit waits \
         (recipe.usage_limit_max_waits_per_step = 5) are used up; not waiting again"
    );
    assert_eq!(
        step_cap_note(1),
        "amplihack: this step's usage-limit waits \
         (recipe.usage_limit_max_waits_per_step = 1) are used up; not waiting again"
    );
}

#[test]
fn no_plain_line_holds_a_usage_limit_marker_or_the_log_prefix() {
    let notes = [
        budget_spent_note(0, false),
        budget_spent_note(21_600, true),
        budget_spent_note(u64::MAX, false),
        step_cap_note(1),
        step_cap_note(100),
        step_cap_note(u32::MAX),
    ];
    let markers = limit_patterns::all_markers();
    for note in &notes {
        assert!(note.starts_with("amplihack: "), "{note}");
        assert!(note.ends_with("; not waiting again"), "{note}");
        assert!(
            !note.contains("amplihack.recipe.usage_limit_wait"),
            "{note}"
        );
        assert!(note.bytes().all(|b| (0x20..0x7f).contains(&b)), "{note:?}");
        let lower = note.to_ascii_lowercase();
        for m in &markers {
            assert!(
                !lower.contains(&m.to_ascii_lowercase()),
                "{note:?} holds the runner marker {m:?}"
            );
        }
        assert!(
            limit_patterns::limit_message(note.as_bytes(), b"", 0).is_none(),
            "{note:?} reads as a limit message"
        );
    }
}

#[test]
fn a_valid_profile_name_is_printed() {
    for name in ["team", "personal", "X", "a.b_c-1", &"a".repeat(64)] {
        assert!(
            login_line(name).ends_with(&format!(" login={name}")),
            "{}",
            login_line(name)
        );
    }
}

#[test]
fn anything_but_a_valid_profile_name_prints_unknown() {
    let too_long = "a".repeat(65);
    for bad in [
        "",
        "a@b.com",
        "me@example.invalid",
        "\u{1b}[31mred",
        "x\ny",
        "two words",
        "../etc",
        "trailing.",
        ".hidden",
        "-dash",
        "CON",
        "uuid=1 login=evil",
        too_long.as_str(),
    ] {
        let l = login_line(bad);
        assert!(l.ends_with(" login=unknown"), "{bad:?} gave {l:?}");
        if !bad.is_empty() {
            assert!(!l.contains(bad), "{bad:?} leaked into {l:?}");
        }
    }
}
