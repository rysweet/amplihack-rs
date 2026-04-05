//! CLI extensions for agent bundle generation, packaging, and distribution.
//!
//! Ports Python `amplihack/cli_extensions.py`. These are stub interfaces —
//! actual bundle generation is handled by a separate crate. This module
//! defines the public API surface for CLI integration.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{debug, info};

/// Generate an agent bundle from a prompt description.
///
/// # Arguments
/// * `prompt` — Natural-language description of the desired agent.
/// * `output` — Directory to write the generated bundle to.
/// * `validate` — Whether to run validation on the generated bundle.
/// * `test` — Whether to run tests on the generated bundle.
///
/// Returns the path to the generated bundle directory.
pub fn generate_bundle(prompt: &str, output: &Path, validate: bool, test: bool) -> Result<PathBuf> {
    info!(
        output = %output.display(),
        validate,
        test,
        "generating agent bundle (stub)"
    );
    debug!(
        prompt_len = prompt.len(),
        "bundle generation prompt received"
    );

    // Stub: create the output directory structure
    std::fs::create_dir_all(output)?;
    let manifest_path = output.join("manifest.json");
    let manifest = serde_json::json!({
        "name": "generated-agent",
        "version": "0.1.0",
        "prompt": prompt,
        "validated": validate,
        "tested": test,
    });
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    Ok(output.to_path_buf())
}

/// Package an agent bundle into a distributable archive.
///
/// # Arguments
/// * `bundle_path` — Path to the agent bundle directory.
/// * `format` — Archive format (`"tar.gz"` or `"zip"`).
/// * `output` — Path for the output archive.
///
/// Returns the path to the created package.
pub fn package_bundle(bundle_path: &Path, format: &str, output: &Path) -> Result<PathBuf> {
    info!(
        bundle = %bundle_path.display(),
        format,
        output = %output.display(),
        "packaging bundle (stub)"
    );

    if !bundle_path.is_dir() {
        anyhow::bail!(
            "bundle path does not exist or is not a directory: {}",
            bundle_path.display()
        );
    }

    // Stub: create an empty file at the output path
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, format!("stub-package-{}\n", format))?;

    Ok(output.to_path_buf())
}

/// Distribute a packaged bundle to one or more targets.
///
/// # Arguments
/// * `package_path` — Path to the packaged archive.
/// * `github` — Publish to GitHub Releases.
/// * `pypi` — Publish to PyPI.
/// * `local` — Install locally.
/// * `release` — Create a tagged release.
pub fn distribute_bundle(
    package_path: &Path,
    github: bool,
    pypi: bool,
    local: bool,
    release: bool,
) -> Result<()> {
    info!(
        package = %package_path.display(),
        github,
        pypi,
        local,
        release,
        "distributing bundle (stub)"
    );

    if !package_path.exists() {
        anyhow::bail!("package file not found: {}", package_path.display());
    }

    if github {
        debug!("would publish to GitHub Releases");
    }
    if pypi {
        debug!("would publish to PyPI");
    }
    if local {
        debug!("would install locally");
    }
    if release {
        debug!("would create tagged release");
    }

    Ok(())
}

/// Run the complete bundle pipeline: generate → package → distribute.
///
/// # Arguments
/// * `prompt` — Natural-language description of the desired agent.
/// * `output` — Directory for intermediate and final outputs.
/// * `format` — Archive format for packaging.
/// * `distribute` — Whether to distribute after packaging.
pub fn run_pipeline(prompt: &str, output: &Path, format: &str, distribute: bool) -> Result<()> {
    info!("running full bundle pipeline");

    let bundle_dir = output.join("bundle");
    let bundle_path = generate_bundle(prompt, &bundle_dir, true, false)?;

    let package_file = output.join(format!("agent-bundle.{}", format));
    let package_path = package_bundle(&bundle_path, format, &package_file)?;

    if distribute {
        distribute_bundle(&package_path, false, false, true, false)?;
    }

    info!(output = %output.display(), "pipeline complete");
    Ok(())
}

/// Register CLI extensions with the command parser.
///
/// Currently a no-op — extensions are registered via clap derive macros
/// in the main CLI module. This function exists for API compatibility
/// with the Python implementation.
pub fn register_cli_extensions() {
    debug!("CLI extensions registered (no-op — handled by clap derive)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_bundle_creates_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("test-bundle");
        let result = generate_bundle("test prompt", &output, true, false);
        assert!(result.is_ok());
        let bundle_path = result.unwrap();
        assert!(bundle_path.join("manifest.json").exists());

        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(bundle_path.join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["prompt"], "test prompt");
        assert_eq!(manifest["validated"], true);
    }

    #[test]
    fn package_bundle_requires_existing_dir() {
        let result = package_bundle(
            Path::new("/nonexistent/bundle"),
            "tar.gz",
            Path::new("/nonexistent/output.tar.gz"),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn distribute_bundle_requires_existing_file() {
        let result = distribute_bundle(
            Path::new("/nonexistent/package.tar.gz"),
            false,
            false,
            true,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn run_pipeline_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("pipeline-out");
        let result = run_pipeline("build an agent", &output, "tar.gz", false);
        assert!(result.is_ok());
        assert!(output.join("bundle").join("manifest.json").exists());
        assert!(output.join("agent-bundle.tar.gz").exists());
    }

    #[test]
    fn register_cli_extensions_does_not_panic() {
        register_cli_extensions();
    }
}
