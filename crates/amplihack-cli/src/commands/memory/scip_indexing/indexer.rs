use super::helpers::augmented_path;
use super::types::ScipIndexResult;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

pub(super) fn run_indexer_for_language(
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

pub(super) fn restore_root_index(root_index: &Path, backup_path: Option<&Path>) -> Result<()> {
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
