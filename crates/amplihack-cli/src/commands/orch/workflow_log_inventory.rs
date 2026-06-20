//! Deterministic metadata-only workflow log inventory helper.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct Inventory {
    root: String,
    artifacts: Vec<LogArtifact>,
}

#[derive(Debug, Serialize)]
struct LogArtifact {
    path: String,
    kind: &'static str,
    size_bytes: u64,
    modified_utc: String,
}

pub(super) fn run(root: PathBuf, format: &str) -> Result<()> {
    let inventory = build_inventory(&root)?;
    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&inventory)?);
            Ok(())
        }
        "text" => {
            print_text_inventory(&inventory);
            Ok(())
        }
        other => bail!("unsupported workflow-log-inventory format: {other}"),
    }
}

fn build_inventory(root: &Path) -> Result<Inventory> {
    let canonical_root = root.canonicalize().with_context(|| {
        format!(
            "workflow-log-inventory root does not exist: {}",
            root.display()
        )
    })?;
    if !canonical_root.is_dir() {
        bail!(
            "workflow-log-inventory root is not a directory: {}",
            canonical_root.display()
        );
    }

    let mut artifacts = Vec::new();
    scan_known_log_dir(
        &canonical_root,
        Path::new(".amplihack/recipes"),
        "recipe",
        &mut artifacts,
    )?;
    scan_known_log_dir(
        &canonical_root,
        Path::new(".amplihack/workflows"),
        "workflow",
        &mut artifacts,
    )?;
    push_known_log_file(
        &canonical_root,
        Path::new("recipe-runner.log"),
        "recipe",
        &mut artifacts,
    )?;

    artifacts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Inventory {
        root: canonical_root.to_string_lossy().to_string(),
        artifacts,
    })
}

fn scan_known_log_dir(
    root: &Path,
    relative_dir: &Path,
    kind: &'static str,
    artifacts: &mut Vec<LogArtifact>,
) -> Result<()> {
    let dir = root.join(relative_dir);
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() {
        bail!(
            "workflow log location is not a directory: {}",
            dir.display()
        );
    }
    scan_dir_for_logs(root, &dir, kind, artifacts)
}

fn scan_dir_for_logs(
    root: &Path,
    dir: &Path,
    kind: &'static str,
    artifacts: &mut Vec<LogArtifact>,
) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read workflow log directory {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| {
            format!(
                "failed to enumerate workflow log directory {}",
                dir.display()
            )
        })?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        if file_type.is_dir() {
            scan_dir_for_logs(root, &entry.path(), kind, artifacts)?;
        } else if file_type.is_file()
            && entry.path().extension().and_then(|ext| ext.to_str()) == Some("log")
        {
            push_log_artifact(root, &entry.path(), kind, artifacts)?;
        }
    }
    Ok(())
}

fn push_known_log_file(
    root: &Path,
    relative_path: &Path,
    kind: &'static str,
    artifacts: &mut Vec<LogArtifact>,
) -> Result<()> {
    let path = root.join(relative_path);
    if !path.exists() {
        return Ok(());
    }
    let file_type = fs::symlink_metadata(&path)
        .with_context(|| format!("failed to inspect workflow log file {}", path.display()))?
        .file_type();
    if file_type.is_file() {
        push_log_artifact(root, &path, kind, artifacts)?;
    }
    Ok(())
}

fn push_log_artifact(
    root: &Path,
    path: &Path,
    kind: &'static str,
    artifacts: &mut Vec<LogArtifact>,
) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read workflow log metadata {}", path.display()))?;
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("workflow log path escapes root: {}", path.display()))?;
    let modified = metadata.modified().with_context(|| {
        format!(
            "failed to read workflow log modification time {}",
            path.display()
        )
    })?;
    let modified_utc: DateTime<Utc> = modified.into();
    artifacts.push(LogArtifact {
        path: relative.to_string_lossy().replace('\\', "/"),
        kind,
        size_bytes: metadata.len(),
        modified_utc: modified_utc.to_rfc3339_opts(SecondsFormat::Secs, true),
    });
    Ok(())
}

fn print_text_inventory(inventory: &Inventory) {
    println!("workflow-log-inventory root={}", inventory.root);
    println!("found {} log artifacts", inventory.artifacts.len());
    if inventory.artifacts.is_empty() {
        return;
    }
    println!();
    println!(
        "{:<48} {:<10} {:>10} modified_utc",
        "path", "kind", "size_bytes"
    );
    for artifact in &inventory.artifacts {
        println!(
            "{:<48} {:<10} {:>10} {}",
            artifact.path, artifact.kind, artifact.size_bytes, artifact.modified_utc
        );
    }
}
