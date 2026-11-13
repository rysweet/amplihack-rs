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

1. QA was judged and recorded as [criterion-1-qa-judgement.md](criterion-1-qa-judgement.md) describes. A PR that changes user-facing behaviour has outside-in scenarios for the affected journeys, written or extended in the PR where missing, and they were run and pass. A PR that changes nothing user-facing (documentation, configuration with no behaviour change) has a recorded reason citing its files. Only a recorded hand-off, naming its reason, owner and what they need, stops at this criterion. Every other criterion still applies.
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
- Do **not** waive criterion 1 for anything but a change that is documentation only. One changed file that is not documentation — a build, workflow, configuration or agent-instruction file included — costs the whole PR/pull request the exemption, and an unreadable diff is not a documentation-only change.
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

### 2. Judge the QA criterion

Follow [criterion-1-qa-judgement.md](criterion-1-qa-judgement.md). First
judge whether this PR/pull request changes user-facing behaviour.

- If it does not (documentation, or configuration with no behaviour change),
  record the reason and cite the changed files it rests on. Criterion 1 is
  then **verified** with that evidence, not "not applicable". A build,
  workflow, configuration or agent-instruction file that changes behaviour is
  user-facing, however it is formatted.
- If it does, invoke `qa-team` for the affected user journeys. Where a
  journey has no outside-in scenario, or the scenario does not cover the
  change, write or extend one in the PR. Run the scenarios, preferably in CI
  or an isolated environment; when one fails, classify the cause, fix it,
  commit and run again until it passes. Never weaken a failing scenario.

Required deliverables:

- the judgement (`user_facing` or `not_user_facing`) and its reason
- the cited files, for a `not_user_facing` judgement
- scenario file path(s), each with whether it was written in this PR and its result
- the command used to run each scenario and the target environment (CI, isolated or deployed)
- the attempts made, with the cause and fix commit of each failure
- evidence location or execution output summary

The repository's declared QA command (for example from `.amplihack/qa.toml`)
is evidence for the judgement, not a verdict: a passing suite alone never
satisfies a user-facing PR.

If a scenario cannot be run because only a person can clear the cause (an
unreachable service, missing access, a cause outside the PR), record a
hand-off naming the reason, the owner and what they need. A missing scenario
is never a reason to stop: write it.

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

Keep the change's description and the readiness report apart. The report goes **below** the description, starting with the template's marker line (`<!-- merge-ready-report: … -->`), which stays exactly as written; replace an earlier report below the marker rather than adding a second one above it. Auto-drive's finalize step cuts the squash commit body at the first line that starts with `<!-- merge-ready-report` or `## Merge readiness`, so the text above becomes the commit message on the base branch and the report never does (#2056). Nothing refreshes the report after the last merge-ready round, so readiness state written above the marker would land as history: a head SHA, a verdict, `open` as the platform state, or a round-by-round log. Keep it below.

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

## Running under auto-drive

The `auto-drive-to-merge` workflow applies these criteria from an agent step in its merge round. It does not invoke this skill. `disable-model-invocation: true` stays set, so only a person starts `/merge-ready`.

Auto-drive reads this skill and its description template from the runner-pinned launch bundle. It never falls back to instructions in the PR worktree. Platform command examples are suggestions; agents may recover using equivalent complete observations.


Every criterion in [Required outcome](#required-outcome) applies as written, except that auto-drive measures criteria 1, 3 and 6 as follows:

| Criterion | What auto-drive requires |
| --- | --- |
| 1. QA judgement | An agent step judges criterion 1 by [criterion-1-qa-judgement.md](criterion-1-qa-judgement.md) and records the outcome with `orch helper autodrive qa-record`; nothing else writes `qa-evidence.json`. `autodrive_qa_evidence.sh` only measures: it runs the declared QA command from the first source present (`AUTODRIVE_QA_COMMAND`/`AUTODRIVE_QA_COMMANDS`, then `.amplihack/qa.toml` at the merge base, then one clean merge-base CI test step, then merge-base manifest detection) and writes the changed files, `docs_only`, repository type, scenario directory and existing scenarios to `qa-inputs.json`. The declared command, an unresolved scope or an invalid `qa.toml` are inputs to the judgement, not verdicts. `qa_status` is `PASS`, `FAIL` (fix, commit and record again) or `HANDOFF` (blocker `qa-handoff: <reason> (owner: <owner>, needs: <needs>)`, owned by a person). Criterion 1 is met only by `PASS` in a consistent record: cited files appear in the diff for `not_user_facing`, scenario results are listed for `user_facing`, and the record has `qa_judgement` and no `gadugi_*` fields. See [Auto-drive QA](../../../docs/reference/auto-drive-qa.md). |
| 3. Quality-audit | The `crusty-old-engineer` loop of the same auto-drive run must have ended DONE, and its last round record, as listed in the loop's manifest `crusty-records.tsv`, must have verdict `CLEAN` or accepted `MINOR_ONLY` with all follow-up obligations satisfied, and a `reviewed_head_sha`. The crusty loop stops at its first clean round, so no minimum round count applies. Every commit from `reviewed_head_sha` to the head being merged that is not already on the base branch must be a clean base merge or change only `PR_DESCRIPTION.md`, `.github/pull_request_template.md` or files under `.autodrive/evidence/`. Any other commit, scenario files included, is reported as blocker `crusty-review-required:<sha>`, and before the next merge round the merge-ready loop runs the crusty loop again on the current head, from its own shell, at the depth phase 2's crusty loop runs at. A range that cannot be read, for example in a shallow clone, is reported as blocker `crusty-range-unreadable`; crusty is not re-run for it, and a person must fix the clone. |
| 6. Reviews/approvals | The platform-observation agent measures `approval_status`; `autodrive_platform_facts.sh` is an optional example. GitHub's `reviewDecision` decides it when it is set: `APPROVED` is met, `CHANGES_REQUESTED` and `REVIEW_REQUIRED` are not. An empty `reviewDecision` does not mean that no review is required. GitHub publishes it only when a rule requires at least one approving review, so at a required count of `0` it stays empty even while a code-owner, last-push or required-reviewer rule keeps the merge blocked. The step therefore reads the required approval count, the highest of branch protection's `required_approving_review_count` and the `pull_request` rules in the branch's rulesets: `0` is met only when no such rule applies. A `0` beside `require_code_owner_reviews` or `require_last_push_approval` (classic protection), or beside `require_code_owner_review`, `require_last_push_approval` or a non-empty `required_reviewers` (rulesets), is an unknown count, never `0`. A token without admin rights gets 404 from the branch-protection endpoint, and a 404 is never read as zero. When the count cannot be read or is unknown, `mergeStateStatus` `CLEAN`, `HAS_HOOKS` or `UNSTABLE` is met, because GitHub would block the merge if a required review were missing; `required_approvals` is then left empty, never recorded as `0`, and `approval_source` is `merge-state`. When the count cannot be read and `mergeStateStatus` is `DIRTY` or `BEHIND`, or `BLOCKED` while a check in the rollup has not passed, the branch conflicts or a review thread, outdated or not, is unresolved, GitHub's one state cannot say whether a review is also missing: `PENDING` (`approval_source` `other-blockers`), a blocker the merge round clears by fixing the other blockers first. Anything else is `UNREADABLE`, a blocker. |

The merge round and finalization require concrete current evidence for every criterion. The Rust record validator rejects missing, stale, contradictory or mismatched observations. Agents retain the semantic responsibility for applying these interpretations and recovering equivalent observations; a failed command never waives a criterion.

Private snapshots, current invocation bindings and artifact hashes detect stale or altered evidence. They are workflow safeguards, not an OS security boundary against the same credential holder. Required checks, protection and expected-head merge requests remain necessary.

**Running `/merge-ready` by hand.** Criterion 1 is judged the same way, by [criterion-1-qa-judgement.md](criterion-1-qa-judgement.md), and the judgement is reported in the merge-ready report. `qa-record` is not required by hand; [Record a criterion-1 QA judgement by hand](../../../docs/howto/record-criterion-1-qa-judgement.md) shows how to keep a record file. Criterion 3 still needs a separate `quality-audit` of at least 3 SEEK -> VALIDATE -> FIX cycles ending on a clean cycle. The crusty-loop substitution for criterion 3 applies only inside auto-drive.

See [Auto Drive To Merge Reference](../../../docs/reference/auto-drive-to-merge.md#how-the-merge-round-applies-merge-ready) for the evidence fields, structured outcomes and validity checks.

## When to stop and hand back a blocker

Stop and report `NOT_MERGE_READY` when any of these are true:

- criterion 1's judgement ended in a recorded hand-off (reason, owner and needs)
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
