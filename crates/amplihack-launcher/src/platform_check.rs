//! Platform compatibility checking for amplihack.
//!
//! Detects the host operating system (macOS, Linux, Windows, WSL) and
//! reports compatibility status with actionable guidance.

use std::path::Path;

/// Result of a platform compatibility check.
#[derive(Debug, Clone)]
pub struct PlatformCheckResult {
    /// `true` if the platform is supported.
    pub compatible: bool,
    /// Human-readable platform name (e.g. `"macOS"`, `"Linux (WSL)"`).
    pub platform_name: String,
    /// `true` if running inside WSL.
    pub is_wsl: bool,
    /// Guidance message; empty when fully compatible.
    pub message: String,
}

/// Returns `true` when running on native Windows (not WSL).
pub fn is_native_windows() -> bool {
    cfg!(target_os = "windows")
}

/// Check current platform compatibility with amplihack.
///
/// All platforms are currently reported as compatible, but native Windows
/// receives a warning about partial feature support.
pub fn check_platform_compatibility() -> PlatformCheckResult {
    let is_wsl = detect_wsl();

    if is_native_windows() {
        return PlatformCheckResult {
            compatible: true,
            platform_name: "Windows (native, partial)".into(),
            is_wsl: false,
            message: concat!(
                "⚠️  Native Windows detected — running with partial support.\n",
                "   Unavailable features: fleet (requires tmux/SSH).\n",
                "   On ARM64 Windows, use x86_64 Python for memory features (LadybugDB).\n",
                "   For full support, use WSL: ",
                "https://learn.microsoft.com/en-us/windows/wsl/install",
            )
            .into(),
        };
    }

    let platform_name = if cfg!(target_os = "macos") {
        "macOS".to_string()
    } else if is_wsl {
        "Linux (WSL)".to_string()
    } else {
        "Linux".to_string()
    };

    PlatformCheckResult {
        compatible: true,
        platform_name,
        is_wsl,
        message: String::new(),
    }
}

/// Detect WSL by reading `/proc/version`.
fn detect_wsl() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let proc_version = Path::new("/proc/version");
    if !proc_version.exists() {
        return false;
    }
    match std::fs::read_to_string(proc_version) {
        Ok(content) => {
            let lower = content.to_lowercase();
            lower.contains("microsoft") || lower.contains("wsl")
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_native_windows_on_current_platform() {
        // On CI / Linux this should be false
        if cfg!(target_os = "windows") {
            assert!(is_native_windows());
        } else {
            assert!(!is_native_windows());
        }
    }

    #[test]
    fn check_platform_returns_compatible() {
        let result = check_platform_compatibility();
        assert!(result.compatible);
        assert!(!result.platform_name.is_empty());
    }

    #[test]
    fn check_platform_linux_name() {
        if cfg!(target_os = "linux") {
            let result = check_platform_compatibility();
            assert!(
                result.platform_name.starts_with("Linux"),
                "expected Linux*, got {}",
                result.platform_name
            );
        }
    }

    #[test]
    fn check_platform_macos_name() {
        if cfg!(target_os = "macos") {
            let result = check_platform_compatibility();
            assert_eq!(result.platform_name, "macOS");
        }
    }

    #[test]
    fn detect_wsl_returns_bool() {
        let _w = detect_wsl();
        // Just ensure it doesn't panic
    }
}
