//! Workstream usage accounting from explicit, read-only native JSONL inputs.
mod adapters;
mod aggregate;
mod assemble;
mod collect;
mod manifest;
pub mod models;
mod publish;
mod report;
#[cfg(test)]
mod tests;

use anyhow::{Result, bail};
use clap::Subcommand;
pub use collect::collect;
pub use report::summary;
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum UsageCommands {
    /// Read only the manifest's explicit files; never invokes a model or network
    Report {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long, default_value = "summary", value_parser = ["json", "summary"])]
        format: String,
    },
    /// Explicitly create/update the stable informational comment on a completed PR
    Publish {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        pr: u64,
    },
}

fn load(path: &Path) -> Result<models::Manifest> {
    let Ok(bytes) = std::fs::read(path) else {
        bail!("cannot read usage manifest");
    };
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid usage manifest schema (version 1 required)"))
}

pub fn run(cmd: UsageCommands) -> Result<()> {
    let (path, format, pr) = match cmd {
        UsageCommands::Report { manifest, format } => (manifest, format, None),
        UsageCommands::Publish { manifest, pr } => (manifest, "summary".into(), Some(pr)),
    };
    let m = load(&path)?;
    let r = collect(&m, path.parent().unwrap_or(Path::new(".")))?;
    if let Some(pr) = pr {
        println!(
            "Usage comment {}",
            publish::publish(&m, &r, pr, &crate::commands::pr::RealGhRunner)?
        );
    } else if format == "json" {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{}", summary(&r));
    }
    Ok(())
}
