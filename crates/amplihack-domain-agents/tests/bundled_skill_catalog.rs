//! Loading the real bundle must not silently omit nested or validly extended skills.
use amplihack_domain_agents::SkillCatalog;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn frontmatter(source: &str) -> Result<String, &'static str> {
    let mut lines = source.lines();
    if lines.next() != Some("---") {
        return Err("frontmatter must start with a complete delimiter line");
    }
    let mut metadata = Vec::new();
    for line in lines {
        if line == "---" {
            return Ok(metadata.join("\n"));
        }
        metadata.push(line);
    }
    Err("frontmatter must end with a complete delimiter line")
}

#[test]
fn independent_inventory_requires_complete_frontmatter_delimiters() {
    assert_eq!(
        frontmatter("---\nname: example\n---\nbody"),
        Ok("name: example".into())
    );
    assert_eq!(
        frontmatter("---\r\nname: example\r\n---\r\nbody"),
        Ok("name: example".into())
    );
    assert!(frontmatter("---suffix\nname: example\n---\nbody").is_err());
    assert!(frontmatter("---\nname: example\n---suffix\nbody").is_err());
    assert!(frontmatter("---\nname: example\n").is_err());
}

// Independent line-based oracle: retain every byte after the delimiter,
// except CRLF normalization and the parser's leading LF separators.
fn expected_body(source: &str) -> String {
    let normalized = source.replace("\r\n", "\n");
    let mut offset = 0;
    for (index, line) in normalized.split_inclusive('\n').enumerate() {
        offset += line.len();
        if index > 0 && line.strip_suffix('\n').unwrap_or(line) == "---" {
            return normalized[offset..].trim_start_matches('\n').to_owned();
        }
    }
    panic!("missing closing delimiter");
}

#[test]
fn body_oracle_preserves_whitespace_for_lf_and_crlf() {
    for newline in ["\n", "\r\n"] {
        let source = ["---", "name: example", "---", "", "  Body  ", "", ""].join(newline);
        assert_eq!(expected_body(&source), "  Body  \n\n");
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("SKILL.md"), &source).unwrap();
        assert_eq!(
            SkillCatalog::load(root.path())
                .unwrap()
                .get("example")
                .unwrap()
                .prompt,
            expected_body(&source)
        );
    }
}

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
    let mut paths = std::collections::BTreeMap::new();
    let mut bodies = std::collections::BTreeMap::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read skill");
        let yaml = frontmatter(&text).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        let meta: serde_yaml::Value = serde_yaml::from_str(&yaml)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        let name = meta["name"].as_str().expect("string skill name");
        bodies.insert(name.to_owned(), expected_body(&text));
        assert!(
            paths
                .insert(
                    name.to_owned(),
                    file.parent().unwrap().canonicalize().unwrap()
                )
                .is_none(),
            "duplicate skill name: {name}"
        );
    }
    let expected: BTreeSet<String> = paths.keys().cloned().collect();
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
        assert_eq!(
            skill.prompt.as_bytes(),
            bodies[name].as_bytes(),
            "body: {name}"
        );
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
