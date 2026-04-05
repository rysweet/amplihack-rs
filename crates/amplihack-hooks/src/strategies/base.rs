//! Base trait for host-specific hook strategies.
//!
//! Each launcher (Claude Code, Copilot, etc.) implements [`HookStrategy`] to
//! customise how context is injected and how power-steering is dispatched.

use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

/// Strategy interface for host-specific hook behaviour.
///
/// Implementors provide launcher-aware context injection and power-steering
/// dispatch. The trait is object-safe so strategies can be used dynamically.
pub trait HookStrategy: Send + Sync {
    /// Inject context into the hook output for the host launcher.
    ///
    /// Returns a map of keys to JSON values that should be merged into the
    /// hook's response payload.
    fn inject_context(&self, context: &str) -> Result<HashMap<String, Value>>;

    /// Trigger power-steering via the host launcher.
    ///
    /// Returns `true` if the steering prompt was successfully dispatched.
    fn power_steer(&self, prompt: &str, session_id: &str) -> Result<bool>;

    /// Human-readable name of the launcher this strategy targets.
    fn get_launcher_name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dummy strategy for testing trait object usage.
    struct DummyStrategy;

    impl HookStrategy for DummyStrategy {
        fn inject_context(&self, context: &str) -> Result<HashMap<String, Value>> {
            let mut map = HashMap::new();
            map.insert("context".to_string(), Value::String(context.to_string()));
            Ok(map)
        }

        fn power_steer(&self, _prompt: &str, _session_id: &str) -> Result<bool> {
            Ok(true)
        }

        fn get_launcher_name(&self) -> &'static str {
            "dummy"
        }
    }

    #[test]
    fn trait_is_object_safe() {
        let strategy: Box<dyn HookStrategy> = Box::new(DummyStrategy);
        assert_eq!(strategy.get_launcher_name(), "dummy");
    }

    #[test]
    fn inject_context_returns_map() {
        let strategy = DummyStrategy;
        let map = strategy.inject_context("hello").unwrap();
        assert_eq!(map["context"], Value::String("hello".to_string()));
    }

    #[test]
    fn power_steer_returns_bool() {
        let strategy = DummyStrategy;
        assert!(strategy.power_steer("do stuff", "sess-1").unwrap());
    }

    #[test]
    fn inject_context_empty_string() {
        let strategy = DummyStrategy;
        let map = strategy.inject_context("").unwrap();
        assert_eq!(map["context"], Value::String(String::new()));
    }

    #[test]
    fn launcher_name_is_static() {
        let strategy = DummyStrategy;
        let name: &'static str = strategy.get_launcher_name();
        assert!(!name.is_empty());
    }
}
