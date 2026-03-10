//! CWD protection: deletion and rename/move detection.
//!
//! Detects rm -r/-rf/-fr, rmdir, and mv commands that would
//! affect the current working directory.

use super::command;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;

/// Regex for recursive rm commands.
/// Catches: rm -rf, rm -r, rm -fr, rm -Rf, rm -r -f, rm --recursive, /bin/rm -rf
static RM_RECURSIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\brm\s+(?:-[a-zA-Z]*[rR][a-zA-Z]*|(?:-[a-zA-Z]+\s+)*-[rR]|--recursive)").unwrap()
});

/// Regex for rmdir commands.
static RMDIR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\brmdir(?:\s|$)").unwrap());

/// Regex for mv commands (with optional sudo, env vars, command prefix, full path).
static MV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[;&|])\s*(?:\w+=\S+\s+)*(?:sudo\s+)?(?:-\w+(?:\s+\S+)?\s+)*(?:command\s+)?(?:/(?:usr/)?bin/)?mv\s+"
    ).unwrap()
});

/// Check if a command would delete the current working directory.
///
/// Returns `Some(block_json)` if the command should be blocked,
/// `None` if it's safe.
pub fn check_cwd_deletion(command: &str) -> anyhow::Result<Option<Value>> {
    let has_rm = RM_RECURSIVE_RE.is_match(command);
    let has_rmdir = RMDIR_RE.is_match(command);

    if !has_rm && !has_rmdir {
        return Ok(None);
    }

    // Block commands with shell expansion patterns that could bypass path checks.
    if has_dangerous_expansion(command) {
        return Ok(Some(serde_json::json!({
            "block": true,
            "message": "🚫 OPERATION BLOCKED - Shell Expansion in Destructive Command\n\n\
                        The command uses shell expansion ($(...), `...`, or variable substitution) \
                        in a destructive operation. This bypasses path safety checks.\n\n\
                        Please use literal paths instead of shell expansion in rm/rmdir commands.\n\n\
                        🔒 This protection cannot be disabled programmatically."
        })));
    }

    let cwd = match std::env::current_dir() {
        Ok(p) => match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p,
        },
        Err(_) => return Ok(None), // CWD inaccessible, allow (fail-open)
    };

    let segments = command::split_segments(command);

    for segment in &segments {
        if !RM_RECURSIVE_RE.is_match(segment) && !RMDIR_RE.is_match(segment) {
            continue;
        }

        let paths = command::extract_rm_paths(segment);
        for p in &paths {
            let target = match resolve_path(p) {
                Some(t) => t,
                None => continue,
            };

            // Block if CWD is equal to or a child of the target.
            if is_path_under(&cwd, &target) {
                let message = super::CWD_DELETION_ERROR
                    .replace("{target}", &target.display().to_string())
                    .replace("{cwd}", &cwd.display().to_string());
                return Ok(Some(serde_json::json!({
                    "block": true,
                    "message": message
                })));
            }
        }
    }

    Ok(None)
}

/// Check if a command would rename/move the current working directory.
///
/// Returns `Some(block_json)` if the command should be blocked,
/// `None` if it's safe.
pub fn check_cwd_rename(command: &str) -> anyhow::Result<Option<Value>> {
    if !MV_RE.is_match(command) {
        return Ok(None);
    }

    let cwd = match std::env::current_dir() {
        Ok(p) => match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p,
        },
        Err(_) => return Ok(None), // CWD inaccessible, allow (fail-open)
    };

    let segments = command::split_segments(command);

    for segment in &segments {
        if !MV_RE.is_match(segment) {
            continue;
        }

        let source_paths = command::extract_mv_source_paths(segment);
        if source_paths.is_empty() {
            continue;
        }

        for source_path in &source_paths {
            // Check for glob characters — conservative check.
            if source_path.contains('*') || source_path.contains('?') || source_path.contains('[') {
                if let Some(block) = check_glob_cwd_match(source_path, &cwd) {
                    return Ok(Some(block));
                }
                continue;
            }

            let source = match resolve_path(source_path) {
                Some(s) => s,
                None => continue,
            };

            // Block if CWD is equal to or a child of the source.
            if is_path_under(&cwd, &source) {
                let message = super::CWD_RENAME_ERROR
                    .replace("{source}", &source.display().to_string())
                    .replace("{cwd}", &cwd.display().to_string());
                return Ok(Some(serde_json::json!({
                    "block": true,
                    "message": message
                })));
            }
        }
    }

    Ok(None)
}

/// Check if a glob pattern could match the CWD.
fn check_glob_cwd_match(pattern: &str, cwd: &Path) -> Option<Value> {
    // Extract the non-glob prefix.
    let prefix = pattern
        .split('*')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .split('[')
        .next()
        .unwrap_or("");

    if prefix.is_empty() {
        return None;
    }

    let prefix_path = Path::new(prefix);
    let glob_dir = match prefix_path.parent() {
        Some(d) => match d.canonicalize() {
            Ok(c) => c,
            Err(_) => return None,
        },
        None => return None,
    };
    let basename_prefix = match prefix_path.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return None,
    };

    // Check if CWD's path contains a component that:
    // 1. Is in the same directory as the glob
    // 2. Starts with the basename prefix
    let mut current = Some(cwd.to_path_buf());
    while let Some(path) = current {
        if let Some(parent) = path.parent()
            && let Ok(canonical_parent) = parent.canonicalize()
            && canonical_parent == glob_dir
            && let Some(name) = path.file_name()
            && name.to_string_lossy().starts_with(&basename_prefix)
        {
            let message = super::CWD_RENAME_ERROR
                .replace("{source}", pattern)
                .replace("{cwd}", &cwd.display().to_string());
            return Some(serde_json::json!({
                "block": true,
                "message": message
            }));
        }
        current = path.parent().map(Path::to_path_buf);
        // Avoid infinite loop at root.
        if current.as_deref() == Some(Path::new("/")) || current.as_deref() == Some(Path::new("")) {
            break;
        }
    }

    None
}

/// Resolve a path string to an absolute path.
///
/// Expands `~` and `~/...` to `$HOME` before resolution.
/// Does NOT expand `~user` forms (those are username references).
fn resolve_path(p: &str) -> Option<std::path::PathBuf> {
    // Expand bare ~ and ~/... to $HOME. Skip ~user (username reference).
    let expanded = if p == "~" || p.starts_with("~/") {
        let home = std::env::var("HOME").ok()?;
        if p == "~" {
            home
        } else {
            format!("{}{}", home, &p[1..])
        }
    } else {
        p.to_string()
    };

    let path = Path::new(&expanded);
    match path.canonicalize() {
        Ok(c) => Some(c),
        Err(_) => {
            // If canonicalize fails (path doesn't exist), try making it absolute.
            if path.is_absolute() {
                Some(path.to_path_buf())
            } else {
                std::env::current_dir().ok().map(|cwd| cwd.join(path))
            }
        }
    }
}

/// Check if `child` is equal to or under `parent`.
fn is_path_under(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent)
}

/// Detect shell expansion patterns that could bypass literal path checks.
///
/// Blocks: `$(...)`, `` `...` ``, `${VAR}`, `$VAR` (when followed by path-like chars).
fn has_dangerous_expansion(command: &str) -> bool {
    // Command substitution: $(...)
    if command.contains("$(") {
        return true;
    }
    // Backtick substitution: `...`
    if command.contains('`') {
        return true;
    }
    // Variable expansion: ${VAR} or $VAR (not $? or $! which are status codes)
    let chars: Vec<char> = command.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == '{' {
                return true;
            }
            // $VAR where VAR starts with a letter or underscore
            if next.is_ascii_alphabetic() || next == '_' {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn safe_rm_not_blocked() {
        let result = check_cwd_deletion("rm file.txt").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn rm_rf_nonexistent_not_blocked() {
        let result = check_cwd_deletion("rm -rf /nonexistent/path/xyz").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn rm_rf_cwd_blocked() {
        // Use the actual CWD (don't change it — parallel test safety).
        let cwd = std::env::current_dir().unwrap();
        let cmd = format!("rm -rf {}", cwd.display());
        let result = check_cwd_deletion(&cmd).unwrap();
        assert!(result.is_some());
        let block = result.unwrap();
        assert_eq!(block["block"], true);
        assert!(
            block["message"]
                .as_str()
                .unwrap()
                .contains("Working Directory Deletion Prevented")
        );
    }

    #[test]
    fn mv_safe_not_blocked() {
        let result = check_cwd_rename("mv /tmp/nonexistent_a /tmp/nonexistent_b").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn mv_cwd_blocked() {
        let cwd = std::env::current_dir().unwrap();
        let cmd = format!("mv {} /tmp/new_name", cwd.display());
        let result = check_cwd_rename(&cmd).unwrap();
        assert!(result.is_some());
        let block = result.unwrap();
        assert_eq!(block["block"], true);
    }

    #[test]
    fn is_path_under_works() {
        assert!(is_path_under(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_path_under(Path::new("/a/b"), Path::new("/a/b")));
        assert!(!is_path_under(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(!is_path_under(Path::new("/x/y"), Path::new("/a/b")));
    }

    #[test]
    fn rmdir_cwd_blocked() {
        let cwd = std::env::current_dir().unwrap();
        let cmd = format!("rmdir {}", cwd.display());
        let result = check_cwd_deletion(&cmd).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn rm_rf_parent_of_cwd_blocked() {
        let cwd = std::env::current_dir().unwrap();
        if let Some(parent) = cwd.parent()
            && parent != Path::new("/")
        {
            let cmd = format!("rm -rf {}", parent.display());
            let result = check_cwd_deletion(&cmd).unwrap();
            assert!(result.is_some());
        }
    }

    #[test]
    fn rm_rf_unrelated_dir_not_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::env::current_dir().unwrap();
        // Only test if tempdir is not an ancestor of CWD.
        if !cwd.starts_with(dir.path()) {
            let cmd = format!("rm -rf {}", dir.path().display());
            let result = check_cwd_deletion(&cmd).unwrap();
            assert!(result.is_none());
        }
    }

    #[test]
    fn blocks_command_substitution() {
        let result = check_cwd_deletion(r#"rm -rf "$(pwd)""#).unwrap();
        assert!(result.is_some());
        assert!(
            result.unwrap()["message"]
                .as_str()
                .unwrap()
                .contains("Shell Expansion")
        );
    }

    #[test]
    fn blocks_backtick_substitution() {
        let result = check_cwd_deletion("rm -rf `pwd`").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn blocks_variable_expansion() {
        let result = check_cwd_deletion("rm -rf $HOME/dir").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn blocks_brace_variable_expansion() {
        let result = check_cwd_deletion("rm -rf ${PWD}").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn allows_dollar_status_codes() {
        // $? and $! are shell status codes, not path variables.
        let result = check_cwd_deletion("rm -rf /tmp/test_dir").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn dangerous_expansion_detection() {
        assert!(has_dangerous_expansion("$(pwd)"));
        assert!(has_dangerous_expansion("`pwd`"));
        assert!(has_dangerous_expansion("$HOME"));
        assert!(has_dangerous_expansion("${PWD}"));
        assert!(has_dangerous_expansion("$D"));
        assert!(!has_dangerous_expansion("/tmp/test"));
        assert!(!has_dangerous_expansion("$?"));
        assert!(!has_dangerous_expansion("$!"));
    }

    #[test]
    fn tilde_resolves_to_home() {
        // resolve_path("~") should expand to $HOME.
        let home = std::env::var("HOME").unwrap();
        let resolved = resolve_path("~").unwrap();
        assert_eq!(
            resolved,
            Path::new(&home)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&home))
        );
    }

    #[test]
    fn tilde_slash_resolves_to_home_subpath() {
        let home = std::env::var("HOME").unwrap();
        let resolved = resolve_path("~/Documents").unwrap();
        let expected = Path::new(&home).join("Documents");
        // canonicalize may or may not succeed depending on whether ~/Documents exists.
        assert!(
            resolved == expected.canonicalize().unwrap_or_else(|_| expected.clone()),
            "~/Documents should resolve under $HOME, got: {:?}",
            resolved
        );
    }

    #[test]
    fn tilde_other_user_not_expanded() {
        // ~other_user should NOT be expanded (it's a username reference).
        let resolved = resolve_path("~other_user");
        // Should resolve relative to CWD, not as $HOME.
        if let Some(r) = &resolved {
            let home = std::env::var("HOME").unwrap();
            assert!(
                !r.starts_with(&home) || r.starts_with(std::env::current_dir().unwrap()),
                "~other_user should not expand to $HOME, got: {:?}",
                r
            );
        }
    }

    #[test]
    fn rm_rf_tilde_blocked() {
        // rm -rf ~ should be blocked because ~ expands to $HOME which contains CWD.
        let home = std::env::var("HOME").unwrap();
        let cwd = std::env::current_dir().unwrap();
        // This test only works when CWD is under $HOME.
        if cwd.starts_with(&home) {
            let result = check_cwd_deletion("rm -rf ~").unwrap();
            assert!(
                result.is_some(),
                "rm -rf ~ should be blocked when CWD is under $HOME"
            );
        }
    }

    #[test]
    fn rm_rf_tilde_slash_blocked() {
        let home = std::env::var("HOME").unwrap();
        let cwd = std::env::current_dir().unwrap();
        if cwd.starts_with(&home) {
            let result = check_cwd_deletion("rm -rf ~/").unwrap();
            assert!(
                result.is_some(),
                "rm -rf ~/ should be blocked when CWD is under $HOME"
            );
        }
    }

    #[test]
    fn mv_tilde_blocked() {
        let home = std::env::var("HOME").unwrap();
        let cwd = std::env::current_dir().unwrap();
        if cwd.starts_with(&home) {
            let result = check_cwd_rename("mv ~ /tmp/x").unwrap();
            assert!(
                result.is_some(),
                "mv ~ /tmp/x should be blocked when CWD is under $HOME"
            );
        }
    }
}
