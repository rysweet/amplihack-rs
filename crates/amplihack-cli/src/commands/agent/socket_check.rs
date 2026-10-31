//! The checks made before every tmux call that passes `-S` (S12). A tmux
//! client sends its whole environment to whatever listens at the socket
//! path, so the socket's directory must be this user's own and closed to
//! others, every directory above it must be safe from replacement, and an
//! existing socket file must be this user's socket. Missing directories are
//! created one level at a time at mode 0700 (D10).

use amplihack_utils::secure_files::{ensure_private_dir, euid, writable_by_others};
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

/// The reason given whenever a check fails.
pub const NOT_PRIVATE: &str = "socket directory is not private";
/// The refusal of a socket a command was asked to use.
pub const REFUSED: &str =
    "tmux socket: its directory must be owned by you and closed to other users";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketCheck {
    /// The directory and its ancestors pass; the socket file passes or does
    /// not exist.
    Ok,
    /// The socket's directory does not exist.
    MissingParent,
    NotPrivate,
}

/// S12 steps 1 to 3 for `socket`, read with `lstat`.
pub fn check(socket: &Path) -> SocketCheck {
    let Some(parent) = socket
        .parent()
        .filter(|p| socket.is_absolute() && !p.as_os_str().is_empty())
    else {
        return SocketCheck::NotPrivate;
    };
    let meta = match fs::symlink_metadata(parent) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return SocketCheck::MissingParent,
        Err(_) => return SocketCheck::NotPrivate,
    };
    let private = meta.is_dir()
        && meta.uid() == euid()
        && meta.mode() & 0o007 == 0
        && !writable_by_others(&meta);
    let above = parent.parent().is_some_and(ancestors_ok);
    if private && above && socket_ok(socket) {
        SocketCheck::Ok
    } else {
        SocketCheck::NotPrivate
    }
}

/// Makes `socket`'s directory ready for `new-session`: the deepest existing
/// ancestor must pass step 2, each missing directory is created at 0700 one
/// level at a time, and then [`check`] must pass.
pub fn prepare(socket: &Path) -> Result<(), String> {
    let parent = socket.parent().ok_or(NOT_PRIVATE)?;
    let mut missing = Vec::new();
    let mut cur = parent;
    loop {
        match fs::symlink_metadata(cur) {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                missing.push(cur);
                cur = cur.parent().ok_or(NOT_PRIVATE)?;
            }
            Err(_) => return Err(NOT_PRIVATE.into()),
        }
    }
    if !missing.is_empty() {
        if !ancestors_ok(cur) {
            return Err(NOT_PRIVATE.into());
        }
        for dir in missing.iter().rev() {
            ensure_private_dir(dir)
                .map_err(|e| format!("cannot create the socket directory: {e}"))?;
        }
    }
    match check(socket) {
        SocketCheck::Ok => Ok(()),
        SocketCheck::MissingParent | SocketCheck::NotPrivate => Err(NOT_PRIVATE.into()),
    }
}

/// S12 step 2 for `dir` and everything above it: each directory is owned by
/// this user or root and, if others can write to it, has the sticky bit. A
/// symlink on the way must be owned by this user or root; the directories
/// it leads to are checked through the canonical path.
fn ancestors_ok(dir: &Path) -> bool {
    let Ok(real) = dir.canonicalize() else {
        return false;
    };
    let links_ok = dir.ancestors().all(|a| {
        fs::symlink_metadata(a).is_ok_and(|m| !m.file_type().is_symlink() || owner_ok(&m))
    });
    links_ok
        && real.ancestors().all(|a| {
            fs::symlink_metadata(a).is_ok_and(|m| {
                m.is_dir() && owner_ok(&m) && (!writable_by_others(&m) || m.mode() & 0o1000 != 0)
            })
        })
}

fn owner_ok(m: &fs::Metadata) -> bool {
    m.uid() == euid() || m.uid() == 0
}

/// S12 step 3: an existing socket path must be a socket owned by this user,
/// not a symlink.
fn socket_ok(socket: &Path) -> bool {
    match fs::symlink_metadata(socket) {
        Ok(m) => m.file_type().is_socket() && m.uid() == euid(),
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}
