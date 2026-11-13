---
title: Run Operations and mixed requests
description: Route administrative tasks, repository edits and explicitly tracked work.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: howto
---

# Run Operations and mixed requests

> **[IMPLEMENTED - Validation Pending]** These prompts and commands describe
> the intended issue #2553 behaviour. Reconciliation and result binding require
> implementation and validation before use.

Use this guide when a request includes issue management, research answers,
coordination or repository documentation changes.

## Prerequisites

Use an installed amplihack session in the intended repository. GitHub actions
require authenticated `gh` access to that repository and the permissions needed
for the requested action. Cross-repository requests require trusted repository
contexts for each target.

The examples below are prompts to enter in the agent chat, not shell commands.
Their expected results describe the route and delivered output; issue numbers
and execution UUIDs depend on the run.

## Run an administrative checklist

Ask for an answer with an explicit destination:

```text
For rysweet/amplihack-rs, read issue #2553 and its comments. Answer in chat:
1. Which actions are Operations?
2. Which actions change tracked repository files?
3. What happens when a workstream has no issue number?
```

Expected result: one Operations leaf returns the answers in chat. It creates no
worktree, branch or implementation PR. The numbered checklist does not change
its classification. Reading #2553 as source material does not itself make that
issue the leaf's tracking issue.

For issue triage, specify the requested scope, for example:

```text
Triage issue #2553 in rysweet/amplihack-rs. Report its acceptance criteria and
suggest an owner in chat.
```

Expected result: direct Operations execution returns a triage report. Requests
to create an issue and assign a named, authorised owner use the same route.

## Combine research with a documentation edit

Separate the answer from the file change and state their dependency:

```text
Explain Operations versus Development in chat for rysweet/amplihack-rs.
Then update docs/operations-routing.md using that explanation.
```

Expected result: an Operations leaf produces the explanation; a Development
leaf receives that persisted result and updates the document through the
Development workflow. The original request is not executed again as a second
administrative task.

For wording changes across repositories, name every target repository and file.
Each repository-editing leaf uses Development, even when the change is a single
word. Coordination and status reports remain separate Operations leaves.

## Choose whether work needs tracking

To use existing tracking, bind it to the work explicitly:

```text
Use rysweet/amplihack-rs issue #2553 as tracking for the update to
docs/operations-routing.md. Explain the routing distinction in that document.
```

Expected result: the Development leaf reuses #2553 without creating another
issue.

To request new tracking explicitly:

```text
In rysweet/amplihack-rs, create a tracking issue titled "Clarify Operations
routing examples" with a body requesting clearer administrative and repository
edit examples in docs/operations-routing.md. Then make that documentation
change under the new issue.
```

Expected result: tracking resolution records its attempt, creates one real
issue, persists its repository-scoped number, then starts the Development
leaf. A definite creation failure is reported before the edit starts. An
uncertain outcome requires reconciliation, without an automatic second
creation attempt.

If tracking is unnecessary, omit it. Missing values, `null`, blanks and
recognised placeholders are absent, and execution still proceeds. For complete
input rules, see [optional issue parsing](../reference/dev-orchestrator-routing.md#optional-issue-parsing).

## Resume without repeating administrative actions

Continue with the saved normalised configuration and its execution identities.
Completed leaves are skipped. Sequential execution retains the original leaves
and routes.

When a run reports an uncertain external outcome:

1. Stop the affected run and inspect its saved workstream UUID, repository,
   revision and attempt UUID. Check the declared repository's issue or assignment
   record against the saved request. Preserve evidence of what actually happened.
2. Write a resolution file using the [reconciliation JSON contract](../reference/workstream-state.md#reconciliation-and-safe-resume).
   Record `succeeded` only with verified matching output. Use `not_applied` and
   `retry_authorized: true` only with proof that the action had no effect and an
   explicit decision to retry. If evidence is inconclusive, record `unresolved`.
3. Submit the resolution through the planned supported command. For a saved
   `workstreams.json` and a resolution saved as `resolution.json`:

   ```bash
   amplihack multitask reconcile workstreams.json --resolution-file resolution.json
   ```

   Expected result: a receipt names the same workstream, repository, resolution
   UUID, new revision and lifecycle. The command records state without launching
   work. A stale revision, active child, invalid evidence or persistence failure
   stops it without granting permission to replay.
4. If resolved, resume the same configuration and original mode/options:

   ```bash
   amplihack multitask run workstreams.json
   ```

   Expected result: a resolved tracking issue is reused; a fully completed leaf
   is skipped. For partially completed checklists, recorded successful actions
   are reused and only remaining work proceeds. Explicitly authorised retries
   get a new attempt UUID under the same workstream. `unresolved` stays blocked.

Do not delete state, edit persisted outcomes by hand, replace UUIDs or remove
attempts to force replay. An empty issue search is not proof of non-creation.
If ownership or external outcomes remain ambiguous, keep the barrier in place.

## Related documentation

- [Operations routing explained](../operations-routing.md)
- [Routing and configuration reference](../reference/dev-orchestrator-routing.md)
- [Verify routing behaviour](verify-dev-orchestrator-routing.md)
