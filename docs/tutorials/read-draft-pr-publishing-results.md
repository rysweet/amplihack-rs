---
title: "Read a draft PR publishing result"
description: "Use the Step 16 JSON fields to recognize a successful draft and find the cause of a terminal rejection."
doc_type: tutorial
last_updated: 2026-11-20
review_schedule: as-needed
owner: amplihack
---

# Read a draft PR publishing result

Use `jq` to inspect two examples based on the
[publishing regression fixtures](../../tests/integration/issue_2306_publish/).
The commands read sample output locally.

## Confirm a recovered draft

This example uses the issue #2306 fixture. Title derivation fails and the
issue lookup returns an unusable title, so the request uses
`Update feat/issue-2306-publish (#2306)`. On success, the runner parses the
publisher's JSON into `context.pr_publish_result`. Read the outcome and PR
identity from that object.

```sh
jq -r '.context.pr_publish_result | [.state, .pr_url, .pr_number] | @tsv' <<'JSON'
{
  "success": true,
  "context": {
    "pr_publish_result": {
      "state": "FOLLOWUP_CREATED",
      "terminal_status": "success",
      "pr_url": "https://github.com/owner/repo/pull/99",
      "pr_number": "99"
    }
  }
}
JSON
```

Expected output

```text
FOLLOWUP_CREATED	https://github.com/owner/repo/pull/99	99
```

`FOLLOWUP_CREATED` confirms that the step created the draft. The URL and
number identify the PR.

## Read a terminal rejection

The failure fixture emits a long transient warning before GitHub rejects the
final attempt. Read the failed publishing step's `error` field
(`StepResult.error`). This excerpt includes the runner's wrapper and final
publisher error. The ellipsis stands for earlier stderr lines.

```sh
jq -r '.step_results[] | select(.step_id == "step-16-create-draft-pr" and .status == "failed") | .error' <<'JSON'
{
  "success": false,
  "step_results": [
    {
      "step_id": "step-16-create-draft-pr",
      "status": "failed",
      "error": "bash step failed: Command failed (exit 23): ...\nERROR: FAILED_PR_CREATE (exit 23): gh pr create failed (exit 23): distinctive final creation rejection REDACTED_TOKEN"
    }
  ]
}
JSON
```

Expected output

```text
bash step failed: Command failed (exit 23): ...
ERROR: FAILED_PR_CREATE (exit 23): gh pr create failed (exit 23): distinctive final creation rejection REDACTED_TOKEN
```

For a saved runner report, pass the report file to either `jq` filter in
place of the sample input. In the failure example, `gh pr create` is the
failing operation and `23` is its exit status. `REDACTED_TOKEN` marks a
credential removed from the provider message.

See [Draft PR metadata recovery and diagnostics](../bugfixes/bugfix-2306-publish-metadata-recovery-diagnostics.md)
for the outcome fields and diagnostic contract.
