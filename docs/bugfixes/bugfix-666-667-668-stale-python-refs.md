# Bug Fixes #666, #667, #668 — Stale Python References & Rate-Limit Resilience

> **PR:** Fixes #666, #667, #668 in a single branch.

---

## Summary

Three documentation and configuration bugs fixed in one pass:

| Bug | Title | Root Cause | Fix |
|-----|-------|------------|-----|
| #666 | Stale `orch_helper.py` references | Python-to-Rust migration left documentation pointing at removed files | Updated 13 doc files to reference native Rust equivalents |
| #667 | `build_publish_validation_scope.py` hard dependency | Test infrastructure references legacy Python script | Confirmed existing graceful handling; no code change needed |
| #668 | Rate-limit kills recipe run | `step-06c` treated API failures as hard errors | Added `continue_on_error: true` to non-critical doc-polish step |

---

## Bug #666: Stale Python Tool References

### Problem

After the Python-to-Rust migration, several documentation files still
referenced Python scripts that no longer exist:

- `amplifier-bundle/tools/orch_helper.py` → replaced by `amplihack orch helper`
  (native Rust at `crates/amplihack-cli/src/commands/orch.rs`)
- `session_tree.py` → replaced by `AMPLIHACK_MAX_DEPTH` env var (native recursion guard)
- `ci_status.py` / `github_issue.py` → replaced by `gh CLI` commands

### What Changed

**Active documentation** (skills, tutorials, references) was updated to
reference the Rust replacements directly:

| File | Change |
|------|--------|
| `amplifier-bundle/skills/amplihack-expert/SKILL.md` L148-149 | `session_tree.py` → native recursion guard; `orch_helper.py` → `amplihack orch helper` |
| `amplifier-bundle/skills/amplihack-expert/reference.md` L124 | `ci_status.py, github_issue.py` → `gh CLI` / `amplihack orch helper` |
| `amplifier-bundle/skills/dependency-resolver/README.md` L72 | `ci_status.py` → `gh CLI` (`gh run list`) |
| `docs/atlas/runtime-topology/README.md` L14 | `session_tree.py` → native recursion guard |
| `docs/tutorials/dev-orchestrator-tutorial.md` L393-406 | Troubleshooting rewritten for native Rust |
| `docs/reference/resolve-bundle-asset-command.md` L28,59 | `helper-path` → `amplihack orch helper` |

**Historical documentation** (audits, P1 plans, publish-validation tutorials)
received inline deprecation notes preserving historical accuracy:

```markdown
> **Note:** `orch_helper.py` has been replaced by native Rust (`amplihack orch helper`).
> This example references the legacy Python codebase for historical context.
```

Files annotated:
- `docs/tutorials/workflow-publish-import-validation.md`
- `docs/reference/workflow-publish-import-validation.md`
- `docs/howto/configure-workflow-publish-import-validation.md`
- `docs/recipes/P1_WORKFLOW_RELIABILITY_FIXES.md`
- `docs/audits/recipe-runner-quality-robustness-audit.md`

### What Was NOT Changed (by design)

| File/Area | Reason |
|-----------|--------|
| `docs/concepts/amplihack-retirement-direction.md` | This IS the migration doc — Python refs are the point |
| `docs/reference/orch-run-command.md` L167 | Already says "port of" — accurate as-is |
| `oxidizer-workflow.yaml` | Python-to-Rust migration recipe — Python refs legitimate |
| `dynamic-debugger` | `debugpy` is a real Python debugging tool |
| `code-atlas` test scripts | Python YAML parsing — tracked separately |

### Usage

After this fix, all documentation paths resolve correctly:

```bash
# Resolves to native Rust (no Python dependency)
amplihack resolve-bundle-asset helper-path

# Native recursion guard — no session_tree.py needed
export AMPLIHACK_MAX_DEPTH=3
amplihack recipe run smart-orchestrator -c task_description="..." -c repo_path=.

# CI status — use gh CLI directly
gh run list --limit 5
gh api repos/{owner}/{repo}/actions/runs
```

---

## Bug #667: `build_publish_validation_scope.py` Dependency

### Problem

Test infrastructure at `amplifier-bundle/recipes/tests/` references
`build_publish_validation_scope.py`, a legacy Python script.

### Analysis

Investigation confirmed existing graceful handling:

- **`test-pr-always-opens.sh` L131-133**: Already implements warn-and-continue:
  ```bash
  if ! command -v build_publish_validation_scope.py >/dev/null 2>&1; then
      echo "WARN: build_publish_validation_scope.py missing, continuing" >&2
  ```
- **`test-static-guard-validation-scope.sh`**: Legitimate test fixture that
  validates the static guard regex pattern itself — it _should_ reference
  the `.py` pattern it's testing for.

### Result

No code changes needed. Existing handling is correct.

---

## Bug #668: Rate-Limit Resilience

### Problem

When an upstream API (Copilot, Anthropic) returns a rate-limit error during
a recipe step, `recipe-runner-rs` treats it as a hard failure. Non-critical
steps like documentation polish would abort the entire recipe.

### What Changed

**`workflow-design.yaml` — step-06c-documentation-refinement:**

```yaml
- id: "step-06c-documentation-refinement"
  agent: "amplihack:documentation-writer"
  working_dir: "{{worktree_setup.worktree_path}}"
  continue_on_error: true    # ← NEW: rate-limit resilience
  prompt: |
    # Step 6c: Documentation Refinement
    ...
```

**`amplihack-expert/SKILL.md` — new Known Failure Points section:**

```markdown
## Known Failure Points

### Rate-limit resilience

When an upstream API returns a rate-limit error during an agent step, the
recipe runner treats it as a hard failure by default. Non-critical steps
such as `step-06c-documentation-refinement` set `continue_on_error: true`
so that documentation polish does not abort the entire recipe. For critical
steps, the agent runtime retries with exponential back-off (configured by
the SDK adapter). If a rate-limit error aborts your workflow, check which
step failed — if it is a polish/review step, adding `continue_on_error: true`
is the recommended fix.
```

### Configuration

The `continue_on_error` key is supported by `recipe-runner-rs` on any step.
Use it for non-critical steps where a transient failure should not block
the overall workflow:

```yaml
# In any recipe YAML step definition:
- id: "my-non-critical-step"
  agent: "amplihack:some-agent"
  continue_on_error: true     # Step failure logged but does not abort recipe
  prompt: |
    ...
```

**When to use `continue_on_error: true`:**
- Documentation refinement steps
- Style/formatting polish steps
- Optional notification steps
- Any step where partial output is acceptable

**When NOT to use it:**
- Test execution steps (must know if tests fail)
- Security validation steps
- Commit/push steps (must know if push fails)
- Any step that produces artifacts consumed by later steps

---

## Exhaustive Audit Results

After fixing all three bugs, a full `.py` invocation audit confirmed no
remaining stale Python invocations in the bundle:

```bash
grep -rn '\.py\b' amplifier-bundle/recipes/ amplifier-bundle/skills/ \
  amplifier-bundle/agents/ amplifier-bundle/behaviors/ \
  --include='*.yaml' --include='*.yml' --include='*.sh' --include='*.md'
```

All remaining `.py` references fall into these categories (no action needed):

| Category | Example | Reason |
|----------|---------|--------|
| Test fixtures | `test-static-guard-validation-scope.sh` | Tests the guard regex pattern itself |
| Graceful fallback | `test-pr-always-opens.sh` | Already warn-and-continue |
| Glob patterns | `workflow-finalize.yaml` ("no `test_*.py` patterns") | Pattern description, not invocation |
| Example code | `session-learning/SKILL.md`, `pr-review-assistant/` | Python filenames in code review examples |
| Real tools | `dynamic-debugger` (debugpy) | Legitimate Python debugging tool |

---

## Files Modified (14 total)

| File | Bug |
|------|-----|
| `amplifier-bundle/recipes/workflow-design.yaml` | #668 |
| `amplifier-bundle/skills/amplihack-expert/SKILL.md` | #666, #668 |
| `amplifier-bundle/skills/amplihack-expert/reference.md` | #666 |
| `amplifier-bundle/skills/dependency-resolver/README.md` | #666 |
| `docs/atlas/runtime-topology/README.md` | #666 |
| `docs/audits/recipe-runner-quality-robustness-audit.md` | #666 |
| `docs/howto/configure-workflow-publish-import-validation.md` | #666 |
| `docs/recipes/P1_WORKFLOW_RELIABILITY_FIXES.md` | #666 |
| `docs/reference/environment-variables.md` | #666 |
| `docs/reference/resolve-bundle-asset-command.md` | #666 |
| `docs/reference/workflow-publish-import-validation.md` | #666 |
| `docs/tutorials/dev-orchestrator-tutorial.md` | #666 |
| `docs/tutorials/workflow-publish-import-validation.md` | #666 |
| `CHANGELOG.md` | #666, #667, #668 |
