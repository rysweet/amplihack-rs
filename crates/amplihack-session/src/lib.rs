//! amplihack-session: native Rust port of `amplifier-bundle/tools/amplihack/session`.
//!
//! See `docs/design/wave-3b-session-port.md` (Step 5/6) for the full contract.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::manual_inspect)]
#![allow(dead_code)]

pub mod batch;
pub mod config;
pub mod file_utils;
pub mod logger;
pub mod manager;
pub mod session;
pub mod toolkit;

pub use batch::{BatchFileOperations, BatchOp};
pub use config::{SessionConfig, SessionError, SessionState};
pub use file_utils::{
    ChecksumAlgorithm, MAX_JSON_FILE_BYTES, cleanup_temp_files, get_file_checksum, safe_copy_file,
    safe_move_file, safe_read_file, safe_read_json, safe_write_file, safe_write_json,
};
pub use logger::{LogEntry, LogLevel, OperationContext, ToolkitLogger, ToolkitLoggerBuilder};
pub use manager::SessionManager;
pub use session::{ClaudeSession, CommandExecutor, CommandRecord, NoopExecutor};
pub use toolkit::{SessionToolkit, quick_session};
