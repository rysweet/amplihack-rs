//! Signal handling for the CLI launcher.
//!
//! Registers handlers for SIGINT, SIGTERM, and SIGHUP that set an
//! `AtomicBool` flag, allowing the main loop to detect shutdown requests.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Register signal handlers that set a shared `AtomicBool` on shutdown signals.
///
/// Returns the `AtomicBool` that will be set to `true` when any of
/// SIGINT, SIGTERM, or SIGHUP is received.
pub fn register_handlers() -> Result<Arc<AtomicBool>> {
    let shutdown = Arc::new(AtomicBool::new(false));

    #[cfg(unix)]
    {
        for sig in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
        ] {
            signal_hook::flag::register(sig, Arc::clone(&shutdown))
                .with_context(|| format!("failed to register signal handler for signal {sig}"))?;
        }
    }

    Ok(shutdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    static SIGNAL_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn register_handlers_creates_false_flag() {
        let _guard = SIGNAL_TEST_LOCK.lock().expect("signal test lock poisoned");
        let shutdown = register_handlers().unwrap();
        assert!(!shutdown.load(Ordering::Relaxed));
    }

    /// Set in the child that [`signal_sets_flag`] spawns.
    #[cfg(unix)]
    const SIGNAL_CHILD_ENV: &str = "AMPLIHACK_SIGNALS_TEST_CHILD";

    /// The SIGTERM below reaches the whole process. In the shared lib test
    /// binary that includes the recipe-run stop handler, whose flag never
    /// clears, so every recipe run any other test started afterwards failed
    /// with "stopped by signal 15", depending on test order. The signal is
    /// therefore sent in a child running only this test.
    #[cfg(unix)]
    #[test]
    fn signal_sets_flag() {
        if std::env::var_os(SIGNAL_CHILD_ENV).is_some() {
            signal_sets_flag_in_this_process();
            return;
        }
        let module = module_path!();
        let name = format!(
            "{}::signal_sets_flag",
            module.split_once("::").map_or(module, |(_, rest)| rest)
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture", "--test-threads=1"])
            .env(SIGNAL_CHILD_ENV, "1")
            .output()
            .expect("failed to spawn the signal test child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "signal test child failed or ran nothing:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    fn signal_sets_flag_in_this_process() {
        let _guard = SIGNAL_TEST_LOCK.lock().expect("signal test lock poisoned");
        let shutdown = register_handlers().unwrap();
        assert!(!shutdown.load(Ordering::Relaxed));

        // Send ourselves SIGTERM
        // SAFETY: Sending SIGTERM to our own process is safe and a standard
        // pattern for testing signal handlers.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }

        // Give the signal handler a moment
        std::thread::sleep(std::time::Duration::from_millis(100));

        assert!(shutdown.load(Ordering::Relaxed));
    }
}
