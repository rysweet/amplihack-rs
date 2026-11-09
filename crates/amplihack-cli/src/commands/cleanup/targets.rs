//! Cargo target folders tagged with `CACHEDIR.TAG` (issue #1808, D8), and the
//! identity of the pre-commit target of a checkout (issue #2039, D1).
//!
//! Backup tools that honour the Cache Directory Tagging Specification
//! (<https://bford.info/cachedir/>) skip a folder holding this tag.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The name prefix `scripts/git-hook-cargo.sh` gives every target it uses.
pub(crate) const HOOK_TARGET_PREFIX: &str = "amplihack-precommit-target-";

/// Hex characters of the checkout hash in a target name.
const CHECKOUT_HASH_LEN: usize = 12;

/// Profile folders the hook's own `is_cargo_target` looks in.
const PROFILES: [&str; 2] = ["debug", "release"];

/// The standard signature line plus a comment naming amplihack.
pub(crate) const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
# This file is a cache directory tag created by amplihack.\n\
# For information about cache directory tags see https://bford.info/cachedir/\n";

/// Length of the `Signature: <hash>` line that starts every valid tag.
const SIGNATURE_LEN: usize = 43;

/// Creates `path` (and its parents) and writes `CACHEDIR.TAG` into it. The
/// Rust twin of the tagging in `scripts/git-hook-cargo.sh`, for code that
/// creates a target folder.
///
/// Calling it again is harmless: an existing tag is left alone and nothing
/// else in the folder is touched.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn create_tagged_target_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    let tag = path.join("CACHEDIR.TAG");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o644)
        .open(&tag)
    {
        Ok(mut file) => file.write_all(CACHEDIR_TAG.as_bytes()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// True only when `path/CACHEDIR.TAG` is a regular file (not a symlink) that
/// starts with the standard signature.
pub(crate) fn is_tagged(path: &Path) -> bool {
    let tag = path.join("CACHEDIR.TAG");
    let Ok(meta) = fs::symlink_metadata(&tag) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tag)
    else {
        return false;
    };
    let signature = &CACHEDIR_TAG.as_bytes()[..SIGNATURE_LEN];
    let mut head = Vec::with_capacity(SIGNATURE_LEN);
    if file
        .take(SIGNATURE_LEN as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    head == signature
}

/// True when `dir` is a real folder (not a link) that looks like a Cargo
/// target: a valid `CACHEDIR.TAG`, a regular `.rustc_info.json`, or a
/// `<profile>/.fingerprint` folder or `<profile>/.cargo-lock` file. The Rust
/// twin of `is_cargo_target` in `scripts/git-hook-cargo.sh`, except that no
/// link is followed at any level.
pub(crate) fn is_cargo_target(dir: &Path) -> bool {
    let is = |path: &Path, want_dir: bool| {
        fs::symlink_metadata(path).is_ok_and(|m| {
            if want_dir {
                m.file_type().is_dir()
            } else {
                m.file_type().is_file()
            }
        })
    };
    if !is(dir, true) {
        return false;
    }
    if is_tagged(dir) || is(&dir.join(".rustc_info.json"), false) {
        return true;
    }
    PROFILES.iter().any(|profile| {
        let profile = dir.join(profile);
        is(&profile, true)
            && (is(&profile.join(".fingerprint"), true) || is(&profile.join(".cargo-lock"), false))
    })
}

/// The path git prints for `checkout` with `rev-parse --show-toplevel`: the
/// canonical path, or for a checkout that no longer exists the canonical
/// parent joined with the last component. `None` when the parent cannot be
/// resolved or there is no last component.
fn canonical_checkout(checkout: &Path) -> Option<PathBuf> {
    let name = checkout.file_name()?;
    if let Ok(canonical) = fs::canonicalize(checkout) {
        return Some(canonical);
    }
    let parent = fs::canonicalize(checkout.parent()?).ok()?;
    Some(parent.join(name))
}

/// The first 12 lowercase hex characters of the SHA-256 of the canonical
/// checkout path, with no trailing newline: the hook's
/// `printf '%s' "$top" | sha256sum`.
pub(crate) fn checkout_hash(checkout: &Path) -> Option<String> {
    let canonical = canonical_checkout(checkout)?;
    let digest = Sha256::digest(canonical.as_os_str().as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some(hex[..CHECKOUT_HASH_LEN].to_string())
}

/// Whether `name` is `amplihack-precommit-target-` plus 12 lowercase hex
/// characters, the only name a target path may be built from.
pub(crate) fn is_hook_target_name(name: &str) -> bool {
    name.strip_prefix(HOOK_TARGET_PREFIX).is_some_and(|hash| {
        hash.len() == CHECKOUT_HASH_LEN
            && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// The hook's target name for `checkout`, checked against
/// `^amplihack-precommit-target-[0-9a-f]{12}$` before any caller joins it.
pub(crate) fn target_name(checkout: &Path) -> Option<String> {
    let name = format!("{HOOK_TARGET_PREFIX}{}", checkout_hash(checkout)?);
    is_hook_target_name(&name).then_some(name)
}

#[cfg(test)]
#[path = "tests_targets.rs"]
mod tests;
