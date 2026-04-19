use super::*;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::process::Command;
use std::time::Duration;
use tar::Archive;

/// Verify a downloaded archive against its SHA-256 checksum.
pub(super) fn verify_sha256(archive_bytes: &[u8], checksum_url: &str) -> Result<()> {
    let checksum_body = super::network::http_get_with_retry(checksum_url)
        .with_context(|| format!("failed to download checksum from {checksum_url}"))?;
    let checksum_text =
        std::str::from_utf8(&checksum_body).context("checksum file is not valid UTF-8")?;

    let expected_hex = checksum_text
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| anyhow!("checksum file is empty or malformed: {checksum_url}"))?;

    if expected_hex.len() != 64 || !expected_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "checksum file does not contain a valid SHA-256 hex digest (got {:?}): {checksum_url}",
            expected_hex
        );
    }

    let mut hasher = Sha256::new();
    hasher.update(archive_bytes);
    let actual_bytes = hasher.finalize();
    let actual_hex = format!("{actual_bytes:x}");

    if !actual_hex.eq_ignore_ascii_case(expected_hex) {
        bail!(
            "SHA-256 checksum mismatch for downloaded archive:\n  expected: {expected_hex}\n  actual:   {actual_hex}\nAborted to prevent installing a corrupt or tampered binary."
        );
    }

    tracing::debug!(
        checksum_url,
        sha256 = actual_hex,
        "archive checksum verified"
    );
    Ok(())
}

pub(super) fn download_and_replace(release: &UpdateRelease) -> Result<()> {
    let archive_bytes = super::network::http_get_with_retry(&release.asset_url)?;

    if let Some(checksum_url) = &release.checksum_url {
        verify_sha256(&archive_bytes, checksum_url)
            .context("binary download checksum verification failed")?;
        println!("SHA-256 checksum verified.");
    } else {
        tracing::warn!(
            "no checksum file found for release {}; skipping SHA-256 verification",
            release.version
        );
    }
    let temp_dir = tempfile::tempdir().context("failed to create update temp directory")?;
    extract_archive(&archive_bytes, temp_dir.path())?;

    let new_amplihack = find_binary(temp_dir.path(), binary_filename("amplihack"))?;
    let new_hooks = find_binary(temp_dir.path(), binary_filename("amplihack-hooks"))?;
    let current_exe =
        std::env::current_exe().context("cannot determine current executable path")?;
    let install_dir = current_exe
        .parent()
        .context("current executable has no parent directory")?;
    let hooks_dest = install_dir.join(binary_filename("amplihack-hooks"));

    // Preflight: verify the destination filesystem has enough space for the
    // new binaries. Without this, a low-disk host can crash `fs::copy` with a
    // partial write that leaves a corrupt `.tmp` file behind — the user sees
    // "Updated amplihack" but the rename actually failed (or wrote a
    // truncated binary), and every subsequent launch re-prompts for update.
    let new_hooks_size = fs::metadata(&new_hooks)
        .with_context(|| format!("stat {}", new_hooks.display()))?
        .len();
    let new_amplihack_size = fs::metadata(&new_amplihack)
        .with_context(|| format!("stat {}", new_amplihack.display()))?
        .len();
    let existing_hooks_size = fs::metadata(&hooks_dest).map(|m| m.len()).unwrap_or(0);
    let existing_amplihack_size = fs::metadata(&current_exe).map(|m| m.len()).unwrap_or(0);
    // Worst case: both destinations keep their current contents while the
    // `.tmp` files are written, so we need headroom for every new_* byte
    // plus a safety margin. `saturating_sub` keeps us honest if a future
    // build shrinks the binaries below the existing sizes.
    let required_free = new_hooks_size.saturating_sub(existing_hooks_size)
        + new_amplihack_size.saturating_sub(existing_amplihack_size)
        + 8 * 1024 * 1024;
    check_free_space(install_dir, required_free)?;

    install_binary_atomic(&new_hooks, &hooks_dest)?;
    install_binary_atomic(&new_amplihack, &current_exe)?;

    // Defensive verification: exec the replaced binary with `--version` and
    // confirm its self-reported version actually matches the release tag.
    // This catches the "release asset was built from an un-bumped Cargo.toml"
    // failure mode where the update claims success but the new binary still
    // self-reports the old version — which then retriggers the update prompt
    // forever. We warn (not fail) because the binary swap did happen; the
    // user can choose to re-run or ignore.
    let amplihack_ok = verify_installed_version(&current_exe, &release.version);
    // Verify the hooks binary as well — a version skew between amplihack and
    // amplihack-hooks can silently break hook execution at runtime if the
    // wire protocol changes between releases.
    let hooks_ok = verify_installed_version(&hooks_dest, &release.version);

    match (&amplihack_ok, &hooks_ok) {
        (Ok(()), Ok(())) => {
            println!(
                "Updated amplihack: {} -> {}",
                CURRENT_VERSION, release.version
            );
            println!("Restart amplihack to use the new version.");
        }
        _ => {
            if let Err(err) = &amplihack_ok {
                eprintln!(
                    "⚠️  amplihack was written to {} but reports an unexpected version: {err}",
                    current_exe.display()
                );
            }
            if let Err(err) = &hooks_ok {
                eprintln!(
                    "⚠️  amplihack-hooks was written to {} but reports an unexpected version: {err}",
                    hooks_dest.display()
                );
            }
            eprintln!(
                "   Expected v{} in both binaries. If the next launch still offers an update, the release asset may have been built without a version bump.",
                release.version
            );
            eprintln!(
                "   To stop the update loop, set AMPLIHACK_NO_UPDATE_CHECK=1 and report the mismatch."
            );
        }
    }

    // Invalidate the startup update cache regardless of outcome — its entry
    // was written by the previous (pre-swap) binary and may lie about what
    // version is actually installed now. Leaving it in place causes the new
    // binary to trust the old cache for up to 24h. (Belt-and-braces: the
    // cache's mtime-line check should already refuse stale entries, but we
    // delete here to avoid waiting for the next launch to discover that.)
    if let Ok(path) = super::cache_path() {
        let _ = fs::remove_file(&path);
    }

    Ok(())
}

/// Invoke `<binary> --version` and confirm the output contains `expected`.
///
/// `clap`'s default `--version` output is `"<name> <version>"`, so we match
/// on substring rather than equality. Runs with a short timeout so a hung
/// or broken binary cannot stall the caller.
fn verify_installed_version(binary: &Path, expected: &str) -> Result<()> {
    let mut child = Command::new(binary)
        .arg("--version")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to exec updated binary with --version")?;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .try_wait()
            .context("failed while waiting for updated binary --version")?
            .is_some()
        {
            break;
        }

        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out waiting for --version from updated binary");
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let output = child
        .wait_with_output()
        .context("failed while collecting output from updated binary --version")?;

    if !output.status.success() {
        bail!(
            "updated binary exited with status {} when run with --version",
            output.status
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected) {
        bail!(
            "expected `{expected}` in --version output, got: {}",
            stdout.trim()
        );
    }
    Ok(())
}

/// Refuse to start the binary swap when the destination filesystem can't
/// hold the new binaries plus a small safety margin.
///
/// On Unix we use `statvfs`. On other platforms we soft-fail open (log the
/// attempt and let the copy run) — better to let the error surface from
/// `fs::copy` than to block updates on hosts we can't probe.
#[cfg(unix)]
fn check_free_space(dir: &Path, required_bytes: u64) -> Result<()> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let c_path =
        CString::new(dir.as_os_str().as_bytes()).context("install dir contains a NUL byte")?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        // Can't measure — don't block the update.
        tracing::warn!(
            "statvfs({}) failed; skipping free-space preflight",
            dir.display()
        );
        return Ok(());
    }
    let stat = unsafe { stat.assume_init() };
    // `statvfs` field widths differ by OS: Linux uses `c_ulong` (u64 on our
    // 64-bit targets), macOS uses `u32`. Cast both to `u64` so the math works
    // on every supported host. Clippy flags the cast as unnecessary on Linux
    // but it's mandatory on macOS, so silence the lint on the one line.
    #[allow(clippy::unnecessary_cast)]
    let available: u64 = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
    if available < required_bytes {
        bail!(
            "not enough free space to install update: {} needs {} bytes free but has {} bytes. Free up disk and re-run `amplihack update`.",
            dir.display(),
            required_bytes,
            available
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_free_space(_dir: &Path, _required_bytes: u64) -> Result<()> {
    Ok(())
}

pub(super) fn binary_filename(name: &'static str) -> &'static str {
    if cfg!(windows) {
        match name {
            "amplihack" => "amplihack.exe",
            "amplihack-hooks" => "amplihack-hooks.exe",
            _ => name,
        }
    } else {
        name
    }
}

pub(crate) fn extract_archive(archive_bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(std::io::Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    archive
        .unpack(destination)
        .with_context(|| format!("failed to unpack archive into {}", destination.display()))?;
    Ok(())
}

pub(super) fn find_binary(root: &Path, binary_name: &str) -> Result<PathBuf> {
    fn search(root: &Path, binary_name: &str, depth: usize) -> Option<PathBuf> {
        if depth > 3 {
            return None;
        }

        let entries = fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name() == Some(OsStr::new(binary_name)) {
                return Some(path);
            }
            if path.is_dir()
                && let Some(found) = search(&path, binary_name, depth + 1)
            {
                return Some(found);
            }
        }
        None
    }

    search(root, binary_name, 0)
        .ok_or_else(|| anyhow!("binary '{}' not found in downloaded archive", binary_name))
}

fn install_binary_atomic(source: &Path, destination: &Path) -> Result<()> {
    let temp_destination = destination.with_extension("tmp");
    fs::copy(source, &temp_destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            temp_destination.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_destination, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", temp_destination.display()))?;
    }

    fs::rename(&temp_destination, destination).with_context(|| {
        format!(
            "failed to replace {} with {}",
            destination.display(),
            temp_destination.display()
        )
    })?;
    Ok(())
}
