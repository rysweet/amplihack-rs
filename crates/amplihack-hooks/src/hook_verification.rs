//! Hook file verification.
//!
//! Checks that the expected amplihack hook files exist under
//! `~/.amplihack/.claude/tools/{amplihack,xpia}/hooks/`.

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Hook files that must exist for a complete installation.
const REQUIRED_HOOKS: &[(&str, &str)] = &[
    ("amplihack", "PreToolUse.js"),
    ("amplihack", "PostToolUse.js"),
    ("amplihack", "Stop.js"),
    ("amplihack", "SessionStart.js"),
    ("amplihack", "SessionStop.js"),
    ("amplihack", "UserPromptSubmit.js"),
    ("amplihack", "PreCompact.js"),
    ("xpia", "PreToolUse.js"),
];

/// Resolve the amplihack home directory.
fn amplihack_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| Path::new(&h).join(".amplihack"))
}

/// Verify that all required hook files exist.
///
/// Returns `true` if every expected hook file is present under
/// `~/.amplihack/.claude/tools/{namespace}/hooks/`.
pub fn verify_hooks() -> bool {
    let home = match amplihack_home() {
        Some(h) => h,
        None => {
            warn!("HOME not set — cannot verify hooks");
            return false;
        }
    };

    let hooks_base = home.join(".claude").join("tools");
    let mut all_present = true;

    for (namespace, filename) in REQUIRED_HOOKS {
        let path = hooks_base.join(namespace).join("hooks").join(filename);
        if path.exists() {
            debug!("hook present: {}", path.display());
        } else {
            warn!("missing hook: {}", path.display());
            all_present = false;
        }
    }

    all_present
}

/// Verify hooks under a custom root (for testing).
pub fn verify_hooks_at(root: &Path) -> bool {
    let hooks_base = root.join(".claude").join("tools");
    let mut all_present = true;

    for (namespace, filename) in REQUIRED_HOOKS {
        let path = hooks_base.join(namespace).join("hooks").join(filename);
        if path.exists() {
            debug!("hook present: {}", path.display());
        } else {
            all_present = false;
        }
    }

    all_present
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_all_hooks(root: &Path) {
        for (namespace, filename) in REQUIRED_HOOKS {
            let dir = root
                .join(".claude")
                .join("tools")
                .join(namespace)
                .join("hooks");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(filename), "// hook").unwrap();
        }
    }

    #[test]
    fn all_hooks_present() {
        let tmp = tempfile::tempdir().unwrap();
        create_all_hooks(tmp.path());
        assert!(verify_hooks_at(tmp.path()));
    }

    #[test]
    fn missing_hook_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        create_all_hooks(tmp.path());
        // Remove one hook.
        let path = tmp
            .path()
            .join(".claude")
            .join("tools")
            .join("amplihack")
            .join("hooks")
            .join("Stop.js");
        fs::remove_file(path).unwrap();
        assert!(!verify_hooks_at(tmp.path()));
    }

    #[test]
    fn empty_dir_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!verify_hooks_at(tmp.path()));
    }

    #[test]
    fn required_hooks_not_empty() {
        assert!(!REQUIRED_HOOKS.is_empty());
    }

    #[test]
    fn all_hooks_have_js_extension() {
        for (_, filename) in REQUIRED_HOOKS {
            assert!(filename.ends_with(".js"), "{filename} should end with .js");
        }
    }
}
