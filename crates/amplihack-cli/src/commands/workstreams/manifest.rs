//! The workstream manifest (R4): `label<TAB>issue<TAB>pr` per line, adding
//! labels to discovered runs and listing work that has not started.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::{Result, bail};

use crate::user_config::safe::{display_path, read_bounded};

/// Mode bits a manifest may not have: write access for group or others.
/// It holds only labels and numbers, so others may read it.
const LOOSE_BITS: u32 = 0o022;

/// One manifest line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestRow {
    /// `None` for `-`: the row uses the default label.
    pub label: Option<String>,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
}

/// A malformed line, 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestError {
    pub line: usize,
    pub reason: String,
}

/// Every row of `text`; `#` lines and blank lines are skipped.
pub(crate) fn parse(text: &str) -> Result<Vec<ManifestRow>, ManifestError> {
    let mut rows = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let error = |reason: String| ManifestError {
            line: i + 1,
            reason,
        };
        let fields: Vec<&str> = line.split('\t').collect();
        let [label, issue, pr] = fields.as_slice() else {
            return Err(error(format!(
                "expected 3 tab-separated fields, found {}",
                fields.len()
            )));
        };
        let issue = number(issue).map_err(|()| {
            error("the issue must be a whole number, optionally after `#`, or `-`".into())
        })?;
        let pr = number(pr).map_err(|()| {
            error("the PR must be a whole number, optionally after `#`, or `-`".into())
        })?;
        if issue.is_none() && pr.is_none() {
            return Err(error("a row needs an issue or a PR".into()));
        }
        let label = label.trim();
        rows.push(ManifestRow {
            label: (!label.is_empty() && label != "-").then(|| label.to_string()),
            issue,
            pr,
        });
    }
    Ok(rows)
}

/// `-` is empty; otherwise digits, with at most one leading `#`.
fn number(field: &str) -> Result<Option<u64>, ()> {
    let field = field.trim();
    if field == "-" {
        return Ok(None);
    }
    let digits = field.strip_prefix('#').unwrap_or(field);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    digits.parse().map(Some).map_err(|_| ())
}

/// The rows of the manifest at `path`. A missing default is empty; a
/// default that fails the file check is empty, with a warning. An
/// `explicit` path that is missing or fails the check is an error. A
/// malformed line is an error in either case.
pub(crate) fn load(path: &Path, explicit: bool) -> Result<Vec<ManifestRow>> {
    let shown = display_path(path);
    let refused = |why: &str| -> Result<Vec<ManifestRow>> {
        if explicit {
            bail!("manifest {shown} {why}");
        }
        eprintln!("warning: ignored the manifest {shown}: it {why}");
        Ok(Vec::new())
    };
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
            return Ok(Vec::new());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("manifest {shown} does not exist")
        }
        Err(e) => return refused(&format!("cannot be read ({})", e.kind())),
    };
    if !meta.is_file() {
        return refused("is not a regular file");
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    if meta.uid() != unsafe { libc::geteuid() } {
        return refused("is not owned by you");
    }
    if meta.mode() & LOOSE_BITS != 0 {
        return refused("is writable by group or others");
    }
    let text = match read_bounded(path) {
        Ok(Some(text)) => text,
        Ok(None) => return refused("does not exist"),
        Err(reason) => return refused(&reason),
    };
    parse(&text).or_else(|e| bail!("manifest {shown}:{}: {}", e.line, e.reason))
}

/// The row and a run are the same workstream: the same issue, or the same
/// PR when both have one.
pub(crate) fn matches(row: &ManifestRow, issue: Option<u64>, pr: Option<u64>) -> bool {
    (row.issue.is_some() && row.issue == issue) || (row.pr.is_some() && row.pr == pr)
}
