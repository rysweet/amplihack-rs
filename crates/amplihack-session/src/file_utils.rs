//! Defensive file I/O utilities ported from `file_utils.py`.

use crate::config::{Result, SessionError};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::digest::Digest;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

/// Maximum file size accepted by [`safe_read_json`] (defends against OOM on
/// crafted input). Reads beyond this fail with [`crate::SessionError::TooLarge`].
pub const MAX_JSON_FILE_BYTES: u64 = 64 * 1024 * 1024;

const RETRY_ATTEMPTS: u32 = 3;
const RETRY_INITIAL_DELAY: Duration = Duration::from_millis(100);
const RETRY_BACKOFF: u32 = 2;

/// Hash algorithm selector for [`get_file_checksum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
}

fn retry<T, F: FnMut() -> std::io::Result<T>>(op_name: &str, mut f: F) -> Result<T> {
    let mut delay = RETRY_INITIAL_DELAY;
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..=RETRY_ATTEMPTS {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(
                    "file op `{op_name}` failed (attempt {}/{}): {e}",
                    attempt + 1,
                    RETRY_ATTEMPTS + 1
                );
                last_err = Some(e);
                if attempt < RETRY_ATTEMPTS {
                    std::thread::sleep(delay);
                    delay *= RETRY_BACKOFF;
                }
            }
        }
    }
    Err(SessionError::RetryExhausted(format!(
        "{op_name}: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown".into())
    )))
}

/// Read a UTF-8 text file with retry, size check, and structured errors.
///
/// Returns `Ok(None)` if the file does not exist (parity with Python's
/// `safe_read_file`).
pub fn safe_read_file(path: impl AsRef<Path>) -> Result<Option<String>> {
    let p = path.as_ref();
    if !p.exists() {
        return Ok(None);
    }
    let s = retry("safe_read_file", || fs::read_to_string(p))?;
    Ok(Some(s))
}

/// Atomic write of a UTF-8 text file with optional backup and verify-after-write.
pub fn safe_write_file(
    path: impl AsRef<Path>,
    content: &str,
    atomic: bool,
    backup: bool,
    verify: bool,
) -> Result<()> {
    let p = path.as_ref();
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| SessionError::io(parent, e))?;
        }
    }

    if backup && p.exists() {
        let backup_path = backup_path_for(p);
        fs::copy(p, &backup_path).map_err(|e| SessionError::io(&backup_path, e))?;
    }

    let write_to = |dest: &Path| -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(dest)?;
        f.write_all(content.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    };

    if atomic {
        let parent = p.parent().unwrap_or_else(|| Path::new("."));
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("file");
        let tmp_name = format!(".{fname}.{}.tmp", std::process::id());
        let tmp = parent.join(&tmp_name);
        retry("safe_write_file_atomic", || {
            write_to(&tmp)?;
            fs::rename(&tmp, p)?;
            Ok(())
        })
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            e
        })?;
    } else {
        retry("safe_write_file_direct", || write_to(p))?;
    }

    if verify {
        let read_back = fs::read_to_string(p).map_err(|e| SessionError::io(p, e))?;
        if read_back != content {
            return Err(SessionError::Corruption(format!(
                "verify-after-write mismatch at {}",
                p.display()
            )));
        }
    }
    Ok(())
}

fn backup_path_for(p: &Path) -> std::path::PathBuf {
    // Python's behavior: with_suffix(f"{suffix}.backup")
    // For "data.txt" → "data.txt.backup"; for "data" → "data.backup"
    let mut s = p.as_os_str().to_owned();
    s.push(".backup");
    std::path::PathBuf::from(s)
}

/// Read & deserialize JSON, returning the deserialized `default` value if the
/// file does not exist or is unreadable.
///
/// **Security:** files larger than [`MAX_JSON_FILE_BYTES`] are rejected with
/// [`crate::SessionError::TooLarge`] before deserialization (OOM defense).
pub fn safe_read_json<T: DeserializeOwned>(path: impl AsRef<Path>, default: T) -> Result<T> {
    let p = path.as_ref();
    if !p.exists() {
        return Ok(default);
    }
    let meta = fs::metadata(p).map_err(|e| SessionError::io(p, e))?;
    if meta.len() > MAX_JSON_FILE_BYTES {
        return Err(SessionError::TooLarge {
            path: p.to_path_buf(),
            size: meta.len(),
            max: MAX_JSON_FILE_BYTES,
        });
    }
    let content = match fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("safe_read_json: read failed at {}: {e}", p.display());
            return Ok(default);
        }
    };
    match serde_json::from_str::<T>(&content) {
        Ok(v) => Ok(v),
        Err(e) => {
            tracing::warn!("safe_read_json: parse failed at {}: {e}", p.display());
            Ok(default)
        }
    }
}

/// Atomic write of a JSON-serializable value.
pub fn safe_write_json<T: Serialize>(path: impl AsRef<Path>, data: &T) -> Result<()> {
    let p = path.as_ref();
    let json = serde_json::to_string_pretty(data).map_err(|e| SessionError::json(p, e))?;
    safe_write_file(p, &json, true, false, true)
}

/// Compute a hex checksum of a file's contents.
pub fn get_file_checksum(path: impl AsRef<Path>, algorithm: ChecksumAlgorithm) -> Result<String> {
    let p = path.as_ref();
    match algorithm {
        ChecksumAlgorithm::Md5 => hash_file::<md5::Md5>(p),
        ChecksumAlgorithm::Sha1 => hash_file::<sha1::Sha1>(p),
        ChecksumAlgorithm::Sha256 => hash_file::<sha2::Sha256>(p),
    }
}

fn hash_file<D: Digest>(p: &Path) -> Result<String> {
    let mut f = fs::File::open(p).map_err(|e| SessionError::io(p, e))?;
    let mut hasher = D::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf).map_err(|e| SessionError::io(p, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    }))
}

/// Copy a file, optionally verifying both files have identical SHA-256.
pub fn safe_copy_file(src: impl AsRef<Path>, dst: impl AsRef<Path>, verify: bool) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    if !src.exists() {
        return Err(SessionError::NotFound(src.display().to_string()));
    }
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| SessionError::io(parent, e))?;
        }
    }
    fs::copy(src, dst).map_err(|e| SessionError::io(dst, e))?;
    if verify {
        let src_sum = get_file_checksum(src, ChecksumAlgorithm::Sha256)?;
        let dst_sum = get_file_checksum(dst, ChecksumAlgorithm::Sha256)?;
        if src_sum != dst_sum {
            let _ = fs::remove_file(dst);
            return Err(SessionError::Corruption(format!(
                "copy verify mismatch: {} -> {}",
                src.display(),
                dst.display()
            )));
        }
    }
    Ok(())
}

/// Move a file, optionally verifying integrity by checksum.
pub fn safe_move_file(src: impl AsRef<Path>, dst: impl AsRef<Path>, verify: bool) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    if !src.exists() {
        return Err(SessionError::NotFound(src.display().to_string()));
    }
    let pre_sum = if verify {
        Some(get_file_checksum(src, ChecksumAlgorithm::Sha256)?)
    } else {
        None
    };
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| SessionError::io(parent, e))?;
        }
    }
    if let Err(rename_err) = fs::rename(src, dst) {
        // Cross-device or other rename failure: fall back to copy+remove.
        fs::copy(src, dst).map_err(|e| SessionError::io(dst, e))?;
        fs::remove_file(src).map_err(|e| SessionError::io(src, e))?;
        tracing::debug!("rename fell back to copy+remove: {rename_err}");
    }
    if let Some(pre) = pre_sum {
        let post = get_file_checksum(dst, ChecksumAlgorithm::Sha256)?;
        if pre != post {
            return Err(SessionError::Corruption(format!(
                "move verify mismatch: {} -> {}",
                src.display(),
                dst.display()
            )));
        }
    }
    Ok(())
}

/// Translate a simple shell-style glob (`*`, `?`) into a regex anchored to the
/// full filename. Other regex metacharacters are escaped.
pub(crate) fn glob_to_regex(pattern: &str) -> regex::Regex {
    let mut s = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => s.push_str(".*"),
            '?' => s.push('.'),
            '.' | '+' | '(' | ')' | '{' | '}' | '[' | ']' | '|' | '^' | '$' | '\\' => {
                s.push('\\');
                s.push(ch);
            }
            _ => s.push(ch),
        }
    }
    s.push('$');
    regex::Regex::new(&s).expect("valid translated glob regex")
}

/// Remove temp files in `temp_dir` matching `pattern` and older than
/// `max_age_hours`. Symlinks are NOT followed out of `temp_dir`.
pub fn cleanup_temp_files(
    temp_dir: impl AsRef<Path>,
    max_age_hours: f64,
    pattern: &str,
) -> Result<u64> {
    let dir = temp_dir.as_ref();
    if !dir.exists() {
        return Ok(0);
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(Duration::from_secs_f64(max_age_hours * 3600.0))
        .ok_or_else(|| SessionError::Corruption("cutoff time underflow".into()))?;
    let re = glob_to_regex(pattern);
    let mut cleaned = 0u64;

    let entries = fs::read_dir(dir).map_err(|e| SessionError::io(dir, e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        // Don't follow symlinks (security: avoid escape from temp dir).
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !re.is_match(&name) {
            continue;
        }
        if let Ok(mtime) = meta.modified() {
            if mtime < cutoff {
                if let Err(e) = fs::remove_file(&path) {
                    tracing::warn!("cleanup_temp_files: remove {} failed: {e}", path.display());
                } else {
                    cleaned += 1;
                }
            }
        }
    }
    Ok(cleaned)
}
