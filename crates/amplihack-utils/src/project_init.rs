//! Project initialization and `PROJECT.md` management.
//!
//! Ported from `amplihack/utils/project_initializer.py`. Detects the current
//! state of a project's `PROJECT.md`, analyzes the project structure
//! (languages, frameworks), and initializes or updates the file from a
//! template.

use std::path::{Path, PathBuf};

use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by project initialization operations.
#[derive(Debug, Error)]
pub enum ProjectInitError {
    /// An I/O error occurred reading or writing project files.
    #[error("project init I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The project directory does not exist.
    #[error("project directory does not exist: {path}")]
    DirNotFound {
        /// The missing directory path.
        path: String,
    },
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Observed state of the `PROJECT.md` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectState {
    /// No `PROJECT.md` file exists.
    Missing,
    /// The file contains amplihack-generated template markers.
    Template,
    /// The file contains user-written content.
    Custom,
    /// The file exists but is empty or unreadable.
    Stale,
}

/// Desired initialization behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitMode {
    /// Create the file if missing; leave existing user content alone.
    Create,
    /// Overwrite the file regardless of current state.
    Update,
    /// Report state only — never write.
    Skip,
}

/// Action that was actually taken during initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTaken {
    /// A new `PROJECT.md` was created.
    Initialized,
    /// An existing template file was regenerated.
    Regenerated,
    /// No write was performed.
    Skipped,
}

// ---------------------------------------------------------------------------
// Result / analysis structs
// ---------------------------------------------------------------------------

/// Outcome of a [`initialize_project_md`] call.
#[derive(Debug, Clone)]
pub struct InitResult {
    /// What action was performed.
    pub action: ActionTaken,
    /// Path to the `PROJECT.md` file.
    pub path: PathBuf,
    /// Name of the template that was applied, if any.
    pub template_used: Option<String>,
}

/// High-level analysis of a project's structure.
#[derive(Debug, Clone, Default)]
pub struct ProjectAnalysis {
    /// Inferred project name (directory stem).
    pub name: String,
    /// Programming languages detected via file extensions.
    pub languages: Vec<String>,
    /// Whether a `README.md` exists at the project root.
    pub has_readme: bool,
    /// First 500 characters of `README.md`, if present.
    pub readme_preview: Option<String>,
    /// Snippets from package manifest files (key = filename).
    pub package_files: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Markers that indicate amplihack-generated content inside `PROJECT.md`.
const AMPLIHACK_INDICATORS: &[&str] = &[
    "Microsoft Hackathon 2025",
    "Agentic Coding Framework",
    "Building the tools that build the future",
    "AI agents to accelerate software development",
];

/// Minimum number of indicator matches to classify a file as template-generated.
const INDICATOR_THRESHOLD: usize = 2;

/// Relative path from the project root to `PROJECT.md`.
const PROJECT_MD_REL: &str = ".claude/context/PROJECT.md";

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
// Public API
// ---------------------------------------------------------------------------

/// Detect the current state of `PROJECT.md` in `project_dir`.
///
/// Reads `.claude/context/PROJECT.md` relative to `project_dir` and
/// classifies its content.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::project_init::detect_project_state;
/// use std::path::Path;
///
/// let state = detect_project_state(Path::new("/my/project"));
/// ```
pub fn detect_project_state(project_dir: &Path) -> ProjectState {
    let md_path = project_dir.join(PROJECT_MD_REL);

    if !md_path.exists() {
        return ProjectState::Missing;
    }

    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %md_path.display(), error = %e, "could not read PROJECT.md");
            return ProjectState::Stale;
        }
    };

    if content.trim().is_empty() {
        return ProjectState::Stale;
    }

    let lower = content.to_lowercase();
    let matches = AMPLIHACK_INDICATORS
        .iter()
        .filter(|ind| lower.contains(&ind.to_lowercase()))
        .count();

    if matches >= INDICATOR_THRESHOLD {
        ProjectState::Template
    } else {
        ProjectState::Custom
    }
}

/// Analyze the structure of a project directory.
///
/// Inspects file extensions to detect languages, reads package manifests, and
/// captures a README preview.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::project_init::analyze_project_structure;
/// use std::path::Path;
///
/// let analysis = analyze_project_structure(Path::new("/my/project"));
/// ```
pub fn analyze_project_structure(project_dir: &Path) -> ProjectAnalysis {
    let name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());

    let languages = detect_languages(project_dir);
    let has_readme = project_dir.join("README.md").is_file();
    let readme_preview = read_preview(&project_dir.join("README.md"));

    let manifest_names = ["pyproject.toml", "package.json", "Cargo.toml", "go.mod"];
    let package_files: Vec<(String, String)> = manifest_names
        .iter()
        .filter_map(|fname| {
            let preview = read_preview(&project_dir.join(fname))?;
            Some(((*fname).to_owned(), preview))
        })
        .collect();

    ProjectAnalysis {
        name,
        languages,
        has_readme,
        readme_preview,
        package_files,
    }
}

/// Initialize or update `PROJECT.md` according to `mode`.
///
/// # Errors
///
/// Returns [`ProjectInitError`] on I/O failures or if the project directory
/// does not exist.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::project_init::{initialize_project_md, InitMode};
/// use std::path::Path;
///
/// let result = initialize_project_md(Path::new("/my/project"), InitMode::Create)?;
/// # Ok::<(), amplihack_utils::project_init::ProjectInitError>(())
/// ```
pub fn initialize_project_md(
    project_dir: &Path,
    mode: InitMode,
) -> Result<InitResult, ProjectInitError> {
    if !project_dir.is_dir() {
        return Err(ProjectInitError::DirNotFound {
            path: project_dir.display().to_string(),
        });
    }

    let md_path = project_dir.join(PROJECT_MD_REL);
    let state = detect_project_state(project_dir);

    // Skip mode: report only.
    if mode == InitMode::Skip {
        return Ok(InitResult {
            action: ActionTaken::Skipped,
            path: md_path,
            template_used: None,
        });
    }

    // Create mode: only write if missing or template.
    if mode == InitMode::Create && state == ProjectState::Custom {
        tracing::debug!("PROJECT.md has custom content — skipping");
        return Ok(InitResult {
            action: ActionTaken::Skipped,
            path: md_path,
            template_used: None,
        });
    }

    // If already a template and mode is Create (not Update), skip.
    if mode == InitMode::Create && state == ProjectState::Template {
        tracing::debug!("PROJECT.md already contains template — skipping");
        return Ok(InitResult {
            action: ActionTaken::Skipped,
            path: md_path,
            template_used: None,
        });
    }

    // Generate content.
    let analysis = analyze_project_structure(project_dir);
    let content = generate_from_template(&analysis);

    // Backup existing file if present.
    if md_path.is_file() {
        let backup = md_path.with_extension("md.bak");
        std::fs::rename(&md_path, &backup)?;
        tracing::info!(backup = %backup.display(), "backed up existing PROJECT.md");
    }

    // Ensure parent directory exists.
    if let Some(parent) = md_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&md_path, &content)?;

    let action = if state == ProjectState::Missing {
        ActionTaken::Initialized
    } else {
        ActionTaken::Regenerated
    };

    Ok(InitResult {
        action,
        path: md_path,
        template_used: Some("default".to_owned()),
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Detect programming languages by scanning for characteristic file extensions.
fn detect_languages(project_dir: &Path) -> Vec<String> {
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
fn read_preview(path: &Path) -> Option<String> {
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
fn generate_from_template(analysis: &ProjectAnalysis) -> String {
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
fn extract_description(preview: &str) -> String {
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
#[path = "tests/project_init_tests.rs"]
mod tests;
