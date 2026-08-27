//! Shared staging for the amplihack slash-command markdown files.
//!
//! Exactly one command set — `docs/claude/commands/amplihack/*.md` — feeds two
//! independent surfaces:
//!
//! * the Copilot CLI plugin, at `<plugin_dir>/commands/` (flat, with the
//!   `amplihack:` namespace stripped from each file's frontmatter `name:`
//!   because Copilot flattens every plugin's commands into one namespace and
//!   rejects the colon), and
//! * Claude Code, at `~/.claude/commands/amplihack/` (verbatim; Claude derives
//!   `/amplihack:<name>` from the directory plus the file stem).
//!
//! Only the target directory and the per-file transform differ, so the source
//! probing and the atomic staging-then-swap live here once. Issue #1344 was
//! precisely the cost of having only the Copilot half: the commands existed in
//! the repo, were documented, and never arrived in a Claude session.
//!
//! Two properties of the Claude target make it different from the Copilot one
//! and shape everything below:
//!
//! 1. **The parent is a scan root.** Every subdirectory of `~/.claude/commands/`
//!    is a command namespace, so a leftover `amplihack.staging/` would surface
//!    23 phantom `/amplihack.staging:*` commands. The scratch directories
//!    therefore live in a caller-supplied `scratch_root` that Claude does not
//!    scan, and are cleaned on every exit path (including `?`, via
//!    [`ScratchGuard`]) and recovered on the next run after a crash.
//! 2. **The directory is user-reachable.** A user may legitimately keep their
//!    own `~/.claude/commands/amplihack/my-thing.md`. Unless the caller can
//!    prove the whole directory is still amplihack's (`target_is_owned`,
//!    derived from the recorded content digest), anything the new command set
//!    does not itself provide is carried across the swap and reported.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// One staging request: where the commands come from, where they go, and what
/// the installer is allowed to destroy on the way.
pub(super) struct StageRequest<'a> {
    /// Directory holding the `*.md` command sources.
    pub(super) source: &'a Path,
    /// Directory to publish the transformed command set into.
    pub(super) target: &'a Path,
    /// Directory to create the `.staging`/`.old` scratch dirs in. Must be on
    /// the same filesystem as `target`, and must NOT be a directory the host
    /// tool scans (see the module docs).
    pub(super) scratch_root: &'a Path,
    /// `true` only when the caller has verified that everything currently in
    /// `target` was put there by amplihack. `false` makes the swap preserve
    /// entries the new command set does not provide instead of deleting them.
    pub(super) target_is_owned: bool,
}

/// Outcome of a staging run.
#[derive(Debug)]
pub(super) struct StagedCommands {
    /// Number of `*.md` files written into `target`.
    pub(super) copied: usize,
    /// Entries that were already in `target`, are not part of the command set,
    /// and were carried across the swap rather than deleted.
    pub(super) preserved: Vec<String>,
}

/// Locate the slash-command markdown source directory inside `repo_root`.
///
/// In the bundle layout the canonical command markdowns live at
/// `<repo>/docs/claude/commands/amplihack/`; in legacy `.claude` checkouts
/// they live at `<repo>/.claude/commands/amplihack/` (or one parent up, for
/// nested checkouts). All three are probed in that order; the first existing
/// directory wins. Returns `None` when the source tree ships no commands.
pub(super) fn command_source_dir(repo_root: &Path) -> Option<PathBuf> {
    command_source_dir_excluding(repo_root, None)
}

/// [`command_source_dir`], skipping any candidate that resolves to `exclude`.
///
/// `find_bundled_framework_root` resolves `repo_root` to `~/.amplihack` on any
/// host with a prior staged install, and the third probe is
/// `<repo_root>/../.claude/commands/amplihack` — which is then literally
/// `$HOME/.claude/commands/amplihack`, the Claude staging target. Staging a
/// directory from itself reports a green command count that can never refresh,
/// so the caller passes its target here and the probe falls through to
/// "no commands shipped" instead.
pub(super) fn command_source_dir_excluding(
    repo_root: &Path,
    exclude: Option<&Path>,
) -> Option<PathBuf> {
    [
        repo_root
            .join("docs")
            .join("claude")
            .join("commands")
            .join("amplihack"),
        repo_root.join(".claude").join("commands").join("amplihack"),
        repo_root
            .parent()
            .map(|parent| parent.join(".claude").join("commands").join("amplihack"))
            .unwrap_or_default(),
    ]
    .into_iter()
    .find(|candidate| {
        candidate.is_dir() && !exclude.is_some_and(|excluded| same_path(candidate, excluded))
    })
}

/// Copy every `*.md` file in `request.source` into `request.target`, passing
/// each file's body through `transform` first, and swap the result into place
/// atomically.
///
/// Files land in a `<scratch_root>/<name>.staging` directory and are only
/// renamed over `target` once every file has been written, so a reader never
/// observes a half-populated command directory and a failed copy leaves the
/// previously staged command set intact. An existing `target` is moved aside to
/// `<scratch_root>/<name>.old` for the duration of the swap and restored if the
/// swap fails.
///
/// Non-`.md` entries and subdirectories of `source` are ignored. `copied == 0`
/// means nothing was staged and `target` was left exactly as it was.
pub(super) fn stage_command_files(
    request: &StageRequest<'_>,
    transform: impl Fn(&Path, &str) -> Result<String>,
) -> Result<StagedCommands> {
    let StageRequest {
        source,
        target,
        scratch_root,
        target_is_owned,
    } = *request;

    if same_path(source, target) {
        bail!(
            "refusing to stage commands from {} into itself; the install source \
             was resolved to the staging target, so no command could ever refresh",
            source.display()
        );
    }

    fs::create_dir_all(scratch_root)
        .with_context(|| format!("failed to create {}", scratch_root.display()))?;
    // The scratch dirs used to be siblings of `target`, so creating them
    // created `target`'s parent as a side effect. They are not any more, and
    // both the recovery below and the final rename need that parent to exist.
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let backup = scratch_path(scratch_root, target, ".old");
    recover_interrupted_swap(target, &backup)?;

    let staging = ScratchGuard::create(scratch_path(scratch_root, target, ".staging"))?;
    let copied = copy_markdown(source, staging.path(), transform)?;
    if copied == 0 {
        return Ok(StagedCommands {
            copied: 0,
            preserved: Vec::new(),
        });
    }

    let preserved = if target_is_owned {
        Vec::new()
    } else {
        carry_over_unrecognized(target, staging.path())?
    };
    swap_into_place(staging.path(), target, &backup)?;
    Ok(StagedCommands { copied, preserved })
}

/// Write `source`'s transformed `*.md` files into `staging`.
fn copy_markdown(
    source: &Path,
    staging: &Path,
    transform: impl Fn(&Path, &str) -> Result<String>,
) -> Result<usize> {
    let mut copied = 0_usize;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|ext| ext.to_str()) != Some("md")
        {
            continue;
        }
        let dst = staging.join(entry.file_name());
        let body = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read {}", entry.path().display()))?;
        let transformed = transform(&entry.path(), &body)?;
        fs::write(&dst, transformed).with_context(|| {
            format!(
                "failed to write command {} to {}",
                entry.path().display(),
                dst.display()
            )
        })?;
        copied += 1;
    }
    Ok(copied)
}

/// Publish `staging` as `target`, moving any existing `target` aside first.
///
/// By the time this runs, `staging` already holds everything that must survive
/// (see [`carry_over_unrecognized`]), so deleting the backup afterwards can no
/// longer lose anything.
fn swap_into_place(staging: &Path, target: &Path, backup: &Path) -> Result<()> {
    if !target.exists() {
        return fs::rename(staging, target)
            .with_context(|| format!("failed to move commands into {}", target.display()));
    }

    fs::rename(target, backup).with_context(|| {
        format!(
            "failed to back up existing {} to {}",
            target.display(),
            backup.display()
        )
    })?;
    if let Err(error) = fs::rename(staging, target) {
        let _ = fs::rename(backup, target);
        return Err(error)
            .with_context(|| format!("failed to swap commands into {}", target.display()));
    }
    let _ = fs::remove_dir_all(backup);
    Ok(())
}

/// Copy every entry of the existing `target` that the new command set does not
/// itself provide into `staging`, so the swap carries it across.
///
/// Copy, and *before* the swap, rather than moving entries out of the backup
/// afterwards: the target is not touched at all until the atomic rename, so a
/// failure anywhere here leaves the user's directory exactly as it was and
/// there is no window in which the only copy of their file lives in a scratch
/// directory that is about to be deleted. That deletion — the old
/// `let _ = fs::remove_dir_all(&backup)` — is what destroyed a user's own
/// `~/.claude/commands/amplihack/my-thing.md` under a green success line.
fn carry_over_unrecognized(target: &Path, staging: &Path) -> Result<Vec<String>> {
    if !target.is_dir() {
        return Ok(Vec::new());
    }
    let mut preserved = Vec::new();
    for entry in
        fs::read_dir(target).with_context(|| format!("failed to read {}", target.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let destination = staging.join(&name);
        if destination.exists() {
            continue;
        }
        copy_entry(&entry.path(), &destination)?;
        preserved.push(name.to_string_lossy().into_owned());
    }
    preserved.sort();
    Ok(preserved)
}

/// Copy one filesystem entry, preserving files, directories and symlinks.
fn copy_entry(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let link = fs::read_link(source)
            .with_context(|| format!("failed to read symlink {}", source.display()))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link, destination).with_context(|| {
            format!(
                "failed to preserve symlink {} -> {}",
                destination.display(),
                link.display()
            )
        })?;
        #[cfg(not(unix))]
        let _ = link;
        return Ok(());
    }
    if kind.is_dir() {
        fs::create_dir_all(destination)
            .with_context(|| format!("failed to create {}", destination.display()))?;
        for entry in
            fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
        {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if kind.is_file() {
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed to preserve {} as {}",
                source.display(),
                destination.display()
            )
        })?;
    }
    // Sockets, FIFOs and device nodes have no business in a command namespace.
    Ok(())
}

/// Reconcile scratch state left behind by an interrupted previous run.
///
/// A crash between the two renames of [`swap_into_place`] leaves no `target`
/// and the user's previous directory sitting in `backup`. Move it back so the
/// normal path can preserve anything unrecognized inside it. A stale backup
/// beside an existing `target` is simply removed, because leaving it means it
/// is never cleaned at all — the old "target missing" branch skipped it
/// entirely, so `.old` became permanent and, being inside Claude's scan root,
/// surfaced every command a second time.
///
/// A backup that cannot be restored is an error, never a deletion: at that
/// moment it holds the only copy of the user's command directory.
fn recover_interrupted_swap(target: &Path, backup: &Path) -> Result<()> {
    if !backup.exists() {
        return Ok(());
    }
    if target.exists() {
        let _ = fs::remove_dir_all(backup);
        return Ok(());
    }
    fs::rename(backup, target).with_context(|| {
        format!(
            "failed to restore {} from the interrupted install backup at {}",
            target.display(),
            backup.display()
        )
    })?;
    println!(
        "  ♻️  Recovered {} from an interrupted previous install",
        target.display()
    );
    Ok(())
}

/// A scratch directory that removes itself on drop, whatever the exit path.
///
/// Every `?` in the copy loop used to return without cleaning `.staging`, and
/// the orphan then surfaced as a phantom command namespace. The guard is held
/// across the swap too: once `.staging` has been renamed onto the target the
/// drop simply finds nothing there.
struct ScratchGuard(PathBuf);

impl ScratchGuard {
    fn create(path: PathBuf) -> Result<Self> {
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch_path(scratch_root: &Path, target: &Path, suffix: &str) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("commands"));
    name.push(suffix);
    scratch_root.join(name)
}

/// Whether two paths resolve to the same location on disk.
///
/// Falls back to a lexical comparison when either side does not exist yet —
/// `canonicalize` fails on a not-yet-created staging target.
fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests;
