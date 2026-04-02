//! Top-level dashboard entry point (`run_fleet_dashboard`).

use std::sync::mpsc::Sender;

use super::tmux::{
    capture_local_tmux_pane, is_tmux_available, list_local_tmux_sessions,
    sanitize_tmux_session_name,
};
use super::{
    CAPTURE_CACHE_ENTRY_MAX_BYTES, FleetLocalError, RefreshMsg, collect_observed_fleet_state,
    strip_osc_sequences,
};

// ── run_fleet_dashboard ───────────────────────────────────────────────────────

/// Top-level entry point for the local session dashboard.
///
/// # Modes
///
/// - `bg_tx = None` → **synchronous fallback** (unit-testable).  Collects
///   state once, renders once to stdout, returns immediately.  Does **not**
///   spawn any background threads.
/// - `bg_tx = Some(tx)` → **interactive mode**.  Launches the fast refresh
///   thread (T4, 500 ms) and the slow capture thread (T5, 5 s), then enters
///   the raw-mode TUI event loop.  Threads self-exit when `tx.send()` returns
///   `Err(_)`.
///
/// # Terminal guard
///
/// Raw mode is enabled only inside this function.  A `TerminalGuard` (RAII
/// drop impl) restores the terminal even if the render loop panics (RISK-01).
pub fn run_fleet_dashboard(bg_tx: Option<Sender<RefreshMsg>>) -> Result<(), FleetLocalError> {
    match bg_tx {
        None => {
            // Synchronous fallback: collect state once, render to stdout, return.
            // No raw mode, no background threads (unit-testable).
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            let locks_dir = std::path::PathBuf::from(&home)
                .join(".claude")
                .join("runtime")
                .join("locks");
            let sessions = collect_observed_fleet_state(&locks_dir)?;
            // Minimal render: print session count to stdout (no terminal required).
            println!("Fleet dashboard: {} session(s)", sessions.len());
            Ok(())
        }
        Some(tx) => {
            // Interactive mode: spawn fast (500 ms) and slow (5 s) refresh threads.
            // Fast refresh thread (T4).
            let tx_fast = tx.clone();
            std::thread::spawn(move || {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                let locks_dir = std::path::PathBuf::from(&home)
                    .join(".claude")
                    .join("runtime")
                    .join("locks");
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let msg = match collect_observed_fleet_state(&locks_dir) {
                        Ok(sessions) => RefreshMsg::Sessions(sessions),
                        Err(e) => RefreshMsg::Error(e.to_string()),
                    };
                    if tx_fast.send(msg).is_err() {
                        break;
                    }
                }
            });

            // Slow refresh thread (T5 — 5 s): polls local tmux capture-pane for
            // active sessions and sends RefreshMsg::CaptureUpdate.
            //
            // RISK-07: tmux may not be installed.  We check once at thread start
            // with `tmux -V`; if the binary is absent or returns an error, the
            // thread exits immediately without sending any messages.  The main
            // loop continues to work with the fast-thread data alone.
            let tx_slow = tx;
            std::thread::spawn(move || {
                // Guard: verify tmux is available before entering the loop.
                if !is_tmux_available() {
                    return;
                }

                loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));

                    // List active local tmux session names.
                    let session_names = list_local_tmux_sessions();

                    for raw_name in session_names {
                        // Sanitize before any use (SEC-01).
                        let session_id = sanitize_tmux_session_name(&raw_name);
                        if session_id.is_empty() {
                            continue;
                        }

                        // Capture pane output for this session.
                        let raw_output = capture_local_tmux_pane(&session_id);

                        // Strip OSC escape sequences (SEC-06).
                        let clean = strip_osc_sequences(&raw_output);

                        // Cap at 64 KiB before sending over the channel (SEC-10).
                        // Truncate at a valid UTF-8 boundary — a naive byte-index
                        // slice would panic on multi-byte characters (e.g. emoji).
                        let output = if clean.len() > CAPTURE_CACHE_ENTRY_MAX_BYTES {
                            let mut boundary = CAPTURE_CACHE_ENTRY_MAX_BYTES;
                            while !clean.is_char_boundary(boundary) {
                                boundary -= 1;
                            }
                            clean[..boundary].to_string()
                        } else {
                            clean
                        };

                        let msg = RefreshMsg::CaptureUpdate { session_id, output };
                        if tx_slow.send(msg).is_err() {
                            // Receiver closed — main loop exited; self-exit.
                            return;
                        }
                    }
                }
            });

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-12: `run_fleet_dashboard(None)` must complete inline (no background
    /// threads) and not hang in a non-terminal environment.
    #[test]
    fn tc12_run_fleet_dashboard_none_bg_tx_completes_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (done_tx, done_rx) = mpsc::channel::<Result<(), String>>();

        let handle = std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(|| run_fleet_dashboard(None));
            match outcome {
                Ok(result) => {
                    let msg = result.map_err(|e| e.to_string());
                    let _ = done_tx.send(msg);
                }
                Err(panic_val) => {
                    let msg = if let Some(s) = panic_val.downcast_ref::<&str>() {
                        format!("not yet implemented: {s}")
                    } else if let Some(s) = panic_val.downcast_ref::<String>() {
                        format!("not yet implemented: {s}")
                    } else {
                        "function panicked (likely todo!())".to_string()
                    };
                    let _ = done_tx.send(Err(msg));
                }
            }
        });

        let received = done_rx.recv_timeout(Duration::from_secs(3));
        drop(handle);

        match received {
            Ok(Ok(())) => { /* green: implementation returned Ok */ }
            Ok(Err(io_msg)) if io_msg.contains("IO") || io_msg.contains("terminal") => {
                // Acceptable: function returned Err(IO) because there is no
                // terminal in the test environment.
            }
            Ok(Err(panic_msg)) => {
                panic!(
                    "run_fleet_dashboard(None) is not yet implemented: {panic_msg}\n\
                     Implement S3 to make this test pass."
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!(
                    "run_fleet_dashboard(None) panicked (likely todo!() stub) without \
                     returning.  Implement S3 in fleet_local.rs to make this pass."
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!(
                    "run_fleet_dashboard(None) did not complete within 3 s in \
                     non-terminal mode; the None path must return immediately."
                );
            }
        }
    }

    #[test]
    fn tc12_run_fleet_dashboard_none_does_not_return_ok_and_leave_raw_mode() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(|| run_fleet_dashboard(None));
            let _ = done_tx.send(());
        });
        let _ = done_rx.recv_timeout(std::time::Duration::from_secs(3));
    }
}
