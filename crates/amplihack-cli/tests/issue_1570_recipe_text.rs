//! Issue #1570 — static guards on the recipe and shell sites that put large
//! agent text into argv or the environment.
//!
//! Each site below once passed unbounded text as a single argv element (`-c
//! key="$(cat file)"`, `--body "$BODY"`, `git commit -m "$MSG"`, `jq --arg`),
//! or read it from an environment variable that is unset once the context is
//! too large. Above 131,072 bytes the spawn fails with `Argument list too long
//! (os error 7)`.
//!
//! Contract: docs/reference/large-payload-delivery.md, section "Recipe and
//! shell sites", and docs/howto/pass-large-recipe-context.md.
//!
//! These guards sit beside the issue #646 tests for quality-audit-cycle, which
//! still require the `mktemp` and the heredoc in `run-recursive-cycle`; nothing
//! here forbids either.

use serde_yaml::Value;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn bundle_file(rel: &str) -> String {
    let path = repo_root().join("amplifier-bundle").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The text with whole-line shell and YAML comments removed, so a comment
/// explaining the old pattern does not trip a guard.
fn code(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn re(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).unwrap()
}

fn step_command(recipe_rel: &str, step_id: &str) -> String {
    let text = bundle_file(recipe_rel);
    let recipe: Value = serde_yaml::from_str(&text).expect("recipe parses");
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe has steps")
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{recipe_rel}: step {step_id} has no command"))
        .to_string()
}

// -------------------------------------------------------------------------
// Finding 3: quality-audit-cycle run-recursive-cycle
// -------------------------------------------------------------------------

#[test]
fn quality_audit_recursive_cycle_passes_history_and_task_by_file() {
    let cmd = code(&step_command(
        "recipes/quality-audit-cycle.yaml",
        "run-recursive-cycle",
    ));
    assert!(
        cmd.contains("--context-from-file cycle_history="),
        "run-recursive-cycle must pass cycle_history with --context-from-file"
    );
    assert!(
        cmd.contains("--context-from-file task_description="),
        "run-recursive-cycle must pass task_description with --context-from-file"
    );
    assert!(
        !cmd.contains("cycle_history=\"$(cat") && !cmd.contains("cycle_history=$("),
        "run-recursive-cycle must not put the history file back into argv with $(cat ...)"
    );
}

#[test]
fn quality_audit_recursive_cycle_reads_values_through_the_helper() {
    let cmd = code(&step_command(
        "recipes/quality-audit-cycle.yaml",
        "run-recursive-cycle",
    ));
    assert!(
        cmd.contains("recipe_context_value.sh"),
        "run-recursive-cycle must source recipe_context_value.sh"
    );
    assert!(
        cmd.contains("write_context_value cycle_history"),
        "cycle_history must be read with write_context_value, which also works \
         when the runner moved the context to AMPLIHACK_CONTEXT_FILE"
    );
    assert!(
        cmd.contains("write_context_value task_description"),
        "task_description must be read with write_context_value"
    );
    for placeholder in ["{{cycle_history}}", "{{task_description}}"] {
        assert!(
            !cmd.contains(placeholder),
            "run-recursive-cycle must not render {placeholder} into the script text"
        );
    }
}

// -------------------------------------------------------------------------
// Findings 4 to 6: GitHub bodies and commit messages
// -------------------------------------------------------------------------

fn assert_body_file_site(rel: &str) {
    let text = code(&bundle_file(rel));
    // `--body "$X"`, `--body '$X'`, `--body $X` and `--body=$X`; `--body-file`
    // does not match because `-` follows `--body`.
    let inline_body = re(r"--body[\s=]");
    let hits: Vec<&str> = text.lines().filter(|l| inline_body.is_match(l)).collect();
    assert!(
        hits.is_empty(),
        "{rel}: gh bodies must use --body-file, found inline --body:\n{}",
        hits.join("\n")
    );
    assert!(
        text.contains("--body-file"),
        "{rel}: expected gh ... --body-file"
    );
    assert!(
        text.contains("workflow_body_file.sh") && text.contains("write_body_file"),
        "{rel}: must source workflow_body_file.sh and call write_body_file"
    );
    let commit_m = re(r#"git\s+commit\b[^\n]*\s-m\s*"?\$"#);
    assert!(
        !commit_m.is_match(&text),
        "{rel}: agent-written commit messages must use git commit -F, not -m \"$VAR\""
    );
}

#[test]
fn workflow_prep_creates_issue_with_body_file_and_stays_within_budget() {
    assert_body_file_site("recipes/workflow-prep.yaml");
    let lines = bundle_file("recipes/workflow-prep.yaml").lines().count();
    assert!(
        lines <= 400,
        "workflow-prep.yaml must stay at 400 lines or fewer, has {lines}"
    );
}

#[test]
fn consensus_publish_uses_body_file_and_commit_file() {
    assert_body_file_site("recipes/consensus-publish.yaml");
    let text = code(&bundle_file("recipes/consensus-publish.yaml"));
    assert!(
        re(r"git\s+commit\b[^\n]*\s-F\s").is_match(&text),
        "consensus-publish.yaml must commit with git commit -F <file>"
    );
}

#[test]
fn consensus_issue_worktree_creates_issue_with_body_file() {
    assert_body_file_site("recipes/consensus-issue-worktree.yaml");
}

#[test]
fn workflow_publish_pr_creates_pr_with_body_file() {
    assert_body_file_site("tools/workflow_publish_pr.sh");
}

// -------------------------------------------------------------------------
// Final requirements in the consensus recipes
// -------------------------------------------------------------------------

#[test]
fn consensus_recipes_read_final_requirements_through_the_helper() {
    for rel in [
        "recipes/consensus-publish.yaml",
        "recipes/consensus-issue-worktree.yaml",
    ] {
        let text = code(&bundle_file(rel));
        let hits: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("$FINAL_REQUIREMENTS_") || l.contains("${FINAL_REQUIREMENTS_"))
            .collect();
        assert!(
            hits.is_empty(),
            "{rel}: must not read final requirements from uppercase aliases, \
             which are unset for step outputs:\n{}",
            hits.join("\n")
        );
        assert!(
            text.contains("recipe_context_value.sh"),
            "{rel}: must source recipe_context_value.sh"
        );
        assert!(
            text.contains("write_context_value final_requirements."),
            "{rel}: must read fields with write_context_value final_requirements.<field>"
        );
    }
}

#[test]
fn consensus_merge_finalize_reads_task_with_rawfile() {
    let text = code(&bundle_file("recipes/consensus-merge-finalize.yaml"));
    assert!(
        !re(r#"--arg\s+task\s+"?\$\{?TASK_DESCRIPTION"#).is_match(&text),
        "consensus-merge-finalize.yaml must not pass the task to jq with --arg"
    );
    assert!(
        re(r"--rawfile\s+task\s").is_match(&text),
        "consensus-merge-finalize.yaml must pass the task to jq with --rawfile task <file>"
    );
    assert!(
        text.contains("write_context_value task_description"),
        "consensus-merge-finalize.yaml must write the task with write_context_value"
    );
}

// -------------------------------------------------------------------------
// Finding 10: autodrive
// -------------------------------------------------------------------------

#[test]
fn autodrive_loop_recipes_do_not_pass_task_description_as_context() {
    for rel in [
        "recipes/autodrive-crusty-loop.yaml",
        "recipes/autodrive-merge-loop.yaml",
    ] {
        let text = code(&bundle_file(rel));
        assert!(
            !text.contains("--context \"task_description="),
            "{rel}: must not pass --context \"task_description=...\" to autodrive_loop.sh"
        );
        assert!(
            !text.contains("task_description={{"),
            "{rel}: must not render task_description into a --context argument"
        );
    }
}

#[test]
fn autodrive_loop_passes_task_description_from_a_state_file() {
    let text = code(&bundle_file("tools/autodrive_loop.sh"));
    assert!(
        text.contains("task_description.txt"),
        "autodrive_loop.sh must write $STATE_DIR/task_description.txt"
    );
    assert!(
        text.contains("--context-from-file") && text.contains("task_description="),
        "autodrive_loop.sh must pass --context-from-file task_description=<file> each round"
    );
}

// -------------------------------------------------------------------------
// D15: the dead max_env_value_bytes field
// -------------------------------------------------------------------------

#[test]
fn dead_max_env_value_bytes_field_is_removed() {
    for (rel, text) in production_recipes() {
        assert!(
            !text.contains("max_env_value_bytes"),
            "{rel}: the runner does not recognise max_env_value_bytes; remove it"
        );
        assert!(
            !text.contains("FIX (#4231)"),
            "{rel}: remove the FIX (#4231) comment that described the dead field"
        );
    }
}

// -------------------------------------------------------------------------
// D4: context helpers are sourced, and RECIPE_VAR_* values are never code
// -------------------------------------------------------------------------

/// Helpers that read context from the calling shell's variables. A value the
/// runner spilled is not exported, so a helper run as its own process cannot
/// see it.
const SOURCED_HELPERS: [&str; 4] = [
    "recipe_context_value.sh",
    "workflow_body_file.sh",
    "workflow_context.sh",
    "workflow_doc_review_checkpoint.sh",
];

/// Every production recipe, as `(path relative to the repo, text)`.
fn production_recipes() -> Vec<(String, String)> {
    let dir = repo_root().join("amplifier-bundle/recipes");
    let mut recipes: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .map(|path| {
            let rel = path
                .strip_prefix(repo_root())
                .unwrap()
                .display()
                .to_string();
            (rel, std::fs::read_to_string(&path).unwrap())
        })
        .collect();
    recipes.sort();
    assert!(recipes.len() > 10, "found only {} recipes", recipes.len());
    recipes
}

/// Every `command` string in a parsed recipe, at any depth.
fn commands(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Mapping(map) => {
            for (key, child) in map {
                match (key.as_str(), child.as_str()) {
                    (Some("command"), Some(command)) => out.push(command.to_string()),
                    _ => commands(child, out),
                }
            }
        }
        Value::Sequence(items) => items.iter().for_each(|item| commands(item, out)),
        _ => {}
    }
}

#[test]
fn recipe_helpers_are_sourced_not_executed() {
    let helpers = SOURCED_HELPERS
        .iter()
        .map(|h| regex::escape(h))
        .collect::<Vec<_>>()
        .join("|");
    let assigned = re(&format!(
        r#"\b([A-Za-z_][A-Za-z0-9_]*)="[^"]*(?:{helpers})""#
    ));
    let run_by_name = re(&format!(
        r#"(?:^|[;&|]|\bthen|\bdo)\s*(?:(?:bash|sh|exec)\s+(?:-\S+\s+)*)?"?[^\s"';|&=()]*/(?:{helpers})"?(?:\s|$|;)"#
    ));
    let mut failures = Vec::new();
    for (rel, text) in production_recipes() {
        let code = code(&text);
        let vars: std::collections::BTreeSet<String> = assigned
            .captures_iter(&code)
            .map(|c| c[1].to_string())
            .collect();
        let run_by_var: Vec<regex::Regex> = vars
            .iter()
            .map(|var| {
                re(&format!(
                    r#"(?:^|[;&|]|\bthen|\bdo)\s*(?:(?:bash|sh|exec)\s+(?:-\S+\s+)*)?"?\$\{{?{var}\}}?"?(?:\s|$|;)"#
                ))
            })
            .collect();
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            let runs = run_by_name.is_match(line) || run_by_var.iter().any(|r| r.is_match(line));
            if runs {
                failures.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "these lines run a context helper as a command; source it with `.` instead:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_step_that_reads_context_through_a_helper_sources_it() {
    let sources = |command: &str, helper: &str| {
        command.lines().any(|line| {
            let line = line.trim_start();
            (line.starts_with(". ") || line.starts_with("source ") || line.contains("; . "))
                && line.contains(helper)
        })
    };
    let mut failures = Vec::new();
    for (rel, text) in production_recipes() {
        let recipe: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let mut all = Vec::new();
        commands(&recipe, &mut all);
        for command in all {
            let command = code(&command);
            for (function, helper) in [
                ("write_context_value ", "recipe_context_value.sh"),
                ("write_body_file ", "workflow_body_file.sh"),
            ] {
                if command.contains(function) && !sources(&command, helper) {
                    failures.push(format!("{rel}: calls {function}without sourcing {helper}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn no_production_recipe_evaluates_a_recipe_var_value() {
    // A test harness that stands in for the runner may export the variable it
    // simulates; nothing else may.
    const ALLOWED: [&str; 1] =
        ["amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh"];
    let misuse = [
        re(r"\beval\b[^#]*RECIPE_VAR_"),
        re(r"\bexport\b[^#]*RECIPE_VAR_"),
        re(r#"(?:^|[;&|]|\bthen|\bdo)\s*(?:\.|source)\s+"?\$\{?RECIPE_VAR_"#),
    ];
    let mut files = Vec::new();
    for dir in [
        "amplifier-bundle/recipes",
        "amplifier-bundle/recipes/tests",
        "amplifier-bundle/tools",
    ] {
        for entry in std::fs::read_dir(repo_root().join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    let mut failures = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(repo_root())
            .unwrap()
            .display()
            .to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with('#') || !misuse.iter().any(|r| r.is_match(line)) {
                continue;
            }
            if ALLOWED.contains(&rel.as_str()) {
                continue;
            }
            failures.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    assert!(
        failures.is_empty(),
        "a RECIPE_VAR_* value is untrusted text; never eval, export or source it:\n{}",
        failures.join("\n")
    );
}

// -------------------------------------------------------------------------
// D8: the copied helper lookup stays identical
// -------------------------------------------------------------------------

/// Recipe YAML shell steps cannot include a shared file, so each step that
/// needs `recipe_context_value.sh` carries the same lookup. This test keeps
/// the copies in sync; it is not a reason to build an include mechanism.
#[test]
fn recipe_helper_lookup_snippets_are_identical() {
    let lookup = re(r#"^(\s*)([A-Za-z_][A-Za-z0-9_]*)=""; for c in .*recipe_context_value\.sh"#);
    let mut copies = Vec::new();
    for (rel, text) in production_recipes() {
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            let Some(caps) = lookup.captures(line) else {
                continue;
            };
            let var = &caps[2];
            // The lookup, its not-found check and the `.` that sources it.
            let block = lines[n..(n + 3).min(lines.len())]
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n");
            let normalized = re(&format!(r"\b{var}\b")).replace_all(&block, "DIR");
            copies.push((format!("{rel}:{}", n + 1), normalized.into_owned()));
        }
    }
    assert_eq!(
        copies.len(),
        7,
        "expected seven copies of the recipe_context_value.sh lookup, found: {:?}",
        copies.iter().map(|(at, _)| at).collect::<Vec<_>>()
    );
    let (first_at, first) = &copies[0];
    for (at, copy) in &copies[1..] {
        assert_eq!(copy, first, "{at} differs from {first_at}");
    }
    assert!(
        first.contains(". \"$DIR/recipe_context_value.sh\""),
        "{first}"
    );
}
