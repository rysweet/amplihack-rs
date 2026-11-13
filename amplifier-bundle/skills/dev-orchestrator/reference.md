# Dev orchestrator reference

The authoritative internal contracts are documented in
[workstream state](../../../docs/reference/workstream-state.md) and
[Operations routing](../../../docs/operations-routing.md).

## Decomposition

```json
{
  "task_type": "Operations",
  "workstreams": [
    {
      "name": "research",
      "classification": "Operations",
      "description": "Answer the question set in chat.",
      "depends_on": [],
      "required_outputs": {"answer": "answer"}
    },
    {
      "name": "docs",
      "classification": "Development",
      "description": "Commit the research answers to README.md.",
      "depends_on": ["research"],
      "required_outputs": {"delivery": "delivery"}
    }
  ]
}
```

Use one leaf per trusted repository for cross-repository edits. Omitted repository
names use the current checkout. Explicit repository names must match the origins
of the current checkout or roots in `AMPLIHACK_REPOSITORY_ROOTS`.

Classification precedence is `classification`, then `task_type`, then `type`.
An invalid explicit value is an error. Legacy homogeneous plans may inherit the
top-level classification. Mixed plans need explicit classifications on every
leaf. Empty plans, duplicate keys or identities, invalid outputs, unknown or
duplicate dependencies, self-edges and cycles are rejected before execution.

## Normalised configuration

`amplihack orch helper build-workstreams-config` reads decomposition JSON on
stdin and atomically saves a private JSON configuration. Each leaf has an
immutable UUID, key, repository, canonical classification, trusted recipe, task,
resolved dependency UUIDs, required outputs and optional tracking. Operations
has no generated branch. Unknown issues are omitted, never serialised as zero
or a placeholder.

`amplihack multitask run <CONFIG>` executes the saved leaves. Use
`AMPLIHACK_MAX_PARALLEL_WORKSTREAMS=1` for sequential execution; zero retains the
existing unlimited-capacity setting. Capacity is released by child completion,
with stdout and stderr drained to EOF before result validation.

## Administrative mutations

`operations-direct` runs one agent. Issue creation and assignment use:

```text
amplihack multitask operation --context-file <CONTEXT> \
  --output-name <DECLARED_OUTPUT> --request-file <PRIVATE_JSON>
```

Requests have an action and typed arguments. Supported issue actions are
`create_issue` with title/body and `assign` with issue/assignees. The adapter
records an attempt before invoking `gh` with direct arguments. Successful
receipts are reused; unknown outcomes require reconciliation. Other external
systems need inspectable receipts and the same conservative interruption barrier.

## Explicit tracking

Optional tracking is the default. Required tracking reuses a supplied positive
number, a saved success or an explicit prerequisite binding. Without these,
creation requires a non-blank title and body. Issue bodies pass literally through
stdin. A CLI failure cannot establish that the service rejected the mutation;
ambiguous outcomes remain visible without automatic retry.

A producer binding names a declared issue output and a dependency. The normaliser
resolves its producer to a UUID and checks the repository. Administrative issue
outputs never become downstream tracking implicitly.

## Resume and reconciliation

Saved successes are validated and skipped. An interrupted Operations leaf is
conservatively fenced until its external outcome is known. Use:

```text
amplihack multitask reconcile <CONFIG> --resolution-file <RESOLUTION_JSON>
```

The command takes the dispatcher lock and records a revision-bound decision
without launching children or mutating GitHub. Decisions use a unique resolution
UUID, workstream UUID, repository, current revision, inspectable evidence and a
reason. Attempt decisions also identify the saved attempt. An identical repeated
decision returns its saved receipt; conflicting payloads and stale revisions fail.
Unknown outcomes retain the barrier. `not_applied` requires explicit retry
authorisation before another attempt can run.

Legacy issue-keyed state is adopted only after repository, directory and actual
branch ownership checks. Recorded working, state, log and progress paths remain
in place. Ambiguous adoption is rejected rather than guessed from an issue
number or UUID alone.

## Delivery and validation

Operations requires its declared answer, issue or mutation outputs with evidence.
Development retains its selected workflow and delivery gates; the completion
adapter verifies the delivered Git head against the PR. A legitimate no-change
result needs terminal adjudication evidence. The supported quick-change hand-over
records `default-workflow` as the effective route. Investigation and Q&A retain
their reports, and Consensus retains its delivery path.

All state and result writes are atomic. A failed durable write stops dispatch
before dependants can launch. Cleanup checks identity plus repository, path,
branch and process-start evidence; Operations cleanup never removes its working
repository. Runtime budgets no longer drive polling or wall-clock interruption.
