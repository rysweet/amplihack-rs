//! Skill frontmatter name validation tests — Issue #592.
//!
//! These tests enforce the constraint documented in FRONTMATTER_STANDARDS.md:
//! the YAML `name:` field in every SKILL.md must match
//! `^[a-z0-9]+(-[a-z0-9]+)*$` and each bundled skill must use a canonical
//! uppercase `SKILL.md` entrypoint with frontmatter at byte 0.
//!
//! # Regression guard
//!
//! TC-SKILL-01 is a blanket scan of *all* bundled SKILL.md files.
//! TC-SKILL-02 is a specific regression test for the `amplihack:migrate`
//! → `amplihack-migrate` fix (issue #592).
//! TC-SKILL-03 validates that `activation_keywords` also comply.
//! TC-SKILL-04 verifies the SKILL_CATALOG.md references `amplihack-migrate`.
//!
//! TC-SKILL-12..13 are the issue #860 regression guards: TC-SKILL-12 enforces
//! registry ↔ bundle consistency (no one-sided drift) and TC-SKILL-13 pins the
//! `pr-guide` skill on both sides, so a skill can never silently disappear from
//! the Copilot CLI listing again (see
//! `docs/troubleshooting/pr-guide-skill-missing.md`).
//!
//! # Running
//!
//! ```bash
//! cargo test --test skill_frontmatter_name -- --nocapture
//! ```

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use amplihack_hooks::known_skills::{is_amplihack_skill, skill_count};
use regex::Regex;
use serde_yaml::Value;

// ── Helpers ───────────────────────────────────────────────────────────────────

static WORKSPACE_ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // bins/amplihack → bins/
    root.pop(); // bins/ → workspace root
    root
});

static SKILL_FILES: LazyLock<Vec<PathBuf>> =
    LazyLock::new(|| find_files_named(&skills_dir(), "SKILL.md"));

/// Workspace root: `bins/amplihack/` → pop twice → workspace root.
fn workspace_root() -> &'static Path {
    WORKSPACE_ROOT.as_path()
}

/// Path to `amplifier-bundle/skills/`.
fn skills_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle/skills")
}

fn relative_path(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Recursively find every file named `filename` under `dir`.
fn find_files_named(dir: &Path, filename: &str) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if !dir.is_dir() {
        return result;
    }
    for entry in fs::read_dir(dir).expect("read skills dir") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.is_dir() {
            result.extend(find_files_named(&path, filename));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == filename)
        {
            result.push(path);
        }
    }
    result
}

fn skill_files() -> &'static [PathBuf] {
    SKILL_FILES.as_slice()
}

/// Extract the YAML frontmatter `name:` value from a SKILL.md file.
/// Returns `None` if no frontmatter or no `name:` field found.
fn extract_frontmatter_name(content: &str) -> Option<String> {
    let frontmatter = extract_frontmatter(content)?;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Extract activation_keywords list from YAML frontmatter.
fn extract_activation_keywords(content: &str) -> Vec<String> {
    let Some(frontmatter) = extract_frontmatter(content) else {
        return Vec::new();
    };

    let mut keywords = Vec::new();
    let mut in_keywords_block = false;

    for line in frontmatter.lines() {
        let trimmed_line = line.trim();

        if trimmed_line.starts_with("activation_keywords:") {
            in_keywords_block = true;
            continue;
        }

        if in_keywords_block {
            if let Some(keyword) = trimmed_line.strip_prefix("- ") {
                keywords.push(keyword.trim().to_string());
            } else if !trimmed_line.is_empty() && !trimmed_line.starts_with('#') {
                // Non-list, non-comment line → we've left the keywords block
                break;
            }
        }
    }

    keywords
}

fn extract_frontmatter(content: &str) -> Option<&str> {
    let after_open = content.strip_prefix("---\n")?;
    let close_idx = after_open.find("\n---")?;
    Some(&after_open[..close_idx])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// TC-SKILL-01: Every bundled SKILL.md `name:` field must match
/// `^[a-z0-9]+(-[a-z0-9]+)*$`.
///
/// This is a blanket guard — if any future skill is added with a colon,
/// space, underscore, or uppercase character in the name field, this test
/// catches it before merge.
#[test]
fn tc_skill_01_all_bundled_skill_names_match_kebab_pattern() {
    let name_re = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("compile name regex");
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let skill_files = skill_files();
    assert!(
        !skill_files.is_empty(),
        "Expected to find SKILL.md files under {}",
        skills.display()
    );

    let mut violations = Vec::new();

    for path in skill_files {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        if let Some(name) = extract_frontmatter_name(&content)
            && !name_re.is_match(&name)
        {
            violations.push(format!(
                "  {} → name: \"{}\" (must be lowercase letters, numbers, hyphens only)",
                relative_path(path),
                name
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "SKILL.md files with invalid `name:` field ({} violations):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// TC-SKILL-02: The migrate skill name must be `amplihack-migrate` (not
/// `amplihack:migrate`). Direct regression test for issue #592.
#[test]
fn tc_skill_02_migrate_skill_name_is_amplihack_hyphen_migrate() {
    let skill_path = skills_dir().join("migrate/SKILL.md");
    if !skill_path.exists() {
        eprintln!(
            "SKIP: migrate SKILL.md not found at {}",
            skill_path.display()
        );
        return;
    }

    let content = fs::read_to_string(&skill_path).expect("read migrate SKILL.md");
    let name = extract_frontmatter_name(&content)
        .expect("migrate SKILL.md must have a frontmatter name: field");

    assert_eq!(
        name, "amplihack-migrate",
        "migrate SKILL.md name: field must be 'amplihack-migrate', got '{name}'"
    );

    // Ensure the old colon-based name is not present in the name field
    assert!(
        !name.contains(':'),
        "migrate SKILL.md name: field must not contain colons, got '{name}'"
    );
}

/// TC-SKILL-03: The migrate skill's `activation_keywords` must not contain
/// colons. The keyword should be `amplihack-migrate`, not `amplihack:migrate`.
#[test]
fn tc_skill_03_migrate_activation_keywords_have_no_colons() {
    let skill_path = skills_dir().join("migrate/SKILL.md");
    if !skill_path.exists() {
        eprintln!(
            "SKIP: migrate SKILL.md not found at {}",
            skill_path.display()
        );
        return;
    }

    let content = fs::read_to_string(&skill_path).expect("read migrate SKILL.md");
    let keywords = extract_activation_keywords(&content);

    assert!(
        !keywords.is_empty(),
        "migrate SKILL.md must have at least one activation_keyword"
    );

    let colon_keywords: Vec<_> = keywords.iter().filter(|k| k.contains(':')).collect();
    assert!(
        colon_keywords.is_empty(),
        "activation_keywords must not contain colons: {:?}",
        colon_keywords
    );

    // Specifically verify the first keyword is the skill name
    assert_eq!(
        keywords[0], "amplihack-migrate",
        "first activation_keyword should be 'amplihack-migrate', got '{}'",
        keywords[0]
    );
}

/// TC-SKILL-04: SKILL_CATALOG.md must reference `amplihack-migrate` (not
/// `amplihack:migrate`).
#[test]
fn tc_skill_04_catalog_references_amplihack_hyphen_migrate() {
    let catalog_path = workspace_root().join("docs/skills/SKILL_CATALOG.md");
    if !catalog_path.exists() {
        eprintln!(
            "SKIP: SKILL_CATALOG.md not found at {}",
            catalog_path.display()
        );
        return;
    }

    let content = fs::read_to_string(&catalog_path).expect("read SKILL_CATALOG.md");

    assert!(
        content.contains("amplihack-migrate"),
        "SKILL_CATALOG.md must contain 'amplihack-migrate'"
    );
    assert!(
        !content.contains("amplihack:migrate"),
        "SKILL_CATALOG.md must not contain the old 'amplihack:migrate' name"
    );
}

/// TC-SKILL-05: Name validation helper rejects colons (unit-level check
/// against the same regex used by plugin_manifest).
#[test]
fn tc_skill_05_name_regex_rejects_colons() {
    let name_re = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("compile name regex");

    // Valid names
    assert!(name_re.is_match("amplihack-migrate"));
    assert!(name_re.is_match("my-skill"));
    assert!(name_re.is_match("skill123"));
    assert!(name_re.is_match("a"));

    // Invalid: colon (the bug this issue fixes)
    assert!(
        !name_re.is_match("amplihack:migrate"),
        "colon must be rejected"
    );

    // Other invalid patterns
    assert!(!name_re.is_match("My-Skill"), "uppercase must be rejected");
    assert!(!name_re.is_match("my_skill"), "underscore must be rejected");
    assert!(!name_re.is_match("my skill"), "spaces must be rejected");
    assert!(
        !name_re.is_match("-skill"),
        "leading hyphen must be rejected"
    );
    assert!(
        !name_re.is_match("skill-"),
        "trailing hyphen must be rejected"
    );
    assert!(
        !name_re.is_match("my--skill"),
        "empty hyphen segment must be rejected"
    );
    assert!(!name_re.is_match(""), "empty string must be rejected");
    assert!(!name_re.is_match("skill/path"), "slashes must be rejected");
}

/// TC-SKILL-06: Bundled skills must use exact `SKILL.md` filenames.
///
/// Copilot skill discovery is sensitive to canonical skill metadata paths; a
/// lowercase `skill.md` can be missed even when its frontmatter is otherwise
/// valid.
#[test]
fn tc_skill_06_no_lowercase_skill_md_files() {
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let lowercase_files = find_files_named(&skills, "skill.md");
    assert!(
        lowercase_files.is_empty(),
        "Bundled skills must use canonical SKILL.md filenames, found lowercase files:\n{}",
        lowercase_files
            .iter()
            .map(|path| format!("  {}", relative_path(path)))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// TC-SKILL-07: Frontmatter must start at the first byte of every SKILL.md.
///
/// A Markdown title or comment before `---` prevents Copilot from parsing the
/// skill metadata, which made the old `azure-devops-cli` skill invisible.
#[test]
fn tc_skill_07_frontmatter_starts_at_first_byte() {
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let mut violations = Vec::new();
    for path in skill_files() {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        if !content.starts_with("---\n") {
            violations.push(format!("  {}", relative_path(path)));
        }
    }

    assert!(
        violations.is_empty(),
        "SKILL.md frontmatter must start at the first byte:\n{}",
        violations.join("\n")
    );
}

/// TC-SKILL-08: Bundled skill names must be unique.
#[test]
fn tc_skill_08_bundled_skill_names_are_unique() {
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    let mut duplicates = Vec::new();

    for path in skill_files() {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        let name = extract_frontmatter_name(&content).unwrap_or_else(|| {
            panic!(
                "SKILL.md must have a frontmatter name: field: {}",
                path.display()
            )
        });

        if let Some(first_path) = seen.insert(name.clone(), path.clone()) {
            duplicates.push(format!(
                "  {name}: {} and {}",
                relative_path(&first_path),
                relative_path(path)
            ));
        }
    }

    assert!(
        duplicates.is_empty(),
        "Bundled skill names must be unique:\n{}",
        duplicates.join("\n")
    );
}

/// TC-SKILL-09: Skill names must match their containing directory name.
///
/// Nested category directories are allowed, but the leaf directory is the
/// canonical skill directory. `migrate` is a deliberate legacy exception pinned
/// by TC-SKILL-02 because the published skill name is `amplihack-migrate`.
#[test]
fn tc_skill_09_skill_names_match_directory_names() {
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let exceptions = HashSet::from([("migrate", "amplihack-migrate")]);
    let mut violations = Vec::new();

    for path in skill_files() {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        let name = extract_frontmatter_name(&content).unwrap_or_else(|| {
            panic!(
                "SKILL.md must have a frontmatter name: field: {}",
                path.display()
            )
        });
        let dir_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .expect("skill file has parent directory");

        if name != dir_name && !exceptions.contains(&(dir_name, name.as_str())) {
            violations.push(format!(
                "  {} → name: \"{}\" but directory is \"{}\"",
                relative_path(path),
                name,
                dir_name
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Bundled skill names must match their containing directory:\n{}",
        violations.join("\n")
    );
}

/// TC-SKILL-10: `azure-devops-cli` is supporting documentation, not a
/// standalone loadable skill.
#[test]
fn tc_skill_10_azure_devops_cli_is_not_registered_as_a_skill() {
    let root = workspace_root();
    let files = [
        root.join("amplifier-bundle/bundle.md"),
        root.join("docs/skills/SKILL_CATALOG.md"),
        root.join("crates/amplihack-hooks/src/known_skills.rs"),
    ];

    let mut violations = Vec::new();
    for file in files {
        let content = fs::read_to_string(&file).expect("read registry file");
        let registered_content = if file.ends_with("known_skills.rs") {
            content
                .split_once("static AMPLIHACK_SKILLS: &[&str] = &[")
                .and_then(|(_, rest)| rest.split_once("];"))
                .map(|(registry, _)| registry)
                .unwrap_or(&content)
        } else {
            content.as_str()
        };
        if registered_content.contains("azure-devops-cli") {
            violations.push(relative_path(&file));
        }
    }

    assert!(
        violations.is_empty(),
        "azure-devops-cli must not be registered as a standalone skill:\n{}",
        violations.join("\n")
    );

    assert!(
        skills_dir()
            .join("azure-devops/cli-automation.md")
            .is_file(),
        "Azure DevOps CLI automation material must be preserved under azure-devops"
    );
}

/// TC-SKILL-11: Every bundled SKILL.md must have syntactically valid YAML
/// frontmatter.
#[test]
fn tc_skill_11_frontmatter_is_valid_yaml() {
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let mut violations = Vec::new();
    for path in skill_files() {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        let Some(frontmatter) = extract_frontmatter(&content) else {
            violations.push(format!("  {} -> missing frontmatter", relative_path(path)));
            continue;
        };
        if let Err(err) = serde_yaml::from_str::<Value>(frontmatter) {
            violations.push(format!(
                "  {} -> invalid YAML frontmatter: {err}",
                relative_path(path)
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "SKILL.md frontmatter must be valid YAML:\n{}",
        violations.join("\n")
    );
}

// ── Issue #860 regression guards: registry ↔ bundle consistency ─────────────────
//
// Root cause of #860: a stale-tree checkout dropped the `pr-guide` skill from
// *both* the bundle (`amplifier-bundle/skills/pr-guide/`) and the compile-time
// registry (`crates/amplihack-hooks/src/known_skills.rs`). Copilot CLI staging
// is filesystem-driven, so once the bundle directory was gone the skill
// disappeared from the listing. Two guards pin the fix:
//
//   * TC-SKILL-12 — no one-sided drift: every bundled skill is registered and
//     the registry count equals the on-disk bundle count. Given unique names on
//     both sides this is set-equality between the two sources of truth.
//   * TC-SKILL-13 — `pr-guide` is pinned concretely on *both* sides. This is the
//     #860 backstop for the wholesale two-sided removal, which TC-SKILL-12
//     cannot catch (dropping a skill from both sides keeps the counts equal and
//     leaves no bundled name unregistered).
//
// These tests reuse the file's existing helpers (`skills_dir`, `skill_files`,
// `extract_frontmatter_name`, `relative_path`) plus the public
// `amplihack_hooks::known_skills` API. No production code change is required on
// this branch — `pr-guide` is already present in both sources.

/// TC-SKILL-12: No drift between the bundle and the `AMPLIHACK_SKILLS` registry.
///
/// Guards both one-sided drift directions that can hide a skill:
///   * a bundled skill whose frontmatter `name:` is missing from the registry
///     (on disk but unrecognised by hook/classification code), and
///   * a registry entry with no matching bundled `SKILL.md` (recognised by name
///     but never staged for Copilot CLI).
///
/// Because bundled names are unique (TC-SKILL-08) and registry entries are
/// strictly sorted/unique (`skills_are_sorted_for_binary_search`), "every
/// bundled name is registered" plus "equal counts" is exactly set-equality
/// between the two sources of truth.
#[test]
fn tc_skill_12_registry_matches_bundle() {
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let files = skill_files();
    assert!(
        !files.is_empty(),
        "Expected to find SKILL.md files under {}",
        skills.display()
    );

    // Direction 1: every bundled skill's frontmatter name is registered.
    let mut unregistered = Vec::new();
    for path in files {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        let name = extract_frontmatter_name(&content).unwrap_or_else(|| {
            panic!(
                "SKILL.md must have a frontmatter name: field: {}",
                relative_path(path)
            )
        });
        if !is_amplihack_skill(&name) {
            unregistered.push(format!(
                "  {} → name: \"{}\" is not in the AMPLIHACK_SKILLS registry",
                relative_path(path),
                name
            ));
        }
    }
    assert!(
        unregistered.is_empty(),
        "Every bundled skill must be registered in known_skills.rs \
         (Copilot CLI recognition depends on it):\n{}",
        unregistered.join("\n")
    );

    // Direction 2: the registry count equals the on-disk bundle count. With
    // direction 1 holding and both sides unique, equal cardinality proves the
    // registry carries no extra (unbundled) entries.
    assert_eq!(
        skill_count(),
        files.len(),
        "Registry skill_count() ({}) must equal the bundled SKILL.md count ({}); \
         the registry and bundle have drifted",
        skill_count(),
        files.len()
    );
}

/// TC-SKILL-13: `pr-guide` must be pinned in *both* the registry and the
/// bundle. Direct regression guard for issue #860.
///
/// This is the backstop for the wholesale two-sided removal: unlike the drift
/// check in TC-SKILL-12 (which stays green when a skill vanishes from *both*
/// sides — counts still match and no bundled name is left unregistered), this
/// test asserts the skill's concrete presence on each side.
#[test]
fn tc_skill_13_pr_guide_pinned_in_registry_and_bundle() {
    // Registry side — enforced unconditionally.
    assert!(
        is_amplihack_skill("pr-guide"),
        "pr-guide must be registered in known_skills.rs (regression guard for issue #860)"
    );

    // Bundle side — a SKILL.md whose frontmatter name is exactly `pr-guide`.
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {} (registry check still enforced)",
            skills.display()
        );
        return;
    }

    let bundled_pr_guide = skill_files().iter().find(|path| {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        extract_frontmatter_name(&content).as_deref() == Some("pr-guide")
    });

    assert!(
        bundled_pr_guide.is_some(),
        "A bundled SKILL.md with frontmatter name \"pr-guide\" must exist under {} \
         (regression guard for issue #860)",
        skills.display()
    );
}
