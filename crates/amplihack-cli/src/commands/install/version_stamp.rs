//! Per-binary install version stamp.
//!
//! Records the amplihack binary version that last successfully staged
//! framework assets into `~/.amplihack`. Read on startup by the
//! [`crate::self_heal`] module to detect drift between the running binary
//! and the staged assets, and re-stage automatically when they disagree.
//!
//! The stamp file lives at `~/.amplihack/.installed-version` and contains
//! a single line — the value of [`crate::VERSION`] — with no trailing
//! newline. Writes are atomic via a sibling tempfile + `rename` (the same
//! pattern used by `write_layout_marker` in this crate).

use anyhow::{Context, Result, bail};
use regex::Regex;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use super::paths::home_dir;

/// Filename of the stamp inside `~/.amplihack/`.
const STAMP_FILE: &str = ".installed-version";

/// Sibling tempfile used for atomic writes.
const STAMP_TMP: &str = ".installed-version.tmp";

/// Permitted stamp content per issue #502 R3: `MAJOR.MINOR.PATCH` with an
/// optional `-PRERELEASE` tag. Build metadata (`+...`) is intentionally
/// rejected — the stamp must round-trip `crate::VERSION`, which never
/// carries build metadata.
const SEMVER_PATTERN: &str = r"^\d+\.\d+\.\d+(-[\w.]+)?$";

fn semver_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(SEMVER_PATTERN).expect("hard-coded SEMVER_PATTERN must compile"))
}

/// Resolve the absolute path of the install version stamp file.
///
/// Returns `Err` if `HOME` is unset (matches the failure mode of every other
/// path helper in this crate; we do not silently fall back to the current
/// directory).
pub(crate) fn installed_version_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(STAMP_FILE))
}

/// Atomically write the stamp file with `version` as its sole contents.
///
/// Creates `~/.amplihack/` (with parents) if missing. Writes to a sibling
/// `.installed-version.tmp` first and then renames into place, so a crashed
/// or interrupted write never produces a torn read.
///
/// Issue #502 R1/R2 hardening:
/// - Refuses to follow / overwrite a symlink at the final stamp path
///   (uses `symlink_metadata`, not `metadata`, so the check is not fooled
///   by `lstat`-vs-`stat` semantics).
/// - On Unix, the stamp file is `chmod 0o600` after write. Failure to set
///   permissions propagates as `Err` (Zero-BS — we do not silently leave a
///   world-readable file behind).
pub(crate) fn write_installed_version(version: &str) -> Result<()> {
    let final_path = installed_version_path()?;
    let parent = final_path.parent().with_context(|| {
        format!(
            "version stamp path has no parent directory: {}",
            final_path.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    // R1: refuse to overwrite a symlink at the stamp path. We do this
    // BEFORE writing the tempfile so an attacker symlink cannot redirect
    // a subsequent rename. `fs::rename` on Unix replaces atomically and
    // would clobber the symlink target if we proceeded.
    match fs::symlink_metadata(&final_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!(
                "refusing to overwrite symlink at stamp path: {final_path:?} \
                 (delete it manually after verifying it is not malicious)"
            );
        }
        Ok(_) | Err(_) => {
            // Regular file (will be replaced by rename) or NotFound — both fine.
        }
    }

    let tmp_path = parent.join(STAMP_TMP);
    fs::write(&tmp_path, version.as_bytes())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path).with_context(|| {
        format!(
            "failed to rename {} to {}",
            tmp_path.display(),
            final_path.display()
        )
    })?;

    // R2: post-write chmod 0o600 on Unix. Failure propagates loudly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&final_path, perms).with_context(|| {
            format!(
                "failed to set 0o600 permissions on {}",
                final_path.display()
            )
        })?;
    }

    Ok(())
}

/// Read the stamp file, returning `Ok(None)` when it does not exist OR when
/// its contents fail the strict semver validation (issue #502 R3/R4).
///
/// On malformed contents we emit a one-line stderr warning and return
/// `Ok(None)`. Treating malformed-as-missing forces a re-stage on next
/// startup, which is the desired self-heal behaviour. Returns `Err` for any
/// IO error other than `NotFound` so corruption fails loud.
pub(crate) fn read_installed_version() -> Result<Option<String>> {
    let path = installed_version_path()?;
    let raw = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let trimmed = raw.trim();
    if !semver_regex().is_match(trimmed) {
        // R4: one-line warning so operators see the rejection in CI logs.
        // Use `{:?}` so embedded newlines / control chars cannot forge a
        // second log line or inject terminal escapes.
        eprintln!(
            "amplihack: ignoring malformed install stamp at {} (contents={:?}); will re-stage",
            path.display(),
            trimmed
        );
        return Ok(None);
    }
    Ok(Some(trimmed.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::TempDir;

    /// Save/restore guard for `HOME`. Drops always restore, even on panic.
    /// Uses the crate-wide [`crate::self_heal::test_support::ENV_LOCK`] so
    /// that `version_stamp` and `self_heal` tests cannot race each other —
    /// both mutate the process-wide `HOME` env var.
    struct HomeGuard {
        prior: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &std::path::Path) -> Self {
            let lock = crate::test_support::env_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prior = std::env::var_os("HOME");
            // SAFETY: edition 2024 requires unsafe for env mutation;
            // serialized via the crate-wide env_lock above.
            unsafe { std::env::set_var("HOME", home) };
            HomeGuard { prior, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: serialized via ENV_LOCK held in self._lock.
            unsafe {
                match self.prior.take() {
                    Some(prev) => std::env::set_var("HOME", prev),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    #[test]
    fn write_then_read_round_trip() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.111").expect("write");
        let read = read_installed_version().expect("read");
        assert_eq!(read.as_deref(), Some("0.8.111"));
    }

    #[test]
    fn read_returns_none_when_file_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let read = read_installed_version().expect("read");
        assert_eq!(read, None);
    }

    #[test]
    fn write_overwrites_existing_stamp() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.110").expect("first write");
        write_installed_version("0.8.111").expect("second write");
        let read = read_installed_version().expect("read");
        assert_eq!(read.as_deref(), Some("0.8.111"));
    }

    #[test]
    fn write_creates_amplihack_dir_if_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());
        // Note: ~/.amplihack does not exist yet inside the tempdir.
        assert!(!tmp.path().join(".amplihack").exists());

        write_installed_version("0.9.0").expect("write");
        assert!(tmp.path().join(".amplihack").exists());
        assert!(tmp.path().join(".amplihack").join(STAMP_FILE).exists());
    }

    #[test]
    fn write_contents_have_no_trailing_newline() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.111").expect("write");
        let raw = fs::read(installed_version_path().unwrap()).unwrap();
        assert_eq!(raw, b"0.8.111");
    }

    #[test]
    fn read_trims_whitespace_for_compat() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "0.8.111\n").unwrap();

        let read = read_installed_version().expect("read");
        assert_eq!(read.as_deref(), Some("0.8.111"));
    }

    #[test]
    fn installed_version_path_under_home() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().expect("path");
        assert_eq!(path, tmp.path().join(".amplihack").join(STAMP_FILE));
    }

    // ----- TDD tests for issue #502 hardening -----
    //
    // These tests pin the contract for the security/concurrency
    // requirements that PR #500 deferred. They fail until the
    // implementation is added to this module.

    /// R1: writing the stamp must REFUSE to follow / overwrite a symlink
    /// at the stamp path. The symlink is left in place; the write
    /// returns Err. Caller (self_heal) is expected to surface the
    /// error and abort — never silently delete or follow.
    #[test]
    #[cfg(unix)]
    fn write_refuses_symlink_at_stamp_path() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());
        let amp = tmp.path().join(".amplihack");
        fs::create_dir_all(&amp).unwrap();

        let target = tmp.path().join("attacker-target");
        fs::write(&target, b"sensitive").unwrap();
        let stamp = amp.join(STAMP_FILE);
        std::os::unix::fs::symlink(&target, &stamp).unwrap();

        let err = write_installed_version("0.9.0").expect_err("must refuse symlink");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("symlink"),
            "error must mention symlink; got: {msg}"
        );

        // Symlink and target must be untouched.
        let meta = fs::symlink_metadata(&stamp).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "symlink must not be deleted or replaced"
        );
        assert_eq!(
            fs::read(&target).unwrap(),
            b"sensitive",
            "symlink target must not be overwritten"
        );
    }

    /// R2: the stamp file must be persisted with mode 0o600 on Unix.
    /// Default umask could leave it world-readable; an explicit chmod
    /// is required and any failure to set permissions must propagate
    /// (Zero-BS — no silent let _ = ).
    #[test]
    #[cfg(unix)]
    fn stamp_mode_is_0o600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.111").expect("write");
        let mode = fs::metadata(installed_version_path().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "stamp must be owner-only (0o600), got {mode:o}"
        );
    }

    /// R3+R4: malformed stamp contents (failing the semver regex) must
    /// be treated as "no prior install" — read returns Ok(None) and a
    /// one-line stderr warning is emitted. Test with a deliberately
    /// invalid value.
    #[test]
    fn read_rejects_malformed_semver() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not-a-version").unwrap();

        let read = read_installed_version().expect("read should not Err on malformed");
        assert_eq!(
            read, None,
            "malformed semver must be reported as no-prior-install"
        );
    }

    /// R3: shell-injection style content must also be rejected (control
    /// chars, newlines, path separators).
    #[test]
    fn read_rejects_shell_injection_content() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"0.8.111;rm -rf /").unwrap();

        let read = read_installed_version().expect("read");
        assert_eq!(read, None, "injected content must be rejected");
    }

    /// R3: build-metadata (`+...`) is intentionally NOT accepted per
    /// the docs decision. Only `^\d+\.\d+\.\d+(-[\w.]+)?$`.
    #[test]
    fn read_rejects_build_metadata() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"0.8.111+build.1").unwrap();

        assert_eq!(read_installed_version().expect("read"), None);
    }

    /// R3: prerelease tags MUST be accepted (e.g. `0.9.0-rc1`).
    #[test]
    fn read_accepts_prerelease_semver() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"0.9.0-rc1").unwrap();

        assert_eq!(
            read_installed_version().expect("read").as_deref(),
            Some("0.9.0-rc1")
        );
    }

    /// R3: well-formed plain semver still works (regression guard).
    #[test]
    fn read_accepts_plain_semver_after_validation_added() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("1.2.3").expect("write");
        assert_eq!(
            read_installed_version().expect("read").as_deref(),
            Some("1.2.3")
        );
    }
}
