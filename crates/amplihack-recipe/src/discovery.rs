//! Recipe discovery — find, list, and cache recipe YAML files.
//!
//! Matches Python `amplihack/recipes/discovery.py`:
//! - Search-path priority: package → global → local
//! - Recipe metadata extraction from YAML front matter
//! - Content hashing for change detection
//! - Qualified keys (search_dir:name) and bare names

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Metadata for a discovered recipe.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecipeInfo {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub description: Option<String>,
    pub content_hash: String,
    pub qualified_key: String,
}

/// Cache of discovered recipes with qualified and bare name lookup.
pub struct RecipeCache {
    by_qualified: HashMap<String, RecipeInfo>,
    bare_to_qualified: HashMap<String, String>,
}

impl RecipeCache {
    pub fn new() -> Self {
        Self {
            by_qualified: HashMap::new(),
            bare_to_qualified: HashMap::new(),
        }
    }

    fn register(&mut self, info: RecipeInfo) {
        let bare = info.name.clone();
        let qualified = info.qualified_key.clone();
        // Last-wins: later search directories override earlier ones (Python parity).
        self.bare_to_qualified.insert(bare, qualified.clone());
        self.by_qualified.insert(qualified, info);
    }

    /// Look up by qualified key or bare name.
    pub fn get(&self, key: &str) -> Option<&RecipeInfo> {
        self.by_qualified.get(key).or_else(|| {
            self.bare_to_qualified
                .get(key)
                .and_then(|qk| self.by_qualified.get(qk))
        })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// All qualified keys.
    pub fn qualified_keys(&self) -> Vec<&str> {
        self.by_qualified.keys().map(|s| s.as_str()).collect()
    }

    /// Map of bare names to their last-registered qualified key.
    pub fn bare_name_map(&self) -> &HashMap<String, String> {
        &self.bare_to_qualified
    }

    pub fn len(&self) -> usize {
        self.by_qualified.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_qualified.is_empty()
    }
}

impl Default for RecipeCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Default search directories for recipes, in priority order.
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".amplihack/.claude/recipes"));
    }
    // AMPLIHACK_HOME-relative
    if let Ok(ah) = std::env::var("AMPLIHACK_HOME") {
        let ah = PathBuf::from(ah);
        dirs.push(ah.join("amplifier-bundle/recipes"));
        dirs.push(ah.join(".claude/recipes"));
    }
    dirs.push(PathBuf::from("amplifier-bundle/recipes"));
    dirs.push(PathBuf::from("src/amplihack/amplifier-bundle/recipes"));
    dirs.push(PathBuf::from(".claude/recipes"));
    dirs
}

/// Discover all recipes in the given search directories.
pub fn discover_recipes(search_dirs: &[PathBuf]) -> RecipeCache {
    let mut cache = RecipeCache::new();
    for dir in search_dirs {
        if !dir.is_dir() {
            continue;
        }
        debug!(dir = %dir.display(), "Scanning for recipes");
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_recipe_file(&path)
                        && let Some(info) = load_recipe_info(&path, dir)
                    {
                        cache.register(info);
                    }
                }
            }
            Err(e) => {
                debug!(dir = %dir.display(), error = %e, "Could not read recipe directory");
            }
        }
    }
    debug!(count = cache.len(), "Recipes discovered");
    cache
}

/// List all discoverable recipes.
pub fn list_recipes(search_dirs: Option<&[PathBuf]>) -> Vec<RecipeInfo> {
    let default = default_search_dirs();
    let dirs = search_dirs.unwrap_or(&default);
    let cache = discover_recipes(dirs);
    let mut recipes: Vec<RecipeInfo> = cache.by_qualified.into_values().collect();
    recipes.sort_by(|a, b| a.name.cmp(&b.name));
    recipes
}

/// Find a specific recipe by name.
pub fn find_recipe(name: &str, search_dirs: Option<&[PathBuf]>) -> Option<PathBuf> {
    let default = default_search_dirs();
    let dirs = search_dirs.unwrap_or(&default);
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        for ext in &["yaml", "yml"] {
            let candidate = dir.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Verify global recipe installation.
pub fn verify_global_installation() -> VerifyResult {
    let dirs = default_search_dirs();
    let found_dirs: Vec<_> = dirs.iter().filter(|d| d.is_dir()).cloned().collect();
    let cache = discover_recipes(&found_dirs);
    VerifyResult {
        search_dirs_checked: dirs.len(),
        search_dirs_found: found_dirs.len(),
        recipes_found: cache.len(),
        recipe_names: cache
            .qualified_keys()
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Result of global installation verification.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyResult {
    pub search_dirs_checked: usize,
    pub search_dirs_found: usize,
    pub recipes_found: usize,
    pub recipe_names: Vec<String>,
}

/// Compute content hash for change detection.
pub fn file_hash(path: &Path) -> Result<String> {
    let content =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(format!("{:x}", hasher.finalize()))
}

fn load_recipe_info(path: &Path, search_dir: &Path) -> Option<RecipeInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    let name = path.file_stem()?.to_str()?.to_string();
    let qualified_key = qualified_key(search_dir, &name);
    let content_hash = {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    };

    // Extract version and description from YAML
    let (version, description) = match serde_yaml::from_str::<serde_yaml::Value>(&content) {
        Ok(doc) => {
            let version = doc
                .get("version")
                .and_then(|v| v.as_str())
                .map(String::from);
            let description = doc
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from);
            (version, description)
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Failed to parse recipe YAML");
            (None, None)
        }
    };

    Some(RecipeInfo {
        name,
        path: path.to_path_buf(),
        version,
        description,
        content_hash,
        qualified_key,
    })
}

fn qualified_key(search_dir: &Path, name: &str) -> String {
    let dir_name = search_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    format!("{dir_name}:{name}")
}

fn is_recipe_file(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e == "yaml" || e == "yml")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_finds_yaml_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test-recipe.yaml"),
            "name: test\nversion: '1.0'\ndescription: A test recipe\nsteps: []",
        )
        .unwrap();
        std::fs::write(dir.path().join("not-recipe.txt"), "ignored").unwrap();

        let cache = discover_recipes(&[dir.path().to_path_buf()]);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains("test-recipe"));
    }

    #[test]
    fn qualified_and_bare_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let recipes = dir.path().join("recipes");
        std::fs::create_dir_all(&recipes).unwrap();
        std::fs::write(recipes.join("my-recipe.yaml"), "name: my-recipe\nsteps: []").unwrap();

        let cache = discover_recipes(&[recipes]);
        let qualified = cache.qualified_keys();
        assert_eq!(qualified.len(), 1);
        assert!(qualified[0].contains("my-recipe"));

        // Bare name lookup
        assert!(cache.get("my-recipe").is_some());
    }

    #[test]
    fn find_recipe_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("default-workflow.yaml"), "steps: []").unwrap();
        let found = find_recipe("default-workflow", Some(&[dir.path().to_path_buf()]));
        assert!(found.is_some());
        assert!(find_recipe("nonexistent", Some(&[dir.path().to_path_buf()])).is_none());
    }

    #[test]
    fn file_hash_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.yaml");
        std::fs::write(&file, "content: hello").unwrap();
        let h1 = file_hash(&file).unwrap();
        let h2 = file_hash(&file).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn verify_result_serializes() {
        let result = VerifyResult {
            search_dirs_checked: 5,
            search_dirs_found: 2,
            recipes_found: 3,
            recipe_names: vec!["a".into(), "b".into()],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["recipes_found"], 3);
    }

    #[test]
    fn recipe_info_extracts_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("smart.yaml"),
            "version: '2.0'\ndescription: Smart orchestrator\nsteps: []",
        )
        .unwrap();
        let cache = discover_recipes(&[dir.path().to_path_buf()]);
        let info = cache.get("smart").unwrap();
        assert_eq!(info.version.as_deref(), Some("2.0"));
        assert_eq!(info.description.as_deref(), Some("Smart orchestrator"));
    }

    #[test]
    fn empty_cache() {
        let cache = RecipeCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert!(cache.get("anything").is_none());
    }
}
