//! Issue #1449 — a shadowed skill must be reported, never counted as published.
//!
//! `install` prints one line per Claude destination (#1447). That line was
//! computed from what is *discoverable* under `~/.claude/skills`, which cannot
//! tell amplihack's copy from someone else's: a directory that shadows a
//! canonical skill has its own `SKILL.md`, so the name is discoverable and the
//! shortfall check saw nothing wrong. On the reporting host that produced a
//! green install whose result was a session with no amplihack skills at all.
//!
//! The check is now provenance-based: a skill amplihack skipped is named in
//! the install output, and the count of published skills excludes it.

use super::*;
use std::fs;
use std::path::Path;

fn write_skill(skills_root: &Path, name: &str, body: &str) {
    let dir = skills_root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

/// Run the `skills` row of `CLAUDE_DESTINATIONS` — the same code path the
/// install output is built from.
fn publish_skills_row(repo_root: &Path) -> claude_publication::Publication {
    let row = claude_publication::CLAUDE_DESTINATIONS
        .iter()
        .find(|destination| destination.label == "skills")
        .expect("the skills destination must be enumerated");
    (row.publish)(repo_root).unwrap()
}

#[test]
fn a_shadowed_skill_is_named_in_the_install_output_and_not_counted_as_published() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staged = temp.path().join(".amplihack/.claude/skills");
    for name in ["anthropologist-analyst", "crusty-old-engineer"] {
        write_skill(&staged, name, &format!("---\nname: {name}\n---\nbundled\n"));
    }
    // The reporting host's condition: a stale directory of the same name,
    // written months earlier by something else, with drifted content.
    let shadow = temp.path().join(".claude/skills/anthropologist-analyst");
    fs::create_dir_all(&shadow).unwrap();
    fs::write(shadow.join("SKILL.md"), "stale copy\n").unwrap();

    let published = publish_skills_row(temp.path());

    crate::test_support::restore_home(previous);

    assert!(
        published.detail.contains("anthropologist-analyst"),
        "the install line must name the skill a session will not get: {}",
        published.detail
    );
    assert!(
        published.detail.starts_with("⚠️"),
        "a shadowed skill must not read as a win: {}",
        published.detail
    );
    assert_eq!(
        published.feature.as_deref(),
        Some("1 Claude Code skills (1 unavailable)"),
        "the summary must count only what amplihack actually published"
    );
    assert_eq!(
        fs::read_to_string(shadow.join("SKILL.md")).unwrap(),
        "stale copy\n",
        "the shadowing directory must never be overwritten"
    );
    assert!(
        temp.path()
            .join(".claude/skills/crusty-old-engineer/SKILL.md")
            .is_file(),
        "every non-colliding skill must still be published (#1449)"
    );
}
