//! File operations, locking, and environment configuration.
//!
//! Provides the infrastructure layer for hooks: atomic file operations,
//! timed and blocking locking, and typed environment variable parsing.

/// Atomic JSON file operations with crash-safe writes.
pub mod atomic_json;
/// Atomic counter for metrics and sequencing.
pub mod counter;
/// Typed environment variable parsing and configuration.
pub mod env_config;
/// Timed and blocking file locking for session coordination.
pub mod file_lock;
/// Atomic flag (semaphore) for single-writer coordination.
pub mod semaphore;

/// Atomic JSON file operations with crash-safe writes.
pub use atomic_json::AtomicJsonFile;
/// Atomic counter for metrics and sequencing.
pub use counter::AtomicCounter;
/// Typed environment variable parsing.
pub use env_config::EnvVar;
/// Exclusive file lock with timed and blocking acquisition.
pub use file_lock::FileLock;
/// Atomic boolean flag for coordination.
pub use semaphore::AtomicFlag;

#[cfg(all(test, unix))]
mod stop_counter_tests;
