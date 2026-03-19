use super::code_graph::{
    CodeGraphImportCounts, code_graph_compatibility_notice_for_project,
    resolve_code_graph_db_path_for_project,
};
use super::import_scip_file;
use super::{project_artifact_paths, required_parent_dir};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub(crate) const LANGUAGE_ORDER: &[&str] = &[
    "python",
    "typescript",
    "javascript",
    "go",
    "rust",
    "csharp",
    "cpp",
];

const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    "node_modules",
    ".mypy_cache",
    ".tox",
    "dist",
    "build",
    ".eggs",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageStatus {
    pub language: String,
    pub available: bool,
    pub error_message: Option<String>,
    pub missing_tools: Vec<String>,
    pub install_instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerequisiteResult {
    pub can_proceed: bool,
    pub available_languages: Vec<String>,
    pub unavailable_languages: Vec<String>,
    pub partial_success: bool,
    pub language_statuses: BTreeMap<String, LanguageStatus>,
}

impl PrerequisiteResult {
    pub fn generate_report(&self) -> String {
        let mut lines = vec!["Prerequisite Check Report".to_string(), "=".repeat(40)];
        if !self.available_languages.is_empty() {
            lines.push(format!(
                "\nAvailable Languages ({}):",
                self.available_languages.len()
            ));
            for lang in &self.available_languages {
                lines.push(format!("  ✓ {lang}"));
            }
        }
        if !self.unavailable_languages.is_empty() {
            lines.push(format!(
                "\nUnavailable Languages ({}):",
                self.unavailable_languages.len()
            ));
            for lang in &self.unavailable_languages {
                if let Some(status) = self.language_statuses.get(lang) {
                    if let Some(error) = &status.error_message {
                        lines.push(format!("  ✗ {lang}: {error}"));
                        if let Some(instructions) = &status.install_instructions {
                            lines.push(format!("      Install: {instructions}"));
                        }
                    } else {
                        lines.push(format!("  ✗ {lang}"));
                    }
                }
            }
        }
        lines.push(format!("\nCan Proceed: {}", self.can_proceed));
        if self.partial_success {
            lines.push("Note: Partial success - some languages unavailable".to_string());
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScipIndexResult {
    pub language: String,
    pub success: bool,
    pub artifact_path: Option<PathBuf>,
    pub index_size_bytes: u64,
    pub duration_seconds: f64,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeScipIndexSummary {
    pub success: bool,
    pub completed_languages: Vec<String>,
    pub failed_languages: Vec<String>,
    pub skipped_languages: Vec<String>,
    pub artifacts: Vec<PathBuf>,
    pub errors: Vec<String>,
    pub partial_success: bool,
    pub import_counts: CodeGraphImportCounts,
}

pub fn run_index_scip(project_path: Option<&Path>, languages: &[String]) -> Result<()> {
    let mut summary = run_native_scip_indexing(project_path, languages)?;
    let resolved_project_path = project_path
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    if let Some(notice) = code_graph_compatibility_notice_for_project(&resolved_project_path, None)?
    {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    let db_path = resolve_code_graph_db_path_for_project(&resolved_project_path)?;
    let mut import_counts = CodeGraphImportCounts::default();
    for (language, artifact) in summary
        .completed_languages
        .iter()
        .zip(summary.artifacts.iter())
    {
        let counts = import_scip_file(
            artifact,
            &resolved_project_path,
            Some(language),
            Some(&db_path),
        )?;
        import_counts.files += counts.files;
        import_counts.classes += counts.classes;
        import_counts.functions += counts.functions;
        import_counts.imports += counts.imports;
        import_counts.relationships += counts.relationships;
    }
    summary.import_counts = import_counts;
    println!("Native SCIP indexing summary");
    println!("{}", "=".repeat(40));
    println!("Success: {}", summary.success);
    if !summary.completed_languages.is_empty() {
        println!("Completed: {}", summary.completed_languages.join(", "));
    }
    if !summary.failed_languages.is_empty() {
        println!("Failed: {}", summary.failed_languages.join(", "));
    }
    if !summary.skipped_languages.is_empty() {
        println!("Skipped: {}", summary.skipped_languages.join(", "));
    }
    for artifact in &summary.artifacts {
        println!("Artifact: {}", artifact.display());
    }
    println!(
        "Imported: files={}, classes={}, functions={}, imports={}, relationships={}",
        summary.import_counts.files,
        summary.import_counts.classes,
        summary.import_counts.functions,
        summary.import_counts.imports,
        summary.import_counts.relationships
    );
    if !summary.errors.is_empty() {
        for error in &summary.errors {
            eprintln!("Error: {error}");
        }
    }
    if !summary.success {
        bail!("native SCIP indexing did not complete successfully");
    }
    Ok(())
}

pub fn run_native_scip_indexing(
    project_path: Option<&Path>,
    explicit_languages: &[String],
) -> Result<NativeScipIndexSummary> {
    let project_path = project_path
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
    let languages = if explicit_languages.is_empty() {
        detect_project_languages(&project_path)?
    } else {
        normalize_languages(explicit_languages)
    };

    if languages.is_empty() {
        bail!(
            "no supported source files found in {}",
            project_path.display()
        );
    }

    let prereqs = check_prerequisites(&languages);
    if !prereqs.can_proceed {
        bail!("{}", prereqs.generate_report());
    }

    let mut completed_languages = Vec::new();
    let mut failed_languages = Vec::new();
    let skipped_languages = prereqs.unavailable_languages.clone();
    let mut artifacts = Vec::new();
    let mut errors = Vec::new();

    let artifact_paths = project_artifact_paths(&project_path);
    fs::create_dir_all(&artifact_paths.indexes_dir)
        .with_context(|| format!("failed to create {}", artifact_paths.indexes_dir.display()))?;

    let root_index = artifact_paths.root_index_scip.clone();
    let backup_index = if root_index.exists() {
        let backup = artifact_paths.index_scip_backup.clone();
        fs::create_dir_all(required_parent_dir(&backup)?)
            .with_context(|| format!("failed to create backup parent for {}", backup.display()))?;
        fs::copy(&root_index, &backup)
            .with_context(|| format!("failed to back up {}", root_index.display()))?;
        Some(backup)
    } else {
        None
    };

    for language in prereqs.available_languages {
        let result = run_indexer_for_language(
            &language,
            &project_path,
            &artifact_paths.indexes_dir,
            &root_index,
        );
        if result.success {
            completed_languages.push(result.language.clone());
            if let Some(path) = result.artifact_path {
                artifacts.push(path);
            }
        } else {
            failed_languages.push(result.language.clone());
            if let Some(error) = result.error_message {
                errors.push(format!("{}: {}", result.language, error));
            }
        }
    }

    restore_root_index(&root_index, backup_index.as_deref())?;

    let success = !completed_languages.is_empty();
    let partial_success =
        success && (!failed_languages.is_empty() || !skipped_languages.is_empty());

    Ok(NativeScipIndexSummary {
        success,
        completed_languages,
        failed_languages,
        skipped_languages,
        artifacts,
        errors,
        partial_success,
        import_counts: CodeGraphImportCounts::default(),
    })
}

pub fn detect_project_languages(project_path: &Path) -> Result<Vec<String>> {
    let mut found = BTreeMap::<String, bool>::new();
    scan_languages(project_path, &mut found)?;
    Ok(LANGUAGE_ORDER
        .iter()
        .filter(|lang| found.get(**lang).copied().unwrap_or(false))
        .map(|lang| (*lang).to_string())
        .collect())
}

pub fn check_prerequisites(languages: &[String]) -> PrerequisiteResult {
    let mut available = Vec::new();
    let mut unavailable = Vec::new();
    let mut statuses = BTreeMap::new();

    for language in normalize_languages(languages) {
        let status = check_language(&language);
        if status.available {
            available.push(language.clone());
        } else {
            unavailable.push(language.clone());
        }
        statuses.insert(language, status);
    }

    let can_proceed = !available.is_empty();
    let partial_success = can_proceed && !unavailable.is_empty();
    PrerequisiteResult {
        can_proceed,
        available_languages: available,
        unavailable_languages: unavailable,
        partial_success,
        language_statuses: statuses,
    }
}

fn scan_languages(path: &Path, found: &mut BTreeMap<String, bool>) -> Result<()> {
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

fn check_language(language: &str) -> LanguageStatus {
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

fn run_indexer_for_language(
    language: &str,
    project_path: &Path,
    artifact_dir: &Path,
    root_index: &Path,
) -> ScipIndexResult {
    if root_index.exists() {
        let _ = fs::remove_file(root_index);
    }

    let setup = setup_language_indexer(language, project_path);
    let (command, cleanup) = match setup {
        Ok(value) => value,
        Err(err) => {
            return ScipIndexResult {
                language: language.to_string(),
                success: false,
                artifact_path: None,
                index_size_bytes: 0,
                duration_seconds: 0.0,
                error_message: Some(err.to_string()),
            };
        }
    };

    let started = Instant::now();
    let output = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(project_path)
        .env("PATH", augmented_path())
        .output();
    let elapsed = started.elapsed().as_secs_f64();

    if let Some(cleanup) = cleanup {
        let _ = cleanup();
    }

    let output = match output {
        Ok(output) => output,
        Err(err) => {
            return ScipIndexResult {
                language: language.to_string(),
                success: false,
                artifact_path: None,
                index_size_bytes: 0,
                duration_seconds: elapsed,
                error_message: Some(format!("failed to start indexer: {err}")),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return ScipIndexResult {
            language: language.to_string(),
            success: false,
            artifact_path: None,
            index_size_bytes: 0,
            duration_seconds: elapsed,
            error_message: Some(format!(
                "indexer exited with {}: {}",
                output.status,
                stderr.trim()
            )),
        };
    }

    if !root_index.exists() {
        return ScipIndexResult {
            language: language.to_string(),
            success: false,
            artifact_path: None,
            index_size_bytes: 0,
            duration_seconds: elapsed,
            error_message: Some("index.scip was not created".to_string()),
        };
    }

    let artifact_path = artifact_dir.join(format!("{language}.scip"));
    let size = fs::metadata(root_index).map(|meta| meta.len()).unwrap_or(0);
    if let Err(err) = fs::rename(root_index, &artifact_path) {
        return ScipIndexResult {
            language: language.to_string(),
            success: false,
            artifact_path: None,
            index_size_bytes: size,
            duration_seconds: elapsed,
            error_message: Some(format!(
                "failed to move {} to {}: {err}",
                root_index.display(),
                artifact_path.display()
            )),
        };
    }

    ScipIndexResult {
        language: language.to_string(),
        success: true,
        artifact_path: Some(artifact_path),
        index_size_bytes: size,
        duration_seconds: elapsed,
        error_message: None,
    }
}

type CleanupFn = Box<dyn FnOnce() -> Result<()> + Send>;

fn setup_language_indexer(
    language: &str,
    project_path: &Path,
) -> Result<(Vec<String>, Option<CleanupFn>)> {
    match language {
        "python" => Ok((vec!["scip-python".to_string(), "index".to_string()], None)),
        "typescript" => Ok((
            vec!["scip-typescript".to_string(), "index".to_string()],
            None,
        )),
        "javascript" => {
            let tsconfig_path = project_path.join("tsconfig.json");
            let created = if !tsconfig_path.exists() {
                fs::write(
                    &tsconfig_path,
                    r#"{
  "compilerOptions": {
    "target": "es2020",
    "module": "commonjs",
    "allowJs": true,
    "checkJs": false,
    "skipLibCheck": true
  },
  "include": ["**/*.js", "**/*.jsx"],
  "exclude": ["node_modules", "dist", "build", "coverage"]
}
"#,
                )
                .with_context(|| format!("failed to create {}", tsconfig_path.display()))?;
                true
            } else {
                false
            };

            let cleanup = if created {
                Some(Box::new(move || {
                    if tsconfig_path.exists() {
                        fs::remove_file(&tsconfig_path).with_context(|| {
                            format!("failed to remove temporary {}", tsconfig_path.display())
                        })?;
                    }
                    Ok(())
                }) as CleanupFn)
            } else {
                None
            };

            Ok((
                vec!["scip-typescript".to_string(), "index".to_string()],
                cleanup,
            ))
        }
        "go" => Ok((vec!["scip-go".to_string()], None)),
        "rust" => Ok((
            vec![
                "rust-analyzer".to_string(),
                "scip".to_string(),
                project_path.display().to_string(),
            ],
            None,
        )),
        "csharp" => Ok((vec!["scip-dotnet".to_string(), "index".to_string()], None)),
        "cpp" => Ok((vec!["scip-clang".to_string()], None)),
        other => bail!("unsupported language: {other}"),
    }
}

fn restore_root_index(root_index: &Path, backup_path: Option<&Path>) -> Result<()> {
    if root_index.exists() {
        fs::remove_file(root_index)
            .with_context(|| format!("failed to remove temporary {}", root_index.display()))?;
    }
    if let Some(backup_path) = backup_path {
        fs::rename(backup_path, root_index).with_context(|| {
            format!(
                "failed to restore backed up index from {}",
                backup_path.display()
            )
        })?;
    }
    Ok(())
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

fn augmented_path() -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_executable(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn detect_project_languages_discovers_supported_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        fs::write(dir.path().join("src/app.ts"), "export {};\n").unwrap();

        let languages = detect_project_languages(dir.path()).unwrap();

        assert_eq!(languages, vec!["python", "typescript", "rust"]);
    }

    #[test]
    fn run_native_scip_indexing_with_stubbed_python_indexer_creates_artifact() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let project = tempfile::tempdir().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        fs::write(project.path().join("app.py"), "print('hi')\n").unwrap();

        write_executable(
            &bin_dir.path().join("scip-python"),
            "#!/bin/sh\nprintf 'stub-scip' > index.scip\n",
        );

        let old_path = env::var_os("PATH");
        unsafe { env::set_var("PATH", bin_dir.path()) };

        let summary = run_native_scip_indexing(Some(project.path()), &[]).unwrap();

        match old_path {
            Some(path) => unsafe { env::set_var("PATH", path) },
            None => unsafe { env::remove_var("PATH") },
        }

        assert!(summary.success);
        assert_eq!(summary.completed_languages, vec!["python"]);
        let artifact = project.path().join(".amplihack/indexes/python.scip");
        assert!(artifact.exists());
        assert_eq!(fs::read_to_string(artifact).unwrap(), "stub-scip");
    }
}
