//! `amplihack wait --seconds N`: wait for a fixed time without starting
//! anything (issue #1794).
//!
//! Agents that are told not to call `sleep` used `tail -f /dev/null` or
//! `timeout N tail -f /dev/null` instead, and the `tail` outlived the tool
//! call as an orphan. This command sleeps in the calling thread: it starts no
//! child process, async runtime or timer thread, prints nothing and installs
//! no signal handlers, so SIGINT and SIGTERM end it with their default
//! action. Clap checks `N` against `0..=86400`, so the duration cannot
//! overflow.

use std::time::Duration;

/// Sleeps for `seconds` and returns.
pub fn run_wait(seconds: u64) -> anyhow::Result<()> {
    std::thread::sleep(Duration::from_secs(seconds));
    Ok(())
}
