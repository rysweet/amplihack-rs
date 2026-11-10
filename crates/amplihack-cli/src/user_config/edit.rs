//! Writing config files: `toml_edit` set/unset that keeps comments, and an
//! atomic 0600 write that keeps a symlinked target a symlink.
//!
//! Durability (issue #1992). Every write through [`write_atomic`] calls
//! `sync_all` on the temporary file before renaming it over the target. That
//! sync can be slow: one `sync_all` of `~/.amplihack/config.toml` took 9.76 s,
//! measured with strace on the first launch with a fresh HOME, on a build
//! host under write load, on `main` at 606f3c746. The launcher saves the
//! answer to the Node heap prompt, `node.heap_consent`, before claude starts,
//! so that sync delayed the start by the same amount.
//!
//! The decision: the launch-path consent save goes through
//! [`set_value_nosync`], which writes the same way (temporary file in the same
//! directory, mode 0600 before any byte, rename) but skips `sync_all`. Its
//! only caller is `user_config::save_heap_consent_nosync`, which takes no key
//! and always writes `node.heap_consent`. `config set`, `config unset` and
//! the legacy migration still sync, because they are explicit user actions
//! or one-off rewrites whose loss the user would not expect. `config edit` does not use this
//! module: it hands the file to the user's editor, which decides whether to
//! sync. Deferring the write until after claude has started was rejected,
//! because it would spread consent handling across the launch code.
//!
//! Why this is acceptable: `node.heap_consent` can be recovered. If it is
//! lost, the next interactive launch asks again, and a non-interactive launch
//! consents, as it would for a fresh HOME. The no-sync write rewrites the
//! whole user file, so a power loss or kernel crash between the rename and
//! the kernel's writeback can also lose the other keys in it. On the first
//! launch the file usually does not exist yet, and every key falls back to a
//! safe default. After such a crash the file can be:
//!
//! - unchanged: the consent was not saved, and the next launch asks again;
//! - missing or empty: every key takes its default, and the next launch asks
//!   again or, if non-interactive, consents;
//! - unreadable, for example filled with NUL bytes: no saved consent is
//!   found, so every launch asks again until the file is fixed. Each consent
//!   save fails without overwriting it, with the existing message pointing to
//!   `amplihack config edit`, and `save_user_preference` logs a warning.
//!
//! ext4 with `auto_da_alloc` and btrfs flush the data before a rename over an
//! existing file, which makes a zero-length result unlikely there; other
//! filesystems give no such guarantee. Errors that only appear at fsync time
//! (EIO, ENOSPC, EDQUOT, NFS writeback) are not reported on the no-sync path;
//! errors from the write and the rename still are.
//!
//! Out of scope: the syncs in `user_config::migrate` and syncing the parent
//! directory. The launcher-context sync was removed in #2068; see
//! `launcher_context::replace_file`.

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
/// `~/.amplihack`) when needed. The file is synced before the rename.
pub(crate) fn set_value(path: &Path, key: &KeyDef, value: &Value) -> Result<(), ConfigError> {
    set_value_with(path, key, value, true)
}

/// [`set_value`] without the sync before the rename.
///
/// The whole user file is rewritten, so after a crash the other keys in it
/// are at risk as well as `key`. It exists only for the recoverable consent
/// saved on the launch path; the module comment gives the details.
pub(crate) fn set_value_nosync(
    path: &Path,
    key: &KeyDef,
    value: &Value,
) -> Result<(), ConfigError> {
    set_value_with(path, key, value, false)
}

fn set_value_with(path: &Path, key: &KeyDef, value: &Value, sync: bool) -> Result<(), ConfigError> {
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
    write_atomic_with(path, doc.to_string().as_bytes(), sync)
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
    write_atomic_with(path, bytes, true)
}

/// The writer behind [`write_atomic`]; `sync` decides whether `sync_all` runs
/// before the rename. Everything else is the same in both modes.
fn write_atomic_with(path: &Path, bytes: &[u8], sync: bool) -> Result<(), ConfigError> {
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
    if sync {
        temp.as_file()
            .sync_all()
            .map_err(|e| io_error(&target, &e))?;
        #[cfg(test)]
        SYNCS.with(|count| count.set(count.get() + 1));
    }
    temp.persist(&target)
        .map_err(|e| io_error(&target, &e.error))?;
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Successful `sync_all` calls on this thread.
    static SYNCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Successful `sync_all` calls made by the atomic writer on this thread.
/// Tests compare the count before and after a call, never its absolute value.
#[cfg(test)]
pub(crate) fn sync_count() -> usize {
    SYNCS.with(std::cell::Cell::get)
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

    // Issue #1992: the launch-path consent save must not fsync, and every
    // other write through the atomic writer must keep doing so. The counter is
    // per thread, so these tests compare deltas and cannot see other tests.

    #[test]
    fn set_value_syncs_once_and_set_value_nosync_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# top\n[node]\n# keep\nheap_consent = false\n").unwrap();
        let consent = key("node.heap_consent").unwrap();

        let before = sync_count();
        set_value(&path, consent, &Value::Bool(true)).unwrap();
        assert_eq!(sync_count() - before, 1, "set_value must sync once");

        let before = sync_count();
        set_value_nosync(&path, consent, &Value::Bool(false)).unwrap();
        assert_eq!(sync_count(), before, "set_value_nosync must not sync");

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("# top") && body.contains("# keep"), "{body}");
        assert!(body.contains("heap_consent = false"), "{body}");
    }

    #[test]
    fn unset_value_and_write_atomic_still_sync() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[recipe]\nverbose = true\n").unwrap();

        let before = sync_count();
        assert!(unset_value(&path, key("recipe.verbose").unwrap()).unwrap());
        assert_eq!(sync_count() - before, 1, "config unset must sync");

        let before = sync_count();
        write_atomic(&path, b"[recipe]\nverbose = false\n").unwrap();
        assert_eq!(sync_count() - before, 1, "write_atomic must sync");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[recipe]\nverbose = false\n"
        );
    }

    #[test]
    fn nosync_creates_a_missing_file_with_only_the_new_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let before = sync_count();
        set_value_nosync(&path, key("node.heap_consent").unwrap(), &Value::Bool(true)).unwrap();
        assert_eq!(sync_count(), before);
        let parsed: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["node"]["heap_consent"].as_bool(), Some(true));
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".amplihack-config.")
            })
            .collect();
        assert!(leftovers.is_empty(), "temporary file left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn nosync_writes_are_0600_and_keep_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        fs::write(&real, "").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("link.toml");
        symlink(&real, &link).unwrap();
        let before = sync_count();
        set_value_nosync(&link, key("node.heap_consent").unwrap(), &Value::Bool(true)).unwrap();
        assert_eq!(sync_count(), before);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(&real)
                .unwrap()
                .contains("heap_consent = true")
        );
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn nosync_refuses_a_syntax_error_and_leaves_the_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[agent\nbinary = 1\n").unwrap();
        let err = set_value_nosync(&path, key("node.heap_consent").unwrap(), &Value::Bool(true))
            .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("line 1"), "{err}");
        assert!(err.to_string().contains("amplihack config edit"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[agent\nbinary = 1\n");
    }

    #[test]
    fn module_comment_records_the_durability_decision() {
        let source = include_str!("edit.rs");
        let header: String = source
            .lines()
            .take_while(|line| line.starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in ["#1992", "9.76 s", "606f3c746", "node.heap_consent"] {
            assert!(header.contains(needle), "module comment lacks {needle:?}");
        }
        assert!(
            !header.contains('\u{2014}'),
            "module comment has an em dash"
        );
    }
}
