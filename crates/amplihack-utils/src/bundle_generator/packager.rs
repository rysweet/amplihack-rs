//! Filesystem packaging for agent bundles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::error::BundleGeneratorError;
use super::models::AgentBundle;

/// Unsafe system directories that must not be used as output targets.
const UNSAFE_PATHS: &[&str] = &[
    "/", "/etc", "/usr", "/bin", "/sbin", "/sys", "/proc", "/dev",
];

/// Creates complete filesystem packages for agent bundles.
///
/// Orchestrates writing agents, documentation, configuration, and scripts
/// to a target directory.
pub struct FilesystemPackager {
    output_dir: PathBuf,
}

impl FilesystemPackager {
    /// Create a new packager targeting `output_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Packaging`] if the path points to a
    /// system directory.
    pub fn new(output_dir: impl Into<PathBuf>) -> Result<Self, BundleGeneratorError> {
        let output_dir = output_dir.into();
        validate_output_dir(&output_dir)?;
        Ok(Self { output_dir })
    }

    /// Create a complete filesystem package for a bundle.
    ///
    /// Creates:
    /// - `agents/` — agent markdown files
    /// - `tests/` — test files
    /// - `docs/` — documentation
    /// - `config/` — configuration
    /// - `manifest.json` — bundle metadata
    /// - `README.md`
    ///
    /// Returns the path to the created package directory.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Packaging`] on I/O failures.
    pub fn create_package(
        &self,
        bundle: &AgentBundle,
        _options: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<PathBuf, BundleGeneratorError> {
        let package_name = format!("{}-{}", bundle.name, bundle.version);
        let package_path = self.output_dir.join(&package_name);

        // Create directory structure.
        for subdir in &["agents", "tests", "docs", "config"] {
            std::fs::create_dir_all(package_path.join(subdir))?;
        }

        // Write agent files.
        for agent in &bundle.agents {
            let agent_file = package_path
                .join("agents")
                .join(format!("{}.md", agent.name));
            std::fs::write(&agent_file, &agent.content)?;
        }

        // Write manifest.
        let manifest_path = package_path.join("manifest.json");
        let manifest_json = serde_json::to_string_pretty(bundle)?;
        std::fs::write(&manifest_path, manifest_json)?;

        // Write README.
        let readme = format!(
            "# {}\n\n{}\n\n## Agents\n\n{}\n",
            bundle.name,
            bundle.description,
            bundle
                .agents
                .iter()
                .map(|a| format!("- **{}**: {}", a.name, a.description))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write(package_path.join("README.md"), readme)?;

        Ok(package_path)
    }
}

/// Validate that `output_dir` is not a system directory.
fn validate_output_dir(path: &Path) -> Result<(), BundleGeneratorError> {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let resolved_str = resolved.to_string_lossy();

    for &unsafe_path in UNSAFE_PATHS {
        if resolved_str == unsafe_path {
            return Err(BundleGeneratorError::Packaging {
                message: format!(
                    "Cannot write to system directory: {resolved_str}. \
                     Choose a user directory for output."
                ),
                format: None,
                path: Some(resolved_str.into_owned()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_output_dir_rejects_root() {
        let result = validate_output_dir(Path::new("/"));
        assert!(result.is_err());
    }
}
