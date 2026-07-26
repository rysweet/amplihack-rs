//! SCIP protobuf parsing and conversion to blarify format.

use super::types::{BlarifyClass, BlarifyFile, BlarifyFunction, BlarifyOutput};

use prost::Message;
use std::path::Path;

pub(super) const SCIP_SYMBOL_ROLE_DEFINITION: i32 = 1;
pub(super) const SCIP_KIND_CLASS: i32 = 7;
const SCIP_KIND_CONSTRUCTOR: i32 = 9;
const SCIP_KIND_ENUM: i32 = 11;
pub(super) const SCIP_KIND_FUNCTION: i32 = 17;
const SCIP_KIND_METHOD: i32 = 26;
const SCIP_KIND_INTERFACE: i32 = 21;
const SCIP_KIND_MODULE: i32 = 29;
const SCIP_KIND_OBJECT: i32 = 33;
const SCIP_KIND_PROTOCOL_METHOD: i32 = 68;
const SCIP_KIND_PURE_VIRTUAL_METHOD: i32 = 69;
const SCIP_KIND_STATIC_METHOD: i32 = 80;
const SCIP_KIND_STRUCT: i32 = 49;
const SCIP_KIND_TRAIT: i32 = 53;
const SCIP_KIND_TRAIT_METHOD: i32 = 70;
const SCIP_KIND_TYPE: i32 = 54;
const SCIP_KIND_ABSTRACT_METHOD: i32 = 66;

#[derive(Clone, PartialEq, Message)]
pub(super) struct ScipIndex {
    #[prost(message, repeated, tag = "2")]
    pub(super) documents: Vec<ScipDocument>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ScipDocument {
    #[prost(string, tag = "4")]
    pub(super) language: String,
    #[prost(string, tag = "1")]
    pub(super) relative_path: String,
    #[prost(message, repeated, tag = "2")]
    pub(super) occurrences: Vec<ScipOccurrence>,
    #[prost(message, repeated, tag = "3")]
    pub(super) symbols: Vec<ScipSymbolInformation>,
    #[prost(string, tag = "5")]
    pub(super) text: String,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ScipSymbolInformation {
    #[prost(string, tag = "1")]
    pub(super) symbol: String,
    #[prost(string, repeated, tag = "3")]
    pub(super) documentation: Vec<String>,
    #[prost(int32, tag = "5")]
    pub(super) kind: i32,
    #[prost(string, tag = "6")]
    pub(super) display_name: String,
    #[prost(string, tag = "8")]
    pub(super) enclosing_symbol: String,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct ScipOccurrence {
    #[prost(int32, repeated, tag = "1")]
    pub(super) range: Vec<i32>,
    #[prost(string, tag = "2")]
    pub(super) symbol: String,
    #[prost(int32, tag = "3")]
    pub(super) symbol_roles: i32,
}

pub(super) fn convert_scip_to_blarify(
    index: &ScipIndex,
    project_root: &Path,
    language_hint: Option<&str>,
) -> BlarifyOutput {
    let mut payload = BlarifyOutput::default();

    for doc in &index.documents {
        let language = if doc.language.trim().is_empty() {
            language_hint.unwrap_or_default().to_string()
        } else {
            doc.language.clone()
        };
        let file_path = project_root.join(&doc.relative_path);
        let file_path = file_path.to_string_lossy().replace('\\', "/");
        let lines_of_code = doc.text.lines().count() as i64;
        payload.files.push(BlarifyFile {
            path: file_path.clone(),
            language,
            lines_of_code,
            last_modified: None,
        });

        for symbol in &doc.symbols {
            let symbol_name = symbol.symbol.trim();
            if symbol_name.is_empty() {
                continue;
            }

            let line_number = find_definition_line(symbol_name, &doc.occurrences);
            let docstring = symbol.documentation.join(" ");

            if is_function_symbol(symbol) {
                payload.functions.push(BlarifyFunction {
                    id: symbol_name.to_string(),
                    name: extract_name_from_symbol(symbol_name),
                    file_path: file_path.clone(),
                    line_number,
                    docstring,
                    parameters: Vec::new(),
                    return_type: String::new(),
                    is_async: false,
                    complexity: 0,
                    class_id: enclosing_class_id(symbol),
                });
            } else if is_class_symbol(symbol) {
                payload.classes.push(BlarifyClass {
                    id: symbol_name.to_string(),
                    name: extract_name_from_symbol(symbol_name),
                    file_path: file_path.clone(),
                    line_number,
                    docstring,
                    is_abstract: matches!(symbol.kind, SCIP_KIND_INTERFACE | SCIP_KIND_TRAIT),
                });
            }
        }
    }

    payload
}

fn find_definition_line(symbol: &str, occurrences: &[ScipOccurrence]) -> i64 {
    occurrences
        .iter()
        .find(|occ| occ.symbol == symbol && (occ.symbol_roles & SCIP_SYMBOL_ROLE_DEFINITION) != 0)
        .and_then(|occ| occ.range.first().copied())
        .map(i64::from)
        .unwrap_or(0)
}

fn extract_name_from_symbol(symbol: &str) -> String {
    if let Some(part) = symbol.rsplit('/').next() {
        return part
            .trim_end_matches('.')
            .trim_end_matches("()")
            .to_string();
    }
    if let Some(part) = symbol.split_whitespace().last() {
        return part
            .trim_end_matches('.')
            .trim_end_matches("()")
            .to_string();
    }
    symbol
        .trim_end_matches('.')
        .trim_end_matches("()")
        .to_string()
}

fn enclosing_class_id(symbol: &ScipSymbolInformation) -> Option<String> {
    let enclosing = symbol.enclosing_symbol.trim();
    if enclosing.is_empty() || !is_class_symbol_by_name(enclosing) {
        return None;
    }
    Some(enclosing.to_string())
}

fn is_function_symbol(symbol: &ScipSymbolInformation) -> bool {
    matches!(
        symbol.kind,
        SCIP_KIND_FUNCTION
            | SCIP_KIND_METHOD
            | SCIP_KIND_CONSTRUCTOR
            | SCIP_KIND_PROTOCOL_METHOD
            | SCIP_KIND_STATIC_METHOD
            | SCIP_KIND_TRAIT_METHOD
            | SCIP_KIND_ABSTRACT_METHOD
            | SCIP_KIND_PURE_VIRTUAL_METHOD
    ) || symbol.symbol.contains('(')
}

fn is_class_symbol(symbol: &ScipSymbolInformation) -> bool {
    matches!(
        symbol.kind,
        SCIP_KIND_CLASS
            | SCIP_KIND_INTERFACE
            | SCIP_KIND_STRUCT
            | SCIP_KIND_TRAIT
            | SCIP_KIND_OBJECT
            | SCIP_KIND_TYPE
            | SCIP_KIND_MODULE
            | SCIP_KIND_ENUM
    ) || is_class_symbol_by_name(&symbol.symbol)
}

fn is_class_symbol_by_name(symbol: &str) -> bool {
    if symbol.contains('(') {
        return false;
    }
    let name = extract_name_from_symbol(symbol);
    !name.is_empty()
        && name.chars().next().is_some_and(|ch| ch.is_uppercase())
        && !name.chars().all(|ch| ch.is_uppercase())
}
