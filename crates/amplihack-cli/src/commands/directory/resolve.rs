//! Exact mutation identities, separate from discovery's suffix search.

use std::collections::BTreeSet;
use std::path::Path;

use amplihack_utils::list_selection::EntryId;
use anyhow::{Context, Result, bail};

use super::index::{Entry, EntryKind};

/// Resolve a canonical identity or tool-qualified alias against the whole index.
/// Ambiguous aliases report every canonical alternative, including agent paths.
pub(crate) fn resolve(staged: &Path, entries: &[Entry], query: &str) -> Result<EntryId> {
    let query = query.trim_start_matches("amplihack:");
    let (kind, name) = if let Some(name) = query.strip_prefix("skill:") {
        (Some(EntryKind::Skill), name)
    } else if let Some(name) = query.strip_prefix("agent:") {
        (Some(EntryKind::Agent), name)
    } else {
        (None, query)
    };
    let qualified = name.replace(':', "/");
    let mut candidates = BTreeSet::new();
    let mut exact = None;
    for entry in entries {
        if kind.is_some_and(|kind| kind != entry.kind) {
            continue;
        }
        let id = identity(staged, entry)?;
        if kind.is_some() && qualified == id.name() {
            exact = Some(id.clone());
        }
        let leaf = id.name().rsplit('/').next().unwrap_or(id.name());
        // A full path is exact. Only an unqualified name may match a leaf
        // or a front-matter alias.
        if qualified == id.name()
            || (!qualified.contains('/') && (name == leaf || name == entry.name))
        {
            candidates.insert(id);
        }
    }
    if let Some(id) = exact {
        return Ok(id);
    }
    match candidates.len() {
        0 => bail!("unknown skill or agent; use `amplihack directory find` to find an entry"),
        1 => Ok(candidates.into_iter().next().expect("one candidate")),
        _ => bail!(
            "ambiguous skill or agent; use one of: {}",
            candidates
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn identity(staged: &Path, entry: &Entry) -> Result<EntryId> {
    let name = match entry.kind {
        EntryKind::Skill => entry.name.clone(),
        EntryKind::Agent => entry
            .path
            .strip_prefix(staged.join("agents"))
            .context(
                "an indexed agent is outside the staged agents folder; run `amplihack install`",
            )?
            .with_extension("")
            .to_str()
            .context("an indexed agent path is not UTF-8; run `amplihack install`")?
            .replace(std::path::MAIN_SEPARATOR, "/"),
    };
    format!("{}:{name}", entry.kind.label())
        .parse()
        .context("invalid indexed identity; run `amplihack install`")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(staged: &Path) -> Vec<Entry> {
        [
            (
                EntryKind::Skill,
                "tla-plus-expert",
                "skills/tla-plus-expert/SKILL.md",
            ),
            (
                EntryKind::Agent,
                "tla-plus-expert",
                "agents/specialized/tla-plus-expert.md",
            ),
            (EntryKind::Agent, "guide", "agents/guide.md"),
            (EntryKind::Agent, "guide", "agents/core/guide.md"),
            (EntryKind::Agent, "builder", "agents/core/builder.md"),
            (EntryKind::Skill, "extra", "skills/category/extra/SKILL.md"),
        ]
        .into_iter()
        .map(|(kind, name, path)| Entry {
            kind,
            name: name.into(),
            path: staged.join(path),
            description: String::new(),
        })
        .collect()
    }

    #[test]
    fn qualified_aliases_preserve_the_agent_path() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        let entries = entries(staged);
        for query in [
            "agent:core/guide",
            "amplihack:core:guide",
            "amplihack:amplihack:core:guide",
            "amplihack:amplihack:amplihack:agent:core/guide",
            "agent:core:guide",
        ] {
            assert_eq!(
                resolve(staged, &entries, query).unwrap().to_string(),
                "agent:core/guide"
            );
        }
        assert_eq!(
            resolve(staged, &entries, "agent:guide")
                .unwrap()
                .to_string(),
            "agent:guide"
        );
        for query in [
            "extra",
            "skill:extra",
            "amplihack:extra",
            "amplihack:amplihack:extra",
            "amplihack:amplihack:skill:extra",
        ] {
            assert_eq!(
                resolve(staged, &entries, query).unwrap().to_string(),
                "skill:extra"
            );
        }
        assert_eq!(
            resolve(staged, &entries, "amplihack:amplihack:core:builder")
                .unwrap()
                .to_string(),
            "agent:core/builder"
        );
        assert_eq!(
            resolve(staged, &entries, "skill:tla-plus-expert")
                .unwrap()
                .to_string(),
            "skill:tla-plus-expert"
        );
        assert_eq!(
            resolve(staged, &entries, "amplihack:specialized:tla-plus-expert")
                .unwrap()
                .to_string(),
            "agent:specialized/tla-plus-expert"
        );
    }

    #[test]
    fn ambiguous_aliases_report_canonical_alternatives() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        let entries = entries(staged);
        for query in ["tla-plus-expert", "amplihack:tla-plus-expert"] {
            let error = resolve(staged, &entries, query).unwrap_err().to_string();
            assert!(error.contains("skill:tla-plus-expert"));
            assert!(error.contains("agent:specialized/tla-plus-expert"));
        }
        for query in ["guide", "amplihack:guide"] {
            let error = resolve(staged, &entries, query).unwrap_err().to_string();
            assert!(error.contains("agent:guide"));
            assert!(error.contains("agent:core/guide"));
        }
    }

    #[test]
    fn unknown_or_incomplete_aliases_do_not_select_entries() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        let entries = entries(staged);
        for query in [
            "",
            "amplihack:",
            "amplihack:amplihack:",
            "skill:",
            "agent:",
            "agent:extra",
            "core",
            "guide:",
            "missing",
        ] {
            assert!(resolve(staged, &entries, query).is_err(), "{query}");
        }
    }
}
