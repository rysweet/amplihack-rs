//! Whether a Codex process of the user's own reaches its login through the
//! default home link, from `/proc`. Fails closed: anything that cannot be
//! read counts as in use, except a process that has exited. Processes are
//! never signalled, and nothing from `environ` but the `CODEX_HOME` value is
//! kept.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// The most of `environ` read.
const MAX_ENVIRON: u64 = 256 * 1024;
/// The most of `cmdline` read.
const MAX_CMDLINE: u64 = 64 * 1024;
const MAX_COMM: u64 = 64;
/// The most symlinks followed from a `CODEX_HOME` towards the link: the
/// kernel's own limit. A count, not a timer.
const MAX_HOPS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InUse {
    Free,
    /// This process uses the default home, or could not be judged.
    Pid(u32),
    /// The `/proc` root could not be listed.
    ProcUnreadable,
}

/// What one `/proc/<pid>` entry gave.
enum Seen {
    /// Not a Codex process, not ours, or gone.
    Skip,
    /// Uses the default home, or could not be read.
    InUse,
}

/// A process that exited between the listing and the read.
fn gone(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH)
}

fn read_capped(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    fs::File::open(path)?.take(max).read_to_end(&mut buf)?;
    Ok(buf)
}

/// `p` absolute with `.` and `..` removed by text, without following any
/// link, and no trailing `/`.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The link as the kernel finds it: its parent made canonical, and its own
/// name, which is never resolved (that would give the account it points at).
struct LinkId {
    /// The link with `.` and `..` removed by text, for the quick check.
    text: PathBuf,
    parent: PathBuf,
    name: OsString,
}

impl LinkId {
    fn of(link: &Path) -> io::Result<Self> {
        let text = lexical(link);
        let (Some(parent), Some(name)) = (text.parent(), text.file_name()) else {
            return Err(io::Error::other("the link has no parent or no name"));
        };
        Ok(Self {
            parent: fs::canonicalize(parent)?,
            name: name.to_owned(),
            text,
        })
    }
}

/// Whether the absolute path `start` names the link itself: the same text,
/// or the same canonical parent and name, after following any symlinks
/// that `start` and its aliases are, at most [`MAX_HOPS`] of them. So
/// `CODEX_HOME=/d0/you/.codex` names the link `/home/you/.codex` when
/// `/home/you -> /d0/you`, and so does `~/codex-alias -> ~/.codex`. A path
/// that ends in the account the link points at is not the link: a swap does
/// not change what such a process uses. A path that does not exist names
/// nothing. Any other error is returned, and the caller counts it as in use.
///
/// Paths are resolved in the rotator's mount namespace, not the process's.
fn names_link(start: &Path, link: &LinkId) -> io::Result<bool> {
    if lexical(start) == link.text {
        return Ok(true);
    }
    let mut p = start.to_path_buf();
    for _ in 0..=MAX_HOPS {
        // A path ending in `..` or `/` is a real directory, never the link.
        let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
            return Ok(false);
        };
        let parent = match fs::canonicalize(parent) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if parent == link.parent && name == link.name {
            return Ok(true);
        }
        let here = parent.join(name);
        match fs::symlink_metadata(&here) {
            Ok(m) if m.file_type().is_symlink() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        // A relative target is relative to the folder holding the symlink;
        // an absolute one replaces the path.
        p = parent.join(fs::read_link(&here)?);
    }
    Err(io::Error::other("too many symlinks"))
}

/// The scan: the first of the user's Codex processes that uses `link`.
pub(super) fn default_home_in_use(proc_root: &Path, link: &Path, uid: u32) -> InUse {
    let Ok(entries) = fs::read_dir(proc_root) else {
        return InUse::ProcUnreadable;
    };
    let mut pids: Vec<u32> = Vec::new();
    for e in entries {
        let Ok(e) = e else {
            return InUse::ProcUnreadable;
        };
        let name = e.file_name();
        if let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse().ok())
        {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    // Fail closed: without the link's own identity no process can be shown
    // not to use it.
    let Ok(link) = LinkId::of(link) else {
        return InUse::ProcUnreadable;
    };
    for pid in pids {
        if let Seen::InUse = examine(&proc_root.join(pid.to_string()), &link, uid) {
            return InUse::Pid(pid);
        }
    }
    InUse::Free
}

fn examine(dir: &Path, link: &LinkId, uid: u32) -> Seen {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.uid() != uid => return Seen::Skip,
        Ok(_) => {}
        Err(e) if gone(&e) => return Seen::Skip,
        Err(_) => return Seen::InUse,
    }
    match is_codex(dir) {
        Ok(false) => return Seen::Skip,
        Ok(true) => {}
        Err(e) if gone(&e) => return Seen::Skip,
        Err(_) => return Seen::InUse,
    }
    let home = match codex_home(dir) {
        Ok(h) => h,
        Err(e) if gone(&e) => return Seen::Skip,
        Err(_) => return Seen::InUse,
    };
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return Seen::InUse;
    };
    let home = PathBuf::from(OsStr::from_bytes(&home));
    let abs = if home.is_absolute() {
        home
    } else {
        match fs::read_link(dir.join("cwd")) {
            Ok(cwd) => cwd.join(home),
            Err(_) => return Seen::InUse,
        }
    };
    match names_link(&abs, link) {
        Ok(true) | Err(_) => Seen::InUse,
        Ok(false) => Seen::Skip,
    }
}

/// A Codex process: `comm` is `codex`, the file name of `exe` starts with
/// `codex`, or `comm` is `node` and the file name of its first argument is
/// `codex` or `codex.js`.
fn is_codex(dir: &Path) -> io::Result<bool> {
    let comm = read_capped(&dir.join("comm"), MAX_COMM)?;
    let comm = String::from_utf8_lossy(&comm);
    let comm = comm.trim_end_matches('\n');
    if comm == "codex" {
        return Ok(true);
    }
    let exe_codex = fs::read_link(dir.join("exe")).ok().is_some_and(|exe| {
        exe.file_name()
            .is_some_and(|n| n.as_bytes().starts_with(b"codex"))
    });
    if exe_codex {
        return Ok(true);
    }
    if comm != "node" {
        return Ok(false);
    }
    let cmdline = read_capped(&dir.join("cmdline"), MAX_CMDLINE)?;
    let first = cmdline.split(|b| *b == 0).nth(1).unwrap_or_default();
    let name = Path::new(OsStr::from_bytes(first)).file_name();
    Ok(name.is_some_and(|n| n == "codex" || n == "codex.js"))
}

/// The `CODEX_HOME` value from `environ`, read capped; the rest of the
/// buffer is dropped here.
fn codex_home(dir: &Path) -> io::Result<Option<Vec<u8>>> {
    let env = read_capped(&dir.join("environ"), MAX_ENVIRON)?;
    let home = env
        .split(|b| *b == 0)
        .find_map(|kv| kv.strip_prefix(b"CODEX_HOME="))
        .map(<[u8]>::to_vec);
    drop(env);
    Ok(home)
}
