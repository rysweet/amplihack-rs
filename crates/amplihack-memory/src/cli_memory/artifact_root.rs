//! Per-project artifact root resolution (issue #1476).
//!
//! Code-index artifacts — SCIP indexes, `blarify.json`, the code-graph store,
//! the staleness marker and the background-indexing PID file — live in a
//! per-project cache directory **outside** the indexed checkout.
//!
//! The resolution order is `AMPLIHACK_ARTIFACT_DIR`, then `XDG_CACHE_HOME`,
//! then `$HOME/.cache`, then an error. There is deliberately no fall-back to a
//! project-relative or cwd-relative path: writing an 8 MB graph database into
//! whatever repository the process happens to be standing in is the failure
//! this module exists to prevent.
//!
//! See `docs/reference/project-artifact-cache.md`.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Longest human-readable prefix kept in a slug, before the `-<16 hex>` suffix.
const SLUG_HUMAN_MAX: usize = 48;

/// Hex digits of the path hash kept in a slug. 64 bits is plenty for local
/// filesystem paths the user already controls, and a 64-character directory
/// name would make the cache unreadable for the humans who debug it.
const SLUG_HASH_HEX: usize = 16;

/// A pointer file is one line holding a path. Refusing to read more means a
/// corrupt or hostile cache entry cannot be turned into an allocation.
const POINTER_MAX_BYTES: u64 = 4 * 1024;

/// Name of the file inside an artifact dir recording the canonical project path.
const POINTER_FILE: &str = "project";

/// Directory mode for everything amplihack creates under the cache root. The
/// cache holds a searchable index of the user's source code; on a shared host
/// the `0o755` that `create_dir_all` produces under a typical umask would make
/// every indexed project world-readable.
#[cfg(unix)]
const ARTIFACT_DIR_MODE: u32 = 0o700;

/// Outcome of [`ensure_artifact_root`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRootInit {
    /// The resolved artifact directory. Guaranteed to exist on success.
    pub root: PathBuf,
    /// Whether this call created the directory. `false` for a directory that
    /// already existed — amplihack never `chmod`s a directory it did not
    /// create.
    pub created: bool,
}

/// Validate a directory path that came from an environment variable.
///
/// `var_name` is carried so the rejection names the variable the user actually
/// set rather than the subsystem that happened to read it.
pub fn validate_env_dir_path(var_name: &str, path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("{var_name} is empty");
    }
    if !path.is_absolute() {
        bail!("{var_name} must be an absolute path: {}", path.display());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "{var_name} must not contain parent traversal: {}",
            path.display()
        );
    }
    for blocked in ["/proc", "/sys", "/dev"] {
        if path.starts_with(blocked) {
            bail!("{var_name} must not be under {blocked}: {}", path.display());
        }
    }
    Ok(())
}

/// Additional rejections that apply only to `AMPLIHACK_ARTIFACT_DIR`, which is
/// used *directly* as the artifact dir rather than having
/// `amplihack/projects/<slug>/` appended to it. Migration and permission
/// repair operate on this directory, so `/`, `$HOME` and the world-writable
/// temp directories are not acceptable targets.
fn validate_artifact_dir_override(path: &Path) -> Result<()> {
    const VAR: &str = "AMPLIHACK_ARTIFACT_DIR";
    validate_env_dir_path(VAR, path)?;

    if path.parent().is_none() {
        bail!("{VAR} must not be the filesystem root: {}", path.display());
    }
    for dangerous in ["/tmp", "/var/tmp"] {
        if path == Path::new(dangerous) {
            bail!(
                "{VAR} must not be the world-writable {dangerous}: {}",
                path.display()
            );
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && path == home
    {
        bail!("{VAR} must not be the home directory itself: {}", path.display());
    }
    Ok(())
}

/// Stable, collision-resistant directory name for `project_path`.
///
/// `<sanitised basename>-<sha256(canonical path)[..16]>`. The hash carries the
/// uniqueness; the readable prefix exists so a human can tell which cache entry
/// belongs to which checkout.
pub fn project_slug(project_path: &Path) -> String {
    let canonical = canonical_project_path(project_path);

    // Hash the raw OS bytes, not `to_string_lossy()`: lossy conversion maps
    // every invalid UTF-8 byte to the same U+FFFD, so two distinct non-UTF-8
    // paths could hash identically and collide into one cache entry.
    let mut hasher = Sha256::new();
    hasher.update(os_str_bytes(canonical.as_os_str()));
    let digest = hasher.finalize();
    let hash: String = digest
        .iter()
        .take(SLUG_HASH_HEX.div_ceil(2))
        .map(|byte| format!("{byte:02x}"))
        .collect();

    let human = sanitize_slug_component(
        canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    format!("{human}-{}", &hash[..SLUG_HASH_HEX])
}

#[cfg(unix)]
fn os_str_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_str_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

fn sanitize_slug_component(raw: String) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return "project".to_string();
    }
    let mut truncated: String = trimmed.chars().take(SLUG_HUMAN_MAX).collect();
    while truncated.ends_with('-') {
        truncated.pop();
    }
    if truncated.is_empty() {
        "project".to_string()
    } else {
        truncated
    }
}

/// The canonical absolute path used as the project's identity.
///
/// Canonicalisation makes a symlink to a checkout and the checkout itself one
/// cache entry rather than two. A path that cannot be canonicalised (it does
/// not exist yet) falls back to absolutisation, which is still never
/// cwd-relative.
fn canonical_project_path(project_path: &Path) -> PathBuf {
    project_path
        .canonicalize()
        .or_else(|_| std::path::absolute(project_path))
        .unwrap_or_else(|_| project_path.to_path_buf())
}

/// Resolve the artifact directory for `project_path` without creating it.
pub fn project_artifact_root(project_path: &Path) -> Result<PathBuf> {
    if let Some(override_dir) = artifact_dir_override(project_path)? {
        return Ok(override_dir);
    }
    Ok(cache_projects_dir()?.join(project_slug(project_path)))
}

/// Read and validate `AMPLIHACK_ARTIFACT_DIR`.
///
/// Returns `Ok(None)` when the variable is unset, empty, or names a directory
/// whose pointer file records a *different* project — the git-worktree hazard,
/// where a child launched for project B inherits project A's value and would
/// otherwise write B's index into A's cache.
fn artifact_dir_override(project_path: &Path) -> Result<Option<PathBuf>> {
    let Some(raw) = std::env::var_os("AMPLIHACK_ARTIFACT_DIR") else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let candidate = PathBuf::from(raw);
    validate_artifact_dir_override(&candidate)?;

    // A pointer naming another project means this value was inherited, not
    // chosen for us. Ignore it rather than corrupting the other project's
    // cache. The check cannot fire before a pointer exists; see
    // `docs/reference/project-artifact-cache.md#inheritance-hazard`.
    if let Some(recorded) = project_for_artifact_dir(&candidate)? {
        let ours = canonical_project_path(project_path);
        if recorded != ours {
            tracing::warn!(
                "ignoring inherited AMPLIHACK_ARTIFACT_DIR={}: it belongs to {}, not {}",
                candidate.display(),
                recorded.display(),
                ours.display()
            );
            return Ok(None);
        }
    }
    Ok(Some(candidate))
}

/// `<XDG_CACHE_HOME|$HOME/.cache>/amplihack/projects`.
fn cache_projects_dir() -> Result<PathBuf> {
    Ok(cache_base_dir()?.join("amplihack").join("projects"))
}

fn cache_base_dir() -> Result<PathBuf> {
    // An invalid XDG_CACHE_HOME falls through with a warning: the user very
    // likely did not set it for amplihack's benefit, and a usable fallback
    // exists. An invalid AMPLIHACK_ARTIFACT_DIR, which they did set for us, is
    // an error instead.
    if let Some(raw) = std::env::var_os("XDG_CACHE_HOME")
        && !raw.is_empty()
    {
        let candidate = PathBuf::from(raw);
        match validate_env_dir_path("XDG_CACHE_HOME", &candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) => tracing::warn!("ignoring XDG_CACHE_HOME: {err:#}"),
        }
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("cannot locate the artifact cache: neither XDG_CACHE_HOME nor HOME is set")?;
    validate_env_dir_path("HOME", &home)?;
    Ok(home.join(".cache"))
}

/// Resolve the artifact directory and make sure it exists.
pub fn ensure_artifact_root(project_path: &Path) -> Result<ArtifactRootInit> {
    let root = project_artifact_root(project_path)?;
    let created = create_artifact_chain(&root)?;
    write_project_pointer(&root, &canonical_project_path(project_path))?;
    Ok(ArtifactRootInit { root, created })
}

/// Create `root` and the `amplihack/` chain above it at 0o700, repairing loose
/// modes on directories amplihack owns. Returns whether `root` was created.
///
/// `~/.cache` and `$HOME` belong to the user and to other tools; they are
/// created if missing but never re-`chmod`ed.
fn create_artifact_chain(root: &Path) -> Result<bool> {
    let existed = root.exists();

    if let Some(projects) = root.parent()
        && projects.file_name().is_some_and(|name| name == "projects")
        && let Some(amplihack) = projects.parent()
        && amplihack.file_name().is_some_and(|name| name == "amplihack")
    {
        if let Some(base) = amplihack.parent() {
            fs::create_dir_all(base)
                .with_context(|| format!("failed to create {}", base.display()))?;
        }
        for owned in [amplihack, projects] {
            create_private_dir(owned, Repair::Yes)?;
        }
        create_private_dir(root, Repair::Yes)?;
    } else {
        // An `AMPLIHACK_ARTIFACT_DIR` the user pointed us at: create it if it
        // is missing, but never re-`chmod` a directory we did not create.
        if let Some(parent) = root.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        create_private_dir(root, Repair::No)?;
    }

    verify_private_dir(root)?;
    Ok(!existed)
}

/// Whether a pre-existing directory's mode may be tightened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repair {
    /// amplihack owns this directory (`amplihack/` and below).
    Yes,
    /// The user pointed us here; leave their permissions alone.
    No,
}

/// Create `dir` at 0o700 if missing, or repair a loose mode when amplihack owns
/// it. Never widens permissions.
fn create_private_dir(dir: &Path, repair: Repair) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        if !dir.exists() {
            // `mode()` on the builder means the directory is never briefly
            // visible at umask width.
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(ARTIFACT_DIR_MODE);
            builder
                .create(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            return Ok(());
        }

        if repair == Repair::No {
            return Ok(());
        }

        // `create_dir_all` does not re-tighten an existing directory, so a
        // `~/.cache/amplihack` left at 0o755 by an earlier version keeps those
        // bits forever unless we repair it.
        let metadata = fs::symlink_metadata(dir)
            .with_context(|| format!("failed to stat {}", dir.display()))?;
        if metadata.file_type().is_dir() && metadata.permissions().mode() & 0o077 != 0 {
            tracing::info!(
                "tightening permissions on {} from {:o} to {:o}",
                dir.display(),
                metadata.permissions().mode() & 0o777,
                ARTIFACT_DIR_MODE
            );
            fs::set_permissions(dir, fs::Permissions::from_mode(ARTIFACT_DIR_MODE))
                .with_context(|| format!("failed to tighten permissions on {}", dir.display()))?;
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = repair;
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))
    }
}

/// `symlink_metadata`, not `metadata` — the distinction is the whole point. A
/// pre-existing entry that is a symlink is the shape a planted redirect takes.
fn verify_private_dir(dir: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(dir).with_context(|| format!("failed to stat {}", dir.display()))?;
    if !metadata.file_type().is_dir() {
        bail!(
            "artifact directory is not a real directory (symlink or file): {}",
            dir.display()
        );
    }
    Ok(())
}

/// Record the canonical project path inside its artifact dir.
///
/// This is the reverse mapping: `blarify.json` no longer sits two parents below
/// its project, so `project_root_for_blarify_input` reads this file instead of
/// counting `..`.
fn write_project_pointer(root: &Path, canonical_project: &Path) -> Result<()> {
    let pointer = root.join(POINTER_FILE);
    if fs::symlink_metadata(&pointer).is_ok() {
        return Ok(());
    }

    // A predictable temp name in a directory an attacker can reach is the
    // classic symlink race, so the suffix is random and the create is
    // exclusive.
    let temp = root.join(format!(".{POINTER_FILE}.{}.tmp", random_suffix()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        let mut file = options
            .open(&temp)
            .with_context(|| format!("failed to create {}", temp.display()))?;
        writeln!(file, "{}", canonical_project.display())
            .with_context(|| format!("failed to write {}", temp.display()))?;
    }
    if let Err(err) = fs::rename(&temp, &pointer) {
        let _ = fs::remove_file(&temp);
        return Err(err)
            .with_context(|| format!("failed to install pointer file {}", pointer.display()));
    }
    Ok(())
}

fn random_suffix() -> String {
    use std::hash::{BuildHasher, RandomState};
    format!("{:016x}", RandomState::new().hash_one(std::process::id()))
}

/// Recover the project a given artifact directory belongs to.
///
/// `Ok(None)` means the directory has no pointer yet. An unreadable, oversized,
/// non-regular or malformed pointer is an error — it decides a write path, so
/// it is untrusted input, and guessing would put a graph database in the wrong
/// repository.
pub fn project_for_artifact_dir(artifact_dir: &Path) -> Result<Option<PathBuf>> {
    let pointer = artifact_dir.join(POINTER_FILE);
    let metadata = match fs::symlink_metadata(&pointer) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to stat {}", pointer.display()));
        }
    };
    if !metadata.file_type().is_file() {
        bail!(
            "pointer file is not a regular file (symlink or directory): {}",
            pointer.display()
        );
    }
    if metadata.len() > POINTER_MAX_BYTES {
        bail!(
            "pointer file is {} bytes, over the {POINTER_MAX_BYTES} byte limit: {}",
            metadata.len(),
            pointer.display()
        );
    }

    let raw = fs::read_to_string(&pointer)
        .with_context(|| format!("failed to read {}", pointer.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("pointer file is empty: {}", pointer.display());
    }
    if trimmed.contains('\0') {
        bail!("pointer file contains a NUL byte: {}", pointer.display());
    }
    let recorded = PathBuf::from(trimmed);
    validate_env_dir_path("project pointer", &recorded)
        .with_context(|| format!("invalid pointer file {}", pointer.display()))?;
    Ok(Some(recorded))
}

#[cfg(test)]
#[path = "artifact_root_tests.rs"]
mod tests;
