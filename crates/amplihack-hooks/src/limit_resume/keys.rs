//! The keys `limit-resume` sends to a stopped session's tmux pane (issue
//! #2085, D6-D9), and the check that the pane is that session's own
//! terminal (issue #2695).
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
//! to nine digits ([`Pane::parse`]), and when [`check_pane_with`] proves the
//! pane is the terminal of this hook's own session.
//!
//! The ownership proof does not assume that Claude Code itself holds the
//! pane's terminal. On Claude Code 2.1.329 the hook's parent has a pipe on
//! its standard input, so a check that read `/proc/<parent>/fd/0` and
//! compared it with `#{pane_tty}` could never match, and every standing
//! session silently took the exit-2 wake. Instead [`tty_ancestor`] walks up
//! the hook's own ancestry to the nearest process whose standard input is a
//! terminal, and the pane must be that terminal, that process must be its
//! foreground process group, and the session must be reading it (S3). Both
//! of the old predicates, [`same_tty`] and [`ProcStat::foreground`], are
//! kept; only the process they are applied to changed.
//!
//! Every refusal names itself with one of [`UNVERIFIED_REASONS`], a closed
//! set of fixed words, so no `/proc`, tmux or environment text can reach the
//! log. The pane id never reaches a log either: only [`Pane::log_tag`] does.

use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
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

/// Whether `id` is a pane id as a whole: `%` and 1 to 9 digits, nothing
/// else. The only statement of the rule: [`Pane::parse`] admits an id by
/// it, and [`check_pane_with`] and [`check_pane_owned`] apply it again
/// before any tmux call, so an id can never be an option such as `-t` or
/// carry a separator. One rule, so the two uses cannot drift apart.
fn valid_pane_id(id: &str) -> bool {
    id.strip_prefix('%').is_some_and(|d| {
        !d.is_empty() && d.len() <= MAX_PANE_DIGITS && d.bytes().all(|b| b.is_ascii_digit())
    })
}

impl Pane {
    /// `$TMUX` and `$TMUX_PANE` to a pane: `$TMUX` set and non-empty (an
    /// inherited `$TMUX_PANE` alone is not a pane, D6), and `$TMUX_PANE`
    /// exactly `^%[0-9]{1,9}$`, by the one `valid_pane_id` rule.
    pub fn parse(tmux: Option<&str>, tmux_pane: Option<&str>) -> Option<Self> {
        tmux.filter(|t| !t.is_empty())?;
        let id = tmux_pane?;
        valid_pane_id(id).then(|| Self(id.to_string()))
    }

    /// The pane id, `%N`, for tmux's `-t`.
    pub fn id(&self) -> &str {
        &self.0
    }

    /// The first 12 hex digits of a SHA-256 of the id: what logs show. It is a
    /// correlation aid, not a secret: the salt is a fixed constant and the pane-id
    /// space is small, so the tag is enumerable by design. It keeps raw ids and
    /// tty paths out of the log; it does not hide them from someone who holds it.
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

/// The `/proc/<pid>/stat` fields the pane check needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStat {
    /// The parent process.
    pub ppid: i32,
    /// The process group.
    pub pgrp: i32,
    /// The foreground process group of the controlling terminal, or -1.
    pub tpgid: i32,
}

/// The one foreground rule: a process's group is its terminal's foreground
/// process group. `tpgid` is -1 for a process with no controlling terminal,
/// so such a process is never foreground. Both [`ProcStat`], which
/// [`foreground_now`] reads, and [`ProcEntry`], which the pane check reads,
/// state the rule through this, so the two cannot drift apart.
const fn is_foreground(pgrp: i32, tpgid: i32) -> bool {
    tpgid > 0 && pgrp == tpgid
}

impl ProcStat {
    /// The process's group is its terminal's foreground group.
    pub fn foreground(&self) -> bool {
        is_foreground(self.pgrp, self.tpgid)
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
    let ppid: i32 = f.next()?.parse().ok()?;
    let pgrp: i32 = f.next()?.parse().ok()?;
    let _session: i32 = f.next()?.parse().ok()?;
    let _tty_nr: i64 = f.next()?.parse().ok()?;
    let tpgid: i32 = f.next()?.parse().ok()?;
    Some(ProcStat { ppid, pgrp, tpgid })
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

/// The hook's own ancestry is walked no further than this (issue #2695).
/// A session's launcher sits one or two processes above Claude Code; a
/// chain longer than this is malformed, not a session.
pub const MAX_ANCESTRY: usize = 16;

/// No ancestor of the hook, up to [`MAX_ANCESTRY`], has a terminal on its
/// standard input.
pub const NO_TTY_ANCESTOR: &str = "no-tty-ancestor";
/// The pane's `#{pane_tty}` is not the terminal the hook's own ancestry
/// holds, or tmux could not be asked for it.
pub const TTY_MISMATCH: &str = "tty-mismatch";
/// The process holding the terminal is not its foreground process group.
pub const NOT_FOREGROUND: &str = "not-foreground";
/// The walk reached [`MAX_ANCESTRY`] processes, or the chain looped.
pub const WALK_LIMIT: &str = "walk-limit";
/// A `/proc` entry on the way up could not be read or parsed, or a parent
/// pid of 0 ended the chain.
pub const UNREADABLE_PROC: &str = "unreadable-proc";
/// `$TMUX_PANE` is not `%` followed by digits.
pub const BAD_PANE_ID: &str = "bad-pane-id";

/// Every reason a pane can be refused. The log's `detail` for
/// `pane-unverified` is always one of these, so neither `/proc` content,
/// tmux output, environment values nor error text can reach the file.
pub const UNVERIFIED_REASONS: [&str; 6] = [
    NO_TTY_ANCESTOR,
    TTY_MISMATCH,
    NOT_FOREGROUND,
    WALK_LIMIT,
    UNREADABLE_PROC,
    BAD_PANE_ID,
];

/// What one process on the way up looks like to the pane check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    /// The parent process.
    pub ppid: u32,
    /// The process group.
    pub pgrp: i32,
    /// The foreground process group of the controlling terminal, or -1.
    pub tpgid: i32,
    /// Who owns the `/proc` entry.
    pub uid: u32,
    /// What standard input points at, when the link can be read.
    pub stdin: Option<PathBuf>,
}

impl ProcEntry {
    /// The process's group is its terminal's foreground group, by the one
    /// `is_foreground` rule [`ProcStat::foreground`] also states.
    pub fn foreground(&self) -> bool {
        is_foreground(self.pgrp, self.tpgid)
    }
}

/// The seam over `/proc` the pane check reads through: [`ProcFs`] in the
/// hook, an injected tree in tests.
pub trait ProcView {
    /// One process, or `None` when its `/proc` entry cannot be read.
    fn entry(&self, pid: u32) -> Option<ProcEntry>;
}

/// `/proc` as the kernel presents it.
pub struct ProcFs;

impl ProcView for ProcFs {
    fn entry(&self, pid: u32) -> Option<ProcEntry> {
        let dir = Path::new("/proc").join(pid.to_string());
        // The directory's owner is the process's real user; a process of
        // another user ends the walk.
        let uid = std::fs::metadata(&dir).ok()?.uid();
        let stat = parse_stat(&std::fs::read_to_string(dir.join("stat")).ok()?)?;
        Some(ProcEntry {
            ppid: u32::try_from(stat.ppid).ok()?,
            pgrp: stat.pgrp,
            tpgid: stat.tpgid,
            uid,
            stdin: std::fs::read_link(dir.join("fd").join("0")).ok(),
        })
    }
}

/// Whether `p` names a terminal device: an absolute path with no `..`,
/// either `/dev/pts/<digits>` or `/dev/tty…`. A pipe, a socket and
/// `/dev/null` are not terminals, so they never end the walk early.
fn is_terminal_path(p: &Path) -> bool {
    if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let Ok(rest) = p.strip_prefix("/dev") else {
        return false;
    };
    let mut parts = rest.components();
    let Some(Component::Normal(first)) = parts.next() else {
        return false;
    };
    if first.as_encoded_bytes() == b"pts" {
        let Some(Component::Normal(n)) = parts.next() else {
            return false;
        };
        let digits = !n.is_empty() && n.as_encoded_bytes().iter().all(u8::is_ascii_digit);
        return digits && parts.next().is_none();
    }
    parts.next().is_none() && first.as_encoded_bytes().starts_with(b"tty")
}

/// The nearest ancestor of `hook_pid` whose standard input is a terminal.
///
/// The walk starts at the hook's parent and goes up at most
/// [`MAX_ANCESTRY`] processes. It stops at pid 1, which adopts orphans and
/// owns no session; at a parent pid of 0, which no readable chain has; at a
/// pid it has already visited, so a malformed chain cannot loop; and at the
/// first process that is not the hook's own user, so the walk never leaves
/// the user that started the session. Every failure is one of
/// [`UNVERIFIED_REASONS`], and none of them is a pane.
pub fn tty_ancestor<V: ProcView + ?Sized>(
    view: &V,
    hook_pid: u32,
    euid: u32,
) -> Result<u32, &'static str> {
    let Some(start) = view.entry(hook_pid) else {
        return Err(UNREADABLE_PROC);
    };
    if start.uid != euid {
        return Err(NO_TTY_ANCESTOR);
    }
    let mut pid = start.ppid;
    let mut visited: Vec<u32> = Vec::with_capacity(MAX_ANCESTRY);
    for _ in 0..MAX_ANCESTRY {
        if pid == 0 {
            return Err(UNREADABLE_PROC);
        }
        if pid == 1 {
            return Err(NO_TTY_ANCESTOR);
        }
        if visited.contains(&pid) {
            return Err(WALK_LIMIT);
        }
        visited.push(pid);
        let Some(entry) = view.entry(pid) else {
            return Err(UNREADABLE_PROC);
        };
        if entry.uid != euid {
            return Err(NO_TTY_ANCESTOR);
        }
        if entry.stdin.as_deref().is_some_and(is_terminal_path) {
            return Ok(pid);
        }
        pid = entry.ppid;
    }
    Err(WALK_LIMIT)
}

/// The verdict on the pane named by the session's own environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneCheck {
    /// The pane is this session's own terminal: the keys may be sent.
    Ready(Pane),
    /// The session named no pane, so there is nothing to check.
    None,
    /// A pane was named but not proved to be this session's terminal. The
    /// reason is one of [`UNVERIFIED_REASONS`].
    Unverified(&'static str),
    /// The pane is this session's terminal, but the session is not the
    /// process reading it.
    NotReading,
}

/// The pane check once the terminal's holder is known: the whole of the
/// decision and the only place `Ready` is produced.
///
/// `holder_pid` must be what [`tty_ancestor`] returned for this hook, the
/// nearest ancestor whose standard input is a terminal. Keeping the walk
/// outside this function is what lets both callers walk the ancestry
/// exactly once.
///
/// `Ready` needs all of: `pane_tty_output` naming the holder's own terminal
/// ([`same_tty`]); the holder being that terminal's foreground process
/// group ([`ProcEntry::foreground`]); and `parent`, the session,
/// sharing that foreground group, so the keys reach the session and not a
/// launcher or a shell prompt behind it. Every other path refuses.
pub(crate) fn check_pane_held<V: ProcView + ?Sized>(
    view: &V,
    holder_pid: u32,
    parent: u32,
    pane: Pane,
    pane_tty_output: &[u8],
) -> PaneCheck {
    let Some(holder) = view.entry(holder_pid) else {
        return PaneCheck::Unverified(UNREADABLE_PROC);
    };
    let Some(tty) = holder.stdin.as_deref() else {
        return PaneCheck::Unverified(UNREADABLE_PROC);
    };
    if !same_tty(pane_tty_output, tty) {
        return PaneCheck::Unverified(TTY_MISMATCH);
    }
    if !holder.foreground() {
        return PaneCheck::Unverified(NOT_FOREGROUND);
    }
    let Some(session) = view.entry(parent) else {
        return PaneCheck::Unverified(UNREADABLE_PROC);
    };
    if session.pgrp != holder.tpgid {
        return PaneCheck::NotReading;
    }
    PaneCheck::Ready(pane)
}

/// The pane check, with `/proc` and tmux's answer supplied: a pane id that
/// is one, one walk up the hook's ancestry ([`tty_ancestor`]), then
/// `check_pane_held`. This is the pure core the tests drive.
pub fn check_pane_with<V: ProcView + ?Sized>(
    view: &V,
    hook_pid: u32,
    parent: u32,
    euid: u32,
    pane: Option<Pane>,
    pane_tty_output: &[u8],
) -> PaneCheck {
    let Some(pane) = pane else {
        return PaneCheck::None;
    };
    if !valid_pane_id(pane.id()) {
        return PaneCheck::Unverified(BAD_PANE_ID);
    }
    let holder_pid = match tty_ancestor(view, hook_pid, euid) {
        Ok(pid) => pid,
        Err(reason) => return PaneCheck::Unverified(reason),
    };
    check_pane_held(view, holder_pid, parent, pane, pane_tty_output)
}

/// [`check_pane_with`] against the live `/proc` tree and the tmux server
/// `$TMUX` names, in the order validate, walk, ask tmux, decide.
///
/// The pane id is checked before anything is spawned, and the ancestry is
/// walked once, so a tmux call is made only once the hook knows which
/// terminal it is looking for. A tmux run that fails or prints nothing
/// leaves the pane's terminal unproved, which is a [`TTY_MISMATCH`]. No
/// other pane is ever considered: this never lists panes or clients. There
/// is no retry, delay or poll: the check runs once.
pub fn check_pane_owned(pane: Option<&Pane>, parent: u32) -> PaneCheck {
    let Some(pane) = pane else {
        return PaneCheck::None;
    };
    if !valid_pane_id(pane.id()) {
        return PaneCheck::Unverified(BAD_PANE_ID);
    }
    let view = ProcFs;
    let hook_pid = std::process::id();
    let euid = amplihack_utils::secure_files::euid();
    let holder_pid = match tty_ancestor(&view, hook_pid, euid) {
        Ok(pid) => pid,
        Err(reason) => return PaneCheck::Unverified(reason),
    };
    let out = pane_tty(pane).unwrap_or_default();
    check_pane_held(&view, holder_pid, parent, pane.clone(), &out)
}
