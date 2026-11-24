//! The workstream usage manifest that `governor status` reads for
//! `token_share` (`AMPLIHACK_GOVERNOR_MANIFEST`). Read only; the path is
//! never printed or logged.

use crate::commands::usage::models::{Manifest, Provider};
use std::collections::BTreeMap;
use std::path::Path;

/// The largest manifest read.
const MAX_MANIFEST: u64 = 4096;

/// The manifest workstream's share of the observed tokens: its attributed
/// tokens over all tokens, unattributed ones included. Empty when the
/// manifest is not an absolute path to a regular file owned by this user
/// (symlinks are not followed; permissions are not checked, since the file
/// holds only token counts), names a session whose provider has no usage
/// adapter, or has no observed tokens. The path is never printed or logged.
///
/// The usage report keeps an unsupported-provider session (unknown usage,
/// report incomplete) instead of rejecting the manifest. Its tokens are then
/// missing from both sides of the fraction, so a share would look exact
/// while describing only part of the work; it stays empty, as it did when
/// such a manifest failed to parse.
pub(super) fn token_shares(path: &Path) -> BTreeMap<String, f64> {
    use amplihack_utils::secure_files::{euid, read_bounded_nofollow};
    use std::os::unix::fs::MetadataExt;
    let mut out = BTreeMap::new();
    if !path.is_absolute() {
        return out;
    }
    let raw = match read_bounded_nofollow(path, MAX_MANIFEST) {
        Ok(Some((raw, meta))) if meta.uid() == euid() => raw,
        _ => {
            tracing::debug!("governor status: usage manifest not readable as a file of this user");
            return out;
        }
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&raw) else {
        tracing::debug!("governor status: usage manifest is not valid");
        return out;
    };
    if manifest
        .sessions
        .iter()
        .any(|s| matches!(s.provider, Provider::Unsupported(_)))
    {
        tracing::debug!("governor status: usage manifest names an unsupported provider");
        return out;
    }
    let Some(dir) = path.parent() else {
        return out;
    };
    let Ok(report) = crate::commands::usage::collect(&manifest, dir) else {
        return out;
    };
    let (Some(ws), Some(total)) = (report.workstream.clone(), report.total.map(|t| t.total()))
    else {
        return out;
    };
    let unattributed = report.unattributed.map_or(0, |t| t.total());
    if total > 0 {
        out.insert(ws, total.saturating_sub(unattributed) as f64 / total as f64);
    }
    out
}
