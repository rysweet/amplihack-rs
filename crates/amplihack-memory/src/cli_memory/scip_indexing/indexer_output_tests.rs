//! crates/amplihack-memory/src/cli_memory/scip_indexing/indexer_output_tests.rs
//!
//! Issue #1476 / AC6' — contracts for the per-language SCIP output ladder.
//!
//! Wire up from `indexer.rs` with:
//! ```ignore
//! #[cfg(test)]
//! #[path = "indexer_output_tests.rs"]
//! mod output_tests;
//! ```
//!
//! Every `scip-*` indexer has its own idea of where output goes, and the old
//! code relied on the convention that they all drop `index.scip` into the
//! current directory — which is the project checkout. The ladder replaces that
//! convention with an explicit per-language decision:
//!
//! * `Flag` — the tool takes an output path; pass it.
//! * `RootArgThenMove` — the tool takes the project root as an argument, so it
//!   can run from a tempdir outside the checkout and have its result moved.
//! * `Unsupported` — neither. The language is skipped with a named reason, and
//!   is *not* indexed into the repository.
//!
//! `Unsupported` is expected to stay empty. It exists so that discovering a
//! tool without an output flag during implementation cannot quietly
//! reintroduce the in-repo write while a stub-based test stays green.

use super::super::types::LANGUAGE_ORDER;
use super::{ScipOutput, scip_plan_for_language};
use std::path::Path;
use tempfile::TempDir;

fn plan_for(language: &str, project: &Path, output: &Path) -> super::ScipPlan {
    scip_plan_for_language(language, project, output)
        .unwrap_or_else(|e| panic!("no plan for {language}: {e:#}"))
}

/// The ladder must cover every language the detector can return. A language
/// that reaches `run_indexer_for_language` without a table entry is exactly the
/// case that falls back to convention.
#[test]
fn every_supported_language_has_a_plan() {
    let project = TempDir::new().unwrap();
    let output_dir = TempDir::new().unwrap();

    for language in LANGUAGE_ORDER {
        let output = output_dir.path().join(format!("{language}.scip"));
        let plan = plan_for(language, project.path(), &output);
        match &plan.output {
            ScipOutput::Unsupported { reason } => assert!(
                !reason.is_empty(),
                "{language} is skipped without saying why"
            ),
            _ => assert!(!plan.argv.is_empty(), "{language} has an empty argv"),
        }
    }
}

/// AC6' — the assertion that would have caught the bug. For every language that
/// runs, the explicit out-of-tree output path appears in the argv, and nothing
/// in the argv points inside the checkout except the project root itself.
#[test]
fn no_language_is_ever_told_to_write_inside_the_checkout() {
    let project = TempDir::new().unwrap();
    let output_dir = TempDir::new().unwrap();
    let project_str = project.path().display().to_string();

    for language in LANGUAGE_ORDER {
        let output = output_dir.path().join(format!("{language}.scip"));
        let plan = plan_for(language, project.path(), &output);

        match plan.output {
            ScipOutput::Unsupported { .. } => continue,
            ScipOutput::Flag => {
                assert!(
                    plan.argv.iter().any(|a| Path::new(a) == output),
                    "{language}: tier-1 argv must carry the explicit output path \
                     {}; got {:?}",
                    output.display(),
                    plan.argv
                );
            }
            ScipOutput::RootArgThenMove => {
                assert!(
                    plan.argv.iter().any(|a| a == &project_str),
                    "{language}: tier-2 must pass the project root explicitly, \
                     because it cannot take it from the working directory; got {:?}",
                    plan.argv
                );
                assert!(
                    !plan.working_dir.starts_with(project.path()),
                    "{language}: tier-2 must run from outside the checkout, got {}",
                    plan.working_dir.display()
                );
            }
        }

        for arg in &plan.argv {
            let candidate = Path::new(arg);
            if candidate == project.path() {
                continue; // the project root argument is the one legal in-tree path
            }
            assert!(
                !candidate.starts_with(project.path()),
                "{language}: argument {arg} points inside the checkout"
            );
        }
    }
}

/// Tier 1 runs from the checkout (that is how these tools find sources); tier 2
/// must not, because its working directory is not what selects the project.
#[test]
fn tier_one_runs_in_the_project_and_tier_two_does_not() {
    let project = TempDir::new().unwrap();
    let output_dir = TempDir::new().unwrap();

    for language in LANGUAGE_ORDER {
        let output = output_dir.path().join(format!("{language}.scip"));
        let plan = plan_for(language, project.path(), &output);
        match plan.output {
            ScipOutput::Flag => assert_eq!(plan.working_dir, project.path()),
            ScipOutput::RootArgThenMove => {
                assert_ne!(plan.working_dir, project.path())
            }
            ScipOutput::Unsupported { .. } => {}
        }
    }
}

/// The two flags actually verified in this repo today
/// (`amplihack-blarify/src/code_refs/scip.rs:44,59`). Pinned so a refactor of
/// the table cannot silently drop them to tier 3.
#[test]
fn python_and_typescript_are_tier_one_with_an_output_flag() {
    let project = TempDir::new().unwrap();
    let output_dir = TempDir::new().unwrap();

    for (language, binary) in [("python", "scip-python"), ("typescript", "scip-typescript")] {
        let output = output_dir.path().join(format!("{language}.scip"));
        let plan = plan_for(language, project.path(), &output);

        assert!(matches!(plan.output, ScipOutput::Flag), "{language}");
        assert_eq!(plan.argv.first().map(String::as_str), Some(binary));
        let flag_position = plan
            .argv
            .iter()
            .position(|a| a == "--output")
            .unwrap_or_else(|| panic!("{language}: no --output flag in {:?}", plan.argv));
        assert_eq!(
            Path::new(&plan.argv[flag_position + 1]),
            output,
            "{language}: --output must be followed by the resolved path"
        );
    }
}

/// `rust-analyzer scip <path>` takes its project root as an argument, so it is
/// the ladder's motivating tier-2 case regardless of whether it also grows an
/// output flag.
#[test]
fn rust_passes_the_project_root_explicitly() {
    let project = TempDir::new().unwrap();
    let output_dir = TempDir::new().unwrap();
    let output = output_dir.path().join("rust.scip");

    let plan = plan_for("rust", project.path(), &output);

    assert_eq!(plan.argv.first().map(String::as_str), Some("rust-analyzer"));
    assert!(
        plan.argv.iter().any(|a| Path::new(a) == project.path()),
        "rust-analyzer must be told which project to index: {:?}",
        plan.argv
    );
}

#[test]
fn unknown_languages_are_an_error_not_a_silent_convention() {
    let project = TempDir::new().unwrap();
    let output = TempDir::new().unwrap().path().join("x.scip");

    assert!(scip_plan_for_language("cobol", project.path(), &output).is_err());
}
