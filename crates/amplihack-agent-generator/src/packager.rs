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
