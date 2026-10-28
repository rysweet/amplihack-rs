//! What `profile rotate` (issue #1592) reuses from auto-switch: the state
//! lock, the toggle, the liveness scan, the file helpers, the login-move
//! primitives, auto-switch recovery and the no-capacity file writer.
//! Re-exports only.

pub use super::capacity::{Rotated, refresh_from_rotation as capacity_from_rotation};
pub use super::config::{Source, effective};
pub use super::fsx::{
    check_root_private, ensure_private_dir, euid, exists, fsync_dir, movable_login, now,
    read_bounded_nofollow, writable_by_others, write_atomic_keep_mode, write_private_atomic,
};
pub use super::liveness::{Liveness, scan_system};
pub use super::mv::{
    MoveError, REFRESH_LOCK, abort_point, exchange_logins, move_login, release_refresh_lock_at,
    take_refresh_lock_at,
};
pub use super::state::{CLAUDE_JSON, CREDS, Ledger, Lock, MAX_CLAUDE_JSON, State};
pub use super::swap::{account, guard_held, read_claude_json, recover as recover_auto_switch};
