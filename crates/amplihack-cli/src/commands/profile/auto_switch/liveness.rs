//! Which profile directories a running process is using, from
//! `/proc/<pid>/environ`. A directory is live when a process of the same user,
//! other than this one and not a zombie, has `CLAUDE_CONFIG_DIR` resolving to
//! it. Any doubt counts as live.

use super::fsx;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_ENVIRON: u64 = 16 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;
const KEY: &[u8] = b"CLAUDE_CONFIG_DIR=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// Profile names with a live process.
    Known(BTreeSet<String>),
    /// A Claude-like process's environment could not be read.
    Everything,
    /// `/proc` cannot be used here.
    Unsupported,
}

impl Liveness {
    pub fn is_live(&self, name: &ProfileName) -> bool {
        match self {
            Self::Known(set) => set.contains(name.as_str()),
            Self::Everything | Self::Unsupported => true,
        }
    }

    pub fn supported(&self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// The live set for this process on this machine.
pub fn scan_system(root: &Path) -> Liveness {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        match root.canonicalize() {
            Ok(root) => scan(Path::new("/proc"), &root, std::process::id(), uid),
            Err(_) => Liveness::Unsupported,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Liveness::Unsupported
    }
}

/// `root` must be canonical.
pub fn scan(proc_root: &Path, root: &Path, self_pid: u32, uid: u32) -> Liveness {
    if !matches!(
        fsx::read_bounded_nofollow(&proc_root.join("self/environ"), MAX_ENVIRON),
        Ok(Some(_))
    ) {
        return Liveness::Unsupported;
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Liveness::Unsupported;
    };
    let mut live = BTreeSet::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let dir = entry.path();
        let Some((state, owner)) = status(&dir) else {
            continue; // gone
        };
        if owner != uid || state == 'Z' {
            continue;
        }
        let environ = match fsx::read_bounded_nofollow(&dir.join("environ"), MAX_ENVIRON) {
            Ok(Some((raw, _))) => raw,
            Ok(None) => continue, // exited
            Err(_) => {
                if looks_like_claude(&dir) {
                    return Liveness::Everything;
                }
                continue;
            }
        };
        if let Some(value) = environ.split(|&b| b == 0).find_map(|e| e.strip_prefix(KEY))
            && let Some(name) = resolve(&dir, value, root)
        {
            live.insert(name);
        }
    }
    Liveness::Known(live)
}

/// `(state letter, real uid)` from `/proc/<pid>/status`.
fn status(dir: &Path) -> Option<(char, u32)> {
    let (raw, _) = fsx::read_bounded_nofollow(&dir.join("status"), MAX_SMALL).ok()??;
    let text = String::from_utf8_lossy(&raw);
    let mut state = None;
    let mut uid = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("State:") {
            state = v.trim().chars().next();
        } else if let Some(v) = line.strip_prefix("Uid:") {
            uid = v.split_whitespace().next().and_then(|u| u.parse().ok());
        }
    }
    Some((state?, uid?))
}

fn looks_like_claude(dir: &Path) -> bool {
    match fsx::read_bounded_nofollow(&dir.join("cmdline"), MAX_SMALL) {
        Ok(Some((raw, _))) => {
            let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
            text.contains("claude") || text.contains("amplihack")
        }
        _ => false,
    }
}

/// The profile name `value` points at, if it is a directory under `root`.
/// A path that cannot be resolved falls back to its last component.
fn resolve(dir: &Path, value: &[u8], root: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(value));
    let path: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::fs::read_link(dir.join("cwd")).ok()?.join(path)
    };
    match path.canonicalize() {
        Ok(canonical) => {
            if canonical.parent() != Some(root) {
                return None;
            }
            canonical.file_name()?.to_str().map(str::to_string)
        }
        // fail-safe: unresolvable → treated as live. A wrong name here can
        // only keep a directory counted as live, which blocks a swap.
        Err(_) => path.file_name()?.to_str().map(str::to_string),
    }
}
