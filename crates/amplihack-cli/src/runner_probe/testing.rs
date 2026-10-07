use super::*;
type CleanupHook = Box<dyn FnMut() -> Result<()>>;
thread_local! { static HOOK: std::cell::RefCell<Option<CleanupHook>> = std::cell::RefCell::new(None); }
#[allow(dead_code)] // The source is also included by lifecycle-only integration tests.
pub fn with_cleanup_result_hook<R>(
    hook: impl FnMut() -> Result<()> + 'static,
    run: impl FnOnce() -> R,
) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOOK.with(|h| *h.borrow_mut() = None);
        }
    }
    HOOK.with(|h| {
        assert!(h.borrow().is_none());
        *h.borrow_mut() = Some(Box::new(hook));
    });
    let _reset = Reset;
    run()
}
pub(super) fn check() -> Result<()> {
    HOOK.with(|h| match h.borrow_mut().as_mut() {
        Some(h) => h(),
        None => Ok(()),
    })
}

// These faults make the operation itself fail. Fixture cleanup is a separate
// guard, after capture returns; it must never be confused with probe cleanup.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Setup,
    Read,
    Termination,
    Reap,
}
#[cfg(unix)]
thread_local! {
    static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
    static CHILD: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}
#[cfg(unix)]
pub(super) fn fault(expected: Fault) -> bool {
    FAULT.with(|f| f.get() == Some(expected))
}
#[cfg(unix)]
pub(super) fn record_child(pid: u32) {
    if FAULT.with(|f| f.get().is_some()) {
        CHILD.with(|p| p.set(Some(pid)));
    }
}
#[cfg(unix)]
pub(super) fn termination_signal(signal: i32) -> i32 {
    if fault(Fault::Termination) {
        i32::MAX
    } else {
        signal
    }
}
#[cfg(unix)]
pub(super) fn failed_signal(pid: u32) -> io::Result<()> {
    let result = unsafe { libc::kill(pid as i32, i32::MAX) };
    assert_eq!(
        result, -1,
        "invalid signal must fail before terminating fixture"
    );
    Err(io::Error::last_os_error())
}
#[cfg(unix)]
pub(super) fn reap(pid: u32) -> io::Result<()> {
    if fault(Fault::Reap) {
        let result = unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), i32::MAX) };
        assert_eq!(result, -1, "invalid wait options must fail without reaping");
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(unix)]
pub(super) fn read() -> Result<()> {
    if fault(Fault::Read) {
        let result = unsafe { libc::read(-1, std::ptr::null_mut(), 0) };
        assert_eq!(result, -1, "invalid descriptor must fail actual read");
        return Err(io::Error::last_os_error()).context("capability pipe read failed");
    }
    Ok(())
}
#[cfg(unix)]
#[allow(dead_code)] // Used by the dedicated integration test source inclusion.
pub fn with_fault<R>(injected: Fault, run: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAULT.with(|f| f.set(None));
            if let Some(pid) = CHILD.with(|p| p.take()) {
                // Do not signal a PID already reaped by successful probe cleanup.
                let status =
                    unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
                if status == pid as i32
                    || (status == -1
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD))
                {
                    return;
                }
                // The fixture owns cleanup when the operation was deliberately prevented.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                    libc::kill(pid as i32, libc::SIGKILL);
                }
                let deadline = Instant::now() + Duration::from_secs(1);
                loop {
                    let result =
                        unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
                    if result == pid as i32
                        || (result == -1
                            && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD))
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline, "fixture cleanup failed to reap");
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    FAULT.with(|f| {
        assert!(f.get().is_none());
        f.set(Some(injected));
    });
    let _reset = Reset;
    run()
}

#[cfg(unix)]
#[allow(dead_code)]
pub fn child_pid() -> u32 {
    CHILD.with(|p| p.get().expect("probe child recorded"))
}

#[cfg(windows)]
thread_local! {
    static WINDOWS_HANDLES: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}
#[cfg(windows)]
pub(super) fn record_windows_handles(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    WINDOWS_HANDLES.with(|handles| {
        *handles.borrow_mut() = vec![
            child.as_raw_handle() as usize,
            child.stdout.as_ref().unwrap().as_raw_handle() as usize,
            child.stderr.as_ref().unwrap().as_raw_handle() as usize,
        ]
    });
}
#[cfg(windows)]
pub fn windows_handles() -> Vec<usize> {
    WINDOWS_HANDLES.with(|handles| handles.borrow().clone())
}
