use std::path::Path;

use serde_json::{json, Value};

struct LanguagePattern {
    id: &'static str,
    extensions: &'static [&'static str],
}

const LANGUAGE_PATTERNS: &[LanguagePattern] = &[
    LanguagePattern {
        id: "py",
        extensions: &["*.py"],
    },
    LanguagePattern {
        id: "js",
        extensions: &["*.js", "*.jsx"],
    },
    LanguagePattern {
        id: "ts",
        extensions: &["*.ts", "*.tsx"],
    },
    LanguagePattern {
        id: "rs",
        extensions: &["*.rs"],
    },
    LanguagePattern {
        id: "go",
        extensions: &["*.go"],
    },
];

const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "venv",
    "__pycache__",
    "target",
    ".venv",
    "dist",
    "build",
];

/// Maximum directory depth to recurse when scanning for source files.
const MAX_SCAN_DEPTH: usize = 5;

/// Detects project languages and generates LSP server configurations.
pub struct LSPDetector;

impl LSPDetector {
    /// Walk `project_path` and return the set of detected language IDs.
    pub fn detect_languages(project_path: &Path) -> Vec<String> {
        LANGUAGE_PATTERNS
            .iter()
            .filter(|lp| Self::has_files(project_path, lp.extensions, 0))
            .map(|lp| lp.id.to_string())
            .collect()
    }

    /// Build an `{ "lsp_servers": { … } }` config for the given language
    /// IDs.
    pub fn generate_lsp_config(languages: &[String]) -> Value {
        let mut servers = serde_json::Map::new();
        for lang in languages {
            if let Some(cfg) = Self::lsp_config_for(lang) {
                servers.insert(lang.clone(), cfg);
            }
        }
        json!({ "lsp_servers": servers })
    }

    /// Shallow-merge `lsp_config` into `existing` (top-level keys only).
    pub fn update_settings(existing: &Value, lsp_config: &Value) -> Value {
        let mut merged = existing.clone();
        if let (Some(base), Some(new)) = (merged.as_object_mut(), lsp_config.as_object()) {
            for (k, v) in new {
                base.insert(k.clone(), v.clone());
            }
        }
        merged
    }

    // -- internal --------------------------------------------------------

    fn has_files(dir: &Path, patterns: &[&str], depth: usize) -> bool {
        if depth > MAX_SCAN_DEPTH {
            return false;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return false,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if path.is_dir() {
                if IGNORED_DIRS.contains(&name_str.as_ref()) {
                    continue;
                }
                if Self::has_files(&path, patterns, depth + 1) {
                    return true;
                }
            } else if patterns.iter().any(|p| matches_glob(p, &name_str)) {
                return true;
            }
        }
        false
    }

    fn lsp_config_for(language: &str) -> Option<Value> {
        Some(match language {
            "py" => json!({
                "command": ["pyright-langserver", "--stdio"],
                "languages": ["python"]
            }),
            "js" | "ts" => json!({
                "command": ["typescript-language-server", "--stdio"],
                "languages": ["javascript", "typescript"]
            }),
            "rs" => json!({
                "command": ["rust-analyzer"],
                "languages": ["rust"]
            }),
            "go" => json!({
                "command": ["gopls"],
                "languages": ["go"]
            }),
            _ => return None,
        })
    }
}

/// Trivial glob: only supports `*.ext` patterns.
fn matches_glob(pattern: &str, filename: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(ext) => filename.ends_with(&format!(".{ext}")),
        None => filename == pattern,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn project_with_files(files: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for f in files {
            let path = dir.path().join(f);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, "// stub").unwrap();
        }
        dir
    }

    #[test]
    fn detect_python() {
        let dir = project_with_files(&["app.py"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.contains(&"py".to_string()));
    }

    #[test]
    fn detect_rust() {
        let dir = project_with_files(&["src/main.rs"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.contains(&"rs".to_string()));
    }

    #[test]
    fn detect_javascript() {
        let dir = project_with_files(&["index.js"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.contains(&"js".to_string()));
    }

    #[test]
    fn detect_typescript_tsx() {
        let dir = project_with_files(&["src/App.tsx"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.contains(&"ts".to_string()));
    }

    #[test]
    fn detect_go() {
        let dir = project_with_files(&["main.go"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.contains(&"go".to_string()));
    }

    #[test]
    fn detect_no_languages_in_empty_dir() {
        let dir = TempDir::new().unwrap();
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.is_empty());
    }

    #[test]
    fn detect_multiple_languages() {
        let dir = project_with_files(&["main.py", "lib.rs", "app.go"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(langs.contains(&"py".to_string()));
        assert!(langs.contains(&"rs".to_string()));
        assert!(langs.contains(&"go".to_string()));
    }

    #[test]
    fn ignored_dirs_are_skipped() {
        let dir = project_with_files(&["node_modules/dep/index.js"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(!langs.contains(&"js".to_string()));
    }

    #[test]
    fn ignored_dirs_target() {
        let dir = project_with_files(&["target/debug/build.rs"]);
        let langs = LSPDetector::detect_languages(dir.path());
        assert!(!langs.contains(&"rs".to_string()));
    }

    #[test]
    fn generate_config_python() {
        let config = LSPDetector::generate_lsp_config(&["py".to_string()]);
        let servers = &config["lsp_servers"];
        assert!(servers["py"]["command"].is_array());
    }

    #[test]
    fn generate_config_empty() {
        let config = LSPDetector::generate_lsp_config(&[]);
        let servers = config["lsp_servers"].as_object().unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn generate_config_unknown_language_skipped() {
        let config = LSPDetector::generate_lsp_config(&["brainfuck".to_string()]);
        let servers = config["lsp_servers"].as_object().unwrap();
        assert!(!servers.contains_key("brainfuck"));
    }

    #[test]
    fn update_settings_merges() {
        let existing = json!({ "editor": "vim", "lsp_servers": {} });
        let lsp = json!({ "lsp_servers": { "py": { "command": ["pyright"] } } });
        let merged = LSPDetector::update_settings(&existing, &lsp);
        assert_eq!(merged["editor"], "vim");
        assert!(merged["lsp_servers"]["py"].is_object());
    }

    #[test]
    fn update_settings_overwrites_key() {
        let existing = json!({ "lsp_servers": { "py": "old" } });
        let lsp = json!({ "lsp_servers": { "py": "new" } });
        let merged = LSPDetector::update_settings(&existing, &lsp);
        assert_eq!(merged["lsp_servers"]["py"], "new");
    }

    #[test]
    fn matches_glob_extension() {
        assert!(matches_glob("*.rs", "main.rs"));
        assert!(!matches_glob("*.rs", "main.py"));
    }

    #[test]
    fn matches_glob_exact() {
        assert!(matches_glob("Makefile", "Makefile"));
        assert!(!matches_glob("Makefile", "makefile"));
    }
}
