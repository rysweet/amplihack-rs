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
    ///
    /// Prefer [`get_or_err`] when graceful error handling is needed.
    pub fn get(&self) -> T {
        match self.get_or_err() {
            Ok(val) => val,
            Err(e) => panic!("{e}"),
        }
    }

    /// Get the value, returning an error instead of panicking.
    ///
    /// Returns `Err` if the env var is required (no default) and missing,
    /// or if the value cannot be parsed as `T`.
    pub fn get_or_err(&self) -> Result<T, String> {
        match env::var(self.name) {
            Ok(val) => val.parse().map_err(|e| {
                format!("Failed to parse env var {}={:?}: {}", self.name, val, e)
            }),
            Err(_) => self.default.clone().ok_or_else(|| {
                format!("Required env var {} not set", self.name)
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
    fn env_bool_defaults() {
        // Test default behavior without modifying env vars.
        assert!(!env_bool("AMPLIHACK_TEST_BOOL_UNSET_XYZ", false));
        assert!(env_bool("AMPLIHACK_TEST_BOOL_UNSET_XYZ", true));
    }

    #[test]
    fn env_str_default() {
        assert_eq!(
            env_str("AMPLIHACK_TEST_NONEXISTENT_456", "default_val"),
            "default_val"
        );
    }

    #[test]
    fn env_u64_default() {
        assert_eq!(env_u64("AMPLIHACK_TEST_NONEXISTENT_789", 99), 99);
    }

    #[test]
    fn get_or_err_returns_default() {
        let val: Result<u64, _> = EnvVar::new("AMPLIHACK_TEST_NONEXISTENT_GET_ERR")
            .default(7)
            .get_or_err();
        assert_eq!(val.unwrap(), 7);
    }

    #[test]
    fn get_or_err_missing_no_default() {
        let val: Result<u64, _> = EnvVar::new("AMPLIHACK_TEST_MISSING_NO_DEFAULT")
            .get_or_err();
        assert!(val.is_err());
        assert!(val.unwrap_err().contains("Required env var"));
    }
}
