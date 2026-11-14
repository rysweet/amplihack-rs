//! Descendant coverage at the CLI boundary. The Rust substitute tests do
//! not establish pinned-runner/real-agent coverage; see the dedicated test.
use super::{contract_support as boundary, *};

fn assert_attributed_add(f: &Fixture, label: &str, filename: &str) {
    let records = boundary::records(f);
    let adds: Vec<_> = records
        .iter()
        .filter(|entry| {
            entry["argv"].as_array().is_some_and(|argv| {
                argv.iter().any(|arg| arg == "add") && argv.iter().any(|arg| arg == filename)
            })
        })
        .collect();
    assert_eq!(
        adds.len(),
        1,
        "executed add must have one durable record: {records:#?}"
    );
    let add = adds[0];
    assert!(
        add["schema_version"]
            .as_u64()
            .is_some_and(|version| version > 0),
        "versioned command evidence is required: {add}"
    );
    for field in [
        "run_id",
        "attempt_id",
        "executable",
        "cwd",
        "index_file",
        "git_dir",
        "time",
    ] {
        assert!(
            add[field].as_str().is_some_and(|value| !value.is_empty()),
            "missing {field}: {add}"
        );
    }
    assert!(Path::new(add["executable"].as_str().unwrap()).is_absolute());
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        assert!(
            add["git_environment"].get(key).is_some(),
            "record even the explicit absence of launch routing {key}: {add}"
        );
    }
    assert_eq!(
        Path::new(add["cwd"].as_str().unwrap()),
        boundary::path(f, label, "cwd")
    );
    assert_eq!(
        Path::new(add["index_file"].as_str().unwrap()),
        boundary::path(f, label, "index")
    );
}

#[test]
fn absolute_git_with_a_cleared_child_environment_is_observed_and_attributed() {
    let f = Fixture::new();
    succeeds(
        &boundary::command(&f, "untraced")
            .env("ISSUE_2096_MODE", "untraced-add")
            .output()
            .unwrap(),
    );
    assert_eq!(
        f.observed("untraced", "git-succeeded"),
        b"yes",
        "prove execution independently of the journal"
    );
    assert_attributed_add(&f, "untraced", "untraced-output");
}

#[test]
fn a_rust_commit_hook_descendant_that_drops_trace_variables_is_recorded() {
    let f = Fixture::new();
    let before = f.snapshot();
    succeeds(
        &boundary::command(&f, "hook")
            .env("ISSUE_2096_MODE", "hook-commit")
            .output()
            .unwrap(),
    );
    assert_eq!(f.observed("hook", "hook-committed"), b"hook descendant\n");
    assert_eq!(f.snapshot(), before);
    assert_attributed_add(&f, "hook", "hook-output");
}

#[test]
fn empty_evidence_requires_an_explicit_established_coverage_receipt() {
    let f = Fixture::new();
    succeeds(&boundary::command(&f, "idle").output().unwrap());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(boundary::run_dir(&f).join("run.json")).unwrap()).unwrap();
    assert_eq!(
        report["git_recording"]["coverage"], "established",
        "an empty cooperative trace is not proof of observation: {report}"
    );
    assert_eq!(report["git_recording"]["complete"], true);
}

#[test]
fn pinned_runner_shell_to_rust_to_absolute_git_keeps_required_coverage() {
    let f = Fixture::new();
    let runner = std::env::var_os("RECIPE_RUNNER_QA_BINARY")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|dir| dir.join("recipe-runner-rs"))
                .find(|path| path.is_file())
        })
        .expect("required external runner missing; install claude-plugin/recipe-runner.rev or set RECIPE_RUNNER_QA_BINARY");
    let receipt_path = std::env::var_os("RECIPE_RUNNER_QA_RECEIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            runner
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(".crates2.json")
        });
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&receipt_path)
        .unwrap_or_else(|error| panic!("verify pinned runner via {}: {error}; provide the Cargo install receipt with RECIPE_RUNNER_QA_RECEIPT", receipt_path.display())))
        .expect("valid runner Cargo install receipt");
    let pin = include_str!("../../../claude-plugin/recipe-runner.rev").trim();
    let suffix = format!("#{pin})");
    assert!(
        receipt["installs"]
            .as_object()
            .expect("Cargo installs map")
            .iter()
            .any(|(source, install)| {
                source.starts_with("recipe-runner-rs ")
                    && source.contains("git+https://github.com/rysweet/amplihack-recipe-runner")
                    && source.ends_with(&suffix)
                    && install["bins"]
                        .as_array()
                        .is_some_and(|bins| bins.iter().any(|name| name == "recipe-runner-rs"))
            }),
        "runner receipt does not establish pinned revision {pin}; install that revision before claiming pinned-boundary coverage"
    );
    fs::write(
        f.root.join("git-path"),
        boundary::absolute_git().as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    let quote = |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    let recipe = serde_json::json!({
        "name": "issue-2096-real-runner", "version": "1.0.0",
        "description": "Observe actual runner descendants without cooperative tracing",
        "steps": [{"id": "rust-descendant", "type": "bash", "command": format!("{} --step-root {} --mode untraced-add", quote(boundary::driver()), quote(&f.root))}]
    });
    fs::write(
        f.root.join("recipe.yaml"),
        serde_yaml::to_string(&recipe).unwrap(),
    )
    .unwrap();
    // Everything after CLI launch, including the runner's update cache,
    // stays under the fixture home. No network lookup or real agent login.
    let output = f
        .command("real-runner")
        .env("RECIPE_RUNNER_RS_PATH", runner)
        .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    succeeds(&output);
    assert_eq!(f.observed("real-runner", "git-succeeded"), b"yes");
    assert_attributed_add(&f, "real-runner", "untraced-output");
}
