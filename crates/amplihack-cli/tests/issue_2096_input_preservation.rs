//! Behavioral contracts at the production CLI preparation boundary.
use super::{contract_support as boundary, *};
use std::os::unix::{ffi::OsStringExt, fs::symlink};

#[test]
fn committed_index_and_working_contents_remain_three_distinct_layers() {
    let f = Fixture::new();
    fs::write(f.repo.join("tracked"), b"staged version\n").unwrap();
    fs::write(
        f.repo.join("staged-addition"),
        b"unreachable staged object\n",
    )
    .unwrap();
    f.git(&["add", "tracked", "staged-addition"]);
    fs::write(f.repo.join("tracked"), b"working version\n").unwrap();
    fs::remove_file(f.repo.join("staged-addition")).unwrap();
    let before = f.snapshot();
    let entries = f.git(&["ls-files", "--stage", "-z"]);
    succeeds(&boundary::command(&f, "layers").output().unwrap());
    assert_eq!(f.observed("layers", "head"), before.1);
    assert_eq!(f.observed("layers", "staged"), b"staged version\n");
    assert_eq!(f.observed("layers", "tracked"), b"working version\n");
    assert_eq!(f.observed("layers", "entries"), entries);
    assert_eq!(
        f.snapshot(),
        before,
        "capture must leave caller bytes intact"
    );
}

#[test]
fn required_untracked_and_binary_inputs_are_available_to_the_first_step() {
    let f = Fixture::new();
    fs::write(f.repo.join("untracked input"), b"local recipe dependency\n").unwrap();
    fs::write(f.repo.join("binary"), [0, 255, 13, 10, 0]).unwrap();
    // These literal relative paths are consumed by a supported local recipe.
    fs::write(f.root.join("recipe.yaml"), "name: issue-2096\nsteps:\n  - id: consume\n    type: bash\n    command: cat './untracked input' binary\n").unwrap();
    succeeds(&boundary::command(&f, "inputs").output().unwrap());
    assert_eq!(
        f.observed("inputs", "untracked input"),
        b"local recipe dependency\n"
    );
    assert_eq!(f.observed("inputs", "binary"), [0, 255, 13, 10, 0]);
}

#[test]
fn index_paths_with_newlines_and_non_utf8_bytes_are_preserved() {
    let f = Fixture::new();
    let name = std::ffi::OsString::from_vec(b"local\n\t\xff-input".to_vec());
    fs::write(f.repo.join(&name), b"byte-preserving contents\n").unwrap();
    let mut add = amplihack_git::command();
    f.environment(&mut add);
    succeeds(
        &add.current_dir(&f.repo)
            .args(["add", "--"])
            .arg(&name)
            .output()
            .unwrap(),
    );
    let expected_index = f.git(&["ls-files", "--stage", "-z"]);
    succeeds(
        &boundary::command(&f, "bytes")
            .env("ISSUE_2096_INPUT", &name)
            .output()
            .unwrap(),
    );
    assert_eq!(f.observed("bytes", "entries"), expected_index);
    assert_eq!(
        f.observed("bytes", "selected-input"),
        b"byte-preserving contents\n"
    );
}

#[test]
fn rejected_opaque_context_has_a_terminal_failure_report_without_launching_the_runner() {
    let f = Fixture::new();
    let before = f.snapshot();
    let output = boundary::command(&f, "opaque")
        .arg("-c")
        .arg(format!(
            "task_description=edit {} then commit",
            f.repo.display()
        ))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(!f.root.join("opaque/cwd").exists());
    let run = boundary::run_dir(&f);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("run.json")).unwrap()).unwrap();
    assert_eq!(report["outcome"], "failure");
    assert!(report["ended_at"].is_string());
    assert!(!run.join("runtime").exists());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn an_explicit_repo_path_is_mapped_to_the_execution_repository() {
    let f = Fixture::new();
    succeeds(
        &boundary::command(&f, "mapped")
            .arg("-c")
            .arg(format!("repo_path={}", f.repo.display()))
            .output()
            .unwrap(),
    );
    assert_eq!(
        boundary::path(&f, "mapped", "repo_path"),
        boundary::path(&f, "mapped", "cwd"),
        "a mutable context path must not send a step back to the caller"
    );
}

#[test]
fn an_explicit_ignored_local_recipe_is_copied_and_mapped_before_runner_launch() {
    let f = Fixture::new();
    fs::write(f.repo.join(".gitignore"), b"ignored-recipe.yaml\n").unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-qm", "ignored local recipe input"]);
    let recipe = f.repo.join("ignored-recipe.yaml");
    let contents =
        b"name: issue-2096\nsteps:\n  - id: local\n    type: bash\n    command: 'true'\n";
    fs::write(&recipe, contents).unwrap();
    let output = f
        .command_for_recipe("local-recipe", &f.repo, &recipe)
        .env("RECIPE_RUNNER_RS_PATH", boundary::driver())
        .env("ISSUE_2096_GIT", boundary::absolute_git())
        .output()
        .unwrap();
    succeeds(&output);
    assert_eq!(f.observed("local-recipe", "recipe-bytes"), contents);
    assert_ne!(
        boundary::path(&f, "local-recipe", "recipe-path"),
        recipe,
        "execution must not keep reading a mutable caller input by absolute path"
    );
}

#[test]
fn escaping_required_symlinks_fail_before_execution_with_an_actionable_error() {
    let f = Fixture::new();
    fs::write(f.root.join("outside-input"), b"caller-owned\n").unwrap();
    symlink("../outside-input", f.repo.join("escaping-input")).unwrap();
    f.git(&["add", "escaping-input"]);
    fs::write(f.root.join("recipe.yaml"), "name: issue-2096\nsteps:\n  - id: consume\n    type: bash\n    command: cat escaping-input\n").unwrap();
    let output = boundary::command(&f, "escape").output().unwrap();
    assert!(
        !output.status.success(),
        "unsafe required input was silently substituted: {output:?}"
    );
    assert!(
        !f.root.join("escape/cwd").exists(),
        "validation must precede runner launch"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("escaping-input")
            && (diagnostic.contains("symlink") || diagnostic.contains("input")),
        "{diagnostic}"
    );
    assert_eq!(
        fs::read(f.root.join("outside-input")).unwrap(),
        b"caller-owned\n"
    );
}

#[test]
fn corrupt_git_metadata_is_not_treated_as_confirmed_nonrepository_operation() {
    let f = Fixture::new();
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    fs::write(
        f.repo.join(".git"),
        b"gitdir: /missing/issue-2096-metadata\n",
    )
    .unwrap();
    let output = boundary::command(&f, "corrupt").output().unwrap();
    assert!(
        !output.status.success(),
        "discovery failure must block launch: {output:?}"
    );
    assert!(!f.root.join("corrupt/cwd").exists());
}

#[test]
fn preparation_never_executes_caller_post_checkout_hooks() {
    preparation_does_not_execute_caller_code(false);
}

#[test]
fn preparation_never_executes_caller_smudge_filters() {
    preparation_does_not_execute_caller_code(true);
}

fn preparation_does_not_execute_caller_code(filter: bool) {
    let f = Fixture::new();
    let marker = f.root.join("preparation-executed");
    if filter {
        fs::write(f.repo.join(".gitattributes"), b"tracked filter=untrusted\n").unwrap();
        f.git(&["add", ".gitattributes"]);
        f.git(&["commit", "-qm", "filter declaration"]);
        let executable = boundary::driver().to_str().unwrap().replace('\'', "'\\''");
        f.git(&[
            "config",
            "filter.untrusted.smudge",
            &format!("'{executable}' --filter"),
        ]);
        f.git(&["config", "filter.untrusted.required", "true"]);
    } else {
        fs::copy(boundary::driver(), f.repo.join(".git/hooks/post-checkout")).unwrap();
    }
    let before = f.snapshot();
    let output = boundary::command(&f, "prepare")
        .env("ISSUE_2096_PREPARATION_MARKER", &marker)
        .output()
        .unwrap();
    assert!(
        !marker.exists(),
        "untrusted preparation code ran (filter={filter}): {output:?}"
    );
    succeeds(&output);
    assert_eq!(f.snapshot(), before);
}
