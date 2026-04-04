//! UVX environment manager for `--add-dir` integration with Claude Code.
//!
//! Ports Python `amplihack/uvx/manager.py`:
//! - Detects UVX environments via env vars and filesystem probes
//! - Resolves framework paths for `--add-dir` arguments
//! - Validates path security (no traversal, no system dirs)
//! - Enhances Claude commands with `--add-dir` when appropriate

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// UVX detection configuration.
#[derive(Debug, Clone)]
pub struct UvxConfig {
    /// Known environment variable markers for UVX deployment.
    pub env_markers: Vec<String>,
    /// Directories considered sensitive (reject path validation).
    pub sensitive_prefixes: Vec<String>,
}

impl Default for UvxConfig {
    fn default() -> Self {
        Self {
            env_markers: vec![
                "UV_PYTHON".to_string(),
                "AMPLIHACK_ROOT".to_string(),
                "AMPLIHACK_IN_UVX".to_string(),
            ],
            sensitive_prefixes: vec![
                "/etc".to_string(),
                "/private/etc".to_string(),
                "/root".to_string(),
                "/private/root".to_string(),
                "/sys".to_string(),
                "/proc".to_string(),
                "/dev".to_string(),
                "/boot".to_string(),
                "/usr/bin".to_string(),
                "/usr/sbin".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
                "/var/root".to_string(),
                "/System/Library".to_string(),
                "/Library/Security".to_string(),
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Detection result
// ---------------------------------------------------------------------------

/// Result of UVX environment detection.
#[derive(Debug, Clone)]
pub struct UvxDetectionState {
    /// Whether a UVX deployment was detected.
    pub is_uvx_deployment: bool,
    /// Reasons why detection succeeded or failed.
    pub detection_reasons: Vec<String>,
}

/// Result of framework path resolution.
#[derive(Debug, Clone)]
pub struct PathResolutionResult {
    /// Resolved framework root path.
    pub framework_path: Option<PathBuf>,
    /// Whether staging is required instead of `--add-dir`.
    pub requires_staging: bool,
    /// Resolution attempts with strategy and notes.
    pub attempts: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// UVX Manager
// ---------------------------------------------------------------------------

/// Manages UVX environment detection and Claude command enhancement.
///
/// Caches detection and path resolution results for efficiency.
pub struct UvxManager {
    force_staging: bool,
    config: UvxConfig,
    detection: Option<UvxDetectionState>,
    path_resolution: Option<PathResolutionResult>,
}

impl UvxManager {
    /// Create a new UVX manager.
    pub fn new(force_staging: bool) -> Self {
        Self {
            force_staging,
            config: UvxConfig::default(),
            detection: None,
            path_resolution: None,
        }
    }

    /// Create with default settings (no forced staging).
    pub fn with_defaults() -> Self {
        Self::new(false)
    }

    /// Create with a custom config.
    pub fn with_config(force_staging: bool, config: UvxConfig) -> Self {
        Self {
            force_staging,
            config,
            detection: None,
            path_resolution: None,
        }
    }

    /// Detect if we're running in a UVX environment.
    pub fn is_uvx_environment(&mut self) -> bool {
        self.ensure_detection();
        self.detection
            .as_ref()
            .map(|d| d.is_uvx_deployment)
            .unwrap_or(false)
    }

    /// Get the framework root path.
    pub fn get_framework_path(&mut self) -> Option<PathBuf> {
        self.ensure_path_resolution();
        self.path_resolution
            .as_ref()
            .and_then(|r| r.framework_path.clone())
    }

    /// Determine if `--add-dir` should be used.
    pub fn should_use_add_dir(&mut self) -> bool {
        if self.force_staging {
            debug!("UVX --add-dir disabled: force_staging=true");
            return false;
        }

        if !self.is_uvx_environment() {
            debug!("UVX --add-dir disabled: not in UVX environment");
            return false;
        }

        let framework_path = match self.get_framework_path() {
            Some(p) => p,
            None => {
                debug!("UVX --add-dir disabled: framework path not found");
                return false;
            }
        };

        if !self.validate_path_security(&framework_path) {
            warn!(
                "UVX --add-dir disabled: path failed security validation: {}",
                framework_path.display()
            );
            return false;
        }

        debug!(
            "UVX --add-dir enabled: framework_path='{}'",
            framework_path.display()
        );
        true
    }

    /// Determine if staging approach should be used.
    pub fn should_use_staging(&mut self) -> bool {
        if self.force_staging {
            return true;
        }

        self.ensure_path_resolution();
        if let Some(ref pr) = self.path_resolution
            && pr.requires_staging
        {
            return true;
        }

        if self.is_uvx_environment() && !self.should_use_add_dir() {
            return true;
        }

        false
    }

    /// Get `--add-dir` arguments for Claude command.
    pub fn get_add_dir_args(&mut self) -> Vec<String> {
        if !self.should_use_add_dir() {
            return vec![];
        }

        match self.get_framework_path() {
            Some(path) => vec!["--add-dir".to_string(), path.display().to_string()],
            None => vec![],
        }
    }

    /// Validate that a path is safe (no directory traversal, no system dirs).
    pub fn validate_path_security(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // Check for traversal patterns
        if path_str.contains("/../") || path_str.starts_with("../") || path_str.ends_with("/..") {
            warn!("Path contains directory traversal pattern: {}", path_str);
            return false;
        }

        // Check for null bytes
        if path_str.contains('\0') {
            warn!("Path contains null bytes: {}", path_str);
            return false;
        }

        // Resolve to absolute for checking (canonicalize resolves symlinks)
        let abs_path = match path.canonicalize() {
            Ok(p) => p,
            Err(_) if path.is_absolute() => path.to_path_buf(),
            Err(e) => {
                warn!("Path validation error: {}", e);
                return false;
            }
        };

        let check_str = abs_path.to_string_lossy();
        // Strip macOS data volume prefix
        let check_path = check_str
            .strip_prefix("/System/Volumes/Data")
            .unwrap_or(&check_str);

        for prefix in &self.config.sensitive_prefixes {
            if check_path == prefix.as_str() || check_path.starts_with(&format!("{prefix}/")) {
                warn!("Path targets system directory: {}", abs_path.display());
                return false;
            }
        }

        debug!("Path validation passed: {}", path_str);
        true
    }

    /// Enhance a Claude command with `--add-dir` if appropriate.
    pub fn enhance_claude_command(&mut self, base_command: Vec<String>) -> Vec<String> {
        let add_dir_args = self.get_add_dir_args();
        if add_dir_args.is_empty() {
            return base_command;
        }

        let mut enhanced = base_command;
        enhanced.extend(add_dir_args);
        enhanced
    }

    /// Get environment variables to set for UVX mode.
    pub fn get_environment_variables(&mut self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        if let Some(path) = self.get_framework_path() {
            vars.insert(
                "AMPLIHACK_PROJECT_ROOT".to_string(),
                path.display().to_string(),
            );
        }
        vars
    }

    /// Get the current detection state.
    pub fn get_detection_state(&mut self) -> &UvxDetectionState {
        self.ensure_detection();
        self.detection.as_ref().unwrap()
    }

    // -- Internal --

    fn ensure_detection(&mut self) {
        if self.detection.is_some() {
            return;
        }
        self.detection = Some(detect_uvx_deployment(&self.config));
    }

    fn ensure_path_resolution(&mut self) {
        self.ensure_detection();
        if self.path_resolution.is_some() {
            return;
        }
        self.path_resolution = Some(resolve_framework_paths(&self.config));
    }
}

// ---------------------------------------------------------------------------
// Detection logic
// ---------------------------------------------------------------------------

/// Detect whether we're in a UVX deployment.
fn detect_uvx_deployment(config: &UvxConfig) -> UvxDetectionState {
    let mut reasons = Vec::new();
    let mut is_uvx = false;

    // Check environment markers
    for marker in &config.env_markers {
        if env::var_os(marker).is_some() {
            reasons.push(format!("env {marker} is set"));
            is_uvx = true;
        }
    }

    // Check if cwd lacks .claude/ (indicator of UVX rather than local install)
    if let Ok(cwd) = env::current_dir()
        && !cwd.join(".claude").exists()
        && is_uvx
    {
        reasons.push("cwd lacks .claude/ directory".to_string());
    }

    if !is_uvx {
        reasons.push("no UVX environment markers found".to_string());
    }

    UvxDetectionState {
        is_uvx_deployment: is_uvx,
        detection_reasons: reasons,
    }
}

/// Resolve framework paths, returning the best candidate.
fn resolve_framework_paths(config: &UvxConfig) -> PathResolutionResult {
    let mut attempts = Vec::new();
    let _ = config; // reserved for future strategy-based resolution

    // Strategy 1: AMPLIHACK_ROOT env var
    if let Some(root) = env::var_os("AMPLIHACK_ROOT").map(PathBuf::from) {
        if is_framework_root(&root) {
            attempts.push((
                "env:AMPLIHACK_ROOT".to_string(),
                format!("found: {}", root.display()),
            ));
            return PathResolutionResult {
                framework_path: Some(root),
                requires_staging: false,
                attempts,
            };
        }
        attempts.push((
            "env:AMPLIHACK_ROOT".to_string(),
            format!("set but not a framework root: {}", root.display()),
        ));
    }

    // Strategy 2: Current working directory
    if let Ok(cwd) = env::current_dir() {
        if is_framework_root(&cwd) {
            attempts.push(("cwd".to_string(), format!("found: {}", cwd.display())));
            return PathResolutionResult {
                framework_path: Some(cwd),
                requires_staging: false,
                attempts,
            };
        }
        attempts.push(("cwd".to_string(), "not a framework root".to_string()));
    }

    // Strategy 3: ~/.amplihack
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        let staged = home.join(".amplihack");
        if is_framework_root(&staged) {
            attempts.push((
                "home:~/.amplihack".to_string(),
                format!("found: {}", staged.display()),
            ));
            return PathResolutionResult {
                framework_path: Some(staged),
                requires_staging: false,
                attempts,
            };
        }
        attempts.push((
            "home:~/.amplihack".to_string(),
            "not a framework root".to_string(),
        ));
    }

    // No resolution — staging required
    attempts.push((
        "fallback".to_string(),
        "no framework root found".to_string(),
    ));
    PathResolutionResult {
        framework_path: None,
        requires_staging: true,
        attempts,
    }
}

/// Check if a directory looks like a framework root.
fn is_framework_root(path: &Path) -> bool {
    path.join(".claude").is_dir()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_has_env_markers() {
        let config = UvxConfig::default();
        assert!(config.env_markers.contains(&"UV_PYTHON".to_string()));
        assert!(config.env_markers.contains(&"AMPLIHACK_ROOT".to_string()));
    }

    #[test]
    fn default_config_has_sensitive_prefixes() {
        let config = UvxConfig::default();
        assert!(config.sensitive_prefixes.contains(&"/etc".to_string()));
        assert!(config.sensitive_prefixes.contains(&"/proc".to_string()));
    }

    #[test]
    fn manager_with_defaults() {
        let mgr = UvxManager::with_defaults();
        assert!(!mgr.force_staging);
    }

    #[test]
    fn manager_force_staging_disables_add_dir() {
        let mut mgr = UvxManager::new(true);
        assert!(!mgr.should_use_add_dir());
        assert!(mgr.should_use_staging());
    }

    #[test]
    fn validate_path_rejects_traversal() {
        let mgr = UvxManager::with_defaults();
        assert!(!mgr.validate_path_security(Path::new("../../../etc/passwd")));
        assert!(!mgr.validate_path_security(Path::new("/home/user/../../../etc")));
        assert!(!mgr.validate_path_security(Path::new("/safe/dir/..")));
    }

    #[test]
    fn validate_path_rejects_system_dirs() {
        let mgr = UvxManager::with_defaults();
        // Use non-existent paths that won't canonicalize away from their prefix
        assert!(!mgr.validate_path_security(Path::new("/etc/shadow")));
        assert!(!mgr.validate_path_security(Path::new("/proc/nonexistent")));
        assert!(!mgr.validate_path_security(Path::new("/boot/nonexistent")));
        assert!(!mgr.validate_path_security(Path::new("/root/.bashrc")));
    }

    #[test]
    fn validate_path_accepts_safe_paths() {
        let dir = TempDir::new().unwrap();
        let mgr = UvxManager::with_defaults();
        assert!(mgr.validate_path_security(dir.path()));
    }

    #[test]
    fn validate_path_rejects_null_bytes() {
        let mgr = UvxManager::with_defaults();
        assert!(!mgr.validate_path_security(Path::new("/tmp/test\0inject")));
    }

    #[test]
    fn get_add_dir_args_empty_when_not_uvx() {
        let mut mgr = UvxManager::with_defaults();
        assert!(mgr.get_add_dir_args().is_empty());
    }

    #[test]
    fn enhance_command_noop_when_no_args() {
        let mut mgr = UvxManager::with_defaults();
        let cmd = vec!["claude".to_string()];
        let enhanced = mgr.enhance_claude_command(cmd.clone());
        assert_eq!(enhanced, cmd);
    }

    #[test]
    fn get_environment_variables_returns_map() {
        let mut mgr = UvxManager::with_defaults();
        let vars = mgr.get_environment_variables();
        // Returns a HashMap (may include host-inherited vars)
        // Framework-specific vars require valid path resolution
        assert!(vars.len() < 20, "unexpectedly large env map");
    }

    #[test]
    fn detection_state_populated_after_call() {
        let mut mgr = UvxManager::with_defaults();
        let state = mgr.get_detection_state();
        assert!(!state.detection_reasons.is_empty());
    }

    #[test]
    fn is_framework_root_checks_claude_dir() {
        let dir = TempDir::new().unwrap();
        assert!(!is_framework_root(dir.path()));

        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        assert!(is_framework_root(dir.path()));
    }

    #[test]
    fn resolution_attempts_are_recorded() {
        let config = UvxConfig::default();
        let result = resolve_framework_paths(&config);
        assert!(!result.attempts.is_empty());
    }

    #[test]
    fn detection_no_markers_returns_false() {
        let config = UvxConfig {
            env_markers: vec!["NONEXISTENT_AMPLIHACK_TEST_VAR_12345".to_string()],
            ..Default::default()
        };
        let state = detect_uvx_deployment(&config);
        assert!(!state.is_uvx_deployment);
    }

    #[test]
    fn validate_path_strips_macos_prefix() {
        let mgr = UvxManager::with_defaults();
        // /System/Volumes/Data/etc should still be rejected
        assert!(!mgr.validate_path_security(Path::new("/System/Volumes/Data/etc/passwd")));
    }
}
