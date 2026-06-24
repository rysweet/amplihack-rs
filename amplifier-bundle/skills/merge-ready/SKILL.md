---
name: merge-ready
description: Checks whether a PR/pull request satisfies the project's merge criteria and records the required evidence in the PR/pull request description. Use with `/merge-ready` before review or merge when QA-team scenarios, docs links, quality-audit convergence, checks/build validation status, and diff scope must be verified.
disable-model-invocation: true
argument-hint: "[pr-number]"
---

# Merge Ready

Use this skill as the final gate before calling a PR/pull request ready for review, merge, or closure.

This skill coordinates existing project workflows. It does **not** replace:

- `qa-team` for outside-in scenario authoring and execution
- `quality-audit` for iterative SEEK -> VALIDATE -> FIX review
- `default-workflow` for substantive fixes discovered while making the PR/pull request merge-ready

Use [pr-description-template.md](pr-description-template.md) to update the PR/pull request description with the required evidence.

## Platform scope

The top-level merge-readiness contract is platform-neutral. Use platform-specific commands and terminology only inside the matching platform path:

- **GitHub**: GitHub pull requests, GitHub Actions, `gh`, GitHub issues, review conversations, and branch protection.
- **Azure DevOps/AzDO**: Azure DevOps, AzDO, Azure Repos pull requests, Azure Pipelines, `az repos`, `az pipelines`, Azure Boards work items, PR threads, and branch policies.

When the platform cannot be detected, inspect the repository remote, PR URL, or available CLI context first. If the platform still cannot be proven, report `NOT_MERGE_READY` with a platform-access blocker instead of assuming GitHub behavior.

Platform detection signals:

- **GitHub**: `github.com` remotes or PR URLs, `gh repo view`, `gh pr status`, or hosting metadata that identifies a GitHub repository.
- **Azure DevOps/AzDO**: `dev.azure.com` or `*.visualstudio.com` remotes or PR URLs, Azure Repos remote paths, `az repos pr show`, or hosting metadata that identifies an Azure DevOps project/repository.

## Required outcome

A PR/pull request is merge-ready only when **all** of the following are true:

1. `qa-team` scenarios were written or updated, validated with `gadugi-test validate`, and actually run with `gadugi-test run`.
2. User-facing docs were updated when the change affects APIs, configuration, deployment, CLI behavior, or other external surfaces.
3. `quality-audit` completed at least 3 SEEK -> VALIDATE -> FIX cycles, continued past 3 if critical or high findings remained, and ended on a clean final cycle.
4. Checks/build validation for the current head revision passed according to the active platform and repository policy.
5. PR/pull request metadata is complete and reviewable.
6. Required reviews/approvals are satisfied, with no outstanding requested changes.
7. Merge conflicts are absent.
8. Required policies/protection rules are satisfied.
9. Required linked work items/issues are present or explicitly not applicable under project policy.
10. Required comments/threads are resolved or explicitly acknowledged as non-blocking.
11. Final merge/close verification is documented for the intended outcome, and, if a merge or close action was performed, the platform state confirms it.
12. The PR/pull request description contains concrete evidence for all other criteria, including scope.
13. The diff contains no unrelated changes.

If any criterion is missing, stale, inaccessible, or blocked by external approval, check, or policy state, the PR/pull request is **not merge-ready**.

## Non-negotiable guardrails

- Do **not** treat "scenario YAML exists" as sufficient. The scenarios must be validated **and** run.
- Do **not** claim docs are irrelevant without checking the changed surfaces.
- Do **not** accept fewer than 3 quality-audit cycles.
- Do **not** accept a quality-audit result unless the final cycle is clean.
- Do **not** accept pending, failing, cancelled, stale, or unavailable checks/build validation.
- Do **not** mark a PR/pull request merge-ready until the description itself is updated with evidence.
- Do **not** bypass policies/protection rules, required checks, required approvals, or required linked work item rules.
- Do **not** silently ignore blockers such as missing environment access, missing PR/pull request access, missing test tooling, missing CLI authentication, or missing platform permissions. Report them explicitly.

## Workflow

### 1. Establish PR/pull request context

Identify the target PR/pull request from `$ARGUMENTS` if provided; otherwise use the current branch's PR/pull request.

Gather:

- platform and repository/project identity
- PR/pull request ID, URL, title, source branch, target branch, and current description
- changed files and current diff scope
- whether the change touches external or user-facing surfaces
- current checks/build validation status for the current head revision
- metadata, reviews/approvals, mergeability, policies/protection, linked work items/issues, and comments/threads

#### GitHub path

Use GitHub-specific commands only when the PR is a GitHub pull request.

```bash
gh pr view "$PR" --json number,url,title,body,headRefName,baseRefName,mergeStateStatus,mergeable,reviewDecision,isDraft,closingIssuesReferences
gh pr diff "$PR" --name-only
gh pr checks "$PR"
git diff --name-only "origin/$(gh pr view "$PR" --json baseRefName --jq .baseRefName)"...HEAD
```

For unresolved review conversations or branch protection details, use `gh pr view`, `gh api`, the GitHub UI, or authorized repository settings evidence. Record the exact source used.

#### Azure DevOps/AzDO path

Use AzDO-specific commands only when the PR is an Azure Repos pull request.

```bash
az repos pr show --id "$PR_ID" --output json
az repos pr policy list --id "$PR_ID" --output table
az repos pr work-item list --id "$PR_ID" --output table
az repos pr reviewer list --id "$PR_ID" --output table
az repos pr show --id "$PR_ID" --query "{source:sourceRefName,target:targetRefName,mergeStatus:mergeStatus,status:status,title:title}" --output json
```

For PR threads or build validation details that are not exposed by the installed CLI version, use the Azure DevOps UI or authorized REST API evidence. Record the exact source used.

If no PR/pull request exists yet, you may still prepare evidence, but the final verdict must remain blocked until the PR/pull request exists and platform checks/build validation status is available.

### 2. Satisfy the QA criterion with `qa-team`

Invoke `qa-team` and ensure it covers the changed behavior from an external user perspective.

Required deliverables:

- scenario file path(s)
- proof that `gadugi-test validate` succeeded
- proof that `gadugi-test run` succeeded
- target environment used for the run (local or deployed)
- evidence location or execution output summary

Minimum commands:

```bash
gadugi-test validate path/to/scenario.yaml
gadugi-test run path/to/scenario.yaml
```

If the environment needed to run the scenario does not exist, stop and report a blocker. Do not downgrade this to "definition only".

### 3. Satisfy the docs criterion

Inspect the changed files and decide whether the PR/pull request changes any user-facing surface:

- APIs
- configuration
- deployment or operations flow
- CLI or TUI behavior
- user-visible workflows

If yes, update the corresponding docs and add links to those docs in the PR/pull request description.

If no, list the changed surfaces you checked and explain why each is internal-only. For example: "Changed surfaces reviewed: `rust_runner.py`, `smart-orchestrator.yaml` -- both are internal orchestration plumbing with no CLI/API/config impact."

### 4. Satisfy the quality-audit criterion

Invoke `quality-audit` and require the full iterative loop:

- minimum 3 cycles
- each cycle is SEEK -> VALIDATE (multi-agent consensus) -> FIX
- continue past 3 cycles if critical or high findings remain
- final cycle must be clean: zero critical or high findings, and zero medium findings that pose correctness or security risks

Every fix uncovered during this audit must follow `default-workflow`. If the audit finds issues that require code or docs changes, switch to `default-workflow` for those fixes, complete the work, then return to this skill and resume verification.

Capture in the PR/pull request description:

- cycle count
- confirmed findings per cycle
- fixes applied
- convergence summary
- explicit statement that the final cycle was clean (zero critical/high findings, zero medium correctness/security findings)

### 5. Verify platform merge-readiness aspects

Collect explicit evidence for every aspect below before declaring `MERGE_READY`.

| Aspect | GitHub path | Azure DevOps/AzDO path |
| --- | --- | --- |
| Checks/build validation | Use GitHub Actions and other status checks for the current head. `gh pr checks "$PR"` must show required checks green; skipped conditional checks are allowed only when named with the skip reason. Rerun only clearly flaky jobs and fix real failures. | Use Azure Pipelines/build validation and branch policy status for the current source commit. `az repos pr policy list --id "$PR_ID"` and pipeline run evidence must show required validations approved/succeeded; optional or skipped validations must be named with the policy reason. |
| PR/pull request metadata | Use `gh pr view` or the GitHub UI to confirm title, description, base branch, head branch, draft state, labels/milestone if required, and reviewer visibility. A draft pull request is not merge-ready unless project policy explicitly allows draft readiness. | Use `az repos pr show` or the AzDO UI to confirm title, description, source branch, target branch, draft state if supported, required labels/tags if used by the project, and reviewer visibility. A draft Azure Repos PR is not merge-ready unless project policy explicitly allows draft readiness. |
| Reviews/approvals | Use `gh pr view --json reviewDecision,reviews,reviewRequests` or the GitHub UI. Required approving reviews must be present, requested changes must be cleared, and stale approvals must be revalidated after source changes when protection requires it. | Use `az repos pr reviewer list --id "$PR_ID"` and branch policy status. Required reviewers or reviewer groups must have approved or not voted blocking/rejected; stale votes must be revalidated after source changes when policy requires it. |
| Merge conflicts | Use `gh pr view --json mergeable,mergeStateStatus` or the GitHub UI. `CONFLICTING`, dirty, unknown, or stale mergeability is blocking until refreshed and clean. | Use `az repos pr show --id "$PR_ID" --query mergeStatus` or the AzDO UI. Conflict, failure, rejected, or unknown merge status is blocking until refreshed and clean. |
| Policies/protection | Use branch protection/ruleset evidence from `gh pr view`, `gh api`, or the GitHub UI. Required checks, required reviews, signed commit rules, linear history, merge queue, and required conversation resolution must all be satisfied. | Use branch policy evidence from `az repos pr policy list --id "$PR_ID"`, `az repos policy`, or the AzDO UI. Build validation, minimum reviewers, required reviewers, comment resolution, linked work item, status checks, and merge strategy policies must all be satisfied. |
| Linked work items/issues | Use `gh pr view --json closingIssuesReferences` or the GitHub UI. Required issue links or closing keywords must point to the correct issue(s), or the PR description must state why no issue is required under project policy. | Use `az repos pr work-item list --id "$PR_ID"` or the AzDO UI. Required Azure Boards work items must be linked to the PR, or the PR description must state why no work item is required under project policy. |
| Comments/threads | Use `gh pr view --json comments,reviews` plus review conversation evidence from `gh api` or the GitHub UI. Required review conversations must be resolved; unresolved comments must be explicitly marked non-blocking by an authorized reviewer or project policy. | Use PR thread evidence from the AzDO UI or authorized REST API. Required threads must be resolved; active comments must be explicitly marked non-blocking by an authorized reviewer or project policy. |
| Final merge/close verification | If authorized to merge, use `gh pr merge` according to project policy, then verify `gh pr view "$PR" --json state,mergedAt,mergedBy,mergeCommit`. If closing without merge, verify `state:CLOSED` and document the close reason. If not authorized to perform the final action, document the exact external approval or permission blocker. | If authorized to complete the PR, use the platform-approved completion command, UI, or automation according to project policy, then verify `az repos pr show --id "$PR_ID" --query "{status:status,closedBy:closedBy,closedDate:closedDate,lastMergeCommit:lastMergeCommit}"`. `az repos pr update --id "$PR_ID" --status completed` is an example only, not the default path. If abandoning/closing without merge, verify `status:abandoned` and document the reason. If not authorized to perform the final action, document the exact external approval or permission blocker. |

### 6. Enforce scope

Review the diff and confirm the PR/pull request is focused on the intended change. If unrelated changes are present, report them as blockers and do not mark the PR/pull request ready.

Platform-neutral evidence is enough for scope when it names the source and target branches and lists changed files. Use the platform diff, local Git diff, or both.

### 7. Update the PR/pull request description

**This step must not be performed until steps 2-6 have all completed and evidence has been collected.** Do not update the description with placeholder or anticipated evidence.

Use [pr-description-template.md](pr-description-template.md) and replace every placeholder with actual evidence. Keep the wording concise, but make the evidence concrete enough that a reviewer can verify the work without guessing.

If the description already has useful sections, merge the evidence into the existing structure instead of duplicating headings.

#### GitHub path

```bash
gh pr edit "$PR" --body-file pr-description.md
```

#### Azure DevOps/AzDO path

```bash
az repos pr update --id "$PR_ID" --description "$(cat pr-description.md)"
```

If the platform CLI cannot update the description safely, update it through the authorized UI/API and record that evidence source.

### 8. Return a binary verdict

End with one of these outcomes:

- `MERGE_READY`: every criterion passed, required evidence was added to the PR/pull request description, and no platform blocker remains
- `NOT_MERGE_READY`: one or more blockers remain

Use this format:

```markdown
Verdict: MERGE_READY | NOT_MERGE_READY

Platform: GitHub | Azure DevOps/AzDO | unknown

Criteria:

- QA-team: pass | fail
- Docs: pass | fail | not-applicable
- Quality-audit: pass | fail
- Checks/build validation: pass | fail
- Metadata: pass | fail
- Reviews/approvals: pass | fail
- Merge conflicts: pass | fail
- Policies/protection: pass | fail
- Linked work items/issues: pass | fail | not-applicable
- Comments/threads: pass | fail | not-applicable
- Final merge/close verification: pass | fail | not-performed
- Scope: pass | fail
- Description evidence: pass | fail

Blockers:

- <concrete missing item or `none`>
```

## When to stop and hand back a blocker

Stop and report `NOT_MERGE_READY` when any of these are true:

- no runnable environment exists for the required gadugi scenario
- the PR/pull request or checks/build validation status cannot be accessed
- platform access or authentication cannot prove required metadata, reviews/approvals, mergeability, policies/protection, linked work items/issues, comments/threads, or final state
- quality-audit has not converged to a clean final cycle
- docs changes are needed but not yet written
- checks/build validation is pending, failing, cancelled, stale, skipped without policy support, or unavailable
- required reviews/approvals, linked work items/issues, comments/threads, or policies/protection remain unsatisfied
- merge conflicts are present or mergeability cannot be determined
- final merge/close action is blocked by permissions, external approval, required waiting period, or platform policy
- the diff contains unrelated changes

Do not substitute promises or TODOs for evidence.
