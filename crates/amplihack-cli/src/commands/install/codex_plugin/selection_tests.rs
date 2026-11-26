//! Selection publishes complete selected skills and shared resources.
use super::*;
use amplihack_utils::list_selection::{DEFAULT_AGENTS, DEFAULT_SKILLS};
use std::collections::BTreeSet;

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn narrowing_counts_personas_as_agents_and_other_skill_folders_as_skills() {
    let before = package(&source(), &ListSelection::new(ListSet::All, []));
    let after = package(&source(), &ListSelection::default());
    let before = before.path().join("package");
    let after = after.path().join("package");
    let removed = transaction::removed_inventory(&before, &after).unwrap();
    let old = inventory(&before);
    let new = inventory(&after);
    let removed_personas = old
        .difference(&new)
        .filter(|name| name.starts_with("amplihack-persona-"))
        .count();
    assert!(removed_personas > 0);
    assert_eq!(removed.removed_agents.len(), removed_personas);
    assert_eq!(
        removed.removed_skills.len() + removed.removed_agents.len(),
        old.difference(&new).count()
    );
    let unchanged = transaction::removed_inventory(&after, &after).unwrap();
    assert!(unchanged.removed_agents.is_empty());
    assert!(unchanged.removed_skills.is_empty());
    let fresh = transaction::removed_inventory(&after.join("absent"), &after).unwrap();
    assert!(fresh.removed_agents.is_empty());
    assert!(fresh.removed_skills.is_empty());
}

fn package(source: &Path, selection: &ListSelection) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join("hooks-binary");
    fs::write(&hooks, b"hooks").unwrap();
    build_with_selection(source, &hooks, &dir.path().join("package"), selection).unwrap();
    dir
}

fn inventory(root: &Path) -> BTreeSet<String> {
    fs::read_dir(root.join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("SKILL.md").is_file())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn defaults() -> BTreeSet<String> {
    DEFAULT_SKILLS
        .iter()
        .map(|name| (*name).to_owned())
        .chain(
            DEFAULT_AGENTS
                .iter()
                .map(|name| format!("amplihack-persona-{}", name.replace('/', "-"))),
        )
        .collect()
}

fn definition_count(root: &Path) -> usize {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                definition_count(&entry.path())
            } else {
                usize::from(entry.file_name() == "SKILL.md")
            }
        })
        .sum()
}

#[test]
fn default_package_has_exactly_the_regular_and_persona_skills() {
    let dir = package(&source(), &ListSelection::default());
    let root = dir.path().join("package");
    assert_eq!(inventory(&root), defaults());
    assert_eq!(definition_count(&root.join("skills")), 42);
    assert_eq!(
        fs::read(root.join("bin/hook")).unwrap(),
        HOOK_WRAPPER.as_bytes()
    );
    assert_eq!(
        fs::read(root.join("bin/amplihack-hooks")).unwrap(),
        b"hooks"
    );
    assert!(root.join("skills/reviewing-code/SKILL.md").is_file());
    assert!(root.join("skills/docx/ooxml/scripts").is_dir());
    assert!(root.join("skills/pptx/ooxml/scripts").is_dir());
}

#[test]
fn default_extensions_include_analysts_and_agent_paths() {
    let selection = ListSelection::new(
        ListSet::Default,
        [
            "skill:economist-analyst".parse().unwrap(),
            "agent:core/guide".parse().unwrap(),
        ],
    );
    let dir = package(&source(), &selection);
    let mut expected = defaults();
    expected.extend([
        "economist-analyst".to_owned(),
        "amplihack-persona-core-guide".to_owned(),
    ]);
    assert_eq!(inventory(&dir.path().join("package")), expected);
}

#[test]
fn full_package_retains_commands_personas_and_analyst_exclusions() {
    let dir = package(&source(), &ListSelection::new(ListSet::All, []));
    let root = dir.path().join("package");
    let entries = inventory(&root);
    assert!(defaults().is_subset(&entries));
    assert_eq!(
        entries
            .iter()
            .filter(|name| name.starts_with("amplihack-command-"))
            .count(),
        18
    );
    assert_eq!(
        entries
            .iter()
            .filter(|name| name.starts_with("amplihack-persona-"))
            .count(),
        41
    );
    assert!(entries.len() > 100);
    assert!(!root.join("skills/economist-analyst").exists());
    assert!(root.join("bin/hook").is_file());
    let enabled = package(
        &source(),
        &ListSelection::new(ListSet::All, ["skill:economist-analyst".parse().unwrap()]),
    );
    assert_eq!(
        digest(&root).unwrap(),
        digest(&enabled.path().join("package")).unwrap()
    );
}

#[test]
fn adding_then_removing_extensions_restores_package_bytes() {
    let initial = package(&source(), &ListSelection::default());
    let enabled = package(
        &source(),
        &ListSelection::new(
            ListSet::Default,
            ["skill:economist-analyst".parse().unwrap()],
        ),
    );
    let restored = package(&source(), &ListSelection::default());
    let initial_digest = digest(&initial.path().join("package")).unwrap();
    assert_ne!(
        initial_digest,
        digest(&enabled.path().join("package")).unwrap()
    );
    assert_eq!(
        initial_digest,
        digest(&restored.path().join("package")).unwrap()
    );
}

#[test]
fn unlisted_skills_still_require_valid_canonical_definitions() {
    let source = tempfile::tempdir().unwrap();
    let skill = source.path().join("amplifier-bundle/skills/unlisted");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: invalid name\ndescription: fixture\n---\n",
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let hooks = output.path().join("hooks-binary");
    fs::write(&hooks, b"hooks").unwrap();
    let staged = output.path().join("package");
    assert!(
        build_with_selection(source.path(), &hooks, &staged, &ListSelection::default()).is_err()
    );
    assert!(!staged.exists());
}

#[cfg(unix)]
#[test]
fn selected_skill_can_read_shared_resources_without_packaging_unselected_skills() {
    let source = tempfile::tempdir().unwrap();
    let skills = source.path().join("amplifier-bundle/skills");
    for name in ["qa-team", "unlisted", "category/another-unlisted"] {
        fs::create_dir_all(skills.join(name)).unwrap();
        fs::write(
            skills.join(name).join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: resource fixture\n---\n",
                name.rsplit('/').next().unwrap()
            ),
        )
        .unwrap();
    }
    fs::create_dir_all(skills.join("common")).unwrap();
    fs::write(skills.join("common/resource.txt"), b"shared resource").unwrap();
    fs::write(skills.join("unlisted/resource.txt"), b"private to skill").unwrap();
    std::os::unix::fs::symlink(
        "../common/resource.txt",
        skills.join("qa-team/resource.txt"),
    )
    .unwrap();
    for set in [ListSet::Default, ListSet::All] {
        let dir = package(source.path(), &ListSelection::new(set, []));
        let root = dir.path().join("package");
        assert_eq!(
            fs::read(root.join("skills/qa-team/resource.txt")).unwrap(),
            b"shared resource"
        );
        assert_eq!(
            fs::read(root.join("skills/common/resource.txt")).unwrap(),
            b"shared resource"
        );
        if set == ListSet::Default {
            assert_eq!(inventory(&root), BTreeSet::from(["qa-team".to_owned()]));
            assert!(!root.join("skills/unlisted").exists());
            assert!(!root.join("skills/category/another-unlisted").exists());
        } else {
            assert_eq!(
                fs::read(root.join("skills/unlisted/resource.txt")).unwrap(),
                b"private to skill"
            );
            assert!(
                root.join("skills/category/another-unlisted/SKILL.md")
                    .is_file()
            );
        }
    }
    assert!(skills.join("unlisted/SKILL.md").is_file());
}

#[cfg(unix)]
#[test]
fn hook_wrapper_keeps_protocol_subcommands_and_argument_boundaries_in_both_modes() {
    use std::os::unix::fs::PermissionsExt;
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("amplifier-bundle/skills")).unwrap();
    for set in [ListSet::Default, ListSet::All] {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks-binary");
        crate::test_support::write_executable(
            &hooks,
            "#!/bin/sh\nprintf '%s\\n' \"$AMPLIHACK_HOOK_PROTOCOL\" \"$@\"\n",
        );
        let root = dir.path().join("package");
        build_with_selection(source.path(), &hooks, &root, &ListSelection::new(set, [])).unwrap();
        assert_ne!(
            fs::metadata(root.join("bin/hook"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        for command in ["session-start", "pre-tool-use", "post-tool-use", "stop"] {
            let output = Command::new(root.join("bin/hook"))
                .args([command, "two words", ""])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                output.stdout,
                format!("codex\n{command}\ntwo words\n\n").as_bytes()
            );
        }
    }
}
