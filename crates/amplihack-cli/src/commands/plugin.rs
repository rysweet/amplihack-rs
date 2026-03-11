//! Native plugin management commands.

use crate::command_error::exit_error;
use amplihack_state::AtomicJsonFile;
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const SUCCESS_ICON: &str = "✅";
const FAILURE_ICON: &str = "❌";
const LINK_SUCCESS_ICON: &str = "✓";

#[derive(Debug, Clone)]
struct PluginManager {
    plugin_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallResult {
    success: bool,
    plugin_name: String,
    installed_path: PathBuf,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidationResult {
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationResult {
    success: bool,
    installed: bool,
    discoverable: bool,
    hooks_loaded: bool,
    issues: Vec<String>,
}

impl PluginManager {
    fn new(plugin_root: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            plugin_root: plugin_root.unwrap_or(default_plugin_root()?),
        })
    }

    fn install(&self, source: &str, force: bool) -> Result<InstallResult> {
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
            let result = Command::new("git")
                .arg("clone")
                .arg(source)
                .arg(&clone_path)
                .output()
                .context("failed to run git clone")?;

            if !result.status.success() {
                return Ok(InstallResult {
                    success: false,
                    plugin_name,
                    installed_path: PathBuf::new(),
                    message: format!(
                        "Git clone failed: {}",
                        String::from_utf8_lossy(&result.stderr).trim()
                    ),
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

    fn uninstall(&self, plugin_name: &str) -> Result<bool> {
        let plugin_path = self.plugin_root.join(plugin_name);
        if !plugin_path.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&plugin_path)
            .with_context(|| format!("failed to remove {}", plugin_path.display()))?;
        Ok(true)
    }

    fn validate_manifest(&self, manifest_path: &Path) -> Result<ValidationResult> {
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
            && !is_valid_semver(version)
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

    fn register_plugin(&self, plugin_name: &str) -> Result<bool> {
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

struct PluginVerifier {
    plugin_name: String,
    plugin_root: PathBuf,
    settings_path: PathBuf,
}

impl PluginVerifier {
    fn new(plugin_name: &str) -> Result<Self> {
        Ok(Self {
            plugin_name: plugin_name.to_string(),
            plugin_root: default_plugin_root()?.join(plugin_name),
            settings_path: home_dir()?.join(".claude").join("settings.json"),
        })
    }

    fn verify(&self) -> Result<VerificationResult> {
        let mut issues = Vec::new();
        let installed = self.check_installed();
        if !installed {
            issues.push(format!(
                "Plugin directory not found: {}",
                self.plugin_root.display()
            ));
        }

        let discoverable = self.check_discoverable()?;
        if !discoverable {
            issues.push(format!(
                "Plugin not found in {}",
                self.settings_path.display()
            ));
        }

        let hooks_loaded = self.check_hooks_loaded()?;
        if !hooks_loaded {
            issues.push("Hooks not registered or hooks.json missing".to_string());
        }

        Ok(VerificationResult {
            success: installed && discoverable && hooks_loaded,
            installed,
            discoverable,
            hooks_loaded,
            issues,
        })
    }

    fn check_installed(&self) -> bool {
        self.plugin_root
            .join(".claude-plugin")
            .join("plugin.json")
            .exists()
    }

    fn check_discoverable(&self) -> Result<bool> {
        if !self.settings_path.exists() {
            return Ok(false);
        }
        let settings: Value = serde_json::from_str(
            &fs::read_to_string(&self.settings_path)
                .with_context(|| format!("failed to read {}", self.settings_path.display()))?,
        )
        .unwrap_or(Value::Null);
        Ok(settings
            .get("enabledPlugins")
            .and_then(Value::as_array)
            .map(|plugins| {
                plugins
                    .iter()
                    .any(|value| value.as_str() == Some(self.plugin_name.as_str()))
            })
            .unwrap_or(false))
    }

    fn check_hooks_loaded(&self) -> Result<bool> {
        let hooks_json = self
            .plugin_root
            .join(".claude")
            .join("tools")
            .join("amplihack")
            .join("hooks")
            .join("hooks.json");
        if !hooks_json.exists() {
            return Ok(false);
        }
        let hooks: Value = serde_json::from_str(
            &fs::read_to_string(&hooks_json)
                .with_context(|| format!("failed to read {}", hooks_json.display()))?,
        )
        .unwrap_or(Value::Null);
        Ok(hooks
            .as_object()
            .map(|object| !object.is_empty())
            .unwrap_or(false)
            || hooks
                .as_array()
                .map(|array| !array.is_empty())
                .unwrap_or(false))
    }
}

pub fn run_install(source: &str, force: bool) -> Result<()> {
    let manager = PluginManager::new(None)?;
    let result = manager.install(source, force)?;
    if result.success {
        println!("{SUCCESS_ICON} Plugin installed: {}", result.plugin_name);
        println!("   Location: {}", result.installed_path.display());
        println!("   {}", result.message);
        return Ok(());
    }
    println!("{FAILURE_ICON} Installation failed: {}", result.message);
    Err(exit_error(1))
}

pub fn run_uninstall(plugin_name: &str) -> Result<()> {
    let manager = PluginManager::new(None)?;
    if manager.uninstall(plugin_name)? {
        println!("{SUCCESS_ICON} Plugin removed: {plugin_name}");
        return Ok(());
    }
    println!("{FAILURE_ICON} Failed to remove plugin: {plugin_name}");
    println!("   Plugin may not be installed or removal failed");
    Err(exit_error(1))
}

pub fn run_link(plugin_name: &str) -> Result<()> {
    let plugin_root = home_dir()?.join(".amplihack").join("plugins");
    let plugin_path = plugin_root.join(plugin_name);
    if !plugin_path.exists() {
        println!("Error: Plugin not found at {}", plugin_path.display());
        println!("Install the plugin first with: amplihack install");
        return Err(exit_error(1));
    }

    let manager = PluginManager::new(Some(plugin_root))?;
    if manager.register_plugin(plugin_name)? {
        println!("{LINK_SUCCESS_ICON} Plugin linked successfully: {plugin_name}");
        println!("  Settings updated in: ~/.claude/settings.json");
        println!("  Plugin should now appear in /plugin command");
        return Ok(());
    }

    println!("Error: Failed to link plugin: {plugin_name}");
    Err(exit_error(1))
}

pub fn run_verify(plugin_name: &str) -> Result<()> {
    let verifier = PluginVerifier::new(plugin_name)?;
    let result = verifier.verify()?;
    println!("Plugin: {plugin_name}");
    println!(
        "  Installed: {}",
        if result.installed {
            SUCCESS_ICON
        } else {
            FAILURE_ICON
        }
    );
    println!(
        "  Discoverable: {}",
        if result.discoverable {
            SUCCESS_ICON
        } else {
            FAILURE_ICON
        }
    );
    println!(
        "  Hooks loaded: {}",
        if result.hooks_loaded {
            SUCCESS_ICON
        } else {
            FAILURE_ICON
        }
    );

    if !result.success {
        println!();
        println!("Diagnostics:");
        for issue in &result.issues {
            println!("  - {issue}");
        }
        return Err(exit_error(1));
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else if kind.is_file() {
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        } else if kind.is_symlink() {
            let link_target = fs::read_link(&source)
                .with_context(|| format!("failed to read {}", source.display()))?;
            create_symlink(&link_target, &target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("failed to create symlink {}", link.display()))?;
    Ok(())
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    let metadata = fs::metadata(target)
        .with_context(|| format!("failed to stat symlink target {}", target.display()))?;
    if metadata.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
    .with_context(|| format!("failed to create symlink {}", link.display()))?;
    Ok(())
}

fn default_plugin_root() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("plugins"))
}

fn plugins_json_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".config")
        .join("claude-code")
        .join("plugins.json"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

fn is_path_safe(path: &Path, base: &Path) -> Result<bool> {
    let resolved_base = base
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", base.display()))?;
    let resolved_path = path
        .parent()
        .unwrap_or(base)
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    Ok(resolved_path.starts_with(&resolved_base))
}

fn plugin_name_from_git_url(source: &str) -> Result<String> {
    let mut name = source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    if let Some(stripped) = name.strip_suffix(".git") {
        name = stripped.to_string();
    }
    Ok(name)
}

fn is_valid_semver(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), Some(c), None)
            if !a.is_empty()
                && !b.is_empty()
                && !c.is_empty()
                && a.chars().all(|ch| ch.is_ascii_digit())
                && b.chars().all(|ch| ch.is_ascii_digit())
                && c.chars().all(|ch| ch.is_ascii_digit())
    )
}

fn is_valid_plugin_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_plugin_source(root: &Path, name: &str) -> PathBuf {
        let source = root.join(name);
        fs::create_dir_all(source.join(".claude-plugin")).unwrap();
        fs::create_dir_all(source.join(".claude/tools/amplihack/hooks")).unwrap();
        fs::write(
            source.join(".claude-plugin/plugin.json"),
            serde_json::json!({
                "name": name,
                "version": "1.2.3",
                "entry_point": "main.py",
                "description": "desc",
                "author": "me"
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            source.join(".claude/tools/amplihack/hooks/hooks.json"),
            r#"{"PreToolUse": []}"#,
        )
        .unwrap();
        source
    }

    #[test]
    fn validate_manifest_rejects_bad_name() {
        let temp = tempfile::tempdir().unwrap();
        let source = create_plugin_source(temp.path(), "demo");
        fs::write(
            source.join(".claude-plugin/plugin.json"),
            r#"{"name":"Bad_Name","version":"1.0.0","entry_point":"main.py"}"#,
        )
        .unwrap();
        let manager = PluginManager::new(Some(temp.path().join("plugins"))).unwrap();
        let result = manager
            .validate_manifest(&source.join(".claude-plugin/plugin.json"))
            .unwrap();
        assert!(!result.valid);
    }

    #[test]
    fn install_local_plugin_registers_it() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        let source = create_plugin_source(temp.path(), "demo");
        let manager = PluginManager::new(None).unwrap();
        let result = manager.install(source.to_str().unwrap(), false).unwrap();
        assert!(result.success);
        assert!(
            temp.path()
                .join(".amplihack/.claude/plugins/demo/.claude-plugin/plugin.json")
                .exists()
        );
        let plugins: Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join(".config/claude-code/plugins.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(plugins["enabledPlugins"][0], "demo");
        crate::test_support::restore_home(previous);
    }

    #[test]
    fn uninstall_missing_plugin_returns_false() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        let manager = PluginManager::new(None).unwrap();
        let result = manager.uninstall("missing").unwrap();
        assert!(!result);
        crate::test_support::restore_home(previous);
    }

    #[test]
    fn verifier_matches_python_checks() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        let source = create_plugin_source(temp.path(), "demo");
        fs::create_dir_all(temp.path().join(".amplihack/.claude/plugins")).unwrap();
        copy_dir_recursive(
            &source,
            &temp.path().join(".amplihack/.claude/plugins/demo"),
        )
        .unwrap();
        fs::create_dir_all(temp.path().join(".claude")).unwrap();
        fs::write(
            temp.path().join(".claude/settings.json"),
            r#"{"enabledPlugins":["demo"]}"#,
        )
        .unwrap();
        let result = PluginVerifier::new("demo").unwrap().verify().unwrap();
        assert!(result.success);
        crate::test_support::restore_home(previous);
    }

    #[test]
    fn link_uses_cli_specific_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        fs::create_dir_all(temp.path().join(".amplihack/plugins/amplihack")).unwrap();
        run_link("amplihack").unwrap();
        let plugins: Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join(".config/claude-code/plugins.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(plugins["enabledPlugins"][0], "amplihack");
        crate::test_support::restore_home(previous);
    }

    #[test]
    fn plugin_name_from_git_url_strips_dot_git() {
        assert_eq!(
            plugin_name_from_git_url("https://example.com/demo.git").unwrap(),
            "demo"
        );
    }

    #[test]
    fn semver_validation_is_strict() {
        assert!(is_valid_semver("1.2.3"));
        assert!(!is_valid_semver("1.2"));
        assert!(!is_valid_semver("1.2.beta"));
    }

    #[test]
    fn plugin_name_validation_matches_python_pattern() {
        assert!(is_valid_plugin_name("demo-plugin1"));
        assert!(!is_valid_plugin_name("Demo"));
        assert!(!is_valid_plugin_name("../demo"));
    }
}
