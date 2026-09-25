use super::helpers::augmented_path;
use super::types::ScipIndexResult;
use crate::cli_memory::fs_move::move_path;
use crate::util::run_output_with_timeout;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SCIP_INDEXER_TIMEOUT: Duration = Duration::from_secs(600);

/// How a given `scip-*` indexer is told where to put its output.
///
/// The old code relied on the convention that every indexer drops `index.scip`
/// into the current directory — which is the project checkout. That convention
/// is what put an 8 MB binary in front of `git add -A` in
/// `rysweet/amplihack-recipe-runner` (issue #1476). This replaces it with an
/// explicit per-language decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScipOutput {
    /// Tier 1: the tool takes an output path; pass it.
    Flag,
    /// Tier 2: the tool takes the project root as an argument, so it can run
    /// from a directory outside the checkout and have its result moved.
    RootArgThenMove,
    /// Tier 3: neither. The language is skipped with a named reason and is
    /// **not** indexed into the repository.
    ///
    /// Expected to stay unused. It exists so that discovering a tool without
    /// an output flag cannot quietly reintroduce the in-repo write while a
    /// stub-based test stays green.
    Unsupported { reason: String },
}

/// A resolved invocation for one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScipPlan {
    pub(super) argv: Vec<String>,
    pub(super) working_dir: PathBuf,
    pub(super) output: ScipOutput,
}

/// Decide how to invoke `language`'s indexer so the result lands at `output`.
///
/// `output` is always outside the checkout; `project_path` is the one path that
/// may legitimately appear in an argument list.
pub(super) fn scip_plan_for_language(
    language: &str,
    project_path: &Path,
    output: &Path,
) -> Result<ScipPlan> {
    let out = output.display().to_string();
    let plan = match language {
        // `--output` confirmed in this workspace's own indexer wrapper,
        // `crates/amplihack-blarify/src/code_refs/scip.rs:44`.
        "python" => ScipPlan {
            argv: vec![
                "scip-python".to_string(),
                "index".to_string(),
                "--output".to_string(),
                out,
            ],
            working_dir: project_path.to_path_buf(),
            output: ScipOutput::Flag,
        },
        // `--output` confirmed in `crates/amplihack-blarify/src/code_refs/scip.rs:59`.
        // `javascript` runs the same binary; its temporary `tsconfig.json` is an
        // *input* and stays in the project root where the tool reads it.
        "typescript" | "javascript" => ScipPlan {
            argv: vec![
                "scip-typescript".to_string(),
                "index".to_string(),
                "--output".to_string(),
                out,
            ],
            working_dir: project_path.to_path_buf(),
            output: ScipOutput::Flag,
        },
        // `scip-go --output <file>` (sourcegraph/scip-go).
        "go" => ScipPlan {
            argv: vec!["scip-go".to_string(), "--output".to_string(), out],
            working_dir: project_path.to_path_buf(),
            output: ScipOutput::Flag,
        },
        // `rust-analyzer scip <project>` takes its project root as an argument
        // and writes `index.scip` into the working directory. That makes it the
        // ladder's motivating tier-2 case: "write into the repo, then move it
        // out" is not available to a tool whose project root comes from the
        // working directory, so the working directory moves instead.
        "rust" => ScipPlan {
            argv: vec![
                "rust-analyzer".to_string(),
                "scip".to_string(),
                project_path.display().to_string(),
            ],
            working_dir: staging_dir(output),
            output: ScipOutput::RootArgThenMove,
        },
        // `scip-dotnet index --output <file>`.
        "csharp" => ScipPlan {
            argv: vec![
                "scip-dotnet".to_string(),
                "index".to_string(),
                "--output".to_string(),
                out,
            ],
            working_dir: project_path.to_path_buf(),
            output: ScipOutput::Flag,
        },
        // `scip-clang --index-output-path <file>`; passed as two tokens so the
        // destination stays a path rather than being glued into a flag.
        "cpp" => ScipPlan {
            argv: vec![
                "scip-clang".to_string(),
                "--index-output-path".to_string(),
                out,
            ],
            working_dir: project_path.to_path_buf(),
            output: ScipOutput::Flag,
        },
        other => bail!("unsupported language: {other}"),
    };
    Ok(plan)
}

/// Where a tier-2 indexer runs and drops its `index.scip`: beside the final
/// artifact, which is already outside the checkout by construction.
fn staging_dir(output: &Path) -> PathBuf {
    output
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The file a tier-2 indexer writes by convention, inside its working
/// directory.
fn staged_index_path(working_dir: &Path) -> PathBuf {
    working_dir.join("index.scip")
}

pub(super) fn run_indexer_for_language(
    language: &str,
    project_path: &Path,
    artifact_dir: &Path,
) -> ScipIndexResult {
    let artifact_path = artifact_dir.join(format!("{language}.scip"));

    let plan = match scip_plan_for_language(language, project_path, &artifact_path) {
        Ok(plan) => plan,
        Err(err) => return failure(language, 0.0, err.to_string()),
    };
    if let ScipOutput::Unsupported { reason } = &plan.output {
        return failure(language, 0.0, reason.clone());
    }

    let (cleanup, setup_error) = match setup_language_indexer(language, project_path) {
        Ok(cleanup) => (cleanup, None),
        Err(err) => (None, Some(err.to_string())),
    };
    if let Some(error) = setup_error {
        return failure(language, 0.0, error);
    }

    // A leftover staging file from an interrupted run would otherwise be
    // mistaken for this run's output.
    let staged = staged_index_path(&plan.working_dir);
    if plan.output == ScipOutput::RootArgThenMove && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    if let Err(err) = fs::create_dir_all(&plan.working_dir) {
        return failure(
            language,
            0.0,
            format!("failed to create {}: {err}", plan.working_dir.display()),
        );
    }

    let started = Instant::now();
    let mut indexer = Command::new(&plan.argv[0]);
    indexer
        .args(&plan.argv[1..])
        .current_dir(&plan.working_dir)
        .env("PATH", augmented_path());
    let output = run_output_with_timeout(indexer, SCIP_INDEXER_TIMEOUT);
    let elapsed = started.elapsed().as_secs_f64();

    if let Some(cleanup) = cleanup {
        let _ = cleanup();
    }

    let output = match output {
        Ok(output) => output,
        Err(err) => return failure(language, elapsed, format!("failed to start indexer: {err}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return failure(
            language,
            elapsed,
            format!("indexer exited with {}: {}", output.status, stderr.trim()),
        );
    }

    if plan.output == ScipOutput::RootArgThenMove
        && let Err(err) = move_path(&staged, &artifact_path)
    {
        return failure(
            language,
            elapsed,
            format!(
                "failed to move {} to {}: {err:#}",
                staged.display(),
                artifact_path.display()
            ),
        );
    }

    if !artifact_path.exists() {
        return failure(
            language,
            elapsed,
            format!("{} was not created", artifact_path.display()),
        );
    }

    let size = fs::metadata(&artifact_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    ScipIndexResult {
        language: language.to_string(),
        success: true,
        artifact_path: Some(artifact_path),
        index_size_bytes: size,
        duration_seconds: elapsed,
        error_message: None,
    }
}

fn failure(language: &str, duration_seconds: f64, error_message: String) -> ScipIndexResult {
    ScipIndexResult {
        language: language.to_string(),
        success: false,
        artifact_path: None,
        index_size_bytes: 0,
        duration_seconds,
        error_message: Some(error_message),
    }
}

type CleanupFn = Box<dyn FnOnce() -> Result<()> + Send>;

/// Prepare inputs an indexer needs inside the project, returning the cleanup
/// that removes them again.
///
/// Only `javascript` needs anything: `scip-typescript` reads a `tsconfig.json`
/// from the project root. That file is an *input*, not an artifact — it is
/// written and then removed by the returned closure.
fn setup_language_indexer(language: &str, project_path: &Path) -> Result<Option<CleanupFn>> {
    if language != "javascript" {
        return Ok(None);
    }

    let tsconfig_path = project_path.join("tsconfig.json");
    if tsconfig_path.exists() {
        return Ok(None);
    }
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

    Ok(Some(Box::new(move || {
        if tsconfig_path.exists() {
            fs::remove_file(&tsconfig_path).with_context(|| {
                format!("failed to remove temporary {}", tsconfig_path.display())
            })?;
        }
        Ok(())
    }) as CleanupFn))
}

#[cfg(test)]
#[path = "indexer_output_tests.rs"]
mod output_tests;
