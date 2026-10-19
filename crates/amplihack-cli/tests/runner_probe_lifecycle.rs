//! Linux resource-ownership regression; escaped descendants belong to the fixture.
#![cfg(target_os = "linux")]
use crate::support::exec_stub::write_executable;
use crate::support::quiet_process::in_quiet_process;
use crate::support::runner_probe;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Escaped(PathBuf);
impl Drop for Escaped {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(&self.0)
            && let Ok(pid) = pid.trim().parse::<i32>()
        {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}
fn resources() -> (usize, usize) {
    (
        fs::read_dir("/proc/self/task").unwrap().count(),
        fs::read_dir("/proc/self/fd").unwrap().count(),
    )
}
#[test]
fn repeated_escaped_pipe_holders_release_probe_readers_within_cleanup_budget() {
    in_quiet_process(
        module_path!(),
        "repeated_escaped_pipe_holders_release_probe_readers_within_cleanup_budget",
        || {
            let dir = tempfile::tempdir().unwrap();
            let baseline = resources();
            let mut observed = Vec::new();
            for index in 0..2 {
                let pidfile = dir.path().join(format!("escaped-{index}.pid"));
                let _escaped = Escaped(pidfile.clone());
                let binary = dir.path().join("probe");
                // setsid escapes the probe group while retaining BOTH output descriptors.
                // The fixture remains alive when the resource assertions execute.
                write_executable(
                    &binary,
                    format!(
                        "#!/bin/sh\nsetsid sh -c 'echo $$ > \"{}\"; exec sleep 60' &\nwait\n",
                        pidfile.display()
                    ),
                    0o700,
                );
                let started = Instant::now();
                let error = runner_probe::capture(&binary).unwrap_err();
                assert!(
                    started.elapsed() < Duration::from_secs(12),
                    "10s execution + 1s cleanup budget with scheduling margin"
                );
                assert!(format!("{error:#}").contains("deadline"));
                let pid: i32 = fs::read_to_string(&pidfile)
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert_eq!(
                    unsafe { libc::kill(pid, 0) },
                    0,
                    "escaped fixture must still hold pipes"
                );
                observed.push(resources());
            }
            for (index, actual) in observed.into_iter().enumerate() {
                assert!(
                    actual.0 <= baseline.0,
                    "probe {index} leaked a task: before={}, after={}",
                    baseline.0,
                    actual.0
                );
                assert_eq!(
                    actual.1, baseline.1,
                    "probe {index} leaked a file descriptor"
                );
            }
        },
    );
}

#[test]
fn continuous_stdout_and_stderr_writers_both_advance_without_starving_deadline() {
    in_quiet_process(
        module_path!(),
        "continuous_stdout_and_stderr_writers_both_advance_without_starving_deadline",
        || {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("probe");
            let stdout_marker = dir.path().join("stdout-progress");
            let stderr_marker = dir.path().join("stderr-progress");
            // Each marker is written only after more than pipe capacity has drained.
            // Both independent writers continue forever after demonstrating progress.
            write_executable(
                &binary,
                format!(
                    "#!/bin/sh\n(/usr/bin/head -c 262144 /dev/zero; : > '{}'; exec /bin/cat /dev/zero) &\n(/usr/bin/head -c 262144 /dev/zero; : > '{}'; exec /bin/cat /dev/zero) >&2 &\nwait\n",
                    stdout_marker.display(),
                    stderr_marker.display()
                ),
                0o700,
            );
            let baseline = resources();
            let started = Instant::now();
            let error = runner_probe::capture(&binary).unwrap_err();
            assert!(format!("{error:#}").contains("deadline"));
            assert!(
                stdout_marker.exists() && stderr_marker.exists(),
                "each stream must make progress beyond pipe capacity"
            );
            assert!(started.elapsed() < Duration::from_secs(12));
            let after = resources();
            assert!(
                after.0 <= baseline.0,
                "probe leaked a task: before={}, after={}",
                baseline.0,
                after.0
            );
            assert_eq!(after.1, baseline.1, "probe leaked a file descriptor");
        },
    );
}
