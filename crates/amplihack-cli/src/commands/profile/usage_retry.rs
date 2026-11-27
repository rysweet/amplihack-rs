//! Shared eligibility for the OAuth usage endpoint. The lock covers the
//! request and publication of any server deadline.

use amplihack_utils::secure_files::{
    check_private_dir, check_private_file, dir_writable_by_others, ensure_private_dir, euid,
    open_private_nofollow, read_bounded_nofollow, write_private_atomic,
    write_private_atomic_nosync,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::usage_parse::UsageReport;

pub(super) struct Held {
    dir: PathBuf,
    id: String,
    lock: fs::File,
}

/// The shared state directory's parent and the login's logical profile
/// path. The latter stays fixed when a named profile lives in ~/.claude.
#[derive(Clone, Copy)]
pub(super) struct Location<'a> {
    pub(super) parent: &'a Path,
    pub(super) slot: &'a Path,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    access: String,
    report: UsageReport,
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

    pub(super) fn slot(endpoint: &str, path: &Path) -> Self {
        let mut hash = Sha256::new();
        hash.update(endpoint.as_bytes());
        // Token keys cannot contain a second separator.
        hash.update(b"\0slot\0");
        hash.update(path.as_os_str().as_encoded_bytes());
        Self(format!("{:x}", hash.finalize()))
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
            id: id.to_owned(),
            lock,
        })
    }

    fn name(&self, extension: &str) -> String {
        format!("{}.{extension}", self.id)
    }

    /// Remember the credential key before a request can be sent. This
    /// pointer lives outside the credentials, so a native rewrite leaves it
    /// intact. Pure moves can still find the same deadline by credential key.
    pub(super) fn bind(&self, key: &Key) -> io::Result<()> {
        let name = self.name("slot");
        if self.bound_key()?.as_ref() != Some(key) {
            write_private_atomic(&self.dir, &name, key.as_str().as_bytes())?;
        }
        Ok(())
    }

    pub(super) fn bound_key(&self) -> io::Result<Option<Key>> {
        let Some((raw, meta)) = read_bounded_nofollow(&self.dir.join(self.name("slot")), 64)?
        else {
            return Ok(None);
        };
        check_private_file(&meta)?;
        let key = std::str::from_utf8(&raw).ok().and_then(Key::parse);
        key.map(Some)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "invalid usage retry slot"))
    }

    pub(super) fn deadline(&self, now_ms: i64) -> io::Result<Option<i64>> {
        let path = self.dir.join(self.name("deadline"));
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
        let name = self.name("deadline");
        write_private_atomic(&self.dir, &name, PENDING)?;
        open_private_nofollow(&self.dir.join(name))
    }

    pub(super) fn clear(&self) -> io::Result<()> {
        fs::remove_file(self.dir.join(self.name("deadline")))
    }

    pub(super) fn publish(mut record: fs::File, until_ms: i64) -> io::Result<()> {
        record.write_all(format!("\n{until_ms}\n").as_bytes())?;
        record.sync_all()
    }

    /// The display cache is disposable. A failed save leaves the read itself
    /// valid, and an atomic replacement is enough for concurrent readers.
    pub(super) fn cache(&self, access: &Key, report: UsageReport) -> io::Result<()> {
        let bytes = serde_json::to_vec(&Cached {
            access: access.as_str().to_owned(),
            report,
        })?;
        write_private_atomic_nosync(&self.dir, &self.name("usage"), &bytes)
    }
}

pub(super) fn cached(parent: &Path, slot: &Key, access: &Key) -> Option<UsageReport> {
    let dir = parent.join(DIR);
    check_private_dir(&dir).ok()?;
    let path = dir.join(format!("{}.usage", slot.as_str()));
    let (raw, meta) = read_bounded_nofollow(&path, super::usage_direct::MAX_BODY).ok()??;
    check_private_file(&meta).ok()?;
    let cached: Cached = serde_json::from_slice(&raw).ok()?;
    (cached.access == access.as_str()).then_some(cached.report)
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
