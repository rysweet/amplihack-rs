//! Defensive helpers for untrusted config input: printing it safely, and
//! reading a file without hanging on a FIFO or swallowing `/dev/zero`.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Largest config file amplihack reads.
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024;

/// Characters shown before [`display_safe`] truncates.
const MAX_DISPLAY_CHARS: usize = 200;

/// `s` made safe for a terminal: control characters, DEL and bidirectional
/// formatting characters are shown as `\u{..}` escapes, and anything longer
/// than 200 characters is truncated with `…`.
pub(crate) fn display_safe(s: &str) -> String {
    let mut out = String::new();
    for (count, c) in s.chars().enumerate() {
        if count == MAX_DISPLAY_CHARS {
            out.push('…');
            break;
        }
        if c.is_control() || is_bidi_control(c) {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The Unicode bidirectional formatting characters, which can make printed
/// text read differently from what it contains.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// [`display_safe`] for a path.
pub(crate) fn display_path(path: &Path) -> String {
    display_safe(&path.to_string_lossy())
}

/// Read `path` as UTF-8 text if it is a regular file of at most 64 KiB.
///
/// `Ok(None)` when nothing exists there. `Err(reason)` for anything that is
/// not a regular file (a FIFO, a device, a directory), a file that is too
/// large, unreadable or not UTF-8. The reason never quotes the contents.
///
/// The file is opened non-blocking, so a FIFO that appears between the type
/// check and the open cannot block, and its type is checked again on the open
/// handle.
pub(crate) fn read_bounded(path: &Path) -> Result<Option<String>, String> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is reported, not taken for "no file".
            return match fs::symlink_metadata(path) {
                Ok(_) => Err("is a symlink to a file that does not exist".to_string()),
                Err(_) => Ok(None),
            };
        }
        Err(error) => return Err(format!("cannot be read ({})", error.kind())),
    };
    if !meta.is_file() {
        return Err("is not a regular file; ignored".to_string());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err("is larger than 64 KiB; ignored".to_string());
    }
    let file =
        open_nonblocking(path).map_err(|error| format!("cannot be read ({})", error.kind()))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("cannot be read ({})", error.kind()))?;
    if !opened.is_file() {
        return Err("is not a regular file; ignored".to_string());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot be read ({})", error.kind()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("is larger than 64 KiB; ignored".to_string());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "is not valid UTF-8; ignored".to_string())
}

#[cfg(unix)]
fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

/// Why the user file cannot be trusted with user-only keys, or `None` when it
/// is owned by this user and not writable by group or others.
#[cfg(unix)]
pub(crate) fn insecure_reason(path: &Path) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if meta.uid() != unsafe { libc::geteuid() } {
        Some("ignored because the file is owned by another user")
    } else if meta.mode() & 0o022 != 0 {
        Some("ignored because the file is writable by group or others")
    } else {
        None
    }
}

#[cfg(not(unix))]
pub(crate) fn insecure_reason(_path: &Path) -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_safe_escapes_controls_and_bidi() {
        assert_eq!(display_safe("a\u{1b}[31m"), "a\\u{1b}[31m");
        assert_eq!(display_safe("x\u{202e}y\u{7f}"), "x\\u{202e}y\\u{7f}");
        assert_eq!(display_safe("plain.key"), "plain.key");
    }

    #[test]
    fn display_safe_truncates() {
        let long = "x".repeat(500);
        let shown = display_safe(&long);
        assert_eq!(shown.chars().count(), 201);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn read_bounded_handles_missing_large_and_special_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_bounded(&dir.path().join("none")), Ok(None));
        assert!(read_bounded(dir.path()).is_err());
        let big = dir.path().join("big");
        fs::write(&big, vec![b'#'; 70 * 1024]).unwrap();
        assert!(read_bounded(&big).unwrap_err().contains("64 KiB"));
        let ok = dir.path().join("ok");
        fs::write(&ok, "a = 1\n").unwrap();
        assert_eq!(read_bounded(&ok).unwrap().as_deref(), Some("a = 1\n"));
        #[cfg(unix)]
        assert!(read_bounded(Path::new("/dev/zero")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_bounded_does_not_block_on_a_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_bounded(&fifo).is_err());
    }
}
