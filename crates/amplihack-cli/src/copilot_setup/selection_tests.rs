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
fn real_bundle_publishes_exact_defaults_as_flat_markdown() {
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
    assert!(!home.path().join("skills/docx/ooxml").exists());
    assert!(
        fs_helpers::walk_files(&home.path().join("skills"))
            .unwrap()
            .iter()
            .all(|path| path.extension().is_some_and(|ext| ext == "md"))
    );
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
fn extensions_use_flat_markdown_and_restore_tool_bytes() {
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
    assert!(
        !home
            .path()
            .join("skills/extra/resources/input.bin")
            .exists()
    );
    assert_eq!(
        fs::read(home.path().join("skills/extra/SKILL.md")).unwrap(),
        b"extra"
    );
    assert!(!home.path().join("agents/amplihack/core--guide.md").exists());
    assert!(home.path().join("agents/amplihack/guide.md").is_file());
    let extended = snapshot(home.path());
    publish_tool_assets(source.path(), home.path(), &selection).unwrap();
    assert_eq!(snapshot(home.path()), extended);
    publish_tool_assets(source.path(), home.path(), &default).unwrap();
    assert_eq!(snapshot(home.path()), original);
}

#[test]
fn narrowing_rewrites_selected_markdown_and_clears_bundle_named_folders() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fixture(source.path());
    write(source.path(), "skills/edited/SKILL.md", "original");
    write(source.path(), "skills/shared/reference.md", "resource only");
    publish_tool_assets(source.path(), home.path(), &all()).unwrap();
    let notice = source.path().join("install/list-changes.txt");
    assert!(!notice.exists());
    write(home.path(), "skills/edited/SKILL.md", "user edit");
    write(home.path(), "skills/pdf/SKILL.md", "older selected skill");
    write(home.path(), "skills/edited/notes.txt", "keep non-markdown");
    write(
        home.path(),
        "skills/edited/nested/notes.md",
        "keep nested markdown",
    );
    write(home.path(), "skills/independent/SKILL.md", "independent");
    let report =
        publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert_eq!(
        report.removed_skills,
        BTreeSet::from([
            "category".to_owned(),
            "edited".to_owned(),
            "shared".to_owned()
        ])
    );
    assert_eq!(report.removed_agents, BTreeSet::from(["guide".to_owned()]));
    assert!(report.preserved.is_empty());
    assert!(report.skipped.is_empty());
    assert!(!home.path().join("skills/edited/SKILL.md").exists());
    assert_eq!(
        fs::read(home.path().join("skills/edited/notes.txt")).unwrap(),
        b"keep non-markdown"
    );
    assert_eq!(
        fs::read(home.path().join("skills/edited/nested/notes.md")).unwrap(),
        b"keep nested markdown"
    );
    assert_eq!(
        fs::read(home.path().join("skills/pdf/SKILL.md")).unwrap(),
        b"pdf"
    );
    assert_eq!(
        fs::read(home.path().join("skills/independent/SKILL.md")).unwrap(),
        b"independent"
    );
    let report =
        publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert!(!notice.exists());
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
fn legacy_flattened_skills_are_cleared_regardless_of_previous_bytes() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(source.path(), "skills/extra/SKILL.md", "extra");
    write(source.path(), "skills/edited/SKILL.md", "original");
    write(home.path(), "skills/extra/SKILL.md", "extra");
    write(home.path(), "skills/edited/SKILL.md", "user content");
    write(home.path(), "skills/independent/SKILL.md", "independent");
    let report =
        publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert_eq!(
        report.removed_skills,
        BTreeSet::from(["extra".to_owned(), "edited".to_owned()])
    );
    assert!(!home.path().join("skills/extra").exists());
    assert!(!home.path().join("skills/edited").exists());
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
fn default_to_all_removes_leaf_folders_and_matches_historical_staging() {
    let source = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fixture(source.path());
    write(
        source.path(),
        "skills/category/reviewing-code/SKILL.md",
        "review",
    );
    write(source.path(), "skills/docx/SKILL.md", "docx");
    write(
        source.path(),
        "skills/common/ooxml/reference.md",
        "shared support",
    );
    write(
        source.path(),
        "skills/outside-in-testing/SKILL.md",
        "outside-in",
    );
    write(source.path(), "skills/qa-team/README.md", "qa reference");
    let historical = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(source.path().join("skills")).unwrap() {
        let entry = entry.unwrap();
        let destination = historical.path().join("skills").join(entry.file_name());
        fs_helpers::reset_markdown_dir(&destination).unwrap();
        fs_helpers::flatten_markdown_tree(&entry.path(), &destination).unwrap();
    }
    fs_helpers::reset_markdown_dir(&historical.path().join("agents/amplihack")).unwrap();
    fs_helpers::flatten_markdown_tree(
        &source.path().join("agents"),
        &historical.path().join("agents/amplihack"),
    )
    .unwrap();
    // The index completes links that historical installation omitted.
    crate::claude_plugin::complete_indexed_skill_resources(source.path()).unwrap();
    publish_tool_assets(source.path(), home.path(), &ListSelection::default()).unwrap();
    assert!(home.path().join("skills/reviewing-code/SKILL.md").exists());
    publish_tool_assets(source.path(), home.path(), &all()).unwrap();
    assert!(!home.path().join("skills/reviewing-code").exists());
    assert_eq!(snapshot(home.path()), snapshot(historical.path()));
    assert!(!home.path().join("copilot-skills-manifest.json").exists());
}

#[test]
fn selected_agents_with_the_same_leaf_use_historical_collision_behaviour() {
    let source = tempfile::tempdir().unwrap();
    let default_home = tempfile::tempdir().unwrap();
    let all_home = tempfile::tempdir().unwrap();
    write(source.path(), "agents/amplihack/core/guide.md", "core");
    write(source.path(), "agents/amplihack/guide.md", "top level");
    let selected = ListSelection::new(
        ListSet::Default,
        [
            "agent:core/guide".parse().unwrap(),
            "agent:guide".parse().unwrap(),
        ],
    );
    publish_tool_assets(source.path(), default_home.path(), &selected).unwrap();
    publish_tool_assets(source.path(), all_home.path(), &all()).unwrap();
    assert_eq!(snapshot(default_home.path()), snapshot(all_home.path()));
    assert_eq!(snapshot(default_home.path()).len(), 1);
}
