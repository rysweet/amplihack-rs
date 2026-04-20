//! Validate YAML frontmatter in markdown skill/agent files.
//!
//! Scans a file (or all `.md` files in the current directory) for valid
//! YAML frontmatter delimited by `---` fences.

use anyhow::{Result, bail};
use std::fs;
use std::path::Path;

/// Run validation on a single file or all .md files in cwd.
pub fn run_validate_frontmatter(file: Option<&str>) -> Result<()> {
    match file {
        Some(path) => validate_single(Path::new(path)),
        None => validate_directory(Path::new(".")),
    }
}

fn validate_single(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", path.display()))?;

    match extract_and_validate_frontmatter(&content) {
        Ok(()) => {
            println!("\u{2713} {}: valid frontmatter", path.display());
            Ok(())
        }
        Err(e) => {
            eprintln!("\u{2717} {}: {e}", path.display());
            bail!("Frontmatter validation failed for {}", path.display());
        }
    }
}

fn validate_directory(dir: &Path) -> Result<()> {
    let mut found = false;
    let mut errors = 0;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            found = true;
            if let Err(e) = validate_single(&path) {
                eprintln!("{e}");
                errors += 1;
            }
        }
    }

    if !found {
        println!("No .md files found in {}", dir.display());
    }

    if errors > 0 {
        bail!("{errors} file(s) failed validation");
    }

    Ok(())
}

fn extract_and_validate_frontmatter(content: &str) -> Result<()> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        bail!("No frontmatter found (file must start with ---)");
    }

    let after_first_fence = &trimmed[3..];
    // Find closing ---
    let close_pos = after_first_fence
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("No closing --- found for frontmatter"))?;

    let yaml_content = &after_first_fence[..close_pos].trim();

    if yaml_content.is_empty() {
        bail!("Frontmatter is empty");
    }

    // Parse as YAML to validate
    let _: serde_yaml::Value = serde_yaml::from_str(yaml_content)
        .map_err(|e| anyhow::anyhow!("Invalid YAML in frontmatter: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_frontmatter() {
        let content = "---\nname: test\nversion: 1.0\n---\n# Body\nContent here.";
        assert!(extract_and_validate_frontmatter(content).is_ok());
    }

    #[test]
    fn no_frontmatter() {
        let content = "# Just a heading\nNo frontmatter here.";
        assert!(extract_and_validate_frontmatter(content).is_err());
    }

    #[test]
    fn invalid_yaml() {
        let content = "---\n: invalid: yaml: [broken\n---\n# Body";
        assert!(extract_and_validate_frontmatter(content).is_err());
    }

    #[test]
    fn empty_frontmatter() {
        let content = "---\n\n---\n# Body";
        assert!(extract_and_validate_frontmatter(content).is_err());
    }

    #[test]
    fn no_closing_fence() {
        let content = "---\nname: test\n# No closing fence";
        assert!(extract_and_validate_frontmatter(content).is_err());
    }

    #[test]
    fn complex_frontmatter() {
        let content = "---\nname: my-skill\nversion: 2.0.0\ndescription: |\n  Multi-line\n  description\ntags:\n  - cli\n  - tools\n---\n# Content";
        assert!(extract_and_validate_frontmatter(content).is_ok());
    }
}
