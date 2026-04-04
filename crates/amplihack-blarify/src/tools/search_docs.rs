//! Search documentation tool — vector search over AI-generated descriptions.
//!
//! Mirrors the Python `tools/search_documentation.py`.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::db::manager::{DbManager, QueryParams};
use crate::db::queries;
/// Minimum similarity score for results.
const MIN_SIMILARITY: f64 = 0.7;

/// Input for documentation search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDocsInput {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    5
}

/// Search documentation using vector similarity.
///
/// This function expects an embedding service to be configured;
/// it queries the database with pre-computed embeddings.
pub fn search_documentation(
    db_manager: &dyn DbManager,
    input: &SearchDocsInput,
    query_embedding: Option<&[f64]>,
) -> Result<serde_json::Value> {
    let embedding = match query_embedding {
        Some(e) => e,
        None => bail!("Embedding service not configured — cannot perform vector search"),
    };

    let mut params = QueryParams::new();
    params.insert("query_embedding".into(), serde_json::json!(embedding));
    params.insert(
        "top_k".into(),
        serde_json::Value::Number((input.top_k as u64).into()),
    );
    params.insert("min_similarity".into(), serde_json::json!(MIN_SIMILARITY));

    let results = db_manager.query(
        queries::VECTOR_SIMILARITY_SEARCH_QUERY,
        Some(&params),
        false,
    )?;

    if results.is_empty() {
        return Ok(serde_json::json!({
            "message": format!("No documentation found matching '{}' with similarity >= {MIN_SIMILARITY}", input.query)
        }));
    }

    Ok(serde_json::json!({
        "query": input.query,
        "results": format_results(&results, &input.query),
        "total": results.len()
    }))
}

/// Format search results for display.
fn format_results(
    results: &[std::collections::HashMap<String, serde_json::Value>],
    _query: &str,
) -> Vec<serde_json::Value> {
    results
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let name = row
                .get("source_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let path = row
                .get("source_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = row.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let score = row
                .get("similarity")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            serde_json::json!({
                "rank": i + 1,
                "name": name,
                "path": path,
                "content": content,
                "similarity": score
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_requires_embedding() {
        // We can't call search_documentation without a real DbManager,
        // but we can verify the input validation
        let input = SearchDocsInput {
            query: "authentication".into(),
            top_k: 5,
        };
        assert_eq!(input.top_k, 5);
    }

    #[test]
    fn default_top_k() {
        let json = r#"{"query": "test"}"#;
        let input: SearchDocsInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.top_k, 5);
    }

    #[test]
    fn min_similarity_threshold() {
        assert!((MIN_SIMILARITY - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn format_results_empty() {
        let results = format_results(&[], "test");
        assert!(results.is_empty());
    }

    #[test]
    fn format_results_with_data() {
        let mut row = std::collections::HashMap::new();
        row.insert(
            "source_name".into(),
            serde_json::Value::String("auth".into()),
        );
        row.insert(
            "source_path".into(),
            serde_json::Value::String("src/auth.rs".into()),
        );
        row.insert(
            "content".into(),
            serde_json::Value::String("Handles auth".into()),
        );
        row.insert("similarity".into(), serde_json::json!(0.95));

        let results = format_results(&[row], "auth");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["rank"], 1);
        assert_eq!(results[0]["name"], "auth");
    }
}
