//! Checks the launch core makes on a local-model launch: the Codex
//! version's hook-trust format (item 2 of the review), caller arguments
//! that turn hooks off (item 5) and a selection with no launch in scope
//! (item 6).
//!
//! The tests check behaviour: whether the launch goes on, and that what is
//! rejected is reported with the data that caused it.

use super::tests_fixture::*;
use super::{
    KNOWN_CODEX_VERSIONS, LOCAL_MODEL_ENV, ProfileName, caller_disables_hooks, check_in_scope,
    check_trust_format,
};

fn name() -> ProfileName {
    ProfileName(NAME.to_owned())
}

// ---- item 2: the Codex version ----

#[test]
fn an_interactive_launch_on_a_known_version_goes_on() {
    for version in KNOWN_CODEX_VERSIONS {
        for given in [version.to_string(), format!("codex-cli {version}")] {
            check_trust_format(&name(), Some(&given), &strings(&[]))
                .unwrap_or_else(|e| panic!("{given}: {e:#}"));
            check_trust_format(&name(), Some(&given), &strings(&["--model", "m"]))
                .unwrap_or_else(|e| panic!("{given}: {e:#}"));
        }
    }
}

#[test]
fn an_interactive_launch_on_an_unknown_version_is_rejected_with_the_version() {
    for version in ["0.167.0", "0.165.9", "codex-cli 1.0.0", "0.166.0-alpha.1"] {
        let args = strings(&[HOOK_TRUST]);
        let error = format!(
            "{:#}",
            check_trust_format(&name(), Some(version), &args)
                .expect_err("an unknown version must be rejected")
        );
        let shown = version.split_whitespace().last().unwrap();
        assert!(error.contains(shown), "{version}: {error}");
        assert!(error.contains(NAME), "{version}: {error}");
    }
}

#[test]
fn an_interactive_launch_whose_version_cannot_be_read_is_rejected() {
    for version in [None, Some(""), Some("   ")] {
        let error = check_trust_format(&name(), version, &strings(&[]))
            .expect_err("an unreadable version must be rejected");
        assert!(format!("{error:#}").contains(NAME), "{version:?}");
    }
}

/// An `exec` launch keeps the hook-trust bypass, so any version may run it.
#[test]
fn an_exec_launch_goes_on_whatever_the_version() {
    for version in [Some("0.167.0"), None] {
        check_trust_format(&name(), version, &strings(&["exec", "-"]))
            .unwrap_or_else(|e| panic!("{version:?}: {e:#}"));
    }
}

// ---- item 5: caller arguments that turn hooks off ----

#[test]
fn caller_arguments_that_turn_hooks_off_are_recognised() {
    for args in [
        &["-c", "features.hooks=false"][..],
        &["-cfeatures.hooks=false"],
        &["--config", "features.hooks=false"],
        &["--config=features.hooks=false"],
        &["-c", " features.hooks = false "],
        &["-c", "features.hooks=0"],
        &["--disable", "hooks"],
        &["--disable=hooks"],
        &["exec", "-c", "features.hooks=false", "-"],
        &["-c", "features={hooks=false}"],
        &["-c", "features = { hooks = false, goals = true }"],
        &["--config=features={hooks=0}"],
    ] {
        assert!(caller_disables_hooks(&strings(args)), "{args:?}");
    }
}

#[test]
fn other_caller_arguments_leave_hooks_on() {
    for args in [
        &[][..],
        &["-c", "features.hooks=true"],
        &["-c", "features.goals=false"],
        &["-c", "features={hooks=true}"],
        &["-c", "features={goals=false}"],
        &["-c", "features=not a table"],
        &["--disable", "goals"],
        &["exec", "-"],
        &["--", "-c", "features.hooks=false"],
        &["exec", "--", "--disable", "hooks"],
    ] {
        assert!(!caller_disables_hooks(&strings(args)), "{args:?}");
    }
}

/// The caller's argument still applies: the launch goes on with it after
/// amplihack's own setting. The stderr line is checked by the launch tests.
#[test]
fn a_caller_argument_that_turns_hooks_off_still_applies() {
    let f = LocalFixture::new();
    let local = super::prepare_launch(Some(NAME), no_conflicts())
        .expect("prepared")
        .expect("a local-model launch")
        .refresh()
        .expect("refreshed");
    f.assert_home_is_private_to_the_test();
    let args = local.codex_args(strings(&["--disable", "hooks"]));
    let ours = args
        .iter()
        .position(|a| a == "features.hooks=true")
        .unwrap();
    let theirs = args.iter().position(|a| a == "--disable").unwrap();
    assert!(ours < theirs, "{args:?}");
}

// ---- item 6: a selection with no launch in scope ----

#[test]
fn a_codex_launch_with_a_selection_but_no_scope_fails() {
    let mut f = LocalFixture::new();
    f.select(NAME);
    let error = check_in_scope("codex", false).expect_err("must fail");
    assert!(format!("{error:#}").contains(LOCAL_MODEL_ENV));
    check_in_scope("codex", true).expect("in scope");
    check_in_scope("claude", false).expect("another tool");
    f.set(LOCAL_MODEL_ENV, "");
    check_in_scope("codex", false).expect("an empty variable selects nothing");
    f.unset(LOCAL_MODEL_ENV);
    check_in_scope("codex", false).expect("no selection");
}
