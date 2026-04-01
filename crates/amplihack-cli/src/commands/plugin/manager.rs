use super::*;
use super::helpers::{
    copy_dir_recursive, is_path_safe, is_valid_plugin_name, plugin_name_from_git_url,
};

#[derive(Debug, Clone)]
pub(super) struct PluginManager {
    plugin_root: PathBuf,
}

impl PluginManager {
    pub(super) fn new(plugin_root: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            plugin_root: plugin_root.unwrap_or(default_plugin_root()?),
        })
    }

    pub(super) fn install(&self, source: &str, force: bool) -> Result<InstallResult> {
        if source.trim().is_empty() {
            return Ok(InstallResult {
                success: false,
                plugin_name: String::new(),
                installed_path: PathBuf::new(),
                message: "Empty source provided".to_string(),
            });
        }

        let is_git_url = source.starts_with("http://")
            || source.starts_with("https://")
            || source.starts_with("git@");

        let mut temp_dir: Option<TempDir> = None;
        let (plugin_name, source_path) = if is_git_url {
            let plugin_name = plugin_name_from_git_url(source)?;
            if !is_valid_plugin_name(&plugin_name) {
                return Ok(InstallResult {
                    success: false,
                    plugin_name,
                    installed_path: PathBuf::new(),
                    message: "Invalid plugin name from URL: must be lowercase letters, numbers, hyphens only".to_string(),
                });
            }

            let dir = tempfile::tempdir().context("failed to create temp dir for plugin clone")?;
            let clone_path = dir.path().join(&plugin_name);

            let mut git_cmd = Command::new("git");
            git_cmd.arg("clone").arg(source).arg(&clone_path);
            let status = match run_with_timeout(git_cmd, GIT_CLONE_TIMEOUT) {
                Ok(s) => s,
                Err(err) => {
                    return Ok(InstallResult {
                        success: false,
                        plugin_name,
                        installed_path: PathBuf::new(),
                        message: format!("Git clone error: {err}"),
                    });
                }
            };

            if !status.success() {
                return Ok(InstallResult {
                    success: false,
                    plugin_name,
                    installed_path: PathBuf::new(),
                    message: "Git clone failed (non-zero exit status)".to_string(),
                });
            }

            temp_dir = Some(dir);
            (plugin_name, clone_path)
        } else {
            let source_path = PathBuf::from(source);
            if !source_path.is_dir() {
                return Ok(InstallResult {
                    success: false,
                    plugin_name: String::new(),
                    installed_path: PathBuf::new(),
                    message: format!("Source must be a directory: {source}"),
                });
            }
            let plugin_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            (plugin_name, source_path)
        };

        let manifest_path = source_path.join(".claude-plugin").join("plugin.json");
        if !is_path_safe(&manifest_path, &source_path)? {
            return Ok(InstallResult {
                success: false,
                plugin_name,
                installed_path: PathBuf::new(),
                message: "Manifest path traversal detected".to_string(),
            });
        }

        let validation = self.validate_manifest(&manifest_path)?;
        if !validation.valid {
            return Ok(InstallResult {
                success: false,
                plugin_name,
                installed_path: PathBuf::new(),
                message: format!("Invalid manifest: {}", validation.errors.join(", ")),
            });
        }

        let target_path = self.plugin_root.join(&plugin_name);
        if target_path.exists() && !force {
            return Ok(InstallResult {
                success: false,
                plugin_name: plugin_name.clone(),
                installed_path: target_path.clone(),
                message: format!(
                    "Plugin already installed: {} (use force=True to overwrite)",
                    target_path
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or_default()
                ),
            });
        }

        if target_path.exists() && force {
            fs::remove_dir_all(&target_path)
                .with_context(|| format!("failed to remove {}", target_path.display()))?;
        }

        fs::create_dir_all(&self.plugin_root)
            .with_context(|| format!("failed to create {}", self.plugin_root.display()))?;
        copy_dir_recursive(&source_path, &target_path)?;

        if !self.register_plugin(&plugin_name)? {
            return Ok(InstallResult {
                success: false,
                plugin_name: plugin_name.clone(),
                installed_path: target_path.clone(),
                message: format!("Plugin copied but registration failed: {}", plugin_name),
            });
        }

        drop(temp_dir);

        Ok(InstallResult {
            success: true,
            plugin_name: plugin_name.clone(),
            installed_path: target_path,
            message: format!("Plugin installed successfully: {plugin_name}"),
        })
    }

    pub(super) fn uninstall(&self, plugin_name: &str) -> Result<bool> {
        let plugin_path = self.plugin_root.join(plugin_name);
        if !plugin_path.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&plugin_path)
            .with_context(|| format!("failed to remove {}", plugin_path.display()))?;
        Ok(true)
    }

    pub(super) fn validate_manifest(&self, manifest_path: &Path) -> Result<ValidationResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if !manifest_path.exists() {
            errors.push(format!(
                "Manifest file not found: {}",
                manifest_path.display()
            ));
            return Ok(ValidationResult {
                valid: false,
                errors,
                warnings,
            });
        }

        let manifest_text = fs::read_to_string(manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let raw_value: Value = match serde_json::from_str(&manifest_text) {
            Ok(value) => value,
            Err(error) => {
                errors.push(format!("Invalid JSON in manifest: {error}"));
                return Ok(ValidationResult {
                    valid: false,
                    errors,
                    warnings,
                });
            }
        };

        let object = raw_value
            .as_object()
            .context("manifest JSON must be an object")?;

        for field in ["name", "version", "entry_point"] {
            if !object.contains_key(field) {
                errors.push(format!("Missing required field: {field}"));
            }
        }

        if let Some(version) = object.get("version").and_then(Value::as_str)
            && !super::helpers::is_valid_semver(version)
        {
            errors.push(format!(
                "Invalid version format: {} (expected semver like 1.0.0)",
                version
            ));
        }

        if let Some(name) = object.get("name").and_then(Value::as_str)
            && !is_valid_plugin_name(name)
        {
            errors.push(format!(
                "Invalid name format: {} (must be lowercase letters, numbers, hyphens only)",
                name
            ));
        }

        for field in ["description", "author"] {
            if !object.contains_key(field) {
                warnings.push(format!("Missing recommended field: {field}"));
            }
        }

        Ok(ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings,
        })
    }

    pub(super) fn register_plugin(&self, plugin_name: &str) -> Result<bool> {
        let settings_path = plugins_json_path()?;
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let file = AtomicJsonFile::new(&settings_path);
        let result: Value = file
            .update(|settings: &mut Value| {
                if !settings.is_object() {
                    *settings = Value::Object(Map::new());
                }
                let object = settings.as_object_mut().expect("object just created");
                let plugins = object
                    .entry("enabledPlugins")
                    .or_insert_with(|| Value::Array(vec![]));
                if !plugins.is_array() {
                    *plugins = Value::Array(vec![]);
                }
                let list = plugins.as_array_mut().expect("array just created");
                if !list.iter().any(|value| value.as_str() == Some(plugin_name)) {
                    list.push(Value::String(plugin_name.to_string()));
                }
            })
            .with_context(|| format!("failed to update {}", settings_path.display()))?;
        Ok(result
            .get("enabledPlugins")
            .and_then(Value::as_array)
            .is_some())
    }
}
