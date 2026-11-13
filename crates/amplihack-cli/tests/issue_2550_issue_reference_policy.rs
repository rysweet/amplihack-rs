//! Component contracts through the real CLI. `keyword` reads the existing
//! workflow_issue_reference.sh --classify representation on stdin.
#![cfg(unix)]

use crate::support::issue_2550::{Fixture, describe, input, succeeds};
use std::os::unix::ffi::OsStringExt;

#[test]
fn omitted_and_documented_policies_validate_without_stdout() {
    let f = Fixture::new();
    for policy in [None, Some("auto"), Some("refs"), Some("closes")] {
        let mut command = f.workflow("validate");
        if let Some(p) = policy {
            command.env("RECIPE_VAR_pr_issue_link", p);
        }
        let out = command.output().unwrap();
        succeeds(&out);
        assert!(
            out.stdout.is_empty(),
            "validation stdout: {}",
            describe(&out)
        );
    }
}

#[test]
fn policy_matrix_uses_substantive_evidence_and_conservative_empty_or_unusable_evidence() {
    let f = Fixture::new();
    for (policy, substantive) in [
        (None, "Closes\n"),
        (Some("auto"), "Closes\n"),
        (Some("refs"), "Refs\n"),
        (Some("closes"), "Closes\n"),
    ] {
        for (evidence, expected) in [
            ("substantive src/main.rs\n", substantive),
            ("planning docs/implementation-plan.md\n", "Refs\n"),
            ("generated Cargo.lock\n", "Refs\n"),
            ("", "Refs\n"),
            ("unrecognised src/main.rs\n", "Refs\n"),
            ("substantive src/main.rs\nunusable evidence\n", "Refs\n"),
            ("substantive\n", "Refs\n"),
        ] {
            let mut command = f.workflow("keyword");
            if let Some(p) = policy {
                command.env("RECIPE_VAR_pr_issue_link", p);
            }
            let out = input(command, evidence.as_bytes());
            succeeds(&out);
            assert_eq!(
                out.stdout,
                expected.as_bytes(),
                "policy={policy:?}, evidence={evidence:?}: {}",
                describe(&out)
            );
        }
    }
}

#[test]
fn actual_classifier_retains_planning_precedence_and_ordinary_documentation() {
    let f = Fixture::new();
    for (paths, expected) in [
        (
            "plans/implementation.rs\nCargo.lock\nCargo.toml\n",
            "Refs\n",
        ),
        ("docs/proposal.md\n", "Refs\n"),
        ("docs/howto/run-the-cli.md\n", "Closes\n"),
        ("plans/work.md\nsrc/main.rs\n", "Closes\n"),
    ] {
        let mut classifier = f.command("/bin/bash");
        classifier
            .arg(
                f.bundle
                    .join("amplifier-bundle/tools/workflow_issue_reference.sh"),
            )
            .arg("--classify");
        let classified = input(classifier, paths.as_bytes());
        succeeds(&classified);
        let mut keyword = f.workflow("keyword");
        keyword.env("RECIPE_VAR_pr_issue_link", "closes");
        let out = input(keyword, &classified.stdout);
        succeeds(&out);
        assert_eq!(
            out.stdout,
            expected.as_bytes(),
            "paths={paths:?}: {}",
            describe(&out)
        );
    }
}

#[test]
fn only_the_exported_recipe_value_controls_policy() {
    let f = Fixture::new();
    for (authoritative, inherited, expected) in [
        (None, "refs", "Closes\n"),
        (Some("refs"), "closes", "Refs\n"),
        (Some("closes"), "refs", "Closes\n"),
    ] {
        let mut command = f.workflow("keyword");
        command
            .env("PR_ISSUE_LINK", inherited)
            .env("pr_issue_link", inherited);
        if let Some(p) = authoritative {
            command.env("RECIPE_VAR_pr_issue_link", p);
        }
        let out = input(command, b"substantive src/main.rs\n");
        succeeds(&out);
        assert_eq!(out.stdout, expected.as_bytes(), "{}", describe(&out));
    }
}

#[test]
fn invalid_policy_is_rejected_before_evidence_and_diagnostics_escape_controls() {
    let f = Fixture::new();
    for invalid in [
        "",
        " ",
        "Auto",
        "REFS",
        "close",
        "always",
        "refs\ncloses",
        "\u{1b}[31m",
    ] {
        for action in ["validate", "keyword"] {
            let mut command = f.workflow(action);
            command.env("RECIPE_VAR_pr_issue_link", invalid);
            let out = input(command, b"substantive src/main.rs\n");
            assert_eq!(
                out.status.code(),
                Some(2),
                "{action}, {invalid:?}: {}",
                describe(&out)
            );
            assert!(out.stdout.is_empty(), "{}", describe(&out));
            let error = String::from_utf8(out.stderr).unwrap();
            assert!(
                error.contains("pr_issue_link")
                    && error.contains("auto")
                    && error.contains("refs")
                    && error.contains("closes"),
                "{error}"
            );
            assert!(
                !error.contains('\u{1b}'),
                "terminal control must be escaped: {error:?}"
            );
        }
    }
}

#[test]
fn non_unicode_policy_is_an_invocation_error() {
    let f = Fixture::new();
    let out = f
        .workflow("validate")
        .env(
            "RECIPE_VAR_pr_issue_link",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(out.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("pr_issue_link"),
        "{}",
        describe(&out)
    );
}

#[test]
fn unusable_binary_and_oversized_classifier_evidence_cannot_close_an_issue() {
    let f = Fixture::new();
    for evidence in [
        b"substantive src/main.rs\n\xff".to_vec(),
        format!("substantive {}\n", "x".repeat(1_048_576)).into_bytes(),
    ] {
        let out = input(f.workflow("keyword"), &evidence);
        succeeds(&out);
        assert_eq!(out.stdout, b"Refs\n", "{}", describe(&out));
    }
}

#[test]
fn policy_operations_do_not_repair_or_create_framework_assets() {
    let f = Fixture::new();
    let empty_bundle = f.root.join("absent-bundle");
    let out = f
        .workflow("validate")
        .env("AMPLIHACK_HOME", &empty_bundle)
        .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
        .env("RECIPE_VAR_pr_issue_link", "invalid")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("pr_issue_link"));
    assert!(
        !empty_bundle.exists(),
        "validation created framework assets"
    );
}
