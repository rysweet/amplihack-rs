use super::*;
use sha2::{Digest, Sha256};

/// Verify a downloaded archive against its SHA-256 checksum.
pub(super) fn verify_sha256(archive_bytes: &[u8], checksum_url: &str) -> Result<()> {
    let checksum_body = super::network::http_get_with_retry(checksum_url)
        .with_context(|| format!("failed to download checksum from {checksum_url}"))?;
    verify_sha256_bytes(archive_bytes, &checksum_body, checksum_url)
}

/// Verify an archive against the contents of its `.sha256` file. `label`
/// names where the checksum came from, for error messages.
pub(super) fn verify_sha256_bytes(
    archive_bytes: &[u8],
    checksum_body: &[u8],
    label: &str,
) -> Result<()> {
    let checksum_text =
        std::str::from_utf8(checksum_body).context("checksum file is not valid UTF-8")?;

    let expected_hex = checksum_text
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| anyhow!("checksum file is empty or malformed: {label}"))?;

    if expected_hex.len() != 64 || !expected_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "checksum file does not contain a valid SHA-256 hex digest (got {expected_hex:?}): {label}"
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
        checksum = label,
        sha256 = actual_hex,
        "archive checksum verified"
    );
    Ok(())
}
