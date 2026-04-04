//! `LanguageServer` trait and factory function.

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::{Language, MultilspyConfig};
use crate::servers;
use crate::types::{CompletionItem, Diagnostic, HoverResult, Location, SymbolInfo};

/// Trait implemented by every language server backend.
///
/// Lifecycle: call [`start`](LanguageServer::start) before any query methods,
/// and [`shutdown`](LanguageServer::shutdown) when done.
#[async_trait]
pub trait LanguageServer: Send + Sync {
    /// Spawns the language server process, performs initialization handshake.
    async fn start(&mut self) -> Result<()>;

    /// Gracefully shuts down the language server.
    async fn shutdown(&mut self) -> Result<()>;

    /// Returns go-to-definition locations for the given position.
    async fn definitions(&self, file: &str, line: u32, col: u32) -> Result<Vec<Location>>;

    /// Returns find-references locations for the given position.
    async fn references(&self, file: &str, line: u32, col: u32) -> Result<Vec<Location>>;

    /// Returns hover information for the given position.
    async fn hover(&self, file: &str, line: u32, col: u32) -> Result<Option<HoverResult>>;

    /// Returns completions at the given position.
    async fn completions(&self, file: &str, line: u32, col: u32) -> Result<Vec<CompletionItem>>;

    /// Returns diagnostics for a file.
    async fn diagnostics(&self, file: &str) -> Result<Vec<Diagnostic>>;

    /// Returns document symbols for a file.
    async fn document_symbols(&self, file: &str) -> Result<Vec<SymbolInfo>>;
}

/// Creates a language server instance for the given configuration.
///
/// The returned server is not yet started — call [`LanguageServer::start`] first.
pub fn create_server(
    config: &MultilspyConfig,
    root_path: &Path,
) -> Result<Box<dyn LanguageServer>> {
    let root = root_path.to_path_buf();
    let cfg = config.clone();

    match config.code_language {
        Language::Rust => Ok(Box::new(servers::rust_analyzer::RustAnalyzerServer::new(
            cfg, root,
        ))),
        Language::TypeScript | Language::JavaScript => Ok(Box::new(
            servers::typescript::TypeScriptServer::new(cfg, root),
        )),
        Language::Go => Ok(Box::new(servers::gopls::GoplsServer::new(cfg, root))),
        Language::Python => Ok(Box::new(servers::jedi::JediServer::new(cfg, root))),
        Language::CSharp => Ok(Box::new(servers::omnisharp::OmniSharpServer::new(
            cfg, root,
        ))),
        Language::Java => Ok(Box::new(servers::eclipse_jdtls::EclipseJdtlsServer::new(
            cfg, root,
        ))),
        Language::Php => Ok(Box::new(servers::intelephense::IntelephenseServer::new(
            cfg, root,
        ))),
        Language::Ruby => Ok(Box::new(servers::solargraph::SolargraphServer::new(
            cfg, root,
        ))),
        Language::Dart => Err(anyhow::anyhow!(
            crate::error::MultilspyError::UnsupportedLanguage("dart".into())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn factory_rust() {
        let cfg = MultilspyConfig::new(Language::Rust);
        let server = create_server(&cfg, &PathBuf::from("/tmp/test"));
        assert!(server.is_ok());
    }

    #[test]
    fn factory_typescript() {
        let cfg = MultilspyConfig::new(Language::TypeScript);
        let server = create_server(&cfg, &PathBuf::from("/tmp/test"));
        assert!(server.is_ok());
    }

    #[test]
    fn factory_javascript_uses_typescript_server() {
        let cfg = MultilspyConfig::new(Language::JavaScript);
        let server = create_server(&cfg, &PathBuf::from("/tmp/test"));
        assert!(server.is_ok());
    }

    #[test]
    fn factory_all_supported() {
        let languages = [
            Language::Rust,
            Language::Python,
            Language::TypeScript,
            Language::JavaScript,
            Language::Go,
            Language::CSharp,
            Language::Java,
            Language::Php,
            Language::Ruby,
        ];
        for lang in &languages {
            let cfg = MultilspyConfig::new(*lang);
            assert!(
                create_server(&cfg, &PathBuf::from("/tmp/test")).is_ok(),
                "failed for {lang}"
            );
        }
    }

    #[test]
    fn factory_unsupported() {
        let cfg = MultilspyConfig::new(Language::Dart);
        assert!(create_server(&cfg, &PathBuf::from("/tmp/test")).is_err());
    }
}
