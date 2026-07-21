//! Safe JSON state-file reads that never silently discard data.
//!
//! Persisted state files (VM pool, sessions) are read-modify-written. A naive
//! `unwrap_or_default()` on a parse error turns a *corrupt* file into an empty
//! value which the next write then persists back, destroying the user's data.
//!
//! [`read_json_state`] draws a hard line between three cases:
//!
//! * **missing** or **empty/whitespace** → `Ok(None)` (safe to start empty)
//! * **valid JSON** → `Ok(Some(value))`
//! * **corrupt** (non-empty, unparseable) → [`StateReadError::Corrupt`]
//!
//! Callers must propagate the corrupt error and must **not** overwrite the file
//! (`read_json_state` never mutates it), so the user can recover.

use std::path::Path;

use serde::de::DeserializeOwned;

use crate::state_lock::file_lock;

/// Failure reading a JSON state file.
#[derive(Debug)]
pub enum StateReadError {
    /// The file exists but could not be read (permissions, I/O, ...).
    Io(std::io::Error),
    /// The file has content but is not valid JSON. The file is left untouched.
    Corrupt {
        path: String,
        source: serde_json::Error,
    },
}

impl std::fmt::Display for StateReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateReadError::Io(e) => write!(f, "failed to read state file: {e}"),
            StateReadError::Corrupt { path, source } => {
                write!(f, "state file corrupt at {path}: {source}")
            }
        }
    }
}

impl std::error::Error for StateReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateReadError::Io(e) => Some(e),
            StateReadError::Corrupt { source, .. } => Some(source),
        }
    }
}

/// Read and parse a JSON state file, distinguishing "missing/empty" from
/// "corrupt".
///
/// Returns `Ok(None)` when the file is absent or contains only whitespace, and
/// `Ok(Some(value))` for valid JSON. A non-empty, unparseable file yields
/// [`StateReadError::Corrupt`] and the file is never modified.
pub fn read_json_state(path: &Path) -> Result<Option<serde_json::Value>, StateReadError> {
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path).map_err(StateReadError::Io)?;
    if content.trim().is_empty() {
        return Ok(None);
    }

    serde_json::from_str(&content)
        .map(Some)
        .map_err(|source| StateReadError::Corrupt {
            path: path.display().to_string(),
            source,
        })
}

/// Read a single top-level `key` out of a JSON state file and deserialize it
/// into `T`, preserving the missing/empty/corrupt distinction.
///
/// Returns `Ok(None)` when the file is missing/empty or the key is absent, and
/// `Ok(Some(value))` when the key parses into `T`. A corrupt outer file *or* a
/// value whose shape does not match `T` yields [`StateReadError::Corrupt`]
/// (schema mismatch is a form of corruption); the file is never modified.
pub fn read_keyed_state<T: DeserializeOwned>(
    path: &Path,
    key: &str,
) -> Result<Option<T>, StateReadError> {
    let Some(mut data) = read_json_state(path)? else {
        return Ok(None);
    };
    // `data` is a freshly parsed tree we own and drop right after, so move the
    // keyed sub-value out (leaving a `null` behind) instead of deep-cloning it.
    // Avoids duplicating potentially large state (all sessions / pool entries)
    // on every load.
    match data.get_mut(key) {
        None => Ok(None),
        Some(slot) => serde_json::from_value(slot.take())
            .map(Some)
            .map_err(|source| StateReadError::Corrupt {
                path: path.display().to_string(),
                source,
            }),
    }
}

/// Failure merging a value into a JSON state file.
#[derive(Debug)]
pub enum StateWriteError {
    /// The existing on-disk state could not be read (I/O or corrupt). The write
    /// is aborted so the file is preserved for recovery.
    Read(StateReadError),
    /// The advisory write lock could not be acquired.
    Lock(std::io::Error),
    /// Creating the parent directory or writing the file failed.
    Io(std::io::Error),
    /// The merged state could not be serialized back to JSON.
    Serialize(serde_json::Error),
}

impl std::fmt::Display for StateWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateWriteError::Read(e) => write!(f, "{e}"),
            StateWriteError::Lock(e) => write!(f, "failed to acquire state lock: {e}"),
            StateWriteError::Io(e) => write!(f, "failed to write state file: {e}"),
            StateWriteError::Serialize(e) => write!(f, "failed to serialize state: {e}"),
        }
    }
}

impl std::error::Error for StateWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateWriteError::Read(e) => Some(e),
            StateWriteError::Lock(e) | StateWriteError::Io(e) => Some(e),
            StateWriteError::Serialize(e) => Some(e),
        }
    }
}

/// Set `key` to `value` in a JSON state file under an advisory lock, merging
/// with any co-resident keys rather than replacing the whole file.
///
/// The existing file is read via [`read_json_state`], so a corrupt file surfaces
/// [`StateWriteError::Read`] and is left untouched instead of being overwritten
/// (which would wipe co-resident state). Missing/empty files start from an empty
/// object.
pub fn merge_key_into_state(
    path: &Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), StateWriteError> {
    let lock_path = path.with_extension("lock");
    let _guard = file_lock(&lock_path).map_err(StateWriteError::Lock)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(StateWriteError::Io)?;
    }

    let mut existing = read_json_state(path)
        .map_err(StateWriteError::Read)?
        .unwrap_or_else(|| serde_json::json!({}));
    existing[key] = value;

    let content = serde_json::to_string_pretty(&existing).map_err(StateWriteError::Serialize)?;
    std::fs::write(path, content).map_err(StateWriteError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.json");
        assert!(read_json_state(&path).unwrap().is_none());
    }

    #[test]
    fn empty_or_whitespace_file_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        for body in ["", "   \n\t "] {
            let path = dir.path().join("empty.json");
            std::fs::write(&path, body).unwrap();
            assert!(
                read_json_state(&path).unwrap().is_none(),
                "empty/whitespace is not corruption"
            );
        }
    }

    #[test]
    fn valid_json_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.json");
        std::fs::write(&path, r#"{"a": 1}"#).unwrap();
        let value = read_json_state(&path).unwrap().unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn corrupt_file_surfaces_error_naming_corruption_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "}{ not json").unwrap();

        let err = read_json_state(&path).unwrap_err();
        assert!(matches!(err, StateReadError::Corrupt { .. }));
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("corrupt"),
            "error must mention 'corrupt' to distinguish from a missing file: {msg}"
        );
        assert!(
            msg.contains(&path.display().to_string()),
            "error must name the offending path: {msg}"
        );
    }

    #[test]
    fn corrupt_file_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        let corrupt = "}{ not json";
        std::fs::write(&path, corrupt).unwrap();

        let _ = read_json_state(&path);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "read must never modify a corrupt file"
        );
    }

    #[test]
    fn read_keyed_state_absent_key_or_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.json");
        assert!(read_keyed_state::<i64>(&missing, "k").unwrap().is_none());

        let present = dir.path().join("state.json");
        std::fs::write(&present, r#"{"other": 1}"#).unwrap();
        assert!(read_keyed_state::<i64>(&present, "k").unwrap().is_none());
    }

    #[test]
    fn read_keyed_state_parses_matching_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"k": [1, 2, 3]}"#).unwrap();
        let value: Vec<i64> = read_keyed_state(&path, "k").unwrap().unwrap();
        assert_eq!(value, vec![1, 2, 3]);
    }

    #[test]
    fn read_keyed_state_schema_mismatch_is_corrupt_naming_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"k": "not-a-list"}"#).unwrap();

        let err = read_keyed_state::<Vec<i64>>(&path, "k").unwrap_err();
        assert!(matches!(err, StateReadError::Corrupt { .. }));
        let msg = err.to_string();
        assert!(msg.to_lowercase().contains("corrupt"), "{msg}");
        assert!(msg.contains(&path.display().to_string()), "{msg}");
    }

    #[test]
    fn merge_key_into_state_creates_and_preserves_co_resident_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");

        merge_key_into_state(&path, "a", serde_json::json!({"x": 1})).unwrap();
        merge_key_into_state(&path, "b", serde_json::json!([2, 3])).unwrap();

        let stored = read_json_state(&path).unwrap().unwrap();
        assert_eq!(stored["a"]["x"], 1, "first key survives the second merge");
        assert_eq!(stored["b"][0], 2);
    }

    #[test]
    fn merge_key_into_state_refuses_to_overwrite_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let corrupt = r#"{"a": BROKEN"#;
        std::fs::write(&path, corrupt).unwrap();

        let err = merge_key_into_state(&path, "b", serde_json::json!(1)).unwrap_err();
        assert!(matches!(err, StateWriteError::Read(_)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "merge must not overwrite a corrupt file"
        );
    }
}
