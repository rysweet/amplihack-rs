//! The preconditions and the per-name state table of the shared session
//! store (issue #1634), Unix only.

use super::link::{Swap, shell_quote, swap_in_link, symlink_at};
use super::merge::{MergeError, merge_tree, stat_at};
use super::{Conflict, Deferred, Migrated, Mode, Report, SHARED_SESSION_DIRS};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::fdwalk;
use std::ffi::OsStr;
use std::fs;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY;

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// `mode` evaluated once, on first use.
struct LazyMode<F> {
    f: Option<F>,
    v: Option<Mode>,
}

impl<F: FnOnce() -> Mode> LazyMode<F> {
    fn get(&mut self) -> Mode {
        if let Some(v) = self.v {
            return v;
        }
        let v = match self.f.take() {
            Some(f) => f(),
            None => Mode::LinkOnly(Deferred::LiveSession),
        };
        self.v = Some(v);
        v
    }
}

/// The two sides every name is checked against.
struct Sides<'a> {
    profile: &'a ProfileName,
    profile_dir: &'a Path,
    dir: OwnedFd,
    /// As given (messages); `canon` is canonicalized (link targets).
    shown_root: &'a Path,
    canon: PathBuf,
    canon_fd: OwnedFd,
}

pub(super) fn ensure(
    profile_dir: &Path,
    canonical_root: &Path,
    profile: &ProfileName,
    mode: impl FnOnce() -> Mode,
    r: &mut Report,
) {
    let Some((canon, canon_fd)) = check_canonical_root(canonical_root, r) else {
        return;
    };
    let Some(dir) = check_profile_dir(profile_dir, &canon, profile, r) else {
        return;
    };
    let sides = Sides {
        profile,
        profile_dir,
        dir,
        shown_root: canonical_root,
        canon,
        canon_fd,
    };
    let mut mode = LazyMode {
        f: Some(mode),
        v: None,
    };
    for name in SHARED_SESSION_DIRS {
        one_name(&sides, name, &mut mode, r);
    }
}

fn check_canonical_root(root: &Path, r: &mut Report) -> Option<(PathBuf, OwnedFd)> {
    let refused = |r: &mut Report| {
        r.warn(format!(
            "{} is a symlink or not a directory; sessions not shared",
            root.display()
        ));
    };
    let meta = match fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("{} does not exist; sessions not shared", root.display());
            return None;
        }
        Err(e) => {
            r.warn(format!("{}: {e}; sessions not shared", root.display()));
            return None;
        }
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        refused(r);
        return None;
    }
    if meta.uid() != euid() || meta.mode() & 0o022 != 0 {
        r.warn(format!(
            "{} is not owned by you or is writable by group or others; sessions not shared",
            root.display()
        ));
        return None;
    }
    let opened = root.canonicalize().and_then(|canon| {
        fdwalk::openat(libc::AT_FDCWD, canon.as_os_str(), DIR_FLAGS, 0).map(|fd| (canon, fd))
    });
    match opened {
        Ok(pair) => Some(pair),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            refused(r);
            None
        }
        Err(e) => {
            r.warn(format!("{}: {e}; sessions not shared", root.display()));
            None
        }
    }
}

fn check_profile_dir(
    dir: &Path,
    canon: &Path,
    profile: &ProfileName,
    r: &mut Report,
) -> Option<OwnedFd> {
    let unsafe_dir = |r: &mut Report| {
        r.warn(
            "profile directory is not owned by you or has group or other permissions; sessions not shared"
                .to_string(),
        );
    };
    let meta = match fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) => {
            r.warn(format!("{}: {e}; sessions not shared", dir.display()));
            return None;
        }
    };
    if meta.file_type().is_symlink()
        || !meta.is_dir()
        || meta.uid() != euid()
        || meta.mode() & 0o077 != 0
    {
        unsafe_dir(r);
        return None;
    }
    let real = match dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            r.warn(format!("{}: {e}; sessions not shared", dir.display()));
            return None;
        }
    };
    // Path::starts_with compares whole components, so `~/.claude-profiles`
    // is not inside `~/.claude`.
    if real.starts_with(canon) {
        tracing::debug!("profile {profile}: inside the shared store; never linked");
        return None;
    }
    match fdwalk::openat(libc::AT_FDCWD, dir.as_os_str(), DIR_FLAGS, 0) {
        Ok(fd) => Some(fd),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            unsafe_dir(r);
            None
        }
        Err(e) => {
            r.warn(format!("{}: {e}; sessions not shared", dir.display()));
            None
        }
    }
}

/// `<canonical root>/<name>`, created `0700` when missing. `None` (with a
/// warning) when it is a symlink, not a directory, or cannot be made.
fn canonical_subdir(s: &Sides, name: &'static str, r: &mut Report) -> Option<OwnedFd> {
    let n = OsStr::new(name);
    let c = fdwalk::cstr(n).ok()?;
    // SAFETY: a valid directory descriptor and NUL-terminated name.
    if unsafe { libc::mkdirat(s.canon_fd.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EEXIST) {
            r.warn(format!("{name}: mkdir failed: {e}; nothing removed"));
            return None;
        }
    }
    match fdwalk::openat(s.canon_fd.as_raw_fd(), n, DIR_FLAGS, 0) {
        Ok(fd) => Some(fd),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            r.warn(format!(
                "{} is a symlink or not a directory; {name} not shared",
                s.shown_root.join(name).display()
            ));
            None
        }
        Err(e) => {
            r.warn(format!("{name}: open failed: {e}; nothing removed"));
            None
        }
    }
}

fn one_name<F: FnOnce() -> Mode>(
    s: &Sides,
    name: &'static str,
    mode: &mut LazyMode<F>,
    r: &mut Report,
) {
    let Some(shared_fd) = canonical_subdir(s, name, r) else {
        return;
    };
    let target = s.canon.join(name);
    let at = s.profile_dir.join(name);
    let st = match stat_at(s.dir.as_raw_fd(), OsStr::new(name)) {
        Ok(st) => st,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
            match symlink_at(&target, &s.dir, OsStr::new(name)) {
                Ok(()) => r.linked.push(name),
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => r.defer(name, Deferred::Raced),
                Err(e) => r.warn(format!("{name}: link failed: {e}; nothing removed")),
            }
            return;
        }
        Err(e) => {
            r.warn(format!("{name}: open failed: {e}; nothing removed"));
            return;
        }
    };
    match st.st_mode & libc::S_IFMT {
        libc::S_IFLNK => {
            if at.canonicalize().is_ok_and(|p| p == target) {
                r.already_ok.push(name);
            } else {
                let shown = fs::read_link(&at)
                    .map_or_else(|e| format!("({e})"), |t| t.display().to_string());
                r.warn(format!("{name} is a symlink to {shown}; left alone"));
            }
        }
        libc::S_IFDIR => match mode.get() {
            Mode::Migrate => migrate(s, name, &shared_fd, r),
            Mode::LinkOnly(why) => r.defer(name, why),
        },
        _ => r.warn(format!("{name} is not a directory or a link; left alone")),
    }
}

fn migrate(s: &Sides, name: &'static str, shared_fd: &OwnedFd, r: &mut Report) {
    let target = s.canon.join(name);
    let shown = s.shown_root.join(name);
    let src = match fdwalk::openat(s.dir.as_raw_fd(), OsStr::new(name), DIR_FLAGS, 0) {
        Ok(fd) => fd,
        Err(e) => {
            r.warn(format!("{name}: open failed: {e}; nothing removed"));
            return;
        }
    };
    let outcome = match merge_tree(&src, shared_fd, &target, s.profile) {
        Ok(o) => o,
        Err(MergeError::Io { step, err }) => {
            r.warn(format!("{name}: {step} failed: {err}; nothing removed"));
            return;
        }
        Err(MergeError::NoSpace { path, bytes }) => {
            r.warn(format!(
                "{name}: not enough free space to copy {} ({bytes} bytes) to {}; merge stopped, nothing removed",
                path.display(),
                shown.display()
            ));
            return;
        }
    };
    drop(src);
    let real = s.profile_dir.join(name);
    let backup = match swap_in_link(&s.dir, s.profile_dir, name, &target) {
        Swap::Done(backup) => Some(backup),
        Swap::RolledBack(e) => {
            r.warn(format!(
                "{name}: could not replace the directory with a link ({e}); left as it was"
            ));
            None
        }
        Swap::Stranded { err, backup } => {
            // Real absolute paths, so the printed command works as is.
            r.warn_with(
                format!(
                    "{name}: could not restore the old directory ({err}); run: mv {} {}",
                    shell_quote(&backup),
                    shell_quote(&real)
                ),
                false,
            );
            None
        }
        Swap::Leftover { err, at } => {
            r.warn(format!(
                "{name}: rename failed: {err}; nothing removed, the old directory is at {}",
                at.display()
            ));
            None
        }
    };
    let left_in = backup.clone().unwrap_or(real);
    for (path, saved_as) in &outcome.conflicts {
        r.note(format!(
            "{name}: conflict: {} kept the shared copy; this profile's copy saved as {}",
            path.display(),
            saved_as.display()
        ));
        r.conflicts.push(Conflict {
            name,
            path: path.clone(),
            saved_as: saved_as.clone(),
        });
    }
    for path in &outcome.logins {
        r.warn(format!(
            "{name}: skipped credential file {}; left in {}",
            path.display(),
            left_in.display()
        ));
    }
    if outcome.skipped > 0 {
        r.note(format!(
            "{name}: skipped {} symbolic links or special files; left in {}",
            outcome.skipped,
            left_in.display()
        ));
    }
    r.note(format!(
        "{name}: moved {} files into {}, {} identical, {} conflict",
        outcome.moved,
        shown.display(),
        outcome.identical,
        outcome.conflicts.len()
    ));
    if let Some(backup) = backup {
        r.note(format!(
            "{name}: old directory kept as {}",
            backup.display()
        ));
        r.migrated.push(Migrated {
            name,
            moved: outcome.moved,
            identical: outcome.identical,
            conflicts: outcome.conflicts.len(),
            backup,
        });
    }
}
