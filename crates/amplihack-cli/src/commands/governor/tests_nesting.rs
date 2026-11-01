//! Issue #1811: nesting (D2). A start runs under a live slot only when the
//! slot's process is an ancestor; a tree id or parent marker that leaked into
//! an unrelated environment nests nothing (crusty round 1 on #1831).

use super::admit::admit_with;
use super::liveness::start_time;
use super::model::Slot;
use super::nesting::{marked_parent, nested_parent};
use super::store::{Queue, State};
use super::tests_admit::{ROOMY, ctx, load, me_slot, save};
use super::tests_seed::Root;

#[test]
fn nested_launch_runs_under_the_parent_slot() {
    let r = Root::new(ROOMY);
    let state = State {
        limit: 1,
        slots: vec![me_slot("t-parent")],
        ..State::default()
    };
    save(&r, &state, &Queue::default());
    let mut c = ctx(&r);
    c.session_depth = Some("1".into());
    c.tree_id = Some("t-parent".into());
    let guard = admit_with(&c).expect("nested is not limited by concurrency");
    assert!(guard.nested());
    let (state, _) = load(&r);
    assert_eq!(state.slots.len(), 1, "a nested launch takes no slot");
    assert_eq!(state.slots[0].nested_children, 1);
    drop(guard);
    assert_eq!(
        load(&r).0.slots.len(),
        1,
        "dropping a nested guard keeps the parent slot"
    );
}

/// A `recipe run` inside a top-level `amplihack claude` has no session
/// depth; the parent marker alone nests it, so limit 1 does not deadlock.
#[test]
fn parent_marker_nests_a_start_without_session_depth() {
    let r = Root::new(ROOMY);
    let state = State {
        limit: 1,
        slots: vec![me_slot("t-claude")],
        ..State::default()
    };
    save(&r, &state, &Queue::default());
    let mut c = ctx(&r);
    c.parent = Some("t-claude".into());
    let guard = admit_with(&c).expect("the marker nests the start");
    assert!(guard.nested());
    assert_eq!(load(&r).0.slots[0].nested_children, 1);
    let slots = [me_slot("t-a")];
    assert_eq!(marked_parent(Some("t-a"), &slots), Some(0));
    assert_eq!(marked_parent(Some("t-z"), &slots), None);
    assert_eq!(marked_parent(Some("t a"), &slots), None);
    assert_eq!(marked_parent(None, &slots), None);
}

/// A live process that is not an ancestor of this test: a child. It is
/// killed and reaped on drop.
struct Unrelated(std::process::Child);

impl Unrelated {
    fn spawn() -> Self {
        Self(
            std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn sleep"),
        )
    }

    fn slot(&self, tree: &str) -> Slot {
        let pid = self.0.id();
        Slot {
            pid,
            start_time: start_time(pid),
            ..me_slot(tree)
        }
    }
}

impl Drop for Unrelated {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Crusty round 1 on #1831: a tree id and a session depth copied into an
/// unrelated environment (a tmux server a step started, say) name a live
/// slot whose process is not an ancestor. Like the parent marker, that nests
/// nothing.
#[cfg(target_os = "linux")]
#[test]
fn a_leaked_tree_id_nests_nothing() {
    let other = Unrelated::spawn();
    let slots = [other.slot("t-leaked"), me_slot("t-mine")];
    assert_eq!(nested_parent(Some("1"), Some("t-leaked"), &slots), None);
    assert_eq!(nested_parent(Some("1"), Some("t-mine"), &slots), Some(1));
    assert_eq!(marked_parent(Some("t-leaked"), &slots), None);
}

/// The same leak through the admission loop: the start counts against the
/// limit, so with the one slot held it waits and, with no wait, is refused.
#[cfg(target_os = "linux")]
#[test]
fn a_start_with_a_leaked_tree_id_counts_against_the_limit() {
    let other = Unrelated::spawn();
    let r = Root::new(ROOMY);
    let state = State {
        limit: 1,
        slots: vec![other.slot("t-leaked")],
        ..State::default()
    };
    save(&r, &state, &Queue::default());
    let mut c = ctx(&r);
    c.settings.max_concurrency = 1;
    c.session_depth = Some("1".into());
    c.tree_id = Some("t-leaked".into());
    c.parent = Some("t-leaked".into());
    let denied = admit_with(&c)
        .err()
        .expect("the leaked tree id must not nest");
    assert_eq!(denied.code(), "wait-expired");
    let (state, queue) = load(&r);
    assert_eq!(state.slots.len(), 1);
    assert_eq!(state.slots[0].nested_children, 0, "nothing nested");
    assert!(queue.entries.is_empty());
}

#[test]
fn orphaned_nested_launch_is_top_level() {
    let r = Root::new(ROOMY);
    let mut c = ctx(&r);
    c.session_depth = Some("2".into());
    c.tree_id = Some("t-gone".into());
    let guard = admit_with(&c).expect("room");
    assert!(!guard.nested());
    assert_eq!(load(&r).0.slots.len(), 1);
}

#[test]
fn nested_parent_detection() {
    let slots = [me_slot("t-a"), me_slot("t-b")];
    assert_eq!(nested_parent(Some("1"), Some("t-b"), &slots), Some(1));
    assert_eq!(nested_parent(Some("0"), Some("t-b"), &slots), None);
    assert_eq!(nested_parent(None, Some("t-b"), &slots), None);
    assert_eq!(nested_parent(Some("x"), Some("t-b"), &slots), None);
    assert_eq!(nested_parent(Some("1"), Some("t-c"), &slots), None);
    assert_eq!(nested_parent(Some("1"), Some("T-B"), &slots), None);
    assert_eq!(
        nested_parent(Some("1"), Some(&"t".repeat(129)), &slots),
        None
    );
}
