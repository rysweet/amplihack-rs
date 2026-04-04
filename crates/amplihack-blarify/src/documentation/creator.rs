//! Documentation creator — orchestrates full documentation generation.
//!
//! Mirrors the Python `documentation/documentation_creator.py`.

use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use super::batch::{BatchConfig, BottomUpBatchProcessor};
use super::models::{DocumentationResult, FrameworkDetectionResult};
use super::queries;
use crate::db::manager::{DbManager, QueryParams};
use crate::graph::node::GraphEnvironment;

/// Orchestrates documentation generation for a codebase.
pub struct DocumentationCreator<'a> {
    db_manager: &'a dyn DbManager,
    graph_environment: GraphEnvironment,
    max_workers: usize,
    overwrite_documentation: bool,
}

impl<'a> DocumentationCreator<'a> {
    /// Create a new documentation creator.
    pub fn new(
        db_manager: &'a dyn DbManager,
        graph_environment: GraphEnvironment,
        max_workers: usize,
        overwrite_documentation: bool,
    ) -> Self {
        Self {
            db_manager,
            graph_environment,
            max_workers,
            overwrite_documentation,
        }
    }

    /// Create documentation, either targeted or full.
    pub fn create_documentation(
        &self,
        target_paths: Option<&[String]>,
        generate_embeddings: bool,
    ) -> Result<DocumentationResult> {
        let start = Instant::now();
        info!(
            targeted = target_paths.is_some(),
            "Starting documentation creation"
        );

        let result = if let Some(paths) = target_paths {
            self.create_targeted_documentation(paths, generate_embeddings)?
        } else {
            self.create_full_documentation(generate_embeddings)?
        };

        let elapsed = start.elapsed().as_secs_f64();
        info!(
            total_processed = result.total_nodes_processed,
            elapsed_secs = elapsed,
            "Documentation creation complete"
        );

        Ok(result)
    }

    /// Create documentation for specific target paths.
    fn create_targeted_documentation(
        &self,
        target_paths: &[String],
        generate_embeddings: bool,
    ) -> Result<DocumentationResult> {
        let mut total_processed = 0;

        for path in target_paths {
            let config = BatchConfig {
                max_workers: self.max_workers,
                overwrite_documentation: self.overwrite_documentation,
                generate_embeddings,
                ..Default::default()
            };

            let processor = BottomUpBatchProcessor::new(
                self.db_manager,
                self.graph_environment.clone(),
                config,
                None,
            );

            match processor.process_node(path) {
                Ok(result) => total_processed += result.total_nodes_processed,
                Err(e) => warn!(path, error = %e, "Failed to process path"),
            }
        }

        Ok(DocumentationResult {
            total_nodes_processed: total_processed,
            ..Default::default()
        })
    }

    /// Create documentation for the full codebase.
    fn create_full_documentation(&self, generate_embeddings: bool) -> Result<DocumentationResult> {
        let entry_points = self.discover_entry_points(None)?;

        let config = BatchConfig {
            max_workers: self.max_workers,
            overwrite_documentation: self.overwrite_documentation,
            generate_embeddings,
            ..Default::default()
        };

        let processor = BottomUpBatchProcessor::new(
            self.db_manager,
            self.graph_environment.clone(),
            config,
            None,
        );

        let mut total_processed = 0;
        for ep in &entry_points {
            if let Some(path) = ep.get("path").and_then(|v| v.as_str()) {
                match processor.process_node(path) {
                    Ok(result) => total_processed += result.total_nodes_processed,
                    Err(e) => warn!(path, error = %e, "Failed to process entry point"),
                }
            }
        }

        Ok(DocumentationResult {
            total_nodes_processed: total_processed,
            ..Default::default()
        })
    }

    /// Discover entry points from the graph.
    fn discover_entry_points(
        &self,
        file_paths: Option<&[String]>,
    ) -> Result<Vec<std::collections::HashMap<String, serde_json::Value>>> {
        let mut params = QueryParams::new();
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );

        let query = if let Some(paths) = file_paths {
            params.insert("file_paths".into(), serde_json::json!(paths));
            crate::db::queries::ENTRY_POINTS_FOR_FILE_PATHS_QUERY
        } else {
            crate::db::queries::POTENTIAL_ENTRY_POINTS_QUERY
        };

        self.db_manager
            .query(query, Some(&params), false)
            .context("Failed to discover entry points")
    }

    /// Clean up orphaned documentation nodes.
    pub fn cleanup_orphaned_documentation(&self) -> Result<usize> {
        let results = self
            .db_manager
            .query(queries::CLEANUP_ORPHANED_DOCUMENTATION, None, true)
            .context("Failed to cleanup orphaned documentation")?;

        let count = results
            .first()
            .and_then(|r| r.get("deleted_orphans"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        info!(deleted = count, "Cleaned up orphaned documentation nodes");
        Ok(count)
    }

    /// Parse a framework analysis string into a structured result.
    pub fn parse_framework_analysis(analysis: &str) -> FrameworkDetectionResult {
        match serde_json::from_str::<FrameworkDetectionResult>(analysis) {
            Ok(result) => result,
            Err(_) => {
                debug!("Framework analysis is not valid JSON, using raw text");
                FrameworkDetectionResult {
                    raw_analysis: analysis.into(),
                    ..Default::default()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_framework_analysis_json() {
        let json = r#"{"primary_framework": "Django", "confidence_score": 0.9}"#;
        let result = DocumentationCreator::parse_framework_analysis(json);
        assert_eq!(result.primary_framework, Some("Django".into()));
        assert!((result.confidence_score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_framework_analysis_raw_text() {
        let text = "This is a React application with TypeScript";
        let result = DocumentationCreator::parse_framework_analysis(text);
        assert!(result.primary_framework.is_none());
        assert_eq!(result.raw_analysis, text);
    }

    #[test]
    fn documentation_result_default() {
        let r = DocumentationResult::default();
        assert_eq!(r.total_nodes_processed, 0);
        assert!(r.warnings.is_empty());
    }
}
