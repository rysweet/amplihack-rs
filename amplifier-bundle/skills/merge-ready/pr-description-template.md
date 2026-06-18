# PR/pull request description evidence template

Copy these sections into the PR/pull request description and replace every placeholder with actual evidence. Use platform-neutral wording for shared evidence, then include the GitHub or Azure DevOps/AzDO details that apply.

## Merge readiness

- Platform: `<GitHub / Azure DevOps/AzDO>`
- PR/pull request: `<number or ID and URL>`
- Source -> target: `<source branch> -> <target branch>`
- Current head revision: `<commit SHA>`

### QA-team evidence

- Scenario files: `<path/to/scenario.yaml>`
- Validation command: `gadugi-test validate <scenario>`
- Validation result: `<passed / failed>`
- Run command: `gadugi-test run <scenario>`
- Run target: `<local env / deployed env>`
- Run result: `<passed / failed>`
- Evidence location: `<artifact path or log link>` (must include pass/fail counts and command output, not just a claim)

### Documentation

- User-facing docs impact: `<yes / no>`
- Updated docs: `<doc path or link, or "none">`
- Description links added: `<list of links or "n/a">`
- Rationale if not applicable: `<list of changed surfaces checked and why each is internal-only>`

### Quality-audit

- Cycle 1 summary: `<findings, validation result, fixes>`
- Cycle 2 summary: `<findings, validation result, fixes>`
- Cycle 3 summary: `<findings, validation result, fixes>`
- Additional cycles: `<none or summary>`
- Final clean cycle: `<cycle number>` (zero critical/high, zero medium correctness/security findings)
- Fixes followed default-workflow: `<yes / no>`
- Convergence summary: `<why the audit is considered complete>`

### Checks/build validation

- Evidence source: `<platform CLI command, UI link, API response, or artifact>`
- Current head validated: `<commit SHA>`
- Required checks/build validations: `<all passed / failures remain / pending / unavailable>`
- Optional or skipped validations: `<list of names and policy-supported skip reason, or "none">`
- Reruns performed: `<none or list>`
- Real failures fixed: `<none or summary>`

GitHub example:

```text
Evidence source: gh pr checks 123
Required checks/build validations: all passed in GitHub Actions for abc1234
Optional or skipped validations: docs-preview skipped by path filter
```

Azure DevOps/AzDO example:

```text
Evidence source: az repos pr policy list --id 123 and Azure Pipelines run 456
Required checks/build validations: build validation succeeded for abc1234
Optional or skipped validations: deployment validation not required for docs-only path
```

### PR/pull request metadata

- Title: `<reviewable title>`
- Description complete: `<yes / no>`
- Source branch: `<branch>`
- Target branch: `<branch>`
- Draft state: `<not draft / draft / not supported>`
- Required labels/tags/milestone: `<satisfied / not applicable / missing details>`
- Metadata evidence source: `<platform CLI command, UI link, or API response>`

GitHub example: `gh pr view <pr> --json title,body,headRefName,baseRefName,isDraft,labels,milestone`

Azure DevOps/AzDO example: `az repos pr show --id <id> --output json`

### Reviews/approvals

- Required approvals: `<satisfied / missing / not applicable>`
- Requested changes or blocking votes: `<none / list>`
- Stale approval policy checked: `<yes / no / not applicable>`
- Review evidence source: `<platform CLI command, UI link, or API response>`

GitHub example: `gh pr view <pr> --json reviewDecision,reviews,reviewRequests`

Azure DevOps/AzDO example: `az repos pr reviewer list --id <id>`

### Merge conflicts

- Mergeability status: `<clean / conflicts / unknown>`
- Conflict evidence source: `<platform CLI command, UI link, or API response>`
- Conflict resolution required: `<none / summary>`

GitHub example: `gh pr view <pr> --json mergeable,mergeStateStatus`

Azure DevOps/AzDO example: `az repos pr show --id <id> --query mergeStatus`

### Policies/protection

- Required policies/protection: `<satisfied / missing / blocked / not applicable>`
- Required checks/build policy: `<satisfied / blocked / not applicable>`
- Required review policy: `<satisfied / blocked / not applicable>`
- Required conversation/thread policy: `<satisfied / blocked / not applicable>`
- Policy evidence source: `<platform CLI command, UI link, or API response>`

GitHub example: branch protection or ruleset evidence from `gh api`, `gh pr view`, or the GitHub UI.

Azure DevOps/AzDO example: `az repos pr policy list --id <id>`

### Linked work items/issues

- Required linked work items/issues: `<satisfied / not applicable / missing>`
- Linked items: `<issue numbers, work item IDs, or "none">`
- Rationale if not applicable: `<project policy reason>`
- Link evidence source: `<platform CLI command, UI link, or API response>`

GitHub example:

```text
Linked issues: Fixes #123
Evidence source: gh pr view 456 --json closingIssuesReferences
```

Azure DevOps/AzDO example:

```text
Linked work items: AB#12345
Evidence source: az repos pr work-item list --id 456
```

### Comments/threads

- Required comments/threads resolved: `<yes / no / not applicable>`
- Unresolved but non-blocking comments/threads: `<none or list with policy/reviewer reason>`
- Comment/thread evidence source: `<platform CLI command, UI link, or API response>`

GitHub example: review conversation evidence from `gh api`, `gh pr view`, or the GitHub UI.

Azure DevOps/AzDO example: PR thread evidence from the Azure DevOps UI or authorized REST API.

### Scope

- Changed files reviewed: `<summary or command used>`
- Unrelated changes: `<none or list>`

### Final merge/close verification

- Intended final action: `<merge / close without merge / abandon / readiness only>`
- Authorized actor: `<name, team, automation, or "not authorized">`
- Final action performed: `<yes / no>`
- Final platform state: `<merged / closed / completed / abandoned / open / not performed>`
- Verification evidence source: `<platform CLI command, UI link, API response, or "not performed because readiness only">`
- Remaining external blockers: `<none or list>`

GitHub example:

```text
Final platform state: merged
Verification evidence source: gh pr view 456 --json state,mergedAt,mergedBy,mergeCommit
```

Azure DevOps/AzDO example:

```text
Final platform state: completed
Verification evidence source: az repos pr show --id 456 --query "{status:status,closedBy:closedBy,closedDate:closedDate,lastMergeCommit:lastMergeCommit}"
```

### Verdict

- Verdict: `<MERGE_READY / NOT_MERGE_READY>`
- Remaining blockers: `<none or list>`
