//! Defect 2 (staging): `workflow_stage_task_changes.sh` stages the worktree,
//! then removes generated `.github/hooks/` paths and own-version churn in
//! Cargo.toml / Cargo.lock / package.json from the index.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use super::{Sandbox, assert_line, dump, text, write_file};

const STAGE: &str = "workflow_stage_task_changes.sh";

const CARGO_TOML: &str = "[package]\nname = \"demo\"\nversion = \"0.5.0\"\nedition = \"2021\"\n\n\
[dependencies.serde]\nversion = \"1.0.1\"\n";
const CARGO_LOCK: &str = "version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.5.0\"\n\
dependencies = [\n \"serde\",\n]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.1\"\n\
source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
const PACKAGE_JSON: &str = "{\n  \"name\": \"demo\",\n  \"version\": \"0.5.0\",\n  \"config\": {\n    \"version\": \"1\"\n  }\n}\n";

/// A plain repository with committed version files, `src/lib.rs` and a
/// nested crate.
pub(super) fn repo(sb: &Sandbox) -> PathBuf {
    let repo = sb.root().join("repo");
    sb.git(
        sb.root(),
        &["init", "-q", "-b", "main", repo.to_str().unwrap()],
    );
    write_file(&repo.join("Cargo.toml"), CARGO_TOML);
    write_file(&repo.join("Cargo.lock"), CARGO_LOCK);
    write_file(&repo.join("package.json"), PACKAGE_JSON);
    write_file(&repo.join("crates/inner/Cargo.toml"), CARGO_TOML);
    write_file(&repo.join("src/lib.rs"), "pub fn a() {}\n");
    sb.git(&repo, &["add", "-A"]);
    sb.git(&repo, &["commit", "-q", "-m", "base"]);
    sb.git(&repo, &["switch", "-q", "-c", "feature"]);
    repo
}

fn bump_own_versions(repo: &Path) {
    write_file(
        &repo.join("Cargo.toml"),
        &CARGO_TOML.replacen("0.5.0", "0.6.0", 1),
    );
    write_file(
        &repo.join("Cargo.lock"),
        &CARGO_LOCK.replacen("0.5.0", "0.6.0", 1),
    );
}

pub(super) fn touch_source(repo: &Path) {
    write_file(&repo.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n");
}

pub(super) fn stage(sb: &Sandbox, repo: &Path, env: &[(&str, &str)]) -> Output {
    sb.run_helper(repo, STAGE, &[], env)
}

pub(super) fn staged(sb: &Sandbox, repo: &Path) -> Vec<String> {
    sb.git(repo, &["diff", "--cached", "--name-only"])
        .lines()
        .map(str::to_string)
        .collect()
}

pub(super) fn git_path(sb: &Sandbox, repo: &Path, name: &str) -> PathBuf {
    let rel = sb.git(repo, &["rev-parse", "--git-path", name]);
    let path = PathBuf::from(&rel);
    if path.is_absolute() {
        path
    } else {
        repo.join(path)
    }
}

pub(super) fn assert_ok(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", dump(output));
    text(&output.stderr)
}

// --- version churn ---------------------------------------------------------

fn assert_churn_excluded(env: &[(&str, &str)], why: &str) {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    bump_own_versions(&repo);
    touch_source(&repo);
    let stderr = assert_ok(&stage(&sb, &repo, env));
    assert_eq!(staged(&sb, &repo), vec!["src/lib.rs"], "{stderr}");
    for file in ["Cargo.toml", "Cargo.lock"] {
        let prefix =
            format!("WARNING: step-15 excluded version-only change in {file} ({why}); saved to ");
        let line = stderr
            .lines()
            .find(|l| l.starts_with(&prefix))
            .unwrap_or_else(|| panic!("missing `{prefix}…`\n{stderr}"));
        assert!(
            line.ends_with("amplihack/excluded-version-churn.patch"),
            "{line}"
        );
    }
    assert_eq!(
        fs::read_to_string(repo.join("Cargo.toml")).unwrap(),
        CARGO_TOML,
        "restored"
    );
    assert_eq!(
        fs::read_to_string(repo.join("Cargo.lock")).unwrap(),
        CARGO_LOCK,
        "restored"
    );

    let patch = git_path(&sb, &repo, "amplihack/excluded-version-churn.patch");
    let meta = fs::symlink_metadata(&patch).expect("patch file");
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let check = sb.git_output(&repo, &["apply", "--check", patch.to_str().unwrap()]);
    assert!(
        check.status.success(),
        "the saved patch must apply\n{}",
        dump(&check)
    );
}

#[test]
fn churn_is_excluded_by_default() {
    assert_churn_excluded(&[], "no version bump requested by step 14");
}

#[test]
fn churn_is_excluded_when_allow_is_false_even_with_a_requested_bump() {
    assert_churn_excluded(
        &[
            ("ALLOW_VERSION_BUMP", "false"),
            ("VERSION_BUMP_NEW_VERSION", "0.6.0"),
            ("VERSION_BUMP_CURRENT_VERSION", "0.5.0"),
        ],
        "allow_version_bump=false",
    );
}

#[test]
fn churn_is_excluded_when_allowed_but_step14_requested_no_bump() {
    assert_churn_excluded(
        &[("ALLOW_VERSION_BUMP", "true")],
        "no version bump requested by step 14",
    );
    assert_churn_excluded(
        &[
            ("ALLOW_VERSION_BUMP", "true"),
            ("VERSION_BUMP_NEW_VERSION", "0.5.0"),
            ("VERSION_BUMP_CURRENT_VERSION", "0.5.0"),
        ],
        "no version bump requested by step 14",
    );
}

#[test]
fn nested_manifest_churn_is_excluded_too() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    write_file(
        &repo.join("crates/inner/Cargo.toml"),
        &CARGO_TOML.replacen("0.5.0", "0.6.0", 1),
    );
    touch_source(&repo);
    assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(staged(&sb, &repo), vec!["src/lib.rs"]);
}

fn assert_bump_kept(env: &[(&str, &str)], allow: &str) {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    bump_own_versions(&repo);
    let stderr = assert_ok(&stage(&sb, &repo, env));
    assert_eq!(
        staged(&sb, &repo),
        vec!["Cargo.lock", "Cargo.toml"],
        "{stderr}"
    );
    assert_line(
        &stderr,
        &format!(
            "INFO: step-15 keeping version change 0.5.0 -> 0.6.0 (allow_version_bump={allow})"
        ),
        "allowed bump",
    );
}

#[test]
fn requested_and_allowed_bump_is_kept_under_both_variable_names() {
    for allow in ["true", "1", "yes"] {
        assert_bump_kept(
            &[
                ("ALLOW_VERSION_BUMP", allow),
                ("VERSION_BUMP_NEW_VERSION", "0.6.0"),
                ("VERSION_BUMP_CURRENT_VERSION", "0.5.0"),
            ],
            allow,
        );
    }
    assert_bump_kept(
        &[
            ("ALLOW_VERSION_BUMP", "true"),
            ("RECIPE_VAR_version_bump__new_version", "0.6.0"),
            ("RECIPE_VAR_version_bump__current_version", "0.5.0"),
        ],
        "true",
    );
}

#[test]
fn allow_version_bump_is_case_insensitive() {
    for allow in ["TRUE", "Yes"] {
        assert_bump_kept(
            &[
                ("ALLOW_VERSION_BUMP", allow),
                ("VERSION_BUMP_NEW_VERSION", "0.6.0"),
                ("VERSION_BUMP_CURRENT_VERSION", "0.5.0"),
            ],
            &allow.to_lowercase(),
        );
    }
}

#[test]
fn version_strings_outside_the_allow_list_are_logged_as_invalid() {
    let long = "1".repeat(65);
    for bad in [
        "0.6.0$(touch pwned)",
        "0.6.0\u{1b}[31mred",
        "0.6.0 extra",
        long.as_str(),
    ] {
        let sb = Sandbox::new();
        let repo = repo(&sb);
        bump_own_versions(&repo);
        let stderr = assert_ok(&stage(
            &sb,
            &repo,
            &[
                ("ALLOW_VERSION_BUMP", "true"),
                ("VERSION_BUMP_NEW_VERSION", bad),
                ("VERSION_BUMP_CURRENT_VERSION", "0.5.0"),
            ],
        ));
        assert_line(
            &stderr,
            "INFO: step-15 keeping version change 0.5.0 -> <invalid> (allow_version_bump=true)",
            &format!("version string {bad:?}"),
        );
        assert!(
            !repo.join("pwned").exists(),
            "a version string must never run"
        );
    }
}

#[test]
fn mixed_version_file_keeps_every_version_file_staged() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    bump_own_versions(&repo);
    let mixed =
        CARGO_TOML.replacen("0.5.0", "0.6.0", 1) + "\n[dependencies.anyhow]\nversion = \"1\"\n";
    write_file(&repo.join("Cargo.toml"), &mixed);
    let stderr = assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(
        staged(&sb, &repo),
        vec!["Cargo.lock", "Cargo.toml"],
        "{stderr}"
    );
    assert_line(
        &stderr,
        "WARNING: step-15 kept version files staged: Cargo.toml mixes a version change with other edits",
        "mixed",
    );
}

#[test]
fn dependency_versions_are_never_treated_as_churn() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    write_file(
        &repo.join("Cargo.toml"),
        &CARGO_TOML.replace("1.0.1", "1.0.2"),
    );
    write_file(
        &repo.join("Cargo.lock"),
        &CARGO_LOCK.replace("1.0.1", "1.0.2"),
    );
    write_file(
        &repo.join("package.json"),
        &PACKAGE_JSON.replace("\"1\"", "\"2\""),
    );
    let stderr = assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(
        staged(&sb, &repo),
        vec!["Cargo.lock", "Cargo.toml", "package.json"],
        "{stderr}"
    );
    assert!(!stderr.contains("excluded version-only change"), "{stderr}");
}

#[test]
fn new_manifest_without_a_head_blob_is_never_churn() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    write_file(&repo.join("crates/fresh/Cargo.toml"), CARGO_TOML);
    assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(staged(&sb, &repo), vec!["crates/fresh/Cargo.toml"]);
}

#[test]
fn empty_worktree_exits_0_with_an_empty_index() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    assert_ok(&stage(&sb, &repo, &[]));
    assert!(staged(&sb, &repo).is_empty());
}

// --- failure safety --------------------------------------------------------

#[test]
fn planted_patch_symlink_is_not_followed() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    let victim = sb.root().join("victim.txt");
    write_file(&victim, "untouched\n");
    let patch = git_path(&sb, &repo, "amplihack/excluded-version-churn.patch");
    fs::create_dir_all(patch.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&victim, &patch).unwrap();
    bump_own_versions(&repo);
    touch_source(&repo);
    assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched\n");
    assert!(fs::symlink_metadata(&patch).unwrap().file_type().is_file());
}

#[test]
fn unwritable_patch_location_rolls_the_index_back_and_exits_2() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    bump_own_versions(&repo);
    touch_source(&repo);
    write_file(&repo.join("pre.txt"), "pre\n");
    sb.git(&repo, &["add", "pre.txt"]);
    let tree_before = sb.git(&repo, &["write-tree"]);
    // A regular file where the patch directory must go: mkdir/mktemp fail.
    let dir = git_path(&sb, &repo, "amplihack");
    write_file(&dir, "not a directory\n");

    let output = stage(&sb, &repo, &[]);
    assert_eq!(output.status.code(), Some(2), "{}", dump(&output));
    assert_eq!(
        sb.git(&repo, &["write-tree"]),
        tree_before,
        "index must be rolled back"
    );
    assert_ne!(
        fs::read_to_string(repo.join("Cargo.toml")).unwrap(),
        CARGO_TOML,
        "the working tree must not be restored when the patch could not be saved"
    );
}
