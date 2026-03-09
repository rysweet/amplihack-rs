//! File operations, locking, environment configuration, and Python bridge.
//!
//! Provides the infrastructure layer for hooks: atomic file operations,
//! timeout-based locking, typed environment variable parsing, and
//! subprocess-based Python bridge for SDK calls.

pub mod atomic_json;
pub mod counter;
pub mod env_config;
pub mod file_lock;
pub mod python_bridge;
pub mod semaphore;

pub use atomic_json::AtomicJsonFile;
pub use counter::AtomicCounter;
pub use env_config::EnvVar;
pub use file_lock::FileLock;
pub use python_bridge::PythonBridge;
pub use semaphore::AtomicFlag;
