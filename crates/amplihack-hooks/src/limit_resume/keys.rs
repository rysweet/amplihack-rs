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
//! The pane comes from one of two places (issue #3104). When `$TMUX` is set
//! and `$TMUX_PANE` is `%` and up to nine digits ([`Pane::parse`]), that is
//! the pane, and a pane that fails the proof is refused, never replaced.
//! When the environment names no pane, the hook walks its own ancestry once
//! to the terminal its session holds and runs one read-only
//! `tmux list-panes -a` ([`list_panes_argv`]); the single pane on that
//! terminal ([`pane_on_tty`]) is the pane. Either way the pane is used only
//! when [`check_pane_held`] proves it is the terminal of this hook's own
//! session, by the same proof and with the same refusal words. The listing
//! is matched in memory and dropped; no pane id or tty from it is logged.
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
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, Read};
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
///
/// Escape and C-u must stay first. A commit released after a key failed
/// (issue #3104) lets another hook type the whole sequence again, and these
/// two clear whatever a partial earlier run left on the input line.
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
/// Longest `tmux list-panes` output the pane lookup reads. A listing past
/// it is refused whole, never cut and matched.
const MAX_LISTING_BYTES: usize = 64 * 1024;

/// A tmux pane id (`%N`). Built only by [`Pane::parse`] and, for a pane
/// found on the session's terminal, by the crate's own `Pane::listed`.
/// `Debug` shows the log tag; there is no `Display`, so the id is never
/// formatted by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Pane(String);

/// Whether `id` is a pane id as a whole: `%` and 1 to 9 digits, nothing
/// else. The only statement of the rule: [`Pane::parse`] and `Pane::listed`
/// admit an id by it, and [`check_pane_with`] and [`find_pane_with`] apply
/// it again before any tmux call, so an id can never be an option such as
/// `-t` or carry a separator. One rule, so the uses cannot drift apart.
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

    /// A pane id read from tmux's own pane listing, by the one
    /// `valid_pane_id` rule. Private to the crate: only [`pane_on_tty`]
    /// builds a pane this way, and the pane is then proved like any other.
    pub(crate) fn listed(id: &str) -> Option<Self> {
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

/// The `tmux` argv that lists every pane of the server with its tty, one
/// `<pane id> <tty>` line each. Read-only: it names no target and sends
/// nothing.
pub fn list_panes_argv() -> Vec<String> {
    ["list-panes", "-a", "-F", "#{pane_id} #{pane_tty}"]
        .into_iter()
        .map(str::to_string)
        .collect()
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

/// What `tmux list-panes` prints for every pane of the server `$TMUX`
/// names (the default server when it is unset), read once through a pipe
/// and never past [`MAX_LISTING_BYTES`]. No shell, null stdin and stderr,
/// and the inherited environment. An error when tmux cannot be run, prints
/// more than the cap, or exits non-zero.
fn list_panes() -> io::Result<Vec<u8>> {
    let mut child = Command::new("tmux")
        .args(list_panes_argv())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = Vec::new();
    // The pipe is dropped at the end of this statement, before the wait, so
    // a tmux still writing past the cap gets EPIPE instead of blocking.
    let read = match child.stdout.take() {
        Some(pipe) => pipe
            .take(MAX_LISTING_BYTES as u64 + 1)
            .read_to_end(&mut out),
        None => Err(io::Error::other("tmux list-panes gave no output pipe")),
    };
    let status = child.wait()?;
    read?;
    if out.len() > MAX_LISTING_BYTES {
        return Err(io::Error::other(
            "tmux list-panes printed more than the pane lookup reads",
        ));
    }
    if !status.success() {
        return Err(io::Error::other("tmux list-panes failed"));
    }
    Ok(out)
}

/// One `<pane id> <tty>` line of a pane listing, or `None` when it is not
/// exactly that: one ASCII space, an id by the one `valid_pane_id` rule,
/// and a tty of 1 to [`MAX_TTY_BYTES`] bytes with no ASCII whitespace.
fn listing_line(line: &[u8]) -> Option<(&str, &[u8])> {
    let mut parts = line.split(|b| *b == b' ');
    let (id, tty) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let id = std::str::from_utf8(id)
        .ok()
        .filter(|id| valid_pane_id(id))?;
    let tty_ok =
        !tty.is_empty() && tty.len() <= MAX_TTY_BYTES && !tty.iter().any(u8::is_ascii_whitespace);
    tty_ok.then_some((id, tty))
}

/// The single pane `listing` (what [`list_panes_argv`] prints) shows on
/// `holder_tty`, the terminal the hook's own ancestry holds.
///
/// The listing is untrusted and read strictly: at most
/// [`MAX_LISTING_BYTES`], one trailing newline stripped, and every line
/// `<id> <tty>` as `listing_line` states it. One line that is not, an empty
/// line or a carriage return included, makes the whole listing malformed,
/// and so does one id shown on two different ttys. Identical lines count as
/// one pane, since `-a` lists a window once for each session it is linked
/// to. A line matches when [`same_tty`] holds. `None` unless `holder_tty`
/// is a terminal path and exactly one pane id matches; nothing from the
/// listing is kept but that id.
pub fn pane_on_tty(listing: &[u8], holder_tty: &Path) -> Option<Pane> {
    if listing.len() > MAX_LISTING_BYTES || !is_terminal_path(holder_tty) {
        return None;
    }
    let body = listing.strip_suffix(b"\n").unwrap_or(listing);
    if body.is_empty() {
        return None;
    }
    let mut panes: BTreeMap<&str, &[u8]> = BTreeMap::new();
    for line in body.split(|b| *b == b'\n') {
        let (id, tty) = listing_line(line)?;
        if panes.insert(id, tty).is_some_and(|seen| seen != tty) {
            return None;
        }
    }
    let mut matching = panes
        .iter()
        .filter(|(_, tty)| same_tty(tty, holder_tty))
        .map(|(id, _)| *id);
    let id = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Pane::listed(id)
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

/// The verdict on the session's pane: the one its own environment names,
/// or else the one found on its terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneCheck {
    /// The pane is this session's own terminal: the keys may be sent.
    Ready(Pane),
    /// No pane was named, and none was found on the session's terminal, so
    /// there is nothing to check.
    None,
    /// A pane was named or found but not proved to be this session's
    /// terminal. The reason is one of [`UNVERIFIED_REASONS`].
    Unverified(&'static str),
    /// The pane is this session's terminal, but the session is not the
    /// process reading it.
    NotReading,
}

/// The pane check once the terminal's holder is known: the whole of the
/// decision and the only place `Ready` is produced, for a pane named by the
/// environment and for one found on the terminal alike.
///
/// `holder_pid` must be what [`tty_ancestor`] returned for this hook, the
/// nearest ancestor whose standard input is a terminal. Keeping the walk
/// outside this function is what lets every caller walk the ancestry
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

/// The pane check with `/proc` and both tmux questions supplied: the pure
/// core the tests drive, and what [`check_pane_owned_in`] runs with the
/// real tmux calls. Each question is asked at most once.
///
/// With `env_pane`, the pane the session's environment names, this is
/// exactly [`check_pane_with`]: the id is validated, the ancestry walked
/// once, `pane_tty` asked, and [`check_pane_held`] decides. A refusal is
/// final; it never falls back to the lookup below.
///
/// With no `env_pane` (issue #3104), the ancestry is walked once. When no
/// terminal is found the verdict is [`PaneCheck::None`] and tmux is not
/// run. Otherwise `list_panes` is asked once and [`pane_on_tty`] picks the
/// single pane on the holder's terminal; a failed or unmatched listing is
/// `None`. That pane is then proved exactly as an environment pane is: one
/// `pane_tty` answer, then [`check_pane_held`]. A pane id reused between
/// the listing and the proof is a [`TTY_MISMATCH`].
pub fn find_pane_with<V, L, T>(
    view: &V,
    hook_pid: u32,
    parent: u32,
    euid: u32,
    env_pane: Option<Pane>,
    list_panes: L,
    pane_tty: T,
) -> PaneCheck
where
    V: ProcView + ?Sized,
    L: FnOnce() -> io::Result<Vec<u8>>,
    T: FnOnce(&Pane) -> io::Result<Vec<u8>>,
{
    if let Some(pane) = env_pane {
        if !valid_pane_id(pane.id()) {
            return PaneCheck::Unverified(BAD_PANE_ID);
        }
        let holder_pid = match tty_ancestor(view, hook_pid, euid) {
            Ok(pid) => pid,
            Err(reason) => return PaneCheck::Unverified(reason),
        };
        let out = pane_tty(&pane).unwrap_or_default();
        return check_pane_held(view, holder_pid, parent, pane, &out);
    }
    let Ok(holder_pid) = tty_ancestor(view, hook_pid, euid) else {
        return PaneCheck::None;
    };
    let Some(holder_tty) = view.entry(holder_pid).and_then(|e| e.stdin) else {
        return PaneCheck::None;
    };
    let Ok(listing) = list_panes() else {
        return PaneCheck::None;
    };
    let found = pane_on_tty(&listing, &holder_tty);
    drop(listing);
    let Some(pane) = found else {
        return PaneCheck::None;
    };
    let out = pane_tty(&pane).unwrap_or_default();
    check_pane_held(view, holder_pid, parent, pane, &out)
}

/// [`find_pane_with`] over `view`, with the real tmux calls: one
/// `tmux list-panes -a` when the lookup needs it, and one
/// `tmux display-message` for the proof. The seam the stand-in tests drive
/// with an injected process tree and a stand-in `tmux` on `PATH`.
pub fn check_pane_owned_in<V: ProcView + ?Sized>(
    view: &V,
    hook_pid: u32,
    parent: u32,
    euid: u32,
    env_pane: Option<Pane>,
) -> PaneCheck {
    find_pane_with(view, hook_pid, parent, euid, env_pane, list_panes, pane_tty)
}

/// The pane check against the live `/proc` tree and the tmux server
/// `$TMUX` names: the pane the session's environment names, or else the
/// single pane on the session's terminal, proved either way by
/// [`check_pane_held`] (see [`find_pane_with`]).
///
/// A named pane id is checked before anything is spawned, and the ancestry
/// is walked once, so a tmux call is made only once the hook knows which
/// terminal it is looking for. A proof run that fails or prints nothing
/// leaves the pane's terminal unproved, which is a [`TTY_MISMATCH`]. Panes
/// are listed only when the environment names none, and the listing is
/// matched and dropped. There is no retry, delay or poll: the check runs
/// once.
pub fn check_pane_owned(pane: Option<&Pane>, parent: u32) -> PaneCheck {
    check_pane_owned_in(
        &ProcFs,
        std::process::id(),
        parent,
        amplihack_utils::secure_files::euid(),
        pane.cloned(),
    )
}

// ── What the proved pane is showing (issue #2695) ───────────────────────────

/// Longest capture the screen check looks at. A pane is at most a few
/// hundred columns by a few hundred rows, so this is far above any real
/// screen; bytes past it are ignored rather than matched.
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// The fixed phrases that make a pane's visible screen one the resume keys
/// answer, matched as lowercase substrings.
///
/// A closed set, and deliberately short. `What do you want to do?` is the
/// menu the three standing sessions in #2695 were held on; `usage limit`
/// and `continuing automatically` are the line five sessions were held on
/// in #2085; the three window names are what Claude Code prints when a
/// usage limit stops a turn.
///
/// `amplihack-cli`'s `limit_patterns.rs` holds the recipe runner's own
/// limit vocabulary. It is not shared here: this crate does not depend on
/// the CLI, and the two answer different questions. That list asks whether
/// a run's *output* reported a limit; this one asks whether a *terminal* is
/// showing a prompt that Escape and a typed `continue` are the answer to.
pub const SCREEN_MARKERS: [&str; 6] = [
    "what do you want to do?",
    "usage limit",
    "continuing automatically",
    "session limit",
    "5-hour limit",
    "weekly limit",
];

/// tmux could not be asked what the pane shows, or it printed nothing.
pub const SCREEN_UNREADABLE: &str = "screen-unreadable";
/// The pane was read and shows none of [`SCREEN_MARKERS`].
pub const SCREEN_UNKNOWN: &str = "screen-unknown";

/// Every reason a screen can be refused. The log's `detail` for
/// `screen-unrecognised` is always one of these, so no captured text can
/// reach the file.
pub const UNRECOGNISED_REASONS: [&str; 2] = [SCREEN_UNREADABLE, SCREEN_UNKNOWN];

/// The verdict on what the proved pane is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenCheck {
    /// The screen holds one of [`SCREEN_MARKERS`]: the keys answer it.
    Held,
    /// It does not, or it could not be read. The reason is one of
    /// [`UNRECOGNISED_REASONS`].
    Unrecognised(&'static str),
}

/// The `tmux` argv that prints what `pane` is showing. `-p` writes to
/// standard output; no `-S` is given, so only the visible screen is read
/// and never the scrollback.
pub fn capture_argv(pane: &Pane) -> Vec<String> {
    vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane.id().to_string(),
    ]
}

/// The verdict on text tmux printed: the whole rule, and the only place a
/// marker is matched. Bytes past [`MAX_CAPTURE_BYTES`] are ignored, and
/// nothing read here is returned, so no screen text can reach a log or a
/// message.
pub fn recognise(capture: &[u8]) -> ScreenCheck {
    let head = &capture[..capture.len().min(MAX_CAPTURE_BYTES)];
    if head.iter().all(u8::is_ascii_whitespace) {
        return ScreenCheck::Unrecognised(SCREEN_UNREADABLE);
    }
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    if SCREEN_MARKERS.iter().any(|m| text.contains(m)) {
        ScreenCheck::Held
    } else {
        ScreenCheck::Unrecognised(SCREEN_UNKNOWN)
    }
}

/// [`recognise`] for what the tmux server `$TMUX` names shows in `pane`.
///
/// The pane id is checked against the one `valid_pane_id` rule again before
/// the call, so it can never become an option. A run that cannot start,
/// exits non-zero or prints nothing leaves the screen unrecognised, which
/// sends no keys. The capture runs once: no retry, delay or poll.
pub fn screen_of(pane: &Pane) -> ScreenCheck {
    if !valid_pane_id(pane.id()) {
        return ScreenCheck::Unrecognised(SCREEN_UNREADABLE);
    }
    let Ok(out) = Command::new("tmux")
        .args(capture_argv(pane))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return ScreenCheck::Unrecognised(SCREEN_UNREADABLE);
    };
    if !out.status.success() {
        return ScreenCheck::Unrecognised(SCREEN_UNREADABLE);
    }
    recognise(&out.stdout)
}
