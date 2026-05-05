//! TLA+ Prompt Experiment framework.
//!
//! Ports key structures from Python `amplihack/eval/tla_prompt_experiment.py`:
//! experiment manifests, matrix generation, cell results, and heuristic evaluation
//! for TLA+ prompt refinement experiments.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::EvalError;

/// A generation target describing what the experiment should produce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationTarget {
    /// Unique identifier for the target.
    pub target_id: String,
    /// Human-readable summary of the target.
    pub summary: String,
    /// Expected deliverables.
    pub deliverables: Vec<String>,
    /// Explicit non-goals to avoid scope creep.
    pub non_goals: Vec<String>,
}

/// A prompt refinement variant used in the experiment matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Refinement {
    /// Unique identifier for this refinement.
    pub refinement_id: String,
    /// Human-readable label.
    pub label: String,
    /// Description of the refinement approach.
    pub description: String,
    /// Optional path to a TLA+ spec file (relative to experiment home).
    pub tla_spec_file: Option<String>,
    /// Optional path to a prompt template file (relative to experiment home).
    pub prompt_file: Option<String>,
}

/// Manifest describing a full TLA+ prompt experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentManifest {
    /// Unique experiment identifier.
    pub experiment_id: String,
    /// Human-readable description.
    pub description: String,
    /// The generation target for this experiment.
    pub generation_target: GenerationTarget,
    /// Models to evaluate (e.g. `["gpt-4", "claude-3"]`).
    pub models: Vec<String>,
    /// Prompt refinement variants.
    pub refinements: Vec<Refinement>,
    /// Number of repetitions per (model, refinement) cell.
    pub repetitions: usize,
    /// Root directory for experiment assets.
    pub experiment_home: PathBuf,
}

impl ExperimentManifest {
    /// Load a manifest from a JSON file.
    pub fn from_file(path: &Path) -> Result<Self, EvalError> {
        let content = std::fs::read_to_string(path).map_err(|e| EvalError::IoError {
            path: Some(path.to_path_buf()),
            source: e,
        })?;
        let mut manifest: Self =
            serde_json::from_str(&content).map_err(|e| EvalError::ConfigError {
                message: format!("failed to parse manifest: {e}"),
            })?;
        // Default experiment_home to the manifest's parent directory.
        if manifest.experiment_home.as_os_str().is_empty()
            && let Some(parent) = path.parent()
        {
            manifest.experiment_home = parent.to_path_buf();
        }
        Ok(manifest)
    }

    /// Write the manifest to a JSON file.
    pub fn to_file(&self, path: &Path) -> Result<(), EvalError> {
        let content = serde_json::to_string_pretty(self).map_err(|e| EvalError::ConfigError {
            message: format!("failed to serialize manifest: {e}"),
        })?;
        std::fs::write(path, &content).map_err(|e| EvalError::IoError {
            path: Some(path.to_path_buf()),
            source: e,
        })?;
        Ok(())
    }

    /// Generate all matrix entries for this experiment.
    ///
    /// In smoke mode only the first model, first refinement, and 1 repetition are used.
    pub fn matrix_entries(&self, smoke_mode: bool) -> Vec<MatrixEntry> {
        let models = if smoke_mode {
            self.models.iter().take(1).collect::<Vec<_>>()
        } else {
            self.models.iter().collect::<Vec<_>>()
        };
        let refinements = if smoke_mode {
            self.refinements.iter().take(1).collect::<Vec<_>>()
        } else {
            self.refinements.iter().collect::<Vec<_>>()
        };
        let reps = if smoke_mode {
            1
        } else {
            self.repetitions.max(1)
        };

        let mut entries = Vec::new();
        for model in &models {
            for refinement in &refinements {
                for rep in 0..reps {
                    entries.push(MatrixEntry {
                        model: (*model).clone(),
                        refinement_id: refinement.refinement_id.clone(),
                        repetition: rep,
                    });
                }
            }
        }
        entries
    }

    /// Total number of cells in the experiment matrix.
    pub fn total_cells(&self, smoke_mode: bool) -> usize {
        if smoke_mode {
            return 1;
        }
        let m = self.models.len().max(1);
        let r = self.refinements.len().max(1);
        let reps = self.repetitions.max(1);
        m * r * reps
    }
}

/// A single cell in the experiment matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixEntry {
    /// Model name for this cell.
    pub model: String,
    /// Refinement variant ID.
    pub refinement_id: String,
    /// Repetition index (0-based).
    pub repetition: usize,
}

/// Result of executing a single experiment cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellResult {
    /// Unique cell identifier (e.g. `"model__refinement__rep"`).
    pub cell_id: String,
    /// Model used.
    pub model: String,
    /// Refinement variant used.
    pub refinement_id: String,
    /// Repetition index.
    pub repetition: usize,
    /// Path or content of the generated artifact, if successful.
    pub generated_artifact: Option<String>,
    /// Error message, if the cell failed.
    pub error: Option<String>,
    /// Wall-clock duration in seconds.
    pub duration_secs: f64,
    /// ISO 8601 timestamp of completion.
    pub timestamp: String,
}

/// Aggregated results for an entire experiment run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentResults {
    /// Experiment identifier (matches manifest).
    pub experiment_id: String,
    /// Path to the manifest file.
    pub manifest_path: PathBuf,
    /// Individual cell results.
    pub cells: Vec<CellResult>,
    /// ISO 8601 timestamp when experiment started.
    pub started_at: String,
    /// ISO 8601 timestamp when experiment completed.
    pub completed_at: Option<String>,
}

impl ExperimentResults {
    /// Write results to a JSON file.
    pub fn to_file(&self, path: &Path) -> Result<(), EvalError> {
        let content = serde_json::to_string_pretty(self).map_err(|e| EvalError::ConfigError {
            message: format!("failed to serialize results: {e}"),
        })?;
        std::fs::write(path, &content).map_err(|e| EvalError::IoError {
            path: Some(path.to_path_buf()),
            source: e,
        })?;
        Ok(())
    }

    /// Render a human-readable summary table of results.
    pub fn summary_table(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("Experiment: {}", self.experiment_id));
        lines.push(format!("Cells: {}", self.cells.len()));
        lines.push(String::new());
        lines.push(format!(
            "{:<20} {:<20} {:<5} {:<10} {:<8}",
            "Model", "Refinement", "Rep", "Duration", "Status"
        ));
        lines.push("-".repeat(65));

        for cell in &self.cells {
            let status = if cell.error.is_some() {
                "FAIL"
            } else if cell.generated_artifact.is_some() {
                "OK"
            } else {
                "EMPTY"
            };
            lines.push(format!(
                "{:<20} {:<20} {:<5} {:<10.2} {:<8}",
                cell.model, cell.refinement_id, cell.repetition, cell.duration_secs, status
            ));
        }

        let ok_count = self
            .cells
            .iter()
            .filter(|c| c.error.is_none() && c.generated_artifact.is_some())
            .count();
        let fail_count = self.cells.iter().filter(|c| c.error.is_some()).count();
        lines.push(String::new());
        lines.push(format!(
            "Summary: {} OK, {} FAIL, {} total",
            ok_count,
            fail_count,
            self.cells.len()
        ));
        lines.join("\n")
    }
}

/// Result of heuristic evaluation on a generated artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Score from 0.0 (worst) to 1.0 (best).
    pub score: f64,
    /// Signals detected during evaluation.
    pub signals: Vec<String>,
    /// Kind of evaluation performed.
    pub kind: String,
}

/// Load the prompt text for a given refinement from disk.
///
/// Returns the prompt template content or a default placeholder.
pub fn load_prompt_text(manifest: &ExperimentManifest, refinement: &Refinement) -> String {
    if let Some(ref prompt_file) = refinement.prompt_file {
        let path = manifest.experiment_home.join(prompt_file);
        match std::fs::read_to_string(&path) {
            Ok(content) => return content,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to load prompt file, using default"
                );
            }
        }
    }
    format!(
        "Generate code for: {}\nDeliverables: {}",
        manifest.generation_target.summary,
        manifest.generation_target.deliverables.join(", ")
    )
}

/// Load a TLA+ specification file for a given refinement.
///
/// Returns `None` if no spec file is configured or the file cannot be read.
pub fn load_tla_spec(manifest: &ExperimentManifest, refinement: &Refinement) -> Option<String> {
    let spec_file = refinement.tla_spec_file.as_ref()?;
    let path = manifest.experiment_home.join(spec_file);
    match std::fs::read_to_string(&path) {
        Ok(content) => Some(content),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to load TLA+ spec file"
            );
            None
        }
    }
}

/// Heuristic keyword/pattern-based evaluation of a generated artifact.
///
/// Checks for presence of expected keywords from the prompt and spec,
/// and scores based on signal density. This is intentionally simple —
/// no LLM calls are involved.
pub fn heuristic_evaluate(artifact: &str, prompt: &str, spec: Option<&str>) -> EvaluationResult {
    let mut signals = Vec::new();
    let mut score: f64 = 0.0;
    let artifact_lower = artifact.to_lowercase();

    // Signal: artifact is non-empty
    if !artifact.trim().is_empty() {
        signals.push("non_empty_artifact".to_string());
        score += 0.1;
    }

    // Signal: contains code-like patterns
    let code_keywords = [
        "fn ",
        "def ",
        "class ",
        "function ",
        "module ",
        "import ",
        "use ",
    ];
    let code_hits: usize = code_keywords
        .iter()
        .filter(|kw| artifact_lower.contains(*kw))
        .count();
    if code_hits > 0 {
        signals.push(format!("code_patterns:{code_hits}"));
        score += 0.15 * (code_hits as f64 / code_keywords.len() as f64);
    }

    // Signal: prompt keyword overlap
    let prompt_words: Vec<&str> = prompt.split_whitespace().filter(|w| w.len() > 3).collect();
    let overlap: usize = prompt_words
        .iter()
        .filter(|w| artifact_lower.contains(&w.to_lowercase()))
        .count();
    if !prompt_words.is_empty() {
        let ratio = overlap as f64 / prompt_words.len() as f64;
        if ratio > 0.1 {
            signals.push(format!("prompt_overlap:{ratio:.2}"));
            score += 0.25 * ratio.min(1.0);
        }
    }

    // Signal: TLA+ spec keywords present
    if let Some(spec_text) = spec {
        let spec_keywords: Vec<&str> = spec_text
            .split_whitespace()
            .filter(|w| w.len() > 4 && w.chars().next().is_some_and(|c| c.is_uppercase()))
            .collect();
        let spec_overlap: usize = spec_keywords
            .iter()
            .filter(|w| artifact_lower.contains(&w.to_lowercase()))
            .count();
        if !spec_keywords.is_empty() {
            let ratio = spec_overlap as f64 / spec_keywords.len() as f64;
            signals.push(format!("spec_overlap:{ratio:.2}"));
            score += 0.3 * ratio.min(1.0);
        }
    } else {
        // No spec: distribute remaining weight to other signals
        score += 0.1;
    }

    // Signal: structural completeness (has both start and end markers)
    let has_structure = (artifact.contains('{') && artifact.contains('}'))
        || (artifact.contains("begin") && artifact.contains("end"));
    if has_structure {
        signals.push("has_structure".to_string());
        score += 0.1;
    }

    // Signal: reasonable length
    let char_count = artifact.len();
    if char_count > 100 {
        signals.push(format!("length:{char_count}"));
        score += 0.1;
    }

    EvaluationResult {
        score: score.min(1.0),
        signals,
        kind: "heuristic".to_string(),
    }
}

/// Build a cell ID from matrix entry components.
pub fn build_cell_id(model: &str, refinement_id: &str, repetition: usize) -> String {
    format!("{model}__{refinement_id}__{repetition}")
}

/// Compute per-model aggregated scores from a map of cell_id → EvaluationResult.
pub fn per_model_scores(results: &HashMap<String, EvaluationResult>) -> HashMap<String, f64> {
    let mut model_scores: HashMap<String, Vec<f64>> = HashMap::new();
    for (cell_id, eval) in results {
        // Extract model from cell_id format "model__refinement__rep"
        if let Some(model) = cell_id.split("__").next() {
            model_scores
                .entry(model.to_string())
                .or_default()
                .push(eval.score);
        }
    }
    model_scores
        .into_iter()
        .map(|(model, scores)| {
            let avg = scores.iter().sum::<f64>() / scores.len() as f64;
            (model, avg)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ExperimentManifest {
        ExperimentManifest {
            experiment_id: "test-exp-001".to_string(),
            description: "Test experiment".to_string(),
            generation_target: GenerationTarget {
                target_id: "target-1".to_string(),
                summary: "Generate a REST API".to_string(),
                deliverables: vec!["server.rs".to_string(), "routes.rs".to_string()],
                non_goals: vec!["database migration".to_string()],
            },
            models: vec!["gpt-4".to_string(), "claude-3".to_string()],
            refinements: vec![
                Refinement {
                    refinement_id: "baseline".to_string(),
                    label: "Baseline".to_string(),
                    description: "No refinement".to_string(),
                    tla_spec_file: None,
                    prompt_file: None,
                },
                Refinement {
                    refinement_id: "with-spec".to_string(),
                    label: "With TLA+ spec".to_string(),
                    description: "Includes formal spec".to_string(),
                    tla_spec_file: Some("spec.tla".to_string()),
                    prompt_file: Some("prompt.txt".to_string()),
                },
            ],
            repetitions: 3,
            experiment_home: PathBuf::from("/tmp/test-experiment"),
        }
    }

    #[test]
    fn matrix_entries_full() {
        let manifest = sample_manifest();
        let entries = manifest.matrix_entries(false);
        // 2 models × 2 refinements × 3 reps = 12
        assert_eq!(entries.len(), 12);
        assert_eq!(entries[0].model, "gpt-4");
        assert_eq!(entries[0].refinement_id, "baseline");
        assert_eq!(entries[0].repetition, 0);
    }

    #[test]
    fn matrix_entries_smoke() {
        let manifest = sample_manifest();
        let entries = manifest.matrix_entries(true);
        // smoke: 1 model × 1 refinement × 1 rep = 1
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model, "gpt-4");
    }

    #[test]
    fn total_cells_matches() {
        let manifest = sample_manifest();
        assert_eq!(manifest.total_cells(false), 12);
        assert_eq!(manifest.total_cells(true), 1);
    }

    #[test]
    fn summary_table_renders() {
        let results = ExperimentResults {
            experiment_id: "test".to_string(),
            manifest_path: PathBuf::from("manifest.json"),
            cells: vec![
                CellResult {
                    cell_id: "gpt-4__baseline__0".to_string(),
                    model: "gpt-4".to_string(),
                    refinement_id: "baseline".to_string(),
                    repetition: 0,
                    generated_artifact: Some("fn main() {}".to_string()),
                    error: None,
                    duration_secs: 1.5,
                    timestamp: "2024-01-01T00:00:00Z".to_string(),
                },
                CellResult {
                    cell_id: "gpt-4__baseline__1".to_string(),
                    model: "gpt-4".to_string(),
                    refinement_id: "baseline".to_string(),
                    repetition: 1,
                    generated_artifact: None,
                    error: Some("timeout".to_string()),
                    duration_secs: 30.0,
                    timestamp: "2024-01-01T00:01:00Z".to_string(),
                },
            ],
            started_at: "2024-01-01T00:00:00Z".to_string(),
            completed_at: Some("2024-01-01T00:02:00Z".to_string()),
        };
        let table = results.summary_table();
        assert!(table.contains("1 OK"));
        assert!(table.contains("1 FAIL"));
        assert!(table.contains("2 total"));
    }

    #[test]
    fn heuristic_evaluate_code_artifact() {
        let artifact = r#"
            use std::io;
            fn main() {
                println!("Hello REST API");
            }
            module server {
                fn handle_request() {}
            }
        "#;
        let prompt = "Generate a REST API server with request handling";
        let result = heuristic_evaluate(artifact, prompt, None);
        assert!(
            result.score > 0.2,
            "score should be meaningful: {}",
            result.score
        );
        assert!(!result.signals.is_empty());
        assert_eq!(result.kind, "heuristic");
    }

    #[test]
    fn heuristic_evaluate_empty_artifact() {
        let result = heuristic_evaluate("", "Generate code", None);
        assert!(result.score < 0.2);
    }

    #[test]
    fn heuristic_evaluate_with_spec() {
        let artifact = "module TokenBucket { fn acquire() {} }";
        let prompt = "Implement rate limiter";
        let spec = "MODULE TokenBucket EXTENDS Naturals\nVARIABLE tokens, capacity";
        let result = heuristic_evaluate(artifact, prompt, Some(spec));
        assert!(
            result.signals.iter().any(|s| s.starts_with("spec_overlap")),
            "should detect spec overlap"
        );
    }

    #[test]
    fn build_cell_id_format() {
        let id = build_cell_id("gpt-4", "baseline", 2);
        assert_eq!(id, "gpt-4__baseline__2");
    }

    #[test]
    fn per_model_scores_aggregation() {
        let mut results = HashMap::new();
        results.insert(
            "gpt-4__a__0".to_string(),
            EvaluationResult {
                score: 0.8,
                signals: vec![],
                kind: "heuristic".to_string(),
            },
        );
        results.insert(
            "gpt-4__b__0".to_string(),
            EvaluationResult {
                score: 0.6,
                signals: vec![],
                kind: "heuristic".to_string(),
            },
        );
        results.insert(
            "claude__a__0".to_string(),
            EvaluationResult {
                score: 0.9,
                signals: vec![],
                kind: "heuristic".to_string(),
            },
        );
        let scores = per_model_scores(&results);
        assert!((scores["gpt-4"] - 0.7).abs() < f64::EPSILON);
        assert!((scores["claude"] - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let manifest = sample_manifest();
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let parsed: ExperimentManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.experiment_id, manifest.experiment_id);
        assert_eq!(parsed.models.len(), 2);
        assert_eq!(parsed.refinements.len(), 2);
        assert_eq!(parsed.repetitions, 3);
    }

    #[test]
    fn load_prompt_text_fallback() {
        let manifest = sample_manifest();
        let refinement = &manifest.refinements[0]; // no prompt_file
        let text = load_prompt_text(&manifest, refinement);
        assert!(text.contains("Generate a REST API"));
    }
}
