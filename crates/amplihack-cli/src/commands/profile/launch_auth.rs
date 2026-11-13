//! Fallback's effective authentication contract. Unknown substitutions refuse;
//! credential values are never retained, compared, printed or serialised.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

mod managed;

const REFUSED: &str = "fallback authentication cannot be bound to the child: remove authentication or billing-route overrides and retry";
const OVERRIDES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_API_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "AMPLIHACK_LITELLM_ENDPOINT",
    "AMPLIHACK_LITELLM_API_KEY",
    "AMPLIHACK_LITELLM_MODEL",
];
// Even an empty selector can occupy a credential-precedence slot. These
// sources are not the profile-owned claude.ai login validated for fallback.
const CREDENTIAL_SELECTORS: &[&str] = &[
    "ANTHROPIC_PROFILE",
    "ANTHROPIC_CONFIG_DIR",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_ORGANIZATION_ID",
    "ANTHROPIC_IDENTITY_TOKEN_FILE",
    "ANTHROPIC_IDENTITY_TOKEN",
    "ANTHROPIC_WORKSPACE_ID",
    "ANTHROPIC_SERVICE_ACCOUNT_ID",
    "ANTHROPIC_SCOPE",
    "CLAUDE_CODE_FEDERATION_CACHE_DIR",
];

fn env(cmd: Option<&Command>, key: &str) -> Option<OsString> {
    if let Some((_, value)) = cmd.and_then(|c| c.get_envs().find(|(k, _)| *k == key)) {
        return value.map(OsString::from);
    }
    std::env::var_os(key)
}

fn settings(value: &serde_json::Value) -> Result<(), &'static str> {
    if let Some(obj) = value.as_object() {
        for (key, value) in obj {
            if (OVERRIDES.contains(&key.as_str()) && value.as_str() != Some(""))
                || CREDENTIAL_SELECTORS.contains(&key.as_str())
                || matches!(key.as_str(), "apiKeyHelper" | "primaryApiKey" | "awsAuthRefresh" | "awsCredentialExport" | "policyHelper" | "policyHelpers")
                || (key == "forceLoginMethod" && value.as_str() != Some("claudeai"))
                // Namespace overrides in settings cannot be bound to the
                // identity client's process environment.
                || key == "CLAUDE_CODE_CUSTOM_OAUTH_URL"
                || key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                || key == "CLAUDE_CONFIG_DIR"
                || matches!(key.as_str(), "HOME" | "XDG_CONFIG_HOME" | "APPDATA" | "USERPROFILE")
            {
                return Err(REFUSED);
            }
            settings(value)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn final_child_command_rechecks_managed_dropins_without_a_main_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let dir = root.join("profile");
        std::fs::create_dir(&dir).unwrap();
        let mut cmd = Command::new("fixture-claude");
        cmd.current_dir(&root)
            .env("HOME", &root)
            .env("XDG_CONFIG_HOME", &root)
            .env("TMPDIR", &root)
            .env("AMPLIHACK_TEST_HOME_GUARD", "1")
            .env_remove("WSL_DISTRO_NAME")
            .env_remove("WSL_INTEROP")
            .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR");
        for key in OVERRIDES.iter().chain(CREDENTIAL_SELECTORS) {
            cmd.env_remove(key);
        }
        assert!(validate(&dir, Some(&cmd)).is_ok());
        let path = root.join("managed/managed-settings.d/20-auth.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"fixture-secret"}}"#,
        )
        .unwrap();
        assert!(!root.join("managed/managed-settings.json").exists());
        assert_eq!(validate(&dir, Some(&cmd)), Err(REFUSED));
    }
}

fn file(path: &Path) -> Result<(), &'static str> {
    #[cfg(unix)]
    let bytes = super::fsx::read_bounded_nofollow(path, 1024 * 1024)
        .map_err(|_| REFUSED)?
        .map(|(bytes, _)| bytes);
    #[cfg(not(unix))]
    let bytes = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(REFUSED),
    };
    if let Some(bytes) = bytes {
        document(&bytes)?;
    }
    Ok(())
}

fn document(bytes: &[u8]) -> Result<(), &'static str> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).map_err(|_| REFUSED)?;
    if !value.is_object() {
        return Err(REFUSED);
    }
    settings(&value)
}

/// Before identity requests, inspect inherited configuration. Before spawn,
/// repeat against the built command, including CLI settings and its cwd.
/// No overrides are silently removed to make a fallback appear compatible.
pub(super) fn validate(dir: &Path, cmd: Option<&Command>) -> Result<(), &'static str> {
    #[cfg(target_os = "linux")]
    if ["WSL_DISTRO_NAME", "WSL_INTEROP"]
        .iter()
        .any(|key| env(cmd, key).is_some())
    {
        return Err(
            "fallback authentication cannot be bound to inherited Windows managed policy on WSL: use an explicit profile selection",
        );
    }
    if OVERRIDES
        .iter()
        .any(|key| env(cmd, key).is_some_and(|v| !v.is_empty()))
        || CREDENTIAL_SELECTORS
            .iter()
            .any(|key| env(cmd, key).is_some())
    {
        return Err(REFUSED);
    }
    if let Some(storage) = env(cmd, "CLAUDE_SECURESTORAGE_CONFIG_DIR").filter(|v| !v.is_empty())
        && Path::new(&storage)
            .canonicalize()
            .ok()
            .zip(dir.canonicalize().ok())
            .is_none_or(|(a, b)| a != b)
    {
        return Err(REFUSED);
    }
    for name in ["settings.json", "settings.local.json", ".claude.json"] {
        file(&dir.join(name))?;
    }
    // A cached remote policy may contain helper output or a version-specific
    // envelope. Do not claim to validate that format as a local settings file.
    managed::absent(&dir.join("remote-settings.json"))?;
    if let Some(home) = env(cmd, "HOME") {
        // An implicit Console federation profile can outrank claude.ai OAuth
        // without any explicit selector. Its credentials are never opened.
        let console_home = home.to_str().ok_or(REFUSED)?.trim();
        if !Path::new(console_home).is_absolute() {
            return Err(REFUSED);
        }
        for name in [
            ".config/anthropic/active_config",
            ".config/anthropic/configs/default.json",
        ] {
            managed::absent(&Path::new(console_home).join(name))?;
        }
        for name in [
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".claude.json",
        ] {
            file(&Path::new(&home).join(name))?;
        }
    }
    if let Some(xdg) = env(cmd, "XDG_CONFIG_HOME") {
        let xdg = xdg.to_str().ok_or(REFUSED)?.trim();
        if !xdg.is_empty() {
            let xdg = Path::new(xdg);
            if !xdg.is_absolute() {
                return Err(REFUSED);
            }
            for name in ["anthropic/active_config", "anthropic/configs/default.json"] {
                managed::absent(&xdg.join(name))?;
            }
        }
    }
    let cwd = cmd
        .and_then(Command::get_current_dir)
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .ok_or(REFUSED)?;
    // The existing home/endpoint test guard also confines project discovery
    // to the fixture's TMPDIR. A fixture nested below a real home must never
    // open that home's settings while walking ancestors, or machine policy.
    #[cfg(debug_assertions)]
    let isolated_root = if env(cmd, "AMPLIHACK_TEST_HOME_GUARD").is_some_and(|v| !v.is_empty()) {
        let root = env(cmd, "TMPDIR").ok_or(REFUSED)?;
        let root = Path::new(&root).canonicalize().map_err(|_| REFUSED)?;
        if !cwd.starts_with(&root) {
            return Err(REFUSED);
        }
        Some(root)
    } else {
        None
    };
    #[cfg(not(debug_assertions))]
    let isolated_root: Option<std::path::PathBuf> = None;
    for parent in cwd.ancestors() {
        if isolated_root
            .as_ref()
            .is_some_and(|root| !parent.starts_with(root))
        {
            break;
        }
        file(&parent.join(".claude/settings.json"))?;
        file(&parent.join(".claude/settings.local.json"))?;
    }
    managed::validate(isolated_root.as_deref())?;
    if let Some(cmd) = cmd {
        let mut args = cmd.get_args();
        while let Some(arg) = args.next() {
            let value = if arg == "--settings" {
                Some(args.next().ok_or(REFUSED)?.to_str().ok_or(REFUSED)?)
            } else {
                arg.to_str().and_then(|s| s.strip_prefix("--settings="))
            };
            if let Some(value) = value {
                if value.trim_start().starts_with('{') {
                    settings(
                        &serde_json::from_str::<serde_json::Value>(value).map_err(|_| REFUSED)?,
                    )?;
                } else {
                    file(&cwd.join(value))?;
                }
            }
        }
    }
    Ok(())
}
