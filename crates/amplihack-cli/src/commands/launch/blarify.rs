//! Blarify code-graph indexing prompt and execution logic.

use crate::commands::memory::{
    background_index_job_active, check_index_status, code_graph_compatibility_notice_for_project,
    estimate_indexing_time, record_background_index_pid, resolve_code_graph_db_path_for_project,
    run_index_code, run_index_scip,
};
use crate::util::read_user_input_with_timeout;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const BLARIFY_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlarifyPromptChoice {
    Skip,
    Never,
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlarifyIndexAction {
    ImportExistingJson,
    GenerateNativeScip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlarifyMode {
    Prompt,
    Skip,
    Sync,
    Background,
}

pub(super) fn should_prompt_blarify_indexing(tool: &str, noninteractive: bool) -> bool {
    tool == "claude"
        && std::env::var("AMPLIHACK_ENABLE_BLARIFY").as_deref() == Ok("1")
        && (!noninteractive || blarify_mode() != BlarifyMode::Prompt)
}

pub(super) fn maybe_run_blarify_indexing_prompt(
    tool: &str,
    noninteractive: bool,
    current_dir: Option<&Path>,
) -> Result<()> {
    maybe_run_blarify_indexing_prompt_with(
        tool,
        noninteractive,
        current_dir,
        maybe_prompt_blarify_indexing,
    )
}

pub(super) fn maybe_run_blarify_indexing_prompt_with<F>(
    tool: &str,
    noninteractive: bool,
    current_dir: Option<&Path>,
    prompt_runner: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    if !should_prompt_blarify_indexing(tool, noninteractive) {
        return Ok(());
    }

    let project_path = current_dir.context("failed to resolve current directory")?;
    if let Some(notice) = code_graph_compatibility_notice_for_project(project_path, None)? {
        println!("⚠️ Compatibility mode: {notice}");
    }
    prompt_runner(project_path).with_context(|| {
        format!(
            "code graph indexing prompt failed for {}",
            project_path.display()
        )
    })
}

fn maybe_prompt_blarify_indexing(project_path: &Path) -> Result<()> {
    if has_blarify_consent(project_path)? {
        tracing::debug!(project = %project_path.display(), "skipping code graph prompt due to saved consent");
        return Ok(());
    }
    if background_index_job_active(project_path)? {
        tracing::debug!(project = %project_path.display(), "skipping code graph prompt because indexing is already running");
        return Ok(());
    }

    let status = check_index_status(project_path)?;
    let db_path = resolve_code_graph_db_path_for_project(project_path)?;
    let code_graph_missing = !db_path.exists();
    if !status.needs_indexing && !code_graph_missing {
        tracing::debug!(reason = %status.reason, "code graph artifact is current");
        return Ok(());
    }

    let json_path = blarify_json_path(project_path);
    let action = resolve_blarify_index_action(&status, &json_path);
    let display_reason = if status.needs_indexing {
        status.reason.clone()
    } else {
        format!("missing (no {} found)", db_path.display())
    };
    match blarify_mode() {
        BlarifyMode::Skip => {
            tracing::info!("code indexing skipped by AMPLIHACK_BLARIFY_MODE=skip");
            return Ok(());
        }
        BlarifyMode::Sync => return run_code_indexing(project_path, &json_path, action, false),
        BlarifyMode::Background => {
            return run_code_indexing(project_path, &json_path, action, true);
        }
        BlarifyMode::Prompt => {}
    }
    print_blarify_prompt_banner(
        project_path,
        &display_reason,
        status.estimated_files,
        &json_path,
        action,
    )?;
    let response = read_user_input_with_timeout(
        "\nRun code indexing? [y/N/b/n] (b=background, n=don't ask again): ",
        BLARIFY_PROMPT_TIMEOUT,
    )?;

    match parse_blarify_prompt_choice(response.as_deref()) {
        BlarifyPromptChoice::Skip => {
            println!(
                "\n⏭️  Skipping code indexing (run later with: {})\n",
                manual_indexing_hint(project_path, &json_path, action)
            );
            Ok(())
        }
        BlarifyPromptChoice::Never => {
            save_blarify_consent(project_path)?;
            println!("\n⏭️  Code indexing skipped (won't ask again for this project)\n");
            Ok(())
        }
        BlarifyPromptChoice::Foreground => {
            run_code_indexing(project_path, &json_path, action, false)
        }
        BlarifyPromptChoice::Background => {
            run_code_indexing(project_path, &json_path, action, true)
        }
    }
}

fn print_blarify_prompt_banner(
    project_path: &Path,
    status_reason: &str,
    estimated_files: usize,
    json_path: &Path,
    action: BlarifyIndexAction,
) -> Result<()> {
    let estimate = estimate_indexing_time(project_path, &[]);
    println!();
    println!("{}", "=".repeat(60));
    println!("Code Indexing with Blarify");
    println!("{}", "=".repeat(60));
    println!("Project: {}", project_path.display());
    println!("Status: {status_reason}");
    println!("Files to index: {estimated_files}");
    println!(
        "Estimated time: {}",
        format_duration_seconds(estimate.total_seconds)
    );
    println!();
    println!("Language breakdown:");
    for (language, seconds) in &estimate.by_language {
        let file_count = estimate.file_counts.get(language).copied().unwrap_or(0);
        if file_count == 0 {
            continue;
        }
        println!(
            "  • {}: {} files ({})",
            language_label(language),
            file_count,
            format_duration_seconds(*seconds)
        );
    }
    println!();
    println!("Blarify enables code-aware features:");
    println!("  • Code context in memory retrieval");
    println!("  • Function and class awareness");
    println!("  • Automatic code-memory linking");
    println!();
    if json_path.exists() && action == BlarifyIndexAction::ImportExistingJson {
        println!("Native import input: {}", json_path.display());
    } else {
        println!("Rust will use native SCIP artifact generation to refresh the code graph.");
    }
    println!("{}", "=".repeat(60));
    io::stdout()
        .flush()
        .context("failed to flush prompt banner")
}

fn format_duration_seconds(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.0}s")
    } else {
        let minutes = (seconds / 60.0).floor() as u64;
        let remaining_seconds = (seconds % 60.0).floor() as u64;
        format!("{minutes}m {remaining_seconds}s")
    }
}

fn language_label(language: &str) -> String {
    match language {
        "typescript" => "TypeScript".to_string(),
        "javascript" => "JavaScript".to_string(),
        "go" => "Go".to_string(),
        "rust" => "Rust".to_string(),
        "csharp" => "Csharp".to_string(),
        "cpp" => "Cpp".to_string(),
        "python" => "Python".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

fn run_code_indexing(
    project_path: &Path,
    json_path: &Path,
    action: BlarifyIndexAction,
    background: bool,
) -> Result<()> {
    let db_path = resolve_code_graph_db_path_for_project(project_path)?;
    if background {
        let current_exe =
            std::env::current_exe().context("failed to resolve current executable")?;
        let mut cmd = Command::new(current_exe);
        let child = match action {
            BlarifyIndexAction::ImportExistingJson => {
                cmd.arg("index-code")
                    .arg(json_path)
                    .arg("--db-path")
                    .arg(&db_path);
                cmd.current_dir(project_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .with_context(|| {
                        format!(
                            "failed to spawn background indexing for {}",
                            project_path.display()
                        )
                    })?
            }
            BlarifyIndexAction::GenerateNativeScip => {
                cmd.arg("index-scip")
                    .arg("--project-path")
                    .arg(project_path);
                cmd.current_dir(project_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .with_context(|| {
                        format!(
                            "failed to spawn background indexing for {}",
                            project_path.display()
                        )
                    })?
            }
        };
        record_background_index_pid(project_path, child.id())?;
        println!("\n📊 Started code indexing in the background.\n");
        return Ok(());
    }

    match action {
        BlarifyIndexAction::ImportExistingJson => {
            println!("\n📊 Importing code graph data...\n");
            run_index_code(json_path, Some(&db_path), false)?;
            println!("\n✅ Code graph import complete.\n");
        }
        BlarifyIndexAction::GenerateNativeScip => {
            println!("\n📊 Generating native SCIP artifacts...\n");
            run_index_scip(Some(project_path), &[])?;
            println!("\n✅ Native SCIP artifact generation complete.\n");
        }
    }
    Ok(())
}

pub(super) fn parse_blarify_prompt_choice(response: Option<&str>) -> BlarifyPromptChoice {
    let response = response
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match response.as_str() {
        "n" | "never" | "skip" => BlarifyPromptChoice::Never,
        "b" | "background" => BlarifyPromptChoice::Background,
        "y" | "yes" => BlarifyPromptChoice::Foreground,
        _ => BlarifyPromptChoice::Skip,
    }
}

pub(super) fn has_blarify_consent(project_path: &Path) -> Result<bool> {
    Ok(consent_cache_path(project_path)?.exists())
}

pub(super) fn save_blarify_consent(project_path: &Path) -> Result<()> {
    let cache_path = consent_cache_path(project_path)?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&cache_path, b"1")
        .with_context(|| format!("failed to write {}", cache_path.display()))?;
    Ok(())
}

pub(super) fn consent_cache_path(project_path: &Path) -> Result<PathBuf> {
    let resolved = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(resolved.to_string_lossy().as_bytes());
        let digest = hasher.finalize();
        format!("{digest:x}")[..16].to_string()
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; cannot persist code graph consent")?;
    Ok(home
        .join(".amplihack")
        .join(format!(".blarify_consent_{hash}")))
}

fn blarify_json_path(project_path: &Path) -> PathBuf {
    project_path.join(".amplihack").join("blarify.json")
}

fn manual_indexing_hint(
    project_path: &Path,
    json_path: &Path,
    action: BlarifyIndexAction,
) -> String {
    match action {
        BlarifyIndexAction::ImportExistingJson => format!(
            "amplihack index-code {} --db-path {}",
            json_path.display(),
            resolve_code_graph_db_path_for_project(project_path)
                .unwrap_or_else(|_| project_path.join(".amplihack").join("graph_db"))
                .display()
        ),
        BlarifyIndexAction::GenerateNativeScip => format!(
            "amplihack index-scip --project-path {}",
            project_path.display()
        ),
    }
}

pub(super) fn blarify_mode() -> BlarifyMode {
    match std::env::var("AMPLIHACK_BLARIFY_MODE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "skip" => BlarifyMode::Skip,
        "sync" => BlarifyMode::Sync,
        "background" => BlarifyMode::Background,
        _ => BlarifyMode::Prompt,
    }
}

pub(super) fn resolve_blarify_index_action(
    status: &crate::commands::memory::IndexStatus,
    json_path: &Path,
) -> BlarifyIndexAction {
    if json_path.exists() && !status.needs_indexing {
        BlarifyIndexAction::ImportExistingJson
    } else {
        BlarifyIndexAction::GenerateNativeScip
    }
}
