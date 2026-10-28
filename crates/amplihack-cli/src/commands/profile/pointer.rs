//! `<root>/.default`: a file holding only the default profile name.

use super::ProfileError;
use super::name::ProfileName;
use super::store::check_root_not_symlink;
use std::fs;
use std::io::Read;
use std::path::Path;

const FILE: &str = ".default";
const MAX_BYTES: u64 = 256;

pub fn read(root: &Path) -> Result<Option<ProfileName>, ProfileError> {
    check_root_not_symlink(root)?;
    let path = root.join(FILE);
    let meta = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !meta.file_type().is_file() {
        return Err(ProfileError::PointerCorrupt("not a regular file".into()));
    }
    let mut buf = Vec::new();
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut opts, libc::O_NOFOLLOW);
    opts.open(&path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_BYTES {
        return Err(ProfileError::PointerCorrupt("file too large".into()));
    }
    if std::str::from_utf8(&buf).is_err() {
        return Err(ProfileError::PointerCorrupt("not valid UTF-8".into()));
    }
    let name = parse_name(&buf)
        .ok_or_else(|| ProfileError::PointerCorrupt("not a valid profile name".into()))?;
    match fs::symlink_metadata(root.join(name.as_str())) {
        Ok(m) if m.file_type().is_dir() => Ok(Some(name)),
        _ => Err(ProfileError::PointerDangling(name.to_string())),
    }
}

/// The name in a one-name file: UTF-8, at most one trailing newline, and a
/// valid profile name. Shared with the rotation pin (issue #1691).
pub(crate) fn parse_name(bytes: &[u8]) -> Option<ProfileName> {
    let text = std::str::from_utf8(bytes).ok()?;
    ProfileName::parse(text.strip_suffix('\n').unwrap_or(text)).ok()
}

pub fn write(root: &Path, name: &ProfileName) -> Result<(), ProfileError> {
    use std::io::Write;
    check_root_not_symlink(root)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = root.join(format!(".default.tmp.{}.{nanos}", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let result = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        writeln!(f, "{name}")?;
        f.sync_all()?;
        fs::rename(&tmp, root.join(FILE))
    })();
    if let Err(e) = result {
        if let Err(cleanup) = fs::remove_file(&tmp)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: profile pointer rollback failed to remove {}: {cleanup}",
                tmp.display()
            );
        }
        return Err(e.into());
    }
    Ok(())
}

/// Whether anything (valid or not) is at the pointer's path.
pub fn exists(root: &Path) -> bool {
    fs::symlink_metadata(root.join(FILE)).is_ok()
}

pub fn clear(root: &Path) -> Result<(), ProfileError> {
    check_root_not_symlink(root)?;
    match fs::remove_file(root.join(FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
