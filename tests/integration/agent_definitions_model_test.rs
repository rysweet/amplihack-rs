//! The agent definitions amplihack ships leave the choice of model to the user.
//!
//! Claude Code applies a definition's `model` line before the default model a
//! user sets for helpers, so a definition that says `inherit` makes every helper
//! of its type run on the model of the session that started it. These tests read
//! the definitions under `amplifier-bundle/agents` and the copies under
//! `docs/claude/agents`, check that each one parses, and check that none sets its
//! model to `inherit`. A definition may still name a particular model.

use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};

const DEFINITION_ROOTS: &[&str] = &["amplifier-bundle/agents", "docs/claude/agents"];

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

fn markdown_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            markdown_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            found.push(path);
        }
    }
}

/// The YAML between the opening `---` line and the next `---` line. A file that
/// does not open with a fence has no front matter and returns `None`; one that
/// opens a fence and never closes it is not a definition that parses.
fn front_matter(path: &Path, text: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let (yaml, _body) = rest.split_once("\n---\n").unwrap_or_else(|| {
        panic!(
            "{} opens a front matter fence and never closes it",
            path.display()
        )
    });
    Some(yaml.to_string())
}

fn parse_front_matter(path: &Path, yaml: &str) -> Mapping {
    match serde_yaml::from_str::<Value>(yaml) {
        Ok(Value::Mapping(map)) => map,
        Ok(_) => panic!("{} front matter is not a mapping", path.display()),
        Err(e) => panic!("{} front matter does not parse: {e}", path.display()),
    }
}

/// The model a definition names, read from the top level or from the `meta`
/// block some definitions use.
fn named_model(front_matter: &Mapping) -> Option<String> {
    let top = front_matter.get("model");
    let nested = front_matter
        .get("meta")
        .and_then(Value::as_mapping)
        .and_then(|meta| meta.get("model"));
    top.or(nested)
        .and_then(Value::as_str)
        .map(|model| model.trim().to_string())
}

fn sets_model_to_inherit(front_matter: &Mapping) -> bool {
    named_model(front_matter).is_some_and(|model| model.eq_ignore_ascii_case("inherit"))
}

/// Every definition under the roots that opens with front matter, parsed.
fn shipped_definitions() -> Vec<(PathBuf, Mapping)> {
    let root = workspace_root();
    let mut definitions = Vec::new();
    for relative in DEFINITION_ROOTS {
        let mut files = Vec::new();
        markdown_files(&root.join(relative), &mut files);
        let mut found_in_root = 0;
        for path in files {
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let Some(yaml) = front_matter(&path, &text) else {
                continue;
            };
            definitions.push((path.clone(), parse_front_matter(&path, &yaml)));
            found_in_root += 1;
        }
        assert!(
            found_in_root > 0,
            "no agent definitions found under {relative}"
        );
    }
    definitions
}

fn definition_name(front_matter: &Mapping) -> Option<&str> {
    let top = front_matter.get("name");
    let nested = front_matter
        .get("meta")
        .and_then(Value::as_mapping)
        .and_then(|meta| meta.get("name"));
    top.or(nested).and_then(Value::as_str)
}

#[test]
fn every_agent_definition_parses_and_has_a_name() {
    for (path, front_matter) in shipped_definitions() {
        assert!(
            definition_name(&front_matter).is_some_and(|name| !name.is_empty()),
            "{} must carry a name in its front matter",
            path.display()
        );
    }
}

#[test]
fn no_agent_definition_sets_its_model_to_inherit() {
    let offenders: Vec<String> = shipped_definitions()
        .into_iter()
        .filter(|(_, front_matter)| sets_model_to_inherit(front_matter))
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "these definitions set their model to inherit, which overrides the \
         default model a user set for helpers: {offenders:?}"
    );
}

/// The check separates `inherit` from a named model and from no model at all, so
/// it would catch a definition that brought the line back.
#[test]
fn the_check_tells_inherit_from_a_named_model_and_from_none() {
    let parse = |yaml: &str| parse_front_matter(Path::new("fixture.md"), yaml);
    assert!(sets_model_to_inherit(&parse("name: a\nmodel: inherit\n")));
    assert!(sets_model_to_inherit(&parse(
        "name: a\nmodel: \"Inherit\"\n"
    )));
    assert!(sets_model_to_inherit(&parse(
        "meta:\n  name: a\n  model: inherit\n"
    )));
    assert!(!sets_model_to_inherit(&parse(
        "name: a\nmodel: claude-sonnet-4-5\n"
    )));
    assert!(!sets_model_to_inherit(&parse("name: a\nrole: reviewer\n")));
}
