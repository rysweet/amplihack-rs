//! Linux resource-ownership regression; escaped descendants belong to the fixture.
#![cfg(target_os = "linux")]
#[path = "../src/runner_probe.rs"]
mod runner_probe;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
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
    let dir = tempfile::tempdir().unwrap();
    let baseline = resources();
    let mut observed = Vec::new();
    for index in 0..2 {
        let pidfile = dir.path().join(format!("escaped-{index}.pid"));
        let _escaped = Escaped(pidfile.clone());
        let binary = dir.path().join("probe");
        // setsid escapes the probe group while retaining BOTH output descriptors.
        // The fixture remains alive when the resource assertions execute.
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nsetsid sh -c 'echo $$ > \"{}\"; exec sleep 60' &\nwait\n",
                pidfile.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
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
    assert_eq!(
        observed,
        vec![baseline; 2],
        "every probe must release its readers and descriptors before returning"
    );
}
