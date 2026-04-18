use super::*;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::process::Command;
use std::time::Duration;
use tar::Archive;

/// Verify a downloaded archive against its SHA-256 checksum.
pub(super) fn verify_sha256(archive_bytes: &[u8], checksum_url: &str) -> Result<()> {
    let checksum_body = super::network::http_get(checksum_url)
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

    install_binary_atomic(&new_hooks, &hooks_dest)?;
    install_binary_atomic(&new_amplihack, &current_exe)?;

    // Defensive verification: exec the replaced binary with `--version` and
    // confirm its self-reported version actually matches the release tag.
    // This catches the "release asset was built from an un-bumped Cargo.toml"
    // failure mode where the update claims success but the new binary still
    // self-reports the old version — which then retriggers the update prompt
    // forever. We warn (not fail) because the binary swap did happen; the
    // user can choose to re-run or ignore.
    match verify_installed_version(&current_exe, &release.version) {
        Ok(()) => {
            println!(
                "Updated amplihack: {} -> {}",
                CURRENT_VERSION, release.version
            );
            println!("Restart amplihack to use the new version.");
        }
        Err(err) => {
            eprintln!(
                "⚠️  Update wrote {} but the installed binary reports an unexpected version: {err}",
                current_exe.display()
            );
            eprintln!(
                "   Expected v{} — if the next launch still offers an update, the release asset may have been built without a version bump.",
                release.version
            );
            eprintln!(
                "   To stop the loop, set AMPLIHACK_NO_UPDATE_CHECK=1 and report the mismatch."
            );
        }
    }
    Ok(())
}

/// Invoke `<binary> --version` and confirm the output contains `expected`.
///
/// `clap`'s default `--version` output is `"<name> <version>"`, so we match
/// on substring rather than equality. Runs with a short timeout so a hung
/// or broken binary cannot stall the caller.
fn verify_installed_version(binary: &Path, expected: &str) -> Result<()> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel();
    let binary_for_thread = binary.to_path_buf();
    thread::spawn(move || {
        let result = Command::new(&binary_for_thread)
            .arg("--version")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .output();
        let _ = tx.send(result);
    });

    let output = rx
        .recv_timeout(Duration::from_secs(5))
        .context("timed out waiting for --version from updated binary")?
        .context("failed to exec updated binary with --version")?;

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
