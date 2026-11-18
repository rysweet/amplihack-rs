//! Complete PR exclusion reports, using the same secret redaction as recovery.
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

fn clean(value: &mut Value) {
    match value {
        Value::String(s) => *s = super::evidence::strip_controls(&super::evidence::redact(s)),
        Value::Array(items) => items.iter_mut().for_each(clean),
        Value::Object(fields) => fields.values_mut().for_each(clean),
        _ => (),
    }
}

/// Read diagnostics as JSON data; print safe evidence to stderr and its durable
/// absolute report path to stdout. No report bytes are written before redaction.
pub fn run() -> Result<()> {
    let mut report: Value = serde_json::from_reader(std::io::stdin().lock())
        .context("PR scope diagnostic input must be JSON")?;
    clean(&mut report);
    let text = serde_json::to_string_pretty(&report)?;
    // Emit exclusions even when report persistence itself encounters an error.
    eprintln!("{text}");
    let root = [
        "AMPLIHACK_WORKFLOW_RUNTIME_DIR",
        "AMPLIHACK_RUNTIME_ROOT",
        "XDG_RUNTIME_DIR",
    ]
    .iter()
    .find_map(|key| std::env::var_os(key).filter(|v| !v.is_empty()))
    .map(PathBuf::from)
    .unwrap_or_else(std::env::temp_dir);
    anyhow::ensure!(
        root.is_absolute(),
        "PR diagnostic runtime path must be absolute"
    );
    std::fs::create_dir_all(&root).context("create PR diagnostic runtime directory")?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("pr-scope-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let dir = builder.tempdir_in(&root)?;
    let path = dir.path().join("exclusions.json");
    let mut file = super::secure_fs::output(super::secure_fs::Scope::Strict, &path)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    let _report_dir = dir.keep();
    println!("{}", path.display());
    Ok(())
}
