//! Selection of a local-model launch: the flag and the variable, the name
//! rule, the refused combinations and the scope the launch core reads.
//!
//! The contract `codex_local_model/mod.rs` is held to here:
//!
//! - `pub(crate) const LOCAL_MODEL_ENV: &str` (`AMPLIHACK_CODEX_LOCAL_MODEL`);
//! - `pub(crate) struct Conflicts { pub(crate) docker:
//!   Option<crate::docker::DockerActivation>, pub(crate) auto: bool }`;
//! - `pub(crate) fn requested(flag: Option<&str>) -> bool`;
//! - `pub(crate) fn prepare_launch(flag: Option<&str>, conflicts: Conflicts)
//!   -> anyhow::Result<Option<Prepared>>`, `None` for an ordinary launch;
//! - `pub(crate) fn scoped<T>(prepared: Option<Prepared>, action: impl
//!   FnOnce() -> T) -> T` and `pub(crate) fn current(tool: &str) ->
//!   Option<_>`.
//!
//! Errors are checked for what they name (the source, the path), never for
//! their wording.

use super::tests_fixture::*;
use super::{Conflicts, LOCAL_MODEL_ENV, current, prepare_launch, requested, scoped};
use crate::docker::DockerActivation;

fn error_text(result: anyhow::Result<Option<super::Prepared>>) -> String {
    match result {
        Ok(Some(_)) => panic!("expected an error, got a local-model launch"),
        Ok(None) => panic!("expected an error, got an ordinary launch"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn the_variable_is_named_as_documented() {
    assert_eq!(LOCAL_MODEL_ENV, "AMPLIHACK_CODEX_LOCAL_MODEL");
}

// ---- requested: any selection, valid or not ----

#[test]
fn requested_is_true_for_any_selection() {
    let mut f = LocalFixture::new();
    assert!(!requested(None), "no flag, no variable");
    assert!(requested(Some(NAME)));
    assert!(requested(Some("../not-valid")), "invalid names count (SF8)");
    f.select("");
    assert!(!requested(None), "an empty variable counts as unset");
    f.select(" ");
    assert!(requested(None), "the variable is not trimmed");
    f.select(NAME);
    assert!(requested(None));
    f.select("a/b");
    assert!(requested(None));
}

// ---- ordinary launches ----

#[test]
fn no_selection_is_an_ordinary_launch() {
    let mut f = LocalFixture::new();
    assert!(prepare_launch(None, no_conflicts()).unwrap().is_none());
    f.select("");
    assert!(prepare_launch(None, no_conflicts()).unwrap().is_none());
}

/// Docker and `--auto` are refused only for a local-model launch.
#[test]
fn conflicts_do_not_touch_an_ordinary_launch() {
    let _f = LocalFixture::new();
    for docker in [
        None,
        Some(DockerActivation::Flag),
        Some(DockerActivation::Env),
    ] {
        for auto in [false, true] {
            assert!(
                prepare_launch(None, Conflicts { docker, auto })
                    .unwrap()
                    .is_none(),
                "{docker:?} {auto}"
            );
        }
    }
}

// ---- the name rule (SEC-L1) ----

const BAD: &[&str] = &[
    "../x",
    "a/b",
    "a\\b",
    "a\0b",
    ".",
    "..",
    "a b",
    "caf\u{e9}",
    "x;y",
    "x\ny",
    "\u{1b}[31m",
    "x$y",
    "x*",
    "~x",
    " x",
];

#[test]
fn invalid_names_from_the_flag_are_refused_naming_the_flag() {
    let f = LocalFixture::new();
    let long = "a".repeat(65);
    for name in BAD.iter().copied().chain([long.as_str()]) {
        let text = error_text(prepare_launch(Some(name), no_conflicts()));
        assert!(text.contains("--local-model"), "{name:?}: {text}");
        assert!(
            !text.chars().any(|c| c.is_control()),
            "{name:?}: escaped: {text:?}"
        );
    }
    assert!(!f.codex_local().exists());
}

#[test]
fn invalid_names_from_the_variable_are_refused_naming_the_variable() {
    let mut f = LocalFixture::new();
    let long = "b".repeat(65);
    for name in BAD
        .iter()
        .copied()
        .filter(|n| !n.contains('\0'))
        .chain([long.as_str(), "  ", "\t"])
    {
        f.select(name);
        let text = error_text(prepare_launch(None, no_conflicts()));
        assert!(text.contains(LOCAL_MODEL_ENV), "{name:?}: {text}");
        assert!(
            !text.chars().any(|c| c.is_control()),
            "{name:?}: escaped: {text:?}"
        );
    }
    assert!(!f.codex_local().exists());
}

/// The value is quoted escaped and cut to 64 characters.
#[test]
fn a_long_invalid_name_is_cut_in_the_message() {
    let _f = LocalFixture::new();
    let name = format!("{}/", "\u{e9}".repeat(200));
    let text = error_text(prepare_launch(Some(&name), no_conflicts()));
    let longest = text
        .split(|c| c != '\u{e9}')
        .map(|run| run.chars().count())
        .max()
        .unwrap_or(0);
    assert!(longest <= 64, "{longest} characters of the value: {text}");
}

#[test]
fn valid_names_reach_the_profile() {
    let f = LocalFixture::new();
    let longest = format!("{}.v_1-x", "q".repeat(58));
    assert_eq!(longest.len(), 64);
    for name in [
        "inkling",
        "qwen3-coder.30b",
        "llama_8b",
        "A",
        "-x",
        "x..y",
        longest.as_str(),
    ] {
        f.write_profile(name, PROFILE);
        assert!(
            prepare_launch(Some(name), no_conflicts())
                .unwrap()
                .is_some(),
            "{name:?}"
        );
    }
}

#[test]
fn a_missing_profile_names_the_expected_path() {
    let f = LocalFixture::new();
    let text = error_text(prepare_launch(Some("absent"), no_conflicts()));
    let expected = f.user_codex().join("absent.config.toml");
    assert!(text.contains(&expected.display().to_string()), "{text}");
}

// ---- precedence ----

#[test]
fn the_flag_wins_over_the_variable() {
    let mut f = LocalFixture::new();
    f.write_profile("alpha", PROFILE);
    f.select("beta");
    assert!(
        prepare_launch(Some("alpha"), no_conflicts())
            .unwrap()
            .is_some(),
        "the flag's profile exists; the variable's does not"
    );
    // An invalid flag is never rescued by a valid variable.
    f.select("alpha");
    let text = error_text(prepare_launch(Some("../x"), no_conflicts()));
    assert!(text.contains("--local-model"), "{text}");
}

// ---- refused combinations, and their order ----

#[test]
fn docker_and_auto_are_refused_naming_their_source() {
    let f = LocalFixture::new();
    for (docker, auto, names) in [
        (Some(DockerActivation::Flag), false, "--docker"),
        (Some(DockerActivation::Env), false, "AMPLIHACK_USE_DOCKER"),
        (None, true, "--auto"),
        (Some(DockerActivation::Flag), true, "--docker"),
    ] {
        let text = error_text(prepare_launch(Some(NAME), Conflicts { docker, auto }));
        assert!(text.contains(names), "{docker:?} {auto}: {text}");
    }
    assert!(!f.codex_local().exists());
}

/// The name rule runs first: an invalid name with Docker on is a name error.
#[test]
fn the_name_is_checked_before_the_conflicts() {
    let _f = LocalFixture::new();
    let text = error_text(prepare_launch(
        Some("../x"),
        Conflicts {
            docker: Some(DockerActivation::Flag),
            auto: true,
        },
    ));
    assert!(text.contains("--local-model"), "{text}");
    assert!(
        !text.contains("--docker") && !text.contains("--auto"),
        "{text}"
    );
}

/// Conflicts are refused before the profile is read: no profile is needed.
#[test]
fn conflicts_are_refused_before_the_profile_is_read() {
    let _f = LocalFixture::new();
    let text = error_text(prepare_launch(
        Some("absent"),
        Conflicts {
            docker: None,
            auto: true,
        },
    ));
    assert!(text.contains("--auto"), "{text}");
}

// ---- prepare_launch writes nothing ----

#[test]
fn preparing_creates_nothing_on_disk() {
    let f = LocalFixture::new();
    let listing = |root: &std::path::Path| -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
        out.sort();
        out
    };
    let ahead = listing(f.home());
    assert!(
        prepare_launch(Some(NAME), no_conflicts())
            .unwrap()
            .is_some()
    );
    assert_eq!(listing(f.home()), ahead);
}

// ---- the scope the launch core reads ----

#[test]
fn current_is_set_only_inside_the_scope_and_only_for_codex() {
    let _f = LocalFixture::new();
    let prepared = prepare_launch(Some(NAME), no_conflicts()).unwrap();
    assert!(prepared.is_some());
    assert!(current("codex").is_none(), "outside the scope");
    scoped(prepared, || {
        assert!(current("codex").is_some());
        for other in ["claude", "copilot", "amplifier", "rustyclawd", ""] {
            assert!(current(other).is_none(), "{other:?}");
        }
    });
    assert!(
        current("codex").is_none(),
        "the scope restores what was there"
    );
    scoped(None, || assert!(current("codex").is_none()));
}

#[test]
fn the_scope_is_restored_after_a_panic() {
    let _f = LocalFixture::new();
    let prepared = prepare_launch(Some(NAME), no_conflicts()).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scoped(prepared, || panic!("launch failed"))
    }));
    assert!(result.is_err());
    assert!(current("codex").is_none());
}
