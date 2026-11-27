//! Shared eligibility for the OAuth usage endpoint. The lock covers the
//! request and publication of any server deadline.

use amplihack_utils::secure_files::{
    ExclusiveLock, check_private_file, dir_writable_by_others, ensure_private_dir, euid,
    lock_exclusive, open_private_nofollow, read_bounded_nofollow, write_private_atomic,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct Held {
    dir: PathBuf,
    deadline_file: String,
    _lock: ExclusiveLock,
}

const DIR: &str = ".usage-retry";

/// A passive read should leave state untouched until there is retry state
/// to consult.
pub(super) fn exists(parent: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(parent.join(DIR)) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

impl Held {
    /// The key is independent of the slot the login occupies. Only its
    /// digest is stored. A refresh token, when present, also makes access
    /// token renewals that keep the refresh token use the same key.
    pub(super) fn take(parent: &Path, endpoint: &str, key: &str) -> io::Result<Self> {
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.uid() != euid() || dir_writable_by_others(parent, &meta) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "usage retry directory is not trusted",
            ));
        }
        let dir = parent.join(DIR);
        ensure_private_dir(&dir)?;
        let mut hash = Sha256::new();
        hash.update(endpoint.as_bytes());
        hash.update([0]);
        hash.update(key.as_bytes());
        let id = hash.finalize();
        let lock = lock_exclusive(open_private_nofollow(&dir.join(format!("{id:x}.lock")))?)?;
        Ok(Self {
            dir,
            deadline_file: format!("{id:x}.deadline"),
            _lock: lock,
        })
    }

    pub(super) fn active(&self, now_ms: i64) -> io::Result<Option<i64>> {
        let path = self.dir.join(&self.deadline_file);
        let Some((raw, meta)) = read_bounded_nofollow(&path, 32)? else {
            return Ok(None);
        };
        check_private_file(&meta)?;
        let until = std::str::from_utf8(&raw)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&ms| DateTime::<Utc>::from_timestamp_millis(ms).is_some())
            .ok_or_else(|| {
                io::Error::new(ErrorKind::InvalidData, "invalid usage retry deadline")
            })?;
        if now_ms < until {
            Ok(Some(until))
        } else {
            fs::remove_file(path)?;
            Ok(None)
        }
    }

    pub(super) fn publish(&self, until_ms: i64) -> io::Result<()> {
        write_private_atomic(
            &self.dir,
            &self.deadline_file,
            until_ms.to_string().as_bytes(),
        )
    }
}

/// A server deadline in Unix milliseconds, strictly after receipt.
pub(super) fn deadline(value: &str, received_ms: i64) -> Option<i64> {
    let value = value.trim();
    let until = if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        received_ms.checked_add(value.parse::<i64>().ok()?.checked_mul(1000)?)?
    } else {
        [
            "%a, %d %b %Y %T GMT",
            "%A, %d-%b-%y %T GMT",
            "%a %b %e %T %Y",
        ]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())?
        .and_utc()
        .timestamp_millis()
    };
    (until > received_ms && DateTime::<Utc>::from_timestamp_millis(until).is_some())
        .then_some(until)
}
