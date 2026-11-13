---
title: Keep workflow issues open after merging
description: Non-closing issue references through workflow recipe context.
doc_type: howto
---

# Keep workflow issues open after merging

Set `pr_issue_link=refs` when one PR contributes to an issue
that needs further work or whose owner closes it after review. The workflow
uses `Refs` in both the publication commit and the PR body.

## Supply the policy at entry

Start in the target repository with `amplihack` installed, an available agent
runtime and authenticated platform access. Normal hooks and repository checks
must be available.

For work on issue #2550 in `rysweet/amplihack-rs`, pass the issue explicitly;
the policy itself does not select an issue:

```bash
amplihack recipe run default-workflow \
  -c repo_path=. \
  -c issue_number=2550 \
  -c task_description="Implement the issue-link policy described in issue #2550" \
  -c pr_issue_link=refs \
  -c no_merge=true
```

The publication commit and PR body use `Refs #2550`, even
when the changes are substantive. The PR stays open because this invocation
also sets `no_merge=true`. The two options are independent: `pr_issue_link`
controls the issue keyword; `no_merge` controls permission to merge the PR.

For the same build through auto-drive, supply the policy at the outer entry:

```bash
amplihack recipe run auto-drive-to-merge \
  -c repo_path=. \
  -c task_description="Implement rysweet/amplihack-rs issue #2550" \
  -c pr_issue_link=refs \
  -c no_merge=true
```

The build forwards `refs` through `autodrive-build` and
`default-workflow` to publication. Review and merge-ready description updates
retain the published reference. When a subsequent authorised run squash
merges the PR, its message body retains `Refs #2550`.

Use one entry path for a task; the commands above are alternatives.

## Choose automatic or closing references

Omit the key or pass `-c pr_issue_link=auto` for classification-based behaviour:
substantive changes get `Closes`, while planning-only changes get `Refs`.

Pass `-c pr_issue_link=closes` to request closure subject to the same planning
rule. It still produces `Refs` for planning-only, generated-only, empty or
uncertain classification. It does not force an issue closed.

Use only `auto`, `refs` or `closes`. Empty values, whitespace and variants
such as `Refs` are rejected before publication side effects. Configure recipe
context with `-c`; exporting a bare `PR_ISSUE_LINK` does not configure the run.

## Recover and inspect the published reference

Durable recovery retains the saved policy. A record without the key uses
`auto`; an invalid restored value stops the run. When starting a new recovery
invocation, include `-c pr_issue_link=refs` alongside the PR and branch context
in the [recovery guide](recover-existing-pr-with-default-workflow.md).

Changing the policy on an existing PR does not rewrite its published keyword.
Finalise and merge-ready preserve that reference even if later changes are
substantive. Inspect the live PR body if a preservation operation reports a
readback failure: the update may already have occurred. Resolve the reported
problem before starting another edit.

For the full matrix, internal operation contracts and supported reference
forms, see the [issue-link policy reference](../reference/workflow-issue-link-policy.md).
