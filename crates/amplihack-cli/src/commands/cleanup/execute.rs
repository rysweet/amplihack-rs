//! Apply-time re-checks and removal (issue #1808, S1, S2). Every removal
//! resolves the recorded path again, requires it to match the canonical path
//! seen at collection and to lie in the managed roots, repeats the liveness,
//! Cargo-lock and cleanliness checks, then acts on the canonical path only.
//! Moved out of `sweep.rs`. Run targets (issue #2039) are removed only
//! through `target_remove`, which inspects the path again itself.

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
        // `target_remove::remove` inspects the path again and requires the
        // planned shape; this is the containment half of that check.
        Op::RemoveTarget { .. } => target_remove::location(&item.path)
            .is_ok_and(|at| is_contained(OwnedKind::Target, &at, roots, None)),
        Op::ReclaimExec { .. } => c.parent() == Some(roots.exec.as_path()),
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
///
/// After a worktree is removed, the target of that checkout is handed to the
/// same removal routine (issue #2039, rule c); its entries go to `follow_on`.
pub(super) fn execute(
    item: &Item,
    settings: &Settings,
    roots: &ManagedRoots,
    links: &LinkRoots,
    procs: &ProcScan,
    follow_on: &mut Vec<Entry>,
) -> Result<()> {
    match &item.op {
        Op::Skip | Op::Fail => return Ok(()),
        Op::Archive { relative } => {
            return archive_file(&item.path, &settings.archive_dir, relative).map(|_| ());
        }
        Op::RemoveTarget {
            run_dir,
            shape,
            after_checkout_gone,
        } => {
            return remove_run_target(
                item,
                run_dir,
                shape,
                after_checkout_gone.as_deref(),
                settings,
                roots,
                links,
                procs,
            );
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
        Op::Skip | Op::Fail | Op::Archive { .. } | Op::RemoveTarget { .. } => Ok(()),
        Op::ReclaimExec { tombstone } => {
            if procs.in_use(target) {
                refuse!("in use by a running process");
            }
            if *tombstone {
                return exec_storage::remove_tombstone(target, None);
            }
            match exec_storage::reclaim(target, run_has_ended)? {
                exec_storage::Verdict::Reclaimable(_) => Ok(()),
                exec_storage::Verdict::Absent => refuse!("{} is already gone", target.display()),
                exec_storage::Verdict::Kept(reason) => refuse!("{reason}"),
            }
        }
        Op::RemoveRunTmp { run_dir } => {
            run_still_inactive(run_dir)?;
            // A child of a killed runner (an agent, cargo) may outlive it and
            // still use its TMPDIR.
            if procs.in_use(target) {
                refuse!("in use by a running process");
            }
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
            if item.category == Category::TmpLeftover && has_live_pid_marker(target) {
                refuse!("in use: a pid marker names a running process");
            }
            if procs.in_use(target) {
                refuse!("in use by a running process");
            }
            // Issue #2039: a Cargo target is locked exclusively from the
            // check through the delete, so a build cannot start in between.
            // Dropping the handles before the delete reopens that race.
            let _held = if *cargo_lock {
                match target_remove::lock_cargo_tree(target) {
                    Ok(held) => held,
                    Err(refusal) => refuse!("{refusal}"),
                }
            } else {
                Vec::new()
            };
            remove_real_dir(target)
        }
        Op::RemoveWorktree {
            main,
            run_dir,
            rule_a_target,
        } => {
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
            if let Some(reason) = unpushed_reason(target) {
                refuse!("{reason}");
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
            if !rule_a_target {
                follow_on.extend(follow_on_target(item, settings, roots, links));
            }
            Ok(())
        }
    }
}

/// Rule (c) of issue #2039: the target of the worktree `item` removed just
/// now. The caller skips this when the sweep planned a rule (a) removal of
/// that target as its own item; a `created` record alone is not enough,
/// because under `--only run-worktree`, or when the rule (a) item was refused
/// at plan time, no such item exists.
fn follow_on_target(
    item: &Item,
    settings: &Settings,
    roots: &ManagedRoots,
    links: &LinkRoots,
) -> Vec<Entry> {
    // A fresh process scan: the worktree was in use a moment ago.
    let mut procs = None;
    worktree_target_entries(
        item.canonical.as_path(),
        settings,
        roots,
        links,
        &mut procs,
        item.run_id.as_deref(),
    )
}

/// Applies `Op::RemoveTarget`: the run is still inactive, no other live run
/// records the target, for rule (a) the checkout is gone now, no process
/// uses it, and then `target_remove::remove` with its own re-inspection and
/// `.cargo-lock` locks.
#[allow(clippy::too_many_arguments)]
fn remove_run_target(
    item: &Item,
    run_dir: &Path,
    shape: &Shape,
    after_checkout_gone: Option<&Path>,
    settings: &Settings,
    roots: &ManagedRoots,
    links: &LinkRoots,
    procs: &ProcScan,
) -> Result<()> {
    if !is_managed(item, roots) {
        refuse!(
            "{} is no longer inside the folders cleanup manages",
            item.path.display()
        );
    }
    // A link that resolved when planned must resolve to the same place now;
    // `remove` repeats this as part of its shape check.
    if !matches!(shape, Shape::DanglingLink) {
        match fs::canonicalize(&item.path) {
            Ok(now) if now == item.canonical => {}
            Ok(now) => refuse!(
                "{} now resolves to {} instead of {}",
                item.path.display(),
                now.display(),
                item.canonical.display()
            ),
            Err(_) => refuse!("{}", target_remove::Refusal::ChangedSincePlan),
        }
    }
    run_still_inactive(run_dir)?;
    refuse_if_other_live_owner(item, settings)?;
    if let Some(checkout) = after_checkout_gone {
        match fs::symlink_metadata(checkout) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => refuse!("worktree kept"),
        }
    }
    let in_use_at = match shape {
        Shape::DanglingLink => None,
        _ => Some(item.canonical.as_path()),
    };
    if in_use_at.is_some_and(|p| procs.in_use(p)) {
        refuse!("in use by a running process");
    }
    match target_remove::remove(&item.path, shape, roots, links, true) {
        Ok(()) => {
            // (a) the run removed this worktree; (b) the checkout was gone.
            let event = if after_checkout_gone.is_some() {
                "a"
            } else {
                "b"
            };
            tracing::info!(
                path = %item.path.display(),
                event,
                reason = %item.reason,
                run_id = item.run_id.as_deref().unwrap_or(""),
                "removed pre-commit target"
            );
            Ok(())
        }
        Err(target_remove::RemoveError::Refused(refusal)) => refuse!("{refusal}"),
        Err(target_remove::RemoveError::Io(error)) => {
            Err(error).with_context(|| format!("cannot remove {}", item.path.display()))
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
    let other = other_live_owner(&runs, &item.canonical, own)
        .or_else(|| other_live_owner(&runs, &item.path, own));
    if let Some(reason) = other {
        refuse!("{reason}");
    }
    Ok(())
}
