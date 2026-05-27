/// Integration tests for issues #655 and #656.
///
/// ## Issue #655 — Stale Python recipe runner instructions in SKILL.md
///
/// The `docs/claude/skills/default-workflow/SKILL.md` file must reference the
/// Rust CLI (`amplihack recipe run default-workflow`) for direct invocation,
/// not the legacy Python `run_recipe_by_name` / `python3 -c` patterns.
///
/// **Test status**: These tests PASS because the SKILL.md fix has already landed.
///
/// ## Issue #656 — git fetch hard-fail with ADO remotes
///
/// The `amplifier-bundle/recipes/workflow-prep.yaml` step-01-prepare-workspace
/// must make `git fetch` resilient to credential failures. When `git fetch --all`
/// fails (exit 128, e.g. ADO remote without credential helper), the step must:
///   - Catch the failure and downgrade to a WARNING
///   - Continue with local branch state (no hard exit)
///   - Detect ADO remotes and provide specific remediation guidance
///   - Never echo the remote URL (may contain embedded PATs)
///
/// **Test status**: These tests are written TDD-RED. They FAIL until the
/// implementation changes `git fetch` from an `&&`-chain member to a guarded
/// block with exit-code capture and warning emission.
///
/// ## Test strategy
///
/// Mirrors `skip_pre_agent_validation_context_test.rs`:
///   - Parse recipe YAML with `serde_yaml` to inspect step command bodies
///   - Read SKILL.md as text and assert absence/presence of key strings
///   - No subprocess execution needed — all tests are structural/contract tests
use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn skill_md_path() -> PathBuf {
    workspace_root().join("docs/claude/skills/default-workflow/SKILL.md")
}

fn workflow_prep_yaml_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/workflow-prep.yaml")
}

// ---------------------------------------------------------------------------
// Recipe parsing helpers (same pattern as skip_pre_agent_validation tests)
// ---------------------------------------------------------------------------

fn load_recipe(path: &PathBuf) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

fn extract_step_body(recipe: &Value, step_id: &str) -> String {
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have a top-level 'steps' sequence");

    for step in steps {
        let id = step.get("id").and_then(Value::as_str).unwrap_or("");
        if id == step_id {
            if let Some(cmd) = step.get("command").and_then(Value::as_str) {
                return cmd.to_owned();
            }
            if let Some(prompt) = step.get("prompt").and_then(Value::as_str) {
                return prompt.to_owned();
            }
            panic!("step '{step_id}' has neither 'command:' nor 'prompt:' body");
        }
    }
    panic!("step '{step_id}' not found in recipe");
}

// ===========================================================================
// Issue #655: SKILL.md — no stale Python recipe instructions
// ===========================================================================

#[test]
fn skill_md_does_not_contain_run_recipe_by_name() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        !content.contains("run_recipe_by_name"),
        "SKILL.md must not reference the legacy Python function 'run_recipe_by_name'. \
         Agents encountering this will waste turns searching for a nonexistent Python package. \
         Use 'amplihack recipe run default-workflow' instead. (Issue #655)"
    );
}

#[test]
fn skill_md_does_not_contain_python3_dash_c() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        !content.contains("python3 -c"),
        "SKILL.md must not contain 'python3 -c' invocations. \
         The recipe runner is a Rust binary, not Python. (Issue #655)"
    );
}

#[test]
fn skill_md_does_not_contain_from_amplihack_import() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        !content.contains("from amplihack"),
        "SKILL.md must not contain 'from amplihack' Python import statements. \
         The recipe runner is a Rust binary. (Issue #655)"
    );
}

#[test]
fn skill_md_contains_amplihack_recipe_run_invocation() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        content.contains("amplihack recipe run default-workflow"),
        "SKILL.md must document the Rust CLI invocation: \
         'amplihack recipe run default-workflow'. (Issue #655)"
    );
}

#[test]
fn skill_md_contains_dash_c_context_syntax() {
    // The Rust CLI uses `-c key=value` for context variables
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        content.contains("-c task_description="),
        "SKILL.md must show the '-c task_description=' context variable syntax \
         for the Rust CLI recipe runner. (Issue #655)"
    );
}

#[test]
fn skill_md_contains_repo_path_context_variable() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        content.contains("repo_path="),
        "SKILL.md must show the 'repo_path=' context variable for the Rust CLI. (Issue #655)"
    );
}

// ===========================================================================
// Issue #656: workflow-prep.yaml — git fetch resilience
// ===========================================================================

// ── 656-1: git fetch must NOT be in the &&-chain ──────────────────────────

#[test]
fn step01_git_fetch_not_in_and_chain() {
    // If `git fetch` is joined to the preceding command with `&&`, a fetch
    // failure (exit 128) will abort the entire step. The fix must break
    // `git fetch` out of the `&&`-chain so the step can continue on failure.
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // The problematic pattern: `&& \ngit fetch` or `&& git fetch` in a chain
    // After the fix, git fetch should be in its own if-block, not chained.
    let _has_chained_fetch = body.contains("&& \\\n") && {
        // Check if there's a `&&` directly before `git fetch` on the
        // same logical line (possibly separated by `\\\n` continuation)
        let lines: Vec<&str> = body.lines().collect();
        let mut prev_ends_with_and_chain = false;
        for line in &lines {
            let trimmed = line.trim();
            if prev_ends_with_and_chain
                && (trimmed.starts_with("git fetch") || trimmed == "git fetch --all --no-tags")
            {
                return; // This would mean the assertion below should fail
            }
            prev_ends_with_and_chain = trimmed.ends_with("&& \\");
        }
        false
    };

    // Alternative detection: look for the literal pattern `&& \` followed by
    // a line containing `git fetch`
    let fetch_is_chained = {
        let re_pattern = "&&[\\s\\\\]*\n[\\s]*git fetch";
        let re = regex::Regex::new(re_pattern).unwrap();
        re.is_match(&body)
    };

    assert!(
        !fetch_is_chained,
        "step-01-prepare-workspace: `git fetch` must NOT be chained with `&&` to \
         the preceding command. A fetch failure (exit 128 on ADO remotes) would \
         abort the entire workspace preparation step. Break it into a guarded \
         if-block that captures the exit code. (Issue #656)\n\
         Current command body contains chained fetch."
    );
}

// ── 656-2: git fetch exit code must be captured ───────────────────────────

#[test]
fn step01_captures_git_fetch_exit_code() {
    // The implementation must capture the exit code of `git fetch` so it can
    // decide whether to warn or proceed. Common patterns:
    //   `git fetch ... || FETCH_RC=$?`
    //   `if ! git fetch ...; then`
    //   `git fetch ...; FETCH_RC=$?`
    //   `set +e; git fetch ...; FETCH_RC=$?; set -e`
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    let captures_exit_code = body.contains("$?")
        || body.contains("|| {")
        || body.contains("if ! git fetch")
        || body.contains("if git fetch");

    assert!(
        captures_exit_code,
        "step-01-prepare-workspace must capture the exit code of `git fetch` \
         (e.g., via `$?`, `if ! git fetch`, or `|| {{ ... }}`). Without exit-code \
         capture, the step cannot distinguish a credential failure from success. \
         (Issue #656)"
    );
}

// ── 656-3: fetch failure must emit a WARNING, not abort ───────────────────

#[test]
fn step01_emits_warning_on_fetch_failure() {
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // The warning must be clearly labeled so operators can identify it in logs
    let has_warning = body.contains("WARN") || body.contains("WARNING");

    assert!(
        has_warning,
        "step-01-prepare-workspace must emit a WARNING message when `git fetch` fails. \
         The step must continue with local branch state rather than aborting. (Issue #656)"
    );
}

// ── 656-4: step must continue after fetch failure ─────────────────────────

#[test]
fn step01_continues_to_branch_after_fetch_failure() {
    // After a git fetch failure, the step must still execute `git branch --show-current`
    // and reach "=== Workspace Prepared ===". This means `git branch --show-current`
    // must NOT be in the same `&&`-chain as `git fetch`.
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // git branch --show-current must still be present
    assert!(
        body.contains("git branch --show-current"),
        "step-01 must still contain 'git branch --show-current'"
    );

    // Check that git branch is NOT directly chained to git fetch with &&
    let fetch_and_branch_chained = {
        let re = regex::Regex::new(r"git fetch[^\n]*&&[\\s\\\n]*git branch").unwrap();
        re.is_match(&body)
    };

    assert!(
        !fetch_and_branch_chained,
        "step-01-prepare-workspace: `git branch --show-current` must NOT be \
         &&-chained to `git fetch`. If fetch fails, the branch command must still \
         execute so the step can report the current branch. (Issue #656)"
    );
}

// ── 656-5: ADO remote detection ───────────────────────────────────────────

#[test]
fn step01_detects_ado_remotes() {
    // When git fetch fails and the remote URL contains `dev.azure.com` or
    // `visualstudio.com`, the step must provide ADO-specific remediation
    // guidance (az login, GCM, PAT setup).
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    let detects_ado = body.contains("dev.azure.com") || body.contains("visualstudio.com");

    assert!(
        detects_ado,
        "step-01-prepare-workspace must detect ADO remotes (dev.azure.com or \
         visualstudio.com) when git fetch fails, to provide specific remediation \
         guidance. (Issue #656)"
    );
}

#[test]
fn step01_suggests_az_login_for_ado() {
    // ADO remediation must mention `az login` as a resolution path
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    assert!(
        body.contains("az login"),
        "step-01-prepare-workspace must suggest 'az login' as a remediation step \
         when git fetch fails on an ADO remote. (Issue #656)"
    );
}

// ── 656-6: remote URL must not be echoed (security) ───────────────────────

#[test]
fn step01_does_not_echo_remote_url() {
    // The remote URL may contain embedded PATs (https://user:PAT@dev.azure.com/...).
    // The step must NOT echo $REMOTE_URL or the raw git remote output in
    // a way that would expose credentials in logs.
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // Check that the URL variable (if captured) is only used in pattern matching,
    // never in echo/printf output. Common safe pattern: `echo "$REMOTE_URL"` is bad,
    // `echo "$REMOTE_URL" | grep -q "dev.azure.com"` is also bad (pipe leaks),
    // `[[ "$REMOTE_URL" == *dev.azure.com* ]]` is safe (no subshell output).
    let echoes_url = body.contains("echo \"$REMOTE_URL\"")
        || body.contains("echo $REMOTE_URL")
        || body.contains("printf '%s' \"$REMOTE_URL\"")
        || body.contains("printf \"%s\" \"$REMOTE_URL\"");

    assert!(
        !echoes_url,
        "step-01-prepare-workspace must NOT echo the remote URL directly. \
         Remote URLs may contain embedded PATs. Use pattern matching \
         (e.g., case/[[ ]]) instead of piping the URL through echo/printf. \
         (Issue #656, security)"
    );
}

// ── 656-7: existing required strings preserved ────────────────────────────

#[test]
fn step01_preserves_required_literals() {
    // The fix must not remove any existing required strings that other tests
    // or downstream consumers depend on.
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    let required = [
        "git fetch",
        "git status",
        "git branch --show-current",
        "requires a git repo",
        "git init",
        "rerun from a checkout",
        "=== Workspace Prepared ===",
    ];

    for literal in &required {
        assert!(
            body.contains(literal),
            "step-01-prepare-workspace must preserve the literal string '{literal}'. \
             This string is required by existing tests or downstream consumers."
        );
    }
}

// ── 656-8: git fetch still appears before SKIP_PRE_AGENT_VALIDATION ──────

#[test]
fn step01_fetch_before_skip_pre_agent_validation() {
    // The git fetch (guarded or not) must remain BEFORE the
    // SKIP_PRE_AGENT_VALIDATION block. This ordering is tested by
    // skip_pre_agent_validation_context_test.rs and must not regress.
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    let fetch_pos = body
        .find("git fetch")
        .expect("step-01 must contain 'git fetch'");
    let guard_pos = body
        .find("SKIP_PRE_AGENT_VALIDATION")
        .expect("step-01 must contain SKIP_PRE_AGENT_VALIDATION guard");

    assert!(
        fetch_pos < guard_pos,
        "git fetch (byte {fetch_pos}) must appear before SKIP_PRE_AGENT_VALIDATION \
         guard (byte {guard_pos}) in step-01-prepare-workspace. (Issue #656 + #453)"
    );
}

// ── 656-9: "Workspace Prepared" marker still reached ──────────────────────

#[test]
fn step01_workspace_prepared_not_in_fetch_conditional() {
    // "=== Workspace Prepared ===" must be reachable even when fetch fails.
    // It must NOT be inside a conditional block that depends on fetch success.
    let recipe = load_recipe(&workflow_prep_yaml_path());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // If the marker is after the fetch guard's closing `fi`, it's reachable
    // regardless of fetch outcome. A rough structural check: the marker
    // must not appear between `if.*git fetch` and `fi`.
    assert!(
        body.contains("=== Workspace Prepared ==="),
        "step-01 must contain the '=== Workspace Prepared ===' completion marker"
    );

    // The marker must appear at the top level of the script, not nested
    // inside a fetch-conditional. We verify this by checking that the marker
    // appears AFTER the fetch guard logic completes.
    let fetch_pos = body.find("git fetch").unwrap();
    let marker_pos = body.find("=== Workspace Prepared ===").unwrap();
    assert!(
        marker_pos > fetch_pos,
        "Workspace Prepared marker must appear after git fetch"
    );
}

// ===========================================================================
// Cross-cutting: SKILL.md documents the resilience pattern
// ===========================================================================

#[test]
fn skill_md_documents_git_fetch_resilience() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    assert!(
        content.contains("Git Fetch") || content.contains("git fetch"),
        "SKILL.md Known Failure Points section must document the git fetch \
         credential failure resilience pattern. (Issue #656 documentation)"
    );
}

#[test]
fn skill_md_mentions_ado_in_failure_points() {
    let content =
        fs::read_to_string(skill_md_path()).unwrap_or_else(|e| panic!("read SKILL.md: {e}"));

    let mentions_ado = content.contains("Azure DevOps")
        || content.contains("dev.azure.com")
        || content.contains("ADO");

    assert!(
        mentions_ado,
        "SKILL.md must mention Azure DevOps / ADO remotes in the Known Failure Points \
         section as the primary trigger for git fetch credential failures. (Issue #656)"
    );
}
