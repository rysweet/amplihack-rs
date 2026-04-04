//! Find symbols tool — locate code entities by name and type.
//!
//! Mirrors the Python `tools/find_symbols.py`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::db::manager::DbManager;
use crate::db::types::NodeFoundByNameTypeDto;

/// Valid symbol types for search.
const VALID_SYMBOL_TYPES: &[&str] = &["FUNCTION", "CLASS", "FILE", "FOLDER"];
/// Maximum number of results to return.
const MAX_RESULTS: usize = 15;

/// A found symbol search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolSearchResult {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub symbol_type: Vec<String>,
    pub file_path: String,
    #[serde(default)]
    pub code: Option<String>,
}

impl From<NodeFoundByNameTypeDto> for SymbolSearchResult {
    fn from(dto: NodeFoundByNameTypeDto) -> Self {
        Self {
            id: dto.node_id,
            name: dto.node_name,
            symbol_type: dto.node_type,
            file_path: dto.file_path,
            code: dto.code,
        }
    }
}

/// Input parameters for find_symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindSymbolsInput {
    pub name: String,
    #[serde(rename = "type")]
    pub symbol_type: String,
}

/// Find symbols by name and type in the code graph.
pub fn find_symbols(
    db_manager: &dyn DbManager,
    input: &FindSymbolsInput,
) -> Result<serde_json::Value> {
    let node_type = input.symbol_type.to_uppercase();

    if !VALID_SYMBOL_TYPES.contains(&node_type.as_str()) {
        return Ok(serde_json::json!({
            "error": format!("Invalid type '{}'. Must be one of: {}", node_type, VALID_SYMBOL_TYPES.join(", "))
        }));
    }

    let results = db_manager.get_node_by_name_and_type(&input.name, &node_type)?;

    if results.len() > MAX_RESULTS {
        return Ok(serde_json::json!({
            "error": format!("Too many results ({}). Please refine your search.", results.len())
        }));
    }

    let symbols: Vec<SymbolSearchResult> =
        results.into_iter().map(SymbolSearchResult::from).collect();
    Ok(serde_json::json!({"symbols": symbols}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_search_result_from_dto() {
        let dto = NodeFoundByNameTypeDto {
            node_id: "abc".into(),
            node_name: "my_func".into(),
            node_type: vec!["FUNCTION".into()],
            file_path: "src/main.rs".into(),
            code: Some("fn my_func() {}".into()),
        };
        let result = SymbolSearchResult::from(dto);
        assert_eq!(result.id, "abc");
        assert_eq!(result.name, "my_func");
    }

    #[test]
    fn valid_symbol_types_are_uppercase() {
        for st in VALID_SYMBOL_TYPES {
            assert_eq!(*st, st.to_uppercase());
        }
    }

    #[test]
    fn find_symbols_input_serialization() {
        let input = FindSymbolsInput {
            name: "handler".into(),
            symbol_type: "function".into(),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("handler"));
    }
}
