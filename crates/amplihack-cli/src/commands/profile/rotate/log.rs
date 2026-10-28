//! The rotate log: `YYYY-MM-DDTHH:MM:SSZ <message>` lines in a private file
//! that is rotated once it passes 1 MiB, and the message grammar. Messages
//! hold only profile names, percentages, phases and fixed text.

use super::decide::{Reading, Reason};
use crate::commands::profile::auto_switch::kit::euid;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const MAX_LOG: u64 = 1024 * 1024;

/// UTC time in the log's format.
pub fn timestamp(now: i64) -> String {
    chrono::DateTime::from_timestamp(now, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

/// One log line, without the newline. Control characters in `msg` are
/// replaced.
pub fn line(now: i64, msg: &str) -> String {
    format!("{} {}", timestamp(now), sanitize_display(msg, false))
}

pub struct RotateLog {
    file: fs::File,
    path: PathBuf,
    rotated: PathBuf,
}

fn refused(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("rotate log {what}"))
}

fn open_checked(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => refused("is a symlink"),
            _ => e,
        })?;
    let m = file.metadata()?;
    if !m.is_file() {
        return Err(refused("is not a regular file"));
    }
    if m.uid() != euid() {
        return Err(refused("is owned by another user"));
    }
    if m.mode() & 0o077 != 0 {
        return Err(refused("is readable or writable by other users"));
    }
    Ok(file)
}

impl RotateLog {
    /// Opens `log` for appending. A symlink, a non-regular file, a file
    /// owned by someone else or one with group or other mode bits is
    /// refused, and its mode is left alone.
    pub fn open(log: &Path, rotated: &Path) -> io::Result<Self> {
        Ok(Self {
            file: open_checked(log)?,
            path: log.to_path_buf(),
            rotated: rotated.to_path_buf(),
        })
    }

    /// Another descriptor for the open log, for a child's output.
    pub fn file(&self) -> io::Result<fs::File> {
        self.file.try_clone()
    }

    /// Appends one line. A log over [`MAX_LOG`] is first renamed to the
    /// rotated name, replacing the previous one, and a new log is started.
    pub fn write(&mut self, now: i64, msg: &str) -> io::Result<()> {
        let m = self.file.metadata()?;
        if m.len() > MAX_LOG {
            let same = fs::symlink_metadata(&self.path)
                .is_ok_and(|p| p.ino() == m.ino() && p.dev() == m.dev());
            if same {
                // Checks that the old rotated file may be replaced.
                if let Some(dir) = self.rotated.parent()
                    && let Some(name) = self.rotated.file_name().and_then(|n| n.to_str())
                {
                    replaceable(dir, name)?;
                }
                fs::rename(&self.path, &self.rotated)?;
            }
            self.file = open_checked(&self.path)?;
        }
        self.file
            .write_all(format!("{}\n", line(now, msg)).as_bytes())
    }
}

/// Fails when `dir/name` is a symlink, not a regular file, or someone
/// else's. Writes nothing.
fn replaceable(dir: &Path, name: &str) -> io::Result<()> {
    match fs::symlink_metadata(dir.join(name)) {
        Ok(m) if m.file_type().is_symlink() || !m.is_file() || m.uid() != euid() => {
            Err(refused("backup cannot be replaced"))
        }
        _ => Ok(()),
    }
}

/// Where a cycle's lines go: the one-shot command writes to the log and
/// stdout, `watch` to the log, `--dry-run` to stdout without the time.
pub struct Sink {
    pub log: Option<RotateLog>,
    pub stdout: bool,
    pub timed: bool,
    warned: bool,
}

impl Sink {
    pub fn new(log: Option<RotateLog>, stdout: bool, timed: bool) -> Self {
        Self {
            log,
            stdout,
            timed,
            warned: false,
        }
    }

    pub fn emit(&mut self, now: i64, msg: &str) {
        if let Some(log) = &mut self.log
            && let Err(e) = log.write(now, msg)
            && !self.warned
        {
            self.warned = true;
            eprintln!("warning: could not write the rotate log: {e}");
        }
        self.print(now, msg);
    }

    /// Prints to stdout only, when this sink prints at all.
    pub fn print(&self, now: i64, msg: &str) {
        if self.stdout {
            if self.timed {
                println!("{}", line(now, msg));
            } else {
                println!("{}", sanitize_display(msg, false));
            }
        }
    }
}

/// Opens the log for a sink, warning once on stderr when it is refused.
pub fn open_or_warn(log: &Path, rotated: &Path) -> Option<RotateLog> {
    RotateLog::open(log, rotated)
        .map_err(|e| eprintln!("warning: not writing the rotate log: {e}"))
        .ok()
}

/// `p` for a shell command (issue #1693): bare when every byte is in
/// `A-Za-z0-9/._+-`, else single-quoted with each `'` written as `'\''`.
/// `None` for a path that is not UTF-8 or has a control or
/// bidirectional-override character, which no command may show.
pub fn sh_quote(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    if s.chars().any(super::ledger::unprintable) {
        return None;
    }
    let bare = |b: u8| b.is_ascii_alphanumeric() || b"/._+-".contains(&b);
    if !s.is_empty() && s.bytes().all(bare) {
        return Some(s.to_string());
    }
    Some(format!("'{}'", s.replace('\'', r"'\''")))
}

/// `s` cut to at most `max` bytes on a character boundary.
pub fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The message grammar of docs/reference/profile-command.md, Rotate log.
pub mod msg {
    use super::{Reading, Reason};
    use crate::commands::profile::name::ProfileName;

    /// A percentage as a whole number, rounded down.
    fn pct(p: f64) -> u64 {
        if p.is_finite() && p > 0.0 {
            p.floor() as u64
        } else {
            0
        }
    }

    /// `s=<n>% w=<n>%` with `sep` between; an exhausted login shows as
    /// 100%, a failed probe as `failed`.
    fn usage(r: &Reading, sep: &str) -> String {
        match r {
            Reading::Measured { session, week } => {
                format!("s={}%{sep}w={}%", pct(*session), pct(*week))
            }
            Reading::Exhausted => format!("s=100%{sep}w=100%"),
            Reading::Failed => "failed".to_string(),
        }
    }

    pub fn swap(
        from: &ProfileName,
        to: &ProfileName,
        live: &Reading,
        target: &Reading,
        reason: Reason,
    ) -> String {
        format!(
            "swap {from}->{to} live {} target {} reason={}",
            usage(live, " "),
            usage(target, " "),
            reason.as_str()
        )
    }

    /// Every measured profile: the live one first, then by name.
    pub fn cycle(
        live: Option<&ProfileName>,
        reading: &Reading,
        others: &[(ProfileName, Reading)],
    ) -> String {
        let label = live.map_or("-", ProfileName::as_str);
        let mut sorted: Vec<&(ProfileName, Reading)> = others.iter().collect();
        sorted.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        let mut out = format!("cycle live={label} {label}:{}", usage(reading, ","));
        for (name, r) in sorted {
            out.push_str(&format!(" {name}:{}", usage(r, ",")));
        }
        out
    }

    #[cfg(test)]
    pub fn alert_no_capacity(live: &ProfileName, reading: &Reading) -> String {
        alert_no_capacity_for(live.as_str(), reading)
    }

    /// As [`alert_no_capacity`], with `-` for a login no profile owns.
    pub fn alert_no_capacity_for(live: &str, reading: &Reading) -> String {
        format!(
            "ALERT: no profile has capacity live={live} {}",
            usage(reading, " ")
        )
    }

    pub const NO_LOGIN: &str = "ALERT: no login in ~/.claude; no swap";
    pub const AMBIGUOUS: &str =
        "ALERT: more than one profile matches the account in ~/.claude; no swap";
    pub const AUTO_SWITCH_INCONSISTENT: &str =
        "ALERT: auto-switch recovery is inconsistent; no swap";
    pub const LOCK_BUSY: &str = "skip: state lock busy";
    pub const CHANGED: &str = "skip: state changed during measurement";
    pub const NO_LIVENESS: &str = "skip: cannot see running sessions; no candidates";
    pub const STOPPED: &str = "stop: signal received; cycle abandoned";
    pub const IDLE: &str = "idle: auto-switch is off";

    /// The live probe failed for the `k`th cycle in a row; no swap yet.
    pub fn stay_failed_probe(k: u32, limit: u32) -> String {
        format!("stay: live probe failed ({k}/{limit})")
    }

    /// Failed live probes would swap, but the last swap is `secs` too recent.
    pub fn stay_held_failed(secs: u64) -> String {
        format!("stay: live probe failed; held after last swap ({secs}s left)")
    }

    pub fn hand_change(live: &ProfileName) -> String {
        format!(
            "ALERT: the account in ~/.claude is not the one the ledger names (live={live}); no swap"
        )
    }

    /// `~/.claude.json` could not be read: no recovery and no swap, and no
    /// `/login` advice, which would replace `live`'s login (#1702).
    pub fn home_unreadable(live: &ProfileName) -> String {
        format!(
            "ALERT: the account in ~/.claude.json cannot be read (live={live}); no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace {live}'s login"
        )
    }

    pub fn inconsistent(reason: &str) -> String {
        format!("ALERT: rotate recovery is inconsistent: {reason}; no swap until it is resolved")
    }

    /// A cycle stopped by the rotation pin (issue #1691).
    pub fn held(name: &ProfileName) -> String {
        format!("skip: held by switch to {name}; run `amplihack profile switch --none` to release")
    }

    /// The pin file cannot be trusted; never names its content.
    pub const PIN_UNREADABLE: &str = "skip: pin unreadable; run `amplihack profile switch --none`";

    pub fn pin_missing(name: &ProfileName) -> String {
        format!("skip: pin names missing profile {name}; ignored")
    }

    /// `profile switch` moved a login. Not `swap ...`, so a search for the
    /// rotator's swaps does not match it.
    pub fn switched(from: &ProfileName, to: &ProfileName) -> String {
        format!("switch {from}->{to}")
    }

    pub fn switched_already_live(name: &ProfileName) -> String {
        format!("skip: {name} is already the live login; nothing moved")
    }

    pub fn recovered(forward: bool, from: &ProfileName, to: &ProfileName) -> String {
        let what = if forward { "finished" } else { "rolled back" };
        format!("recovery: {what} an interrupted swap {from}->{to}")
    }

    /// Written every cycle while `name` is in the ledger's `lost_login`.
    pub fn lost_login(name: &ProfileName) -> String {
        format!(
            "ALERT: profile {name} has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login {name}"
        )
    }

    /// Written once, when `name` is not added to the full `lost_login`
    /// list and so gets no [`lost_login`] line.
    pub fn lost_login_cap(name: &ProfileName) -> String {
        format!(
            "ALERT: profile {name} has no login anywhere, and the list of such profiles is full, so this is not repeated. Run: amplihack profile login {name}"
        )
    }

    /// Hand-change recovery, case A: `m` had no login of its own.
    pub fn recovered_record(m: &ProfileName, l: &ProfileName) -> String {
        format!("recovered: ~/.claude holds {m}'s login (ledger named {l}); recorded {m} as live")
    }

    /// Hand-change recovery, case B: `m`'s own copy was moved to `to`.
    pub fn recovered_moved(m: &ProfileName, to: &std::path::Path) -> String {
        format!(
            "recovered: ~/.claude and {m} held the same account; kept ~/.claude, moved {m}'s copy to {}; recorded {m} as live",
            to.display()
        )
    }
}
