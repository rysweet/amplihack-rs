//! Issue #2967: the note the retry after an earliest-reset wait prints.

use super::NOTE_BLOCK_ENDED;
use crate::commands::recipe::run::limit_patterns::all_markers;
use crate::commands::recipe::run::{FailureClass, classify_failure_text};

#[test]
fn the_block_ended_note_carries_no_usage_limit_marker() {
    // It reaches the step's stderr, which the recipe runner's classifier
    // reads: a note that named a usage limit would be read as one.
    let line = format!("{NOTE_BLOCK_ENDED} \"personal\" (auto-switch)");
    let lowered = line.to_ascii_lowercase();
    for marker in all_markers() {
        assert!(!lowered.contains(marker), "{marker:?} in {line:?}");
    }
    assert_ne!(
        classify_failure_text(&line).0,
        FailureClass::UsageLimit,
        "{line:?}"
    );
    assert!(line.starts_with("amplihack: "), "{line:?}");
    assert_eq!(line.lines().count(), 1, "{line:?}");
}
