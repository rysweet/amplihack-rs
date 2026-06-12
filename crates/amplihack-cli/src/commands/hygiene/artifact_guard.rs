use std::path::PathBuf;

use amplihack_utils::artifact_guard::{
    ArtifactGuardConfig, ArtifactGuardMode, ArtifactGuardReport, scan_artifacts,
};
use anyhow::Result;

use crate::{HygieneArtifactGuardArgs, command_error};

pub fn run(args: HygieneArtifactGuardArgs) -> Result<()> {
    let repo = match args.repo {
        Some(repo) => repo,
        None => std::env::current_dir().map_err(|error| {
            eprintln!("Artifact Guard configuration error: resolve current directory: {error}");
            command_error::exit_error(2)
        })?,
    };

    let mode = match ArtifactGuardMode::parse(&args.mode) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("Artifact Guard configuration error: {error}");
            return Err(command_error::exit_error(2));
        }
    };

    let mut config = ArtifactGuardConfig::new(&repo).with_mode(mode);
    if let Some(allowlist) = args.allowlist {
        config = config.with_allowlist(allowlist);
    } else {
        let default_allowlist = PathBuf::from(".amplihack-artifact-allowlist");
        if repo.join(&default_allowlist).exists() {
            config = config.with_allowlist(default_allowlist);
        }
    }

    match scan_artifacts(&config) {
        Ok(report) if report.is_clean() => {
            println!(
                "Artifact Guard clean: no prohibited artifacts found in {} (mode: {}).",
                report.repo.display(),
                report.mode
            );
            Ok(())
        }
        Ok(report) => {
            print_violation_report(&report);
            Err(command_error::exit_error(1))
        }
        Err(error) => {
            eprintln!("Artifact Guard configuration error: {error}");
            eprintln!(
                "Fix the repository path, Git state, mode, or allowlist and rerun the guard."
            );
            Err(command_error::exit_error(2))
        }
    }
}

fn print_violation_report(report: &ArtifactGuardReport) {
    eprintln!(
        "Artifact Guard blocked {} prohibited artifact path(s) in {} (mode: {}).",
        report.violations.len(),
        report.repo.display(),
        report.mode
    );
    eprintln!();
    eprintln!("{:<16} {:<48} rule", "source", "path");
    for violation in &report.violations {
        eprintln!(
            "{:<16} {:<48} {}",
            violation.source, violation.path, violation.rule_id
        );
    }
    eprintln!();
    eprintln!("Remediation:");
    eprintln!("  - Remove local artifact leftovers from the parent worktree.");
    eprintln!(
        "  - Move generated, plugin, cache, and runtime output into an ignored isolated directory outside the parent worktree."
    );
    eprintln!(
        "  - If intentional source material, add a narrow reviewed entry to .amplihack-artifact-allowlist."
    );
    eprintln!();
    eprintln!("First violation detail:");
    if let Some(first) = report.violations.first() {
        eprintln!("  {}: {}", first.path, first.remediation);
    }
}
