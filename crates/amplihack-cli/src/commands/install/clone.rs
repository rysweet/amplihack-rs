//! Framework source resolution: bundled-first, with network fallback.
//!
//! As of issue #254 the framework assets are bundled inside the amplihack-rs
//! source tree (`amplifier-bundle/`) and no longer fetched from the upstream
//! `rysweet/amplihack` repository at install time.
//!
//! Resolution order:
//! 1. **Compile-time workspace root** — the `CARGO_MANIFEST_DIR` embedded at
//!    build time points two levels up to the workspace root that contains
//!    `amplifier-bundle/` and (if present) a `.claude/` directory.
//! 2. **`AMPLIHACK_HOME`** — user-configured override.
//! 3. **Walk-up from executable** — walks parent directories looking for
//!    `amplifier-bundle/`.
//! 4. **`~/.amplihack`** — staged install location from a prior run.
//! 5. **Network download** (legacy fallback) — `git clone` / tarball from
//!    upstream, only attempted when none of the above yields a usable root.

use super::types::{REPO_ARCHIVE_URL, REPO_GIT_URL};
use crate::update::{extract_archive, http_get, validate_download_url};
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Locate the bundled framework source from the amplihack-rs source tree.
///
/// Returns the repo root (the directory that contains `amplifier-bundle/`
/// and — for a complete source checkout — `.claude/`) without any network
/// access.  Returns `None` when the source tree is not reachable (e.g. the
/// binary was installed via `cargo install` and the original checkout was
/// deleted).
pub(super) fn find_bundled_framework_root() -> Option<PathBuf> {
    // 1. Compile-time workspace root
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    if let Some(ref root) = workspace_root
        && root.join("amplifier-bundle").is_dir()
    {
        return Some(root.clone());
    }

    // 2. AMPLIHACK_HOME env var
    if let Ok(home) = std::env::var("AMPLIHACK_HOME") {
        let p = PathBuf::from(&home);
        if p.join("amplifier-bundle").is_dir() {
            return Some(p);
        }
    }

    // 3. Walk up from executable
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if d.join("amplifier-bundle").is_dir() {
                return Some(d);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }

    // 4. ~/.amplihack (from prior staged install)
    if let Ok(home) = std::env::var("HOME") {
        let dot = PathBuf::from(home).join(".amplihack");
        if dot.join("amplifier-bundle").is_dir() && dot.join(".claude").is_dir() {
            return Some(dot);
        }
    }

    None
}

/// Fetch the framework repository into `destination`.
///
/// **Deprecated path** — only reached when `find_bundled_framework_root()`
/// returns `None` (source tree unavailable).
///
/// Strategy (matches Python `amplihack install` behaviour):
/// 1. If `git` is found on PATH, run `git clone --depth 1 <url> <dest>`.
/// 2. If `git` is NOT on PATH, fall back to HTTP tarball download.
pub(super) fn download_and_extract_framework_repo(destination: &Path) -> Result<PathBuf> {
    if let Ok(git_path) = which_git() {
        git_clone_framework_repo(&git_path, destination)?;
        return find_framework_repo_root(destination);
    }

    // git not available — fall back to HTTP tarball download
    validate_download_url(REPO_ARCHIVE_URL)?;
    let archive_bytes = http_get(REPO_ARCHIVE_URL)
        .with_context(|| format!("failed to download framework archive from {REPO_ARCHIVE_URL}"))?;
    extract_archive(&archive_bytes, destination).with_context(|| {
        format!(
            "failed to extract framework archive into {}",
            destination.display()
        )
    })?;
    find_framework_repo_root(destination)
}

/// Resolve the `git` binary path from PATH.
fn which_git() -> Result<PathBuf> {
    let output = std::process::Command::new("which")
        .arg("git")
        .output()
        .or_else(|_| {
            // `which` may not be available on all platforms; fall back to `command -v git`
            std::process::Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
        })
        .context("failed to locate git binary")?;
    if output.status.success() {
        let path_str = std::str::from_utf8(&output.stdout)
            .context("git path is not valid UTF-8")?
            .trim()
            .to_string();
        if path_str.is_empty() {
            bail!("git not found on PATH");
        }
        Ok(PathBuf::from(path_str))
    } else {
        bail!("git not found on PATH")
    }
}

/// Run `git clone --depth 1 <REPO_GIT_URL> <destination>`.
fn git_clone_framework_repo(git_path: &Path, destination: &Path) -> Result<()> {
    let status = std::process::Command::new(git_path)
        .args([
            "clone",
            "--depth",
            "1",
            REPO_GIT_URL,
            &destination.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to spawn git clone for {REPO_GIT_URL}"))?;
    if !status.success() {
        return Err(crate::command_error::exit_error(1));
    }
    Ok(())
}

pub(super) fn find_framework_repo_root(root: &Path) -> Result<PathBuf> {
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        // Accept either `.claude/` (Python repo layout) or
        // `amplifier-bundle/` (Rust repo layout) as a repo root marker
        // (fix #254).
        if dir.join(".claude").is_dir() || dir.join("amplifier-bundle").is_dir() {
            return Ok(dir);
        }
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry.with_context(|| format!("failed to inspect {}", dir.display()))?;
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
            }
        }
    }

    bail!(
        "downloaded framework archive did not contain a repository root with .claude or amplifier-bundle under {}",
        root.display()
    )
}
