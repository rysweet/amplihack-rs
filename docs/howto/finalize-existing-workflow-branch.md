# How to Finalize an Existing Workflow Branch

Use this guide to recover a branch that already contains implementation work,
exclude generated artifacts, validate finalization evidence, and leave the work
ready for review or terminally closed through the issue #769 agentic finalizer.

## Before you start

- Run from the branch that contains the implementation.
- Keep generated runtime directories out of the commit scope.
- Preserve Node memory settings when validation includes Node tooling:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## 1. Inspect the Worktree

```bash
git --no-pager status --short
git --no-pager diff --name-only
git --no-pager diff --cached --name-only
```

Separate issue-relevant changes from generated artifacts. Issue-relevant files
can include recipes, helper tools, tests, source, docs, discoverability links,
and `.gitignore` changes that directly support the issue.

Generated runtime paths such as `.claude/runtime/` are never issue-relevant
commit scope.

---

## 2. Remove Generated Runtime Artifacts

If `.claude/runtime/` or another generated artifact is staged, unstage it:

```bash
git restore --staged .claude/runtime 2>/dev/null || true
```

If a runtime log is needed for debugging, extract only the minimum redacted
snippet outside the repository. Do not copy the whole runtime directory:

```bash
mkdir -p /tmp/amplihack-finalization-evidence
grep -R "workflow-finalize" .claude/runtime 2>/dev/null \
  | sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' \
  > /tmp/amplihack-finalization-evidence/workflow-finalize-redacted.log || true
```

Remove generated runtime leftovers from the worktree:

```bash
rm -rf .claude/runtime
```

Do not add `.claude/runtime/` to an allowlist. Runtime state is generated output,
not source material.

---

## 3. Run Artifact Guard

Run a full safety scan before staging:

```bash
amplihack hygiene artifact-guard --repo . --mode all
```

A clean result exits `0`. A blocked result names the unsafe paths:

```text
Artifact Guard blocked 1 prohibited artifact paths.

source           path                  rule
untracked        .claude/runtime/      claude-runtime
```

Fix the paths and rerun the guard. Do not weaken Artifact Guard to make a branch
publishable.

---

## 4. Run Workflow Finalization

Run `workflow-finalize` with the existing branch and PR context:

```bash
result_file="$(mktemp -t workflow-finalize-XXXXXX.json)"

amplihack recipe run workflow-finalize \
  -c repo_path="$PWD" \
  -c branch_name="$(git branch --show-current)" \
  -c issue_number=769 \
  -c pr_number=123 \
  -c pr_url="https://github.com/rysweet/amplihack-rs/pull/123" \
  --format json > "$result_file"
```

Inspect terminal-state and finalizer evidence:

```bash
jq '.. | objects | select(has("terminal_state") or has("decision")) | {
  decision,
  terminal_success,
  terminal_state,
  terminal_reason,
  publish_status,
  ready_for_review,
  artifact_scope
}' "$result_file"
```

Expected target outcomes:

| Outcome | Meaning |
| --- | --- |
| `decision="ready"` with `terminal_state="FOLLOWUP_CREATED"` | The branch has reviewable PR work. This is PR-ready, not merged/final terminal success. |
| `decision="finalized"` with `NO_DIFF_SUCCESS`, `MERGED`, or `CLOSED_OBSOLETE` | No further commit or publish work is required. |
| `decision="blocked"` | Fix the named problem and rerun finalization. |

---

## 5. Run Local Validation

Run pre-commit and the tests for the changed area:

```bash
pre-commit run --all-files

cargo test --test workflow_finalize_terminal_state
cargo test --test workflow_finalize_resilience
cargo test --test default_workflow_decomposition
```

Also run the `workflow_agentic_finalization` helper/schema tests when the
change touches finalization evidence or artifact scope.

---

## 6. Stage Only Issue-Relevant Files

Stage explicit files. Do not rely on `git add -A` until Artifact Guard is clean
and status has been reviewed.

Example for an implementation that touches recipes, helper tools, tests, and
docs:

```bash
git add \
  amplifier-bundle/recipes/workflow-finalize.yaml \
  amplifier-bundle/recipes/workflow-terminal-state.yaml \
  amplifier-bundle/tools/workflow_agentic_finalization.sh \
  tests/integration/workflow_finalize_terminal_state.rs \
  tests/integration/workflow_finalize_resilience.rs \
  tests/integration/workflow_agentic_finalization.rs \
  docs/reference/workflow-agentic-finalization.md \
  docs/howto/finalize-existing-workflow-branch.md \
  docs/tutorials/workflow-agentic-finalization.md \
  docs/index.md \
  docs/recipes/README.md \
  docs/tutorials/README.md
```

Adjust the list to match the actual issue-relevant diff. Include `.gitignore`
only when the issue intentionally changes ignore policy.

Review the staged set:

```bash
git --no-pager diff --cached --name-status
git --no-pager diff --cached --stat
```

If `.claude/runtime/`, dependency trees, build output, logs, or local config
appear in the staged diff, unstage and remove them before committing.

---

## 7. Commit and Push

Use a focused commit message that references the issue:

```bash
git commit -m "Fix workflow finalization evidence handling for issue #769"
git push -u origin "$(git branch --show-current)"
```

The pre-commit hook runs Artifact Guard again. A hook failure is a real
finalization failure; fix the reported paths and rerun the commit.

---

## 8. Create or Update the Pull Request

Create a PR if one does not exist:

```bash
gh pr create \
  --base main \
  --head "$(git branch --show-current)" \
  --title "Fix workflow finalization evidence handling" \
  --body "$(cat <<'EOF'
Refs #769

## Summary
- Adds bounded agentic finalization evidence validation.
- Preserves deterministic terminal-state and final-status gates.
- Keeps generated .claude/runtime artifacts out of commit scope.

## Validation
- pre-commit run --all-files
- cargo test --test workflow_finalize_terminal_state
- cargo test --test workflow_finalize_resilience
- cargo test --test workflow_agentic_finalization
EOF
)"
```

Update an existing PR:

```bash
gh pr edit 123 \
  --body "$(cat <<'EOF'
Refs #769

## Summary
- Adds bounded agentic finalization evidence validation.
- Preserves deterministic terminal-state and final-status gates.
- Keeps generated .claude/runtime artifacts out of commit scope.

## Validation
- pre-commit run --all-files
- cargo test --test workflow_finalize_terminal_state
- cargo test --test workflow_finalize_resilience
- cargo test --test workflow_agentic_finalization
EOF
)"
```

The PR is ready for review when Artifact Guard is clean, validation has passed,
the staged scope contains only issue-relevant files, and finalization reports
`ready` with `ready_for_review=true`.

The workflow is terminally finalized only when finalization reports
`finalized` with `NO_DIFF_SUCCESS`, `MERGED`, or `CLOSED_OBSOLETE`.

---

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Artifact Guard blocks `.claude/runtime/` | Agent runtime output leaked into the parent worktree | Unstage it, remove it from the worktree, rerun Artifact Guard. |
| `FAILED_INVALID_EVIDENCE` | Finalizer output is malformed, contradictory, or missing required fields | Fix the producing recipe/helper logic and rerun `workflow-finalize`. |
| `FAILED_DIRTY_WORKTREE` | Meaningful uncommitted work remains after finalization | Stage and commit issue-relevant files, or remove unrelated leftovers. |
| `BLOCKED_CI` | Required checks or PR readiness failed | Diagnose the failing check, fix only issue-related failures, rerun validation. |
| PR lookup fails | Missing or mismatched `pr_number`, `pr_url`, branch, or GitHub auth | Pass the correct PR context and verify `gh auth status`. |
| `decision="ready"` but `terminal_success=false` | The PR is ready for review but not merged or otherwise terminal | Continue normal review/merge flow; do not treat PR readiness as merged closure. |

## See Also

- [Workflow Agentic Finalization Reference](../reference/workflow-agentic-finalization.md)
- [Tutorial: Workflow Agentic Finalization](../tutorials/workflow-agentic-finalization.md)
- [Artifact Guard](../artifact-guard.md)
- [How to Diagnose Workflow Terminal-State Failures](./diagnose-workflow-terminal-state.md)
