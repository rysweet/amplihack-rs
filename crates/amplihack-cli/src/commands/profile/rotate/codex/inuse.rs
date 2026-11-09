//! Whether a Codex process of the user's own reaches its login through the
//! default home link, from `/proc`. Fails closed: anything that cannot be
//! read counts as in use, except a process that has exited. Processes are
//! never signalled, and nothing from `environ` but the `CODEX_HOME` value is
//! kept.

use std::ffi::OsStr;
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
    let link = lexical(link);
    for pid in pids {
        if let Seen::InUse = examine(&proc_root.join(pid.to_string()), &link, uid) {
            return InUse::Pid(pid);
        }
    }
    InUse::Free
}

fn examine(dir: &Path, link: &Path, uid: u32) -> Seen {
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
    if lexical(&abs) == link {
        Seen::InUse
    } else {
        Seen::Skip
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
