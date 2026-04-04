use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Point / Range / Reference
// ---------------------------------------------------------------------------

/// A (line, character) position in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    pub line: u32,
    pub character: u32,
}

impl Point {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A start–end range in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Point,
    pub end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }

    pub fn from_coords(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Self {
        Self {
            start: Point::new(start_line, start_char),
            end: Point::new(end_line, end_char),
        }
    }
}

/// A reference to a code symbol at a specific location in a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reference {
    pub range: Range,
    pub uri: String,
}

impl Reference {
    pub fn new(range: Range, uri: impl Into<String>) -> Self {
        Self {
            range,
            uri: uri.into(),
        }
    }

    /// Construct from a JSON-like dict structure.
    pub fn from_dict(dict: &serde_json::Value) -> Option<Self> {
        let range_val = dict.get("range")?;
        let start = range_val.get("start")?;
        let end = range_val.get("end")?;

        let start_line = start.get("line")?.as_u64()? as u32;
        let start_char = start.get("character")?.as_u64()? as u32;
        let end_line = end.get("line")?.as_u64()? as u32;
        let end_char = end.get("character")?.as_u64()? as u32;

        let uri = dict.get("uri")?.as_str()?;

        Some(Self::new(
            Range::from_coords(start_line, start_char, end_line, end_char),
            uri,
        ))
    }

    /// Create an empty/zero reference.
    pub fn empty() -> Self {
        Self::new(Range::from_coords(0, 0, 0, 0), "")
    }

    pub fn start_line(&self) -> u32 {
        self.range.start.line
    }

    pub fn start_character(&self) -> u32 {
        self.range.start.character
    }

    pub fn end_line(&self) -> u32 {
        self.range.end.line
    }

    pub fn end_character(&self) -> u32 {
        self.range.end.character
    }
}

// ---------------------------------------------------------------------------
// SymbolRole (mirrors SCIP protobuf roles)
// ---------------------------------------------------------------------------

/// SCIP symbol roles for classifying occurrences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolRole;

impl SymbolRole {
    pub const DEFINITION: u32 = 1;
    pub const IMPORT: u32 = 2;
    pub const WRITE_ACCESS: u32 = 4;
    pub const READ_ACCESS: u32 = 8;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_equality() {
        let a = Point::new(1, 5);
        let b = Point::new(1, 5);
        let c = Point::new(2, 5);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn range_from_coords() {
        let r = Range::from_coords(1, 0, 10, 5);
        assert_eq!(r.start.line, 1);
        assert_eq!(r.end.character, 5);
    }

    #[test]
    fn reference_from_dict() {
        let dict = serde_json::json!({
            "range": {
                "start": {"line": 5, "character": 10},
                "end": {"line": 5, "character": 20}
            },
            "uri": "file:///repo/main.py"
        });
        let r = Reference::from_dict(&dict).unwrap();
        assert_eq!(r.start_line(), 5);
        assert_eq!(r.start_character(), 10);
        assert_eq!(r.uri, "file:///repo/main.py");
    }

    #[test]
    fn reference_from_dict_invalid() {
        let dict = serde_json::json!({"bad": "data"});
        assert!(Reference::from_dict(&dict).is_none());
    }

    #[test]
    fn reference_empty() {
        let r = Reference::empty();
        assert_eq!(r.start_line(), 0);
        assert!(r.uri.is_empty());
    }

    #[test]
    fn reference_serde_round_trip() {
        let r = Reference::new(Range::from_coords(1, 2, 3, 4), "file:///a.py");
        let json = serde_json::to_string(&r).unwrap();
        let back: Reference = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn symbol_role_constants() {
        assert_eq!(SymbolRole::DEFINITION, 1);
        assert_eq!(SymbolRole::IMPORT, 2);
        assert_eq!(SymbolRole::WRITE_ACCESS, 4);
        assert_eq!(SymbolRole::READ_ACCESS, 8);
    }
}
