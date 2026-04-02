use super::helpers::{default_plugin_root, home_dir};
use super::*;

pub(super) struct PluginVerifier {
    plugin_name: String,
    plugin_root: PathBuf,
    settings_path: PathBuf,
}

impl PluginVerifier {
    pub(super) fn new(plugin_name: &str) -> Result<Self> {
        Ok(Self {
            plugin_name: plugin_name.to_string(),
            plugin_root: default_plugin_root()?.join(plugin_name),
            settings_path: home_dir()?.join(".claude").join("settings.json"),
        })
    }

    pub(super) fn verify(&self) -> Result<VerificationResult> {
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
