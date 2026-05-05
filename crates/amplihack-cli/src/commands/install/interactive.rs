//! Interactive install wizard for `amplihack install --interactive` (issue #433).
//!
//! Architecture: two layers —
//! - **Pure logic layer** (this file, ~90%): enums, config building, validation,
//!   scope resolution. Fully unit-testable without stdin.
//! - **Thin prompt layer** (`run_wizard`): ~30 lines calling `dialoguer::Select`.
//!   Not unit-tested (requires real TTY). Covered by manual/integration tests.

use super::types::InstallManifest;
#[cfg(test)]
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Default tool launched by bare `amplihack` invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefaultTool {
    Claude,
    Copilot,
    Codex,
}

impl DefaultTool {
    /// Human-readable name shown in the wizard prompt.
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Copilot => "GitHub Copilot",
            Self::Codex => "OpenAI Codex CLI",
        }
    }

    /// Stable lowercase string for manifest serialization.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
        }
    }

    /// All variants in display order (Claude first — most common default).
    pub(crate) fn all_variants() -> Vec<Self> {
        vec![Self::Claude, Self::Copilot, Self::Codex]
    }
}

/// Where amplihack hooks are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookScope {
    Global,
    RepoLocal,
}

impl HookScope {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Global => "Global (~/.claude)",
            Self::RepoLocal => "Repo-local (.claude)",
        }
    }

    pub(crate) fn all_variants() -> Vec<Self> {
        vec![Self::Global, Self::RepoLocal]
    }

    /// Resolve the settings.json path for this scope.
    ///
    /// - `Global`: `~/.claude/settings.json` (ignores `repo_root`).
    /// - `RepoLocal`: `<repo_root>/.claude/settings.json`.
    ///
    /// Currently exercised by tests; will be consumed by hook-wiring
    /// integration when repo-local scope writes land.
    #[cfg(test)]
    pub(crate) fn settings_path_for(self, repo_root: &Path) -> PathBuf {
        match self {
            Self::Global => {
                let home = dirs_home().unwrap_or_else(|| PathBuf::from("~"));
                home.join(".claude").join("settings.json")
            }
            Self::RepoLocal => repo_root.join(".claude").join("settings.json"),
        }
    }
}

/// Update-check cadence preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckPreference {
    AutoWeekly,
    AutoDaily,
    Manual,
    Disabled,
}

impl UpdateCheckPreference {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::AutoWeekly => "Auto (weekly)",
            Self::AutoDaily => "Auto (daily)",
            Self::Manual => "Manual only",
            Self::Disabled => "Disabled",
        }
    }

    /// Stable string for manifest persistence.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AutoWeekly => "auto-weekly",
            Self::AutoDaily => "auto-daily",
            Self::Manual => "manual",
            Self::Disabled => "disabled",
        }
    }

    pub(crate) fn all_variants() -> Vec<Self> {
        vec![
            Self::AutoWeekly,
            Self::AutoDaily,
            Self::Manual,
            Self::Disabled,
        ]
    }
}

// ---------------------------------------------------------------------------
// InteractiveConfig
// ---------------------------------------------------------------------------

/// Configuration produced by the interactive wizard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InteractiveConfig {
    pub default_tool: DefaultTool,
    pub hook_scope: HookScope,
    pub update_check: UpdateCheckPreference,
}

impl Default for InteractiveConfig {
    fn default() -> Self {
        Self {
            default_tool: DefaultTool::Claude,
            hook_scope: HookScope::Global,
            update_check: UpdateCheckPreference::AutoWeekly,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure logic functions (testable)
// ---------------------------------------------------------------------------

/// Check whether stdin is connected to a real terminal.
pub(crate) fn validate_tty() -> bool {
    atty_check()
}

/// Platform TTY check (uses libc on unix).
fn atty_check() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::isatty(libc::STDIN_FILENO) != 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Resolve the effective hook scope, falling back to Global if repo-local
/// was requested but no git repository exists at `cwd`.
///
/// Currently exercised by tests; will be consumed by hook-wiring
/// integration when repo-local scope writes land.
#[cfg(test)]
pub(crate) fn resolve_hook_scope(scope: HookScope, cwd: &Path) -> HookScope {
    match scope {
        HookScope::Global => HookScope::Global,
        HookScope::RepoLocal => {
            if cwd.join(".git").exists() {
                HookScope::RepoLocal
            } else {
                eprintln!(
                    "warning: repo-local scope requested but no .git found in {}; \
                     falling back to global scope",
                    cwd.display()
                );
                HookScope::Global
            }
        }
    }
}

/// Entry point: decide whether to run the wizard and return config.
///
/// - `interactive=false` → `Ok(None)` (skip wizard, use existing defaults).
/// - `interactive=true` + non-TTY → `Ok(Some(default config))` with stderr warning.
/// - `interactive=true` + TTY → runs the interactive `dialoguer` prompts.
pub(crate) fn maybe_run_wizard(interactive: bool) -> anyhow::Result<Option<InteractiveConfig>> {
    if !interactive {
        return Ok(None);
    }

    if !validate_tty() {
        eprintln!(
            "warning: --interactive requires a terminal; falling back to default configuration"
        );
        return Ok(Some(InteractiveConfig::default()));
    }

    let config = run_wizard()?;
    Ok(Some(config))
}

/// Apply wizard results to the install manifest.
pub(super) fn apply_config(config: &InteractiveConfig, manifest: &mut InstallManifest) {
    manifest.default_tool = Some(config.default_tool.as_str().to_string());
    manifest.update_check_preference = Some(config.update_check.as_str().to_string());
}

// ---------------------------------------------------------------------------
// Thin prompt layer (dialoguer — not unit-tested)
// ---------------------------------------------------------------------------

/// Run the interactive wizard using `dialoguer::Select`.
///
/// Only called when stdin is a real TTY.
fn run_wizard() -> anyhow::Result<InteractiveConfig> {
    use dialoguer::Select;

    println!();
    println!("🧙 Interactive Install Wizard");
    println!("   Configure your amplihack installation preferences.");
    println!();

    // 1. Default tool selection
    let tools = DefaultTool::all_variants();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.display_name()).collect();
    let tool_idx = Select::new()
        .with_prompt("Default launch tool (bare `amplihack` command)")
        .items(&tool_names)
        .default(0)
        .interact()?;
    let default_tool = tools[tool_idx];

    // 2. Hook scope selection
    let scopes = HookScope::all_variants();
    let scope_names: Vec<&str> = scopes.iter().map(|s| s.display_name()).collect();
    let scope_idx = Select::new()
        .with_prompt("Hook configuration scope")
        .items(&scope_names)
        .default(0)
        .interact()?;
    let hook_scope = scopes[scope_idx];

    // 3. Update-check preference
    let prefs = UpdateCheckPreference::all_variants();
    let pref_names: Vec<&str> = prefs.iter().map(|p| p.display_name()).collect();
    let pref_idx = Select::new()
        .with_prompt("Update check frequency")
        .items(&pref_names)
        .default(0)
        .interact()?;
    let update_check = prefs[pref_idx];

    println!();
    println!(
        "   Selected: {} | {} | {}",
        default_tool.display_name(),
        hook_scope.display_name(),
        update_check.display_name()
    );
    println!();

    Ok(InteractiveConfig {
        default_tool,
        hook_scope,
        update_check,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the user's home directory.
#[cfg(test)]
fn dirs_home() -> Option<PathBuf> {
    // Reuse the existing home_dir helper from the install paths module.
    super::paths::home_dir().ok()
}
