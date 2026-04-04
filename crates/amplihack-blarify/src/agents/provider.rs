//! LLM provider abstraction and API key management.
//!
//! Mirrors the Python `agents/llm_provider.py` and `agents/api_key_manager.py`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Reasoning effort level for model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

/// Known LLM model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProvider {
    OpenAi,
    Anthropic,
    Google,
}

impl ModelProvider {
    /// Detect provider from a model name.
    pub fn from_model_name(model: &str) -> Option<Self> {
        if model.starts_with("gpt-") || model.starts_with("o3") || model.starts_with("o4") {
            Some(Self::OpenAi)
        } else if model.starts_with("claude-") {
            Some(Self::Anthropic)
        } else if model.starts_with("gemini-") {
            Some(Self::Google)
        } else {
            None
        }
    }
}

/// API key status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatus {
    Available,
    RateLimited,
    QuotaExceeded,
    Invalid,
}

/// State of an individual API key.
#[derive(Debug, Clone)]
pub struct KeyState {
    pub key: String,
    pub status: KeyStatus,
    pub cooldown_until: Option<Instant>,
    pub last_used: Option<Instant>,
    pub error_count: u32,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl KeyState {
    /// Create a new key in available state.
    pub fn new(key: String) -> Self {
        Self {
            key,
            status: KeyStatus::Available,
            cooldown_until: None,
            last_used: None,
            error_count: 0,
            metadata: HashMap::new(),
        }
    }

    /// Check if this key is available for use.
    pub fn is_available(&self) -> bool {
        if self.status != KeyStatus::Available {
            return false;
        }
        if let Some(until) = self.cooldown_until
            && Instant::now() < until
        {
            return false;
        }
        true
    }
}

/// Configuration for the API key manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyManagerConfig {
    pub auto_discover: bool,
    pub validate_keys: bool,
    pub max_error_count: u32,
    pub default_cooldown_seconds: u64,
}

impl Default for KeyManagerConfig {
    fn default() -> Self {
        Self {
            auto_discover: true,
            validate_keys: true,
            max_error_count: 3,
            default_cooldown_seconds: 60,
        }
    }
}

/// Statistics for API key usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyStatistics {
    pub total_keys: usize,
    pub available_keys: usize,
    pub rate_limited_keys: usize,
    pub invalid_keys: usize,
    pub quota_exceeded_keys: usize,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
}

/// Thread-safe API key manager with round-robin rotation.
#[derive(Debug)]
pub struct ApiKeyManager {
    provider: ModelProvider,
    config: KeyManagerConfig,
    inner: Arc<Mutex<ApiKeyManagerInner>>,
}

#[derive(Debug)]
struct ApiKeyManagerInner {
    keys: HashMap<String, KeyState>,
    key_order: Vec<String>,
    current_index: usize,
}

impl ApiKeyManager {
    /// Create a new API key manager for a provider.
    pub fn new(provider: ModelProvider, config: Option<KeyManagerConfig>) -> Self {
        let config = config.unwrap_or_default();
        let mgr = Self {
            provider,
            config,
            inner: Arc::new(Mutex::new(ApiKeyManagerInner {
                keys: HashMap::new(),
                key_order: Vec::new(),
                current_index: 0,
            })),
        };
        debug!(provider = ?mgr.provider, "Initialized ApiKeyManager");
        mgr
    }

    /// Add a key to the manager.
    pub fn add_key(&self, key: &str) -> bool {
        if self.config.validate_keys && !validate_key(key, self.provider) {
            warn!(
                provider = ?self.provider,
                key_prefix = &key[..key.len().min(10)],
                "Invalid key format"
            );
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.keys.contains_key(key) {
            return false;
        }
        inner
            .keys
            .insert(key.to_string(), KeyState::new(key.to_string()));
        inner.key_order.push(key.to_string());
        debug!(
            provider = ?self.provider,
            key_prefix = &key[..key.len().min(8)],
            "Added API key"
        );
        true
    }

    /// Get next available key using round-robin selection.
    pub fn get_next_available_key(&self) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        Self::reset_expired_cooldowns_inner(&mut inner);

        if inner.key_order.is_empty() {
            return None;
        }

        // Try to find an immediately available key
        for _ in 0..inner.key_order.len() {
            let idx = inner.current_index;
            inner.current_index = (inner.current_index + 1) % inner.key_order.len();
            let key = inner.key_order[idx].clone();

            if inner.keys[&key].is_available() {
                let state = inner.keys.get_mut(&key).unwrap();
                state.last_used = Some(Instant::now());
                debug!(
                    provider = ?self.provider,
                    key_prefix = &key[..key.len().min(8)],
                    "Selected key"
                );
                return Some(key);
            }
        }

        // Return the rate-limited key that will be available soonest
        let mut best_key = None;
        let mut earliest = None;
        for (key, state) in &inner.keys {
            if state.status == KeyStatus::RateLimited
                && let Some(until) = state.cooldown_until
                && (earliest.is_none() || until < earliest.unwrap())
            {
                earliest = Some(until);
                best_key = Some(key.clone());
            }
        }

        if let Some(ref key) = best_key {
            warn!(
                provider = ?self.provider,
                "All keys rate limited, returning soonest-available"
            );
            return Some(key.clone());
        }

        warn!(
            provider = ?self.provider,
            "No usable API keys (all invalid or quota exceeded)"
        );
        None
    }

    /// Mark a key as rate limited.
    pub fn mark_rate_limited(&self, key: &str, retry_after_secs: Option<u64>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(state) = inner.keys.get_mut(key) {
            state.status = KeyStatus::RateLimited;
            if let Some(secs) = retry_after_secs {
                state.cooldown_until = Some(Instant::now() + Duration::from_secs(secs));
            }
        }
    }

    /// Mark a key as permanently invalid.
    pub fn mark_invalid(&self, key: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(state) = inner.keys.get_mut(key) {
            state.status = KeyStatus::Invalid;
            state.error_count += 1;
        }
    }

    /// Mark a key as having exceeded quota.
    pub fn mark_quota_exceeded(&self, key: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(state) = inner.keys.get_mut(key) {
            state.status = KeyStatus::QuotaExceeded;
        }
    }

    /// Get the count of available keys.
    pub fn available_count(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        Self::reset_expired_cooldowns_inner(&mut inner);
        inner.keys.values().filter(|s| s.is_available()).count()
    }

    /// Remove a key from management.
    pub fn remove_key(&self, key: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.keys.remove(key).is_some() {
            inner.key_order.retain(|k| k != key);
            if inner.current_index >= inner.key_order.len() && !inner.key_order.is_empty() {
                inner.current_index = 0;
            }
            true
        } else {
            false
        }
    }

    /// Get statistics for all managed keys.
    pub fn statistics(&self) -> KeyStatistics {
        let mut inner = self.inner.lock().unwrap();
        Self::reset_expired_cooldowns_inner(&mut inner);

        KeyStatistics {
            total_keys: inner.keys.len(),
            available_keys: inner
                .keys
                .values()
                .filter(|s| s.status == KeyStatus::Available)
                .count(),
            rate_limited_keys: inner
                .keys
                .values()
                .filter(|s| s.status == KeyStatus::RateLimited)
                .count(),
            invalid_keys: inner
                .keys
                .values()
                .filter(|s| s.status == KeyStatus::Invalid)
                .count(),
            quota_exceeded_keys: inner
                .keys
                .values()
                .filter(|s| s.status == KeyStatus::QuotaExceeded)
                .count(),
            total_requests: inner
                .keys
                .values()
                .map(|s| {
                    s.metadata
                        .get("request_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                })
                .sum(),
            successful_requests: inner
                .keys
                .values()
                .map(|s| {
                    s.metadata
                        .get("success_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                })
                .sum(),
            failed_requests: inner
                .keys
                .values()
                .map(|s| {
                    s.metadata
                        .get("failure_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                })
                .sum(),
        }
    }

    fn reset_expired_cooldowns_inner(inner: &mut ApiKeyManagerInner) {
        let now = Instant::now();
        for state in inner.keys.values_mut() {
            if state.status == KeyStatus::RateLimited
                && let Some(until) = state.cooldown_until
                && now >= until
            {
                state.status = KeyStatus::Available;
                state.cooldown_until = None;
            }
        }
    }
}

/// Well-known model names with their providers.
pub const MODEL_PROVIDER_MAP: &[(&str, ModelProvider)] = &[
    ("gpt-4.1", ModelProvider::OpenAi),
    ("gpt-4.1-nano", ModelProvider::OpenAi),
    ("gpt-4.1-mini", ModelProvider::OpenAi),
    ("o4-mini", ModelProvider::OpenAi),
    ("o3", ModelProvider::OpenAi),
    ("gemini-2.5-flash-preview-05-20", ModelProvider::Google),
    ("gemini-2.5-pro-preview-06-05", ModelProvider::Google),
    ("claude-3-5-haiku-latest", ModelProvider::Anthropic),
    ("claude-sonnet-4-20250514", ModelProvider::Anthropic),
];

/// LLM provider configuration with tiered model selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderConfig {
    pub dumb_model: String,
    pub average_model: String,
    pub reasoning_model: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for LlmProviderConfig {
    fn default() -> Self {
        Self {
            dumb_model: "gpt-4.1-nano".into(),
            average_model: "gpt-4.1-nano".into(),
            reasoning_model: "o4-mini".into(),
            timeout_secs: 80,
            max_retries: 3,
        }
    }
}

/// LLM provider with tiered agent selection and key rotation.
///
/// Mirrors the Python `LLMProvider` class structure.
pub struct LlmProvider {
    pub config: LlmProviderConfig,
    key_managers: HashMap<ModelProvider, ApiKeyManager>,
}

impl LlmProvider {
    /// Create a new LLM provider with default configuration.
    pub fn new(config: Option<LlmProviderConfig>) -> Self {
        Self {
            config: config.unwrap_or_default(),
            key_managers: HashMap::new(),
        }
    }

    /// Register an API key manager for a provider.
    pub fn register_key_manager(&mut self, provider: ModelProvider, manager: ApiKeyManager) {
        self.key_managers.insert(provider, manager);
    }

    /// Resolve the provider for a model name.
    pub fn provider_for_model(&self, model: &str) -> Option<ModelProvider> {
        ModelProvider::from_model_name(model)
    }

    /// Get the appropriate model name for a given reasoning effort.
    pub fn model_for_effort(&self, effort: ReasoningEffort) -> &str {
        match effort {
            ReasoningEffort::Low => &self.config.dumb_model,
            ReasoningEffort::Medium => &self.config.average_model,
            ReasoningEffort::High => &self.config.reasoning_model,
        }
    }

    /// Get the API key manager for a model's provider, if registered.
    pub fn key_manager_for_model(&self, model: &str) -> Option<&ApiKeyManager> {
        let provider = ModelProvider::from_model_name(model)?;
        self.key_managers.get(&provider)
    }
}

/// Discover API keys for a provider from environment variables.
///
/// Checks `{PROVIDER}_API_KEY` and `{PROVIDER}_API_KEY_1`, `_2`, etc.
pub fn discover_keys_for_provider(provider: ModelProvider) -> Vec<String> {
    let prefix = match provider {
        ModelProvider::OpenAi => "OPENAI",
        ModelProvider::Anthropic => "ANTHROPIC",
        ModelProvider::Google => "GOOGLE",
    };

    let mut keys = Vec::new();
    let primary = format!("{prefix}_API_KEY");
    if let Ok(val) = std::env::var(&primary)
        && !val.is_empty()
    {
        keys.push(val);
    }

    for i in 1..=10 {
        let var = format!("{primary}_{i}");
        if let Ok(val) = std::env::var(&var)
            && !val.is_empty()
        {
            keys.push(val);
        }
    }

    if !keys.is_empty() {
        info!(provider = ?provider, count = keys.len(), "Discovered API keys");
    }
    keys
}

/// Validate an API key format for a specific provider.
pub fn validate_key(key: &str, provider: ModelProvider) -> bool {
    if key.is_empty() {
        return false;
    }
    match provider {
        ModelProvider::OpenAi => key.starts_with("sk-"),
        ModelProvider::Anthropic => key.starts_with("sk-ant-"),
        ModelProvider::Google => key.len() > 10,
    }
}

/// Parse structured JSON output from LLM response content.
///
/// Handles markdown code blocks (`\`\`\`json ... \`\`\``) and raw JSON.
pub fn parse_structured_output(content: &str) -> Result<serde_json::Value> {
    let trimmed = content.trim();

    let json_str = if trimmed.starts_with("```json") {
        trimmed
            .strip_prefix("```json")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim()
    } else if trimmed.starts_with("```") {
        trimmed
            .strip_prefix("```")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim()
    } else {
        trimmed
    };

    serde_json::from_str(json_str).context("Failed to parse JSON from LLM response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_provider_detection() {
        assert_eq!(
            ModelProvider::from_model_name("gpt-4.1"),
            Some(ModelProvider::OpenAi)
        );
        assert_eq!(
            ModelProvider::from_model_name("claude-3-5-haiku-latest"),
            Some(ModelProvider::Anthropic)
        );
        assert_eq!(
            ModelProvider::from_model_name("gemini-2.5-flash-preview-05-20"),
            Some(ModelProvider::Google)
        );
        assert_eq!(ModelProvider::from_model_name("unknown-model"), None);
    }

    #[test]
    fn key_state_availability() {
        let state = KeyState::new("sk-test123".into());
        assert!(state.is_available());

        let mut limited = state.clone();
        limited.status = KeyStatus::RateLimited;
        assert!(!limited.is_available());
    }

    #[test]
    fn key_manager_add_and_rotate() {
        let mgr = ApiKeyManager::new(ModelProvider::OpenAi, None);
        assert!(mgr.add_key("sk-key1"));
        assert!(mgr.add_key("sk-key2"));
        assert!(!mgr.add_key("sk-key1")); // duplicate

        let k1 = mgr.get_next_available_key().unwrap();
        let k2 = mgr.get_next_available_key().unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn key_manager_mark_invalid() {
        let mgr = ApiKeyManager::new(ModelProvider::OpenAi, None);
        mgr.add_key("sk-badkey");
        mgr.mark_invalid("sk-badkey");
        assert_eq!(mgr.available_count(), 0);
    }

    #[test]
    fn validate_key_providers() {
        assert!(validate_key("sk-abc123", ModelProvider::OpenAi));
        assert!(!validate_key("invalid", ModelProvider::OpenAi));
        assert!(validate_key("sk-ant-abc123", ModelProvider::Anthropic));
        assert!(!validate_key("sk-abc123", ModelProvider::Anthropic));
        assert!(validate_key("AIzaSyABCDEFGH", ModelProvider::Google));
        assert!(!validate_key("", ModelProvider::Google));
    }

    #[test]
    fn parse_json_from_markdown() {
        let content = "```json\n{\"key\": \"value\"}\n```";
        let result = parse_structured_output(content).unwrap();
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn parse_raw_json() {
        let content = "{\"key\": \"value\"}";
        let result = parse_structured_output(content).unwrap();
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn llm_provider_model_selection() {
        let provider = LlmProvider::new(None);
        assert_eq!(
            provider.model_for_effort(ReasoningEffort::Low),
            "gpt-4.1-nano"
        );
        assert_eq!(provider.model_for_effort(ReasoningEffort::High), "o4-mini");
    }
}
