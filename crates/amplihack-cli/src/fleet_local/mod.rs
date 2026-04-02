//! Local session management dashboard (fleet_local).
//!
//! Reads `~/.claude/runtime/locks/*` lock files to discover and display
//! active Claude sessions on the **local machine**.
//!
//! This module is the Python-to-Rust port of the amploxy local session
//! dashboard.  It is completely separate from the Azure-VM fleet
//! orchestration in `commands/fleet.rs`.
//!
//! # Architecture
//!
//! ```text
//! ~/.claude/runtime/locks/{session_id}   ← one file per active session
//!           ↓ collect_observed_fleet_state()
//!   Vec<FleetSessionEntry>               ← sanitised, PID-validated rows
//!           ↓ run_fleet_dashboard()
//!   TUI render / bg refresh threads      ← two-phase refresh (500 ms / 5 s)
//! ```
//!
//! # Design spec
//!
//! Full spec: docs/concepts/fleet-dashboard-architecture.md (v0.5.0 target).

mod cache;
mod dashboard;
mod editor;
mod osc;
mod state;
mod summary;
mod tmux;
mod types;

pub use cache::*;
pub use dashboard::*;
pub use editor::*;
pub use osc::*;
pub use state::*;
pub use summary::*;
pub use types::*;

// ── Constants ────────────────────────────────────────────────────────────────

/// Maximum PID value accepted when reading lock files (Linux kernel limit).
pub const PID_MAX: u32 = 4_194_304;

/// LRU capture cache capacity (entries).
pub const CAPTURE_CACHE_CAPACITY: usize = 64;

/// Maximum bytes stored per capture cache entry (64 KiB).
pub const CAPTURE_CACHE_ENTRY_MAX_BYTES: usize = 64 * 1024;

/// Maximum number of lines in the multiline editor.
pub const EDITOR_MAX_LINES: usize = 200;

/// Maximum bytes per line in the multiline editor.
pub const EDITOR_MAX_BYTES_PER_LINE: usize = 4096;

/// Maximum characters in a pre-filled prompt handoff to session creation.
pub const PROMPT_MAX_CHARS: usize = 1000;
