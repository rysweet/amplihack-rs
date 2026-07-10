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
}
