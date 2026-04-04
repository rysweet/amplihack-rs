use std::collections::HashSet;

use crate::graph::node::NodeLabel;
use crate::graph::relationship::RelationshipType;

// ---------------------------------------------------------------------------
// FoundRelationshipScope
// ---------------------------------------------------------------------------

/// Result of relationship-type detection from tree-sitter analysis.
#[derive(Debug, Clone)]
pub struct FoundRelationshipScope {
    /// The tree-sitter node where the reference was found (opaque ID).
    pub node_in_scope: Option<String>,
    /// The determined relationship type.
    pub relationship_type: RelationshipType,
}

// ---------------------------------------------------------------------------
// LanguageDefinitions trait
// ---------------------------------------------------------------------------

/// Defines language-specific rules for parsing and node classification.
///
/// Each supported language implements this trait to provide:
/// - Which AST node types should generate graph nodes
/// - How to extract identifiers and body nodes
/// - How to determine relationship types from AST context
/// - File extension → parser mapping
pub trait LanguageDefinitions: std::fmt::Debug + Send + Sync {
    /// Human-readable language name.
    fn language_name(&self) -> &str;

    /// File extensions this language handles (with dots, e.g. `.py`).
    fn file_extensions(&self) -> HashSet<String>;

    /// AST node type names that should generate graph nodes.
    fn node_creating_types(&self) -> HashSet<String>;

    /// Map an AST node type to a [`NodeLabel`].
    fn node_label_from_type(&self, node_type: &str) -> Option<NodeLabel>;

    /// Control-flow statement types (for complexity calculation).
    fn control_flow_statements(&self) -> &[&str];

    /// Consequence statement types (for complexity calculation).
    fn consequence_statements(&self) -> &[&str];
}

// ---------------------------------------------------------------------------
// PythonDefinitions
// ---------------------------------------------------------------------------

/// Python language definitions.
#[derive(Debug)]
pub struct PythonDefinitions;

impl LanguageDefinitions for PythonDefinitions {
    fn language_name(&self) -> &str {
        "python"
    }

    fn file_extensions(&self) -> HashSet<String> {
        [".py", ".pyx", ".pyi"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    fn node_creating_types(&self) -> HashSet<String> {
        [
            "function_definition",
            "class_definition",
            "decorated_definition",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }

    fn node_label_from_type(&self, node_type: &str) -> Option<NodeLabel> {
        match node_type {
            "function_definition" => Some(NodeLabel::Function),
            "class_definition" => Some(NodeLabel::Class),
            "decorated_definition" => Some(NodeLabel::Function),
            _ => None,
        }
    }

    fn control_flow_statements(&self) -> &[&str] {
        &[
            "if_statement",
            "for_statement",
            "while_statement",
            "try_statement",
            "with_statement",
            "match_statement",
        ]
    }

    fn consequence_statements(&self) -> &[&str] {
        &[
            "elif_clause",
            "else_clause",
            "except_clause",
            "finally_clause",
            "case_clause",
        ]
    }
}

// ---------------------------------------------------------------------------
// TypeScriptDefinitions
// ---------------------------------------------------------------------------

/// TypeScript/JavaScript language definitions.
#[derive(Debug)]
pub struct TypeScriptDefinitions;

impl LanguageDefinitions for TypeScriptDefinitions {
    fn language_name(&self) -> &str {
        "typescript"
    }

    fn file_extensions(&self) -> HashSet<String> {
        [".ts", ".tsx", ".js", ".jsx"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    fn node_creating_types(&self) -> HashSet<String> {
        [
            "function_declaration",
            "method_definition",
            "class_declaration",
            "arrow_function",
            "function_expression",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }

    fn node_label_from_type(&self, node_type: &str) -> Option<NodeLabel> {
        match node_type {
            "function_declaration" | "arrow_function" | "function_expression" => {
                Some(NodeLabel::Function)
            }
            "method_definition" => Some(NodeLabel::Method),
            "class_declaration" => Some(NodeLabel::Class),
            _ => None,
        }
    }

    fn control_flow_statements(&self) -> &[&str] {
        &[
            "if_statement",
            "for_statement",
            "for_in_statement",
            "while_statement",
            "do_statement",
            "try_statement",
            "switch_statement",
        ]
    }

    fn consequence_statements(&self) -> &[&str] {
        &[
            "else_clause",
            "catch_clause",
            "finally_clause",
            "switch_case",
            "switch_default",
        ]
    }
}

// ---------------------------------------------------------------------------
// FallbackDefinitions
// ---------------------------------------------------------------------------

/// Fallback definitions for unsupported languages.
/// Returns empty sets — files are treated as raw/opaque.
#[derive(Debug)]
pub struct FallbackDefinitions;

impl LanguageDefinitions for FallbackDefinitions {
    fn language_name(&self) -> &str {
        "unknown"
    }

    fn file_extensions(&self) -> HashSet<String> {
        HashSet::new()
    }

    fn node_creating_types(&self) -> HashSet<String> {
        HashSet::new()
    }

    fn node_label_from_type(&self, _node_type: &str) -> Option<NodeLabel> {
        None
    }

    fn control_flow_statements(&self) -> &[&str] {
        &[]
    }

    fn consequence_statements(&self) -> &[&str] {
        &[]
    }
}

// ---------------------------------------------------------------------------
// Language registry
// ---------------------------------------------------------------------------

/// Get language definitions for a file extension.
pub fn definitions_for_extension(ext: &str) -> Box<dyn LanguageDefinitions> {
    match ext {
        ".py" | ".pyx" | ".pyi" => Box::new(PythonDefinitions),
        ".ts" | ".tsx" | ".js" | ".jsx" => Box::new(TypeScriptDefinitions),
        _ => Box::new(FallbackDefinitions),
    }
}

/// Get language definitions by language name.
pub fn definitions_for_language(lang: &str) -> Box<dyn LanguageDefinitions> {
    match lang {
        "python" => Box::new(PythonDefinitions),
        "typescript" | "javascript" => Box::new(TypeScriptDefinitions),
        _ => Box::new(FallbackDefinitions),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_definitions_basics() {
        let defs = PythonDefinitions;
        assert_eq!(defs.language_name(), "python");
        assert!(defs.file_extensions().contains(".py"));
        assert!(defs.node_creating_types().contains("function_definition"));
    }

    #[test]
    fn python_node_label() {
        let defs = PythonDefinitions;
        assert_eq!(
            defs.node_label_from_type("function_definition"),
            Some(NodeLabel::Function)
        );
        assert_eq!(
            defs.node_label_from_type("class_definition"),
            Some(NodeLabel::Class)
        );
        assert_eq!(defs.node_label_from_type("unknown_type"), None);
    }

    #[test]
    fn typescript_definitions_basics() {
        let defs = TypeScriptDefinitions;
        assert_eq!(defs.language_name(), "typescript");
        assert!(defs.file_extensions().contains(".ts"));
        assert!(defs.file_extensions().contains(".jsx"));
    }

    #[test]
    fn typescript_node_label() {
        let defs = TypeScriptDefinitions;
        assert_eq!(
            defs.node_label_from_type("function_declaration"),
            Some(NodeLabel::Function)
        );
        assert_eq!(
            defs.node_label_from_type("method_definition"),
            Some(NodeLabel::Method)
        );
        assert_eq!(
            defs.node_label_from_type("class_declaration"),
            Some(NodeLabel::Class)
        );
    }

    #[test]
    fn fallback_definitions_empty() {
        let defs = FallbackDefinitions;
        assert_eq!(defs.language_name(), "unknown");
        assert!(defs.file_extensions().is_empty());
        assert!(defs.node_creating_types().is_empty());
        assert_eq!(defs.node_label_from_type("anything"), None);
    }

    #[test]
    fn definitions_for_extension_python() {
        let defs = definitions_for_extension(".py");
        assert_eq!(defs.language_name(), "python");
    }

    #[test]
    fn definitions_for_extension_typescript() {
        let defs = definitions_for_extension(".ts");
        assert_eq!(defs.language_name(), "typescript");
    }

    #[test]
    fn definitions_for_extension_unknown() {
        let defs = definitions_for_extension(".xyz");
        assert_eq!(defs.language_name(), "unknown");
    }

    #[test]
    fn definitions_for_language_python() {
        let defs = definitions_for_language("python");
        assert!(defs.file_extensions().contains(".py"));
    }

    #[test]
    fn python_control_flow_statements() {
        let defs = PythonDefinitions;
        let stmts = defs.control_flow_statements();
        assert!(stmts.contains(&"if_statement"));
        assert!(stmts.contains(&"for_statement"));
    }

    #[test]
    fn typescript_control_flow_statements() {
        let defs = TypeScriptDefinitions;
        let stmts = defs.control_flow_statements();
        assert!(stmts.contains(&"switch_statement"));
    }
}
