//! The daily session archive (issue #1746): once a day the watch loop runs
//! `amplihack sessions archive --apply` in-process, with the days and
//! destination from `AMPLIHACK_SESSIONS_ARCHIVE_DAYS` / `_DEST`. Separate
//! from the cycle: it runs after it, outside the paused check, and never
//! returns an error.
//!
//! The switch is the user config key `sessions.daily_archive`, read from
//! `<home>/.amplihack/config.toml` when a run is due. It is not a variable:
//! `daemon::spawn` starts the background rotator without the launching
//! shell's `AMPLIHACK_ROTATE_*` variables, so a variable would never reach
//! the rotator that `rotate start` and autostart launch. The file reaches
//! every rotator, including one already running.
//!
//! The run time is recorded in `<state>/sessions-archive.last` before the
//! run starts, so a crash, a held lock or an error waits a day rather than
//! retrying on every pass.
//!
//! One pass archives for at most [`RUN_LIMIT`], so a large backlog never
//! holds up rotation for longer than that. A run cut short by the limit, by
//! `rotate stop` or by a signal, that archived at least one session, removes
//! the record, so the next pass carries on where it stopped. One that
//! archived nothing waits a day like any other run, so sessions that keep
//! failing are not retried on every pass.

use super::ledger::RotateState;
use crate::commands::profile::auto_switch::kit::{read_bounded_nofollow, write_private_atomic};
use crate::commands::sessions::archive::{self, Report};
use crate::commands::sessions::config::{self, Opts};
use crate::commands::sessions::live::SystemProbe;
use crate::user_config::{SESSIONS_DAILY_ARCHIVE, sessions_daily_archive};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const RECORD: &str = "sessions-archive.last";
const MAX_RECORD: u64 = 64;
const DAY: i64 = 86_400;
const PREFIX: &str = "sessions archive: ";

/// The longest one pass archives before it hands back to the watch loop.
/// The limit is checked between sessions, so a pass can run past it by the
/// time one session takes.
pub(super) const RUN_LIMIT: Duration = Duration::from_secs(60);

/// What a run gives [`tick_with`]: its log line, and whether it stopped
/// early after archiving something, so the next pass should carry on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ran {
    pub(super) line: String,
    pub(super) more: bool,
}

/// Called once per watch pass, after the cycle. The run checks `stop` and
/// [`RUN_LIMIT`] between sessions, so `rotate stop` and SIGTERM wait for
/// one session at most, and rotation is held up for about a minute at most.
pub(super) fn tick(rs: &RotateState, home: &Path, now: i64, stop: &AtomicBool) {
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let env = |k: &str| std::env::var_os(k);
    let mut log = |line: &str| super::daemon::log_line(rs, line);
    let mut run = |opts: &Opts| {
        let mut warn = |line: &str| super::daemon::log_line(rs, &format!("{PREFIX}{line}"));
        let started = Instant::now();
        let halt = || stop.load(Ordering::Relaxed) || started.elapsed() >= RUN_LIMIT;
        let report = archive::run(opts, &rs.root, &SystemProbe, &mut warn, &halt)
            .map_err(|e| e.to_string())?;
        Ok(ran(&report, opts, stop.load(Ordering::Relaxed)))
    };
    tick_with(rs, home, now, &env, &mut log, &mut run);
}

/// The outcome of one run: the summary line, with why it stopped early if
/// it did (`requested`: `rotate stop` or a signal, else [`RUN_LIMIT`]).
pub(super) fn ran(report: &Report, opts: &Opts, requested: bool) -> Ran {
    let line = archive::summary(report, opts.days, opts.apply);
    if !report.stopped {
        return Ran { line, more: false };
    }
    let why = if requested {
        "stopped by rotate stop or a signal".to_string()
    } else {
        format!("stopped at the {}-second limit", RUN_LIMIT.as_secs())
    };
    let more = report.archived > 0;
    let next = if more {
        "the rest continues at the next pass"
    } else {
        "nothing archived, so the next try is a day later"
    };
    Ran {
        line: format!("{line}; {why}; {next}"),
        more,
    }
}

/// [`tick`] with its environment, log and run injected. The switch is
/// always read from the user config file under `home`.
pub(super) fn tick_with(
    rs: &RotateState,
    home: &Path,
    now: i64,
    env: &dyn Fn(&str) -> Option<OsString>,
    log: &mut dyn FnMut(&str),
    run: &mut dyn FnMut(&Opts) -> Result<Ran, String>,
) {
    if !due(last_run(rs), now) {
        return;
    }
    let switch = sessions_daily_archive(home);
    if switch == Ok(false) {
        return;
    }
    if let Err(e) = write_private_atomic(&rs.dir, RECORD, format!("{now}\n").as_bytes()) {
        let kind = crate::commands::sessions::kind_word(e.kind());
        log(&format!(
            "{PREFIX}cannot record the run time ({kind}); not run"
        ));
        return;
    }
    if let Err(why) = switch {
        return log(&format!(
            "{PREFIX}cannot read {SESSIONS_DAILY_ARCHIVE} ({why}); not run"
        ));
    }
    let opts = match config::resolve(None, None, true, env, Some(home.to_path_buf())) {
        Ok(o) => o,
        Err(e) => return log(&format!("{PREFIX}{e}")),
    };
    match run(&opts) {
        Ok(Ran { line, more }) => {
            log(&format!("{PREFIX}{line}"));
            if more {
                forget_run(rs, log);
            }
        }
        Err(line) => log(&format!("{PREFIX}{line}")),
    }
}

/// Removes the record, so the next pass is due. If that fails the rest of
/// the run waits a day, and the log says why.
fn forget_run(rs: &RotateState, log: &mut dyn FnMut(&str)) {
    match std::fs::remove_file(rs.dir.join(RECORD)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            let kind = crate::commands::sessions::kind_word(e.kind());
            log(&format!(
                "{PREFIX}cannot remove the run record ({kind}); the rest waits a day"
            ));
        }
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
