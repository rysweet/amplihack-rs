//! Repository cloning and archive extraction.

use super::types::{REPO_ARCHIVE_URL, REPO_GIT_URL};
use crate::update::{extract_archive, http_get, validate_download_url};
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Fetch the framework repository into `destination`.
///
/// Strategy (matches Python `amplihack install` behaviour):
/// 1. If `git` is found on PATH, run `git clone --depth 1 <url> <dest>`.
///    git writes "Cloning into '...'..." to stderr automatically.
///    If the clone fails the error is propagated immediately (no fallback).
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
///
/// stderr is inherited so git's "Cloning into '...'" message reaches the
/// terminal (parity with Python's `subprocess.check_call(["git", "clone", ...])`).
///
/// On failure, returns `CliExitError` with git's exit code so that the error
/// message is clean (no extra Rust diagnostic on stderr — parity with Python
/// which only prints to stdout on failure).
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
        // Normalize to exit code 1 for parity with Python:
        //   Python: CalledProcessError → print(f"Failed to install: {e}") to stdout + return 1
        //   Rust:   CliExitError(1)    → std::process::exit(1), no extra stderr message
        // Python always maps any git clone failure to exit 1 regardless of git's
        // actual exit code.  Rust must match this behaviour.
        return Err(crate::command_error::exit_error(1));
    }
    Ok(())
}

/// BFS search for the framework repo root.
///
/// Accepts directories containing either `.claude/` (Python repo layout) or
/// `amplifier-bundle/` (Rust repo layout) as markers. This allows the install
/// to work with both the legacy Python repo and the current Rust repo.
pub(super) fn find_framework_repo_root(root: &Path) -> Result<PathBuf> {
    let markers = [".claude", "amplifier-bundle"];
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        if markers.iter().any(|m| dir.join(m).is_dir()) {
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
        "downloaded framework archive did not contain a repository root \
         with .claude/ or amplifier-bundle/ under {}",
        root.display()
    )
}
