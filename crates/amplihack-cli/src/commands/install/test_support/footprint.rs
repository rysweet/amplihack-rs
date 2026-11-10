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
// Directory links are never fixture boundaries, even when they resolve locally.
fn directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => Ok(true),
        Ok(_) => bail!("invalid footprint directory: {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("inspect directory {}", path.display())),
    }
}
pub fn verify(home: &Path, app: &Path, registry: &ArtifactRegistry) -> Result<Report> {
    let mut report = Report::default();
    directory(home)?;
    directory(&home.join(".local"))?;
    if !directory(app)? {
        bail!("missing application root: {}", app.display());
    }
    let forbidden = app.canonicalize()?;
    let mut bins = vec![home.join(".local/bin")];
    let releases = app.join("releases");
    if directory(&releases)? {
        for entry in fs::read_dir(releases)? {
            let release = entry?.path();
            if directory(&release)? {
                bins.push(release.join("bin"));
            }
        }
    }
    for bin in bins {
        if !directory(&bin)? {
            continue;
        }
        let canonical_bin = bin.canonicalize()?;
        for entry in fs::read_dir(&bin)? {
            let path = entry?.path();
            let resolved = path
                .canonicalize()
                .with_context(|| format!("invalid staged link: {}", path.display()))?;
            let m = fs::metadata(&resolved)?;
            if !m.is_file() {
                bail!("staged binary is not a file: {}", path.display());
            }
            // Publication links into a release are confined; links outside
            // the inspected bin directories require provenance at every size.
            let confined = resolved.parent().is_some_and(|parent| {
                parent == canonical_bin
                    || (parent.parent().and_then(Path::parent)
                        == Some(forbidden.join("releases").as_path())
                        && parent.file_name().is_some_and(|n| n == "bin"))
            });
            if !confined && !registry.matches(&resolved, Some(&forbidden))? {
                bail!("unregistered external binary target: {}", path.display());
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
