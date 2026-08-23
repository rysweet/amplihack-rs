//! Issue #1266 — the framework restage must not run on every launch.
//!
//! `ensure_framework_installed` runs on **every** launch and restages whenever
//! `missing_framework_paths` reports a gap. So a gap that a restage cannot
//! close is a permanent loop: amplihack copies the whole bundle and rewrites
//! settings.json, finishes with the identical gap, and does it again next
//! launch.
//!
//! `essential_files(Bundle)` briefly gained `context/SYSTEM_PROMPT_APPEND.md`
//! to deliver issue #1265's feature to existing installs, which created exactly
//! that loop — and worse, the restage it armed sources from a walk up from
//! `current_dir()`, so a cloned fork could write `$HOME` and have its bytes
//! injected at system-prompt privilege.
//!
//! The fix is that the fragment is `include_str!`d into the binary and is not
//! an installed asset at all. No listing, no gap, no trigger. These tests pin
//! the property that makes the simple restage rule correct: a fully-staged
//! install reports nothing missing, so nothing restages.

use super::*;
use std::fs;

#[test]
fn a_missing_staging_dir_always_bootstraps() {
    assert!(framework_restage_needed(false, &[]));
}

#[test]
fn a_gap_triggers_a_restage() {
    let missing = vec!["tools/statusline.sh (expected at /x/tools/statusline.sh)".to_string()];
    assert!(framework_restage_needed(true, &missing));
}

#[test]
fn no_gap_means_no_restage() {
    assert!(!framework_restage_needed(true, &[]));
}

/// The load-bearing one: drives the real `missing_framework_paths` rather than
/// hand-written strings, so this fails if anything re-adds an asset that a
/// fully-staged install does not have on disk.
///
/// It used to assert the opposite — that a fully-staged Bundle install still
/// reported the fragment as a gap, because `essential_files(Bundle)` listed it.
/// That listing is what armed the cwd-sourced restage of `$HOME`.
#[test]
fn a_fully_staged_bundle_install_reports_no_gap_and_no_restage() {
    let tmp = tempfile::tempdir().unwrap();
    let claude_dir = tmp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();
    write_layout_marker(&claude_dir, SourceLayout::Bundle).unwrap();
    for dir in essential_destinations(SourceLayout::Bundle) {
        fs::create_dir_all(claude_dir.join(dir)).unwrap();
    }
    fs::write(claude_dir.join("tools/statusline.sh"), "echo hi\n").unwrap();
    fs::write(tmp.path().join(".amplihack/CLAUDE.md"), "root\n").unwrap();
    let recipes = tmp.path().join(".amplihack/amplifier-bundle/recipes");
    fs::create_dir_all(&recipes).unwrap();
    for recipe in [
        "smart-orchestrator.yaml",
        "default-workflow.yaml",
        "investigation-workflow.yaml",
    ] {
        fs::write(recipes.join(recipe), "name: x\n").unwrap();
    }
    // Deliberately NOT staging the fragment: it is compiled in, so its absence
    // from disk must be a non-event.

    let missing = missing_framework_paths(&claude_dir).unwrap();
    assert!(
        missing.is_empty(),
        "the fragment must not be an essential file — listing it is what armed \
         a cwd-sourced restage of $HOME on every install. Got {missing:?}"
    );
    assert!(
        !framework_restage_needed(true, &missing),
        "no gap, no restage: {missing:?}"
    );
}
