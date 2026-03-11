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
    use std::sync::atomic::Ordering;

    #[test]
    fn register_handlers_creates_false_flag() {
        let shutdown = register_handlers().unwrap();
        assert!(!shutdown.load(Ordering::Relaxed));
    }

    #[cfg(unix)]
    #[test]
    fn signal_sets_flag() {
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
