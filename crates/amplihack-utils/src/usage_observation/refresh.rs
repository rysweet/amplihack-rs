//! Metadata receipts for an authorised OAuth refresh. They contain no token,
//! credential body or digest. A receipt can only connect the exact old inode
//! metadata to the exact new inode metadata while preserving its generation.
use super::storage::directory::Directory;
use super::storage::{Stamp, stamp};
use crate::secure_files::{check_private_file, invalid};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    old: Option<Stamp>,
    new: Option<Stamp>,
}
/// Captured while the existing OAuth refresh lock is held, before sending
/// the authorised refresh request. It does not take the rotator lock.
pub struct RefreshCapture {
    path: PathBuf,
    old: Option<Stamp>,
}
pub fn capture_refresh(path: &Path) -> io::Result<RefreshCapture> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("refresh path has no parent"))?;
    let dir = Directory::open_refresh(parent)?;
    match dir.file(".amplihack-usage-refresh.json", false) {
        Ok(_) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e),
    }
    Ok(RefreshCapture {
        path: path.to_path_buf(),
        old: stamp(path)?,
    })
}
/// Publish after the private credential replacement and before releasing
/// the OAuth lock. A crash before this point invalidates evidence safely.
pub fn publish_refresh(capture: RefreshCapture) -> io::Result<()> {
    let parent = capture
        .path
        .parent()
        .ok_or_else(|| invalid("refresh path has no parent"))?;
    let dir = Directory::open_refresh(parent)?;
    let receipt = Receipt {
        schema_version: 1,
        old: capture.old,
        new: stamp(&capture.path)?,
    };
    dir.replace(
        ".amplihack-usage-refresh.json",
        &serde_json::to_vec(&receipt)?,
    )
}
pub(super) fn confirms(
    path: &Path,
    previous: &Option<Stamp>,
    current: &Option<Stamp>,
) -> io::Result<bool> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("refresh path has no parent"))?;
    let receipt = parent.join(".amplihack-usage-refresh.json");
    let Some((bytes, meta)) = crate::secure_files::read_bounded_nofollow(&receipt, 4096)? else {
        return Ok(false);
    };
    check_private_file(&meta)?;
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid usage refresh receipt"))?;
    Ok(receipt.schema_version == 1 && &receipt.old == previous && &receipt.new == current)
}
