//! The one trust rule for guidance folders and files (issue #2535, F-1).
//!
//! A path is trusted when it is not a symbolic link, has the expected type,
//! is owned by this process's effective user and is not writable by group or
//! others. The same rule applies on write (`recipe guide`) and on read (the
//! launcher), always to metadata taken without following a link: `lstat` for
//! folders, the opened handle for files.
//!
//! Missing folders are created one component at a time with a
//! non-recursive `DirBuilder` at mode 0700 and checked again afterwards.
//! Nothing here ever changes the permissions of an existing path, so a
//! symlink planted where a folder is expected is refused rather than
//! followed and chmodded (`create_private_dir_all` follows links, which is
//! why it is never used for these folders).
//!
//! Path-based creation is safe here because every component is checked with
//! `lstat` immediately after it is created or found: a link swapped in by
//! another user would have to live in a folder that user can write, and the
//! parent of every created folder has just passed the "not writable by group
//! or others, owned by me" check.

use super::GuideError;
use std::cell::Cell;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

thread_local! {
    static EUID_OVERRIDE: Cell<Option<u32>> = const { Cell::new(None) };
}

/// This process's effective uid, or the test override.
pub(crate) fn euid() -> u32 {
    EUID_OVERRIDE.with(Cell::get).unwrap_or_else(|| {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    })
}

/// Runs `f` as if this thread's effective uid were `uid`.
#[cfg(test)]
pub(crate) fn with_euid_override<T>(uid: u32, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<u32>);
    impl Drop for Reset {
        fn drop(&mut self) {
            EUID_OVERRIDE.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(EUID_OVERRIDE.with(|c| c.replace(Some(uid))));
    f()
}

/// Why `meta` (taken without following links) fails the trust rule, if it
/// does. `want_dir` selects a folder or a regular file.
pub(crate) fn untrusted_reason(meta: &fs::Metadata, want_dir: bool) -> Option<String> {
    let kind = meta.file_type();
    if kind.is_symlink() {
        return Some("it is a symbolic link".to_string());
    }
    if want_dir && !kind.is_dir() {
        return Some("it is not a folder".to_string());
    }
    if !want_dir && !kind.is_file() {
        return Some("it is not a regular file".to_string());
    }
    let me = euid();
    if meta.uid() != me {
        return Some(format!("it is owned by uid {}, not uid {me}", meta.uid()));
    }
    let mode = meta.mode() & 0o7777;
    if mode & 0o022 != 0 {
        return Some(format!(
            "it is writable by group or others (mode {mode:04o})"
        ));
    }
    None
}

/// What an `lstat` of a folder that must be trusted found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirState {
    /// It exists and passes the trust rule.
    Trusted,
    /// It does not exist.
    Missing,
    /// It exists and fails the rule: the fault alone, with no path and no
    /// word for what happens next.
    Untrusted(String),
}

/// `lstat` of a folder that must be trusted, with no verdict attached.
///
/// The reason in [`DirState::Untrusted`] names the fault and stops there,
/// as the reasons in `guide_records::read_pointer_line` do: a caller that
/// ends the command adds `refusing` ([`check_dir`]), and a caller that
/// passes the folder over adds nothing (the two-word rule, issue #2809).
/// The three callers that pass a folder over -- the log-directory sweep,
/// the launcher reading guidance folders, and the reader of
/// `AMPLIHACK_RUN_GUIDANCE_DIRS` -- name the folder in their own message,
/// so the reason here does not repeat it.
pub(crate) fn dir_state(path: &Path) -> Result<DirState, GuideError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(match untrusted_reason(&meta, true) {
            None => DirState::Trusted,
            Some(reason) => DirState::Untrusted(reason),
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(DirState::Missing),
        Err(e) => Err(GuideError::Io(format!(
            "cannot check {}: {e}",
            path.display()
        ))),
    }
}

/// [`dir_state`] for a caller that stops the command when the folder is not
/// trusted: the path and the word `refusing` are added here. `Ok(false)`
/// when the folder does not exist.
///
/// Every caller is a path the command is about to use and will not go on
/// without -- a folder being created or written to under the chosen
/// destination, or the `runs` folder of the work root the caller's own
/// environment names -- so the verdict is always a refusal. A caller that
/// would pass the folder over must use [`dir_state`] instead, so the
/// message it prints does not call a skipped folder a refusal.
pub(crate) fn check_dir(path: &Path) -> Result<bool, GuideError> {
    match dir_state(path)? {
        DirState::Trusted => Ok(true),
        DirState::Missing => Ok(false),
        DirState::Untrusted(reason) => Err(GuideError::Unsafe(format!(
            "refusing {}: {reason}",
            path.display()
        ))),
    }
}

/// Makes sure `path` is a trusted folder, creating it at mode 0700 when it
/// is missing (with its missing ancestors when `recursive`), then checking it
/// again.
fn ensure_dir(path: &Path, recursive: bool) -> Result<(), GuideError> {
    if check_dir(path)? {
        return Ok(());
    }
    match fs::DirBuilder::new()
        .recursive(recursive)
        .mode(0o700)
        .create(path)
    {
        Ok(()) => {}
        // Created by a concurrent `recipe guide`: check it like any other.
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(GuideError::Io(format!(
                "cannot create {}: {e}",
                path.display()
            )));
        }
    }
    if check_dir(path)? {
        Ok(())
    } else {
        Err(GuideError::Io(format!(
            "{} disappeared after it was created",
            path.display()
        )))
    }
}

/// Makes sure `path` is a trusted folder, creating it (one level, mode 0700)
/// when it is missing. Its parent must already exist.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), GuideError> {
    ensure_dir(path, false)
}

/// Makes sure `base` and each folder below it named by `components` are
/// trusted, creating missing ones at 0700. `base` itself is created with its
/// missing ancestors when absent; ancestors above `base` are not checked.
pub(crate) fn ensure_private_chain(base: &Path, components: &[&str]) -> Result<(), GuideError> {
    ensure_dir(base, true)?;
    let mut path = base.to_path_buf();
    for component in components {
        path.push(component);
        ensure_dir(&path, false)?;
    }
    Ok(())
}
