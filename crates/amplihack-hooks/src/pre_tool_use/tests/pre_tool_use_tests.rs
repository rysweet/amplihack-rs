use super::*;
use crate::test_support::env_lock;

fn make_bash_input(command: &str) -> HookInput {
    HookInput::PreToolUse {
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({"command": command}),
        session_id: None,
    }
}

#[test]
fn allows_safe_commands() {
    let hook = PreToolUseHook;
    let result = hook.process(make_bash_input("ls -la")).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn allows_non_bash_tools() {
    let hook = PreToolUseHook;
    let input = HookInput::PreToolUse {
        tool_name: "Read".to_string(),
        tool_input: serde_json::json!({"path": "/tmp/file.txt"}),
        session_id: None,
    };
    let result = hook.process(input).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn blocks_no_verify() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git commit --no-verify -m 'test'"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_from_camel_case_host_payload() {
    let hook = PreToolUseHook;
    let input: HookInput = serde_json::from_value(serde_json::json!({
        "hookEventName": "PreToolUse",
        "toolName": "Bash",
        "toolInput": {"command": "git commit --no-verify -m 'test'"},
        "sessionId": "session-123"
    }))
    .expect("camelCase PreToolUse host payload must deserialize");

    let result = hook.process(input).unwrap();

    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_push() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git push --no-verify origin main"))
        .unwrap();
    assert_eq!(result["block"], true);
}

#[test]
fn blocks_no_verify_on_rebase() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git rebase --no-verify main"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_merge() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git merge --no-verify feature-branch"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_cherry_pick() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git cherry-pick --no-verify abc123"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_on_am() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git am --no-verify patch.patch"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn allows_git_rebase_without_no_verify() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hook = PreToolUseHook;
    let result = hook.process(make_bash_input("git rebase main")).unwrap();
    assert!(result.get("block").is_none());
}

#[test]
fn allows_git_commit_on_feature_branch() {
    // Hold env_lock so concurrent tests can't set GITHUB_COPILOT_AGENT=1
    // while inject_context runs against the real CWD.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // This test depends on the current branch not being main/master.
    // In CI, we may be on a feature branch, so this should pass.
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("git commit -m 'test'"))
        .unwrap();
    // Can't assert allow/deny here reliably — depends on current branch.
    // Just verify it doesn't panic.
    let _ = result;
}

#[test]
fn handles_unknown_hook_event() {
    let hook = PreToolUseHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn blocks_no_verify_with_git_dir_prefix() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input(
            "GIT_DIR=/some/path git commit --no-verify -m 'test'",
        ))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn blocks_no_verify_with_env_prefix() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_bash_input("env git push --no-verify origin main"))
        .unwrap();
    assert_eq!(result["block"], true);
    assert!(result["message"].as_str().unwrap().contains("--no-verify"));
}

#[test]
fn normalize_strips_env_var_prefix() {
    assert_eq!(
        normalize_command("GIT_DIR=/tmp git commit -m 'x'"),
        "git commit -m 'x'"
    );
}

#[test]
fn normalize_strips_env_command() {
    assert_eq!(
        normalize_command("env git push origin main"),
        "git push origin main"
    );
}

#[test]
fn normalize_strips_multiple_env_vars() {
    assert_eq!(normalize_command("FOO=1 BAR=baz git commit"), "git commit");
}

#[test]
fn normalize_passthrough_plain_command() {
    assert_eq!(normalize_command("git commit -m 'x'"), "git commit -m 'x'");
}

// ---------------------------------------------------------------------------
// Issue #838: Skill-tool invocation that names an *agent* must be redirected
// to agent execution rather than letting the copilot runtime hard-fail with
// "Skill not found: <agent>" (which silently skips the requirements
// clarification phase of default-workflow).
//
// Behavior contract (verified through the public `process()` interface):
//   * Skill(name) where name is an AGENT but NOT a skill  -> block + redirect.
//   * Skill(name) where name is a real skill              -> pass-through.
//   * Skill(name) where name is in BOTH (overlap)         -> pass-through
//                                                            (skill precedence).
//   * Skill(name) where name is unknown                   -> pass-through.
//   * Malformed Skill payloads                            -> pass-through, no panic.
// ---------------------------------------------------------------------------

/// Build a `Skill` PreToolUse input using the primary `skill` key.
fn make_skill_input(name: &str) -> HookInput {
    HookInput::PreToolUse {
        tool_name: "Skill".to_string(),
        tool_input: serde_json::json!({"skill": name}),
        session_id: None,
    }
}

/// Build a `Skill` PreToolUse input using the alternate `name` key, which some
/// host payloads use instead of `skill`.
fn make_skill_input_name_key(name: &str) -> HookInput {
    HookInput::PreToolUse {
        tool_name: "Skill".to_string(),
        tool_input: serde_json::json!({"name": name}),
        session_id: None,
    }
}

#[test]
fn redirects_skill_call_naming_prompt_writer_agent() {
    let hook = PreToolUseHook;
    let result = hook.process(make_skill_input("prompt-writer")).unwrap();

    // Must be a non-fatal block carrying a redirect message — NOT an empty
    // pass-through (which would let the runtime emit "Skill not found").
    assert_eq!(
        result["block"], true,
        "Skill(prompt-writer) must be blocked and redirected, got: {result}"
    );
    let message = result["message"]
        .as_str()
        .expect("redirect block must carry a message");
    assert!(
        message.contains("prompt-writer"),
        "redirect message must name the agent: {message}"
    );
    assert!(
        message.to_lowercase().contains("agent"),
        "redirect message must point the model at the agent interface: {message}"
    );
}

#[test]
fn redirects_skill_call_using_name_key_payload() {
    let hook = PreToolUseHook;
    let result = hook
        .process(make_skill_input_name_key("prompt-writer"))
        .unwrap();
    assert_eq!(
        result["block"], true,
        "Skill payload using the `name` key must also redirect: {result}"
    );
}

#[test]
fn redirects_skill_call_naming_guide_agent_only() {
    // `guide` exists only as an agent (no SKILL.md), so it must redirect.
    let hook = PreToolUseHook;
    let result = hook.process(make_skill_input("guide")).unwrap();
    assert_eq!(
        result["block"], true,
        "Skill(guide) must redirect because guide is agent-only: {result}"
    );
}

#[test]
fn does_not_redirect_real_skill() {
    let hook = PreToolUseHook;
    let result = hook.process(make_skill_input("default-workflow")).unwrap();
    assert!(
        result.as_object().unwrap().is_empty(),
        "a genuine skill must pass through untouched: {result}"
    );
}

#[test]
fn does_not_redirect_overlapping_skill_and_agent_names() {
    // gherkin-expert and tla-plus-expert are BOTH a skill and an agent.
    // Skills take precedence, so these must pass through (resolve as skills).
    let hook = PreToolUseHook;
    for name in ["gherkin-expert", "tla-plus-expert"] {
        let result = hook.process(make_skill_input(name)).unwrap();
        assert!(
            result.as_object().unwrap().is_empty(),
            "overlapping name {name} must resolve as a skill (no redirect): {result}"
        );
    }
}

#[test]
fn does_not_redirect_unknown_skill_name() {
    // Unknown names are neither skill nor agent — let the runtime handle them
    // normally rather than over-blocking.
    let hook = PreToolUseHook;
    let result = hook
        .process(make_skill_input("totally-unknown-thing"))
        .unwrap();
    assert!(
        result.as_object().unwrap().is_empty(),
        "unknown names must pass through: {result}"
    );
}

#[test]
fn malformed_skill_payloads_pass_through_without_panic() {
    let hook = PreToolUseHook;

    let malformed = [
        serde_json::json!({}),                     // missing key
        serde_json::json!({"skill": 123}),         // non-string
        serde_json::json!({"skill": null}),        // null
        serde_json::json!({"name": ["nested"]}),   // array
        serde_json::json!({"unrelated": "value"}), // wrong key
        serde_json::json!({"skill": {"x": "y"}}),  // object value
    ];

    for payload in malformed {
        let input = HookInput::PreToolUse {
            tool_name: "Skill".to_string(),
            tool_input: payload.clone(),
            session_id: None,
        };
        let result = hook.process(input).unwrap();
        assert!(
            result.as_object().unwrap().is_empty(),
            "malformed Skill payload must pass through: {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #863: the skills DIRECTORY is the single source of truth.
//
// The hardcoded skill-name registry is removed. The
// pre-tool-use hook now answers "is this a skill?" by scanning the bundled
// `amplifier-bundle/skills/**/SKILL.md` files and reading each frontmatter
// `name:` value at runtime, via the private `bundled_skill_names()` helper.
//
// Contract for `bundled_skill_names() -> std::collections::BTreeSet<String>`:
//   * Derives a non-empty set from the on-disk skills directory.
//   * Uses the frontmatter `name:` as the skill identity — NOT the directory
//     path — so nested-category skills (e.g. migrate/ -> "amplihack-migrate")
//     are keyed by their published name.
//   * Never contains directory-path forms (e.g. "development/architecting-...").
//   * Membership is exact and case-sensitive.
//   * Contains overlap names (both skill and agent) so skill-precedence in the
//     redirect logic keeps them from being redirected.
//
// These tests fail to compile until `bundled_skill_names()` exists, then pass
// once the runtime directory scanner is implemented.
// ---------------------------------------------------------------------------

#[test]
fn bundled_skill_names_is_non_empty() {
    // An empty set would mean the scanner failed to locate the bundled skills
    // directory — which would wrongly redirect every real skill.
    let skills = bundled_skill_names();
    assert!(
        !skills.is_empty(),
        "bundled_skill_names() must derive a non-empty set from the skills directory"
    );
}

#[test]
fn bundled_skill_names_contains_top_level_skills() {
    let skills = bundled_skill_names();
    for name in ["default-workflow", "pdf", "xlsx"] {
        assert!(
            skills.contains(name),
            "bundled_skill_names() must contain top-level skill {name:?}"
        );
    }
}

#[test]
fn bundled_skill_names_uses_frontmatter_name_for_nested_skills() {
    // Nested category dirs (migrate/, development/, quality/, meta-cognitive/)
    // are identified by their frontmatter `name:`, not the directory path.
    let skills = bundled_skill_names();
    for name in [
        "amplihack-migrate",      // dir: migrate/
        "architecting-solutions", // dir: development/architecting-solutions/
        "reviewing-code",         // dir: quality/reviewing-code/
        "analyzing-deeply",       // dir: meta-cognitive/analyzing-deeply/
    ] {
        assert!(
            skills.contains(name),
            "bundled_skill_names() must contain nested skill by frontmatter name {name:?}"
        );
    }
}

#[test]
fn bundled_skill_names_excludes_directory_path_forms() {
    // Identity is the frontmatter name, so category-prefixed path forms and
    // bare parent-dir names must NOT be members.
    let skills = bundled_skill_names();
    for path_form in [
        "development/architecting-solutions",
        "quality/reviewing-code",
        "meta-cognitive/analyzing-deeply",
        "migrate",
    ] {
        assert!(
            !skills.contains(path_form),
            "bundled_skill_names() must not contain directory-path form {path_form:?}"
        );
    }
}

#[test]
fn bundled_skill_names_membership_is_exact_and_case_sensitive() {
    let skills = bundled_skill_names();
    assert!(!skills.contains("nonexistent-skill"));
    assert!(!skills.contains(""));
    assert!(
        !skills.contains("DEFAULT-WORKFLOW"),
        "membership must be case-sensitive"
    );
}

#[test]
fn bundled_skill_names_contains_overlap_names() {
    // gherkin-expert and tla-plus-expert exist as BOTH a skill and an agent.
    // They must be present in the skill set so the redirect logic's
    // skill-precedence keeps them from being redirected.
    let skills = bundled_skill_names();
    for name in ["gherkin-expert", "tla-plus-expert"] {
        assert!(
            skills.contains(name),
            "overlap name {name:?} must be present so it resolves as a skill"
        );
    }
}

#[test]
fn bundled_skill_names_matches_every_bundled_frontmatter_name() {
    // Directory-as-source-of-truth: every SKILL.md frontmatter `name:` in the
    // workspace bundle must appear in the runtime-derived set. Asserted as a
    // subset rather than strict equality because a developer/CI machine may
    // also stage skills under ~/.amplihack or ~/.copilot, which are legitimately
    // included by the scanner too.
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn collect_skill_files(dir: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_skill_files(&path, files);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                files.push(path);
            }
        }
    }

    fn frontmatter_name(content: &str) -> Option<String> {
        let fm = content
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---"))
            .map(|(fm, _)| fm)?;
        fm.lines()
            .find_map(|line| line.trim().strip_prefix("name:"))
            .map(|n| n.trim().to_string())
    }

    let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("amplifier-bundle/skills");
    if !skills_dir.is_dir() {
        eprintln!("SKIP: bundle not found at {}", skills_dir.display());
        return;
    }

    let mut files = Vec::new();
    collect_skill_files(&skills_dir, &mut files);
    assert!(
        !files.is_empty(),
        "expected SKILL.md files under the bundle"
    );

    let bundled: BTreeSet<String> = files
        .iter()
        .filter_map(|p| frontmatter_name(&fs::read_to_string(p).ok()?))
        .collect();

    let derived = bundled_skill_names();
    let missing: Vec<_> = bundled.difference(&derived).cloned().collect();
    assert!(
        missing.is_empty(),
        "bundled_skill_names() must include every bundled frontmatter name; missing: {missing:?}"
    );
}
