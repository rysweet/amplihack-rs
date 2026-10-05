//! Private fault injection contract: cleanup errors remain caller-visible.
#![cfg(unix)]
#[path = "../src/runner_probe.rs"]
mod runner_probe;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

#[test]
fn timeout_and_cleanup_failure_are_both_reported_with_bounded_reader_release() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("probe");
    fs::write(&binary, "#!/bin/sh\nexec sleep 60\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let started = Instant::now();
    // Proposed cfg(test)-only seam: perform real owned-process cleanup first,
    // then inject its reported failure. Never leave the real fixture running.
    let result = runner_probe::testing::with_cleanup_result_hook(
        || anyhow::bail!("injected probe cleanup failure"),
        || runner_probe::capture(&binary),
    );
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("deadline"), "{error}");
    assert!(error.contains("injected probe cleanup failure"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(12));
}
