//! rust-analyzer language server integration.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Notify;
use tracing::info;

use crate::config::MultilspyConfig;
use crate::error::MultilspyError;
use crate::language_server::LanguageServer;
use crate::lsp_client::LspClient;
use crate::types::*;

pub struct RustAnalyzerServer {
    config: MultilspyConfig,
    root_path: PathBuf,
    client: Option<LspClient>,
}

impl RustAnalyzerServer {
    pub fn new(config: MultilspyConfig, root_path: PathBuf) -> Self {
        Self {
            config,
            root_path,
            client: None,
        }
    }

    fn client(&self) -> Result<&LspClient> {
        self.client
            .as_ref()
            .ok_or_else(|| MultilspyError::ServerNotStarted.into())
    }

    fn build_initialize_params(&self) -> serde_json::Value {
        let root_uri = path_to_uri(&self.root_path);
        json!({
            "processId": std::process::id(),
            "rootPath": self.root_path.to_string_lossy(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "completion": {
                        "completionItem": {
                            "snippetSupport": false,
                            "labelDetailsSupport": true
                        }
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "references": {},
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "publishDiagnostics": {}
                },
                "workspace": {
                    "workspaceFolders": true
                }
            },
            "workspaceFolders": [{
                "uri": root_uri,
                "name": self.root_path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "workspace".into())
            }]
        })
    }
}

#[async_trait]
impl LanguageServer for RustAnalyzerServer {
    async fn start(&mut self) -> Result<()> {
        if self.client.is_some() {
            return Err(MultilspyError::ServerAlreadyStarted.into());
        }

        let client = LspClient::new(
            "rust-analyzer",
            &[],
            &self.root_path,
            &[],
            self.config.trace_lsp_communication,
        )
        .await?;

        // Register handlers for server-initiated requests/notifications.
        let server_ready = Arc::new(Notify::new());

        let ready_clone = server_ready.clone();
        client
            .on_notification(
                "experimental/serverStatus",
                Box::new(move |params| {
                    if params.get("quiescent") == Some(&serde_json::Value::Bool(true)) {
                        ready_clone.notify_one();
                    }
                }),
            )
            .await;

        client
            .on_request(
                "client/registerCapability",
                Box::new(|_params| serde_json::Value::Null),
            )
            .await;

        client
            .on_request(
                "workspace/executeClientCommand",
                Box::new(|_params| json!([])),
            )
            .await;

        client.on_notification("$/progress", Box::new(|_| {})).await;

        client
            .on_notification("textDocument/publishDiagnostics", Box::new(|_| {}))
            .await;

        client
            .on_notification(
                "window/logMessage",
                Box::new(|params| {
                    if let Some(msg) = params.get("message").and_then(|v| v.as_str()) {
                        info!("rust-analyzer: {msg}");
                    }
                }),
            )
            .await;

        // Initialize handshake.
        let params = self.build_initialize_params();
        let _caps = client.initialize(params).await?;
        client.initialized().await?;

        info!("rust-analyzer initialized, waiting for server ready");

        // Wait for the server to become quiescent (with timeout).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(120), server_ready.notified())
            .await;

        info!("rust-analyzer ready");
        self.client = Some(client);
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(mut client) = self.client.take() {
            client.shutdown().await?;
        }
        Ok(())
    }

    async fn definitions(&self, file: &str, line: u32, col: u32) -> Result<Vec<Location>> {
        self.client()?.definitions(file, line, col).await
    }

    async fn references(&self, file: &str, line: u32, col: u32) -> Result<Vec<Location>> {
        self.client()?.references(file, line, col).await
    }

    async fn hover(&self, file: &str, line: u32, col: u32) -> Result<Option<HoverResult>> {
        self.client()?.hover(file, line, col).await
    }

    async fn completions(&self, file: &str, line: u32, col: u32) -> Result<Vec<CompletionItem>> {
        self.client()?.completions(file, line, col).await
    }

    async fn diagnostics(&self, file: &str) -> Result<Vec<Diagnostic>> {
        self.client()?.diagnostics(file).await
    }

    async fn document_symbols(&self, file: &str) -> Result<Vec<SymbolInfo>> {
        self.client()?.document_symbols(file).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_server_not_started() {
        let server = RustAnalyzerServer::new(
            MultilspyConfig::new(crate::config::Language::Rust),
            PathBuf::from("/tmp/test"),
        );
        assert!(server.client.is_none());
    }

    #[test]
    fn client_returns_error_when_not_started() {
        let server = RustAnalyzerServer::new(
            MultilspyConfig::new(crate::config::Language::Rust),
            PathBuf::from("/tmp/test"),
        );
        assert!(server.client().is_err());
    }

    #[test]
    fn initialize_params_structure() {
        let server = RustAnalyzerServer::new(
            MultilspyConfig::new(crate::config::Language::Rust),
            PathBuf::from("/home/user/project"),
        );
        let params = server.build_initialize_params();
        assert!(params.get("processId").is_some());
        assert_eq!(
            params["rootUri"].as_str().unwrap(),
            "file:///home/user/project"
        );
        let folders = params["workspaceFolders"].as_array().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0]["name"].as_str().unwrap(), "project");
    }

    #[test]
    fn initialize_params_has_capabilities() {
        let server = RustAnalyzerServer::new(
            MultilspyConfig::new(crate::config::Language::Rust),
            PathBuf::from("/project"),
        );
        let params = server.build_initialize_params();
        let caps = &params["capabilities"];
        assert!(caps.get("textDocument").is_some());
        assert!(caps["textDocument"].get("completion").is_some());
        assert!(caps["textDocument"].get("hover").is_some());
        assert!(caps["textDocument"].get("definition").is_some());
    }
}
