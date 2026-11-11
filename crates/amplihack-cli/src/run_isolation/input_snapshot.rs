//! Byte-preserving snapshots of the index and the supported local inputs.
use super::git;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
enum Contents {
    File(Vec<u8>, u32),
    Link(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InputSnapshot {
    pub head: Vec<u8>,
    pub entries: Vec<u8>,
    files: BTreeMap<PathBuf, Contents>,
    index_digest: Vec<u8>,
}

impl InputSnapshot {
    pub fn capture(root: &Path, inputs: &[PathBuf]) -> Result<Self> {
        let head = git(root, &["rev-parse", "--verify", "HEAD^{commit}"])
            .context("recipe run: unborn HEAD; commit before starting an isolated run")?;
        let entries = git(root, &["ls-files", "--stage", "-z"])?;
        let flags = git(root, &["ls-files", "-v", "-z"])?;
        for flag in flags.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            if flag[0] != b'H' {
                bail!(
                    "unsupported index flags on {:?}; clear assume-unchanged/skip-worktree or resolve conflicts before running",
                    OsString::from_vec(flag[2..].to_vec())
                );
            }
        }
        let index = git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )?;
        let index = PathBuf::from(OsString::from_vec(trim_lf(&index).to_vec()));
        let index_digest =
            Sha256::digest(fs::read(&index).context("read caller index without refreshing it")?)
                .to_vec();
        let mut paths = BTreeSet::new();
        for entry in entries.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let tab = entry
                .iter()
                .position(|b| *b == b'\t')
                .context("invalid index entry")?;
            let header = std::str::from_utf8(&entry[..tab])?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3
                || fields[2] != "0"
                || !["100644", "100755", "120000"].contains(&fields[0])
            {
                bail!(
                    "unsupported index entry {header}; resolve conflicts and remove submodule inputs before running"
                );
            }
            paths.insert(PathBuf::from(OsString::from_vec(entry[tab + 1..].to_vec())));
        }
        let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in untracked.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            paths.insert(PathBuf::from(OsString::from_vec(path.to_vec())));
        }
        for input in inputs {
            if let Ok(relative) = input.strip_prefix(root) {
                add_input(root, relative, &mut paths)?;
            }
        }
        let mut files = BTreeMap::new();
        for path in paths {
            validate_relative(&path)?;
            // A tracked path may have been deleted in the working layer.
            let full = root.join(&path);
            let metadata = match fs::symlink_metadata(&full) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("inspect local input {}", full.display()));
                }
            };
            validate_parents(root, &path)?;
            let contents = if metadata.file_type().is_symlink() {
                let target = fs::read_link(&full)?;
                if target.is_absolute()
                    || !lexically_inside(path.parent().unwrap_or(Path::new("")).join(&target))
                {
                    bail!(
                        "local input {} is an escaping symlink; copy its contents into the repository before running",
                        path.display()
                    );
                }
                Contents::Link(target)
            } else if metadata.is_file() {
                let mut file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&full)?;
                use std::io::Read;
                let opened = file.metadata()?;
                if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
                    bail!("local input changed during capture: {}", path.display());
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                Contents::File(bytes, metadata.permissions().mode() & 0o777)
            } else {
                bail!(
                    "unsupported local input {}; only regular files and in-repository symlinks can be isolated",
                    path.display()
                );
            };
            files.insert(path, contents);
        }
        Ok(Self {
            head,
            entries,
            files,
            index_digest,
        })
    }

    pub fn install(&self, source: &Path, destination: &Path) -> Result<()> {
        use std::io::Write;
        use std::process::Stdio;
        // Staged blobs need not be reachable from any commit. Transfer each
        // through the object protocol rather than sharing the caller's store.
        let mut transferred = BTreeSet::new();
        for entry in self.entries.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let tab = entry
                .iter()
                .position(|b| *b == b'\t')
                .context("invalid index entry")?;
            let header = std::str::from_utf8(&entry[..tab])?;
            let oid = header.split(' ').nth(1).context("missing staged object")?;
            if transferred.insert(oid) {
                let blob = git(source, &["cat-file", "blob", oid])?;
                let mut child = super::preparation_command(destination)
                    .args(["hash-object", "-w", "--stdin"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                let mut pipe = child.stdin.take().context("object input pipe")?;
                let writer = std::thread::spawn(move || pipe.write_all(&blob));
                let output = child.wait_with_output()?;
                writer
                    .join()
                    .map_err(|_| anyhow::anyhow!("object writer panicked"))??;
                if !output.status.success() || trim_lf(&output.stdout) != oid.as_bytes() {
                    bail!("cannot transfer staged object {oid} into isolated repository");
                }
            }
        }
        let mut child = super::preparation_command(destination)
            .args(["update-index", "-z", "--index-info"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut pipe = child.stdin.take().context("index input pipe")?;
        let entries = self.entries.clone();
        let writer = std::thread::spawn(move || pipe.write_all(&entries));
        let output = child.wait_with_output()?;
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("index writer panicked"))??;
        if !output.status.success() {
            bail!(
                "cannot install isolated index: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // The checkout was created without materializing any committed files.
        // Install working contents separately so staged deletions/additions and
        // unstaged changes retain their meanings.
        for (relative, contents) in &self.files {
            let target = destination.join(relative);
            fs::create_dir_all(target.parent().context("input parent")?)?;
            match contents {
                Contents::File(bytes, mode) => {
                    fs::write(&target, bytes)?;
                    fs::set_permissions(&target, fs::Permissions::from_mode(*mode))?;
                }
                Contents::Link(path) => std::os::unix::fs::symlink(path, &target)?,
            }
        }
        Ok(())
    }
}

use std::os::unix::fs::OpenOptionsExt;
pub(super) fn trim_lf(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn lexically_inside(path: PathBuf) -> bool {
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::Normal(name) if !name.as_encoded_bytes().eq_ignore_ascii_case(b".git") => {
                depth += 1
            }
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::CurDir => {}
            _ => return false,
        }
    }
    true
}
fn validate_relative(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(name) if name != ".git"))
    {
        bail!("unsafe local input path {}", path.display());
    }
    Ok(())
}
fn validate_parents(root: &Path, relative: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            current.push(component);
            if fs::symlink_metadata(&current)?.file_type().is_symlink() {
                bail!(
                    "local input {} has a symlink parent; use a direct path inside the repository",
                    relative.display()
                );
            }
        }
    }
    Ok(())
}
fn add_input(root: &Path, relative: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<()> {
    if relative.as_os_str().is_empty() {
        return Ok(());
    }
    validate_relative(relative)?;
    let metadata = fs::symlink_metadata(root.join(relative))?;
    if metadata.is_dir() {
        for entry in fs::read_dir(root.join(relative))? {
            let entry = entry?;
            add_input(root, &relative.join(entry.file_name()), paths)?;
        }
    } else {
        paths.insert(relative.to_path_buf());
    }
    Ok(())
}
