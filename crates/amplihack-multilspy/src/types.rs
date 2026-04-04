use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A zero-indexed position in a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A range in a text document (start inclusive, end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// A location in a text document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
}

/// Identifies a text document by URI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// Newtype wrapper for LSP `CompletionItemKind` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompletionItemKind(pub u32);

impl CompletionItemKind {
    pub const TEXT: Self = Self(1);
    pub const METHOD: Self = Self(2);
    pub const FUNCTION: Self = Self(3);
    pub const CONSTRUCTOR: Self = Self(4);
    pub const FIELD: Self = Self(5);
    pub const VARIABLE: Self = Self(6);
    pub const CLASS: Self = Self(7);
    pub const INTERFACE: Self = Self(8);
    pub const MODULE: Self = Self(9);
    pub const PROPERTY: Self = Self(10);
    pub const UNIT: Self = Self(11);
    pub const VALUE: Self = Self(12);
    pub const ENUM: Self = Self(13);
    pub const KEYWORD: Self = Self(14);
    pub const SNIPPET: Self = Self(15);
    pub const COLOR: Self = Self(16);
    pub const FILE: Self = Self(17);
    pub const REFERENCE: Self = Self(18);
    pub const FOLDER: Self = Self(19);
    pub const ENUM_MEMBER: Self = Self(20);
    pub const CONSTANT: Self = Self(21);
    pub const STRUCT: Self = Self(22);
    pub const EVENT: Self = Self(23);
    pub const OPERATOR: Self = Self(24);
    pub const TYPE_PARAMETER: Self = Self(25);
}

impl fmt::Display for CompletionItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::TEXT => "Text",
            Self::METHOD => "Method",
            Self::FUNCTION => "Function",
            Self::CONSTRUCTOR => "Constructor",
            Self::FIELD => "Field",
            Self::VARIABLE => "Variable",
            Self::CLASS => "Class",
            Self::INTERFACE => "Interface",
            Self::MODULE => "Module",
            Self::PROPERTY => "Property",
            Self::UNIT => "Unit",
            Self::VALUE => "Value",
            Self::ENUM => "Enum",
            Self::KEYWORD => "Keyword",
            Self::SNIPPET => "Snippet",
            Self::COLOR => "Color",
            Self::FILE => "File",
            Self::REFERENCE => "Reference",
            Self::FOLDER => "Folder",
            Self::ENUM_MEMBER => "EnumMember",
            Self::CONSTANT => "Constant",
            Self::STRUCT => "Struct",
            Self::EVENT => "Event",
            Self::OPERATOR => "Operator",
            Self::TYPE_PARAMETER => "TypeParameter",
            _ => return write!(f, "Unknown({})", self.0),
        };
        f.write_str(name)
    }
}

/// A completion result from the language server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub completion_text: String,
    pub kind: CompletionItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Newtype wrapper for LSP `SymbolKind` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKind(pub u32);

impl SymbolKind {
    pub const FILE: Self = Self(1);
    pub const MODULE: Self = Self(2);
    pub const NAMESPACE: Self = Self(3);
    pub const PACKAGE: Self = Self(4);
    pub const CLASS: Self = Self(5);
    pub const METHOD: Self = Self(6);
    pub const PROPERTY: Self = Self(7);
    pub const FIELD: Self = Self(8);
    pub const CONSTRUCTOR: Self = Self(9);
    pub const ENUM: Self = Self(10);
    pub const INTERFACE: Self = Self(11);
    pub const FUNCTION: Self = Self(12);
    pub const VARIABLE: Self = Self(13);
    pub const CONSTANT: Self = Self(14);
    pub const STRING: Self = Self(15);
    pub const NUMBER: Self = Self(16);
    pub const BOOLEAN: Self = Self(17);
    pub const ARRAY: Self = Self(18);
    pub const OBJECT: Self = Self(19);
    pub const KEY: Self = Self(20);
    pub const NULL: Self = Self(21);
    pub const ENUM_MEMBER: Self = Self(22);
    pub const STRUCT: Self = Self(23);
    pub const EVENT: Self = Self(24);
    pub const OPERATOR: Self = Self(25);
    pub const TYPE_PARAMETER: Self = Self(26);
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::FILE => "File",
            Self::MODULE => "Module",
            Self::NAMESPACE => "Namespace",
            Self::PACKAGE => "Package",
            Self::CLASS => "Class",
            Self::METHOD => "Method",
            Self::PROPERTY => "Property",
            Self::FIELD => "Field",
            Self::CONSTRUCTOR => "Constructor",
            Self::ENUM => "Enum",
            Self::INTERFACE => "Interface",
            Self::FUNCTION => "Function",
            Self::VARIABLE => "Variable",
            Self::CONSTANT => "Constant",
            Self::STRING => "String",
            Self::NUMBER => "Number",
            Self::BOOLEAN => "Boolean",
            Self::ARRAY => "Array",
            Self::OBJECT => "Object",
            Self::KEY => "Key",
            Self::NULL => "Null",
            Self::ENUM_MEMBER => "EnumMember",
            Self::STRUCT => "Struct",
            Self::EVENT => "Event",
            Self::OPERATOR => "Operator",
            Self::TYPE_PARAMETER => "TypeParameter",
            _ => return write!(f, "Unknown({})", self.0),
        };
        f.write_str(name)
    }
}

/// Unified symbol information combining `SymbolInformation` and `DocumentSymbol`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: SymbolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_range: Option<Range>,
    #[serde(default)]
    pub deprecated: bool,
}

/// Markup content kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkupKind {
    PlainText,
    Markdown,
}

/// Markup content from the language server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkupContent {
    pub kind: MarkupKind,
    pub value: String,
}

/// Hover information returned by the language server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoverResult {
    pub contents: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// Newtype wrapper for LSP `DiagnosticSeverity` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiagnosticSeverity(pub u32);

impl DiagnosticSeverity {
    pub const ERROR: Self = Self(1);
    pub const WARNING: Self = Self(2);
    pub const INFORMATION: Self = Self(3);
    pub const HINT: Self = Self(4);
}

impl fmt::Display for DiagnosticSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::ERROR => "Error",
            Self::WARNING => "Warning",
            Self::INFORMATION => "Information",
            Self::HINT => "Hint",
            _ => return write!(f, "Unknown({})", self.0),
        };
        f.write_str(name)
    }
}

/// A diagnostic (error, warning, etc.) from the language server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<DiagnosticSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Converts a filesystem path to a `file://` URI.
pub fn path_to_uri(path: &Path) -> String {
    // Canonicalize separators for consistency.
    let abs = path
        .to_str()
        .map(|s| s.replace('\\', "/"))
        .unwrap_or_default();
    if abs.starts_with('/') {
        format!("file://{abs}")
    } else {
        format!("file:///{abs}")
    }
}

/// Converts a `file://` URI back to a filesystem path string.
pub fn uri_to_path(uri: &str) -> Option<String> {
    uri.strip_prefix("file://").map(|s| {
        // Handle the triple-slash form on Windows (`file:///C:/...`)
        if s.starts_with('/') && s.len() > 2 && s.as_bytes()[2] == b':' {
            s[1..].to_string()
        } else {
            s.to_string()
        }
    })
}

/// Detects language id from a file extension.
pub fn detect_language_id(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("py" | "pyi") => "python",
        Some("ts" | "tsx") => "typescript",
        Some("js" | "jsx" | "mjs" | "cjs") => "javascript",
        Some("go") => "go",
        Some("java") => "java",
        Some("cs") => "csharp",
        Some("rb") => "ruby",
        Some("php") => "php",
        Some("dart") => "dart",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_serde() {
        let pos = Position::new(10, 5);
        let json = serde_json::to_value(pos).unwrap();
        assert_eq!(json["line"], 10);
        assert_eq!(json["character"], 5);
        let parsed: Position = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, pos);
    }

    #[test]
    fn range_serde() {
        let range = Range::new(Position::new(1, 0), Position::new(1, 10));
        let json = serde_json::to_string(&range).unwrap();
        let parsed: Range = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, range);
    }

    #[test]
    fn location_optional_fields() {
        let loc = Location {
            uri: "file:///foo.rs".into(),
            range: Range::new(Position::new(0, 0), Position::new(0, 5)),
            absolute_path: None,
            relative_path: None,
        };
        let json = serde_json::to_string(&loc).unwrap();
        assert!(!json.contains("absolute_path"));
        assert!(!json.contains("relative_path"));
    }

    #[test]
    fn completion_item_kind_display() {
        assert_eq!(CompletionItemKind::FUNCTION.to_string(), "Function");
        assert_eq!(CompletionItemKind::CLASS.to_string(), "Class");
        assert_eq!(CompletionItemKind(99).to_string(), "Unknown(99)");
    }

    #[test]
    fn symbol_kind_display() {
        assert_eq!(SymbolKind::METHOD.to_string(), "Method");
        assert_eq!(SymbolKind::STRUCT.to_string(), "Struct");
        assert_eq!(SymbolKind(99).to_string(), "Unknown(99)");
    }

    #[test]
    fn diagnostic_severity_display() {
        assert_eq!(DiagnosticSeverity::ERROR.to_string(), "Error");
        assert_eq!(DiagnosticSeverity::HINT.to_string(), "Hint");
    }

    #[test]
    fn path_to_uri_unix() {
        let uri = path_to_uri(Path::new("/home/user/project/src/main.rs"));
        assert_eq!(uri, "file:///home/user/project/src/main.rs");
    }

    #[test]
    fn uri_to_path_unix() {
        let path = uri_to_path("file:///home/user/main.rs").unwrap();
        assert_eq!(path, "/home/user/main.rs");
    }

    #[test]
    fn detect_language_id_extensions() {
        assert_eq!(detect_language_id("main.rs"), "rust");
        assert_eq!(detect_language_id("app.py"), "python");
        assert_eq!(detect_language_id("index.ts"), "typescript");
        assert_eq!(detect_language_id("main.go"), "go");
        assert_eq!(detect_language_id("Foo.java"), "java");
        assert_eq!(detect_language_id("README.md"), "plaintext");
    }

    #[test]
    fn hover_result_serde() {
        let hover = HoverResult {
            contents: "fn main()".into(),
            range: Some(Range::new(Position::new(0, 0), Position::new(0, 9))),
        };
        let json = serde_json::to_string(&hover).unwrap();
        let parsed: HoverResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, hover);
    }
}
