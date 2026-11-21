//! End-of-run removal of the build output a run created for itself (issue
//! #2886).
//!
//! Two kinds of folder qualify, and nothing else is ever a candidate: the
//! folder named by `CARGO_TARGET_DIR` when the launch claims it as this
//! run's own with `AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR=1`, and `target/` at
//! the root of each linked worktree the run owns or was launched in. The
//! worktree itself is never removed, and a main checkout is never touched.
//!
//! Every candidate passes the same guards before anything is deleted, and
//! anything unexpected is a refusal: one warning line, the folder kept. This
//! runs after the run's outcome is settled, so it returns unit on every path,
//! never panics and never changes the run's result, its exit status or its
//! standard output.

use super::run_dir::{OwnedKind, read_owned_paths};
use super::target_remove::{self, Refusal};
use super::targets::is_cargo_target;
use super::{human_size, sweep};
use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The reason texts shared with [`Refusal`], so one wording covers a target
/// removed by the sweep and one removed at the end of its own run.
pub(crate) const REASON_IN_USE: &str = "in use: .cargo-lock is held";
pub(crate) const REASON_NOT_CARGO: &str = "not a cargo target";
pub(crate) const REASON_FOREIGN_OWNER: &str = "owned by another user";

/// Longest `.git` pointer file read when deciding whether a folder is a
/// linked worktree. A real one is a single short line.
const GIT_FILE_MAX: u64 = 4096;

/// The run that is ending: its own folder, the folder it was launched in and
/// its work root. None of the three may be removed, nor anything holding one.
#[derive(Debug, Clone)]
pub(crate) struct RunEnd {
    pub(crate) run_dir: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) work_root: PathBuf,
}

/// What the launch said about build output, read from this process's own
/// environment at the end of the run.
#[derive(Debug, Clone, Default)]
pub(crate) struct Launch {
    /// `CARGO_TARGET_DIR`, when set and non-empty.
    pub(crate) cargo_target_dir: Option<PathBuf>,
    /// Whether the launch claims that folder as this run's own.
    pub(crate) private_target: bool,
    /// Whether another run is already in progress around this one, so the
    /// inherited target folder and the launch folder belong to that run.
    pub(crate) nested: bool,
}

impl Launch {
    pub(crate) fn from_env() -> Self {
        Self {
            cargo_target_dir: std::env::var_os("CARGO_TARGET_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            private_target: super::private_target_dir_claimed(),
            nested: std::env::var_os("AMPLIHACK_RUN_ID").is_some_and(|value| !value.is_empty()),
        }
    }
}

/// [`end_of_run_with`] on this process's standard error.
///
/// Not `eprintln!`, which panics when standard error is closed: this is the
/// last thing a run does and may be reached while unwinding.
pub(crate) fn end_of_run(end: &RunEnd, launch: &Launch) {
    end_of_run_with(end, launch, &mut std::io::stderr().lock());
}

/// Removes this run's own build output and says what it did on `out`.
pub(crate) fn end_of_run_with(end: &RunEnd, launch: &Launch, out: &mut dyn Write) {
    let ctx = Ctx::new(end);
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut removed: Vec<Option<u64>> = Vec::new();

    // The folder the launch gave this run for itself. A nested run inherited
    // its parent's, and the claim is cleared for steps, so it never applies.
    if launch.private_target
        && !launch.nested
        && let Some(claimed) = &launch.cargo_target_dir
    {
        handle(claimed, &ctx, &mut seen, &mut removed, out);
    }

    // `target/` at the root of every linked worktree this run is responsible
    // for: the ones it recorded, then the one it was launched in.
    let mut worktrees: Vec<PathBuf> = owned_worktrees(&end.run_dir);
    if !launch.nested {
        worktrees.push(end.cwd.clone());
    }
    let mut judged: BTreeSet<PathBuf> = BTreeSet::new();
    for worktree in worktrees {
        if !judged.insert(worktree.clone()) || !is_linked_worktree(&worktree) {
            continue;
        }
        if handle(&worktree.join("target"), &ctx, &mut seen, &mut removed, out) {
            // The worktree stays whatever it holds; #2861's judgement says
            // once when it holds work that exists nowhere else.
            sweep::keep_and_report_unsaved_work_to(&worktree, out);
        }
    }

    if removed.len() > 1 {
        let known = removed
            .iter()
            .flatten()
            .fold(0u64, |total, bytes| total.saturating_add(*bytes));
        let line = total_line(removed.len(), known, removed.iter().any(Option::is_none));
        let _ = out.write_all(line.as_bytes());
    }
}

/// Removes one candidate, reporting what happened. `true` when the folder was
/// there and is now gone.
fn handle(
    candidate: &Path,
    ctx: &Ctx,
    seen: &mut BTreeSet<PathBuf>,
    removed: &mut Vec<Option<u64>>,
    out: &mut dyn Write,
) -> bool {
    let (key, line) = match inspect(candidate, ctx) {
        Verdict::Absent => return false,
        Verdict::Refused(at, reason) => {
            let at = at.unwrap_or_else(|| candidate.to_path_buf());
            let line = warning_line(&at, &reason);
            (at, line)
        }
        Verdict::Approved(approved) => {
            let at = approved.at.clone();
            if seen.contains(&at) {
                return false;
            }
            // The size is read before the delete; a tree too large to walk is
            // removed all the same and reported without one.
            let size = sweep::tree_size(&at);
            match remove(approved) {
                Ok(()) => {
                    seen.insert(at.clone());
                    removed.push(size);
                    let _ = out.write_all(removed_line(&at, size).as_bytes());
                    return true;
                }
                Err(reason) => {
                    let line = warning_line(&at, &reason);
                    (at, line)
                }
            }
        }
    };
    if seen.insert(key) {
        let _ = out.write_all(line.as_bytes());
    }
    false
}

/// What the ending run must never remove, and who it is.
struct Ctx {
    /// The run's own folder, its work root and the folder it was launched in,
    /// resolved as far as they can be.
    protected: Vec<PathBuf>,
    /// `$HOME`, resolved. Read here and not through the user configuration,
    /// which is about files to read; this is only a path to protect.
    home: Option<PathBuf>,
    uid: u32,
}

impl Ctx {
    fn new(end: &RunEnd) -> Self {
        Self {
            protected: [&end.run_dir, &end.work_root, &end.cwd]
                .into_iter()
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| resolved(path))
                .collect(),
            home: std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|value| resolved(Path::new(&value))),
            uid: sweep::euid(),
        }
    }
}

fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A candidate that passed every guard. Holding it holds the `.cargo-lock`
/// files of the folder, so no build can start between the check and the
/// delete.
struct Approved {
    /// Where the folder sits: its canonical parent joined with its name. The
    /// candidate path itself is never followed.
    at: PathBuf,
    /// Its `(device, inode)` when it was approved.
    identity: (u64, u64),
    locks: Vec<target_remove::HeldLock>,
}

enum Verdict {
    /// There is nothing at the path: silence, not a warning.
    Absent,
    /// The folder stays, with the location when it is known.
    Refused(Option<PathBuf>, String),
    Approved(Approved),
}

/// Whether a candidate may be removed. Everything that is not plainly this
/// run's own build output is refused; an error is a refusal too.
fn inspect(candidate: &Path, ctx: &Ctx) -> Verdict {
    let refuse = |reason: &str| Verdict::Refused(None, reason.to_string());
    if !candidate.is_absolute() {
        return refuse("not an absolute path");
    }
    let meta = match fs::symlink_metadata(candidate) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Verdict::Absent,
        Err(error) => return Verdict::Refused(None, format!("cannot examine it: {error}")),
    };
    if meta.file_type().is_symlink() {
        return refuse("a symbolic link; not followed");
    }
    if !meta.file_type().is_dir() {
        return refuse("not a folder");
    }
    let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
        return refuse("it has no parent folder");
    };
    let parent = match fs::canonicalize(parent) {
        Ok(parent) => parent,
        Err(error) => {
            return Verdict::Refused(None, format!("cannot resolve its parent folder: {error}"));
        }
    };
    let at = parent.join(name);
    let refuse = |reason: String| Verdict::Refused(Some(at.clone()), reason);

    if ctx.home.as_ref().is_some_and(|home| home.starts_with(&at)) {
        return refuse("it is the home folder, or holds it".into());
    }
    if ctx.protected.iter().any(|path| path.starts_with(&at)) {
        return refuse("it holds a folder this run is still using".into());
    }
    if fs::symlink_metadata(at.join(".git")).is_ok() {
        return refuse("it is the root of a Git checkout".into());
    }

    let entries = match children(&at, meta.dev()) {
        Ok(entries) => entries,
        Err(error) => return refuse(format!("cannot list it: {error}")),
    };
    if entries.holds_checkout {
        return refuse("it holds a Git checkout".into());
    }
    if meta.uid() != ctx.uid {
        return refuse(REASON_FOREIGN_OWNER.into());
    }
    let parent_meta = match fs::symlink_metadata(&parent) {
        Ok(meta) => meta,
        Err(error) => return refuse(format!("cannot examine its parent folder: {error}")),
    };
    if parent_meta.mode() & 0o002 != 0 && parent_meta.mode() & 0o1000 == 0 {
        return refuse("parent is writable by other users".into());
    }
    if !same_device(meta.dev(), parent_meta.dev()) {
        return refuse("it is a mount point".into());
    }
    if entries.holds_other_device {
        return refuse("it holds a mount point".into());
    }
    // An empty folder holds no build output to recognise and nothing to lose.
    // Anything else must carry positive evidence that Cargo made it.
    if entries.count > 0 && !is_cargo_target(&at) {
        return refuse(REASON_NOT_CARGO.into());
    }
    let locks = match target_remove::lock_cargo_tree(&at) {
        Ok(locks) => locks,
        Err(Refusal::InUse) => return refuse(REASON_IN_USE.into()),
        Err(refusal) => return refuse(refusal.to_string()),
    };
    Verdict::Approved(Approved {
        at,
        identity: (meta.dev(), meta.ino()),
        locks,
    })
}

/// What one level inside a folder holds that would stop it being removed.
struct Children {
    count: usize,
    holds_checkout: bool,
    holds_other_device: bool,
}

/// Reads the entries directly inside `at`, without following any of them.
fn children(at: &Path, device: u64) -> std::io::Result<Children> {
    let mut found = Children {
        count: 0,
        holds_checkout: false,
        holds_other_device: false,
    };
    for entry in fs::read_dir(at)? {
        let entry = entry?;
        found.count += 1;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let child = entry.path();
        if fs::symlink_metadata(child.join(".git")).is_ok() {
            found.holds_checkout = true;
        }
        if !same_device(fs::symlink_metadata(&child)?.dev(), device) {
            found.holds_other_device = true;
        }
    }
    Ok(found)
}

/// Deletes an approved folder, after checking once more that it is the same
/// folder that was approved. The locks are held until the delete is done.
fn remove(approved: Approved) -> Result<(), String> {
    let Approved {
        at,
        identity,
        locks,
    } = approved;
    let meta = fs::symlink_metadata(&at).map_err(|error| format!("cannot examine it: {error}"))?;
    if !meta.file_type().is_dir() || (meta.dev(), meta.ino()) != identity {
        return Err("it changed while it was being checked".to_string());
    }
    // `remove_dir_all` does not follow links inside the tree on the pinned
    // toolchain (CVE-2022-21658).
    let outcome = fs::remove_dir_all(&at).map_err(|error| format!("cannot remove it: {error}"));
    drop(locks);
    outcome?;
    tracing::info!(path = ?at, "removed this run's own build output");
    Ok(())
}

/// The worktrees this run recorded in its `owned-paths`.
///
/// That file lives in the run folder, which steps can write, so nothing in it
/// is trusted: a relative path is already dropped by the parser, and every
/// entry still has to be a linked worktree and pass the whole guard chain.
fn owned_worktrees(run_dir: &Path) -> Vec<PathBuf> {
    read_owned_paths(run_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|owned| owned.kind == OwnedKind::Worktree)
        .map(|owned| owned.path)
        .collect()
}

/// Whether `dir` is a linked worktree: its `.git` is a regular file naming
/// the folder it borrows. A main checkout, whose `.git` is a folder, is never
/// one, so its `target/` is never a candidate.
fn is_linked_worktree(dir: &Path) -> bool {
    let pointer = dir.join(".git");
    let Ok(meta) = fs::symlink_metadata(&pointer) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&pointer)
    else {
        return false;
    };
    let mut head = Vec::new();
    if file.take(GIT_FILE_MAX).read_to_end(&mut head).is_err() {
        return false;
    }
    head.starts_with(b"gitdir:")
}

/// Whether two `(st_dev)` values name the same filesystem.
pub(crate) fn same_device(a: u64, b: u64) -> bool {
    a == b
}

/// `size unknown`, or the size in the units the cleanup report uses.
fn size_text(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "size unknown".to_string(), human_size)
}

/// One removal, with the space it freed.
pub(crate) fn removed_line(path: &Path, bytes: Option<u64>) -> String {
    format!(
        "amplihack: removed this run's build output {path:?} ({})\n",
        size_text(bytes)
    )
}

/// The last line when a run removed more than one folder.
pub(crate) fn total_line(count: usize, known_bytes: u64, any_unknown: bool) -> String {
    let freed = human_size(known_bytes);
    if any_unknown {
        format!(
            "amplihack: end-of-run cleanup removed {count} build output folders, \
             freeing at least {freed} in total (size unknown for one or more of them)\n"
        )
    } else {
        format!(
            "amplihack: end-of-run cleanup removed {count} build output folders, \
             freeing {freed} in total\n"
        )
    }
}

/// One folder that was kept, and why.
///
/// The path is rendered with `{:?}` and the reason is escaped, because both
/// carry names this run did not choose: a newline would forge a second line
/// and an escape sequence would reach a terminal.
pub(crate) fn warning_line(path: &Path, reason: &str) -> String {
    format!(
        "amplihack: warning: end-of-run cleanup could not remove {path:?}: {}\n",
        escape(reason)
    )
}

/// `text` with every control character written as an escape.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_debug());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[path = "tests_build_output.rs"]
mod tests;
