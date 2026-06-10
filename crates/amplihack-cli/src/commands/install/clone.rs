//! Framework source resolution: bundled-first, with network fallback.
//!
//! As of issue #254 the framework assets are bundled inside the amplihack-rs
//! source tree (`amplifier-bundle/`) and no longer fetched from the upstream
//! `rysweet/amplihack` repository at install time.
//!
//! Resolution order (fix #341 — prefer the user's actual checkout over any
//! baked-at-build-time path):
//! 1. **`AMPLIHACK_HOME`** — explicit user-configured override (highest
//!    priority).
//! 2. **CWD walk-up** — walks parent directories of the current working
//!    directory looking for `amplifier-bundle/`. This is the path that
//!    correctly identifies "the checkout the user is installing from" when
//!    they invoke `amplihack install` from inside a clone, even if the
//!    binary was built elsewhere.
//! 3. **Walk-up from executable** — walks parent directories of
//!    `current_exe()` looking for `amplifier-bundle/` (in-tree dev binary
//!    under `target/`).
//! 4. **Compile-time workspace root** — the `CARGO_MANIFEST_DIR` embedded at
//!    build time points two levels up to the workspace root that contains
//!    `amplifier-bundle/`. Only meaningful for `cargo run`-style invocations
//!    from the workspace itself; demoted because for an installed binary it
//!    pins the bundle to whatever was on disk at compile time (issue #341).
//! 5. **`~/.amplihack`** — staged install location from a prior run.
//! 6. **Network download** (legacy fallback) — `git clone` / tarball from
//!    upstream, only attempted when none of the above yields a usable root.

use super::bundle_compat::validate_framework_bundle_compatibility;
use super::types::{REPO_ARCHIVE_URL, REPO_GIT_URL};
use crate::update::{extract_archive, http_get_with_retry, validate_download_url};
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(not(test))]
const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(300);
#[cfg(test)]
const GIT_CLONE_TIMEOUT: Duration = Duration::from_millis(250);
const GIT_CLONE_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CAPTURE_LIMIT: usize = 8192;

/// Locate the bundled framework source from the amplihack-rs source tree.
///
/// Returns the repo root (the directory that contains `amplifier-bundle/`
/// and — for a complete source checkout — `.claude/`) without any network
/// access.  Returns `None` when the source tree is not reachable (e.g. the
/// binary was installed via `cargo install` and the original checkout was
/// deleted).
pub(super) fn find_bundled_framework_root() -> Option<PathBuf> {
    // 1. AMPLIHACK_HOME env var — explicit user override
    if let Ok(home) = std::env::var("AMPLIHACK_HOME") {
        let p = PathBuf::from(&home);
        if let Some(root) = compatible_candidate(p, "AMPLIHACK_HOME") {
            return Some(root);
        }
    }

    // 2. CWD walk-up — the checkout the user is installing from (fix #341)
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir: Option<PathBuf> = Some(cwd);
        while let Some(d) = dir {
            if let Some(root) = compatible_candidate(d.clone(), "current directory") {
                return Some(root);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }

    // 3. Walk up from executable (in-tree dev binary under `target/`)
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if let Some(root) = compatible_candidate(d.clone(), "executable parent") {
                return Some(root);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }

    // 4. Compile-time workspace root (only meaningful for `cargo run` from
    //    the workspace; demoted because for installed binaries it pins the
    //    bundle to whatever was on disk at build time — issue #341).
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    if let Some(ref root) = workspace_root
        && root.join("amplifier-bundle").is_dir()
        && let Some(root) = compatible_candidate(root.clone(), "compile-time workspace root")
    {
        return Some(root);
    }

    // 5. ~/.amplihack (from prior staged install)
    if let Ok(home) = std::env::var("HOME") {
        let dot = PathBuf::from(home).join(".amplihack");
        if dot.join(".claude").is_dir()
            && let Some(root) = compatible_candidate(dot, "~/.amplihack")
        {
            return Some(root);
        }
    }

    None
}

fn compatible_candidate(candidate: PathBuf, label: &str) -> Option<PathBuf> {
    if !candidate.join("amplifier-bundle").is_dir() {
        return None;
    }
    match validate_framework_bundle_compatibility(&candidate) {
        Ok(()) => Some(candidate),
        Err(err) => {
            eprintln!(
                "⚠️  Skipping incompatible framework bundle from {label}: {}: {err:#}",
                candidate.display()
            );
            None
        }
    }
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
        return find_compatible_framework_repo_root(destination, REPO_GIT_URL);
    }

    // git not available — fall back to HTTP tarball download
    validate_download_url(REPO_ARCHIVE_URL)?;
    let archive_bytes = http_get_with_retry(REPO_ARCHIVE_URL)
        .with_context(|| format!("failed to download framework archive from {REPO_ARCHIVE_URL}"))?;
    extract_archive(&archive_bytes, destination).with_context(|| {
        format!(
            "failed to extract framework archive into {}",
            destination.display()
        )
    })?;
    find_compatible_framework_repo_root(destination, REPO_ARCHIVE_URL)
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
    let stdout_file = tempfile::NamedTempFile::new()
        .context("failed to create temporary stdout file for git clone")?;
    let stderr_file = tempfile::NamedTempFile::new()
        .context("failed to create temporary stderr file for git clone")?;
    let mut command = std::process::Command::new(git_path);
    command
        .args([
            "clone",
            "--depth",
            "1",
            REPO_GIT_URL,
            &destination.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout_file
                .as_file()
                .try_clone()
                .context("failed to clone git stdout handle")?,
        ))
        .stderr(Stdio::from(
            stderr_file
                .as_file()
                .try_clone()
                .context("failed to clone git stderr handle")?,
        ));
    #[cfg(unix)]
    // SAFETY: `pre_exec` runs after fork and before exec. `setsid` is async-signal-safe
    // and isolates the git clone process tree so timeout cleanup can terminate it.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn git clone for {REPO_GIT_URL}"))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to poll git clone status")?
        {
            break status;
        }
        if started.elapsed() >= GIT_CLONE_TIMEOUT {
            terminate_git_clone(&mut child);
            let stdout = read_limited(stdout_file.path())?;
            let stderr = read_limited(stderr_file.path())?;
            bail!(
                "git clone timed out after {:?} for {REPO_GIT_URL} into {}\nstdout:\n{}\nstderr:\n{}",
                GIT_CLONE_TIMEOUT,
                destination.display(),
                stdout,
                stderr
            );
        }
        thread::sleep(GIT_CLONE_POLL_INTERVAL);
    };
    if !status.success() {
        let stdout = read_limited(stdout_file.path())?;
        let stderr = read_limited(stderr_file.path())?;
        bail!(
            "git clone failed with status {status} for {REPO_GIT_URL} into {}\nstdout:\n{}\nstderr:\n{}",
            destination.display(),
            stdout,
            stderr
        );
    }
    Ok(())
}

fn terminate_git_clone(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if pid > 0 {
            // Negative pid targets the process group created with setsid above.
            unsafe {
                let _ = libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn read_limited(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open captured output {}", path.display()))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((CAPTURE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read captured output {}", path.display()))?;
    let truncated = bytes.len() > CAPTURE_LIMIT;
    bytes.truncate(CAPTURE_LIMIT);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push_str("\n...<truncated>");
    }
    Ok(text)
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
            if entry
                .file_type()
                .with_context(|| format!("failed to inspect {}", entry.path().display()))?
                .is_dir()
            {
                queue.push_back(entry.path());
            }
        }
    }

    bail!(
        "downloaded framework archive did not contain a repository root with .claude or amplifier-bundle under {}",
        root.display()
    )
}

pub(super) fn find_compatible_framework_repo_root(root: &Path, source: &str) -> Result<PathBuf> {
    let repo_root = find_framework_repo_root(root)?;
    if let Err(error) = validate_framework_bundle_compatibility(&repo_root) {
        return Err(anyhow::anyhow!(
            "downloaded framework bundle from {source} is incompatible at {}: {error}",
            repo_root.display()
        ));
    }

    Ok(repo_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake_git(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("git");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn git_clone_reports_nonzero_exit_with_captured_output() {
        let temp = tempfile::tempdir().unwrap();
        let fake = fake_git(
            temp.path(),
            "echo stdout-marker; echo stderr-marker >&2; exit 42",
        );

        let err = git_clone_framework_repo(&fake, &temp.path().join("dest"))
            .expect_err("non-zero git clone must fail");
        let msg = format!("{err:#}");

        assert!(
            msg.contains("status"),
            "error must include exit status: {msg}"
        );
        assert!(
            msg.contains("stdout-marker") && msg.contains("stderr-marker"),
            "error must include captured stdout/stderr: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_clone_times_out_and_reaps_child() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("orphan-marker");
        let fake = fake_git(
            temp.path(),
            &format!(
                "(/bin/sleep 1; echo orphan > '{}') & echo started; /bin/sleep 5",
                marker.display()
            ),
        );
        let start = Instant::now();

        let err = git_clone_framework_repo(&fake, &temp.path().join("dest"))
            .expect_err("hung git clone must time out");
        let msg = format!("{err:#}");

        assert!(
            start.elapsed() < Duration::from_secs(2),
            "test timeout should be bounded, elapsed {:?}",
            start.elapsed()
        );
        assert!(msg.contains("timed out"), "error must name timeout: {msg}");
        assert!(
            msg.contains("started"),
            "timeout error must include captured output: {msg}"
        );
        thread::sleep(Duration::from_millis(1_200));
        assert!(
            !marker.exists(),
            "timeout cleanup must terminate git clone descendants"
        );
    }
}
