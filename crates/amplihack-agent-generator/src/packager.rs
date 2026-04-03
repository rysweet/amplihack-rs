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
    use crate::assembler::AgentAssembler;
    use crate::GoalDefinition;
    use crate::planner::ObjectivePlanner;
    use crate::synthesizer::SkillSynthesizer;

    fn make_bundle() -> GoalAgentBundle {
        let goal = GoalDefinition::new("prompt", "build tool", "development").unwrap();
        let plan = ObjectivePlanner::new().plan(&goal).unwrap();
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        AgentAssembler::new().assemble(&goal, &plan, skills).unwrap()
    }

    #[test]
    fn package_writes_bundle_json() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = make_bundle();
        let path = GoalAgentPackager::new().package(&bundle, dir.path()).unwrap();
        let contents = std::fs::read_to_string(path.join("bundle.json")).unwrap();
        let back: GoalAgentBundle = serde_json::from_str(&contents).unwrap();
        assert_eq!(back.name, bundle.name);
    }

    #[test]
    fn package_creates_output_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("sub").join("dir");
        let bundle = make_bundle();
        let path = GoalAgentPackager::new().package(&bundle, &nested).unwrap();
        assert!(path.join("bundle.json").exists());
    }

    #[test]
    fn default_impl() {
        let _p = GoalAgentPackager::default();
    }
}
