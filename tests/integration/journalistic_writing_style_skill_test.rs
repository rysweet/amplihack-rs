//! Issue #1697: the bundled `journalistic-writing-style` skill.
//!
//! These tests define what the change must deliver:
//! - the skill file exists with valid frontmatter, keeps the author's
//!   description and body sections, has no credentials, email addresses or
//!   hidden content, and links only to the source's public hosts;
//! - its trigger and `documentation-writing`'s trigger each point to the other,
//!   and the two `documentation-writing` copies keep an identical description;
//! - `bundle.md` registers it in sorted order, and the group count matches the
//!   number of entries in the group;
//! - `SKILL_CATALOG.md` lists it in byte order, with header counts matching the
//!   number of rows;
//! - `amplihack install --local` into a temp HOME publishes it byte for byte.
//!
//! No test fixes today's total skill count. Counts are checked only against
//! the list they describe, so the next skill added does not break this file.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_yaml::Value;

const NAME: &str = "journalistic-writing-style";
const SKILL: &str = "amplifier-bundle/skills/journalistic-writing-style/SKILL.md";
const DOC_WRITING_BUNDLE: &str = "amplifier-bundle/skills/documentation-writing/SKILL.md";
const DOC_WRITING_MIRROR: &str = "docs/claude/skills/documentation-writing/SKILL.md";
const BUNDLE: &str = "amplifier-bundle/bundle.md";
const CATALOG: &str = "docs/skills/SKILL_CATALOG.md";

const NEW_SKILL_POINTER: &str = "For the structure, placement or Diataxis type of software \
     documentation, use documentation-writing instead.";
const DOC_WRITING_POINTER: &str = "For prose style, or editing text written by a language \
     model, use journalistic-writing-style instead.";
const AUTHOR_DESCRIPTION_START: &str =
    "A journalistic writing style for correcting long-form text produced by language models.";

fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        loop {
            if cur.join("amplifier-bundle").is_dir() && cur.join("Cargo.toml").is_file() {
                return cur;
            }
            assert!(cur.pop(), "no workspace root above the crate");
        }
    })
    .as_path()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Splits a SKILL.md into its parsed frontmatter and its body.
fn split(content: &str, what: &str) -> (serde_yaml::Mapping, String) {
    let after = content
        .strip_prefix("---\n")
        .unwrap_or_else(|| panic!("{what}: frontmatter must start at byte 0"));
    let end = after
        .find("\n---\n")
        .unwrap_or_else(|| panic!("{what}: frontmatter must be closed"));
    let map = match serde_yaml::from_str::<Value>(&after[..end]) {
        Ok(Value::Mapping(m)) => m,
        other => panic!("{what}: frontmatter must be a YAML mapping: {other:?}"),
    };
    (map, after[end + "\n---\n".len()..].to_string())
}

fn string_field(map: &serde_yaml::Mapping, key: &str, what: &str) -> String {
    match map.get(Value::String(key.into())) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("{what}: `{key}` must be a string, got {other:?}"),
    }
}

fn description_line(content: &str) -> &str {
    content
        .lines()
        .find(|l| l.starts_with("description:"))
        .expect("description line")
}

// ---------------------------------------------------------------- skill file

#[test]
fn skill_frontmatter_names_its_directory_and_keeps_the_authors_description() {
    let (fm, _) = split(&read(SKILL), SKILL);
    assert_eq!(string_field(&fm, "name", SKILL), NAME);
    let desc = string_field(&fm, "description", SKILL);
    assert!(
        desc.starts_with(AUTHOR_DESCRIPTION_START),
        "the author's description must be kept as written: {desc}"
    );
    for kept in ["README files", "status files", "conversational replies"] {
        assert!(
            desc.contains(kept),
            "author's trigger `{kept}` was removed: {desc}"
        );
    }
    assert!(
        desc.trim_end().ends_with(NEW_SKILL_POINTER),
        "description must end with the pointer to documentation-writing: {desc}"
    );
}

#[test]
fn skill_body_keeps_every_section_of_the_source() {
    let (_, body) = split(&read(SKILL), SKILL);
    for heading in [
        "# Journalistic writing style",
        "## 1. Structural faults",
        "## 2. Words and phrases to avoid",
        "## 3. Mechanics",
        "## 4. Evidence and sources",
        "## 5. The check before a document is finished",
        "## 6. Examples",
    ] {
        assert!(
            body.lines().any(|l| l == heading),
            "{SKILL} must keep the heading `{heading}`"
        );
    }
}

#[test]
fn skill_file_ends_with_one_newline_and_uses_lf() {
    let raw = fs::read(workspace_root().join(SKILL)).expect("read skill bytes");
    assert!(!raw.contains(&b'\r'), "{SKILL} must use LF line endings");
    assert!(raw.ends_with(b"\n"), "{SKILL} must end with a newline");
    assert!(
        !raw.ends_with(b"\n\n"),
        "{SKILL} must end with exactly one newline"
    );
}

#[test]
fn skill_has_no_hidden_characters_or_comments() {
    let skill = read(SKILL);
    let hidden = |c: char| {
        matches!(c as u32,
            0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0xFEFF)
    };
    if let Some((i, c)) = skill.char_indices().find(|&(_, c)| hidden(c)) {
        panic!(
            "{SKILL} has hidden character U+{:04X} at byte {i}",
            c as u32
        );
    }
    for c in skill.chars().filter(|c| !c.is_ascii()) {
        assert!(
            matches!(c, '§' | '—'),
            "{SKILL}: unexpected non-ASCII character {c:?} (U+{:04X})",
            c as u32
        );
    }
    assert!(
        !skill.contains("<!--"),
        "{SKILL} must not contain HTML comments"
    );
}

/// Checks that hold for any public skill file: no owner's name, no access
/// tokens, no email addresses.
///
/// This file deliberately keeps no list of the private wording that was taken
/// out of the source. The repository is public, so such a list would publish
/// the very words it is meant to keep out. The check against that wording is
/// a byte-for-byte diff against the private source, run by its owner outside
/// this repository.
#[test]
fn skill_omits_credentials_and_personal_details() {
    let skill = read(SKILL);
    let lower = skill.to_lowercase();
    for token in ["ryan", "ghp_", "github_pat_"] {
        assert!(!lower.contains(token), "{SKILL} must not contain `{token}`");
    }
    assert!(
        !skill.contains('@'),
        "{SKILL} must not contain email addresses"
    );
}

#[test]
fn skill_links_only_to_the_sources_public_hosts() {
    let skill = read(SKILL);
    for (i, _) in skill.match_indices("://") {
        let rest = &skill[i + 3..];
        let host: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        let allowed = ["profilebooks.com", "economist.com"]
            .iter()
            .any(|ok| host == *ok || host.ends_with(&format!(".{ok}")));
        assert!(
            allowed,
            "{SKILL} links to a host outside the allowlist: {host}"
        );
    }
}

// ---------------------------------------------------- trigger separation

#[test]
fn documentation_writing_points_prose_work_to_the_new_skill() {
    for path in [DOC_WRITING_BUNDLE, DOC_WRITING_MIRROR] {
        let (fm, _) = split(&read(path), path);
        assert_eq!(string_field(&fm, "name", path), "documentation-writing");
        let desc = string_field(&fm, "description", path);
        assert!(
            desc.starts_with("Writing clear, discoverable software documentation"),
            "{path}: the existing description must be kept: {desc}"
        );
        assert!(
            desc.trim_end().ends_with(DOC_WRITING_POINTER),
            "{path}: description must end with the pointer to {NAME}: {desc}"
        );
    }
}

#[test]
fn documentation_writing_copies_share_one_description_line() {
    let bundle = read(DOC_WRITING_BUNDLE);
    let mirror = read(DOC_WRITING_MIRROR);
    assert_eq!(
        description_line(&bundle),
        description_line(&mirror),
        "the bundle and docs copies of documentation-writing must keep the same description"
    );
}

// ------------------------------------------------------------- registration

#[test]
fn bundle_registers_the_skill_in_the_technical_group_in_order() {
    let bundle = read(BUNDLE);
    let entry = format!("  {NAME}: {{ path: skills/{NAME}/SKILL.md }}");
    let lines: Vec<&str> = bundle.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == entry)
        .unwrap_or_else(|| panic!("{BUNDLE} must contain the line `{entry}`"));
    assert_eq!(
        lines.iter().filter(|l| **l == entry).count(),
        1,
        "{BUNDLE} must register {NAME} once"
    );

    // The `# Technical skills (N)` comment must count the entries under it.
    let header = lines[..at]
        .iter()
        .rposition(|l| l.trim_start().starts_with('#'))
        .expect("group comment above the entry");
    let comment = lines[header].trim();
    assert!(comment.starts_with("# Technical skills ("), "{comment}");
    let stated: usize = comment
        .trim_start_matches("# Technical skills (")
        .trim_end_matches(')')
        .parse()
        .expect("group count");
    let entries: Vec<&str> = lines[header + 1..]
        .iter()
        .take_while(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .copied()
        .collect();
    assert_eq!(
        stated,
        entries.len(),
        "`{comment}` must match its {} entries",
        entries.len()
    );
    let names: Vec<&str> = entries
        .iter()
        .map(|l| l.trim().split(':').next().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "technical skills group must stay sorted");

    // The Skills Library table row reports the same group count.
    let row = format!("| Technical Skills | {stated} |");
    assert!(
        bundle.lines().any(|l| l.trim().starts_with(&row)),
        "{BUNDLE} Skills Library table must say `{row}`"
    );
}

#[test]
fn bundle_total_counts_agree_with_each_other() {
    // Existing drift between these totals and the map is out of scope (D5);
    // this only checks that every total was raised together.
    let bundle = read(BUNDLE);
    let grab = |prefix: &str, suffix: &str| -> usize {
        let line = bundle
            .lines()
            .find(|l| l.contains(prefix))
            .unwrap_or_else(|| panic!("{BUNDLE} must contain `{prefix}`"));
        let start = line.find(prefix).unwrap() + prefix.len();
        line[start..]
            .split(suffix)
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("number after `{prefix}` in `{line}`"))
    };
    let library = grab("### Skills Library (", " total)");
    let available = grab("## Available Skills (", " total)");
    let summary = grab("35 agents, ", " skills");
    assert_eq!(
        library, available,
        "Skills Library and Available Skills totals differ"
    );
    assert_eq!(
        library, summary,
        "Skills Library and the summary line totals differ"
    );
}

#[test]
fn catalog_lists_the_skill_in_byte_order_with_matching_counts() {
    let catalog = read(CATALOG);
    let row = format!("| `{NAME}` | `{NAME}/SKILL.md` |");
    let rows: Vec<&str> = catalog.lines().filter(|l| l.starts_with("| `")).collect();
    assert_eq!(
        rows.iter().filter(|l| **l == row).count(),
        1,
        "{CATALOG} must list `{row}` once"
    );
    let names: Vec<&str> = rows
        .iter()
        .map(|l| l.split('`').nth(1).expect("name column"))
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable(); // byte order, same as `LC_ALL=C sort`
    assert_eq!(names, sorted, "{CATALOG} rows must stay in byte order");

    for label in [
        "**Unique bundled skill names:**",
        "**Skill definition files:**",
    ] {
        let stated: usize = catalog
            .lines()
            .find_map(|l| l.split(label).nth(1))
            .unwrap_or_else(|| panic!("{CATALOG} must contain `{label}`"))
            .trim()
            .parse()
            .expect("catalog count");
        assert_eq!(
            stated,
            rows.len(),
            "{CATALOG} `{label}` must match its rows"
        );
    }
}

// ------------------------------------------------------------------ install

#[test]
fn local_install_into_a_temp_home_publishes_the_skill() {
    let bin = env!("CARGO_BIN_EXE_amplihack");
    let home = tempfile::tempdir().expect("temp HOME");
    let h = home.path();
    let bin_dir = Path::new(bin)
        .parent()
        .expect("binary dir")
        .display()
        .to_string();
    let path = match std::env::var("PATH") {
        Ok(p) => format!("{bin_dir}:{p}"),
        Err(_) => bin_dir,
    };
    let output = crate::isolated_amplihack::isolated(bin)
        .env("HOME", h)
        .env("PATH", path)
        .env("TMPDIR", h)
        .env("AMPLIHACK_HOME", h.join(".amplihack"))
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1")
        .env("AMPLIHACK_SKIP_MMDC", "1")
        .env("CARGO_HOME", h.join(".cargo"))
        .env("CODEX_HOME", h.join(".codex"))
        .env("XDG_CONFIG_HOME", h.join(".config"))
        .env("XDG_CACHE_HOME", h.join(".cache"))
        .env("XDG_DATA_HOME", h.join(".local/share"))
        .env("XDG_STATE_HOME", h.join(".local/state"))
        .env("XDG_RUNTIME_DIR", h.join(".runtime"))
        .env("CLAUDE_CONFIG_DIR", h.join(".claude"))
        .env("CLAUDE_PLUGIN_DATA", h.join("claude-plugin-data"))
        .env("COPILOT_HOME", h.join(".copilot"))
        .env("COPILOT_CONFIG_DIR", h.join(".copilot"))
        .env("RUSTC_WRAPPER", "")
        .args(["install", "--local"])
        .arg(workspace_root())
        .output()
        .expect("run amplihack install --local");
    assert!(
        output.status.success(),
        "install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let installed = h.join(".claude/skills").join(NAME).join("SKILL.md");
    let installed_bytes = fs::read(&installed)
        .unwrap_or_else(|e| panic!("install must publish {}: {e}", installed.display()));
    let repo_bytes = fs::read(workspace_root().join(SKILL)).expect("read repo skill");
    assert_eq!(
        installed_bytes, repo_bytes,
        "installed skill must match the repo copy"
    );
}
