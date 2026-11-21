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

use super::run_dir::{OWNED_PATHS, OwnedKind, read_owned_paths};
use super::target_remove::{self, Refusal};
use super::targets::is_cargo_target;
use super::{human_size, sweep};
use crate::user_config::safe::display_safe;
use amplihack_types::is_git_checkout;
use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The reason texts shared with [`Refusal`], so one wording covers a target
/// removed by the sweep and one removed at the end of its own run.
const REASON_IN_USE: &str = "in use: .cargo-lock is held";
const REASON_NOT_CARGO: &str = "not a cargo target";
const REASON_FOREIGN_OWNER: &str = "owned by another user";

/// What a linked worktree's `.git` file begins with. Only these bytes are
/// read: the rest of that line names the borrowed folder, which this does not
/// need to know.
const GITDIR_PREFIX: [u8; 7] = *b"gitdir:";

/// The run that is ending: its own folder, the folder it was launched in and
/// its work root. None of the three may be removed, nor anything holding one.
#[derive(Debug)]
pub(super) struct RunEnd {
    pub(super) run_dir: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) work_root: PathBuf,
}

/// What the launch said about build output, read from this process's own
/// environment at the end of the run.
#[derive(Debug)]
pub(super) struct Launch {
    /// `CARGO_TARGET_DIR`, when set and non-empty.
    cargo_target_dir: Option<PathBuf>,
    /// Whether the launch claims that folder as this run's own.
    private_target: bool,
    /// Whether another run is already in progress around this one, so the
    /// inherited target folder and the launch folder belong to that run.
    nested: bool,
}

impl Launch {
    pub(super) fn from_env() -> Self {
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
pub(super) fn end_of_run(end: &RunEnd, launch: &Launch) {
    end_of_run_with(end, launch, &mut std::io::stderr().lock());
}

/// [`end_of_run_in`] with the context read from this process's environment.
fn end_of_run_with(end: &RunEnd, launch: &Launch, out: &mut dyn Write) {
    end_of_run_in(end, launch, &Ctx::new(end), out);
}

/// Removes this run's own build output and says what it did on `out`.
fn end_of_run_in(end: &RunEnd, launch: &Launch, ctx: &Ctx, out: &mut dyn Write) {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut removed: Vec<Option<u64>> = Vec::new();

    // The folder the launch gave this run for itself. A nested run inherited
    // its parent's, and the claim is cleared for steps, so it never applies.
    if launch.private_target
        && !launch.nested
        && let Some(claimed) = &launch.cargo_target_dir
    {
        if ctx.home.is_none() {
            // The claim is the launcher's word alone, and the home guard is
            // the only check standing between it and a home folder. Without
            // it the claim is refused rather than quietly trusted. A worktree
            // `target/` needs no such rule: it is reached through a git
            // worktree check, not through a path the launcher chose.
            report(
                claimed.clone(),
                warning_line(
                    claimed,
                    "the home folder is unknown, so it cannot be protected",
                ),
                &mut seen,
                out,
            );
        } else {
            handle(claimed, ctx, &mut seen, &mut removed, out);
        }
    }

    // `target/` at the root of every linked worktree this run is responsible
    // for: the ones it recorded, then the one it was launched in. The two
    // stay apart, because only a recorded entry that is not a worktree is
    // worth a line; a launch folder that is a main checkout is the ordinary
    // case.
    let mut worktrees: Vec<(PathBuf, bool)> = owned_worktrees(&end.run_dir, out)
        .into_iter()
        .map(|path| (path, true))
        .collect();
    if !launch.nested {
        worktrees.push((end.cwd.clone(), false));
    }
    let mut judged: BTreeSet<PathBuf> = BTreeSet::new();
    for (worktree, recorded) in worktrees {
        if !judged.insert(worktree.clone()) {
            continue;
        }
        if !is_linked_worktree(&worktree) {
            // Silent when the path is not there at all: a worktree removed
            // earlier in the run is not a fault, which is the same silence
            // `Verdict::Absent` keeps.
            if recorded && fs::symlink_metadata(&worktree).is_ok() {
                let line = skipped_line(&worktree, "not a linked worktree");
                report(worktree, line, &mut seen, out);
            }
            continue;
        }
        if handle(&worktree.join("target"), ctx, &mut seen, &mut removed, out) {
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
    report(key, line, seen, out);
    false
}

/// Writes one line about a folder, at most once for that folder however many
/// times it was named.
fn report(key: PathBuf, line: String, seen: &mut BTreeSet<PathBuf>, out: &mut dyn Write) {
    if seen.insert(key) {
        let _ = out.write_all(line.as_bytes());
    }
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
        Self::with_home(
            end,
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|value| resolved(Path::new(&value))),
        )
    }

    /// [`Ctx::new`] with the home folder given rather than read, so the
    /// unresolved case can be exercised without writing to the environment.
    fn with_home(end: &RunEnd, home: Option<PathBuf>) -> Self {
        Self {
            protected: [&end.run_dir, &end.work_root, &end.cwd]
                .into_iter()
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| resolved(path))
                .collect(),
            home,
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
    /// The folder stays: where it is, and why it was not removed.
    Refused(PathBuf, String),
    Approved(Approved),
}

/// Whether a candidate may be removed. Everything that is not plainly this
/// run's own build output is refused; an error is a refusal too.
fn inspect(candidate: &Path, ctx: &Ctx) -> Verdict {
    let refuse = |reason: &str| Verdict::Refused(candidate.to_path_buf(), reason.to_string());
    if !candidate.is_absolute() {
        return refuse("not an absolute path");
    }
    let meta = match fs::symlink_metadata(candidate) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Verdict::Absent,
        Err(error) => return refuse(&format!("cannot examine it: {error}")),
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
        Err(error) => return refuse(&format!("cannot resolve its parent folder: {error}")),
    };
    let at = parent.join(name);
    let refuse_at = |reason: String| Verdict::Refused(at.clone(), reason);

    if ctx.home.as_ref().is_some_and(|home| home.starts_with(&at)) {
        return refuse_at("it is the home folder, or holds it".into());
    }
    if ctx.protected.iter().any(|path| path.starts_with(&at)) {
        return refuse_at("it holds a folder this run is still using".into());
    }
    if is_git_checkout(&at) {
        return refuse_at("it is the root of a Git checkout".into());
    }

    let entries = match children(&at, meta.dev()) {
        Ok(entries) => entries,
        Err(error) => return refuse_at(format!("cannot list it: {error}")),
    };
    if entries.holds_checkout {
        return refuse_at("it holds a Git checkout".into());
    }
    if meta.uid() != ctx.uid {
        return refuse_at(REASON_FOREIGN_OWNER.into());
    }
    let parent_meta = match fs::symlink_metadata(&parent) {
        Ok(meta) => meta,
        Err(error) => return refuse_at(format!("cannot examine its parent folder: {error}")),
    };
    if parent_meta.mode() & 0o002 != 0 && parent_meta.mode() & 0o1000 == 0 {
        return refuse_at("parent is writable by other users".into());
    }
    if !same_device(meta.dev(), parent_meta.dev()) {
        return refuse_at("it is a mount point".into());
    }
    if entries.holds_other_device {
        return refuse_at("it holds a mount point".into());
    }
    // An empty folder holds no build output to recognise and nothing to lose.
    // Anything else must carry positive evidence that Cargo made it.
    if entries.count > 0 && !is_cargo_target(&at) {
        return refuse_at(REASON_NOT_CARGO.into());
    }
    let locks = match target_remove::lock_cargo_tree(&at) {
        Ok(locks) => locks,
        Err(Refusal::InUse) => return refuse_at(REASON_IN_USE.into()),
        Err(refusal) => return refuse_at(refusal.to_string()),
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
        if is_git_checkout(&child) {
            found.holds_checkout = true;
        }
        if !same_device(entry.metadata()?.dev(), device) {
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
///
/// A missing file is an empty list and says nothing. A file that is there and
/// cannot be read is reported, because it leaves every worktree this run owns
/// unexamined and nothing else would show that.
fn owned_worktrees(run_dir: &Path, out: &mut dyn Write) -> Vec<PathBuf> {
    match read_owned_paths(run_dir) {
        Ok(owned) => owned
            .into_iter()
            .filter(|owned| owned.kind == OwnedKind::Worktree)
            .map(|owned| owned.path)
            .collect(),
        Err(error) => {
            let line = skipped_line(&run_dir.join(OWNED_PATHS), &error.to_string());
            let _ = out.write_all(line.as_bytes());
            Vec::new()
        }
    }
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
    let Ok(mut file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&pointer)
    else {
        return false;
    };
    let mut head = [0u8; GITDIR_PREFIX.len()];
    file.read_exact(&mut head).is_ok() && head == GITDIR_PREFIX
}

/// Whether two `(st_dev)` values name the same filesystem.
fn same_device(a: u64, b: u64) -> bool {
    a == b
}

/// One removal, with the space it freed, or `size unknown` for a tree too
/// large to have been walked.
fn removed_line(path: &Path, bytes: Option<u64>) -> String {
    format!(
        "amplihack: removed this run's build output {path:?} ({})\n",
        bytes.map_or_else(|| "size unknown".to_string(), human_size)
    )
}

/// The last line when a run removed more than one folder.
fn total_line(count: usize, known_bytes: u64, any_unknown: bool) -> String {
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
/// The path is rendered with `{:?}` and the reason through [`display_safe`],
/// because both carry names this run did not choose: a newline would forge a
/// second line and an escape sequence would reach a terminal.
fn warning_line(path: &Path, reason: &str) -> String {
    format!(
        "amplihack: warning: end-of-run cleanup could not remove {path:?}: {}\n",
        display_safe(reason)
    )
}

/// One thing that was not examined, and why. Separate from [`warning_line`]
/// because nothing was attempted on the path: it names a file that could not
/// be read, or a recorded folder that was never a candidate.
fn skipped_line(path: &Path, reason: &str) -> String {
    format!(
        "amplihack: warning: end-of-run cleanup skipped {path:?}: {}\n",
        display_safe(reason)
    )
}

#[cfg(test)]
#[path = "tests_build_output.rs"]
mod tests;
