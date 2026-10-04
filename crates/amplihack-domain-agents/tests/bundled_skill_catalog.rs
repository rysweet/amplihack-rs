//! Loading the real bundle must not silently omit nested or validly extended skills.
use amplihack_domain_agents::SkillCatalog;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn skill_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).expect("read bundled skill directory") {
        let entry = entry.expect("read skill entry");
        let kind = entry.file_type().expect("read skill entry type");
        if kind.is_dir() {
            skill_files(&entry.path(), files);
        } else if kind.is_file() && entry.file_name() == "SKILL.md" {
            files.push(entry.path());
        }
    }
}

#[test]
fn real_catalog_loads_every_bundled_skill_and_body() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
    let mut files = Vec::new();
    skill_files(&root, &mut files);
    assert_eq!(files.len(), 130, "fixed recursive inventory");
    assert_eq!(
        files
            .iter()
            .filter(|file| file.strip_prefix(&root).unwrap().components().count() > 2)
            .count(),
        7,
        "all seven nested skills must be present"
    );
    let mut expected = BTreeSet::new();
    let mut paths = std::collections::BTreeMap::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read skill");
        let yaml = text
            .strip_prefix("---\n")
            .expect("frontmatter at byte zero")
            .split_once("\n---")
            .expect("closed frontmatter")
            .0;
        let meta: serde_yaml::Value = serde_yaml::from_str(yaml).expect("valid YAML frontmatter");
        let name = meta["name"].as_str().expect("string skill name");
        paths.insert(
            name.to_owned(),
            file.parent().unwrap().canonicalize().unwrap(),
        );
        assert!(
            expected.insert(name.to_owned()),
            "duplicate skill name: {name}"
        );
    }
    let catalog = SkillCatalog::load(&root).expect("load real skill catalog");
    let actual: BTreeSet<String> = catalog.names().into_iter().map(str::to_owned).collect();
    let missing: Vec<_> = expected.difference(&actual).collect();
    assert_eq!(
        actual, expected,
        "catalog omitted bundled skills: {missing:?}"
    );
    for name in &expected {
        let skill = catalog.get(name).expect("indexed skill");
        assert_eq!(skill.path.canonicalize().unwrap(), paths[name]);
        assert!(
            !skill.prompt.trim().is_empty(),
            "empty skill prompt: {name}"
        );
        assert!(
            skill.path.join("SKILL.md").is_file(),
            "missing skill source: {name}"
        );
    }
}
