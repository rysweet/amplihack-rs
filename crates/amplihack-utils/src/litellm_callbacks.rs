//! Deprecated compatibility callbacks for the former embedded LiteLLM adapter.
//!
//! New code should use [`crate::trace_logger::TraceLogger`] directly. This
//! module remains available through the 0.18 release line so existing
//! downstream imports continue to compile.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use crate::trace_logger::TraceLogger;

pub const DEFAULT_TRACE_SUBPATH: &str = crate::trace_logger::DEFAULT_TRACE_SUBPATH;

#[derive(Clone)]
pub struct LiteLLMTraceCallback {
    logger: Arc<TraceLogger>,
}

impl LiteLLMTraceCallback {
    pub fn new(logger: TraceLogger) -> Self {
        Self {
            logger: Arc::new(logger),
        }
    }

    pub fn on_llm_start(&self, payload: Option<&Value>) {
        if let Some(data) = payload {
            debug!("on_llm_start");
            self.logger.log(data);
        }
    }

    pub fn on_llm_end(&self, payload: Option<&Value>) {
        if let Some(data) = payload {
            debug!("on_llm_end");
            self.logger.log(data);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.logger.is_enabled()
    }
}

static REGISTRY: Mutex<Vec<LiteLLMTraceCallback>> = Mutex::new(Vec::new());

pub fn register_trace_callbacks(
    enabled: Option<bool>,
    trace_file: Option<&str>,
) -> Option<LiteLLMTraceCallback> {
    let (is_enabled, log_path) = match enabled {
        Some(enabled) => (enabled, trace_file.map(PathBuf::from)),
        None => {
            let env_logger = TraceLogger::from_env();
            (
                env_logger.is_enabled(),
                env_logger.log_file().map(PathBuf::from),
            )
        }
    };

    if !is_enabled {
        return None;
    }

    let callback = LiteLLMTraceCallback::new(TraceLogger::new(true, log_path));
    if let Ok(mut registry) = REGISTRY.lock() {
        registry.push(callback.clone());
    }
    info!("LiteLLM trace callback registered");
    Some(callback)
}

pub fn unregister_trace_callbacks(callback: Option<&LiteLLMTraceCallback>) {
    if let Some(callback) = callback {
        unregister_trace_callback(callback);
    }
}

pub fn unregister_trace_callback(callback: &LiteLLMTraceCallback) {
    let target = Arc::as_ptr(&callback.logger);
    if let Ok(mut registry) = REGISTRY.lock() {
        registry.retain(|existing| Arc::as_ptr(&existing.logger) != target);
    }
    info!("LiteLLM trace callback unregistered");
}

pub fn registered_callback_count() -> usize {
    REGISTRY.lock().map(|registry| registry.len()).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackInfo {
    pub enabled: bool,
    pub trace_file: Option<String>,
}

impl From<&LiteLLMTraceCallback> for CallbackInfo {
    fn from(callback: &LiteLLMTraceCallback) -> Self {
        Self {
            enabled: callback.is_enabled(),
            trace_file: callback
                .logger
                .log_file()
                .map(|path| path.display().to_string()),
        }
    }
}
