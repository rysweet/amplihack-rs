//! Issue #746 regression tests for stale default-workflow guidance.
//!
//! These tests define the documentation contract before the docs are updated:
//! canonical guidance must point users to the `default-workflow` skill/recipe
//! path, while `DEFAULT_WORKFLOW.md` references are allowed only as clearly
//! legacy, deprecated, or migration-contextual references.

use std::fs;
use std::path::PathBuf;

const DEFAULT_WORKFLOW_SKILL_FILES: &[&str] = &[
    "amplifier-bundle/skills/default-workflow/SKILL.md",
    "docs/claude/skills/default-workflow/SKILL.md",
];

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

fn read_rel(relative_path: &str) -> String {
    let path = workspace_root().join(relative_path);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn direct_invocation_section(content: &str) -> &str {
    let start = content
        .find("### Direct invocation")
        .expect("default-workflow skill must document direct invocation");
    let tail = &content[start..];
    let end = tail.find("### Preferred").unwrap_or(tail.len());
    &tail[..end]
}

fn is_legacy_context_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "deprecated",
        "legacy",
        "migration",
        "historical",
        "history",
        "replacement",
        "replaced",
        "reference documentation",
        "human-readable reference",
    ]
    .iter()
    .any(|allowed| lower.contains(allowed))
}

#[test]
fn default_workflow_skill_docs_share_the_same_direct_invocation_contract() {
    let bundle = direct_invocation_section(&read_rel(DEFAULT_WORKFLOW_SKILL_FILES[0])).to_owned();
    let docs = direct_invocation_section(&read_rel(DEFAULT_WORKFLOW_SKILL_FILES[1])).to_owned();

    assert_eq!(
        bundle, docs,
        "The bundled and docs mirror default-workflow skill files must keep the \
         direct invocation section byte-identical so one copy cannot drift into \
         stale workflow guidance."
    );
}

#[test]
fn default_workflow_skill_docs_name_the_direct_recipe_interface() {
    for relative_path in DEFAULT_WORKFLOW_SKILL_FILES {
        let content = read_rel(relative_path);

        assert!(
            content.contains("direct executable recipe interface"),
            "{relative_path} must explicitly describe direct execution as the \
             recipe-runner interface, not as manual DEFAULT_WORKFLOW.md execution."
        );
        assert!(
            content.contains("amplihack recipe run default-workflow"),
            "{relative_path} must show the Rust CLI recipe invocation."
        );
        assert!(
            content.contains("Or with verbose output:"),
            "{relative_path} must preserve the verbose direct-invocation example."
        );
    }
}

#[test]
fn workflow_to_skills_migration_mentions_recipes_in_the_new_architecture() {
    let content = read_rel("docs/WORKFLOW_TO_SKILLS_MIGRATION.md");

    assert!(
        content.contains("**After**: Skills backed by recipes | Commands | Agents"),
        "The migration guide must describe the post-migration architecture as \
         skills backed by recipes, not skills alone."
    );
    assert!(
        content.contains("The default workflow exposes one direct executable recipe interface"),
        "The migration guide must make the recipe-runner execution surface explicit."
    );
}

#[test]
fn workflow_to_skills_migration_contextualizes_python3_c_audit_hits() {
    let content = read_rel("docs/WORKFLOW_TO_SKILLS_MIGRATION.md");

    assert!(
        content.contains("`python3 -c` matches are stale only")
            && content.contains("workflow recipes"),
        "The migration guide must define when `python3 -c` audit hits are stale \
         so unrelated Python examples are not removed blindly."
    );
}

#[test]
fn bundled_user_preferences_select_the_default_workflow_skill_or_recipe() {
    let content = read_rel("amplifier-bundle/context/USER_PREFERENCES.md");
    let selected_line = content
        .lines()
        .find(|line| line.trim_start().starts_with("**Selected**:"))
        .expect("USER_PREFERENCES.md must document the selected workflow");

    assert!(
        !selected_line.contains("DEFAULT_WORKFLOW.md"),
        "The selected workflow line must not point users at the legacy \
         DEFAULT_WORKFLOW.md file: {selected_line}"
    );
    assert!(
        selected_line.contains("default-workflow skill")
            || selected_line.contains("default-workflow recipe")
            || selected_line.contains("`default-workflow` skill/recipe")
            || selected_line.contains("amplihack recipe run default-workflow"),
        "The selected workflow line must point at the default-workflow \
         skill/recipe path: {selected_line}"
    );
}

#[test]
fn workflow_readme_has_no_uncontextualized_legacy_default_workflow_guidance() {
    let content = read_rel("docs/claude/workflow/README.md");
    let stale_markers = ["DEFAULT_WORKFLOW.md", "/ultrathink", "UltraThink"];
    let mut violations = Vec::new();
    let mut active_heading = String::new();

    for (line_number, line) in content.lines().enumerate() {
        if line.trim_start().starts_with('#') {
            active_heading = line.to_owned();
        }

        let has_stale_marker = stale_markers.iter().any(|marker| line.contains(marker));
        let has_allowed_context =
            is_legacy_context_line(line) || is_legacy_context_line(&active_heading);

        if has_stale_marker && !has_allowed_context {
            violations.push(format!("{}: {}", line_number + 1, line.trim()));
        }
    }

    assert!(
        violations.is_empty(),
        "docs/claude/workflow/README.md must not present DEFAULT_WORKFLOW.md \
         or UltraThink-era commands as canonical guidance. Remaining references \
         must be explicitly legacy/deprecated/migration context:\n{}",
        violations.join("\n")
    );
}
