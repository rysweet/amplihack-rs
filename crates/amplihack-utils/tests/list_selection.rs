use amplihack_utils::list_selection::{
    DEFAULT_AGENTS, DEFAULT_SKILLS, EntryId, EntryKind, ListSelection, ListSet,
};
use std::collections::BTreeSet;
use std::path::Path;

#[test]
fn every_default_entry_exists_once_in_the_bundle() {
    fn skills(root: &Path, found: &mut Vec<String>) {
        if root.join("SKILL.md").is_file() {
            found.push(root.file_name().unwrap().to_str().unwrap().to_owned());
        }
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                skills(&entry.path(), found);
            }
        }
    }
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle");
    let mut found = Vec::new();
    skills(&bundle.join("skills"), &mut found);
    assert_eq!(DEFAULT_SKILLS.iter().collect::<BTreeSet<_>>().len(), 30);
    assert_eq!(DEFAULT_AGENTS.iter().collect::<BTreeSet<_>>().len(), 12);
    for name in DEFAULT_SKILLS {
        assert_eq!(
            found.iter().filter(|entry| *entry == name).count(),
            1,
            "{name}"
        );
    }
    for name in DEFAULT_AGENTS {
        assert!(bundle.join("agents").join(format!("{name}.md")).is_file());
    }
}

#[test]
fn absent_settings_select_only_the_default_entries() {
    let selection = ListSelection::default();
    assert_eq!(selection.set, ListSet::Default);
    assert!(selection.enabled().is_empty());
    for name in DEFAULT_SKILLS {
        assert!(selection.includes_skill(name));
    }
    for name in DEFAULT_AGENTS {
        assert!(selection.includes_agent(name));
    }
    assert!(!selection.includes_skill("economist-analyst"));
    assert!(!selection.includes_agent("core/guide"));
    assert!(!selection.includes_agent("guide"));
    assert!(!selection.includes_agent("tla-plus-expert"));
}

#[test]
fn explicit_extensions_are_typed_and_preserve_agent_paths() {
    let selection = ListSelection::new(
        ListSet::Default,
        [
            "skill:verus-expert",
            "agent:core/guide",
            "skill:economist-analyst",
        ]
        .map(|name| name.parse().unwrap()),
    );
    assert!(selection.includes_skill("verus-expert"));
    assert!(!selection.includes_agent("specialized/verus-expert"));
    assert!(selection.includes_agent("core/guide"));
    assert!(!selection.includes_agent("guide"));
    assert!(selection.includes_skill("economist-analyst"));
}

#[test]
fn specialised_workflow_skills_remain_available_as_extensions_and_in_all_mode() {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
    for name in ["auto-drive-to-merge", "claude-profile-rotation"] {
        assert!(bundle.join(name).join("SKILL.md").is_file());
        assert!(!ListSelection::default().includes_skill(name));
        let extended =
            ListSelection::new(ListSet::Default, [format!("skill:{name}").parse().unwrap()]);
        assert!(extended.includes_skill(name));
        assert!(ListSelection::new(ListSet::All, []).includes_skill(name));
    }
}

#[test]
fn extensions_survive_all_mode_and_duplicates_have_no_effect() {
    let mut selection = ListSelection::new(
        ListSet::All,
        ["skill:verus-expert", "skill:verus-expert", "skill:pdf"].map(|name| name.parse().unwrap()),
    );
    assert_eq!(selection.enabled().len(), 1);
    assert!(selection.includes_agent("guide"));
    assert!(selection.includes_skill("economist-analyst"));
    selection.set = ListSet::Default;
    assert!(selection.includes_skill("verus-expert"));
    assert!(selection.includes_skill("pdf"));
    assert!(!selection.includes_agent("guide"));
    assert!(!selection.includes_skill("economist-analyst"));
}

#[test]
fn canonical_identities_round_trip_without_losing_type_or_qualification() {
    for (text, kind, name) in [
        ("skill:tla-plus-expert", EntryKind::Skill, "tla-plus-expert"),
        ("agent:guide", EntryKind::Agent, "guide"),
        ("agent:core/guide", EntryKind::Agent, "core/guide"),
    ] {
        let id: EntryId = text.parse().unwrap();
        assert_eq!(id.kind(), kind);
        assert_eq!(id.name(), name);
        assert_eq!(id.to_string(), text);
    }
}

#[test]
fn malformed_canonical_identities_are_rejected() {
    for text in [
        "pdf",
        "amplihack:pdf",
        "skill:",
        "agent:",
        "thing:pdf",
        "skill:quality/reviewing-code",
        "agent:/core/guide",
        "agent:../guide",
        "agent:core/../guide",
        "agent:core//guide",
        "agent:core/guide/",
        "agent:core/guide.md",
        "agent:core:guide",
        "skill:with space",
        "skill:line\nbreak",
        "skill:pdf\0",
        "agent:core\\guide",
    ] {
        assert!(text.parse::<EntryId>().is_err(), "{text:?}");
    }
}

#[test]
fn list_set_accepts_only_supported_values() {
    for (text, set) in [("default", ListSet::Default), ("all", ListSet::All)] {
        assert_eq!(text.parse::<ListSet>().unwrap(), set);
        assert_eq!(set.to_string(), text);
    }
    for text in ["", "Default", "ALL", " all", "all ", "custom"] {
        assert!(text.parse::<ListSet>().is_err());
    }
}
