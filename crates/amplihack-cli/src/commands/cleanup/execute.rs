//! Apply-time re-checks and removal (issue #1808, S1, S2). Every removal
//! resolves the recorded path again, requires it to match the canonical path
//! seen at collection and to lie in the managed roots, repeats the liveness,
//! Cargo-lock and cleanliness checks, then acts on the canonical path only.
//! Moved out of `sweep.rs` unchanged.

use super::*;

/// A safety re-check at apply time said no. Reported as `skipped`, unlike a
/// removal that was attempted and failed.
#[derive(Debug)]
pub(super) struct Refusal(String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

macro_rules! refuse {
    ($($arg:tt)*) => {
        return Err(anyhow::Error::new(Refusal(format!($($arg)*))))
    };
}

/// Whether an item's canonical path is still where its operation may act.
fn is_managed(item: &Item, roots: &ManagedRoots) -> bool {
    let c = item.canonical.as_path();
    match &item.op {
        Op::Skip | Op::Fail | Op::Archive { .. } => true,
        Op::RemoveRunTmp { .. } => item.run_id.as_deref().is_some_and(|id| {
            c == roots.runs.join(id).join("runtime") || c == roots.short_runtime.join(short_id(id))
        }),
        Op::RemoveWorktree { main, .. } => is_contained(OwnedKind::Worktree, c, roots, Some(main)),
        Op::RemoveDir { .. } if item.category == Category::TmpLeftover => {
            c.parent() == Some(roots.tmp.as_path()) && has_prefix(c, TMP_PREFIXES)
        }
        Op::RemoveDir { .. } => is_contained(OwnedKind::Target, c, roots, None),
    }
}

/// Removes the folder at the canonical `path`, refusing a symlink or a
/// non-folder there.
fn remove_real_dir(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("{} is gone", path.display()))?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        refuse!("{} is no longer a real folder", path.display());
    }
    fs::remove_dir_all(path).with_context(|| format!("cannot remove {}", path.display()))
}

/// `Ok` only when the run's `run.json` can still be read and the run is not
/// active. Anything else means skip.
fn run_still_inactive(run_dir: &Path) -> Result<()> {
    let report = match RunReport::read(run_dir) {
        Ok(report) => report,
        Err(error) => refuse!("run.json can no longer be read: {error:#}"),
    };
    if run_is_active(&report) {
        refuse!("the run became active");
    }
    Ok(())
}

/// Repeats the safety checks on the item immediately before acting, then
/// acts on the canonical path recorded at collection, never on the
/// step-supplied one.
pub(super) fn execute(
    item: &Item,
    settings: &Settings,
    roots: &ManagedRoots,
    procs: &ProcScan,
) -> Result<()> {
    match &item.op {
        Op::Skip | Op::Fail => return Ok(()),
        Op::Archive { relative } => {
            return archive_file(&item.path, &settings.archive_dir, relative).map(|_| ());
        }
        _ => {}
    }
    let now = match fs::canonicalize(&item.path) {
        Ok(now) => now,
        Err(error) => refuse!("{} cannot be resolved: {error}", item.path.display()),
    };
    if now != item.canonical {
        refuse!(
            "{} now resolves to {} instead of {}",
            item.path.display(),
            now.display(),
            item.canonical.display()
        );
    }
    if !is_managed(item, roots) {
        refuse!(
            "{} is no longer inside the folders cleanup manages",
            item.canonical.display()
        );
    }
    let target = item.canonical.as_path();
    match &item.op {
        Op::Skip | Op::Fail | Op::Archive { .. } => Ok(()),
        Op::RemoveRunTmp { run_dir } => {
            run_still_inactive(run_dir)?;
            remove_real_dir(target)
        }
        Op::RemoveDir {
            run_dir,
            cargo_lock,
        } => {
            if let Some(run_dir) = run_dir {
                run_still_inactive(run_dir)?;
                refuse_if_other_live_owner(item, settings)?;
            }
            if *cargo_lock && cargo_lock_held(target) {
                refuse!("in use: .cargo-lock is held");
            }
            if procs.in_use(target) {
                refuse!("in use by a running process");
            }
            remove_real_dir(target)
        }
        Op::RemoveWorktree { main, run_dir } => {
            run_still_inactive(run_dir)?;
            refuse_if_other_live_owner(item, settings)?;
            if !is_real_dir(target) {
                refuse!("{} is no longer a real folder", target.display());
            }
            let now_main = git_common_dir(target).and_then(|c| main_checkout(&c));
            if now_main.as_deref() != Some(main.as_path()) {
                refuse!(
                    "{} no longer belongs to {}",
                    target.display(),
                    main.display()
                );
            }
            if git_has_dirty_state(target) {
                refuse!("has uncommitted changes");
            }
            if git_has_unpushed_commits(target) {
                refuse!("has unpushed commits");
            }
            if procs.in_use(target) {
                refuse!("in use by a running process");
            }
            let mut command = hardened_git();
            command
                .arg("-C")
                .arg(main)
                .args(["worktree", "remove", "--"])
                .arg(target);
            let output = crate::util::run_output_with_timeout(command, GIT_TIMEOUT)?;
            if !output.status.success() {
                bail!(
                    "git worktree remove failed: {}",
                    sanitized_stderr_short(&output.stderr)
                );
            }
            Ok(())
        }
    }
}

/// Why `canonical` must be kept because a run other than `except` records it
/// and is active, or has a `run.json` that cannot be read. A reused worktree
/// is recorded by every run that used it, so the path goes only once all of
/// them are inactive.
pub(super) fn other_live_owner(runs: &[RunInfo], canonical: &Path, except: &str) -> Option<String> {
    runs.iter()
        .filter(|r| r.id != except)
        .filter(|r| {
            r.owned()
                .iter()
                .any(|o| o.path == canonical || canonical_or_self(&o.path) == canonical)
        })
        .find_map(|r| match &r.report {
            Ok(report) if run_is_active(report) => {
                Some(format!("also recorded by active run {}", r.id))
            }
            Ok(_) => None,
            Err(_) => Some(format!(
                "also recorded by run {}, whose run.json cannot be read",
                r.id
            )),
        })
}

/// Apply-time form of [`other_live_owner`]: scans the runs folder again.
fn refuse_if_other_live_owner(item: &Item, settings: &Settings) -> Result<()> {
    let own = item.run_id.as_deref().unwrap_or_default();
    let runs = match scan_runs(&settings.work_root) {
        Ok(runs) => runs,
        Err(error) => refuse!("cannot list runs: {error:#}"),
    };
    if let Some(reason) = other_live_owner(&runs, &item.canonical, own) {
        refuse!("{reason}");
    }
    Ok(())
}
