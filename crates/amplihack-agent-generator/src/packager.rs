use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::models::GoalAgentBundle;

/// Writes a [`GoalAgentBundle`] to disk as a self-contained package.
pub struct GoalAgentPackager;

impl GoalAgentPackager {
    pub fn new() -> Self {
        Self
    }

    /// Serialize *bundle* into *output_dir* and return the root path of the
    /// written package.
    pub fn package(&self, _bundle: &GoalAgentBundle, _output_dir: &Path) -> Result<PathBuf> {
        todo!("GoalAgentPackager::package not yet implemented")
    }
}

impl Default for GoalAgentPackager {
    fn default() -> Self {
        Self::new()
    }
}
