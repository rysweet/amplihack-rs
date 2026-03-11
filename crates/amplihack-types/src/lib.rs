//! Thin IPC boundary types for amplihack hooks.
//!
//! Only types that cross process boundaries live here.
//! Domain types live in their domain crates.

/// Hook input/output types for the JSON stdin/stdout protocol.
pub mod hook_io;
/// Centralized directory layout for the `.claude` runtime tree.
pub mod paths;
/// Global and project-level settings deserialization.
pub mod settings;

/// Top-level hook input enum (tagged by `hook_event_name`).
pub use hook_io::HookInput;
/// Project directory layout.
pub use paths::ProjectDirs;
/// Session ID sanitizer to prevent path traversal.
pub use paths::sanitize_session_id;
/// Deserialized settings from `settings.json`.
pub use settings::Settings;
