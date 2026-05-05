//! Internal helpers for project structure detection and template generation.
//!
//! Extracted from `project_init` to stay within the 400-line limit.

use std::path::Path;

use crate::project_init::ProjectAnalysis;

/// Maximum bytes to read from preview files (README, package manifests).
const PREVIEW_LIMIT: usize = 500;

/// Default template for `PROJECT.md` generation.
const PROJECT_MD_TEMPLATE: &str = "\
# {project_name}

## Description

{project_description}

## Tech Stack

{tech_stack}

## Getting Started

<!-- Add setup instructions here -->

## Architecture

<!-- Describe the high-level architecture -->
";

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Detect programming languages by scanning for characteristic file extensions.
pub(crate) fn detect_languages(project_dir: &Path) -> Vec<String> {
    let checks: &[(&[&str], &str)] = &[
        (&["py"], "Python"),
        (&["js", "ts", "jsx", "tsx"], "JavaScript/TypeScript"),
        (&["rs"], "Rust"),
        (&["go"], "Go"),
        (&["java", "kt"], "Java/Kotlin"),
        (&["cs"], "C#"),
        (&["cpp", "cc", "cxx", "h", "hpp"], "C/C++"),
    ];

    let mut found = Vec::new();

    for (exts, lang) in checks {
        if has_file_with_extensions(project_dir, exts) {
            found.push((*lang).to_owned());
        }
    }

    found
}

/// Return `true` if `dir` (non-recursively for speed, then one level of
/// subdirectories) contains a file with one of the given extensions.
fn has_file_with_extensions(dir: &Path, extensions: &[&str]) -> bool {
    let scan = |d: &Path| -> bool {
        let entries = match std::fs::read_dir(d) {
            Ok(e) => e,
            Err(_) => return false,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Some(ext) = path.extension().and_then(|e| e.to_str())
                && extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
            {
                return true;
            }
        }
        false
    };

    // Check root first.
    if scan(dir) {
        return true;
    }

    // Check one level of subdirectories (src/, lib/, etc.).
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden dirs and common non-source dirs.
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') || name == "node_modules" || name == "target" {
                    continue;
                }
                if scan(&path) {
                    return true;
                }
            }
        }
    }

    false
}

/// Read up to [`PREVIEW_LIMIT`] characters from a file, returning `None` if
/// the file does not exist or cannot be read.
pub(crate) fn read_preview(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.is_empty() {
        return None;
    }
    let end = content
        .char_indices()
        .nth(PREVIEW_LIMIT)
        .map_or(content.len(), |(i, _)| i);
    Some(content[..end].to_owned())
}

/// Generate `PROJECT.md` content from the analysis using the built-in template.
pub(crate) fn generate_from_template(analysis: &ProjectAnalysis) -> String {
    let description = analysis
        .readme_preview
        .as_ref()
        .map(|preview| extract_description(preview))
        .unwrap_or_else(|| "<!-- Describe your project here -->".to_owned());

    let tech_stack = if analysis.languages.is_empty() {
        "<!-- List your tech stack here -->".to_owned()
    } else {
        let mut buf = String::new();
        for lang in &analysis.languages {
            buf.push_str(&format!("- {lang}\n"));
        }
        buf.push_str("- <!-- Add frameworks, databases, etc. -->");
        buf
    };

    PROJECT_MD_TEMPLATE
        .replace("{project_name}", &analysis.name)
        .replace("{project_description}", &description)
        .replace("{tech_stack}", &tech_stack)
}

/// Extract the first meaningful lines from a README preview as a description.
pub(crate) fn extract_description(preview: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();

    for line in preview.lines() {
        let trimmed = line.trim();
        // Skip the title line (starts with #).
        if trimmed.starts_with('#') {
            continue;
        }
        // Skip empty lines before content.
        if trimmed.is_empty() && lines.is_empty() {
            continue;
        }
        // Stop at the next heading.
        if !lines.is_empty() && trimmed.starts_with('#') {
            break;
        }
        if trimmed.is_empty() && !lines.is_empty() {
            break;
        }
        lines.push(trimmed);
        if lines.len() >= 2 {
            break;
        }
    }

    if lines.is_empty() {
        "<!-- Describe your project here -->".to_owned()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_init::ProjectAnalysis;

    #[test]
    fn detect_languages_finds_rust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        let langs = detect_languages(dir.path());
        assert_eq!(langs, vec!["Rust"]);
    }

    #[test]
    fn detect_languages_checks_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("app.py"), "pass").unwrap();
        let langs = detect_languages(dir.path());
        assert_eq!(langs, vec!["Python"]);
    }

    #[test]
    fn detect_languages_skips_hidden_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let hidden = dir.path().join(".cache");
        std::fs::create_dir(&hidden).unwrap();
        std::fs::write(hidden.join("script.py"), "pass").unwrap();
        let langs = detect_languages(dir.path());
        assert!(langs.is_empty());
    }

    #[test]
    fn detect_languages_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let langs = detect_languages(dir.path());
        assert!(langs.is_empty());
    }

    #[test]
    fn read_preview_returns_none_for_missing_file() {
        assert!(read_preview(Path::new("/nonexistent/file.md")).is_none());
    }

    #[test]
    fn read_preview_returns_none_for_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.md");
        std::fs::write(&path, "").unwrap();
        assert!(read_preview(&path).is_none());
    }

    #[test]
    fn read_preview_truncates_long_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.md");
        let content = "x".repeat(1000);
        std::fs::write(&path, &content).unwrap();
        let preview = read_preview(&path).unwrap();
        assert!(preview.len() <= PREVIEW_LIMIT);
    }

    #[test]
    fn extract_description_skips_title() {
        let readme = "# My Project\n\nA useful tool for things.";
        let desc = extract_description(readme);
        assert_eq!(desc, "A useful tool for things.");
    }

    #[test]
    fn extract_description_stops_at_heading() {
        let readme = "# Title\n\nFirst paragraph.\n\n## Details\n\nMore stuff.";
        let desc = extract_description(readme);
        assert_eq!(desc, "First paragraph.");
    }

    #[test]
    fn extract_description_returns_placeholder_for_empty() {
        assert!(extract_description("# Title Only").contains("<!--"));
    }

    #[test]
    fn generate_from_template_includes_languages() {
        let analysis = ProjectAnalysis {
            name: "test-project".to_owned(),
            languages: vec!["Rust".to_owned(), "Python".to_owned()],
            ..Default::default()
        };
        let output = generate_from_template(&analysis);
        assert!(output.contains("# test-project"));
        assert!(output.contains("- Rust"));
        assert!(output.contains("- Python"));
    }

    #[test]
    fn generate_from_template_empty_languages() {
        let analysis = ProjectAnalysis {
            name: "bare".to_owned(),
            ..Default::default()
        };
        let output = generate_from_template(&analysis);
        assert!(output.contains("<!-- List your tech stack here -->"));
    }
}
