//! Issue #1438 — `amplihack install` must publish skills where Claude reads them.
//!
//! `copytree_manifest` copies `amplifier-bundle/skills` into the *staging*
//! tree at `~/.amplihack/.claude/skills` and prints `✅ Copied skills ->
//! skills`. Claude Code never reads that path: it discovers user skills as
//! direct children of `~/.claude/skills`. Before this fix nothing in the
//! install path wrote that directory — the only code that does
//! (`claude_plugin::ensure_claude_plugin_installed`) was reachable solely from
//! `bootstrap::prepare_launcher("claude")`, which additionally returns early
//! in non-interactive mode. So a skill added to the bundle stayed invisible to
//! every Claude session until some later interactive `amplihack claude` launch
//! happened to run the sync, while `install` reported success and exited 0.
//!
//! The assertion below is deliberately an **invariant**, not an action: the set
//! of skill names published under `~/.claude/skills` must equal the set staged
//! under `~/.amplihack/.claude/skills`. Asserting that install printed a
//! success line would not have caught this, and would not catch the next
//! asset type that goes missing the same way.

use super::helpers::*;
use super::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The three skills from the issue's measurement: present in the bundle,
/// absent from `~/.claude/skills` on the reporting host.
const NEW_BUNDLE_SKILLS: &[&str] = &[
    "auto-drive-to-merge",
    "npe-hunting-workflow",
    "repository-oom-audit",
];

/// Skill names discoverable under `root`, using Claude Code's own rule: a
/// directory is a skill when it contains a `SKILL.md`.
fn skill_names(root: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, found: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut is_skill = false;
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_file() && entry.file_name() == "SKILL.md" {
                is_skill = true;
            } else if file_type.is_dir() {
                walk(&entry.path(), found);
            }
        }
        if is_skill && let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
            found.insert(name.to_owned());
        }
    }
    let mut found = BTreeSet::new();
    walk(root, &mut found);
    found
}

fn write_skill(skills_root: &Path, name: &str) {
    let dir = skills_root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: fixture skill\n---\n\nbody\n"),
    )
    .unwrap();
}

#[test]
fn local_install_publishes_every_staged_skill_into_the_claude_skills_dir() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");
    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    create_source_repo(temp.path());
    let bundle_skills = temp.path().join("amplifier-bundle/skills");
    for name in NEW_BUNDLE_SKILLS {
        write_skill(&bundle_skills, name);
    }

    let result = local_install(temp.path(), None);

    if let Some(value) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", value) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(value) = prev_path {
        unsafe { std::env::set_var("PATH", value) };
    }
    crate::test_support::restore_home(previous);

    result.unwrap();

    let staged = skill_names(&temp.path().join(".amplihack/.claude/skills"));
    let published = skill_names(&temp.path().join(".claude/skills"));

    assert!(
        staged.is_superset(&NEW_BUNDLE_SKILLS.iter().map(|s| (*s).to_owned()).collect()),
        "fixture is broken: the bundle skills were not staged at all, got {staged:?}"
    );
    assert_eq!(
        published,
        staged,
        "every skill staged under ~/.amplihack/.claude/skills must also be published \
         under ~/.claude/skills, which is the only directory Claude Code reads (#1438). \
         missing from ~/.claude/skills: {:?}",
        staged.difference(&published).collect::<Vec<_>>()
    );
}
