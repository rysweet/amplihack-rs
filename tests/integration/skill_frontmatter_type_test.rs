//! Skill frontmatter TYPE validation tests — Issue #890.
//!
//! # Bug class
//!
//! Copilot CLI refuses to load a skill whose scalar frontmatter fields are
//! encoded as YAML sequences (lists) or mappings. The concrete failure was:
//!
//! ```text
//! ✖ /home/.../skills/merge-ready/SKILL.md: argument-hint must be a string
//! ```
//!
//! caused by `argument-hint: [pr-number]` (a YAML **list**) instead of
//! `argument-hint: "[pr-number]"` (a **string**). The merge-ready source was
//! fixed in commit e0abfb4, but this class of bug shipped once and must never
//! ship again. This guard fails CI loudly the moment any bundled skill encodes
//! `name`, `description`, or `argument-hint` as a non-string scalar.
//!
//! # The Copilot rule (enforced here)
//!
//! For every `amplifier-bundle/skills/**/SKILL.md`, these fields MUST be a
//! string scalar (`Value::String`), NOT a sequence/list or a mapping:
//!   - `name`         (mandatory)
//!   - `description`  (mandatory)
//!   - `argument-hint` (validated only when present)
//!
//! All YAML string forms are accepted: plain, single-quoted, double-quoted,
//! block (`|`) and folded (`>`) scalars all parse to `Value::String`.
//!
//! Fields that legitimately accept lists/maps (e.g. `allowed-tools`, `tools`,
//! `auto-activation`, `activation_keywords`) are intentionally NOT validated
//! here — that would produce false positives.
//!
//! # Read-only invariant
//!
//! This test only reads files. It never writes, removes, or creates files.
//!
//! # Running
//!
//! ```bash
//! cargo test --test skill_frontmatter_type -- --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

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
///
/// Uses `DirEntry::file_type()` — which reuses the `d_type` already returned by
/// `readdir` — instead of `Path::is_dir()`, avoiding a redundant `stat` syscall
/// per directory entry. `file_name()` is read straight off the entry so the full
/// `PathBuf` is only materialized for the paths we actually keep or recurse into.
///
/// Symlinked directories are not followed: `file_type()` reports the link itself
/// rather than its target, so a symlinked directory is treated as a non-matching
/// entry. The bundled corpus does contain a handful of symlinked directories
/// (e.g. `docx/ooxml`, `pptx/ooxml`, `outside-in-testing/*`); these are
/// intentionally not traversed, and the walk stays bounded to `skills_dir()`.
fn find_files_named(dir: &Path, filename: &str) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if !dir.is_dir() {
        return result;
    }
    for entry in fs::read_dir(dir).expect("read skills dir") {
        let entry = entry.expect("read dir entry");
        let file_type = entry.file_type().expect("read dir entry file type");
        if file_type.is_dir() {
            result.extend(find_files_named(&entry.path(), filename));
        } else if entry.file_name().to_str() == Some(filename) {
            result.push(entry.path());
        }
    }
    result
}

fn skill_files() -> &'static [PathBuf] {
    SKILL_FILES.as_slice()
}

/// Extract the raw YAML frontmatter block (between the opening `---\n` at byte 0
/// and the next `\n---`). Returns `None` when frontmatter is absent.
fn extract_frontmatter(content: &str) -> Option<&str> {
    let after_open = content.strip_prefix("---\n")?;
    let close_idx = after_open.find("\n---")?;
    Some(&after_open[..close_idx])
}

/// Human-readable name for a `serde_yaml::Value` variant, used in violation
/// messages so failures name the *found* type (e.g. "sequence/list").
fn yaml_type_name(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence/list",
        Value::Mapping(_) => "mapping/map",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::Null => "null",
        Value::Tagged(_) => "tagged",
    }
}

/// Fields Copilot CLI requires to be string scalars. `argument-hint` is only
/// validated when the key is present; `name`/`description` are mandatory.
const STRING_SCALAR_FIELDS: &[&str] = &["name", "description", "argument-hint"];
const MANDATORY_FIELDS: &[&str] = &["name", "description"];

// ── Tests ─────────────────────────────────────────────────────────────────────

/// TC-TYPE-01: Sanity — the skill corpus must be discovered and substantial.
///
/// Guards against a broken filesystem walk silently passing every other test
/// by finding zero files. The bundle currently ships 122 skills; `> 100` is a
/// deliberately loose floor.
#[test]
fn tc_type_01_skill_corpus_is_non_empty() {
    let skills = skills_dir();
    assert!(
        skills.is_dir(),
        "amplifier-bundle/skills/ must exist at {}",
        skills.display()
    );

    let files = skill_files();
    assert!(
        !files.is_empty(),
        "Expected to find SKILL.md files under {}",
        skills.display()
    );
    assert!(
        files.len() > 100,
        "Expected the bundled skill corpus to contain > 100 SKILL.md files, \
         found {} — the recursive walk may be broken",
        files.len()
    );
}

/// TC-TYPE-02: Primary guard — every bundled SKILL.md must encode `name`,
/// `description`, and (when present) `argument-hint` as a STRING scalar.
///
/// A field encoded as a YAML sequence (`[a, b]` / `- a`) or mapping is the exact
/// bug that made Copilot CLI reject `argument-hint: [pr-number]`. On any
/// violation this test fails loudly, naming the offending relative file path,
/// the field, and the wrong type that was found.
#[test]
fn tc_type_02_string_scalar_fields_are_strings() {
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

    let mut violations: Vec<String> = Vec::new();

    for path in files {
        let rel = relative_path(path);
        let content = fs::read_to_string(path).expect("read SKILL.md");

        let Some(frontmatter) = extract_frontmatter(&content) else {
            violations.push(format!("  {rel} → missing YAML frontmatter"));
            continue;
        };

        let mapping = match serde_yaml::from_str::<Value>(frontmatter) {
            Ok(Value::Mapping(map)) => map,
            Ok(other) => {
                violations.push(format!(
                    "  {rel} → frontmatter is a {} scalar, expected a mapping",
                    yaml_type_name(&other)
                ));
                continue;
            }
            Err(err) => {
                violations.push(format!("  {rel} → invalid YAML frontmatter: {err}"));
                continue;
            }
        };

        for &field in STRING_SCALAR_FIELDS {
            match mapping.get(field) {
                Some(value) => {
                    if !matches!(value, Value::String(_)) {
                        violations.push(format!(
                            "  {rel} → `{field}` must be a string, found {} \
                             (Copilot CLI rejects non-string scalar fields)",
                            yaml_type_name(value)
                        ));
                    }
                }
                None => {
                    if MANDATORY_FIELDS.contains(&field) {
                        violations.push(format!("  {rel} → missing mandatory `{field}` field"));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "SKILL.md frontmatter TYPE violations ({} found) — every `name`, \
         `description`, and `argument-hint` must be a string scalar, NOT a \
         list/sequence or mapping:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// TC-TYPE-03: Codify the Copilot rule with inline literals.
///
/// This is the executable statement of the rule: a bare-bracket
/// `argument-hint: [pr-number]` parses as a YAML **sequence** (which Copilot
/// rejects), whereas the quoted `argument-hint: "[pr-number]"` parses as a
/// **string** (which Copilot accepts).
#[test]
fn tc_type_03_argument_hint_list_form_is_a_sequence_not_a_string() {
    // NEGATIVE: unquoted brackets → YAML sequence. This is the shipped bug.
    let list_form: Value =
        serde_yaml::from_str("argument-hint: [pr-number]").expect("parse list-form frontmatter");
    let list_value = list_form
        .get("argument-hint")
        .expect("argument-hint key present");
    assert!(
        matches!(list_value, Value::Sequence(_)),
        "`argument-hint: [pr-number]` must parse as a YAML sequence (the bug \
         Copilot CLI rejects), found {}",
        yaml_type_name(list_value)
    );
    assert!(
        !matches!(list_value, Value::String(_)),
        "`argument-hint: [pr-number]` must NOT parse as a string"
    );

    // POSITIVE: quoted brackets → YAML string. This is the correct form.
    let string_form: Value = serde_yaml::from_str("argument-hint: \"[pr-number]\"")
        .expect("parse string-form frontmatter");
    let string_value = string_form
        .get("argument-hint")
        .expect("argument-hint key present");
    assert!(
        matches!(string_value, Value::String(_)),
        "`argument-hint: \"[pr-number]\"` must parse as a string, found {}",
        yaml_type_name(string_value)
    );
    assert_eq!(
        string_value.as_str(),
        Some("[pr-number]"),
        "quoted argument-hint must carry the literal value [pr-number]"
    );
}

/// TC-TYPE-04: merge-ready regression pin (commit e0abfb4).
///
/// Directly pins the original offender: merge-ready's `argument-hint` must be
/// the string `"[pr-number]"`, never reverted to the unquoted list form.
#[test]
fn tc_type_04_merge_ready_argument_hint_is_the_string_pr_number() {
    let skill_path = skills_dir().join("merge-ready/SKILL.md");
    if !skill_path.exists() {
        eprintln!(
            "SKIP: merge-ready SKILL.md not found at {}",
            skill_path.display()
        );
        return;
    }

    let content = fs::read_to_string(&skill_path).expect("read merge-ready SKILL.md");
    let frontmatter =
        extract_frontmatter(&content).expect("merge-ready SKILL.md must have YAML frontmatter");
    let value: Value =
        serde_yaml::from_str(frontmatter).expect("merge-ready frontmatter must be valid YAML");

    let argument_hint = value
        .get("argument-hint")
        .expect("merge-ready SKILL.md must declare an argument-hint field");

    assert!(
        matches!(argument_hint, Value::String(_)),
        "merge-ready `argument-hint` must be a string, found {} — do not revert \
         the e0abfb4 fix to the unquoted list form `argument-hint: [pr-number]`",
        yaml_type_name(argument_hint)
    );
    assert_eq!(
        argument_hint.as_str(),
        Some("[pr-number]"),
        "merge-ready `argument-hint` must be the string \"[pr-number]\""
    );
}
