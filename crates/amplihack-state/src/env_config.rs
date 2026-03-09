//! Typed environment variable parsing.
//!
//! Provides `EnvVar<T>` for type-safe environment variable access
//! with defaults and error reporting.

use std::env;
use std::fmt;
use std::str::FromStr;

/// A typed environment variable reader.
///
/// ```
/// use amplihack_state::EnvVar;
///
/// let timeout: u64 = EnvVar::new("AMPLIHACK_TIMEOUT").default(30).get();
/// let debug: bool = EnvVar::new("AMPLIHACK_DEBUG").default(false).get();
/// ```
pub struct EnvVar<T> {
    name: &'static str,
    default: Option<T>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> EnvVar<T>
where
    T: FromStr + Clone,
    T::Err: fmt::Display,
{
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            default: None,
            _marker: std::marker::PhantomData,
        }
    }

    /// Set a default value if the env var is not set.
    pub fn default(mut self, value: T) -> Self {
        self.default = Some(value);
        self
    }

    /// Get the value. Uses default if env var not set.
    /// Panics if no default and env var not set, or if parse fails.
    pub fn get(&self) -> T {
        match env::var(self.name) {
            Ok(val) => val.parse().unwrap_or_else(|e| {
                panic!("Failed to parse env var {}={:?}: {}", self.name, val, e);
            }),
            Err(_) => self.default.clone().unwrap_or_else(|| {
                panic!("Required env var {} not set", self.name);
            }),
        }
    }

    /// Get the value as an Option. Returns None if env var not set.
    pub fn try_get(&self) -> Option<T> {
        match env::var(self.name) {
            Ok(val) => val.parse().ok(),
            Err(_) => self.default.clone(),
        }
    }

    /// Check if the env var is set (regardless of parseability).
    pub fn is_set(&self) -> bool {
        env::var(self.name).is_ok()
    }
}

/// Get a string env var with a default.
pub fn env_str(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Get a bool env var (truthy: "1", "true", "yes").
pub fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(val) => matches!(val.to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

/// Get a u64 env var with a default.
pub fn env_u64(name: &str, default: u64) -> u64 {
    match env::var(name) {
        Ok(val) => val.parse().unwrap_or(default),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_with_default() {
        let val: u64 = EnvVar::new("AMPLIHACK_TEST_NONEXISTENT_123")
            .default(42)
            .get();
        assert_eq!(val, 42);
    }

    #[test]
    fn env_bool_parsing() {
        unsafe {
            env::set_var("AMPLIHACK_TEST_BOOL", "true");
        }
        assert!(env_bool("AMPLIHACK_TEST_BOOL", false));
        unsafe {
            env::set_var("AMPLIHACK_TEST_BOOL", "0");
        }
        assert!(!env_bool("AMPLIHACK_TEST_BOOL", true));
        unsafe {
            env::remove_var("AMPLIHACK_TEST_BOOL");
        }
    }

    #[test]
    fn env_str_default() {
        assert_eq!(
            env_str("AMPLIHACK_TEST_NONEXISTENT_456", "default_val"),
            "default_val"
        );
    }
}
