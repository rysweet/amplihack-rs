//! Jedi language server (Python) integration.

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tracing::info;

use crate::config::MultilspyConfig;
use crate::error::MultilspyError;
use crate::language_server::LanguageServer;
use crate::lsp_client::LspClient;
use crate::types::*;

pub struct JediServer {
    config: MultilspyConfig,
    root_path: PathBuf,
    client: Option<LspClient>,
}

impl JediServer {
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
                        "completionItem": { "snippetSupport": false }
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
impl LanguageServer for JediServer {
    async fn start(&mut self) -> Result<()> {
        if self.client.is_some() {
            return Err(MultilspyError::ServerAlreadyStarted.into());
        }

        let client = LspClient::new(
            "jedi-language-server",
            &[],
            &self.root_path,
            &[],
            self.config.trace_lsp_communication,
        )
        .await?;

        client
            .on_request(
                "client/registerCapability",
                Box::new(|_| serde_json::Value::Null),
            )
            .await;

        client
            .on_request("workspace/executeClientCommand", Box::new(|_| json!([])))
            .await;

        client.on_notification("$/progress", Box::new(|_| {})).await;

        client
            .on_notification("textDocument/publishDiagnostics", Box::new(|_| {}))
            .await;

        client
            .on_notification("language/status", Box::new(|_| {}))
            .await;

        client
            .on_notification("language/actionableNotification", Box::new(|_| {}))
            .await;

        client
            .on_notification(
                "window/logMessage",
                Box::new(|params| {
                    if let Some(msg) = params.get("message").and_then(|v| v.as_str()) {
                        info!("jedi-language-server: {msg}");
                    }
                }),
            )
            .await;

        let params = self.build_initialize_params();
        let _caps = client.initialize(params).await?;
        client.initialized().await?;

        info!("jedi-language-server ready");
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
    use crate::config::Language;

    #[test]
    fn new_server() {
        let server = JediServer::new(
            MultilspyConfig::new(Language::Python),
            PathBuf::from("/tmp/py-project"),
        );
        assert!(server.client.is_none());
    }

    #[test]
    fn client_not_started() {
        let server = JediServer::new(
            MultilspyConfig::new(Language::Python),
            PathBuf::from("/tmp/py-project"),
        );
        assert!(server.client().is_err());
    }

    #[test]
    fn initialize_params() {
        let server = JediServer::new(
            MultilspyConfig::new(Language::Python),
            PathBuf::from("/home/user/py-app"),
        );
        let params = server.build_initialize_params();
        assert!(params.get("processId").is_some());
        assert_eq!(
            params["rootUri"].as_str().unwrap(),
            "file:///home/user/py-app"
        );
    }

    #[test]
    fn initialize_params_workspace_name() {
        let server = JediServer::new(
            MultilspyConfig::new(Language::Python),
            PathBuf::from("/projects/my-lib"),
        );
        let params = server.build_initialize_params();
        let folders = params["workspaceFolders"].as_array().unwrap();
        assert_eq!(folders[0]["name"].as_str().unwrap(), "my-lib");
    }
}
