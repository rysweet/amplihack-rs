//! Windows retained-pipe ownership, including successful direct-child exit.
#![cfg(windows)]
#[path = "../src/runner_probe.rs"]
mod runner_probe;
use std::{
    ffi::c_void,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}
fn assert_probe_handles_closed() {
    let handles = runner_probe::testing::windows_handles();
    assert_eq!(
        handles.len(),
        3,
        "actual child and both reader handles recorded"
    );
    for handle in handles {
        let mut flags = 0;
        assert_eq!(
            unsafe { GetHandleInformation(handle as *mut c_void, &mut flags) },
            0,
            "owned child and read handles must close before returning"
        );
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(6)); // ERROR_INVALID_HANDLE
    }
}
// Fixture cleanup also runs if a probe ownership assertion fails.
struct Retained(PathBuf);
impl Drop for Retained {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(&self.0)
            && let Ok(pid) = pid.trim().parse::<u32>()
        {
            let process = unsafe { OpenProcess(0x0010_0001, 0, pid) };
            if !process.is_null() {
                drop(Holder(process));
            }
        }
    }
}
struct Holder(*mut c_void);
impl Drop for Holder {
    fn drop(&mut self) {
        unsafe {
            TerminateProcess(self.0, 1);
            WaitForSingleObject(self.0, 5000);
            CloseHandle(self.0);
        }
    }
}
fn holder(pidfile: &Path) -> Holder {
    let pid: u32 = fs::read_to_string(pidfile).unwrap().trim().parse().unwrap();
    let handle = unsafe { OpenProcess(0x0010_0001, 0, pid) }; // SYNCHRONIZE | PROCESS_TERMINATE
    assert!(!handle.is_null());
    assert_eq!(
        unsafe { WaitForSingleObject(handle, 0) },
        258,
        "fixture must still retain both pipes"
    );
    Holder(handle)
}
#[test]
#[ignore = "subprocess-only fixture, invoked with explicit environment by ownership test"]
fn windows_retained_pipe_fixture() {
    let pidfile = PathBuf::from(std::env::var_os("PROBE_FIXTURE_PIDFILE").unwrap());
    if std::env::var_os("PROBE_FIXTURE_HOLDER").is_some() {
        fs::write(&pidfile, std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "windows_retained_pipe_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("PROBE_FIXTURE_HOLDER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pidfile.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    if std::env::var_os("PROBE_FIXTURE_PARENT_EXIT").is_none() {
        child.wait().unwrap();
    }
}
#[test]
fn repeated_retained_pipes_release_owned_handles_before_fixture_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    for exits in [false, true] {
        for index in 0..2 {
            let pidfile = dir.path().join(format!("holder-{exits}-{index}.pid"));
            let _fixture = Retained(pidfile.clone());
            let binary = dir.path().join("probe.cmd");
            fs::write(&binary, format!(
                "@echo off\r\nset PROBE_FIXTURE_PIDFILE={}\r\nset PROBE_FIXTURE_PARENT_EXIT={}\r\n\"{}\" --exact windows_retained_pipe_fixture --ignored --nocapture\r\n",
                pidfile.display(), if exits { "1" } else { "" }, std::env::current_exe().unwrap().display()
            )).unwrap();
            let start = Instant::now();
            let error = runner_probe::capture(&binary).unwrap_err();
            assert!(
                start.elapsed() < Duration::from_secs(12),
                "bounded execution and cleanup"
            );
            assert!(format!("{error:#}").contains("deadline"), "{error:#}");
            // Check exact handles before opening a fixture handle, so handle
            // reuse cannot confuse the ownership assertion.
            assert_probe_handles_closed();
            let _held = holder(&pidfile);
        }
    }
}
#[test]
fn windows_capture_preserves_empty_capabilities_and_output_bound() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("probe.cmd");
    fs::write(
        &binary,
        "@echo off\r\necho {\"schema_version\":1,\"version\":\"0.4.0\",\"capabilities\":[]}\r\n",
    )
    .unwrap();
    let output = runner_probe::capture(&binary).unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("\"capabilities\":[]")
    );
    assert!(output.stderr.is_empty());
    fs::write(
        &binary,
        "@echo off\r\nfor /L %%i in (1,1,20000) do echo abcdefghijklmnopqrstuvwxyz\r\n",
    )
    .unwrap();
    assert!(format!("{:#}", runner_probe::capture(&binary).unwrap_err()).contains("exceeds bound"));
}
