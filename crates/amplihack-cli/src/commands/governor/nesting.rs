//! Which live slot, if any, a start runs under (D2). A nested start takes no
//! slot of its own and is not counted against the concurrency limit, so both
//! ways of naming a parent also require the parent's process to be this
//! process or one of its ancestors.

use super::files::tree_id_ok;
use super::liveness::{in_ancestry, same_process, self_or_ancestor, start_time};
use super::model::{Slot, Start};

/// The index of the live slot a nested launch runs under: a depth above 0,
/// a tree id equal to a slot's, and that slot's process an ancestor of this
/// one. Every child of an admitted launch inherits `AMPLIHACK_TREE_ID` and a
/// session depth, so both also reach a tmux server or a background process
/// that a step starts and that outlives the step. A start from such an
/// environment is not part of the slot's tree and must count against the
/// limit, so the tree id alone nests nothing; [`marked_parent`] applies the
/// same check to the parent marker.
pub fn nested_parent(depth: Option<&str>, tree_id: Option<&str>, slots: &[Slot]) -> Option<usize> {
    let depth: u32 = depth?.trim().parse().ok()?;
    let tree = tree_id?;
    if depth == 0 || !tree_id_ok(tree) {
        return None;
    }
    slots
        .iter()
        .position(|s| s.tree_id == tree && self_or_ancestor(s.pid))
}

/// The index of the live slot named by `AMPLIHACK_GOVERNOR_PARENT` whose
/// process is an ancestor of this one. A top-level `amplihack claude` sets
/// no session depth on its children, so a `recipe run` started inside it is
/// found by this marker instead.
pub fn marked_parent(marker: Option<&str>, slots: &[Slot]) -> Option<usize> {
    let tree = marker.filter(|t| tree_id_ok(t))?;
    slots
        .iter()
        .position(|s| s.tree_id == tree && self_or_ancestor(s.pid))
}

/// What a nested start runs under: the parent slot's own [`Start`], or
/// [`Start::Recipe`] when `AMPLIHACK_GOVERNOR_RECIPE` names a `recipe run`
/// that sits between this process and the slot. A `recipe run` nested under
/// an interactive session takes no slot of its own, so without the marker
/// its agent steps would see only the session's `agent` slot and be held at
/// the reserve part-way through the run (crusty round 4 on #1831).
pub fn parent_start(slot: &Slot, recipe_marker: Option<&str>) -> Start {
    if slot.start == Start::Recipe || recipe_between(slot, recipe_marker) {
        Start::Recipe
    } else {
        slot.start
    }
}

/// The marker as `<pid>:<start time>`, or `<pid>` where `/proc` gives no
/// start time. It counts only when that process is still the one that set
/// it, is this process or an ancestor of it, and runs under `slot`'s
/// process, so a marker that leaked into a tmux server, a background process
/// or an unrelated session marks nothing.
pub fn recipe_between(slot: &Slot, recipe_marker: Option<&str>) -> bool {
    let Some(raw) = recipe_marker.map(str::trim).filter(|m| !m.is_empty()) else {
        return false;
    };
    let (pid, recorded) = match raw.split_once(':') {
        Some((pid, start)) => match start.parse::<u64>() {
            Ok(start) => (pid, Some(start)),
            Err(_) => return false,
        },
        None => (raw, None),
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return false;
    };
    pid > 1
        && same_process(recorded, start_time(pid))
        && self_or_ancestor(pid)
        && in_ancestry(pid, slot.pid)
}

/// The marker a `recipe run` sets on its children: see [`recipe_between`].
pub fn recipe_marker_for(pid: u32) -> String {
    match start_time(pid) {
        Some(start) => format!("{pid}:{start}"),
        None => pid.to_string(),
    }
}
