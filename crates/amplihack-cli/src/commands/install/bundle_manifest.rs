//! Compare a staged bundle with its source (issue #1955), and read the release
//! manifest format (issue #1972).
//!
//! This shows that staging matches its source, not that the source is authentic.

use super::filesystem::{WalkEntryKind, WalkScope, walk_tree};
use super::paths::{sanitize_path, sanitize_text};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::path::Path;

/// Paths listed per category in a mismatch message (SEC-14).
const MAX_LISTED_PATHS: usize = 20;

/// Relative path (`/`-separated) to lowercase hex SHA-256 of each regular file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BundleManifest {
    pub(super) files: BTreeMap<String, String>,
}

/// What `compare` found, each list sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ManifestDiff {
    pub(super) missing: Vec<String>,
    pub(super) extra: Vec<String>,
    pub(super) changed: Vec<String>,
}

impl ManifestDiff {
    pub(super) fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty() && self.changed.is_empty()
    }
}

/// Files the copy would take from a source bundle (`WalkScope::CopySet`).
pub(super) fn of_source(bundle: &Path) -> Result<BundleManifest> {
    build(bundle, WalkScope::CopySet)
}

/// Every regular file actually present in staging (`WalkScope::Everything`), so
/// a file with an excluded name still counts as extra.
pub(super) fn of_staged(bundle: &Path) -> Result<BundleManifest> {
    build(bundle, WalkScope::Everything)
}

fn build(bundle: &Path, scope: WalkScope) -> Result<BundleManifest> {
    let mut files = BTreeMap::new();
    for entry in walk_tree(bundle, scope)? {
        if entry.kind != WalkEntryKind::File {
            continue;
        }
        let rel = entry
            .rel
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.insert(rel, sha256_of_file(&entry.path)?);
    }
    Ok(BundleManifest { files })
}

/// SHA-256 of a regular file. The handle is opened without blocking and its
/// type checked, so a FIFO swapped in after the walk can't stall a launch
/// (issue #1972).
pub(super) fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = open_regular_file(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

fn open_regular_file(path: &Path) -> Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    Ok(file)
}

/// Pure comparison.
pub(super) fn compare(source: &BundleManifest, staged: &BundleManifest) -> ManifestDiff {
    let mut diff = ManifestDiff::default();
    for (path, hash) in &source.files {
        match staged.files.get(path) {
            None => diff.missing.push(path.clone()),
            Some(other) if other != hash => diff.changed.push(path.clone()),
            Some(_) => {}
        }
    }
    diff.extra = staged
        .files
        .keys()
        .filter(|path| !source.files.contains_key(*path))
        .cloned()
        .collect();
    diff
}

/// The user-facing failure text for a non-clean diff.
pub(super) fn render_mismatch(source: &Path, diff: &ManifestDiff) -> String {
    let mut text = format!(
        "the staged bundle does not match its source {} ({} missing, {} extra, {} changed):",
        sanitize_path(source),
        diff.missing.len(),
        diff.extra.len(),
        diff.changed.len()
    );
    write_path_lists(&mut text, diff);
    text
}

/// Issue #1972: a tree compared with the release manifest embedded in the
/// binary. `subject` is already sanitised by the caller and names the tree,
/// for example "the framework source at /x (AMPLIHACK_HOME)".
pub(super) fn render_release_mismatch(subject: &str, version: &str, diff: &ManifestDiff) -> String {
    let mut text = format!(
        "{subject} does not match the release v{version} manifest embedded in this binary \
         ({} missing, {} extra, {} changed):",
        diff.missing.len(),
        diff.extra.len(),
        diff.changed.len()
    );
    write_path_lists(&mut text, diff);
    text
}

/// Up to [`MAX_LISTED_PATHS`] sanitised paths per category, then a count.
fn write_path_lists(text: &mut String, diff: &ManifestDiff) {
    for (label, paths) in [
        ("missing", &diff.missing),
        ("extra", &diff.extra),
        ("changed", &diff.changed),
    ] {
        for path in paths.iter().take(MAX_LISTED_PATHS) {
            let _ = write!(text, "\n  {label}: {}", sanitize_text(path));
        }
        if paths.len() > MAX_LISTED_PATHS {
            let _ = write!(
                text,
                "\n  {label}: and {} more",
                paths.len() - MAX_LISTED_PATHS
            );
        }
    }
}

/// Why a release manifest text was rejected (issue #1972). The reason is one
/// of a fixed set and never repeats bytes from the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ManifestParseError {
    pub(super) line: usize,
    pub(super) reason: &'static str,
}

impl std::fmt::Display for ManifestParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

impl std::error::Error for ManifestParseError {}

/// Length of a lowercase hex SHA-256.
const HASH_LEN: usize = 64;
/// The separator `sha256sum` writes in text mode.
const SEPARATOR: &[u8] = b"  ";

/// Parse the release manifest format, version 1 (issue #1972).
///
/// Every line is `<64 lowercase hex><two spaces><path>\n`, paths in strictly
/// ascending byte order. Empty input gives an empty manifest. Checks run on
/// bytes before any `&str` slicing, so no input can cause a panic.
pub(super) fn parse(text: &str) -> std::result::Result<BundleManifest, ManifestParseError> {
    let mut files = BTreeMap::new();
    if text.is_empty() {
        return Ok(BundleManifest { files });
    }
    let bytes = text.as_bytes();
    if bytes.last() != Some(&b'\n') {
        let line = bytes.iter().filter(|b| **b == b'\n').count() + 1;
        return Err(ManifestParseError {
            line,
            reason: "the last line does not end with a newline",
        });
    }
    let mut previous: Option<&[u8]> = None;
    for (index, line) in bytes[..bytes.len() - 1].split(|b| *b == b'\n').enumerate() {
        let number = index + 1;
        let fail = |reason| ManifestParseError {
            line: number,
            reason,
        };
        let (hash, path) = split_line(line).map_err(fail)?;
        if let Some(previous) = previous
            && previous >= path
        {
            return Err(fail("paths are not in strictly ascending byte order"));
        }
        previous = Some(path);
        let path_text = std::str::from_utf8(path).map_err(|_| fail("the path is not UTF-8"))?;
        check_path(path_text).map_err(fail)?;
        let hash_text = std::str::from_utf8(hash).map_err(|_| fail("the hash is not hex"))?;
        files.insert(path_text.to_string(), hash_text.to_string());
    }
    Ok(BundleManifest { files })
}

/// Split one line (without its newline) into hash and path bytes.
fn split_line(line: &[u8]) -> std::result::Result<(&[u8], &[u8]), &'static str> {
    if line.is_empty() {
        return Err("blank line");
    }
    if line.contains(&b'\r') {
        return Err("carriage return");
    }
    if line[0] == b'\\' {
        return Err("escaped line (leading backslash)");
    }
    if line.len() < HASH_LEN || !line[..HASH_LEN].iter().all(is_lower_hex) {
        return Err("the line does not start with 64 lowercase hex digits");
    }
    if line.len() < HASH_LEN + SEPARATOR.len()
        || &line[HASH_LEN..HASH_LEN + SEPARATOR.len()] != SEPARATOR
    {
        return Err("the hash is not followed by exactly two spaces");
    }
    let path = &line[HASH_LEN + SEPARATOR.len()..];
    if path.first() == Some(&b' ') {
        return Err("the hash is not followed by exactly two spaces");
    }
    Ok((&line[..HASH_LEN], path))
}

fn is_lower_hex(byte: &u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
}

/// The path rule shared by the parser and the generator script.
fn check_path(path: &str) -> std::result::Result<(), &'static str> {
    if path.is_empty() {
        return Err("empty path");
    }
    if path.starts_with('*') {
        return Err("binary-mode marker (leading `*`)");
    }
    if path.starts_with('/') {
        return Err("absolute path");
    }
    if path.contains('\\') {
        return Err("backslash in the path");
    }
    if path.chars().any(|c| c.is_ascii_control()) {
        return Err("control character in the path");
    }
    if path.contains(char::REPLACEMENT_CHARACTER) {
        return Err("U+FFFD in the path");
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("empty, `.` or `..` path component");
    }
    Ok(())
}

#[cfg(test)]
impl BundleManifest {
    /// The canonical text [`parse`] reads back to the same manifest.
    pub(super) fn to_text(&self) -> String {
        self.files
            .iter()
            .fold(String::new(), |mut text, (path, hash)| {
                let _ = writeln!(text, "{hash}  {path}");
                text
            })
    }
}
