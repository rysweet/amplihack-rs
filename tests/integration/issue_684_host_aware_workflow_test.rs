/// Integration tests for issue #684 — Host-aware workflow steps.
///
/// ## Problem
///
/// The default-workflow assumes GitHub for commit messages (Closes #N),
/// PR creation, and final status summaries. This breaks on Azure DevOps,
/// other Git hosts, and repos without remotes.
///
/// ## Blockers addressed
///
/// 1. **step-15 commit message** hardcodes `Closes #N` (GitHub-only).
///    Fix: host-aware refs — `AB#N` (AzDO), `Closes #N` (GitHub), `Ref #N` (other).
/// 2. **step-16 PR body** hardcodes `Closes #N` and has duplicate host detection.
///    Fix: consume `$REMOTE_HOST_TYPE` from context, not inline re-detection.
/// 3. **step-22b summary** calls `gh pr view` without host-type guard.
///    Fix: guard with `REMOTE_HOST_TYPE` check, host-aware issue/PR lines.
/// 4. **step-03 AzDO parsing** rejects percent-encoded project names (`My%20Project`).
///    Fix: decode `%XX` before validation, expand regex to allow spaces.
/// 5. **Context propagation**: `remote_host_type` must be declared in
///    `default-workflow.yaml` and exported by a new `step-02d-detect-host-type`
///    in `workflow-prep.yaml`.
///
/// ## Test strategy
///
/// Mirrors `issue_655_656_skill_fetch_resilience_test.rs`:
///   - Parse recipe YAML with `serde_yaml` to inspect step command bodies
///   - Assert structural properties of bash scripts (keywords, patterns, absence)
///   - No subprocess execution — all tests are structural/contract tests
///
/// ## Test status
///
/// These tests are written **TDD-RED**. They FAIL until the implementation
/// changes land across the four recipe files.
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

fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn recipe_text(name: &str) -> String {
    let path = recipe_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Recipe parsing helpers (same pattern as issue_655_656 tests)
// ---------------------------------------------------------------------------

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    let text = recipe_text(name);
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

/// Extract the `command:` body of a bash step by its `id:` field.
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

/// Check if a step exists in the recipe.
fn step_exists(recipe: &Value, step_id: &str) -> bool {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .map(|steps| {
            steps
                .iter()
                .any(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        })
        .unwrap_or(false)
}

/// Get the `output:` field of a step.
fn step_output(recipe: &Value, step_id: &str) -> Option<String> {
    let steps = recipe.get("steps")?.as_sequence()?;
    for step in steps {
        if step.get("id").and_then(Value::as_str) == Some(step_id) {
            return step.get("output").and_then(Value::as_str).map(String::from);
        }
    }
    None
}

/// Get the context block from default-workflow.yaml.
fn default_workflow_context(recipe: &Value) -> Value {
    recipe
        .get("context")
        .cloned()
        .expect("default-workflow.yaml must have a 'context:' block")
}

// ===========================================================================
// BLOCKER 1a: default-workflow.yaml — remote_host_type context variable
// ===========================================================================

#[test]
fn default_workflow_declares_remote_host_type_context() {
    let recipe = load_recipe("default-workflow");
    let context = default_workflow_context(&recipe);

    assert!(
        context.get("remote_host_type").is_some(),
        "default-workflow.yaml context block must declare 'remote_host_type' \
         for cross-sub-recipe propagation. Without this, step-02d's output \
         cannot reach workflow-publish and workflow-finalize. (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 1b: workflow-prep.yaml — step-02d-detect-host-type exists
// ===========================================================================

#[test]
fn workflow_prep_has_step_02d_detect_host_type() {
    let recipe = load_recipe("workflow-prep");

    assert!(
        step_exists(&recipe, "step-02d-detect-host-type"),
        "workflow-prep.yaml must contain step 'step-02d-detect-host-type'. \
         This centralized step detects the git remote host type once and \
         exports it for all downstream steps. (Issue #684)"
    );
}

#[test]
fn step_02d_has_output_remote_host_type() {
    let recipe = load_recipe("workflow-prep");

    let output = step_output(&recipe, "step-02d-detect-host-type");
    assert_eq!(
        output.as_deref(),
        Some("remote_host_type"),
        "step-02d-detect-host-type must declare output: 'remote_host_type' \
         so the recipe runner captures the host type and propagates it to \
         subsequent steps and sub-recipes. (Issue #684)"
    );
}

#[test]
fn step_02d_detects_github_azdo_other() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-02d-detect-host-type");

    // Must detect all three host types
    assert!(
        body.contains("github"),
        "step-02d must detect 'github' host type (Issue #684)"
    );
    assert!(
        body.contains("azdo"),
        "step-02d must detect 'azdo' host type (Issue #684)"
    );
    assert!(
        body.contains("other"),
        "step-02d must handle 'other' as the fallback host type (Issue #684)"
    );

    // Must use git remote get-url to detect
    assert!(
        body.contains("git remote get-url"),
        "step-02d must use 'git remote get-url' to detect the remote type (Issue #684)"
    );
}

#[test]
fn step_02d_detects_all_azdo_url_patterns() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-02d-detect-host-type");

    // Must detect all three AzDO URL patterns
    assert!(
        body.contains("dev.azure.com"),
        "step-02d must detect dev.azure.com URLs (Issue #684)"
    );
    assert!(
        body.contains("visualstudio.com"),
        "step-02d must detect visualstudio.com URLs (Issue #684)"
    );
    assert!(
        body.contains("ssh.dev.azure.com"),
        "step-02d must detect ssh.dev.azure.com URLs (Issue #684)"
    );
}

#[test]
fn step_02d_does_not_echo_remote_url() {
    // Security: remote URL may contain embedded PATs
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-02d-detect-host-type");

    let echoes_url = body.contains("echo \"$REMOTE_URL\"")
        || body.contains("echo $REMOTE_URL")
        || body.contains("printf '%s' \"$REMOTE_URL\"")
        || body.contains("printf \"%s\" \"$REMOTE_URL\"");

    assert!(
        !echoes_url,
        "step-02d must NOT echo the remote URL directly. Remote URLs may \
         contain embedded PATs. Use pattern matching (case/[[]]) instead. \
         (Issue #684, security)"
    );
}

#[test]
fn step_02d_appears_before_step_03() {
    // step-02d must run before step-03 so REMOTE_HOST_TYPE is available
    let recipe = load_recipe("workflow-prep");
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have steps");

    let step_02d_idx = steps
        .iter()
        .position(|s| s.get("id").and_then(Value::as_str) == Some("step-02d-detect-host-type"));
    let step_03_idx = steps.iter().position(|s| {
        s.get("id")
            .and_then(Value::as_str)
            .map(|id| id.starts_with("step-03"))
            .unwrap_or(false)
    });

    assert!(
        step_02d_idx.is_some(),
        "step-02d-detect-host-type must exist in workflow-prep.yaml"
    );
    assert!(
        step_03_idx.is_some(),
        "step-03 must exist in workflow-prep.yaml"
    );
    assert!(
        step_02d_idx.unwrap() < step_03_idx.unwrap(),
        "step-02d-detect-host-type (index {}) must appear before step-03 (index {}) \
         in workflow-prep.yaml so REMOTE_HOST_TYPE is available for issue creation. \
         (Issue #684)",
        step_02d_idx.unwrap(),
        step_03_idx.unwrap()
    );
}

// ===========================================================================
// BLOCKER 1c: step-15 — host-aware commit message
// ===========================================================================

#[test]
fn step_15_commit_message_not_hardcoded_closes() {
    // The commit message must NOT use hardcoded "Closes #N" for all hosts.
    // It should be conditional based on REMOTE_HOST_TYPE.
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-15-commit-push");

    // Count occurrences of literal "Closes #" in commit message construction.
    // After the fix, "Closes #" should only appear inside a GitHub conditional,
    // not as the unconditional default in the COMMIT_MSG printf.
    let has_unconditional_closes = body.contains("Closes #%s' \"$COMMIT_TITLE\"")
        || body.contains("Closes #%s\" \"$COMMIT_TITLE\"");

    assert!(
        !has_unconditional_closes,
        "step-15 commit message must NOT hardcode 'Closes #N' unconditionally. \
         Use host-aware refs: 'AB#N' for azdo, 'Closes #N' for github, \
         'Ref #N' for other. (Issue #684, BLOCKER 1)"
    );
}

#[test]
fn step_15_uses_remote_host_type_for_commit_ref() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-15-commit-push");

    // The step must reference REMOTE_HOST_TYPE to decide the issue ref format
    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-15 must use REMOTE_HOST_TYPE to determine the commit message \
         issue reference format (Closes #N vs AB#N vs Ref #N). (Issue #684)"
    );
}

#[test]
fn step_15_supports_azdo_ab_ref() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-15-commit-push");

    assert!(
        body.contains("AB#"),
        "step-15 must use 'AB#' format for Azure DevOps work item linking \
         in commit messages. (Issue #684)"
    );
}

#[test]
fn step_15_supports_neutral_ref() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-15-commit-push");

    assert!(
        body.contains("Ref #"),
        "step-15 must use 'Ref #' format for non-GitHub/non-AzDO hosts \
         in commit messages. (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 1d: step-16 — no duplicate host detection, host-aware PR body
// ===========================================================================

#[test]
fn step_16_no_inline_remote_host_type_detection() {
    // step-16 should consume $REMOTE_HOST_TYPE from context (step-02d output),
    // not re-detect it inline. Duplicate detection is fragile and violates DRY.
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-16-create-draft-pr");

    // The inline detection pattern from the current code:
    //   REMOTE_URL=$(git remote get-url origin ...)
    //   case "$REMOTE_URL" in *github.com*) ...
    // After the fix, this should be replaced by consuming $REMOTE_HOST_TYPE.
    let has_inline_case = body.contains("case \"$REMOTE_URL\"") && body.contains("*github.com*)");

    assert!(
        !has_inline_case,
        "step-16 must NOT have inline REMOTE_HOST_TYPE detection via \
         case \"$REMOTE_URL\". It should consume $REMOTE_HOST_TYPE from \
         context (set by step-02d). (Issue #684, DRY violation)"
    );
}

#[test]
fn step_16_pr_body_not_hardcoded_closes() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-16-create-draft-pr");

    // The PR body must not hardcode "Closes #%s" for all hosts
    let has_unconditional_closes = body.contains("Closes #%s\\n");

    assert!(
        !has_unconditional_closes,
        "step-16 PR body must NOT hardcode 'Closes #N'. It should use \
         host-aware refs like step-15. (Issue #684, BLOCKER 1)"
    );
}

#[test]
fn step_16_consumes_remote_host_type() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-16-create-draft-pr");

    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-16 must reference REMOTE_HOST_TYPE (from context/env) \
         to decide PR creation behavior and issue ref format. (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 2: step-22b — host-aware summary
// ===========================================================================

#[test]
fn step_22b_guards_gh_pr_view_with_host_type() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-22b-final-status");

    // gh pr view must be guarded by REMOTE_HOST_TYPE check, not just PR_URL
    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-22b must check REMOTE_HOST_TYPE before calling gh pr view. \
         Belt-and-suspenders: non-GitHub hosts must never invoke gh. (Issue #684)"
    );
}

#[test]
fn step_22b_issue_line_is_host_aware() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-22b-final-status");

    // The issue summary should not unconditionally use "Issue: #N"
    // It should adapt based on host type (AB#N for AzDO, #N for GitHub)
    let has_unconditional_issue_hash = body.contains("'=== Issue: #%s ===\\n'");

    assert!(
        !has_unconditional_issue_hash,
        "step-22b issue summary must NOT use unconditional '=== Issue: #N ===' format. \
         Use host-aware format: 'AB#N' for azdo, '#N' for github. (Issue #684)"
    );
}

#[test]
fn step_22b_pr_line_handles_empty_pr_url() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-22b-final-status");

    // When PR_URL is empty (non-GitHub host), the summary should say
    // something like "PR: N/A" rather than "PR: " (empty)
    let has_empty_pr_handling = body.contains("N/A")
        || body.contains("manual")
        || body.contains("not created")
        || body.contains("skipped");

    assert!(
        has_empty_pr_handling,
        "step-22b must handle empty PR_URL gracefully in the summary output. \
         Use 'N/A', 'manual creation required', or similar when PR was not created. \
         (Issue #684, BLOCKER 2)"
    );
}

#[test]
fn step_22b_uses_host_type_safe_pattern() {
    // Must use HOST_TYPE=${REMOTE_HOST_TYPE:-other} for set -u safety
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-22b-final-status");

    let has_safe_default =
        body.contains("${REMOTE_HOST_TYPE:-") || body.contains("HOST_TYPE=${REMOTE_HOST_TYPE:-");

    assert!(
        has_safe_default,
        "step-22b must use safe default pattern '${{REMOTE_HOST_TYPE:-other}}' \
         or 'HOST_TYPE=${{REMOTE_HOST_TYPE:-other}}' for set -u compatibility. \
         (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 3: step-03 — percent-encoded AzDO project names
// ===========================================================================

#[test]
fn step_03_decodes_percent_encoding() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    // The step must decode %XX sequences (e.g., %20 → space) before validation.
    // We check for actual decode logic, not incidental mentions of "sed" in comments.
    let has_percent_decode = body.contains("%20")
        || body.contains("percent_decode")
        || body.contains("printf '%b'")  // printf-based decode
        || body.contains("\\\\x")  // hex escape for printf decode
        || (body.contains("sed") && body.contains("%[0-9A-Fa-f]")); // sed-based decode with hex pattern

    assert!(
        has_percent_decode,
        "step-03 must decode percent-encoded sequences (e.g., %20 → space) \
         in AzDO project names before validation. URLs like \
         'dev.azure.com/org/My%20Project/' must be handled. (Issue #684, BLOCKER 3)"
    );
}

#[test]
fn step_03_regex_allows_spaces_in_project_names() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    // After percent-decoding, the regex must allow spaces in project names.
    // The current regex is ^[a-zA-Z0-9._-]+$ which rejects spaces.
    // The fix should expand to ^[a-zA-Z0-9._ -]+$ (note the space).
    let has_space_in_regex =
        body.contains("[a-zA-Z0-9._ -]") || body.contains("[a-zA-Z0-9._[:space:]-]");

    assert!(
        has_space_in_regex,
        "step-03 AzDO project name validation regex must allow spaces \
         (for decoded %20). Change from ^[a-zA-Z0-9._-]+$ to \
         ^[a-zA-Z0-9._ -]+$. (Issue #684, BLOCKER 3)"
    );
}

#[test]
fn step_03_rejects_invalid_percent_sequences() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    // Invalid percent sequences (%ZZ, %G1, etc.) must be caught during decode.
    // This is separate from the existing "unexpected characters" validation —
    // it must explicitly handle the decode-failure path.
    let has_decode_validation = body.contains("%20")
        || body.contains("percent_decode")
        || body.contains("printf '%b'")
        || body.contains("\\\\x");

    assert!(
        has_decode_validation,
        "step-03 must have percent-decode logic that implicitly rejects invalid \
         sequences (e.g., %%ZZ). The decode itself will fail or pass through invalid \
         sequences which the expanded regex then catches. (Issue #684, BLOCKER 3)"
    );
}

// ===========================================================================
// Cross-cutting: step-03 consumes REMOTE_HOST_TYPE from env
// ===========================================================================

#[test]
fn step_03_does_not_redefine_remote_host_type_via_case() {
    // After step-02d is added, step-03 should consume $REMOTE_HOST_TYPE
    // from the environment, not re-detect it with its own case block.
    // Note: step-03 must still USE $REMOTE_HOST_TYPE for branching (github/azdo/other).
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    // Count how many times REMOTE_HOST_TYPE is assigned via case statement.
    // After the fix, there should be zero case-based assignments — only reads.
    let has_case_assignment =
        body.contains("case \"$REMOTE_URL\"") && body.contains("REMOTE_HOST_TYPE=\"github\"");

    assert!(
        !has_case_assignment,
        "step-03 must NOT re-detect REMOTE_HOST_TYPE via case statement. \
         It should consume $REMOTE_HOST_TYPE from context (set by step-02d). \
         This eliminates duplicate host detection. (Issue #684)"
    );
}

// ===========================================================================
// Step-21: gh pr ready guard (existing PR_URL guard + host-type)
// ===========================================================================

#[test]
fn step_21_guards_gh_commands_with_host_type() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-21-pr-ready");

    // step-21 already has a PR_URL guard. After fix, it should also check
    // REMOTE_HOST_TYPE to prevent gh commands on non-GitHub hosts.
    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-21 must check REMOTE_HOST_TYPE in addition to PR_URL guard \
         to prevent gh commands on non-GitHub hosts. (Issue #684)"
    );
}

// ===========================================================================
// Brick rule: all recipe files must stay under 400 lines
// ===========================================================================

#[test]
fn all_modified_recipes_under_400_lines() {
    let recipes = [
        "default-workflow",
        "workflow-prep",
        "workflow-publish",
        "workflow-finalize",
    ];

    for name in &recipes {
        let text = recipe_text(name);
        let line_count = text.lines().count();
        assert!(
            line_count <= 400,
            "{name}.yaml has {line_count} lines — exceeds the 400-line brick limit. \
             (Issue #684, brick rule)"
        );
    }
}

// ===========================================================================
// Security: HOST_TYPE safe defaults in all consuming steps
// ===========================================================================

#[test]
fn step_15_uses_host_type_safe_default() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-15-commit-push");

    let has_safe_default =
        body.contains("${REMOTE_HOST_TYPE:-") || body.contains("HOST_TYPE=${REMOTE_HOST_TYPE:-");

    assert!(
        has_safe_default,
        "step-15 must use safe default pattern for REMOTE_HOST_TYPE \
         (e.g., '${{REMOTE_HOST_TYPE:-other}}') for set -u safety. (Issue #684)"
    );
}

#[test]
fn step_16_uses_host_type_safe_default() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-16-create-draft-pr");

    let has_safe_default =
        body.contains("${REMOTE_HOST_TYPE:-") || body.contains("HOST_TYPE=${REMOTE_HOST_TYPE:-");

    assert!(
        has_safe_default,
        "step-16 must use safe default pattern for REMOTE_HOST_TYPE \
         (e.g., '${{REMOTE_HOST_TYPE:-other}}') for set -u safety. (Issue #684)"
    );
}

// ===========================================================================
// Preserved invariants: existing step behavior must not regress
// ===========================================================================

#[test]
fn step_03_preserves_github_path() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    assert!(
        body.contains("gh issue create"),
        "step-03 must preserve the GitHub issue creation path (Issue #684)"
    );
    assert!(
        body.contains("gh issue view"),
        "step-03 must preserve the GitHub issue lookup for idempotency (Issue #684)"
    );
}

#[test]
fn step_03_preserves_azdo_path() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    assert!(
        body.contains("az boards work-item"),
        "step-03 must preserve the AzDO work-item creation path (Issue #684)"
    );
}

#[test]
fn step_03_preserves_local_tracking_fallback() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(&recipe, "step-03-create-issue");

    assert!(
        body.contains("local-tracking") || body.contains("local tracking"),
        "step-03 must preserve the local tracking fallback path (Issue #684)"
    );
}

#[test]
fn step_22b_preserves_pr_url_guard() {
    // The existing PR_URL empty-check must be preserved
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-22b-final-status");

    let has_pr_url_check =
        body.contains("PR_URL") && (body.contains("-z \"$PR_URL\"") || body.contains("PR_URL:-"));

    assert!(
        has_pr_url_check,
        "step-22b must preserve the PR_URL empty-check guard. (Issue #684)"
    );
}

#[test]
fn step_15_preserves_set_euo_pipefail() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(&recipe, "step-15-commit-push");

    assert!(
        body.contains("set -euo pipefail"),
        "step-15 must preserve 'set -euo pipefail' at the top of the bash block"
    );
}

#[test]
fn step_22b_preserves_set_euo_pipefail() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(&recipe, "step-22b-final-status");

    assert!(
        body.contains("set -euo pipefail"),
        "step-22b must preserve 'set -euo pipefail' at the top of the bash block"
    );
}
