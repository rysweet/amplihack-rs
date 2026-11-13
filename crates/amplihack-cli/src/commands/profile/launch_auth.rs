//! Fallback's effective authentication contract. Unknown substitutions refuse;
//! credential values are never retained, compared, printed or serialised.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

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
                || matches!(key.as_str(), "apiKeyHelper" | "primaryApiKey" | "awsAuthRefresh" | "awsCredentialExport")
                || (key == "forceLoginMethod" && value.as_str() != Some("claudeai"))
                // Namespace overrides in settings cannot be bound to the
                // identity client's process environment.
                || key == "CLAUDE_CODE_CUSTOM_OAUTH_URL"
                || key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                || key == "CLAUDE_CONFIG_DIR"
            {
                return Err(REFUSED);
            }
            settings(value)?;
        }
    }
    Ok(())
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
        settings(&serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| REFUSED)?)?;
    }
    Ok(())
}

/// Before identity requests, inspect inherited configuration. Before spawn,
/// repeat against the built command, including CLI settings and its cwd.
/// No overrides are silently removed to make a fallback appear compatible.
pub(super) fn validate(dir: &Path, cmd: Option<&Command>) -> Result<(), &'static str> {
    if OVERRIDES
        .iter()
        .any(|key| env(cmd, key).is_some_and(|v| !v.is_empty()))
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
    if let Some(home) = env(cmd, "HOME") {
        for name in [
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".claude.json",
        ] {
            file(&Path::new(&home).join(name))?;
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
    #[cfg(target_os = "linux")]
    if isolated_root.is_none() {
        file(Path::new("/etc/claude-code/managed-settings.json"))?;
    }
    #[cfg(target_os = "macos")]
    if isolated_root.is_none() {
        file(Path::new(
            "/Library/Application Support/ClaudeCode/managed-settings.json",
        ))?;
    }
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
