//! Reading a config file without hanging on a FIFO or swallowing `/dev/zero`,
//! and parsing it with its syntax error described by position only.
//!
//! Moved from `amplihack-cli`'s `user_config::safe` and `user_config::edit`
//! (issue #3247), so that the command-line program and the hook program read
//! the same files the same way.

use std::fs;
use std::io::Read;
use std::path::Path;

use toml_edit::DocumentMut;

/// Largest config file amplihack reads.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

/// Read `path` as UTF-8 text if it is a regular file of at most 64 KiB.
///
/// `Ok(None)` when nothing exists there. `Err(reason)` for anything that is
/// not a regular file (a FIFO, a device, a directory), a file that is too
/// large, unreadable or not UTF-8. The reason never quotes the contents.
///
/// The file is opened non-blocking, so a FIFO that appears between the type
/// check and the open cannot block, and its type is checked again on the open
/// handle.
pub fn read_bounded(path: &Path) -> Result<Option<String>, String> {
    read_bounded_with(path, |_| None)
}

/// [`read_bounded`], with `check` asked of the open handle before anything is
/// read from it. `Some(reason)` declines the file.
pub fn read_bounded_with(
    path: &Path,
    check: impl Fn(&fs::File) -> Option<String>,
) -> Result<Option<String>, String> {
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
    if let Some(reason) = check(&file) {
        return Err(reason);
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

/// Open `path` for reading without blocking on a FIFO.
#[cfg(unix)]
pub fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

/// Open `path` for reading.
#[cfg(not(unix))]
pub fn open_nonblocking(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

/// Parse `text`, or describe the syntax error by position only.
pub fn parse_document(text: &str) -> Result<DocumentMut, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
