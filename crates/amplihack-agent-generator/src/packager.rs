use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{GeneratorError, Result};
use crate::models::GoalAgentBundle;

/// Writes a [`GoalAgentBundle`] to disk as a self-contained package.
pub struct GoalAgentPackager;

impl GoalAgentPackager {
    pub fn new() -> Self {
        Self
    }

    /// Serialize *bundle* into *output_dir* and return the root path of the
    /// written package.
    pub fn package(&self, bundle: &GoalAgentBundle, output_dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(output_dir)?;
        let bundle_path = output_dir.join("bundle.json");
        let json = serde_json::to_string_pretty(bundle)
            .map_err(|e| GeneratorError::PackagingFailed(e.to_string()))?;
        fs::write(&bundle_path, json)?;
        Ok(output_dir.to_path_buf())
    }
}

impl Default for GoalAgentPackager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BundleStatus, GoalAgentBundle};

    #[test]
    fn package_writes_bundle_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut bundle = GoalAgentBundle::new("test-pkg", "0.1.0").unwrap();
        bundle.status = BundleStatus::Ready;

        let pkg = GoalAgentPackager::new();
        let out = pkg.package(&bundle, dir.path()).unwrap();
        assert_eq!(out, dir.path());

        let contents = fs::read_to_string(dir.path().join("bundle.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed["name"], "test-pkg");
        assert_eq!(parsed["version"], "0.1.0");
        assert_eq!(parsed["status"], "ready");
    }

    #[test]
    fn package_creates_output_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let bundle = GoalAgentBundle::new("nested-pkg", "1.0").unwrap();

        let pkg = GoalAgentPackager::new();
        let out = pkg.package(&bundle, &nested).unwrap();
        assert!(out.join("bundle.json").exists());
    }
}
