//! Issue #1553: `default-workflow.yaml` `max_total_steps` must not be below the
//! recursively counted number of steps it can run.

use serde_yaml::Value;
use std::path::{Path, PathBuf};

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/recipes")
}

fn load(name: &str) -> Value {
    assert!(
        !name.is_empty() && !name.contains("..") && !name.contains('/') && !name.contains('\\'),
        "unsafe recipe reference: {name}"
    );
    let path = recipes_dir().join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// Counts every step per call, including condition-skipped ones.
fn count(name: &str, stack: &mut Vec<String>) -> usize {
    assert!(
        !stack.iter().any(|s| s == name),
        "recipe cycle: {stack:?} -> {name}"
    );
    stack.push(name.to_string());
    let doc = load(name);
    let mut total = 0;
    for step in doc["steps"]
        .as_sequence()
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        total += 1;
        if let Some(sub) = step["recipe"].as_str() {
            total += count(sub, stack);
        }
    }
    stack.pop();
    total
}

fn total_and_cap() -> (usize, usize) {
    let total = count("default-workflow", &mut Vec::new());
    let cap = load("default-workflow")["recursion"]["max_total_steps"]
        .as_u64()
        .expect("max_total_steps") as usize;
    (total, cap)
}

#[test]
fn default_workflow_cap_covers_recursive_step_total() {
    let (total, cap) = total_and_cap();
    assert!(
        cap >= total,
        "max_total_steps={cap} is below recursive step total {total}"
    );
}

#[test]
fn default_workflow_cap_is_bounded() {
    let (total, cap) = total_and_cap();
    assert!(
        cap <= total * 2,
        "cap {cap} is unreasonably above total {total}"
    );
}
