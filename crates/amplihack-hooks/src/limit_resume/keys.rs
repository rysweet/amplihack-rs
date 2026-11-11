//! The keys `limit-resume` sends to a stopped session's tmux pane (issue
//! #2085, D6-D9), and the check that the pane is that session's.
//!
//! Every resume has four steps, [`RESUME_KEYS`]: Escape (closes the limit menu or
//! timed wait), C-u (clears the input line), `continue` (sent literally, with
//! fixed affinity warning text when required) and
//! Enter. C-u deletes any text the user typed and did not submit: only a
//! submitted prompt counts as moving on, so an unsent draft does not stop
//! the resume (D8). Whether these keys end a real Claude Code timed wait,
//! and whether Escape alone clears the line, is checked in #2215. Each is one `tmux send-keys` run with a fixed argv, no shell and
//! null stdio, ended by its exit status. No digit and no slash command can
//! be expressed.
//!
//! A pane is used only when `$TMUX` is set and `$TMUX_PANE` is `%` and up
//! to nine digits ([`Pane::parse`]), and when [`verify_pane`] finds that the
//! pane's tty is Claude Code's stdin and Claude Code is that tty's
//! foreground process group (S3). The id never reaches a log: only
//! [`Pane::log_tag`] does.

use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// A key the hook may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `Escape`: leaves the limit menu or the timed wait.
    Escape,
    /// `C-u`: clears whatever is on the input line, an unsent draft included.
    ClearLine,
    /// `continue`, sent with `-l` so tmux types it as text.
    LiteralContinue,
    LiteralCrossing,
    LiteralUnresolved,
    /// `Enter`: submits the line.
    Enter,
}

/// What a resume sends, in order.
pub const RESUME_KEYS: [Key; 4] = [
    Key::Escape,
    Key::ClearLine,
    Key::LiteralContinue,
    Key::Enter,
];

/// Hash prefix length of a pane's log tag.
const TAG_CHARS: usize = 12;
/// Longest pane id: `%` and nine digits.
const MAX_PANE_DIGITS: usize = 9;
/// Longest accepted `#{pane_tty}` output, without its newline.
const MAX_TTY_BYTES: usize = 256;

/// A tmux pane id (`%N`). Built only by [`Pane::parse`]. `Debug` shows the
/// log tag; there is no `Display`, so the id is never formatted by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Pane(String);

impl Pane {
    /// `$TMUX` and `$TMUX_PANE` to a pane: `$TMUX` set and non-empty (an
    /// inherited `$TMUX_PANE` alone is not a pane, D6), and `$TMUX_PANE`
    /// exactly `^%[0-9]{1,9}$`.
    pub fn parse(tmux: Option<&str>, tmux_pane: Option<&str>) -> Option<Self> {
        tmux.filter(|t| !t.is_empty())?;
        let id = tmux_pane?;
        let digits = id.strip_prefix('%')?;
        let ok = !digits.is_empty()
            && digits.len() <= MAX_PANE_DIGITS
            && digits.bytes().all(|b| b.is_ascii_digit());
        ok.then(|| Self(id.to_string()))
    }

    /// The pane id, `%N`, for tmux's `-t`.
    pub fn id(&self) -> &str {
        &self.0
    }

    /// The first 12 hex digits of a SHA-256 of the id: what logs show.
    pub fn log_tag(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"amplihack-limit-resume-pane-v1\0");
        h.update(self.0.as_bytes());
        let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        hex[..TAG_CHARS].to_string()
    }
}

impl std::fmt::Debug for Pane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Pane").field(&self.log_tag()).finish()
    }
}

/// The `tmux` argv that sends `key` to `pane`. Each key is one run.
pub fn argv(pane: &Pane, key: Key) -> Vec<String> {
    let mut a = vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane.id().to_string(),
    ];
    match key {
        Key::Escape => a.push("Escape".into()),
        Key::ClearLine => a.push("C-u".into()),
        Key::LiteralContinue => a.extend(["-l".to_string(), "continue".to_string()]),
        Key::LiteralCrossing | Key::LiteralUnresolved => {
            let affinity = if key == Key::LiteralCrossing {
                amplihack_utils::organisation_affinity::Affinity::Crossing
            } else {
                amplihack_utils::organisation_affinity::Affinity::Unresolved
            };
            a.extend([
                "-l".to_string(),
                format!("{} Continue.", affinity.warning().unwrap_or_default()),
            ]);
        }
        Key::Enter => a.push("Enter".into()),
    }
    a
}

/// The `tmux` argv that prints `pane`'s tty.
pub fn pane_tty_argv(pane: &Pane) -> Vec<String> {
    vec![
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane.id().to_string(),
        "#{pane_tty}".to_string(),
    ]
}

/// Runs `program` with [`argv`] for `key`, without a shell and with null
/// stdio. A non-zero exit, or a program that cannot start, is an error.
/// The environment is inherited, so `$TMUX` selects the server.
pub fn send_with(program: &OsStr, pane: &Pane, key: Key) -> io::Result<()> {
    let status = Command::new(program)
        .args(argv(pane, key))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(match status.code() {
            Some(code) => format!("tmux send-keys exited with status {code}"),
            None => "tmux send-keys was stopped by a signal".to_string(),
        }))
    }
}

/// [`send_with`] for `tmux`, found on `PATH`.
pub fn tmux_send(pane: &Pane, key: Key) -> io::Result<()> {
    send_with(OsStr::new("tmux"), pane, key)
}

/// `pane`'s tty as tmux prints it, newline included; an error when tmux
/// cannot be run or exits non-zero.
fn pane_tty(pane: &Pane) -> io::Result<Vec<u8>> {
    let out = Command::new("tmux")
        .args(pane_tty_argv(pane))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(io::Error::other("tmux display-message failed"))
    }
}

/// The two `/proc/<pid>/stat` fields the pane check needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStat {
    /// The process group.
    pub pgrp: i32,
    /// The foreground process group of the controlling terminal, or -1.
    pub tpgid: i32,
}

impl ProcStat {
    /// The process's group is its terminal's foreground group.
    pub fn foreground(&self) -> bool {
        self.tpgid > 0 && self.pgrp == self.tpgid
    }
}

/// Parses `/proc/<pid>/stat`. The command name may hold spaces and `)`, so
/// the fields are read after the last `)`: state, ppid, pgrp, session,
/// tty_nr, tpgid. `None` for anything else.
pub fn parse_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let mut f = text[close + 1..].split_ascii_whitespace();
    let _state = f.next()?;
    let _ppid: i32 = f.next()?.parse().ok()?;
    let pgrp: i32 = f.next()?.parse().ok()?;
    let _session: i32 = f.next()?.parse().ok()?;
    let _tty_nr: i64 = f.next()?.parse().ok()?;
    let tpgid: i32 = f.next()?.parse().ok()?;
    Some(ProcStat { pgrp, tpgid })
}

/// Whether tmux's `#{pane_tty}` output names `stdin_target`, the file
/// Claude Code's stdin points at: at most 256 bytes and one trailing
/// newline, an absolute path under `/dev/` without `..`, and the same path.
pub fn same_tty(pane_tty_output: &[u8], stdin_target: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let raw = pane_tty_output
        .strip_suffix(b"\n")
        .unwrap_or(pane_tty_output);
    if raw.is_empty() || raw.len() > MAX_TTY_BYTES || raw.iter().any(|b| b.is_ascii_whitespace()) {
        return false;
    }
    let tty = Path::new(OsStr::from_bytes(raw));
    let plain = tty
        .components()
        .all(|c| !matches!(c, std::path::Component::ParentDir));
    plain && tty.starts_with("/dev/") && tty == stdin_target
}

/// Whether `pid` is in its terminal's foreground process group now: one
/// read of `/proc/<pid>/stat`. A suspended (Ctrl-Z) or vanished process,
/// or any read or parse failure, is `false`.
pub fn foreground_now(pid: u32) -> bool {
    std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat"))
        .is_ok_and(|stat| parse_stat(&stat).is_some_and(|s| s.foreground()))
}

/// Whether `pane` is the terminal of the Claude Code process `parent`: the
/// pane's tty is the target of `/proc/<parent>/fd/0`, and `parent` is the
/// foreground process group there (S3). Any failure is `false`.
pub fn verify_pane(parent: u32, pane: &Pane) -> bool {
    let proc_dir = Path::new("/proc").join(parent.to_string());
    let Ok(stdin_target) = std::fs::read_link(proc_dir.join("fd").join("0")) else {
        return false;
    };
    if !foreground_now(parent) {
        return false;
    }
    pane_tty(pane).is_ok_and(|out| same_tty(&out, &stdin_target))
}
