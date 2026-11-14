//! Rotator liveness, split from daemon.rs to keep it within the line budget:
//! [`running`] and [`rotating`] for takeover (issue #1801), and [`alive`] for
//! usage-limit waits (issue #2022). `daemon.rs` re-exports all three.

use super::{
    DaemonState, ProfileError, RotateState, current_state, paused, proc_root, prototype_guard,
};
use std::path::Path;

/// Whether a login rotator is running for the store at `root`: the built-in
/// daemon, or the external prototype holding `.rotator/lock` (issue #1801).
/// Any read error is `false`. Writes nothing, but the prototype check takes
/// and at once releases a non-blocking exclusive `flock` on `.rotator/lock`
/// when nothing holds it (see [`prototype_guard`]), so a rotator taking that
/// lock with `LOCK_NB` in the same instant can be refused once.
pub(in crate::commands::profile) fn running(root: &Path) -> bool {
    matches!(
        current_state(&RotateState::new(root)),
        DaemonState::Running(_)
    ) || matches!(
        prototype_guard(root),
        Err(ProfileError::RotatePrototypeRunning)
    )
}

/// Whether a running rotator is rotating logins: [`running`], and auto-switch
/// is not turned off. A rotator with auto-switch off stays idle (the watch
/// loop skips every cycle while [`paused`]), so it records no cycle and makes
/// no swap. This uses the watch loop's own [`paused`] rule, so the two cannot
/// drift. `paused` is checked first and needs no lock, so with auto-switch
/// off `.rotator/lock` is not touched. The auto-switch setting is read in the
/// calling process, environment first, as everywhere else.
pub(in crate::commands::profile) fn rotating(root: &Path) -> bool {
    !paused(root) && running(root)
}

/// Whether a rotator process of this user is alive for `root`, under any
/// binary name: the built-in daemon named by `rotate.pid`, or the external
/// prototype holding `.rotator/lock`. Used while a usage-limit wait depends
/// on the rotator (issue #2022). Unlike [`running`] it does not compare the
/// daemon's `argv[0]` with this binary's name, which reads `amplihack
/// (deleted)` once an install replaces the binary, so an upgrade during a
/// wait does not look like a stopped rotator. It reads the process table
/// through [`proc_root`] (issue #2042). Without a `/proc` it is true.
pub(in crate::commands::profile) fn alive(root: &Path) -> bool {
    alive_at(&RotateState::new(root), &proc_root())
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
