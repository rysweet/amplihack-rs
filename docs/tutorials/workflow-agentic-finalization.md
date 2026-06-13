# Tutorial: Workflow Agentic Finalization

This tutorial shows the issue #769 finalization flow for an already-implemented
workflow branch. It uses the `workflow_agentic_finalization.sh` helper and the
schema-validation gate wired into `workflow-finalize.yaml`.

## What you will learn

- Clean generated `.claude/runtime` artifacts from finalization scope.
- Run Artifact Guard before broad staging.
- Run `workflow-finalize` against an existing issue branch.
- Interpret `ready`, `blocked`, and `finalized` decisions without confusing PR
  readiness with terminal merge/no-diff closure.
- Stage only issue-relevant files and prepare a PR body.

## Prerequisites

- `amplihack` is installed.
- You are in a Git checkout on the feature branch.
- `jq`, `git`, and `gh` are installed.
- The branch contains implementation changes for an issue.

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## 1. Start from the Existing Branch

Confirm the branch and current changes:

```bash
git branch --show-current
git --no-pager status --short
```

For this tutorial, the branch is an issue branch:

```text
feat/issue-769-the-recipe-exitfinalization-step-is-brittle-and-of
```

The branch may already contain staged or modified source files. Do not reset the
branch. Preserve the implementation and finalize it safely.

---

## 2. Remove Generated Runtime Output

If `.claude/runtime/` appears in status output, remove it from commit scope:

```bash
git restore --staged .claude/runtime 2>/dev/null || true
```

If runtime output contains evidence you need, extract only a redacted snippet
outside the repository:

```bash
mkdir -p /tmp/amplihack-finalization-evidence
grep -R "workflow-finalize" .claude/runtime 2>/dev/null \
  | sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' \
  > /tmp/amplihack-finalization-evidence/workflow-finalize-redacted.log || true
```

Then remove the generated directory:

```bash
rm -rf .claude/runtime
git --no-pager status --short
```

The status output should not include `.claude/runtime/`.

---

## 3. Run Artifact Guard

Run the guard before staging anything broadly:

```bash
amplihack hygiene artifact-guard --repo . --mode all
```

Expected output is no violations and exit code `0`. If the guard reports a
blocked path, fix that path before continuing.

---

## 4. Run Agentic Finalization

Run `workflow-finalize` with branch and issue context:

```bash
result_file="$(mktemp -t workflow-finalize-XXXXXX.json)"

amplihack recipe run workflow-finalize \
  -c repo_path="$PWD" \
  -c branch_name="$(git branch --show-current)" \
  -c issue_number=769 \
  --format json > "$result_file"
```

If a PR already exists, pass its identity too:

```bash
amplihack recipe run workflow-finalize \
  -c repo_path="$PWD" \
  -c branch_name="$(git branch --show-current)" \
  -c issue_number=769 \
  -c pr_number=123 \
  -c pr_url="https://github.com/rysweet/amplihack-rs/pull/123" \
  --format json > "$result_file"
```

---

## 5. Read the Finalization Decision

Inspect the decision and terminal state:

```bash
jq '.. | objects | select(has("decision") or has("terminal_state")) | {
  decision,
  terminal_success,
  terminal_state,
  terminal_reason,
  publish_status,
  ready_for_review
}' "$result_file"
```

A branch that is ready for PR review looks like this:

```json
{
  "decision": "ready",
  "terminal_success": false,
  "terminal_state": "FOLLOWUP_CREATED",
  "terminal_reason": "implementation is committed, validation passed, and PR is open for issue #769",
  "publish_status": "FOLLOWUP_CREATED",
  "ready_for_review": true
}
```

This means the finalization phase has enough evidence for review readiness. It
does not mean the PR is merged.

An already-complete no-diff branch looks like this:

```json
{
  "decision": "finalized",
  "terminal_success": true,
  "terminal_state": "NO_DIFF_SUCCESS",
  "terminal_reason": "branch has no meaningful diff against the base ref",
  "publish_status": "NO_DIFF_SUCCESS",
  "ready_for_review": false
}
```

A blocked branch names the required action:

```json
{
  "decision": "blocked",
  "terminal_success": false,
  "terminal_state": "FAILED_INVALID_EVIDENCE",
  "terminal_reason": "Artifact Guard blocked generated runtime artifacts in .claude/runtime/",
  "publish_status": "FAILED_INVALID_EVIDENCE",
  "ready_for_review": false
}
```

Do not treat a blocked decision as success even if earlier workflow steps exited
successfully.

---

## 6. Validate the Changed Area

Run the finalization checks:

```bash
pre-commit run --all-files

cargo test --test workflow_finalize_terminal_state
cargo test --test workflow_finalize_resilience
cargo test --test workflow_agentic_finalization
```

Fix failures caused by the finalization change and rerun the failing command.
Unrelated baseline failures belong in the PR notes, not in hidden local state.

---

## 7. Stage the Review Scope

Stage only issue-relevant files. Example:

```bash
git add \
  amplifier-bundle/recipes/workflow-finalize.yaml \
  amplifier-bundle/tools/workflow_agentic_finalization.sh \
  tests/integration/workflow_agentic_finalization.rs \
  docs/reference/workflow-agentic-finalization.md \
  docs/howto/finalize-existing-workflow-branch.md \
  docs/tutorials/workflow-agentic-finalization.md \
  docs/index.md \
  docs/recipes/README.md \
  docs/tutorials/README.md
```

Adjust the list to the actual diff. Include related recipe, helper, test, docs,
discoverability, and `.gitignore` files only when they directly support the
issue.

Review the staged diff:

```bash
git --no-pager diff --cached --name-status
```

The staged diff must not include `.claude/runtime/`, build output, dependency
trees, logs, or local configuration.

---

## 8. Publish the PR

Commit and push:

```bash
git commit -m "Fix workflow finalization evidence handling for issue #769"
git push -u origin "$(git branch --show-current)"
```

Create the PR if needed:

```bash
gh pr create \
  --base main \
  --head "$(git branch --show-current)" \
  --title "Fix workflow finalization evidence handling" \
  --body "Refs #769

Summary:
- Adds bounded agentic finalization evidence validation.
- Preserves deterministic terminal-state and final-status gates.
- Excludes generated .claude/runtime artifacts from commit scope.

Validation:
- pre-commit run --all-files
- cargo test --test workflow_finalize_terminal_state
- cargo test --test workflow_finalize_resilience
- cargo test --test workflow_agentic_finalization"
```

If a PR already exists, update its body with the same summary and validation
notes:

```bash
gh pr edit 123 --body-file <(cat <<'EOF'
Refs #769

Summary:
- Adds bounded agentic finalization evidence validation.
- Preserves deterministic terminal-state and final-status gates.
- Excludes generated .claude/runtime artifacts from commit scope.

Validation:
- pre-commit run --all-files
- cargo test --test workflow_finalize_terminal_state
- cargo test --test workflow_finalize_resilience
- cargo test --test workflow_agentic_finalization
EOF
)
```

The workflow is ready for review when the PR is open, validation is passing,
Artifact Guard is clean, and finalization reports `decision="ready"` with
`ready_for_review=true`.

The workflow is terminally finalized only when finalization reports
`decision="finalized"` with `NO_DIFF_SUCCESS`, `MERGED`, or `CLOSED_OBSOLETE`.

## Next Steps

- Use [How to Finalize an Existing Workflow Branch](../howto/finalize-existing-workflow-branch.md) for the task-oriented checklist.
- Use [Workflow Agentic Finalization Reference](../reference/workflow-agentic-finalization.md) for schema and implementation requirements.
