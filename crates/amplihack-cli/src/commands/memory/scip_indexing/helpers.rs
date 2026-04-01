use super::types::{IGNORED_DIRS, LanguageStatus};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn scan_languages(path: &Path, found: &mut BTreeMap<String, bool>) -> Result<()> {
    let entries = fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?;
    for entry in entries {
        let entry = entry?;
        let entry_path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();

        if entry_path.is_dir() {
            if should_ignore_dir(&file_name) {
                continue;
            }
            scan_languages(&entry_path, found)?;
            continue;
        }

        if !entry_path.is_file() {
            continue;
        }

        if let Some(language) = language_for_path(&entry_path) {
            found.insert(language.to_string(), true);
        }
    }
    Ok(())
}

pub(crate) fn should_ignore_dir(file_name: &str) -> bool {
    IGNORED_DIRS.contains(&file_name) || file_name.ends_with(".egg-info")
}

pub(crate) fn language_for_path(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|ext| ext.to_str())? {
        "py" => Some("python"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" => Some("javascript"),
        "go" => Some("go"),
        "rs" => Some("rust"),
        "cs" => Some("csharp"),
        "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" => Some("cpp"),
        _ => None,
    }
}

pub(super) fn check_language(language: &str) -> LanguageStatus {
    match language {
        "python" => require_tools(
            language,
            &["scip-python"],
            "pip install scip-python",
            "scip-python not found in PATH",
        ),
        "typescript" | "javascript" => require_tools(
            language,
            &["scip-typescript", "node"],
            "npm install -g @sourcegraph/scip-typescript typescript",
            "scip-typescript or node not found in PATH",
        ),
        "go" => require_tools(
            language,
            &["scip-go", "go"],
            "go install github.com/sourcegraph/scip-go@latest",
            "scip-go or go not found in PATH",
        ),
        "rust" => require_tools(
            language,
            &["rust-analyzer", "cargo"],
            "Install rust-analyzer and ensure cargo is on PATH",
            "rust-analyzer or cargo not found in PATH",
        ),
        "csharp" => require_tools(
            language,
            &["scip-dotnet", "dotnet"],
            "Install .NET SDK and scip-dotnet",
            "scip-dotnet or dotnet not found in PATH",
        ),
        "cpp" => require_tools(
            language,
            &["scip-clang"],
            "Install scip-clang and ensure compile_commands.json is available when needed",
            "scip-clang not found in PATH",
        ),
        other => LanguageStatus {
            language: other.to_string(),
            available: false,
            error_message: Some(format!("Unknown language: {other}")),
            missing_tools: Vec::new(),
            install_instructions: None,
        },
    }
}

fn require_tools(
    language: &str,
    tools: &[&str],
    install_instructions: &str,
    error_message: &str,
) -> LanguageStatus {
    let missing_tools: Vec<String> = tools
        .iter()
        .filter(|tool| which(tool).is_none())
        .map(|tool| (*tool).to_string())
        .collect();

    if missing_tools.is_empty() {
        LanguageStatus {
            language: language.to_string(),
            available: true,
            error_message: None,
            missing_tools,
            install_instructions: None,
        }
    } else {
        LanguageStatus {
            language: language.to_string(),
            available: false,
            error_message: Some(error_message.to_string()),
            missing_tools,
            install_instructions: Some(install_instructions.to_string()),
        }
    }
}

pub(crate) fn normalize_languages(languages: &[String]) -> Vec<String> {
    let mut unique = Vec::new();
    for language in languages {
        let normalized = normalize_language(language);
        if !unique.contains(&normalized) {
            unique.push(normalized);
        }
    }
    unique
}

fn normalize_language(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "js" => "javascript".to_string(),
        "ts" => "typescript".to_string(),
        "c++" | "cxx" | "cc" | "c" | "hpp" | "h" => "cpp".to_string(),
        other => other.to_string(),
    }
}

fn which(tool: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(tool);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

pub(super) fn augmented_path() -> String {
    let mut dirs = Vec::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".dotnet").join("tools"));
        dirs.push(home.join("go").join("bin"));
    }
    if let Some(current) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&current));
    }
    env::join_paths(dirs).map_or_else(
        |_| env::var("PATH").unwrap_or_default(),
        |paths| paths.to_string_lossy().into_owned(),
    )
}
