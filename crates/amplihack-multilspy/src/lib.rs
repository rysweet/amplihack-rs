//! # amplihack-multilspy
//!
//! Language-agnostic LSP client that wraps multiple language servers.
//! Ported from the Python multilspy library.

pub mod config;
pub mod error;
pub mod language_server;
pub mod lsp_client;
pub mod servers;
pub mod types;

pub use config::{Language, MultilspyConfig, MultilspySettings};
pub use error::MultilspyError;
pub use language_server::{LanguageServer, create_server};
pub use types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, HoverResult, Location,
    Position, Range, SymbolInfo, SymbolKind,
};
