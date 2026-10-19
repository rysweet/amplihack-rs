//! Private fault injection contract: cleanup errors remain caller-visible.
#![cfg(unix)]
use crate::support::exec_stub::write_executable;
use crate::support::quiet_process::in_quiet_process;
use crate::support::runner_probe;
use std::{
    fs,
    time::{Duration, Instant},
};

#[test]
fn timeout_and_cleanup_failure_are_both_reported_with_bounded_reader_release() {
    in_quiet_process(
        module_path!(),
        "timeout_and_cleanup_failure_are_both_reported_with_bounded_reader_release",
        || {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("probe");
            write_executable(&binary, "#!/bin/sh\nexec sleep 60\n", 0o700);
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
        },
    );
}

#[test]
fn actual_setup_read_termination_and_reap_failures_are_visible() {
    in_quiet_process(
        module_path!(),
        "actual_setup_read_termination_and_reap_failures_are_visible",
        || {
            use runner_probe::testing::{Fault, with_fault};
            for (fault, expected) in [
                (Fault::Setup, "nonblocking setup failed"),
                (Fault::Read, "pipe read failed"),
                (Fault::Termination, "probe termination failed"),
                (Fault::Reap, "probe reap failed"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let binary = dir.path().join("probe");
                write_executable(&binary, "#!/bin/sh\nexec sleep 60\n", 0o700);
                let started = Instant::now();
                #[cfg(target_os = "linux")]
                let descriptors = fs::read_dir("/proc/self/fd").unwrap().count();
                // The invalid syscall arguments prevent the actual operation. The
                // with_fault guard cleans the fixture AFTER observing capture's failure.
                with_fault(fault, || {
                    let error = format!("{:#}", runner_probe::capture(&binary).unwrap_err());
                    assert!(error.contains(expected), "{fault:?}: {error}");
                    #[cfg(target_os = "linux")]
                    assert_eq!(
                        fs::read_dir("/proc/self/fd").unwrap().count(),
                        descriptors,
                        "capture must release pipe descriptors even when cleanup operations fail"
                    );
                    if matches!(fault, Fault::Termination | Fault::Reap) {
                        assert!(error.contains("deadline"), "primary failure lost: {error}");
                    }
                    if fault == Fault::Termination {
                        assert_eq!(
                            unsafe { libc::kill(runner_probe::testing::child_pid() as i32, 0) },
                            0,
                            "failed termination must leave the actual fixture alive until its separate guard"
                        );
                        assert!(error.contains("cleanup exceeded deadline"), "{error}");
                    }
                    assert!(started.elapsed() < Duration::from_secs(12));
                });
            }
        },
    );
}
