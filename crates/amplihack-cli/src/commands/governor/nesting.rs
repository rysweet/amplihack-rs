//! Which live slot, if any, a start runs under (D2). A nested start takes no
//! slot of its own and is not counted against the concurrency limit, so both
//! ways of naming a parent also require the parent's process to be this
//! process or one of its ancestors.

use super::files::tree_id_ok;
use super::liveness::self_or_ancestor;
use super::model::Slot;

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
