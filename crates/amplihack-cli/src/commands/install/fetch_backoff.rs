//! Remember a failed release-tag fetch so launches don't retry it one after
//! another (issue #1955, crusty round 1).
//!
//! Self-heal runs on every launch. Without a record of the last failure, N
//! launches on a host with a hung network each wait the full implicit budget
//! under the install lock. The marker `~/.amplihack/.tag-fetch-failed` holds the
//! version that failed and when; for [`BACKOFF_WINDOW`] after that, an implicit
//! install for the same version skips the fetch and fails at once, so self-heal
//! keeps the installed bundle with its usual warning.
//!
//! The holder of the install lock writes the marker before it releases the lock,
//! so a process that was waiting behind it reads the marker and does not retry.
//!
//! Explicit `install` and `update` never consult the marker: a user who asks for
//! an install gets a fetch. A failed explicit fetch still records one, since the
//! network just failed for this version as well. Any successful fetch removes it.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) const MARKER_FILE: &str = ".tag-fetch-failed";
pub(super) const BACKOFF_WINDOW: Duration = Duration::from_secs(10 * 60);
/// The marker is two short lines; anything larger isn't ours.
const MARKER_LIMIT: u64 = 256;

/// The marker under `~/.amplihack`.
pub(super) fn marker_path(install_root: &Path) -> PathBuf {
    install_root.join(MARKER_FILE)
}

/// How long ago a fetch for `version` failed, when that is inside the window.
///
/// A missing, unreadable, malformed or other-version marker, or one dated in
/// the future, never blocks a fetch.
pub(super) fn recent_failure(marker: &Path, version: &str, now: SystemTime) -> Option<Duration> {
    let meta = fs::symlink_metadata(marker).ok()?;
    if !meta.is_file() || meta.len() > MARKER_LIMIT {
        return None;
    }
    let text = fs::read_to_string(marker).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != version {
        return None;
    }
    let failed_at: u64 = lines.next()?.trim().parse().ok()?;
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let age = Duration::from_secs(now.checked_sub(failed_at)?);
    (age < BACKOFF_WINDOW).then_some(age)
}

/// Record that the fetch for `version` failed at `now`. Best effort: a marker
/// that can't be written only means the next launch retries.
pub(super) fn record_failure(marker: &Path, version: &str, now: SystemTime) {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let written = marker
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(marker, format!("{version}\n{secs}\n")));
    if let Err(error) = written {
        tracing::debug!(
            "fetch_backoff: could not write {}: {error}",
            marker.display()
        );
    }
}

/// Forget a recorded failure after a fetch succeeded.
pub(super) fn clear(marker: &Path) {
    match fs::remove_file(marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::debug!(
                "fetch_backoff: could not remove {}: {error}",
                marker.display()
            );
        }
    }
}

/// The reason given when a fetch is skipped. One line, no control characters.
pub(super) fn skipped_reason(age: Duration) -> String {
    let wait = BACKOFF_WINDOW
        .saturating_sub(age)
        .as_secs()
        .div_ceil(60)
        .max(1);
    format!(
        "a fetch for this version failed {} s ago, so it is not retried for another {wait} min",
        age.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn a_recorded_failure_blocks_the_same_version_inside_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let marker = marker_path(dir.path());
        record_failure(&marker, "1.2.99", at(1_000));

        assert_eq!(
            recent_failure(&marker, "1.2.99", at(1_030)),
            Some(Duration::from_secs(30))
        );
        assert_eq!(recent_failure(&marker, "1.2.98", at(1_030)), None);
        let end = 1_000 + BACKOFF_WINDOW.as_secs();
        assert!(recent_failure(&marker, "1.2.99", at(end - 1)).is_some());
        assert_eq!(recent_failure(&marker, "1.2.99", at(end)), None);
    }

    #[test]
    fn a_marker_from_the_future_or_malformed_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let marker = marker_path(dir.path());
        record_failure(&marker, "1.2.99", at(5_000));
        assert_eq!(recent_failure(&marker, "1.2.99", at(4_000)), None);

        fs::write(&marker, "1.2.99\nnot-a-number\n").unwrap();
        assert_eq!(recent_failure(&marker, "1.2.99", at(5_000)), None);
        fs::write(&marker, "x".repeat(1024)).unwrap();
        assert_eq!(recent_failure(&marker, "1.2.99", at(5_000)), None);
    }

    #[test]
    fn clear_removes_the_marker_and_tolerates_its_absence() {
        let dir = tempfile::tempdir().unwrap();
        let marker = marker_path(dir.path());
        clear(&marker);
        record_failure(&marker, "1.2.99", at(1));
        clear(&marker);
        assert!(!marker.exists());
    }

    #[test]
    fn the_skipped_reason_is_one_line() {
        let reason = skipped_reason(Duration::from_secs(30));
        assert!(!reason.contains('\n'));
        assert!(reason.contains("30 s ago"), "{reason}");
        assert!(reason.contains("10 min"), "{reason}");
    }
}
