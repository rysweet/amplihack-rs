//! Shared eligibility for the OAuth usage endpoint. The lock covers the
//! request and publication of any server deadline.

use amplihack_utils::secure_files::{
    check_private_file, dir_writable_by_others, ensure_private_dir, euid, open_private_nofollow,
    read_bounded_nofollow, write_private_atomic,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct Held {
    dir: PathBuf,
    deadline_file: String,
    lock: fs::File,
}

impl Drop for Held {
    fn drop(&mut self) {
        // A child can inherit the descriptor until it calls exec. Release
        // the lock here so the next probe can read the published state.
        let _ = self.lock.unlock();
    }
}

const DIR: &str = ".usage-retry";
const PENDING: &[u8] = b"pending";

/// A digest of the endpoint and login's token. A refreshed credential can
/// retain the digest without retaining the retired token.
#[derive(PartialEq, Eq)]
pub(super) struct Key(String);

impl Key {
    pub(super) fn new(endpoint: &str, token: &str) -> Self {
        let mut hash = Sha256::new();
        hash.update(endpoint.as_bytes());
        hash.update([0]);
        hash.update(token.as_bytes());
        Self(format!("{:x}", hash.finalize()))
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Self(value.to_ascii_lowercase()))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

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
    /// Attempt the lock once so a concurrent probe cannot delay this caller.
    pub(super) fn take(parent: &Path, key: &Key) -> io::Result<Self> {
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.uid() != euid() || dir_writable_by_others(parent, &meta) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "usage retry directory is not trusted",
            ));
        }
        let dir = parent.join(DIR);
        ensure_private_dir(&dir)?;
        let id = key.as_str();
        let lock = open_private_nofollow(&dir.join(format!("{id}.lock")))?;
        lock.try_lock().map_err(io::Error::from)?;
        Ok(Self {
            dir,
            deadline_file: format!("{id}.deadline"),
            lock,
        })
    }

    pub(super) fn deadline(&self, now_ms: i64) -> io::Result<Option<i64>> {
        let path = self.dir.join(&self.deadline_file);
        let Some((raw, meta)) = read_bounded_nofollow(&path, 32)? else {
            return Ok(None);
        };
        check_private_file(&meta)?;
        if raw == PENDING {
            // The lock is ours, so the earlier request can no longer
            // publish a response. There is no known server deadline.
            return Ok(None);
        }
        // Older records stored the timestamp alone. A new record appends
        // a complete line to the pending record through its open file.
        let raw = match raw.strip_prefix(b"pending\n") {
            Some(line) => {
                let Some(line) = line.strip_suffix(b"\n") else {
                    return Ok(None);
                };
                line
            }
            None => &raw,
        };
        let until = std::str::from_utf8(raw)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&ms| DateTime::<Utc>::from_timestamp_millis(ms).is_some())
            .ok_or_else(|| {
                io::Error::new(ErrorKind::InvalidData, "invalid usage retry deadline")
            })?;
        if now_ms < until {
            Ok(Some(until))
        } else {
            self.clear()?;
            Ok(None)
        }
    }

    /// Open the deadline record before sending the request. Publication uses
    /// this descriptor even if the directory is moved during the request.
    pub(super) fn begin(&self) -> io::Result<fs::File> {
        write_private_atomic(&self.dir, &self.deadline_file, PENDING)?;
        open_private_nofollow(&self.dir.join(&self.deadline_file))
    }

    pub(super) fn clear(&self) -> io::Result<()> {
        fs::remove_file(self.dir.join(&self.deadline_file))
    }

    pub(super) fn publish(mut record: fs::File, until_ms: i64) -> io::Result<()> {
        record.write_all(format!("\n{until_ms}\n").as_bytes())?;
        record.sync_all()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interrupted_deadline_write_leaves_the_next_caller_eligible() {
        let root = tempfile::tempdir().unwrap();
        let key = Key::new("stand-in", "example");
        {
            let held = Held::take(root.path(), &key).unwrap();
            let mut record = held.begin().unwrap();
            record.write_all(b"\n12000").unwrap();
        }

        let next = Held::take(root.path(), &key).unwrap();
        assert_eq!(next.deadline(1000).unwrap(), None);
        Held::publish(next.begin().unwrap(), 120_000).unwrap();
        assert_eq!(next.deadline(1000).unwrap(), Some(120_000));
    }

    #[test]
    fn dropping_the_guard_releases_the_lock_with_an_inherited_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let key = Key::new("stand-in", "example");
        let held = Held::take(root.path(), &key).unwrap();
        // A duplicated descriptor shares the open file description, as a
        // child does between fork and exec.
        let inherited = held.lock.try_clone().unwrap();
        drop(held);

        let next = Held::take(root.path(), &key).unwrap();
        drop(inherited);
        drop(next);
    }
}
