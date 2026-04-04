//! JSON-RPC client for LSP communication over stdin/stdout.
//!
//! Manages child process lifecycle, message framing, request/response
//! correlation, and notification dispatch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, trace, warn};

use crate::error::MultilspyError;
use crate::types::{
    self, CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, HoverResult,
    Location, Position, Range, SymbolInfo, SymbolKind,
};

/// Handler for server-initiated requests (must return a response value).
pub type RequestHandler = Box<dyn Fn(Value) -> Value + Send + Sync>;

/// Handler for server-initiated notifications (fire-and-forget).
pub type NotificationHandler = Box<dyn Fn(Value) + Send + Sync>;

/// Tracks an open file buffer.
struct FileBuffer {
    uri: String,
    #[allow(dead_code)]
    content: String,
    #[allow(dead_code)]
    version: i32,
    #[allow(dead_code)]
    language_id: String,
}

/// Type alias for pending request response senders.
type PendingMap = HashMap<i64, oneshot::Sender<Result<Value, MultilspyError>>>;

/// An LSP client that manages a language server child process and communicates
/// over JSON-RPC using stdin/stdout.
pub struct LspClient {
    stdin: Arc<Mutex<ChildStdin>>,
    process: Arc<Mutex<Child>>,
    reader_handle: Option<JoinHandle<()>>,
    stderr_handle: Option<JoinHandle<()>>,
    pending: Arc<Mutex<PendingMap>>,
    next_id: Arc<AtomicI64>,
    request_handlers: Arc<Mutex<HashMap<String, RequestHandler>>>,
    notification_handlers: Arc<Mutex<HashMap<String, NotificationHandler>>>,
    open_files: Arc<Mutex<HashMap<String, FileBuffer>>>,
    root_path: PathBuf,
    trace: bool,
}

impl LspClient {
    /// Spawns the language server process and starts background reader tasks.
    pub async fn new(
        cmd: &str,
        args: &[&str],
        root_path: &Path,
        env: &[(String, String)],
        trace: bool,
    ) -> Result<Self> {
        let mut command = Command::new(cmd);
        command
            .args(args)
            .current_dir(root_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in env {
            command.env(k, v);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn language server: {cmd}"))?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));
        let request_handlers: Arc<Mutex<HashMap<String, RequestHandler>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let notification_handlers: Arc<Mutex<HashMap<String, NotificationHandler>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(stdin));

        let reader_handle = {
            let pending = pending.clone();
            let req_h = request_handlers.clone();
            let notif_h = notification_handlers.clone();
            let stdin_w = stdin.clone();
            tokio::spawn(async move {
                Self::read_loop(stdout, pending, req_h, notif_h, stdin_w, trace).await;
            })
        };

        let stderr_handle = tokio::spawn(async move {
            Self::stderr_loop(stderr).await;
        });

        Ok(Self {
            stdin,
            process: Arc::new(Mutex::new(child)),
            reader_handle: Some(reader_handle),
            stderr_handle: Some(stderr_handle),
            pending,
            next_id: Arc::new(AtomicI64::new(1)),
            request_handlers,
            notification_handlers,
            open_files: Arc::new(Mutex::new(HashMap::new())),
            root_path: root_path.to_path_buf(),
            trace,
        })
    }

    // ── Handler registration ─────────────────────────────────────────

    /// Registers a handler for server-initiated requests (expects a response).
    pub async fn on_request(&self, method: &str, handler: RequestHandler) {
        self.request_handlers
            .lock()
            .await
            .insert(method.to_string(), handler);
    }

    /// Registers a handler for server-initiated notifications.
    pub async fn on_notification(&self, method: &str, handler: NotificationHandler) {
        self.notification_handlers
            .lock()
            .await
            .insert(method.to_string(), handler);
    }

    // ── Low-level JSON-RPC ───────────────────────────────────────────

    /// Sends a JSON-RPC request and waits for the response.
    pub async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, MultilspyError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.pending.lock().await.insert(id, tx);

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });

        if self.trace {
            debug!("LSP send request: {method} (id={id})");
        }

        self.write_message(&msg).await?;

        rx.await
            .map_err(|_| MultilspyError::Other("response channel closed".into()))?
    }

    /// Sends a JSON-RPC notification (no response expected).
    pub async fn send_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), MultilspyError> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });

        if self.trace {
            debug!("LSP send notification: {method}");
        }

        self.write_message(&msg).await
    }

    /// Encodes and writes a JSON-RPC message to the server's stdin.
    async fn write_message(&self, msg: &Value) -> Result<(), MultilspyError> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(body.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    // ── High-level LSP methods ───────────────────────────────────────

    /// Sends `initialize` with the given params and returns server capabilities.
    pub async fn initialize(&self, params: Value) -> Result<Value> {
        let result = self.send_request("initialize", Some(params)).await?;
        Ok(result)
    }

    /// Sends the `initialized` notification.
    pub async fn initialized(&self) -> Result<()> {
        self.send_notification("initialized", Some(serde_json::json!({})))
            .await?;
        Ok(())
    }

    /// Sends `shutdown` request followed by `exit` notification, then kills process.
    pub async fn shutdown(&mut self) -> Result<()> {
        // Best-effort shutdown sequence.
        let _ = self.send_request("shutdown", None).await;
        let _ = self.send_notification("exit", None).await;

        // Cancel reader tasks.
        if let Some(h) = self.reader_handle.take() {
            h.abort();
        }
        if let Some(h) = self.stderr_handle.take() {
            h.abort();
        }

        // Kill the process.
        let mut proc = self.process.lock().await;
        let _ = proc.kill().await;

        Ok(())
    }

    /// Ensures a file is open on the server side.
    pub async fn ensure_open(&self, relative_path: &str) -> Result<String> {
        let files = self.open_files.lock().await;
        if let Some(fb) = files.get(relative_path) {
            return Ok(fb.uri.clone());
        }
        drop(files);

        let abs_path = self.root_path.join(relative_path);
        let content = tokio::fs::read_to_string(&abs_path)
            .await
            .map_err(|_| MultilspyError::FileNotFound(relative_path.to_string()))?;
        let uri = types::path_to_uri(&abs_path);
        let language_id = types::detect_language_id(relative_path);

        let params = serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": content
            }
        });

        self.send_notification("textDocument/didOpen", Some(params))
            .await?;

        let mut files = self.open_files.lock().await;
        files.insert(
            relative_path.to_string(),
            FileBuffer {
                uri: uri.clone(),
                content,
                version: 1,
                language_id: language_id.to_string(),
            },
        );

        Ok(uri)
    }

    /// Requests go-to-definition results.
    pub async fn definitions(
        &self,
        relative_path: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<Location>> {
        let uri = self.ensure_open(relative_path).await?;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col }
        });
        let result = self
            .send_request("textDocument/definition", Some(params))
            .await?;
        Ok(parse_locations(&result, &self.root_path))
    }

    /// Requests find-references results.
    pub async fn references(
        &self,
        relative_path: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<Location>> {
        let uri = self.ensure_open(relative_path).await?;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col },
            "context": { "includeDeclaration": true }
        });
        let result = self
            .send_request("textDocument/references", Some(params))
            .await?;
        Ok(parse_locations(&result, &self.root_path))
    }

    /// Requests hover information.
    pub async fn hover(
        &self,
        relative_path: &str,
        line: u32,
        col: u32,
    ) -> Result<Option<HoverResult>> {
        let uri = self.ensure_open(relative_path).await?;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col }
        });
        let result = self
            .send_request("textDocument/hover", Some(params))
            .await?;
        Ok(parse_hover(&result))
    }

    /// Requests completions at a position.
    pub async fn completions(
        &self,
        relative_path: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<CompletionItem>> {
        let uri = self.ensure_open(relative_path).await?;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col }
        });
        let result = self
            .send_request("textDocument/completion", Some(params))
            .await?;
        Ok(parse_completions(&result))
    }

    /// Requests diagnostics for a file.
    pub async fn diagnostics(&self, relative_path: &str) -> Result<Vec<Diagnostic>> {
        let uri = self.ensure_open(relative_path).await?;
        let params = serde_json::json!({
            "textDocument": { "uri": uri }
        });
        // Try the pull-based diagnostic request; fall back to empty if unsupported.
        match self
            .send_request("textDocument/diagnostic", Some(params))
            .await
        {
            Ok(result) => Ok(parse_diagnostics(&result)),
            Err(MultilspyError::JsonRpcError { code: -32601, .. }) => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    /// Requests document symbols.
    pub async fn document_symbols(&self, relative_path: &str) -> Result<Vec<SymbolInfo>> {
        let uri = self.ensure_open(relative_path).await?;
        let params = serde_json::json!({
            "textDocument": { "uri": uri }
        });
        let result = self
            .send_request("textDocument/documentSymbol", Some(params))
            .await?;
        Ok(parse_symbols(&result))
    }

    // ── Background reader tasks ──────────────────────────────────────

    async fn read_loop(
        stdout: ChildStdout,
        pending: Arc<Mutex<PendingMap>>,
        request_handlers: Arc<Mutex<HashMap<String, RequestHandler>>>,
        notification_handlers: Arc<Mutex<HashMap<String, NotificationHandler>>>,
        stdin: Arc<Mutex<ChildStdin>>,
        trace_enabled: bool,
    ) {
        let mut reader = BufReader::new(stdout);

        loop {
            // Read headers until blank line.
            let mut content_length: Option<usize> = None;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => return,
                    Err(_) => return,
                    _ => {}
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some(len_str) = trimmed.strip_prefix("Content-Length:")
                    && let Ok(len) = len_str.trim().parse::<usize>()
                {
                    content_length = Some(len);
                }
            }

            let Some(content_length) = content_length else {
                continue;
            };

            // Read body.
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).await.is_err() {
                return;
            }

            let payload: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    error!("failed to parse JSON-RPC message: {e}");
                    continue;
                }
            };

            if trace_enabled {
                trace!("LSP recv: {payload}");
            }

            let has_method = payload.get("method").and_then(|v| v.as_str()).is_some();
            let has_id = payload.get("id").is_some();

            if has_id && !has_method {
                // Response to our request.
                if let Some(id) = payload["id"].as_i64() {
                    let mut pending = pending.lock().await;
                    if let Some(tx) = pending.remove(&id) {
                        if let Some(err_obj) = payload.get("error") {
                            let code = err_obj["code"].as_i64().unwrap_or(-1);
                            let message =
                                err_obj["message"].as_str().unwrap_or("unknown").to_string();
                            let _ = tx.send(Err(MultilspyError::JsonRpcError { code, message }));
                        } else {
                            let result = payload.get("result").cloned().unwrap_or(Value::Null);
                            let _ = tx.send(Ok(result));
                        }
                    }
                }
            } else if has_method && has_id {
                // Server-initiated request — respond.
                let method = payload["method"].as_str().unwrap_or("").to_string();
                let params = payload.get("params").cloned().unwrap_or(Value::Null);
                let id = payload["id"].clone();

                let result = {
                    let handlers = request_handlers.lock().await;
                    if let Some(handler) = handlers.get(&method) {
                        handler(params)
                    } else {
                        warn!("unhandled server request: {method}");
                        Value::Null
                    }
                };

                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });

                let body_bytes = serde_json::to_string(&response).unwrap_or_default();
                let header = format!("Content-Length: {}\r\n\r\n", body_bytes.len());
                let mut w = stdin.lock().await;
                let _ = w.write_all(header.as_bytes()).await;
                let _ = w.write_all(body_bytes.as_bytes()).await;
                let _ = w.flush().await;
            } else if has_method {
                // Server notification.
                let method = payload["method"].as_str().unwrap_or("").to_string();
                let params = payload.get("params").cloned().unwrap_or(Value::Null);

                let handlers = notification_handlers.lock().await;
                if let Some(handler) = handlers.get(&method) {
                    handler(params);
                }
            }
        }
    }

    async fn stderr_loop(stderr: tokio::process::ChildStderr) {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                _ => {}
            }
            debug!("LSP stderr: {}", line.trim_end());
        }
    }
}

// ── Response parsers ─────────────────────────────────────────────────

/// Parses locations from a definition/references response.
/// Handles `Location`, `Location[]`, `LocationLink[]`, or null.
fn parse_locations(value: &Value, root_path: &Path) -> Vec<Location> {
    if value.is_null() {
        return vec![];
    }

    let raw_locations: Vec<Value> = if value.is_array() {
        value.as_array().cloned().unwrap_or_default()
    } else if value.is_object() {
        vec![value.clone()]
    } else {
        return vec![];
    };

    raw_locations
        .iter()
        .filter_map(|v| convert_raw_location(v, root_path))
        .collect()
}

/// Converts a single raw LSP Location or LocationLink to our Location type.
fn convert_raw_location(value: &Value, root_path: &Path) -> Option<Location> {
    // LocationLink format (targetUri + targetRange).
    if let Some(target_uri) = value.get("targetUri").and_then(|v| v.as_str()) {
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("targetRange"))
            .and_then(parse_range)
            .unwrap_or_else(zero_range);

        return Some(build_location(target_uri, range, root_path));
    }

    // Standard Location format (uri + range).
    let uri = value.get("uri").and_then(|v| v.as_str())?;
    let range = value
        .get("range")
        .and_then(parse_range)
        .unwrap_or_else(zero_range);
    Some(build_location(uri, range, root_path))
}

fn build_location(uri: &str, range: Range, root_path: &Path) -> Location {
    let absolute_path = types::uri_to_path(uri);
    let relative_path = absolute_path.as_ref().and_then(|abs| {
        Path::new(abs)
            .strip_prefix(root_path)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    });

    Location {
        uri: uri.to_string(),
        range,
        absolute_path,
        relative_path,
    }
}

fn parse_range(value: &Value) -> Option<Range> {
    let start = value.get("start")?;
    let end = value.get("end")?;
    Some(Range {
        start: Position {
            line: start.get("line")?.as_u64()? as u32,
            character: start.get("character")?.as_u64()? as u32,
        },
        end: Position {
            line: end.get("line")?.as_u64()? as u32,
            character: end.get("character")?.as_u64()? as u32,
        },
    })
}

fn zero_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

fn parse_hover(value: &Value) -> Option<HoverResult> {
    if value.is_null() {
        return None;
    }

    let contents = value.get("contents")?;
    let text = extract_hover_text(contents);
    if text.is_empty() {
        return None;
    }

    let range = value.get("range").and_then(parse_range);
    Some(HoverResult {
        contents: text,
        range,
    })
}

fn extract_hover_text(contents: &Value) -> String {
    // MarkupContent { kind, value }
    if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
        return value.to_string();
    }
    // Plain string
    if let Some(s) = contents.as_str() {
        return s.to_string();
    }
    // MarkedString { language, value }
    if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
        return value.to_string();
    }
    // Array of MarkedString
    if let Some(arr) = contents.as_array() {
        let parts: Vec<String> = arr.iter().map(extract_hover_text).collect();
        return parts.join("\n\n");
    }
    String::new()
}

fn parse_completions(value: &Value) -> Vec<CompletionItem> {
    let items = if let Some(arr) = value.as_array() {
        arr.clone()
    } else if let Some(obj) = value.as_object() {
        // CompletionList { items: [...] }
        obj.get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    } else {
        return vec![];
    };

    items
        .iter()
        .filter_map(|item| {
            let kind_val = item.get("kind").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            // Skip keyword completions (kind == 14)
            if kind_val == 14 {
                return None;
            }

            let completion_text = item
                .get("textEdit")
                .and_then(|te| te.get("newText"))
                .and_then(|v| v.as_str())
                .or_else(|| item.get("insertText").and_then(|v| v.as_str()))
                .or_else(|| item.get("label").and_then(|v| v.as_str()))?;

            let detail = item
                .get("detail")
                .and_then(|v| v.as_str())
                .map(String::from);

            Some(CompletionItem {
                completion_text: completion_text.to_string(),
                kind: CompletionItemKind(kind_val),
                detail,
            })
        })
        .collect()
}

fn parse_diagnostics(value: &Value) -> Vec<Diagnostic> {
    let items = value
        .get("items")
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());

    let Some(items) = items else {
        return vec![];
    };

    items
        .iter()
        .filter_map(|item| {
            let range = item.get("range").and_then(parse_range)?;
            let message = item.get("message").and_then(|v| v.as_str())?.to_string();
            let severity = item
                .get("severity")
                .and_then(|v| v.as_u64())
                .map(|s| DiagnosticSeverity(s as u32));
            let source = item
                .get("source")
                .and_then(|v| v.as_str())
                .map(String::from);
            let code = item.get("code").map(|v| {
                v.as_str()
                    .map(String::from)
                    .unwrap_or_else(|| v.to_string())
            });

            Some(Diagnostic {
                range,
                message,
                severity,
                source,
                code,
            })
        })
        .collect()
}

fn parse_symbols(value: &Value) -> Vec<SymbolInfo> {
    let items = value.as_array();
    let Some(items) = items else {
        return vec![];
    };

    let mut result = Vec::new();
    for item in items {
        flatten_symbol(item, None, &mut result);
    }
    result
}

fn flatten_symbol(value: &Value, container: Option<&str>, out: &mut Vec<SymbolInfo>) {
    let name = match value.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return,
    };
    let kind_val = value.get("kind").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

    let location = value.get("location").and_then(|loc| {
        let uri = loc.get("uri").and_then(|v| v.as_str())?.to_string();
        let range = loc
            .get("range")
            .and_then(parse_range)
            .unwrap_or_else(zero_range);
        Some(Location {
            uri,
            range,
            absolute_path: None,
            relative_path: None,
        })
    });

    let range = value.get("range").and_then(parse_range);
    let selection_range = value.get("selectionRange").and_then(parse_range);
    let detail = value
        .get("detail")
        .and_then(|v| v.as_str())
        .map(String::from);
    let container_name = container.map(String::from).or_else(|| {
        value
            .get("containerName")
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    let deprecated = value
        .get("deprecated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    out.push(SymbolInfo {
        name: name.clone(),
        kind: SymbolKind(kind_val),
        location,
        container_name,
        detail,
        range,
        selection_range,
        deprecated,
    });

    // Recurse into DocumentSymbol children.
    if let Some(children) = value.get("children").and_then(|v| v.as_array()) {
        for child in children {
            flatten_symbol(child, Some(&name), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_single_location() {
        let val = json!({
            "uri": "file:///project/src/main.rs",
            "range": {
                "start": { "line": 10, "character": 4 },
                "end": { "line": 10, "character": 12 }
            }
        });
        let locs = parse_locations(&val, Path::new("/project"));
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].range.start.line, 10);
        assert_eq!(locs[0].range.start.character, 4);
        assert_eq!(locs[0].relative_path.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn parse_location_array() {
        let val = json!([
            {
                "uri": "file:///a/b.rs",
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } }
            },
            {
                "uri": "file:///a/c.rs",
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 3 } }
            }
        ]);
        let locs = parse_locations(&val, Path::new("/a"));
        assert_eq!(locs.len(), 2);
    }

    #[test]
    fn parse_location_link() {
        let val = json!([{
            "originSelectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } },
            "targetUri": "file:///project/lib.rs",
            "targetRange": { "start": { "line": 5, "character": 0 }, "end": { "line": 5, "character": 10 } },
            "targetSelectionRange": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 8 } }
        }]);
        let locs = parse_locations(&val, Path::new("/project"));
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, "file:///project/lib.rs");
        assert_eq!(locs[0].range.start.line, 5);
        assert_eq!(locs[0].range.start.character, 4);
    }

    #[test]
    fn parse_null_locations() {
        let locs = parse_locations(&Value::Null, Path::new("/x"));
        assert!(locs.is_empty());
    }

    #[test]
    fn parse_hover_markup_content() {
        let val = json!({
            "contents": { "kind": "markdown", "value": "```rust\nfn main()\n```" },
            "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } }
        });
        let hover = parse_hover(&val).unwrap();
        assert!(hover.contents.contains("fn main()"));
        assert!(hover.range.is_some());
    }

    #[test]
    fn parse_hover_plain_string() {
        let val = json!({ "contents": "hello world" });
        let hover = parse_hover(&val).unwrap();
        assert_eq!(hover.contents, "hello world");
    }

    #[test]
    fn parse_hover_null() {
        assert!(parse_hover(&Value::Null).is_none());
    }

    #[test]
    fn parse_completion_list() {
        let val = json!({
            "isIncomplete": false,
            "items": [
                { "label": "println", "kind": 3, "detail": "macro" },
                { "label": "if", "kind": 14 },
                { "label": "format", "kind": 3, "insertText": "format!" }
            ]
        });
        let items = parse_completions(&val);
        // keyword (kind 14) is filtered out
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].completion_text, "println");
        assert_eq!(items[0].kind, CompletionItemKind::FUNCTION);
        assert_eq!(items[1].completion_text, "format!");
    }

    #[test]
    fn parse_completion_array() {
        let val = json!([
            { "label": "foo", "kind": 6 }
        ]);
        let items = parse_completions(&val);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, CompletionItemKind::VARIABLE);
    }

    #[test]
    fn parse_diagnostics_basic() {
        let val = json!({
            "items": [
                {
                    "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 10 } },
                    "message": "unused variable",
                    "severity": 2,
                    "source": "rustc",
                    "code": "dead_code"
                }
            ]
        });
        let diags = parse_diagnostics(&val);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "unused variable");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn parse_symbols_flat() {
        let val = json!([
            {
                "name": "main",
                "kind": 12,
                "location": {
                    "uri": "file:///a.rs",
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } }
                }
            }
        ]);
        let syms = parse_symbols(&val);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "main");
        assert_eq!(syms[0].kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn parse_symbols_nested() {
        let val = json!([
            {
                "name": "Foo",
                "kind": 5,
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 10, "character": 1 } },
                "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 10 } },
                "children": [
                    {
                        "name": "bar",
                        "kind": 6,
                        "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 3, "character": 5 } },
                        "selectionRange": { "start": { "line": 1, "character": 7 }, "end": { "line": 1, "character": 10 } }
                    }
                ]
            }
        ]);
        let syms = parse_symbols(&val);
        assert_eq!(syms.len(), 2);
        assert_eq!(syms[0].name, "Foo");
        assert_eq!(syms[1].name, "bar");
        assert_eq!(syms[1].container_name.as_deref(), Some("Foo"));
    }
}
