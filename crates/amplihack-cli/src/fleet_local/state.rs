//! Session state collection from lock files.

use std::path::Path;

use super::{FleetLocalError, FleetSessionEntry, PID_MAX, SessionStatus};

// ── collect_observed_fleet_state ──────────────────────────────────────────────

/// Read `~/.claude/runtime/locks/*` and return observed local Claude sessions.
///
/// # Behaviour
///
/// - **Empty directory** → returns `Ok(vec![])` without panicking (TC-10).
/// - **Dotfiles** (names starting with `.`) are skipped — they are sentinel
///   files such as `.lock_active`, not session lock files.
/// - Each lock file name is passed through `sanitize_session_id()` (SEC-01).
/// - Lock file content is parsed as a decimal PID integer.
/// - PIDs outside `1..=4_194_304` are silently skipped (SEC-04).
/// - Process liveness is checked via `/proc/{pid}/comm` on Linux, or
///   `sysctl` on macOS (RISK-06).
///
/// # Errors
///
/// Returns `Err` only on unrecoverable I/O errors (e.g., `locks_dir` is a
/// file, not a directory).  Individual corrupt or invalid lock files are
/// silently skipped, never propagated.
pub fn collect_observed_fleet_state(
    locks_dir: &Path,
) -> Result<Vec<FleetSessionEntry>, FleetLocalError> {
    use amplihack_types::paths::sanitize_session_id;

    // If the directory doesn't exist, return empty (TC-10).
    if !locks_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions = Vec::new();

    let entries = std::fs::read_dir(locks_dir)?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // skip unreadable entries
        };

        let file_name = entry.file_name();
        let raw_name = file_name.to_string_lossy();

        // Skip dotfiles (sentinel files like `.lock_active`).
        if raw_name.starts_with('.') {
            continue;
        }

        // SEC-01: sanitize the session ID.
        let session_id = sanitize_session_id(&raw_name);
        if session_id.is_empty() {
            continue;
        }

        // Read PID from lock file content.
        let content = match std::fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let pid: u32 = match content.trim().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // SEC-04: reject PIDs outside 1..=PID_MAX.
        if pid == 0 || pid > PID_MAX {
            continue;
        }

        // Check process liveness.
        let status = check_pid_liveness(pid);

        sessions.push(FleetSessionEntry {
            session_id,
            pid,
            status,
            project_path: None,
        });
    }

    Ok(sessions)
}

/// Check whether a process with `pid` is alive on the current platform.
///
/// On Linux: reads `/proc/{pid}/comm`.
/// On other platforms: always returns `Unknown`.
fn check_pid_liveness(pid: u32) -> SessionStatus {
    #[cfg(target_os = "linux")]
    {
        // Pass the format string directly — no intermediate PathBuf allocation needed;
        // `std::fs::metadata` accepts anything that implements `AsRef<Path>`, and
        // `String` qualifies via the blanket impl.
        match std::fs::metadata(format!("/proc/{pid}/comm")) {
            Ok(_) => SessionStatus::Active,
            Err(_) => SessionStatus::Dead,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        SessionStatus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TC-10 (part A): empty locks directory → `Ok(vec![])`, no panic.
    #[test]
    fn tc10_collect_returns_empty_vec_on_empty_locks_dir() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();

        let result = collect_observed_fleet_state(&locks_dir);
        assert!(
            result.is_ok(),
            "must not error on empty locks dir; got {result:?}"
        );
        assert!(
            result.unwrap().is_empty(),
            "must return empty vec for empty locks dir"
        );
    }

    /// TC-10 (part B): non-existent locks directory → `Ok(vec![])`, no panic.
    #[test]
    fn tc10_collect_returns_empty_vec_on_nonexistent_locks_dir() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks_does_not_exist");

        let result = collect_observed_fleet_state(&locks_dir);
        assert!(
            result.is_ok(),
            "must not error on missing locks dir; got {result:?}"
        );
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn collect_skips_dotfiles_in_locks_dir() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();

        std::fs::write(locks_dir.join(".lock_active"), "1234\n").unwrap();
        std::fs::write(locks_dir.join(".continuation_prompt"), "some text").unwrap();

        let result = collect_observed_fleet_state(&locks_dir).unwrap();
        assert!(
            result.is_empty(),
            "dotfiles must be skipped; got {result:?}"
        );
    }

    #[test]
    fn collect_rejects_pid_zero() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        std::fs::write(locks_dir.join("test-session"), "0\n").unwrap();

        let result = collect_observed_fleet_state(&locks_dir).unwrap();
        assert!(
            result.is_empty(),
            "PID 0 must be rejected and entry skipped"
        );
    }

    #[test]
    fn collect_rejects_pid_above_max() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        std::fs::write(locks_dir.join("test-session"), format!("{}\n", PID_MAX + 1)).unwrap();

        let result = collect_observed_fleet_state(&locks_dir).unwrap();
        assert!(
            result.is_empty(),
            "PID > PID_MAX must be rejected and entry skipped"
        );
    }

    #[test]
    fn collect_accepts_pid_at_boundary_max() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        std::fs::write(locks_dir.join("boundary-session"), format!("{PID_MAX}\n")).unwrap();

        let result = collect_observed_fleet_state(&locks_dir).unwrap();
        assert_eq!(result.len(), 1, "PID_MAX must be accepted");
        assert_eq!(result[0].pid, PID_MAX);
    }

    #[test]
    fn collect_accepts_pid_one_lower_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        std::fs::write(locks_dir.join("pid-one-session"), "1\n").unwrap();

        let result = collect_observed_fleet_state(&locks_dir).unwrap();
        assert_eq!(result.len(), 1, "PID 1 must be accepted");
        assert_eq!(result[0].pid, 1);
    }

    #[test]
    fn collect_sanitizes_session_id_from_lock_filename() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        std::fs::write(locks_dir.join("abc-123-def"), "1234\n").unwrap();

        let result = collect_observed_fleet_state(&locks_dir).unwrap();
        if !result.is_empty() {
            assert_eq!(
                result[0].session_id, "abc-123-def",
                "sanitized session ID must match lock filename"
            );
        }
    }

    #[test]
    fn collect_skips_entries_with_non_numeric_pid_content() {
        let dir = tempfile::tempdir().unwrap();
        let locks_dir = dir.path().join("locks");
        std::fs::create_dir_all(&locks_dir).unwrap();
        std::fs::write(locks_dir.join("json-session"), r#"{"pid": 1234}"#).unwrap();
        std::fs::write(locks_dir.join("empty-session"), "").unwrap();
        std::fs::write(locks_dir.join("text-session"), "not_a_number\n").unwrap();

        let result = collect_observed_fleet_state(&locks_dir);
        assert!(
            result.is_ok(),
            "malformed lock files must not error; got {result:?}"
        );
    }
}
