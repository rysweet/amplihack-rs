//! Golden file integration tests.
//!
//! Runs each hook against its golden file fixtures and verifies
//! that output matches expected values.

mod golden_test_runner;

use amplihack_hooks::{
    post_tool_use::PostToolUseHook, pre_compact::PreCompactHook, pre_tool_use::PreToolUseHook,
    protocol::Hook, stop::StopHook,
};
use std::path::Path;

fn run_golden_for_hook<H: Hook>(hook: &H, dir: &str) {
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(dir);

    if !golden_dir.exists() {
        println!("No golden files for {} at {}", dir, golden_dir.display());
        return;
    }

    let results = golden_test_runner::run_golden_tests(hook, &golden_dir);

    let total = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.iter().filter(|r| !r.passed).count();

    println!("{}: {}/{} passed", dir, passed, total);

    for result in &results {
        if !result.passed {
            println!("  FAIL {}: {:?}", result.name, result.mismatches);
        }
    }

    assert_eq!(failed, 0, "{} golden file tests failed for {}", failed, dir);
}

#[test]
fn golden_pre_tool_use() {
    run_golden_for_hook(&PreToolUseHook, "pre_tool_use");
}

#[test]
fn golden_post_tool_use() {
    run_golden_for_hook(&PostToolUseHook, "post_tool_use");
}

#[test]
fn golden_stop() {
    run_golden_for_hook(&StopHook, "stop");
}

#[test]
fn golden_pre_compact() {
    run_golden_for_hook(&PreCompactHook, "pre_compact");
}
