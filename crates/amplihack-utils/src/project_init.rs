//! Project initialization and `PROJECT.md` management.
//!
//! Ported from `amplihack/utils/project_initializer.py`. Detects the current
//! state of a project's `PROJECT.md`, analyzes the project structure, and
//! initializes or updates the file from a template.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors produced by project initialization operations.
#[derive(Debug, Error)]
pub enum ProjectInitError {
    /// An I/O error occurred reading or writing project files.
    #[error("project init I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The project directory does not exist.
    #[error("project directory does not exist: {path}")]
    DirNotFound { path: String },
}

/// Observed state of the `PROJECT.md` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectState {
    /// No `PROJECT.md` file exists.
    Missing,
    /// Contains amplihack-generated template markers.
    Template,
    /// Contains user-written content.
    Custom,
    /// Exists but is empty or unreadable.
    Stale,
}

/// Desired initialization behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitMode {
    /// Create if missing; leave existing user content alone.
    Create,
    /// Overwrite regardless of current state.
    Update,
    /// Report state only — never write.
    Skip,
}

/// Action taken during initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTaken {
    /// A new `PROJECT.md` was created.
    Initialized,
    /// An existing template file was regenerated.
    Regenerated,
    /// No write was performed.
    Skipped,
}

/// Outcome of [`initialize_project_md`].
#[derive(Debug, Clone)]
pub struct InitResult {
    /// What action was performed.
    pub action: ActionTaken,
    /// Path to `PROJECT.md`.
    pub path: PathBuf,
    /// Template applied, if any.
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

const AMPLIHACK_INDICATORS: &[&str] = &[
    "Microsoft Hackathon 2025",
    "Agentic Coding Framework",
    "Building the tools that build the future",
    "AI agents to accelerate software development",
];
const INDICATOR_THRESHOLD: usize = 2;
const PROJECT_MD_REL: &str = ".claude/context/PROJECT.md";
const PREVIEW_LIMIT: usize = 500;

const PROJECT_MD_TEMPLATE: &str = "# {project_name}\n\n\
    ## Description\n\n\
    {project_description}\n\n\
    ## Tech Stack\n\n\
    {tech_stack}\n\n\
    ## Getting Started\n\n\
    <!-- Add setup instructions here -->\n\n\
    ## Architecture\n\n\
    <!-- Describe the high-level architecture -->\n";

/// Detect the current state of `PROJECT.md` in `project_dir`.
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
    let hits = AMPLIHACK_INDICATORS
        .iter()
        .filter(|ind| lower.contains(&ind.to_lowercase()))
        .count();
    if hits >= INDICATOR_THRESHOLD {
        ProjectState::Template
    } else {
        ProjectState::Custom
    }
}

/// Analyze the structure of a project directory.
pub fn analyze_project_structure(project_dir: &Path) -> ProjectAnalysis {
    let name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());
    let languages = detect_languages(project_dir);
    let has_readme = project_dir.join("README.md").is_file();
    let readme_preview = read_preview(&project_dir.join("README.md"));
    let manifests = ["pyproject.toml", "package.json", "Cargo.toml", "go.mod"];
    let package_files = manifests
        .iter()
        .filter_map(|f| Some(((*f).to_owned(), read_preview(&project_dir.join(f))?)))
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
/// Returns [`ProjectInitError`] on I/O failures or if `project_dir` is missing.
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
    if mode == InitMode::Skip {
        return Ok(InitResult {
            action: ActionTaken::Skipped,
            path: md_path,
            template_used: None,
        });
    }
    if mode == InitMode::Create && matches!(state, ProjectState::Custom | ProjectState::Template) {
        return Ok(InitResult {
            action: ActionTaken::Skipped,
            path: md_path,
            template_used: None,
        });
    }
    let analysis = analyze_project_structure(project_dir);
    let content = generate_from_template(&analysis);
    if md_path.is_file() {
        std::fs::rename(&md_path, md_path.with_extension("md.bak"))?;
    }
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
    checks
        .iter()
        .filter(|(exts, _)| has_ext(project_dir, exts))
        .map(|(_, lang)| (*lang).to_owned())
        .collect()
}

fn has_ext(dir: &Path, extensions: &[&str]) -> bool {
    let scan = |d: &Path| -> bool {
        let Ok(entries) = std::fs::read_dir(d) else {
            return false;
        };
        entries.flatten().any(|e| {
            let p = e.path();
            p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| extensions.iter().any(|e| e.eq_ignore_ascii_case(x)))
        })
    };
    if scan(dir) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let p = e.path();
        p.is_dir() && {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !n.starts_with('.') && n != "node_modules" && n != "target" && scan(&p)
        }
    })
}

fn read_preview(path: &Path) -> Option<String> {
    let c = std::fs::read_to_string(path).ok()?;
    if c.is_empty() {
        return None;
    }
    let end = c
        .char_indices()
        .nth(PREVIEW_LIMIT)
        .map_or(c.len(), |(i, _)| i);
    Some(c[..end].to_owned())
}

fn generate_from_template(analysis: &ProjectAnalysis) -> String {
    let description = analysis
        .readme_preview
        .as_ref()
        .map(|p| extract_description(p))
        .unwrap_or_else(|| "<!-- Describe your project here -->".to_owned());
    let tech_stack = if analysis.languages.is_empty() {
        "<!-- List your tech stack here -->".to_owned()
    } else {
        let mut buf: String = analysis
            .languages
            .iter()
            .map(|l| format!("- {l}\n"))
            .collect();
        buf.push_str("- <!-- Add frameworks, databases, etc. -->");
        buf
    };
    PROJECT_MD_TEMPLATE
        .replace("{project_name}", &analysis.name)
        .replace("{project_description}", &description)
        .replace("{tech_stack}", &tech_stack)
}

fn extract_description(preview: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in preview.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            if lines.is_empty() {
                continue;
            } else {
                break;
            }
        }
        if t.is_empty() {
            if lines.is_empty() {
                continue;
            } else {
                break;
            }
        }
        lines.push(t);
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
