//! Minimal LiteLLM tracing callback compatibility surface.
//!
//! Ported from `amplihack/proxy/litellm_callbacks.py`.
//!
//! Since Rust doesn't have LiteLLM's Python runtime, this provides a
//! callback interface that can be wired into any LLM client. The
//! [`LiteLLMTraceCallback`] wraps a [`TraceLogger`] and exposes
//! `on_llm_start` / `on_llm_end` hooks.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use crate::trace_logger::TraceLogger;

/// Default trace-file subpath (re-exported from [`crate::trace_logger`]).
pub const DEFAULT_TRACE_SUBPATH: &str = crate::trace_logger::DEFAULT_TRACE_SUBPATH;

// ---------------------------------------------------------------------------
// Callback
// ---------------------------------------------------------------------------

/// A tracing callback that logs LLM start/end payloads via [`TraceLogger`].
///
/// Thread-safe via internal `Arc<Mutex<..>>`, so it can be shared across
/// callback registrations.
///
/// # Examples
///
/// ```no_run
/// use amplihack_utils::litellm_callbacks::{LiteLLMTraceCallback, register_trace_callbacks};
///
/// if let Some(cb) = register_trace_callbacks(None, None) {
///     cb.on_llm_start(Some(&serde_json::json!({"model": "gpt-4"})));
///     cb.on_llm_end(Some(&serde_json::json!({"usage": {"tokens": 42}})));
/// }
/// ```
#[derive(Clone)]
pub struct LiteLLMTraceCallback {
    logger: Arc<TraceLogger>,
}

impl LiteLLMTraceCallback {
    /// Create a new callback wrapping the given [`TraceLogger`].
    pub fn new(logger: TraceLogger) -> Self {
        Self {
            logger: Arc::new(logger),
        }
    }

    /// Called when an LLM request starts.
    pub fn on_llm_start(&self, payload: Option<&Value>) {
        if let Some(data) = payload {
            debug!("on_llm_start");
            self.logger.log(data);
        }
    }

    /// Called when an LLM request completes.
    pub fn on_llm_end(&self, payload: Option<&Value>) {
        if let Some(data) = payload {
            debug!("on_llm_end");
            self.logger.log(data);
        }
    }

    /// Whether the underlying logger is enabled.
    pub fn is_enabled(&self) -> bool {
        self.logger.is_enabled()
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Global callback registry so that `unregister_trace_callbacks` can remove
/// a previously registered callback without requiring the caller to hold a
/// direct reference to the registration list.
static REGISTRY: Mutex<Vec<LiteLLMTraceCallback>> = Mutex::new(Vec::new());

/// Register a trace callback when tracing is enabled.
///
/// - `enabled`: explicit opt-in. When `None`, the environment is consulted
///   via [`TraceLogger::from_env`].
/// - `trace_file`: explicit log-file path override.
///
/// Returns `Some(callback)` if tracing was enabled and the callback was
/// registered, `None` otherwise.
pub fn register_trace_callbacks(
    enabled: Option<bool>,
    trace_file: Option<&str>,
) -> Option<LiteLLMTraceCallback> {
    let (is_enabled, log_path) = match enabled {
        Some(e) => {
            let path = trace_file.map(PathBuf::from);
            (e, path)
        }
        None => {
            let env_logger = TraceLogger::from_env();
            let is_on = env_logger.is_enabled();
            let path = env_logger.log_file().map(PathBuf::from);
            (is_on, path)
        }
    };

    if !is_enabled {
        return None;
    }

    let logger = TraceLogger::new(true, log_path);
    let callback = LiteLLMTraceCallback::new(logger);

    if let Ok(mut reg) = REGISTRY.lock() {
        reg.push(callback.clone());
    }

    info!("LiteLLM trace callback registered");
    Some(callback)
}

/// Best-effort unregistration of a previously registered callback.
///
/// Removes the callback from the global registry. If the registry mutex is
/// poisoned or the callback is not found, this is a silent no-op.
pub fn unregister_trace_callbacks(callback: Option<&LiteLLMTraceCallback>) {
    let Some(cb) = callback else { return };
    unregister_trace_callback(cb);
}

/// Remove a callback from the global registry by pointer identity on the
/// inner `Arc<TraceLogger>`.
pub fn unregister_trace_callback(callback: &LiteLLMTraceCallback) {
    let target = Arc::as_ptr(&callback.logger);
    if let Ok(mut reg) = REGISTRY.lock() {
        reg.retain(|existing| Arc::as_ptr(&existing.logger) != target);
    }
    info!("LiteLLM trace callback unregistered");
}

/// Return the number of currently registered callbacks.
pub fn registered_callback_count() -> usize {
    REGISTRY.lock().map(|r| r.len()).unwrap_or(0)
}

/// Metadata about a registered callback, for diagnostics / serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackInfo {
    pub enabled: bool,
    pub trace_file: Option<String>,
}

impl From<&LiteLLMTraceCallback> for CallbackInfo {
    fn from(cb: &LiteLLMTraceCallback) -> Self {
        Self {
            enabled: cb.is_enabled(),
            trace_file: cb.logger.log_file().map(|p| p.display().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_logs_start_and_end_without_panic() {
        let logger = TraceLogger::new(false, None);
        let cb = LiteLLMTraceCallback::new(logger);
        // Should be no-ops when disabled.
        cb.on_llm_start(Some(&serde_json::json!({"model": "test"})));
        cb.on_llm_end(Some(&serde_json::json!({"tokens": 10})));
    }

    #[test]
    fn callback_handles_none_payload() {
        let logger = TraceLogger::new(false, None);
        let cb = LiteLLMTraceCallback::new(logger);
        cb.on_llm_start(None);
        cb.on_llm_end(None);
    }

    #[test]
    fn register_returns_none_when_disabled() {
        let result = register_trace_callbacks(Some(false), None);
        assert!(result.is_none());
    }

    #[test]
    fn register_returns_some_when_enabled() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("trace.jsonl");
        let result = register_trace_callbacks(Some(true), Some(path.to_str().unwrap()));
        assert!(result.is_some());
        assert!(result.as_ref().unwrap().is_enabled());
        // Clean up global registry.
        unregister_trace_callback(result.as_ref().unwrap());
    }

    #[test]
    fn unregister_removes_callback() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("unreg.jsonl");
        let before = registered_callback_count();
        // Register and immediately capture the callback.
        let cb = register_trace_callbacks(Some(true), Some(path.to_str().unwrap())).unwrap();
        assert_eq!(registered_callback_count(), before + 1);
        unregister_trace_callback(&cb);
        assert_eq!(registered_callback_count(), before);
    }

    #[test]
    fn callback_info_serialization() {
        let logger = TraceLogger::new(true, Some("/fake/path.jsonl".into()));
        let cb = LiteLLMTraceCallback::new(logger);
        let info = CallbackInfo::from(&cb);
        assert!(info.enabled);
        assert_eq!(info.trace_file.as_deref(), Some("/fake/path.jsonl"));

        let json = serde_json::to_string(&info).unwrap();
        let restored: CallbackInfo = serde_json::from_str(&json).unwrap();
        assert!(restored.enabled);
    }

    #[test]
    fn unregister_noop_when_none() {
        // Should not panic.
        unregister_trace_callbacks(None);
    }
}
