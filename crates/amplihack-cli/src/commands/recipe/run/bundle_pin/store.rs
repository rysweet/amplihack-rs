//! Issue #2528: the content-keyed snapshot store under
//! `<work root>/bundle-pins`.
//!
//! A snapshot lives at `bundle-pins/<key>`, where `<key>` is derived from the
//! choice, the canonical source, the version and a digest of the copied tree.
//! A relaunch of an unchanged bundle therefore gets the same paths, so a
//! resumed run passes the runner the same `AMPLIHACK_HOME`, `-R` and
//! `--agent-dir` as the original run (#1741).
//!
//! Each run holds a shared `flock` on `<key>.use` for as long as it uses the
//! snapshot; the last launch to release it removes the snapshot.
//! `bundle-pins/.lock` serialises pinning, lease release and the sweep of
//! leftovers. Every lock is a blocking or non-blocking `flock`: no timer,
//! retry, thread or process is involved.
//!
//! Reference: `docs/reference/recipe-run-bundle.md`.

use anyhow::{Context, Result, bail};
use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The snapshot store inside a work root.
pub(super) const PINS_DIR: &str = "bundle-pins";
/// Held exclusively while a launch pins or sweeps, or a lease is released.
const LOCK_FILE: &str = ".lock";
/// Suffix of a snapshot's lease file.
const LEASE_SUFFIX: &str = ".use";
/// Prefix of a copy in progress.
const TMP_PREFIX: &str = ".tmp-";
/// Format tag, the first part of every key preimage.
const KEY_FORMAT: &[u8] = b"v1";
/// Hex digits in a key (96 bits of the SHA-256 digest).
const KEY_HEX_LEN: usize = 24;
/// The bits a tree digest keeps from a mode: everything but the write bits,
/// so a fresh 0755/0644 copy and a 0555/0444 snapshot of it give one digest.
const DIGEST_MODE_MASK: u32 = 0o7555;

/// A 32-byte tree digest.
pub(super) type TreeDigest = [u8; 32];

/// True for exactly 24 lower-case hex digits.
pub(super) fn is_key(name: &str) -> bool {
    name.len() == KEY_HEX_LEN && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// True for `.tmp-<uuid v4>` in lower-case hyphenated form.
fn is_tmp_name(name: &str) -> bool {
    name.strip_prefix(TMP_PREFIX)
        .and_then(|rest| {
            uuid::Uuid::parse_str(rest)
                .ok()
                .filter(|id| id.get_version_num() == 4 && id.hyphenated().to_string() == rest)
        })
        .is_some()
}

/// A fresh `.tmp-<uuid v4>` name.
pub(super) fn new_tmp_name() -> String {
    format!("{TMP_PREFIX}{}", uuid::Uuid::new_v4().hyphenated())
}

/// `<pins>/<key>.use`.
fn lease_path(pins: &Path, key: &str) -> PathBuf {
    pins.join(format!("{key}{LEASE_SUFFIX}"))
}

/// The snapshot key: the first 24 hex digits of SHA-256 over the format tag,
/// the choice, the canonical source, the version and the tree digest, each
/// preceded by its length as an 8-byte big-endian integer.
pub(super) fn snapshot_key(
    choice: &str,
    source: &Path,
    version: &str,
    digest: &TreeDigest,
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        KEY_FORMAT,
        choice.as_bytes(),
        source.as_os_str().as_bytes(),
        version.as_bytes(),
        digest.as_slice(),
    ] {
        put(&mut hasher, part);
    }
    hasher
        .finalize()
        .iter()
        .take(KEY_HEX_LEN / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn put(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// The tree digest of the bundle folder `root`: entries in byte order of
/// their names, depth first, never following a symlink. Each entry feeds its
/// relative path, a type tag (`d`, `f` or `l`) and `mode & 0o7555`; a file
/// then feeds its length and bytes, a symlink its link text. Any other type
/// of entry is an error.
pub(super) fn tree_digest(root: &Path) -> Result<TreeDigest> {
    let meta =
        fs::symlink_metadata(root).with_context(|| format!("cannot inspect {}", root.display()))?;
    if !meta.is_dir() {
        bail!("cannot digest {}: not a real directory", root.display());
    }
    let mut hasher = Sha256::new();
    digest_dir(&mut hasher, root, Path::new(""))?;
    Ok(hasher.finalize().into())
}

fn digest_dir(hasher: &mut Sha256, dir: &Path, relative: &Path) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("cannot read {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let rel = relative.join(entry.file_name());
        let meta = fs::symlink_metadata(&path)
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        let kind = meta.file_type();
        let tag: &[u8] = if kind.is_symlink() {
            b"l"
        } else if kind.is_dir() {
            b"d"
        } else if kind.is_file() {
            b"f"
        } else {
            bail!(
                "cannot digest {}: not a regular file, directory or symlink",
                path.display()
            );
        };
        put(hasher, rel.as_os_str().as_bytes());
        hasher.update(tag);
        hasher.update((meta.mode() & DIGEST_MODE_MASK).to_be_bytes());
        if kind.is_symlink() {
            let text = fs::read_link(&path)
                .with_context(|| format!("cannot read link {}", path.display()))?;
            put(hasher, text.as_os_str().as_bytes());
        } else if kind.is_dir() {
            digest_dir(hasher, &path, &rel)?;
        } else {
            digest_file(hasher, &path, &meta)?;
        }
    }
    Ok(())
}

/// Feeds the length and bytes of the file `checked` describes, read from a
/// handle opened with `O_NOFOLLOW` that must be that same file.
fn digest_file(hasher: &mut Sha256, path: &Path, checked: &fs::Metadata) -> Result<()> {
    let changed = || anyhow::anyhow!("{} changed while being digested", path.display());
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let opened = file
        .metadata()
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    if !opened.is_file() || opened.dev() != checked.dev() || opened.ino() != checked.ino() {
        return Err(changed());
    }
    let length = opened.len();
    hasher.update(length.to_be_bytes());
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    if total != length {
        return Err(changed());
    }
    Ok(())
}

/// Clears every write bit in the tree at `root`; symlinks are left alone.
fn make_read_only(root: &Path) -> Result<()> {
    let meta =
        fs::symlink_metadata(root).with_context(|| format!("cannot inspect {}", root.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in
            fs::read_dir(root).with_context(|| format!("cannot read {}", root.display()))?
        {
            let entry = entry.with_context(|| format!("cannot read {}", root.display()))?;
            make_read_only(&entry.path())?;
        }
    }
    let mode = meta.mode() & 0o7777 & !0o222;
    fs::set_permissions(root, fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set mode {mode:04o} on {}", root.display()))
}

/// Gives the owner full access to every real directory in the tree at
/// `root`, so it can be removed. Symlinks are never followed or changed.
fn restore_owner_access(root: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", root.display()));
        }
    };
    if !meta.is_dir() {
        return Ok(());
    }
    let mode = (meta.mode() & 0o7777) | 0o700;
    fs::set_permissions(root, fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set mode {mode:04o} on {}", root.display()))?;
    for entry in fs::read_dir(root).with_context(|| format!("cannot read {}", root.display()))? {
        let entry = entry.with_context(|| format!("cannot read {}", root.display()))?;
        restore_owner_access(&entry.path())?;
    }
    Ok(())
}

/// Removes `path`: a symlink or file is unlinked, a real directory has its
/// owner access restored and is removed with everything in it. A missing
/// path is not an error.
pub(super) fn remove_entry(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    let removed = if meta.is_dir() {
        restore_owner_access(path)?;
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match removed {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot remove {}", path.display())),
    }
}

/// Creates `bundle-pins/` 0700 if missing, then checks it with
/// `trusted_directory` whether it was created now or not (E12).
pub(super) fn ensure_pins_dir(pins: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(pins) {
        Ok(()) => fs::set_permissions(pins, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot set mode 0700 on {}", pins.display()))?,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot create {}", pins.display()));
        }
    }
    crate::commands::orch::autodrive::trusted::trusted_directory(pins).map_err(|error| {
        anyhow::anyhow!(
            "untrusted bundle snapshot directory {}: {error:#}",
            pins.display()
        )
    })
}

/// Opens a lock file with `O_NOFOLLOW` (and `O_CLOEXEC`, as every std open
/// does), creating it 0600 when `create` is set, and checks the handle is a
/// regular file owned by the effective user (E12).
fn open_lock_file(pins: &Path, path: &Path, create: bool) -> std::io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                std::io::Error::other(format!(
                    "untrusted bundle snapshot directory {}: {} is a symlink",
                    pins.display(),
                    path.display()
                ))
            } else {
                error
            }
        })?;
    let meta = file.metadata()?;
    // SAFETY: geteuid has no failure mode and touches no memory.
    let euid = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.uid() != euid {
        return Err(std::io::Error::other(format!(
            "untrusted bundle snapshot directory {}: {} is not a regular file owned by uid {euid}",
            pins.display(),
            path.display()
        )));
    }
    Ok(file)
}

fn lock_error(path: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!("cannot lock {}: {error}", path.display())
}

/// A non-blocking exclusive `flock`: `Ok(false)` only when it would block.
fn try_exclusive(file: &fs::File, path: &Path) -> Result<bool> {
    match FileExt::try_lock_exclusive(file) {
        Ok(granted) => Ok(granted),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(lock_error(path, error)),
    }
}

/// `bundle-pins/.lock`, held exclusively until dropped.
#[derive(Debug)]
pub(super) struct StoreLock {
    _file: fs::File,
}

impl StoreLock {
    /// Takes `.lock` exclusively. One non-blocking attempt first; if another
    /// launch holds it, one line on stderr names the file, then this waits.
    /// With `create` unset, a missing `.lock` gives `Ok(None)`.
    pub(super) fn acquire(pins: &Path, create: bool) -> Result<Option<Self>> {
        let path = pins.join(LOCK_FILE);
        let file = match open_lock_file(pins, &path, create) {
            Ok(file) => file,
            Err(error) if !create && error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(anyhow::Error::new(error))
                    .with_context(|| format!("cannot open {}", path.display()));
            }
        };
        if !try_exclusive(&file, &path)? {
            eprintln!(
                "recipe bundle: waiting for {} (another run holds the bundle snapshot lock)",
                path.display()
            );
            FileExt::lock_exclusive(&file).map_err(|error| lock_error(&path, error))?;
        }
        Ok(Some(Self { _file: file }))
    }
}

/// Whether some run holds a lease file.
enum Probe {
    /// No run holds it. The handle, if any, keeps the probe lock.
    Free(Option<fs::File>),
    Held,
}

/// Tests a lease file with a non-blocking exclusive `flock` on a fresh open.
/// A missing file is free; a symlink is unlinked, never followed, and free.
fn probe_lease(pins: &Path, path: &Path) -> Result<Probe> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Probe::Free(None)),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            remove_entry(path)?;
            return Ok(Probe::Free(None));
        }
        Ok(_) => {}
    }
    let file = match open_lock_file(pins, path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Probe::Free(None)),
        Err(error) => {
            return Err(anyhow::Error::new(error))
                .with_context(|| format!("cannot open {}", path.display()));
        }
    };
    if try_exclusive(&file, path)? {
        Ok(Probe::Free(Some(file)))
    } else {
        Ok(Probe::Held)
    }
}

/// What step 6 found at `<key>/`.
pub(super) enum Placement {
    /// `<key>/` was missing; the copy was renamed into place.
    Created,
    /// `<key>/` held the same tree; the copy is left for the caller to remove.
    Reused,
    /// `<key>/` differed and no run used it; it was replaced by the copy.
    Replaced,
}

/// Step 6: puts the copy at `tmp` (holding `amplifier-bundle/` with tree
/// digest `digest`, and `pin.json`) in place as `<pins>/<key>`, reusing an
/// identical snapshot. Must be called with `.lock` held. The placed
/// `<key>/` folder is read-only throughout, its top folder included, so a
/// step cannot write into a snapshot that concurrent runs share; removal
/// goes through [`remove_entry`], which restores the owner's access first.
pub(super) fn place(
    pins: &Path,
    key: &str,
    tmp: &Path,
    digest: &TreeDigest,
    source: &Path,
) -> Result<Placement> {
    let home = pins.join(key);
    let existing = match fs::symlink_metadata(&home) {
        Ok(meta) => Some(meta),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", home.display()));
        }
    };
    let Some(existing) = existing else {
        make_read_only(tmp)?;
        fs::rename(tmp, &home)
            .with_context(|| format!("cannot rename {} to {}", tmp.display(), home.display()))?;
        return Ok(Placement::Created);
    };
    // The whole `<key>/` folder is checked again before reuse (S1): it must
    // hold nothing but the bundle and its pin record, and the bundle's digest
    // is computed again from its files. One that cannot be read or digested
    // counts as different.
    let same = existing.is_dir()
        && holds_only_bundle(&home)
        && tree_digest(&home.join(super::BUNDLE_DIR))
            .ok()
            .is_some_and(|found| found == *digest);
    if same {
        // The digest ignores write bits, so a reused tree may have been made
        // writable since it was placed; the whole `<key>/` folder is made
        // read-only again.
        make_read_only(&home)?;
        return Ok(Placement::Reused);
    }
    let _probe = match probe_lease(pins, &lease_path(pins, key))? {
        Probe::Held => bail!(
            "bundle snapshot {} is in use by another run and its contents differ from {}; \
             wait for that run to end",
            home.display(),
            source.display()
        ),
        Probe::Free(handle) => handle,
    };
    remove_entry(&home)?;
    make_read_only(tmp)?;
    fs::rename(tmp, &home)
        .with_context(|| format!("cannot rename {} to {}", tmp.display(), home.display()))?;
    Ok(Placement::Replaced)
}

/// True when `home` holds exactly a real `amplifier-bundle/` folder and a
/// regular `pin.json`, neither a symlink. Steps read files directly under
/// `AMPLIHACK_HOME` (such as `config` or `.claude/`), so a snapshot holding
/// anything else is not the snapshot that was placed. A folder that cannot
/// be read gives false.
fn holds_only_bundle(home: &Path) -> bool {
    let Ok(entries) = fs::read_dir(home) else {
        return false;
    };
    let mut found = 0;
    for entry in entries {
        // `DirEntry::file_type` does not follow symlinks.
        let Ok((name, kind)) = entry.and_then(|e| Ok((e.file_name(), e.file_type()?))) else {
            return false;
        };
        let expected = if name == super::BUNDLE_DIR {
            kind.is_dir()
        } else if name == super::PIN_RECORD {
            kind.is_file()
        } else {
            false
        };
        if !expected {
            return false;
        }
        found += 1;
    }
    found == 2
}

/// Step 3: removes copies in progress left by killed launches. Must be
/// called with `.lock` held, so no live launch has a copy in progress.
pub(super) fn clean_stale_copies(pins: &Path) {
    for name in entry_names(pins) {
        if is_tmp_name(&name) {
            let path = pins.join(&name);
            if let Err(error) = remove_entry(&path) {
                eprintln!(
                    "recipe bundle: could not remove the stale copy {}: {error:#}",
                    path.display()
                );
            }
        }
    }
}

/// Step 8: removes the snapshots and lease files of other keys that no run
/// holds. Must be called with `.lock` held. Acts only on key-shaped names
/// and never follows a symlink.
pub(super) fn sweep(pins: &Path, own_key: &str) {
    let names = entry_names(pins);
    let own_lease = format!("{own_key}{LEASE_SUFFIX}");
    let keys: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| is_key(name) && *name != own_key)
        .collect();
    let lone_leases = names.iter().filter_map(|name| {
        let key = name.strip_suffix(LEASE_SUFFIX)?;
        (is_key(key) && *name != own_lease && !keys.contains(&key)).then_some(key)
    });
    for key in keys.iter().copied() {
        if let Err(error) = sweep_key(pins, key) {
            eprintln!(
                "recipe bundle: could not remove the unused snapshot {}: {error:#}",
                pins.join(key).display()
            );
        }
    }
    for key in lone_leases {
        let path = lease_path(pins, key);
        let outcome = probe_lease(pins, &path).and_then(|probe| match probe {
            Probe::Free(Some(_handle)) => remove_entry(&path),
            Probe::Free(None) | Probe::Held => Ok(()),
        });
        if let Err(error) = outcome {
            eprintln!(
                "recipe bundle: could not remove the unused lease file {}: {error:#}",
                path.display()
            );
        }
    }
}

fn sweep_key(pins: &Path, key: &str) -> Result<()> {
    let home = pins.join(key);
    let lease = lease_path(pins, key);
    let meta = match fs::symlink_metadata(&home) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", home.display()));
        }
    };
    if meta.file_type().is_symlink() {
        remove_entry(&home)?;
    }
    match probe_lease(pins, &lease)? {
        Probe::Held => Ok(()),
        Probe::Free(_handle) => {
            remove_entry(&home)?;
            remove_entry(&lease)
        }
    }
}

/// Names of the entries of `dir` that are valid UTF-8; unreadable folders
/// give none, with a warning.
fn entry_names(dir: &Path) -> Vec<String> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
        Err(error) => {
            eprintln!("recipe bundle: cannot read {}: {error}", dir.display());
            Vec::new()
        }
    }
}

/// A run's shared lock on `<key>.use`. Dropping a launch's lease removes the
/// snapshot when no other run holds it; a nested run's lease never removes
/// anything.
#[derive(Debug)]
pub(super) struct PinLease {
    pins: PathBuf,
    home: PathBuf,
    path: PathBuf,
    file: Option<fs::File>,
    remove_on_drop: bool,
}

impl PinLease {
    /// Step 7: opens `<key>.use`, creating it 0600 if missing, and takes a
    /// shared lock on it. Must be called with `.lock` held.
    pub(super) fn take(pins: &Path, key: &str) -> Result<Self> {
        let path = lease_path(pins, key);
        let file = open_lock_file(pins, &path, true)
            .with_context(|| format!("cannot open {}", path.display()))?;
        FileExt::lock_shared(&file).map_err(|error| lock_error(&path, error))?;
        Ok(Self {
            pins: pins.to_path_buf(),
            home: pins.join(key),
            path,
            file: Some(file),
            remove_on_drop: true,
        })
    }

    /// A nested run's lease on the outer run's snapshot `home`: takes the
    /// existing `.lock` and opens the existing `<key>.use`, creating neither
    /// (E13 when either is missing), takes a shared lock and releases
    /// `.lock`.
    pub(super) fn join(home: &Path) -> Result<Self> {
        let (Some(pins), Some(key)) = (home.parent(), home.file_name().and_then(|n| n.to_str()))
        else {
            bail!("inherited bundle snapshot {} has no parent", home.display());
        };
        let path = lease_path(pins, key);
        let ended = || {
            anyhow::anyhow!(
                "inherited bundle snapshot {} has no lease file {}; the run that pinned it has ended",
                home.display(),
                path.display()
            )
        };
        let _lock = StoreLock::acquire(pins, false)?.ok_or_else(|| {
            anyhow::anyhow!(
                "inherited bundle snapshot {} has no lock file {}; the run that pinned it has ended",
                home.display(),
                pins.join(LOCK_FILE).display()
            )
        })?;
        let file = match open_lock_file(pins, &path, false) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Err(ended()),
            Err(error) => {
                return Err(anyhow::Error::new(error))
                    .with_context(|| format!("cannot open {}", path.display()));
            }
        };
        FileExt::lock_shared(&file).map_err(|error| lock_error(&path, error))?;
        // The lease file must still be the one at the path, beside the
        // snapshot: a release that removed both in between leaves neither.
        let opened = file
            .metadata()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        let current = fs::symlink_metadata(&path).ok();
        let same = current.is_some_and(|m| m.dev() == opened.dev() && m.ino() == opened.ino());
        if !same || !home.is_dir() {
            return Err(ended());
        }
        Ok(Self {
            pins: pins.to_path_buf(),
            home: home.to_path_buf(),
            path,
            file: Some(file),
            remove_on_drop: false,
        })
    }

    /// Takes `.lock`, then, when no other run holds `<key>.use`, removes the
    /// snapshot and its lease file.
    fn release_last(&self, file: fs::File) -> Result<()> {
        let _lock = StoreLock::acquire(&self.pins, true)?;
        // Converting this run's own shared lock with a non-blocking request
        // gives up the shared lock either way; it is granted only when no
        // other run holds the file.
        if !try_exclusive(&file, &self.path)? {
            return Ok(());
        }
        remove_entry(&self.home)?;
        remove_entry(&self.path)?;
        drop(file);
        Ok(())
    }
}

impl Drop for PinLease {
    fn drop(&mut self) {
        let Some(file) = self.file.take() else {
            return;
        };
        if !self.remove_on_drop {
            return;
        }
        if let Err(error) = self.release_last(file) {
            eprintln!(
                "recipe bundle: could not remove {}; a later launch removes it: {error:#}",
                self.home.display()
            );
        }
    }
}
