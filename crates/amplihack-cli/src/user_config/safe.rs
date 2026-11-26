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
    read_bounded_with(path, |_| None)
}

/// [`read_bounded`], with `check` asked of the open handle before anything is
/// read from it. `Some(reason)` refuses the file.
fn read_bounded_with(
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

/// [`insecure_reason`] asked of a file that is already open.
///
/// A path is a name, and a name can be made to point somewhere else between
/// being checked and being opened. A handle is the object itself, so the file
/// judged here is the file whose contents the caller goes on to read.
#[cfg(unix)]
pub(crate) fn insecure_reason_of(file: &fs::File) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().ok()?;
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
pub(crate) fn insecure_reason_of(_file: &fs::File) -> Option<&'static str> {
    None
}

/// [`read_bounded`], refusing a file this user would not trust with
/// user-only settings.
///
/// One open serves both the trust check and the read, so the two cannot
/// address different objects. A symlink is followed and then judged by what
/// it points at: a config file kept in a dotfiles directory and linked into
/// place is an ordinary arrangement, and what matters is the trust of the
/// file actually opened.
pub(crate) fn read_checked(path: &Path) -> Result<Option<String>, String> {
    read_bounded_with(path, |file| insecure_reason_of(file).map(str::to_string))
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

    // ---------------------------------------------------------------------
    // Issue #1619 (R3): the trust check and the read address one object
    // ---------------------------------------------------------------------
    //
    // `insecure_reason` answers about a path and `read_bounded` opens that
    // path again. Between the two calls the name can be made to point
    // somewhere else, so the file that was judged safe and the file that is
    // read need not be the same one. `read_checked` opens once and judges
    // the handle it holds.

    #[cfg(unix)]
    #[test]
    fn the_trust_check_answers_about_the_opened_file_not_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model-tiers.toml");
        fs::write(&path, "a = 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let opened = open_nonblocking(&path).unwrap();

        // The name is re-pointed at a group-writable file after the open,
        // which is what a race would achieve.
        let other = dir.path().join("other.toml");
        fs::write(&other, "a = 2\n").unwrap();
        fs::set_permissions(&other, fs::Permissions::from_mode(0o666)).unwrap();
        fs::rename(&other, &path).unwrap();

        assert!(
            insecure_reason(&path).is_some(),
            "the path now names a group-writable file"
        );
        assert_eq!(
            insecure_reason_of(&opened),
            None,
            "the handle still holds the private file, and that is the one \
             whose contents a caller would read"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_handle_check_reports_the_same_reasons_as_the_path_check() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.toml");
        fs::write(&path, "a = 1\n").unwrap();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(insecure_reason_of(&open_nonblocking(&path).unwrap()), None);

        for mode in [0o620, 0o602, 0o666] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                insecure_reason_of(&open_nonblocking(&path).unwrap()),
                Some("ignored because the file is writable by group or others"),
                "mode {mode:o}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_checked_refuses_a_group_writable_file_without_reading_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.toml");
        fs::write(&path, "secret = 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        let error = read_checked(&path).expect_err("a group-writable file is refused");
        assert!(error.contains("writable by group or others"), "{error}");
        assert!(!error.contains("secret"), "the reason quotes no contents");
    }

    #[cfg(unix)]
    #[test]
    fn read_checked_still_follows_a_symlink_to_a_private_file() {
        // A tier file kept in a dotfiles directory and linked into place is
        // an ordinary arrangement. What matters is the trust of the file
        // that is actually opened, so the link is followed and then judged.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-model-tiers.toml");
        fs::write(&real, "a = 1\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("model-tiers.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(read_checked(&link).unwrap().as_deref(), Some("a = 1\n"));
    }

    #[cfg(unix)]
    #[test]
    fn read_checked_refuses_a_symlink_to_a_group_writable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("elsewhere.toml");
        fs::write(&real, "a = 1\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o666)).unwrap();
        let link = dir.path().join("model-tiers.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let error = read_checked(&link).expect_err("the target decides, not the link");
        assert!(error.contains("writable by group or others"), "{error}");
    }

    #[test]
    fn read_checked_keeps_every_bound_read_bounded_already_had() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_checked(&dir.path().join("none")), Ok(None));
        assert!(read_checked(dir.path()).is_err());
        let big = dir.path().join("big");
        fs::write(&big, vec![b'#'; 70 * 1024]).unwrap();
        assert!(read_checked(&big).unwrap_err().contains("64 KiB"));
    }

    #[cfg(unix)]
    #[test]
    fn read_checked_does_not_block_on_a_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_checked(&fifo).is_err());
    }
}
