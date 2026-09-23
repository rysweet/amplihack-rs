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

// `ci_jobs_have_timeout_minutes` used to live here. It required every job in
// every workflow to set `timeout-minutes`, on the reasoning that a runaway job
// should "fail predictably".
//
// It does not fail predictably. It fails as `The operation was canceled.`, with
// no step named and no output kept, which is indistinguishable from a cancelled
// run and reads as a failed required check on a pull request that did nothing
// wrong. That is what happened here: pinning kani stopped the proof gate
// crashing early, the gate began running to completion, and a 25-minute budget
// killed Lint & Format at 25m22s with every other job green. The diagnosis cost
// two full CI runs and a local kani install, because the timeout discarded the
// evidence.
//
// A wall clock cannot tell a slow job from a hung one. Slow CI is a thing to
// measure and fix -- and it was measured: the slowdown was cache deletion after
// 19 idle days, addressed in #1469. Converting it into red checks on unrelated
// pull requests only hides it. Without `timeout-minutes` GitHub still applies
// its own 6-hour ceiling, so a genuinely hung job is still reaped.
//
// The two tests either side of this comment stay: concurrency boundaries and
// the target/ caching opt-out are real resource discipline, and neither
// destroys the evidence when it trips.

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
                // Match rust-cache by action prefix so the check keeps working
                // whether the ref floats (`@v2`) or is SHA-pinned with a
                // trailing version comment (`@<sha> # v2`, issue #951). YAML
                // strips the comment, leaving the SHA, so an exact `@v2` match
                // would silently skip every pinned step and void this test.
                let is_rust_cache = step
                    .get("uses")
                    .and_then(Value::as_str)
                    .is_some_and(|u| u.starts_with("Swatinem/rust-cache@"));
                if !is_rust_cache {
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
