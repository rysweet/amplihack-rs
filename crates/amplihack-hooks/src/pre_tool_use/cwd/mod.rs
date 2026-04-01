//! CWD protection: deletion and rename/move detection.
//!
//! Detects rm -r/-rf/-fr, rmdir, and mv commands that would
//! affect the current working directory.

#[cfg(test)]
mod tests;

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
pub fn check_cwd_deletion(command: &str) -> anyhow::Result<Option<Value>> {
    let has_rm = RM_RECURSIVE_RE.is_match(command);
    let has_rmdir = RMDIR_RE.is_match(command);

    if !has_rm && !has_rmdir {
        return Ok(None);
    }

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
        Err(_) => return Ok(None),
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
pub fn check_cwd_rename(command: &str) -> anyhow::Result<Option<Value>> {
    if !MV_RE.is_match(command) {
        return Ok(None);
    }

    let cwd = match std::env::current_dir() {
        Ok(p) => match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p,
        },
        Err(_) => return Ok(None),
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

fn check_glob_cwd_match(pattern: &str, cwd: &Path) -> Option<Value> {
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
        if current.as_deref() == Some(Path::new("/")) || current.as_deref() == Some(Path::new("")) {
            break;
        }
    }

    None
}

fn resolve_path(p: &str) -> Option<std::path::PathBuf> {
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
            if path.is_absolute() {
                Some(path.to_path_buf())
            } else {
                std::env::current_dir().ok().map(|cwd| cwd.join(path))
            }
        }
    }
}

fn is_path_under(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent)
}

fn has_dangerous_expansion(command: &str) -> bool {
    if command.contains("$(") {
        return true;
    }
    if command.contains('`') {
        return true;
    }
    let chars: Vec<char> = command.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == '{' {
                return true;
            }
            if next.is_ascii_alphabetic() || next == '_' {
                return true;
            }
        }
    }
    false
}
