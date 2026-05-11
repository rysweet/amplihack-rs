//! Skill frontmatter name validation tests — Issue #592.
//!
//! These tests enforce the constraint documented in FRONTMATTER_STANDARDS.md:
//! the YAML `name:` field in every SKILL.md must match `^[a-z0-9-]+$`
//! (lowercase letters, digits, hyphens only).
//!
//! # Regression guard
//!
//! TC-SKILL-01 is a blanket scan of *all* bundled SKILL.md files.
//! TC-SKILL-02 is a specific regression test for the `amplihack:migrate`
//! → `amplihack-migrate` fix (issue #592).
//! TC-SKILL-03 validates that `activation_keywords` also comply.
//! TC-SKILL-04 verifies the SKILL_CATALOG.md references `amplihack-migrate`.
//!
//! # Running
//!
//! ```bash
//! cargo test --test skill_frontmatter_name -- --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Workspace root: `bins/amplihack/` → pop twice → workspace root.
fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // bins/amplihack → bins/
    root.pop(); // bins/ → workspace root
    root
}

/// Path to `amplifier-bundle/skills/`.
fn skills_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle/skills")
}

/// Recursively find every `SKILL.md` (case-insensitive) under `dir`.
fn find_skill_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if !dir.is_dir() {
        return result;
    }
    for entry in fs::read_dir(dir).expect("read skills dir") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.is_dir() {
            result.extend(find_skill_files(&path));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("SKILL.md"))
        {
            result.push(path);
        }
    }
    result
}

/// Extract the YAML frontmatter `name:` value from a SKILL.md file.
/// Returns `None` if no frontmatter or no `name:` field found.
fn extract_frontmatter_name(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    // Find closing `---`
    let after_open = &trimmed[3..];
    let close_idx = after_open.find("\n---")?;
    let frontmatter = &after_open[..close_idx];

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
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Vec::new();
    }
    let after_open = &trimmed[3..];
    let Some(close_idx) = after_open.find("\n---") else {
        return Vec::new();
    };
    let frontmatter = &after_open[..close_idx];

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

// ── Tests ─────────────────────────────────────────────────────────────────────

/// TC-SKILL-01: Every bundled SKILL.md `name:` field must match `^[a-z0-9-]+$`.
///
/// This is a blanket guard — if any future skill is added with a colon,
/// space, underscore, or uppercase character in the name field, this test
/// catches it before merge.
#[test]
fn tc_skill_01_all_bundled_skill_names_match_kebab_pattern() {
    let name_re = Regex::new(r"^[a-z0-9-]+$").expect("compile name regex");
    let skills = skills_dir();
    if !skills.is_dir() {
        eprintln!(
            "SKIP: amplifier-bundle/skills/ not found at {}",
            skills.display()
        );
        return;
    }

    let skill_files = find_skill_files(&skills);
    assert!(
        !skill_files.is_empty(),
        "Expected to find SKILL.md files under {}",
        skills.display()
    );

    let mut violations = Vec::new();

    for path in &skill_files {
        let content = fs::read_to_string(path).expect("read SKILL.md");
        if let Some(name) = extract_frontmatter_name(&content)
            && !name_re.is_match(&name)
        {
            violations.push(format!(
                "  {} → name: \"{}\" (must be lowercase letters, numbers, hyphens only)",
                path.strip_prefix(workspace_root())
                    .unwrap_or(path)
                    .display(),
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
    let name_re = Regex::new(r"^[a-z0-9-]+$").expect("compile name regex");

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
    assert!(!name_re.is_match(""), "empty string must be rejected");
    assert!(!name_re.is_match("skill/path"), "slashes must be rejected");
}
