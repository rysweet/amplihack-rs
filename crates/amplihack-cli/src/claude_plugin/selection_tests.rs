//! Publication contracts for the selected Claude inventory.

use super::*;
use amplihack_utils::list_selection::{DEFAULT_AGENTS, DEFAULT_SKILLS, ListSet};

fn all() -> ListSelection {
    ListSelection::new(ListSet::All, [])
}

fn agent_files(root: &Path) -> BTreeSet<String> {
    if !root.is_dir() {
        return BTreeSet::new();
    }
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries).unwrap();
    entries
        .into_iter()
        .filter_map(|(path, kind)| {
            (matches!(kind, TreeEntry::File) && path.extension().is_some_and(|ext| ext == "md"))
                .then(|| path.with_extension("").to_string_lossy().replace('\\', "/"))
        })
        .collect()
}

fn fixture(home: &Path) -> PathBuf {
    let staged = home.join(".amplihack/.claude");
    for name in ["pdf", "extra", "edited"] {
        let skill = staged.join("skills").join(name);
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), format!("{name}\n")).unwrap();
    }
    for name in ["core/builder", "core/guide", "guide"] {
        let agent = staged.join("agents").join(format!("{name}.md"));
        fs::create_dir_all(agent.parent().unwrap()).unwrap();
        fs::write(agent, format!("{name}\n")).unwrap();
    }
    staged
}

#[test]
fn real_bundle_publishes_exact_defaults_and_keeps_every_framework_entry() {
    let (_lock, home, _env) = tests::isolated_home();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle");
    let staged = home.path().join(".amplihack/.claude");
    for asset in ["skills", "agents"] {
        copy_dir_recursive(&source.join(asset), &staged.join(asset)).unwrap();
    }
    complete_indexed_skill_resources(&staged).unwrap();
    let skills_before = discoverable_skill_names(&staged.join("skills")).unwrap();
    let agents_before = agent_files(&staged.join("agents"));
    let report = ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.published, DEFAULT_SKILLS.map(str::to_owned).into());
    assert!(report.removed_skills.is_empty());
    assert!(report.removed_agents.is_empty());
    assert_eq!(
        discoverable_skill_names(&home.path().join(".claude/skills")).unwrap(),
        DEFAULT_SKILLS.map(str::to_owned).into()
    );
    assert_eq!(
        agent_files(&home.path().join(".claude/skills/amplihack/agents")),
        DEFAULT_AGENTS.map(str::to_owned).into()
    );
    assert_eq!(
        discoverable_skill_names(&staged.join("skills")).unwrap(),
        skills_before
    );
    assert_eq!(agent_files(&staged.join("agents")), agents_before);
    assert!(
        home.path()
            .join(".claude/skills/docx/ooxml/scripts/pack.sh")
            .is_file()
    );
    // The old reader rejects unknown fields and versions. It must still read
    // the exact skill record produced alongside the new agent record.
    let record = read_skill_ownership_manifest(&skill_ownership_manifest_path(&staged)).unwrap();
    assert_eq!(record.version, 2);
    let json: Value =
        serde_json::from_slice(&fs::read(skill_ownership_manifest_path(&staged)).unwrap()).unwrap();
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["plugin", "skills", "version"])
    );
    assert!(agents::record_path(&staged).is_file());
}

#[test]
fn extensions_publish_then_restore_identical_tool_assets() {
    let (_lock, home, _env) = tests::isolated_home();
    let staged = fixture(home.path());
    let selection = ListSelection::new(
        ListSet::Default,
        [
            "skill:extra".parse().unwrap(),
            "agent:core/guide".parse().unwrap(),
        ],
    );
    let root = home.path().join(".claude/skills");
    ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    let original = hash_directory_tree(&root).unwrap();
    ensure_claude_plugin_with_selection(&selection).unwrap();
    assert!(root.join("extra/SKILL.md").is_file());
    assert_eq!(
        agent_files(&root.join("amplihack/agents")),
        BTreeSet::from(["core/builder".into(), "core/guide".into()])
    );
    assert!(!root.join("amplihack/agents").is_symlink());
    let extended = hash_directory_tree(&root).unwrap();
    ensure_claude_plugin_with_selection(&selection).unwrap();
    assert_eq!(hash_directory_tree(&root).unwrap(), extended);
    let removed = ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    assert_eq!(removed.removed_skills, BTreeSet::from(["extra".into()]));
    assert_eq!(
        removed.removed_agents,
        BTreeSet::from(["core/guide".into()])
    );
    assert_eq!(hash_directory_tree(&root).unwrap(), original);
    assert!(staged.join("skills/extra/SKILL.md").is_file());
    ensure_claude_plugin_with_selection(&all()).unwrap();
    assert_eq!(
        agent_files(&root.join("amplihack/agents")),
        agent_files(&staged.join("agents"))
    );
}

#[test]
fn narrowing_preserves_edited_owned_skills_and_independent_skills() {
    let (_lock, home, _env) = tests::isolated_home();
    let staged = fixture(home.path());
    ensure_claude_plugin_with_selection(&all()).unwrap();
    let notice = staged.join("install/list-changes.txt");
    assert!(!notice.exists());
    let root = home.path().join(".claude/skills");
    fs::write(root.join("edited/SKILL.md"), "user changes\n").unwrap();
    fs::create_dir_all(root.join("independent")).unwrap();
    fs::write(root.join("independent/SKILL.md"), "independent\n").unwrap();
    let report = ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    assert_eq!(report.removed_skills, BTreeSet::from(["extra".into()]));
    assert_eq!(
        report
            .preserved
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["edited"]
    );
    assert_eq!(
        fs::read(root.join("edited/SKILL.md")).unwrap(),
        b"user changes\n"
    );
    assert_eq!(
        fs::read(root.join("independent/SKILL.md")).unwrap(),
        b"independent\n"
    );
    let saved_notice = fs::read(&notice).unwrap();
    let again = ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    assert!(again.removed_skills.is_empty());
    assert!(again.removed_agents.is_empty());
    assert_eq!(fs::read(&notice).unwrap(), saved_notice);
}

#[test]
fn narrowing_converts_link_copy_and_legacy_wrappers() {
    for layout in ["link", "copy", "legacy"] {
        let (_lock, home, _env) = tests::isolated_home();
        let staged = fixture(home.path());
        let plugin = home.path().join(".claude/skills/amplihack");
        if layout == "legacy" {
            tests::write_legacy_plugin_wrapper(&staged, &plugin);
        } else {
            ensure_claude_plugin_with_selection(&all()).unwrap();
            if layout == "copy" {
                remove_mirror(&plugin.join("agents")).unwrap();
                copy_dir_recursive(&staged.join("agents"), &plugin.join("agents")).unwrap();
                let path = skill_ownership_manifest_path(&staged);
                let mut record = read_skill_ownership_manifest(&path).unwrap();
                record.plugin.as_mut().unwrap().content_sha256 =
                    hash_directory_tree(&plugin).unwrap();
                write_skill_ownership_manifest(&path, &record).unwrap();
            }
        }
        let report = ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
        assert!(report.is_complete(), "{layout}: {report:?}");
        assert_eq!(
            agent_files(&plugin.join("agents")),
            BTreeSet::from(["core/builder".into()])
        );
        assert!(!plugin.join("skills").exists());
        assert!(staged.join("agents/core/guide.md").is_file());
    }
}

#[test]
fn materialised_index_resources_can_be_published_repeatedly() {
    let (_lock, home, _env) = tests::isolated_home();
    let staged = fixture(home.path());
    let skills = staged.join("skills");
    for link in KNOWN_SKILL_LINKS {
        let skill = link.path.split('/').next().unwrap();
        fs::create_dir_all(skills.join(skill)).unwrap();
        fs::write(skills.join(skill).join("SKILL.md"), skill).unwrap();
        let target = skills.join(link.canonical_target);
        if link.canonical_target.ends_with(".md") {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, "resource\n").unwrap();
        } else {
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("resource.txt"), "resource\n").unwrap();
        }
    }
    complete_indexed_skill_resources(&staged).unwrap();
    let before = hash_directory_tree(&skills).unwrap();
    complete_indexed_skill_resources(&staged).unwrap();
    assert_eq!(hash_directory_tree(&skills).unwrap(), before);
    ensure_claude_plugin_with_selection(&all()).unwrap();
    for link in KNOWN_SKILL_LINKS {
        let indexed = skills.join(link.path);
        let published = home.path().join(".claude/skills").join(link.path);
        assert!(indexed.exists(), "{}", link.path);
        if indexed.is_file() {
            assert_eq!(fs::read(indexed).unwrap(), fs::read(published).unwrap());
        } else {
            assert_eq!(
                hash_directory_tree(&indexed).unwrap(),
                hash_directory_tree(&published).unwrap()
            );
        }
    }
}

#[test]
fn configured_selection_is_used_by_the_public_entry_point() {
    let (_lock, home, _env) = tests::isolated_home();
    fixture(home.path());
    ensure_claude_plugin_installed().unwrap();
    let root = home.path().join(".claude/skills");
    assert_eq!(
        discoverable_skill_names(&root).unwrap(),
        BTreeSet::from(["pdf".into()])
    );
    fs::write(
        home.path().join(".amplihack/config.toml"),
        "[lists]\nset = 'all'\n",
    )
    .unwrap();
    ensure_claude_plugin_installed().unwrap();
    assert_eq!(
        discoverable_skill_names(&root).unwrap(),
        BTreeSet::from(["pdf".into(), "extra".into(), "edited".into()])
    );
}

#[cfg(unix)]
#[test]
fn unchanged_default_wrapper_and_records_are_not_rewritten() {
    let (_lock, home, _env) = tests::isolated_home();
    let staged = fixture(home.path());
    ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    let plugin = home.path().join(".claude/skills/amplihack");
    let wrapper_before = tests::observe_tree(&plugin);
    let staged_before = tests::observe_tree(&staged);
    ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    assert_eq!(tests::observe_tree(&plugin), wrapper_before);
    assert_eq!(tests::observe_tree(&staged), staged_before);
}

#[test]
fn uninstall_removes_the_separate_agent_record() {
    let (_lock, home, _env) = tests::isolated_home();
    let staged = fixture(home.path());
    ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    assert!(agents::record_path(&staged).is_file());
    remove_managed_claude_plugin().unwrap();
    assert!(!agents::record_path(&staged).exists());
    assert!(!home.path().join(".claude/skills/amplihack").exists());
    assert!(staged.join("agents/core/guide.md").is_file());
}

#[test]
fn changed_wrapper_agents_and_invalid_agent_records_preserve_the_live_wrapper() {
    let (_lock, home, _env) = tests::isolated_home();
    let staged = fixture(home.path());
    ensure_claude_plugin_with_selection(&ListSelection::default()).unwrap();
    let plugin = home.path().join(".claude/skills/amplihack");
    let bytes = fs::read(agents::record_path(&staged)).unwrap();
    for invalid in [b"{".as_slice(), br#"{"version":2,"agents":[]}"#.as_slice()] {
        fs::write(agents::record_path(&staged), invalid).unwrap();
        let before = hash_directory_tree(&plugin).unwrap();
        let report = ensure_claude_plugin_with_selection(&all()).unwrap();
        assert!(report.wrapper_error.is_some());
        assert_eq!(hash_directory_tree(&plugin).unwrap(), before);
    }
    fs::write(agents::record_path(&staged), bytes).unwrap();
    fs::write(plugin.join("agents/core/builder.md"), "user edit\n").unwrap();
    let before = hash_directory_tree(&plugin).unwrap();
    let report = ensure_claude_plugin_with_selection(&all()).unwrap();
    assert!(report.wrapper_error.is_some());
    assert_eq!(hash_directory_tree(&plugin).unwrap(), before);
}
