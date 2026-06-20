use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate should live under <repo>/crates/amplihack-cli")
        .to_path_buf()
}

fn workflow_files() -> Vec<PathBuf> {
    let workflow_dir = repo_root().join(".github/workflows");
    let mut files = fs::read_dir(&workflow_dir)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", workflow_dir.display()))
        .map(|entry| entry.expect("workflow entry should be readable").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn load_yaml(path: &Path) -> Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read workflow {}: {err}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|err| panic!("workflow {} should be valid YAML: {err}", path.display()))
}

#[test]
fn ci_workflows_define_concurrency_boundaries() {
    let missing = workflow_files()
        .into_iter()
        .filter(|path| load_yaml(path).get("concurrency").is_none())
        .map(|path| {
            path.strip_prefix(repo_root())
                .unwrap()
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "each GitHub workflow must define top-level concurrency with cancel-in-progress; missing: {missing:?}"
    );
}

#[test]
fn ci_jobs_have_timeout_minutes() {
    let mut missing = Vec::new();
    for path in workflow_files() {
        let workflow = load_yaml(&path);
        let Some(jobs) = workflow.get("jobs").and_then(Value::as_mapping) else {
            continue;
        };
        for job_name in jobs.keys().filter_map(Value::as_str) {
            let job = &jobs[Value::String(job_name.to_string())];
            if job.get("timeout-minutes").is_none() {
                missing.push(format!(
                    "{}:{job_name}",
                    path.strip_prefix(repo_root()).unwrap().display()
                ));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "each CI job must set timeout-minutes so runaway jobs fail predictably; missing: {missing:?}"
    );
}

#[test]
fn ci_rust_cache_entries_do_not_cache_workspace_targets_by_default() {
    let mut offenders = Vec::new();
    for path in workflow_files() {
        let workflow = load_yaml(&path);
        let Some(jobs) = workflow.get("jobs").and_then(Value::as_mapping) else {
            continue;
        };
        for (job_key, job_value) in jobs {
            let Some(job_name) = job_key.as_str() else {
                continue;
            };
            let Some(steps) = job_value.get("steps").and_then(Value::as_sequence) else {
                continue;
            };
            for (index, step) in steps.iter().enumerate() {
                if step.get("uses").and_then(Value::as_str) != Some("Swatinem/rust-cache@v2") {
                    continue;
                }
                let cache_targets = step
                    .get("with")
                    .and_then(|with| with.get("cache-targets"))
                    .and_then(Value::as_bool);
                if cache_targets != Some(false) {
                    offenders.push(format!(
                        "{}:{job_name}:step{}",
                        path.strip_prefix(repo_root()).unwrap().display(),
                        index + 1
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "rust-cache usage must opt out of target/ caching unless a job documents a narrow exception; offenders: {offenders:?}"
    );
}
