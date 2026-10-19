//! Writing config files: `toml_edit` set/unset that keeps comments, and an
//! atomic 0600 write that keeps a symlinked target a symlink.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

use super::ConfigError;
use super::safe::{display_path, read_bounded};
use super::schema::{KeyDef, Value};

/// Parse `text`, or describe the syntax error by position only.
pub(crate) fn parse_document(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>().map_err(|error| {
        let at = error.span().map(|span| span.start).unwrap_or(0);
        let (line, column) = line_column(text, at);
        format!("TOML syntax error at line {line}, column {column}")
    })
}

/// 1-based line and column of byte offset `at` in `text`.
fn line_column(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map(|tail| tail.chars().count() + 1)
        .unwrap_or(1);
    (line, column)
}

/// Read and parse `path` for editing; a missing file is an empty document.
fn load_for_edit(path: &Path) -> Result<DocumentMut, ConfigError> {
    let text = read_bounded(path)
        .map_err(|reason| ConfigError::File {
            path: display_path(path),
            reason,
        })?
        .unwrap_or_default();
    parse_document(&text).map_err(|reason| ConfigError::File {
        path: display_path(path),
        reason: format!("{reason}; fix it with `amplihack config edit` first"),
    })
}

/// Set `key` to `value` in `path`, creating the file (and, for the user file,
/// `~/.amplihack`) when needed.
pub(crate) fn set_value(path: &Path, key: &KeyDef, value: &Value) -> Result<(), ConfigError> {
    let mut doc = load_for_edit(path)?;
    let (table, name) = key.split();
    let entry = doc
        .entry(table)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table_like) = entry.as_table_like_mut() else {
        return Err(ConfigError::File {
            path: display_path(path),
            reason: format!("`{table}` is not a table; fix it with `amplihack config edit`"),
        });
    };
    let item = match value {
        Value::Bool(b) => toml_edit::value(*b),
        Value::Int(i) => {
            toml_edit::value(i64::try_from(*i).map_err(|_| ConfigError::InvalidValue {
                key: key.name.to_string(),
                reason: "is too large".to_string(),
            })?)
        }
        Value::Text(s) => toml_edit::value(s.as_str()),
    };
    // Replace an existing value in place: `insert` would also replace the key,
    // and with it any comment written above the line.
    match table_like.get_mut(name) {
        Some(existing) => *existing = item,
        None => {
            table_like.insert(name, item);
        }
    }
    write_atomic(path, doc.to_string().as_bytes())
}

/// Remove `key` from `path`. `Ok(false)` when it was not set there.
pub(crate) fn unset_value(path: &Path, key: &KeyDef) -> Result<bool, ConfigError> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(false);
    }
    let mut doc = load_for_edit(path)?;
    let (table, name) = key.split();
    let removed = doc
        .get_mut(table)
        .and_then(toml_edit::Item::as_table_like_mut)
        .and_then(|t| t.remove(name))
        .is_some();
    if removed {
        write_atomic(path, doc.to_string().as_bytes())?;
    }
    Ok(removed)
}

/// The file a write to `path` lands in: `path` itself, or the file a symlink
/// at `path` points to, so the symlink is kept.
fn write_target(path: &Path) -> Result<PathBuf, ConfigError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let link = fs::read_link(path).map_err(|e| io_error(path, &e))?;
            let target = if link.is_absolute() {
                link
            } else {
                path.parent().unwrap_or(Path::new("/")).join(link)
            };
            // Follow a chain of links to its end.
            write_target(&target)
        }
        _ => Ok(path.to_path_buf()),
    }
}

fn io_error(path: &Path, error: &std::io::Error) -> ConfigError {
    ConfigError::File {
        path: display_path(path),
        reason: format!("cannot be written ({error})"),
    }
}

/// Create `dir` with mode 0700 when it does not exist. An existing directory
/// is left as it is.
pub(crate) fn ensure_private_dir(dir: &Path) -> Result<(), ConfigError> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|e| io_error(dir, &e))
}

/// Remove group and other access from `dir` when this user owns it. The
/// owner's own bits are kept: this never makes a read-only directory
/// writable. Used for `~/.amplihack` only.
pub(crate) fn tighten_owned_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let Ok(meta) = fs::metadata(dir) else {
            return;
        };
        let mode = meta.mode() & 0o777;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if meta.uid() == unsafe { libc::geteuid() } && mode & 0o077 != 0 {
            let tightened = fs::Permissions::from_mode(mode & 0o700);
            if let Err(error) = fs::set_permissions(dir, tightened) {
                tracing::warn!(path = %dir.display(), %error, "could not remove group and other access from the config directory");
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Write `bytes` to `path` atomically: a random-name temporary file in the
/// target's directory, set to 0600 before anything is written, synced, then
/// renamed over the target. A symlink at `path` is kept and its target
/// written.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let target = write_target(path)?;
    let dir = target
        .parent()
        .ok_or_else(|| ConfigError::File {
            path: display_path(&target),
            reason: "has no parent directory".to_string(),
        })?
        .to_path_buf();
    ensure_private_dir(&dir)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".amplihack-config.")
        .tempfile_in(&dir)
        .map_err(|e| io_error(&target, &e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| io_error(&target, &e))?;
    }
    temp.write_all(bytes).map_err(|e| io_error(&target, &e))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| io_error(&target, &e))?;
    temp.persist(&target)
        .map_err(|e| io_error(&target, &e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_config::schema::key;

    #[test]
    fn set_keeps_comments_and_unset_reports_absence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# top\n[recipe]\n# keep\nverbose = false\n").unwrap();
        let verbose = key("recipe.verbose").unwrap();
        set_value(&path, verbose, &Value::Bool(true)).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("# top") && body.contains("# keep"), "{body}");
        assert!(body.contains("verbose = true"), "{body}");
        assert!(unset_value(&path, verbose).unwrap());
        assert!(!unset_value(&path, verbose).unwrap());
        assert!(!unset_value(&dir.path().join("none.toml"), verbose).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn writes_are_0600_and_keep_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        fs::write(&real, "").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("link.toml");
        symlink(&real, &link).unwrap();
        set_value(
            &link,
            key("agent.binary").unwrap(),
            &Value::Text("codex".into()),
        )
        .unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::read_to_string(&real).unwrap().contains("codex"));
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_syntax_error_refuses_the_write_and_names_the_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[agent\nbinary = 1\n").unwrap();
        let err = set_value(&path, key("recipe.verbose").unwrap(), &Value::Bool(true)).unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("line 1"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[agent\nbinary = 1\n");
    }
}
