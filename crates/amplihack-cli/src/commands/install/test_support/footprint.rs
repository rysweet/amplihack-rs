//! Independent limit: 1 MiB inclusive; larger files require registered identity.
use super::artifacts::ArtifactRegistry;
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub const INDEPENDENT_LIMIT: u64 = 1024 * 1024;
#[derive(Debug, Default)]
pub struct Report {
    pub oversized_independent_bytes: u64,
    pub verified_linked_logical_bytes: u64,
    pub inspected_paths: Vec<PathBuf>,
}
pub fn verify(home: &Path, app: &Path, registry: &ArtifactRegistry) -> Result<Report> {
    let mut report = Report::default();
    let forbidden = app.canonicalize()?;
    let mut bins = vec![home.join(".local/bin")];
    let releases = app.join("releases");
    if releases.exists() {
        for entry in fs::read_dir(releases)? {
            bins.push(entry?.path().join("bin"));
        }
    }
    for bin in bins {
        if !bin.exists() {
            continue;
        }
        for entry in fs::read_dir(&bin)? {
            let path = entry?.path();
            let resolved = path
                .canonicalize()
                .with_context(|| format!("invalid staged link: {}", path.display()))?;
            let m = fs::metadata(&resolved)?;
            if !m.is_file() {
                bail!("staged binary is not a file: {}", path.display());
            }
            report.inspected_paths.push(path.clone());
            if m.len() > INDEPENDENT_LIMIT {
                if registry.matches(&resolved, Some(&forbidden))? {
                    report.verified_linked_logical_bytes += m.len();
                } else {
                    report.oversized_independent_bytes += m.len();
                }
            }
        }
    }
    if report.oversized_independent_bytes > 0 {
        bail!(
            "fake home contains {} oversized independent binary bytes; inspected {:?}",
            report.oversized_independent_bytes,
            report.inspected_paths
        );
    }
    Ok(report)
}
