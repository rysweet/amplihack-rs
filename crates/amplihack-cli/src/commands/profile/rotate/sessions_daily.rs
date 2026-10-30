//! The daily session archive (issue #1746): once a day the watch loop runs
//! `amplihack sessions archive --apply` in-process, with the days and
//! destination from `AMPLIHACK_SESSIONS_ARCHIVE_DAYS` / `_DEST`. Turned off
//! with `AMPLIHACK_SESSIONS_ARCHIVE_DAILY=0`. Separate from the cycle: it
//! runs after it, outside the paused check, and never returns an error.
//!
//! The run time is recorded in `<state>/sessions-archive.last` before the
//! run starts, so a crash, a held lock or an error waits a day rather than
//! retrying on every pass.

use super::autostart::{EnvValue, parse_env};
use super::ledger::RotateState;
use crate::commands::profile::auto_switch::kit::{read_bounded_nofollow, write_private_atomic};
use crate::commands::sessions::archive;
use crate::commands::sessions::config::{self, Opts};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const SWITCH_ENV: &str = "AMPLIHACK_SESSIONS_ARCHIVE_DAILY";
const RECORD: &str = "sessions-archive.last";
const MAX_RECORD: u64 = 64;
const DAY: i64 = 86_400;
const PREFIX: &str = "sessions archive: ";

/// The ignored-value line is written at most once per process.
static WARNED_INVALID: AtomicBool = AtomicBool::new(false);

/// Called once per watch pass, after the cycle.
pub(super) fn tick(rs: &RotateState, home: &Path, now: i64) {
    let env = |k: &str| std::env::var_os(k);
    let mut log = |line: &str| super::daemon::log_line(rs, line);
    let mut run = |opts: &Opts| {
        let mut warn = |line: &str| super::daemon::log_line(rs, &format!("{PREFIX}{line}"));
        archive::run_default(opts, &rs.root, &mut warn)
    };
    tick_with(rs, home, now, &env, &mut log, &mut run);
}

/// [`tick`] with its environment, log and run injected.
pub(super) fn tick_with(
    rs: &RotateState,
    home: &Path,
    now: i64,
    env: &dyn Fn(&str) -> Option<OsString>,
    log: &mut dyn FnMut(&str),
    run: &mut dyn FnMut(&Opts) -> Result<String, String>,
) {
    match parse_env(env(SWITCH_ENV).as_deref()) {
        EnvValue::Off => return,
        EnvValue::Invalid => {
            if !WARNED_INVALID.swap(true, Ordering::Relaxed) {
                log(&format!(
                    "{PREFIX}ignored {SWITCH_ENV} value (use on, off, 1 or 0); running"
                ));
            }
        }
        EnvValue::On | EnvValue::Unset => {}
    }
    if !due(last_run(rs), now) {
        return;
    }
    if let Err(e) = write_private_atomic(&rs.dir, RECORD, format!("{now}\n").as_bytes()) {
        let kind = crate::commands::sessions::kind_word(e.kind());
        log(&format!(
            "{PREFIX}cannot record the run time ({kind}); not run"
        ));
        return;
    }
    let opts = match config::resolve(None, None, true, env, Some(home.to_path_buf())) {
        Ok(o) => o,
        Err(e) => return log(&format!("{PREFIX}{e}")),
    };
    match run(&opts) {
        Ok(line) | Err(line) => log(&format!("{PREFIX}{line}")),
    }
}

/// The recorded epoch seconds; `None` when missing, unreadable, negative or
/// not a plain decimal number.
fn last_run(rs: &RotateState) -> Option<i64> {
    let (raw, _) = read_bounded_nofollow(&rs.dir.join(RECORD), MAX_RECORD).ok()??;
    let text = std::str::from_utf8(&raw).ok()?.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Due without a record, a day after it, or when the clock went back.
fn due(last: Option<i64>, now: i64) -> bool {
    match last {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now.checked_sub(last).is_none_or(|d| d >= DAY),
    }
}
