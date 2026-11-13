//! Authority contracts missing from the continuation's path-only tests.
#[path = "support/issue_1554.rs"]
mod fixture;

use amplihack_git::{task_scope::TaskScope, workspace_validation::Workspace};
use fixture::{Repo, authority, isolated, remove, set};
use serde_json::{Value, json};

fn records() -> (Value, Value) {
    (
        json!({"version": 1, "task": "1554", "permitted_source_paths": ["source.txt"]}),
        json!({"version": 1, "task": "1554", "files_to_change": ["source.txt"]}),
    )
}

#[test]
fn literal_source_path_validation_rejects_ambiguous_or_git_metadata_names() {
    for path in [
        "source.txt",
        "src/new[1]*?.rs",
        "space in name.txt",
        "-source.rs",
    ] {
        assert!(
            amplihack_git::task_scope::valid_path(path),
            "valid literal filename refused: {path:?}"
        );
    }
    for path in [
        "",
        "/source.txt",
        "../source.txt",
        "src/../source.txt",
        "./source.txt",
        "src//source.txt",
        "src/",
        ".git/config",
        "nested/.GIT/config",
        "src\\source.txt",
        "src/\0file",
    ] {
        assert!(
            !amplihack_git::task_scope::valid_path(path),
            "unsafe filename accepted: {path:?}"
        );
    }
}

#[test]
fn both_authority_records_require_supported_integer_versions() {
    isolated(
        "both_authority_records_require_supported_integer_versions",
        || {
            let repo = Repo::new();
            let workspace = Workspace::open(&repo.root).unwrap();
            let mut accepted = Vec::new();
            for field in ["requirements", "design"] {
                for version in [Value::Null, json!(0), json!(2), json!("1")] {
                    let (mut requirements, mut design) = records();
                    let record = if field == "requirements" {
                        &mut requirements
                    } else {
                        &mut design
                    };
                    if version.is_null() {
                        record.as_object_mut().unwrap().remove("version");
                    } else {
                        record["version"] = version.clone();
                    }
                    authority(requirements, design);
                    if TaskScope::from_recipe(&workspace).is_ok() {
                        accepted.push(format!("{field}: {version}"));
                    }
                }
            }
            assert!(
                accepted.is_empty(),
                "unversioned or unsupported authority accepted: {accepted:?}"
            );
        },
    );
}

#[test]
fn unknown_authority_fields_fail_closed_in_both_records() {
    isolated(
        "unknown_authority_fields_fail_closed_in_both_records",
        || {
            let repo = Repo::new();
            let workspace = Workspace::open(&repo.root).unwrap();
            for field in ["requirements", "design"] {
                let (mut requirements, mut design) = records();
                let record = if field == "requirements" {
                    &mut requirements
                } else {
                    &mut design
                };
                record["allow_all_paths"] = json!(true);
                authority(requirements, design);
                assert!(
                    TaskScope::from_recipe(&workspace).is_err(),
                    "unknown {field} authority field accepted"
                );
            }
        },
    );
}

#[test]
fn malformed_primary_authority_never_falls_back_to_compatibility_records() {
    isolated(
        "malformed_primary_authority_never_falls_back_to_compatibility_records",
        || {
            let repo = Repo::new();
            let workspace = Workspace::open(&repo.root).unwrap();
            let mut accepted = Vec::new();
            for primary in ["RECIPE_VAR_final_requirements", "RECIPE_VAR_design_spec"] {
                for malformed in ["", "{invalid JSON"] {
                    let (requirements, design) = records();
                    authority(requirements.clone(), design.clone());
                    set("RECIPE_VAR_requirements", requirements.to_string());
                    set("RECIPE_VAR_design_consensus", design.to_string());
                    set(primary, malformed);
                    if TaskScope::from_recipe(&workspace).is_ok() {
                        accepted.push(format!("{primary}={malformed:?}"));
                    }
                }
            }
            assert!(
                accepted.is_empty(),
                "invalid primary authority bypassed: {accepted:?}"
            );
        },
    );
}

#[test]
fn compatibility_authority_applies_only_when_primary_records_are_absent() {
    isolated(
        "compatibility_authority_applies_only_when_primary_records_are_absent",
        || {
            let repo = Repo::new();
            let workspace = Workspace::open(&repo.root).unwrap();
            let (requirements, design) = records();
            remove("RECIPE_VAR_final_requirements");
            remove("RECIPE_VAR_design_spec");
            set("RECIPE_VAR_requirements", requirements.to_string());
            set("RECIPE_VAR_design_consensus", design.to_string());
            let scope = TaskScope::from_recipe(&workspace).unwrap();
            assert!(scope.permits("source.txt"));
            assert!(!scope.permits("protected.txt"));
        },
    );
}

#[test]
fn authority_is_the_exact_intersection_and_filename_metacharacters_are_literal() {
    isolated(
        "authority_is_the_exact_intersection_and_filename_metacharacters_are_literal",
        || {
            let repo = Repo::new();
            let workspace = Workspace::open(&repo.root).unwrap();
            authority(
                json!({"version": 1, "task": "1554", "permitted_source_paths": ["source.txt", "new[1]*?.rs", "requirements-only.txt"]}),
                json!({"version": 1, "task": "1554", "files_to_change": ["source.txt", "new[1]*?.rs", "design-only.txt"]}),
            );
            let scope = TaskScope::from_recipe(&workspace).unwrap();
            let accepted: Vec<_> = [
                "source.txt",
                "new[1]*?.rs",
                "new1abc.rs",
                "requirements-only.txt",
                "design-only.txt",
            ]
            .into_iter()
            .filter(|p| scope.permits(p))
            .collect();
            assert_eq!(accepted, ["source.txt", "new[1]*?.rs"]);
        },
    );
}

#[test]
fn existing_directory_names_cannot_grant_directory_wide_authority() {
    isolated(
        "existing_directory_names_cannot_grant_directory_wide_authority",
        || {
            let repo = Repo::new();
            std::fs::create_dir(repo.root.join("src")).unwrap();
            fixture::source_authority(&["src"]);
            let workspace = Workspace::open(&repo.root).unwrap();
            assert!(
                TaskScope::from_recipe(&workspace).is_err(),
                "directory-wide source authority accepted"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_directory_is_an_exact_git_filename_not_directory_authority() {
    isolated(
        "a_symlink_to_a_directory_is_an_exact_git_filename_not_directory_authority",
        || {
            let repo = Repo::new();
            let target = repo.temp.path().join("external-source-directory");
            std::fs::create_dir(&target).unwrap();
            std::os::unix::fs::symlink(&target, repo.root.join("source-link")).unwrap();
            fixture::source_authority(&["source-link"]);
            amplihack_git::checkpoint_transaction::checkpoint(
                &repo.root,
                "stage literal symlink",
                true,
            )
            .unwrap();
            assert!(
                fixture::text(&repo.root, &["ls-files", "--stage", "--", "source-link"])
                    .starts_with("120000 ")
            );
        },
    );
}
