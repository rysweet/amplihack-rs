use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::MultilspyError;

/// Supported programming languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    CSharp,
    Python,
    Rust,
    Java,
    TypeScript,
    JavaScript,
    Go,
    Ruby,
    Dart,
    Php,
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.language_id())
    }
}

impl FromStr for Language {
    type Err = MultilspyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "csharp" | "c#" | "cs" => Ok(Self::CSharp),
            "python" | "py" => Ok(Self::Python),
            "rust" | "rs" => Ok(Self::Rust),
            "java" => Ok(Self::Java),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "javascript" | "js" => Ok(Self::JavaScript),
            "go" | "golang" => Ok(Self::Go),
            "ruby" | "rb" => Ok(Self::Ruby),
            "dart" => Ok(Self::Dart),
            "php" => Ok(Self::Php),
            _ => Err(MultilspyError::UnsupportedLanguage(s.to_string())),
        }
    }
}

impl Language {
    /// Returns the LSP `languageId` string for this language.
    pub fn language_id(&self) -> &'static str {
        match self {
            Self::CSharp => "csharp",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Java => "java",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Go => "go",
            Self::Ruby => "ruby",
            Self::Dart => "dart",
            Self::Php => "php",
        }
    }
}

/// Configuration for the multilspy client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultilspyConfig {
    pub code_language: Language,
    #[serde(default)]
    pub trace_lsp_communication: bool,
}

impl MultilspyConfig {
    pub fn new(language: Language) -> Self {
        Self {
            code_language: language,
            trace_lsp_communication: false,
        }
    }

    pub fn with_tracing(mut self, trace: bool) -> Self {
        self.trace_lsp_communication = trace;
        self
    }
}

/// Directory settings for language server installations and caches.
pub struct MultilspySettings;

impl MultilspySettings {
    /// Returns (and creates) the directory for language server binaries.
    pub fn language_server_directory() -> PathBuf {
        let dir = home_dir().join(".multilspy").join("lsp");
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    /// Returns (and creates) the global cache directory.
    pub fn global_cache_directory() -> PathBuf {
        let dir = home_dir().join(".multilspy").join("global_cache");
        std::fs::create_dir_all(&dir).ok();
        dir
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_str_basic() {
        assert_eq!("rust".parse::<Language>().unwrap(), Language::Rust);
        assert_eq!("python".parse::<Language>().unwrap(), Language::Python);
        assert_eq!(
            "typescript".parse::<Language>().unwrap(),
            Language::TypeScript
        );
    }

    #[test]
    fn language_from_str_aliases() {
        assert_eq!("rs".parse::<Language>().unwrap(), Language::Rust);
        assert_eq!("py".parse::<Language>().unwrap(), Language::Python);
        assert_eq!("ts".parse::<Language>().unwrap(), Language::TypeScript);
        assert_eq!("js".parse::<Language>().unwrap(), Language::JavaScript);
        assert_eq!("c#".parse::<Language>().unwrap(), Language::CSharp);
        assert_eq!("golang".parse::<Language>().unwrap(), Language::Go);
    }

    #[test]
    fn language_from_str_invalid() {
        assert!("brainfuck".parse::<Language>().is_err());
    }

    #[test]
    fn language_display() {
        assert_eq!(Language::Rust.to_string(), "rust");
        assert_eq!(Language::TypeScript.to_string(), "typescript");
        assert_eq!(Language::CSharp.to_string(), "csharp");
    }

    #[test]
    fn language_serde_roundtrip() {
        let lang = Language::TypeScript;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"typescript\"");
        let parsed: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, lang);
    }

    #[test]
    fn config_builder() {
        let cfg = MultilspyConfig::new(Language::Rust).with_tracing(true);
        assert_eq!(cfg.code_language, Language::Rust);
        assert!(cfg.trace_lsp_communication);
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = MultilspyConfig::new(Language::Go);
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: MultilspyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.code_language, Language::Go);
        assert!(!parsed.trace_lsp_communication);
    }

    #[test]
    fn settings_directories() {
        let lsp_dir = MultilspySettings::language_server_directory();
        assert!(lsp_dir.ends_with("lsp"));
        let cache_dir = MultilspySettings::global_cache_directory();
        assert!(cache_dir.ends_with("global_cache"));
    }
}
