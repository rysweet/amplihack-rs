use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Known JSON keys that hold filesystem paths.
const PATH_FIELDS: &[&str] = &[
    "path",
    "cwd",
    "script",
    "entry_point",
    "file",
    "files",
    "absolute",
    "relative",
];

/// Resolves `~`-prefixed and relative paths against a plugin root.
pub struct PathResolver;

impl PathResolver {
    /// Expand `~`, then resolve relative paths against `plugin_root`.
    /// Absolute paths are returned unchanged.
    pub fn resolve(path_str: &str, plugin_root: &Path) -> String {
        if path_str.is_empty() {
            return path_str.to_string();
        }

        let expanded = if path_str.starts_with("~/") {
            match std::env::var("HOME") {
                Ok(home) => format!("{home}{}", &path_str[1..]),
                Err(_) => path_str.to_string(),
            }
        } else if path_str == "~" {
            std::env::var("HOME").unwrap_or_else(|_| path_str.to_string())
        } else {
            path_str.to_string()
        };

        if Path::new(&expanded).is_absolute() {
            return expanded;
        }

        plugin_root.join(&expanded).to_string_lossy().into_owned()
    }

    /// Recursively walk a JSON value and resolve every string in a
    /// recognised path field.
    pub fn resolve_map(data: &Value, plugin_root: &Path) -> Value {
        let fields: HashSet<&str> = PATH_FIELDS.iter().copied().collect();
        Self::walk(data, plugin_root, &fields)
    }

    /// Return `PLUGIN_ROOT` env var or fall back to `~/.amplihack/.claude`.
    pub fn get_plugin_root() -> PathBuf {
        if let Ok(root) = std::env::var("PLUGIN_ROOT") {
            return PathBuf::from(root);
        }
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".amplihack/.claude"))
            .unwrap_or_else(|_| PathBuf::from(".amplihack/.claude"))
    }

    fn walk(data: &Value, root: &Path, fields: &HashSet<&str>) -> Value {
        match data {
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (key, val) in map {
                    let resolved = if fields.contains(key.as_str()) {
                        Self::resolve_field(val, root, fields)
                    } else {
                        Self::walk(val, root, fields)
                    };
                    out.insert(key.clone(), resolved);
                }
                Value::Object(out)
            }
            Value::Array(arr) => {
                Value::Array(arr.iter().map(|v| Self::walk(v, root, fields)).collect())
            }
            other => other.clone(),
        }
    }

    fn resolve_field(val: &Value, root: &Path, fields: &HashSet<&str>) -> Value {
        match val {
            Value::String(s) => Value::String(Self::resolve(s, root)),
            Value::Array(arr) => Value::Array(
                arr.iter()
                    .map(|v| match v {
                        Value::String(s) => Value::String(Self::resolve(s, root)),
                        other => Self::walk(other, root, fields),
                    })
                    .collect(),
            ),
            other => Self::walk(other, root, fields),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn resolve_absolute_unchanged() {
        let result = PathResolver::resolve("/usr/bin/foo", Path::new("/root"));
        assert_eq!(result, "/usr/bin/foo");
    }

    #[test]
    fn resolve_tilde() {
        let home = std::env::var("HOME").unwrap();
        let result = PathResolver::resolve("~/docs/x.txt", Path::new("/root"));
        assert_eq!(result, format!("{home}/docs/x.txt"));
    }

    #[test]
    fn resolve_bare_tilde() {
        let home = std::env::var("HOME").unwrap();
        let result = PathResolver::resolve("~", Path::new("/root"));
        assert_eq!(result, home);
    }

    #[test]
    fn resolve_relative_to_plugin_root() {
        let result = PathResolver::resolve("src/main.rs", Path::new("/project"));
        assert_eq!(result, "/project/src/main.rs");
    }

    #[test]
    fn resolve_empty_string() {
        let result = PathResolver::resolve("", Path::new("/root"));
        assert_eq!(result, "");
    }

    #[test]
    fn resolve_map_resolves_path_fields() {
        let data = json!({
            "path": "src/lib.rs",
            "name": "mylib",
            "cwd": "~/work"
        });
        let home = std::env::var("HOME").unwrap();
        let resolved = PathResolver::resolve_map(&data, Path::new("/project"));
        assert_eq!(resolved["path"], "/project/src/lib.rs");
        assert_eq!(resolved["name"], "mylib"); // not a path field
        assert_eq!(resolved["cwd"], format!("{home}/work"));
    }

    #[test]
    fn resolve_map_handles_nested_objects() {
        let data = json!({
            "outer": {
                "file": "readme.md"
            }
        });
        let resolved = PathResolver::resolve_map(&data, Path::new("/root"));
        assert_eq!(resolved["outer"]["file"], "/root/readme.md");
    }

    #[test]
    fn resolve_map_handles_array_of_paths() {
        let data = json!({ "files": ["a.rs", "b.rs"] });
        let resolved = PathResolver::resolve_map(&data, Path::new("/root"));
        assert_eq!(resolved["files"][0], "/root/a.rs");
        assert_eq!(resolved["files"][1], "/root/b.rs");
    }

    #[test]
    fn get_plugin_root_uses_env() {
        unsafe { std::env::set_var("PLUGIN_ROOT", "/custom/root") };
        let root = PathResolver::get_plugin_root();
        unsafe { std::env::remove_var("PLUGIN_ROOT") };
        assert_eq!(root, PathBuf::from("/custom/root"));
    }

    #[test]
    fn get_plugin_root_fallback() {
        unsafe { std::env::remove_var("PLUGIN_ROOT") };
        let root = PathResolver::get_plugin_root();
        // Should contain .amplihack/.claude
        assert!(root.to_string_lossy().contains(".amplihack/.claude"));
    }
}
