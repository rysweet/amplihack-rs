//! Internal helpers for plugin manager path operations.
//!
//! Extracted from `plugin_manager` to keep modules under the 400-line limit.

use std::path::{Path, PathBuf};

use crate::plugin_manifest::PATH_FIELDS;
use crate::plugin_manager::PluginManagerError;

/// Validate that `path` stays under `base` after resolution (no traversal).
pub fn validate_path_safety(path: &Path, base: &Path) -> bool {
    let resolved_base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());

    let resolved = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            let mut stack = Vec::new();
            for component in path.components() {
                match component {
                    std::path::Component::ParentDir => {
                        stack.pop();
                    }
                    std::path::Component::CurDir => {}
                    std::path::Component::RootDir => {
                        stack.clear();
                        stack.push(std::ffi::OsStr::new("/"));
                    }
                    std::path::Component::Normal(c) => {
                        stack.push(c);
                    }
                    std::path::Component::Prefix(p) => {
                        stack.clear();
                        stack.push(p.as_os_str());
                    }
                }
            }
            stack.iter().collect::<PathBuf>()
        }
    };

    resolved.starts_with(&resolved_base)
}

/// Extract plugin name from a git URL.
///
/// Takes the last path segment and strips a trailing `.git` suffix.
pub(crate) fn extract_plugin_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    name.strip_suffix(".git").unwrap_or(name).to_string()
}

/// Recursively copy a directory tree.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Resolve paths in a JSON map, recursing into nested objects.
pub(crate) fn resolve_paths_inner(
    map: &serde_json::Map<String, serde_json::Value>,
    base: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, PluginManagerError> {
    let mut resolved = serde_json::Map::new();

    for (key, value) in map {
        let is_path_field = PATH_FIELDS.contains(&key.as_str());

        let new_val = if is_path_field {
            match value {
                serde_json::Value::String(s) => {
                    let p = Path::new(s);
                    if p.is_absolute() {
                        value.clone()
                    } else {
                        let abs = base.join(p);
                        if !validate_path_safety(&abs, base) {
                            return Err(PluginManagerError::PathTraversal {
                                path: abs.display().to_string(),
                                base: base.display().to_string(),
                            });
                        }
                        serde_json::Value::String(abs.to_string_lossy().to_string())
                    }
                }
                serde_json::Value::Array(arr) => {
                    let mut items = Vec::new();
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            let p = Path::new(s);
                            if p.is_absolute() {
                                items.push(serde_json::Value::String(s.to_string()));
                            } else {
                                let abs = base.join(p);
                                if !validate_path_safety(&abs, base) {
                                    return Err(PluginManagerError::PathTraversal {
                                        path: abs.display().to_string(),
                                        base: base.display().to_string(),
                                    });
                                }
                                items.push(serde_json::Value::String(
                                    abs.to_string_lossy().to_string(),
                                ));
                            }
                        } else {
                            items.push(item.clone());
                        }
                    }
                    serde_json::Value::Array(items)
                }
                _ => value.clone(),
            }
        } else if let serde_json::Value::Object(nested) = value {
            serde_json::Value::Object(resolve_paths_inner(nested, base)?)
        } else {
            value.clone()
        };

        resolved.insert(key.clone(), new_val);
    }

    Ok(resolved)
}

/// Resolve the user's home directory.
pub(crate) fn home_dir() -> PathBuf {
    #[cfg(unix)]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\"))
    }
}
