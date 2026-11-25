use super::*;
use amplihack_utils::list_selection::{DEFAULT_AGENTS, DEFAULT_SKILLS, ListSelection, ListSet};
use std::collections::{BTreeMap, BTreeSet};

fn all() -> ListSelection {
    ListSelection::new(ListSet::All, [])
}

fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn fixture(root: &Path) {
    write(root, "skills/pdf/SKILL.md", "pdf");
    write(root, "skills/category/extra/SKILL.md", "extra");
    write(root, "skills/category/extra/resources/input.bin", [0, 255]);
    write(root, "agents/amplihack/core/reviewer.md", "reviewer");
    write(root, "agents/amplihack/core/guide.md", "core guide");
    write(root, "agents/amplihack/guide.md", "guide");
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs_helpers::walk_files(root)
        .unwrap()
        .into_iter()
        .map(|path| {
            (
                path.strip_prefix(root).unwrap().to_path_buf(),
                fs::read(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn real_bundle_publishes_exact_defaults_with_complete_resources() {
    let home = tempfile::tempdir().unwrap();
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle");
    publish_tool_assets(&bundle, home.path(), &ListSelection::default()).unwrap();
    let skills: BTreeSet<_> = fs::read_dir(home.path().join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(skills, DEFAULT_SKILLS.map(str::to_owned).into());
    let agents: BTreeSet<_> = fs_helpers::walk_files(&home.path().join("agents"))
        .unwrap()
        .into_iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        agents,
        DEFAULT_AGENTS
            .map(|path| format!("{}.md", path.rsplit('/').next().unwrap()))
            .into()
    );
    assert!(home.path().join("skills/reviewing-code/SKILL.md").is_file());
    assert!(home.path().join("skills/docx/ooxml").is_dir());
    assert!(!home.path().join("instructions.md").exists());
    assert!(!home.path().join("config.json").exists());
}

#[test]
fn full_mode_preserves_the_established_flattened_inventory() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fixture(source.path());
    publish_tool_assets(source.path(), home.path(), &all()).unwrap();
    let assets = snapshot(&home.path().join("skills"));
    assert_eq!(
        assets,
        BTreeMap::from([
            (PathBuf::from("pdf/SKILL.md"), b"pdf".to_vec()),
            (PathBuf::from("category/SKILL.md"), b"extra".to_vec()),
        ])
    );
    let agents = snapshot(&home.path().join("agents"));
    assert_eq!(agents.len(), 2);
    assert!(agents.contains_key(Path::new("amplihack/reviewer.md")));
    assert!(agents.contains_key(Path::new("amplihack/guide.md")));
    let first = snapshot(home.path());
    publish_tool_assets(source.path(), home.path(), &all()).unwrap();
    assert_eq!(snapshot(home.path()), first);
}

#[test]
fn extensions_preserve_resources_and_restore_tool_bytes() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fixture(source.path());
    let default = ListSelection::default();
    publish_tool_assets(source.path(), home.path(), &default).unwrap();
    let original = snapshot(home.path());
    let selection = ListSelection::new(
        ListSet::Default,
        [
            "skill:extra".parse().unwrap(),
            "agent:guide".parse().unwrap(),
            "agent:core/guide".parse().unwrap(),
        ],
    );
    publish_tool_assets(source.path(), home.path(), &selection).unwrap();
    assert_eq!(
        fs::read(home.path().join("skills/extra/resources/input.bin")).unwrap(),
        [0, 255]
    );
    assert!(
        home.path()
            .join("agents/amplihack/core--guide.md")
            .is_file()
    );
    assert!(home.path().join("agents/amplihack/guide.md").is_file());
    let extended = snapshot(home.path());
    publish_tool_assets(source.path(), home.path(), &selection).unwrap();
    assert_eq!(snapshot(home.path()), extended);
    publish_tool_assets(source.path(), home.path(), &default).unwrap();
    assert_eq!(snapshot(home.path()), original);
}

#[test]
fn narrowing_removes_owned_entries_and_preserves_user_content() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fixture(source.path());
    write(source.path(), "skills/edited/SKILL.md", "original");
    write(source.path(), "skills/shared/reference.md", "resource only");
    publish_tool_assets(source.path(), home.path(), &all()).unwrap();
    let notice = source.path().join("install/list-changes.txt");
    assert!(!notice.exists());
    write(home.path(), "skills/edited/SKILL.md", "user edit");
    write(home.path(), "skills/independent/SKILL.md", "independent");
    let report =
        publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert_eq!(
        report.removed_skills,
        BTreeSet::from(["category".to_owned()])
    );
    assert_eq!(report.removed_agents, BTreeSet::from(["guide".to_owned()]));
    assert_eq!(
        report
            .preserved
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["edited"]
    );
    assert_eq!(
        fs::read(home.path().join("skills/edited/SKILL.md")).unwrap(),
        b"user edit"
    );
    assert_eq!(
        fs::read(home.path().join("skills/independent/SKILL.md")).unwrap(),
        b"independent"
    );
    let saved_notice = fs::read(&notice).unwrap();
    let report =
        publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert_eq!(fs::read(&notice).unwrap(), saved_notice);
    assert!(report.removed_skills.is_empty());
    assert!(report.removed_agents.is_empty());
    assert!(report.preserved.is_empty());
    assert!(
        source
            .path()
            .join("agents/amplihack/core/guide.md")
            .is_file()
    );
}

#[test]
fn legacy_flattened_skills_are_adopted_only_when_unchanged() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(source.path(), "skills/extra/SKILL.md", "extra");
    write(source.path(), "skills/edited/SKILL.md", "original");
    write(home.path(), "skills/extra/SKILL.md", "extra");
    write(home.path(), "skills/edited/SKILL.md", "user content");
    write(home.path(), "skills/independent/SKILL.md", "independent");
    let report =
        publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert_eq!(report.removed_skills, BTreeSet::from(["extra".to_owned()]));
    assert!(!home.path().join("skills/extra").exists());
    assert_eq!(
        fs::read(home.path().join("skills/edited/SKILL.md")).unwrap(),
        b"user content"
    );
    assert_eq!(
        fs::read(home.path().join("skills/independent/SKILL.md")).unwrap(),
        b"independent"
    );
}

#[test]
fn owned_instruction_section_is_removed_preserving_surrounding_bytes() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("instructions.md");
    let before = "User instructions  \r\n\n";
    let after = "\r\nMore instructions\n\n";
    fs::write(
        &path,
        format!("{before}{INSTRUCTIONS_MARKER_START}\nowned\n{INSTRUCTIONS_MARKER_END}{after}"),
    )
    .unwrap();
    hooks::remove_copilot_instructions(home.path()).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        format!("{before}{after}").as_bytes()
    );
    let once = fs::read(&path).unwrap();
    hooks::remove_copilot_instructions(home.path()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), once);
}

#[test]
fn instruction_migration_removes_empty_file_and_leaves_unowned_files_alone() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("instructions.md");
    hooks::remove_copilot_instructions(home.path()).unwrap();
    assert!(!path.exists());
    fs::write(
        &path,
        format!("{INSTRUCTIONS_MARKER_START}\nowned\n{INSTRUCTIONS_MARKER_END}\n"),
    )
    .unwrap();
    hooks::remove_copilot_instructions(home.path()).unwrap();
    assert!(!path.exists());
    for body in [
        "user only\n",
        INSTRUCTIONS_MARKER_START,
        INSTRUCTIONS_MARKER_END,
    ] {
        fs::write(&path, body).unwrap();
        hooks::remove_copilot_instructions(home.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), body.as_bytes());
    }
    write(home.path(), "copilot-instructions.md", "leave this file");
    hooks::remove_copilot_instructions(home.path()).unwrap();
    assert_eq!(
        fs::read(home.path().join("copilot-instructions.md")).unwrap(),
        b"leave this file"
    );
}

#[test]
fn legacy_upgrade_preserves_selected_edits_and_does_not_readopt_ownership() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    for name in ["lsp-setup", "default-workflow"] {
        write(
            source.path(),
            &format!("skills/{name}/SKILL.md"),
            "old release",
        );
        write(
            home.path(),
            &format!("skills/{name}/SKILL.md"),
            "old release",
        );
    }
    let edited = home.path().join("skills/default-workflow/SKILL.md");
    fs::write(&edited, "user edit").unwrap();
    staging::adopt_previous_skill_ownership(&source.path().join("skills"), home.path()).unwrap();
    let record = home.path().join("copilot-skills-manifest.json");
    let adopted = fs::read(&record).unwrap();
    for name in ["lsp-setup", "default-workflow"] {
        write(
            source.path(),
            &format!("skills/{name}/SKILL.md"),
            "new release",
        );
    }
    // A retry must retain the old digest, which authorizes the upgrade, rather
    // than comparing the destination to the incoming definitions again.
    staging::adopt_previous_skill_ownership(&source.path().join("skills"), home.path()).unwrap();
    assert_eq!(fs::read(&record).unwrap(), adopted);
    let report = staging::stage_skills(
        &source.path().join("skills"),
        home.path(),
        &ListSelection::default(),
    )
    .unwrap();
    assert_eq!(report.published, BTreeSet::from(["lsp-setup".to_owned()]));
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].name, "default-workflow");
    assert_eq!(fs::read(&edited).unwrap(), b"user edit");
    assert_eq!(
        fs::read(home.path().join("skills/lsp-setup/SKILL.md")).unwrap(),
        b"new release"
    );
}
