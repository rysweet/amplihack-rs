//! CWD protection: deletion and rename/move detection.
//!
//! Detects rm -r/-rf/-fr, rmdir, and mv commands that would
//! affect the current working directory.
//!
//! A removal is decided on the words of its own simple command. The command
//! line is split with the shared tokeniser (see [`removals`]), so heredoc
//! bodies, quoted strings and the words of other commands do not affect the
//! decision. When the line cannot be read with certainty, the whole text is
//! checked as before.

mod removals;
#[cfg(test)]
mod tests;

use super::command;
use super::shell;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;

/// Regex for recursive rm commands.
/// Catches: rm -rf, rm -r, rm -fr, rm -Rf, rm -r -f, rm --recursive, /bin/rm -rf
static RM_RECURSIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\brm\s+(?:-[a-zA-Z]*[rR][a-zA-Z]*|(?:-[a-zA-Z]+\s+)*-[rR]|--recursive)")
        .expect("static RM_RECURSIVE regex")
});

/// Regex for rmdir commands.
static RMDIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\brmdir(?:\s|$)").expect("static RMDIR regex"));

/// A word that may be `rm` or `rmdir`. Text without one is not tokenised.
static RM_WORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\brm(?:dir)?\b").expect("static RM_WORD regex"));

/// Regex for mv commands (with optional sudo, env vars, command prefix, full path).
static MV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[;&|])\s*(?:\w+=\S+\s+)*(?:sudo\s+)?(?:-\w+(?:\s+\S+)?\s+)*(?:command\s+)?(?:/(?:usr/)?bin/)?mv\s+"
    ).expect("static MV regex")
});

const EXPANSION_ERROR: &str = "🚫 OPERATION BLOCKED - Shell Expansion in Destructive Command\n\n\
     The command uses shell expansion ($(...), `...`, or variable substitution) \
     in a destructive operation. This bypasses path safety checks.\n\n\
     Instead, write the path out literally rather than using a variable, \
     command substitution or glob.\n\n\
     🔒 This protection cannot be disabled programmatically.";

/// Check if a command would delete the current working directory.
///
/// Each recursive `rm` and each `rmdir` the shell would run is decided on
/// the words of its own simple command: it is rejected when one of them uses
/// shell expansion, or when one of its operands resolves to the working
/// directory or a directory above it.
pub fn check_cwd_deletion(command: &str) -> anyhow::Result<Option<Value>> {
    if !RM_WORD_RE.is_match(command) {
        return Ok(None);
    }

    let Some(found) = removals::removal_commands(command) else {
        return whole_text_decision(command);
    };

    if found
        .iter()
        .any(|r| r.words.iter().any(|w| has_dangerous_expansion(w)))
    {
        return Ok(Some(super::deny(EXPANSION_ERROR)));
    }

    let Some(cwd) = current_cwd() else {
        return Ok(None);
    };

    for removal in &found {
        let argv: Vec<&str> = removal.argv.iter().map(String::as_str).collect();
        let paths = command::extract_rm_paths(&shell::join_argv(&argv));
        if let Some(block) = deny_under_cwd(&paths, &cwd) {
            return Ok(Some(block));
        }
    }

    Ok(None)
}

/// The decision on the whole text, used when the command line cannot be
/// tokenised with certainty: an unclosed quote or substitution, an
/// unterminated heredoc, input over the length limit, or nesting past the
/// depth or work limit. Any expansion anywhere in the text is rejected.
fn whole_text_decision(command: &str) -> anyhow::Result<Option<Value>> {
    if !RM_RECURSIVE_RE.is_match(command) && !RMDIR_RE.is_match(command) {
        return Ok(None);
    }

    if has_dangerous_expansion(command) {
        return Ok(Some(super::deny(EXPANSION_ERROR)));
    }

    let Some(cwd) = current_cwd() else {
        return Ok(None);
    };

    for segment in &command::split_segments(command) {
        if !RM_RECURSIVE_RE.is_match(segment) && !RMDIR_RE.is_match(segment) {
            continue;
        }
        if let Some(block) = deny_under_cwd(&command::extract_rm_paths(segment), &cwd) {
            return Ok(Some(block));
        }
    }

    Ok(None)
}

fn current_cwd() -> Option<std::path::PathBuf> {
    let p = std::env::current_dir().ok()?;
    Some(p.canonicalize().unwrap_or(p))
}

/// The deny response when one of `paths` resolves to `cwd` or a directory
/// above it.
fn deny_under_cwd(paths: &[String], cwd: &Path) -> Option<Value> {
    for p in paths {
        let Some(target) = resolve_path(p) else {
            continue;
        };
        if is_path_under(cwd, &target) {
            let message = super::CWD_DELETION_ERROR
                .replace("{target}", &target.display().to_string())
                .replace("{cwd}", &cwd.display().to_string());
            return Some(super::deny(&message));
        }
    }
    None
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
                return Ok(Some(super::deny(&message)));
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
    let glob_dir = {
        let d = prefix_path.parent()?;
        match d.canonicalize() {
            Ok(c) => c,
            Err(_) => return None,
        }
    };
    let basename_prefix = {
        let n = prefix_path.file_name()?;
        n.to_string_lossy().to_string()
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
            return Some(super::deny(&message));
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
