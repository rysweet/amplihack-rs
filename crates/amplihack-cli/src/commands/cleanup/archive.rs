//! Log archive and restore (issue #1808, C5, D10, R5).
//!
//! Each log goes into its own `.tar.gz` with one regular entry, plus a line in
//! `MANIFEST.jsonl`. The original is removed only after the archive has been
//! read back and its bytes compared. Restore never unpacks a tar: it reads the
//! one regular entry, takes the destination from the manifest, checks the
//! digest and moves the file into place without replacing anything.

use super::run_dir::{now_rfc3339, read_capped_nofollow};
use super::work_root::create_private_dir_all;
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

pub(crate) const MANIFEST: &str = "MANIFEST.jsonl";
const MANIFEST_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ManifestLine {
    pub original_path: String,
    pub archive_path: String,
    pub size_bytes: u64,
    pub digest: String,
    pub archived_at: String,
}

fn open_nofollow(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn create_new_0600(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A reader that hashes what passes through it.
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    count: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }
}

/// Reads the single regular entry of a `.tar.gz`, at most `max` bytes.
fn read_single_entry(archive: &Path, max: u64) -> Result<Vec<u8>> {
    let file =
        open_nofollow(archive).with_context(|| format!("cannot open {}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let mut content: Option<Vec<u8>> = None;
    for entry in tar
        .entries()
        .with_context(|| format!("cannot read {}", archive.display()))?
    {
        let entry = entry.with_context(|| format!("cannot read {}", archive.display()))?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            bail!(
                "{} holds an entry that is not a regular file",
                archive.display()
            );
        }
        if content.is_some() {
            bail!("{} holds more than one entry", archive.display());
        }
        let mut buf = Vec::new();
        entry
            .take(max + 1)
            .read_to_end(&mut buf)
            .with_context(|| format!("cannot read {}", archive.display()))?;
        if buf.len() as u64 > max {
            bail!("{} is larger than its manifest says", archive.display());
        }
        content = Some(buf);
    }
    content.with_context(|| format!("{} is empty", archive.display()))
}

/// Picks `<dir>/<name>.tar.gz`, or `<name>.<n>.tar.gz` when that exists.
fn free_archive_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let first = dir.join(format!("{name}.tar.gz"));
    if fs::symlink_metadata(&first).is_err() {
        return Ok(first);
    }
    for n in 1..1000 {
        let candidate = dir.join(format!("{name}.{n}.tar.gz"));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    bail!("no free archive name for {name} in {}", dir.display())
}

/// Archives `source` to `<archive_root>/<relative>.tar.gz`, appends a manifest
/// line, verifies, and removes the original. Returns the archive path.
pub(crate) fn archive_file(source: &Path, archive_root: &Path, relative: &Path) -> Result<PathBuf> {
    let before = fs::symlink_metadata(source)
        .with_context(|| format!("cannot inspect {}", source.display()))?;
    if !before.file_type().is_file() {
        bail!("{} is not a regular file", source.display());
    }
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe archive path {}", relative.display());
    }
    let name = relative
        .file_name()
        .and_then(|n| n.to_str())
        .context("log file name is not valid UTF-8")?
        .to_string();
    let dir = match relative.parent() {
        Some(parent) => archive_root.join(parent),
        None => archive_root.to_path_buf(),
    };
    create_private_dir_all(archive_root)?;
    create_private_dir_all(&dir)?;
    let final_path = free_archive_path(&dir, &name)?;
    let partial = final_path.with_extension("gz.partial");

    let size = before.len();
    let digest = match write_archive(source, &partial, &name, &before) {
        Ok(digest) => digest,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
    };
    verify_and_place(&partial, &final_path, size, &digest)
        .with_context(|| format!("cannot archive {}; the original is kept", source.display()))?;

    let line = ManifestLine {
        original_path: source.display().to_string(),
        archive_path: final_path.display().to_string(),
        size_bytes: size,
        digest: format!("sha256:{digest}"),
        archived_at: now_rfc3339(),
    };
    append_manifest(&archive_root.join(MANIFEST), &line)?;

    let after = fs::symlink_metadata(source)
        .with_context(|| format!("cannot inspect {}", source.display()))?;
    if after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ino() != before.ino()
    {
        bail!(
            "{} changed while it was archived; the original is kept",
            source.display()
        );
    }
    fs::remove_file(source).with_context(|| format!("cannot remove {}", source.display()))?;
    Ok(final_path)
}

/// Reads `partial` back, compares it with the size and digest taken from the
/// source, and only then renames it to `final_path`. On any failure `partial`
/// is removed, so a failed check leaves no archive behind and a retry does not
/// add another one (PR #1843 review item 6).
fn verify_and_place(partial: &Path, final_path: &Path, size: u64, digest: &str) -> Result<()> {
    let placed = read_single_entry(partial, size)
        .and_then(|content| {
            if content.len() as u64 != size || hex(&Sha256::digest(&content)) != digest {
                bail!("archive {} does not match its source", partial.display());
            }
            Ok(())
        })
        .and_then(|()| {
            fs::rename(partial, final_path)
                .with_context(|| format!("cannot rename to {}", final_path.display()))
        });
    if placed.is_err() {
        let _ = fs::remove_file(partial);
    }
    placed
}

fn write_archive(source: &Path, partial: &Path, name: &str, meta: &fs::Metadata) -> Result<String> {
    let input =
        open_nofollow(source).with_context(|| format!("cannot open {}", source.display()))?;
    let out =
        create_new_0600(partial).with_context(|| format!("cannot create {}", partial.display()))?;
    let mut builder = tar::Builder::new(GzEncoder::new(out, Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(meta.len());
    header.set_mode(0o600);
    header.set_mtime(u64::try_from(meta.mtime()).unwrap_or(0));
    let mut reader = HashingReader {
        inner: input.take(meta.len()),
        hasher: Sha256::new(),
        count: 0,
    };
    builder
        .append_data(&mut header, name, &mut reader)
        .with_context(|| format!("cannot archive {}", source.display()))?;
    if reader.count != meta.len() {
        bail!("{} shrank while it was archived", source.display());
    }
    let digest = hex(&reader.hasher.finalize());
    let encoder = builder
        .into_inner()
        .with_context(|| format!("cannot finish {}", partial.display()))?;
    let file = encoder
        .finish()
        .with_context(|| format!("cannot finish {}", partial.display()))?;
    file.sync_all()
        .with_context(|| format!("cannot flush {}", partial.display()))?;
    Ok(digest)
}

fn append_manifest(path: &Path, line: &ManifestLine) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mut text = serde_json::to_string(line)?;
    text.push('\n');
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write {}", path.display()))
}

fn read_manifest(path: &Path) -> Result<Vec<ManifestLine>> {
    let bytes = read_capped_nofollow(path, MANIFEST_MAX_BYTES)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let text = String::from_utf8(bytes).context("manifest is not valid UTF-8")?;
    let mut lines = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        lines.push(
            serde_json::from_str(line)
                .with_context(|| format!("{} line {} is not valid", path.display(), n + 1))?,
        );
    }
    Ok(lines)
}

fn has_unsafe_components(path: &Path) -> bool {
    !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}

fn lexical_canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Refuses when any existing folder from below `base` down to `dir` is a
/// symbolic link or not a folder. `base` itself may be a symlink.
fn refuse_symlinks_below(base: &Path, dir: &Path) -> Result<()> {
    let rel = dir
        .strip_prefix(base)
        .with_context(|| format!("{} is not under {}", dir.display(), base.display()))?;
    let mut current = base.to_path_buf();
    for part in rel.components() {
        let Component::Normal(name) = part else {
            bail!("unsafe path {}", dir.display());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => bail!(
                "refusing to restore through symbolic link {}",
                current.display()
            ),
            Ok(meta) if !meta.file_type().is_dir() => {
                bail!("{} is not a folder", current.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

/// What a restore did, one line per manifest entry.
#[derive(Debug, Default)]
pub(crate) struct RestoreOutcome {
    pub restored: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// Restores from a manifest (every line) or from one archive (the manifest
/// line naming it). `log_roots` confine destinations when `dest` is `None`.
pub(crate) fn restore(
    target: &Path,
    dest: Option<&Path>,
    log_roots: &[PathBuf],
) -> Result<RestoreOutcome> {
    let target =
        fs::canonicalize(target).with_context(|| format!("cannot find {}", target.display()))?;
    let is_manifest = target.file_name().and_then(|n| n.to_str()) == Some(MANIFEST);
    let (manifest_path, lines) = if is_manifest {
        (target.clone(), read_manifest(&target)?)
    } else {
        let manifest_path = target
            .ancestors()
            .skip(1)
            .map(|dir| dir.join(MANIFEST))
            .find(|candidate| candidate.is_file())
            .with_context(|| format!("no manifest entry for {}", target.display()))?;
        let all = read_manifest(&manifest_path)?;
        let line = all
            .into_iter()
            .rev()
            .find(|line| lexical_canonical(Path::new(&line.archive_path)) == target)
            .with_context(|| format!("no manifest entry for {}", target.display()))?;
        (manifest_path, vec![line])
    };
    let archive_root = manifest_path
        .parent()
        .context("manifest has no parent folder")?
        .to_path_buf();
    let dest = match dest {
        Some(d) if d.is_absolute() => Some(d.to_path_buf()),
        Some(d) => Some(
            std::env::current_dir()
                .context("cannot resolve the current folder")?
                .join(d),
        ),
        None => None,
    };
    if let Some(dest) = &dest
        && has_unsafe_components(dest)
    {
        bail!(
            "--dest {} must not contain `.` or `..` components",
            dest.display()
        );
    }

    let mut outcome = RestoreOutcome::default();
    for line in &lines {
        match restore_one(line, &archive_root, dest.as_deref(), log_roots) {
            Ok(path) => outcome.restored.push(path),
            Err(error) => outcome.errors.push(format!("{error:#}")),
        }
    }
    Ok(outcome)
}

fn restore_one(
    line: &ManifestLine,
    archive_root: &Path,
    dest: Option<&Path>,
    log_roots: &[PathBuf],
) -> Result<PathBuf> {
    let archive = PathBuf::from(&line.archive_path);
    if has_unsafe_components(&archive) || !lexical_canonical(&archive).starts_with(archive_root) {
        bail!(
            "manifest archive path {} is outside {}",
            archive.display(),
            archive_root.display()
        );
    }
    let original = PathBuf::from(&line.original_path);
    if has_unsafe_components(&original) {
        bail!(
            "manifest original path {} is not a clean absolute path",
            original.display()
        );
    }
    // `base` is the folder the destination must stay under: --dest, or the
    // log root that holds the original path.
    let (destination, base) = match dest {
        Some(dest) => {
            let canonical = lexical_canonical(&archive);
            let relative = canonical
                .strip_prefix(archive_root)
                .context("archive path is outside the archive folder")?;
            let text = relative.to_string_lossy();
            let trimmed = text.strip_suffix(".tar.gz").unwrap_or(&text);
            let rel = PathBuf::from(trimmed);
            if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
                bail!("unsafe archive path {}", archive.display());
            }
            (dest.join(rel), dest.to_path_buf())
        }
        None => {
            let Some(root) = log_roots.iter().find(|root| original.starts_with(root)) else {
                bail!(
                    "refusing to restore {}: it is not under a log folder; use --dest",
                    original.display()
                );
            };
            (original.clone(), root.clone())
        }
    };
    if fs::symlink_metadata(&destination).is_ok() {
        bail!(
            "{} already exists; restore never overwrites",
            destination.display()
        );
    }
    let digest = line
        .digest
        .strip_prefix("sha256:")
        .with_context(|| format!("unsupported digest {}", line.digest))?;
    let content = read_single_entry(&archive, line.size_bytes)?;
    if content.len() as u64 != line.size_bytes {
        bail!("{} does not have the recorded size", archive.display());
    }
    if hex(&Sha256::digest(&content)) != digest {
        bail!("{} does not match the recorded digest", archive.display());
    }

    let parent = destination
        .parent()
        .context("destination has no parent folder")?;
    refuse_symlinks_below(&base, parent)?;
    create_private_dir_all(parent)?;
    // Checked again after creating: a symlink planted in between, or one
    // above the first missing folder, must not take the write elsewhere.
    refuse_symlinks_below(&base, parent)?;
    let real_base =
        fs::canonicalize(&base).with_context(|| format!("cannot resolve {}", base.display()))?;
    let real_parent =
        fs::canonicalize(parent).with_context(|| format!("cannot resolve {}", parent.display()))?;
    if !real_parent.starts_with(&real_base) {
        bail!(
            "refusing to restore {}: its folder resolves outside {}",
            destination.display(),
            base.display()
        );
    }
    let file_name = destination
        .file_name()
        .context("destination has no file name")?
        .to_string_lossy()
        .into_owned();
    let partial = parent.join(format!(".{file_name}.{}.partial", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut out = create_new_0600(&partial)
            .with_context(|| format!("cannot create {}", partial.display()))?;
        out.write_all(&content)
            .and_then(|()| out.sync_all())
            .with_context(|| format!("cannot write {}", partial.display()))?;
        drop(out);
        // A hard link fails when the destination exists, so a file created at
        // the same moment is never replaced.
        fs::hard_link(&partial, &destination).map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                anyhow::anyhow!(
                    "{} already exists; restore never overwrites",
                    destination.display()
                )
            } else {
                anyhow::Error::new(error)
                    .context(format!("cannot move into {}", destination.display()))
            }
        })
    })();
    let _ = fs::remove_file(&partial);
    result?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review item 6: an archive that fails its read-back check is removed,
    /// the original stays, and a retry leaves no extra file.
    #[test]
    fn failed_read_back_leaves_no_archive_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("run.log");
        fs::write(&source, "log line\n").unwrap();
        let dir = tmp.path().join("archive");
        fs::create_dir_all(&dir).unwrap();
        let meta = fs::symlink_metadata(&source).unwrap();
        for _ in 0..2 {
            let final_path = free_archive_path(&dir, "run.log").unwrap();
            let partial = final_path.with_extension("gz.partial");
            write_archive(&source, &partial, "run.log", &meta).unwrap();
            let error =
                verify_and_place(&partial, &final_path, meta.len(), "0".repeat(64).as_str())
                    .unwrap_err();
            assert!(format!("{error:#}").contains("does not match"), "{error:#}");
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(&source).unwrap(), "log line\n");
    }

    #[test]
    fn matching_read_back_places_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("run.log");
        fs::write(&source, "log line\n").unwrap();
        let meta = fs::symlink_metadata(&source).unwrap();
        let final_path = tmp.path().join("run.log.tar.gz");
        let partial = final_path.with_extension("gz.partial");
        let digest = write_archive(&source, &partial, "run.log", &meta).unwrap();
        verify_and_place(&partial, &final_path, meta.len(), &digest).unwrap();
        assert!(final_path.is_file());
        assert!(!partial.exists());
    }
}
