use super::helpers::{check_language, normalize_languages, scan_languages};
use super::indexer::{restore_root_index, run_indexer_for_language};
use super::types::{LANGUAGE_ORDER, NativeScipIndexSummary, PrerequisiteResult};
use crate::commands::memory::code_graph::{
    CodeGraphImportCounts, code_graph_compatibility_notice_for_project,
    resolve_code_graph_db_path_for_project,
};
use crate::commands::memory::{import_scip_file, project_artifact_paths, required_parent_dir};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

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
