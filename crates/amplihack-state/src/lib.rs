//! File operations, locking, and environment configuration.
//!
//! Provides the infrastructure layer for hooks: atomic file operations,
//! timeout-based locking, and typed environment variable parsing.

/// Atomic JSON file operations with crash-safe writes.
pub mod atomic_json;
/// Atomic counter for metrics and sequencing.
pub mod counter;
/// Typed environment variable parsing and configuration.
pub mod env_config;
/// Timeout-based file locking for session coordination.
pub mod file_lock;
/// Atomic flag (semaphore) for single-writer coordination.
pub mod semaphore;

/// Atomic JSON file operations with crash-safe writes.
pub use atomic_json::AtomicJsonFile;
/// Atomic counter for metrics and sequencing.
pub use counter::AtomicCounter;
/// Typed environment variable parsing.
pub use env_config::EnvVar;
/// Timeout-based file lock.
pub use file_lock::FileLock;
/// Atomic boolean flag for coordination.
pub use semaphore::AtomicFlag;
