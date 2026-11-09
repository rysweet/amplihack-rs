//! Rotator liveness for usage-limit waits (issue #2022), split from daemon.rs.

use super::{DaemonState, ProfileError, RotateState, prototype_guard};
use std::path::Path;

/// Whether a rotator process of this user is alive for `root`, under any
/// binary name: the built-in daemon named by `rotate.pid`, or the external
/// prototype holding `.rotator/lock`. Used while a usage-limit wait depends
/// on the rotator (issue #2022). Unlike [`super::running`] it does not compare the
/// daemon's `argv[0]` with this binary's name, which reads `amplihack
/// (deleted)` once an install replaces the binary, so an upgrade during a
/// wait does not look like a stopped rotator. Without a `/proc` it is true.
pub(in crate::commands::profile) fn alive(root: &Path) -> bool {
    alive_at(&RotateState::new(root), Path::new("/proc"))
}

/// [`alive`] against another `/proc`.
pub(in crate::commands::profile) fn alive_at(rs: &RotateState, proc_root: &Path) -> bool {
    matches!(
        amplihack_utils::rotator_status::daemon_state(&rs.pid, proc_root, None),
        DaemonState::Running(_) | DaemonState::Unverifiable
    ) || matches!(
        prototype_guard(&rs.root),
        Err(ProfileError::RotatePrototypeRunning)
    )
}
