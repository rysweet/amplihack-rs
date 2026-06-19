# Multi-Provider Workflow Architecture

> [Home](../index.md) > Concepts > Multi-Provider Workflow Architecture

Explains the design decisions behind provider-aware workflow routing. For
implementation details, see the
[Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md).

---

## The Problem

Workflow tracking and publication steps need provider-specific commands.
GitHub repositories use GitHub Issues and GitHub pull requests. Azure DevOps
repositories use Azure Boards or local tracking and require manual PR creation.
Local or unsupported repositories need local tracking without provider network
calls.

---

## Detect-Once, Branch-Everywhere

The core pattern: **one detection step** early in the workflow writes a
`remote_host_type` context variable. Every downstream step reads it and
branches to the correct code path.

```
Step 02d (once) → remote_host_type = "github" | "azdo" | "other"
    ↓
Steps 03, 03b, 15, 16, 21 → case $REMOTE_HOST_TYPE in ...
```

**Why not adapters?** An adapter/strategy pattern would add a layer of
indirection (provider interface, factory, registration) for three simple
`case` branches. The bash `case` statement is self-documenting, testable in
isolation, and avoids the indirection that would make recipe YAML harder to
read. If a fourth provider is added in the future, the cost of adding
another `case` branch is lower than maintaining an adapter registry.

---

## The Tracking Identifier Contract

Remote providers produce the same output shape: a plain integer `issue_number`.
Local tracking produces an opaque local reference instead. This keeps GitHub and
Azure DevOps behavior stable while avoiding the false claim that a local ID is a
remote issue number.

| Provider | Source | Workflow identifier |
| -------- | ------ | ------------------- |
| GitHub | `gh issue create` URL | `issue_number=684` |
| AzDO | `az boards` work item ID | `issue_number=12345` |
| Other/Local | Structured local metadata | `tracking_reference=local-482193`, `issue_number=` |

Step 03b applies numeric extraction only after it has found no local-prefixed
tracking reference. Branch names, commit messages, and PR descriptions use
provider-specific formatting at the point of use: `Closes #684` for GitHub,
`AB#12345` for AzDO, and `Ref local-482193` for local tracking.

---

## PR Creation Asymmetry

GitHub PR creation is fully automated via `gh pr create`. AzDO PR creation
is logged as manual instructions rather than automated. This asymmetry
exists because:

1. `gh pr create` handles draft PRs, labels, reviewers, and closing
   keywords in a single command. `az repos pr create` requires separate
   calls for reviewers and labels.
2. AzDO repository IDs must be resolved separately — they are not
   derivable from the remote URL alone.
3. The AzDO CLI's error messages for misconfigured projects are opaque,
   making automated recovery difficult.

---

## Other/Local Metadata Rationale

The `other` host type is not an error state — it is a first-class mode that
enables the workflow to run in isolated environments (CI containers,
air-gapped systems, fresh `git init` repos). Other/local mode:

- Emits structured local metadata such as `tracking_system=local`,
  `tracking_reference=local-*`, and `issue_creation=local-tracking`; the
  local-prefixed reference is required for successful local extraction
- Skips all network calls (no `gh`, no `az`)
- Produces valid branch names and commit messages without numeric issue
  extraction
- Skips PR creation (no remote to push to)

---

## Trade-offs

| Decision                     | Benefit                            | Cost                                  |
| ---------------------------- | ---------------------------------- | ------------------------------------- |
| Detect-once pattern          | Single detection, no repeated work | One step to maintain                  |
| `case` over adapters         | Simpler, no indirection            | Adding providers requires editing steps |
| Remote issue_number contract | GitHub/AzDO steps stay unchanged | Local steps also need `tracking_reference` |
| Other as fallback            | Works everywhere                   | No tracking system updated             |
| Manual AzDO PR creation      | Avoids brittle automation          | Extra manual step for AzDO users       |
| Context propagation required | Explicit data flow                 | Must declare vars in parent recipe     |

---

## Related

- [Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md) — full implementation details
- [How to Use the Workflow with Azure DevOps](../howto/use-workflow-with-azure-devops.md) — task-oriented guide
- [Step 03 Host-Aware Tracking Idempotency](../reference/recipe-step-03-idempotency.md) — GitHub, AzDO, and local tracking guards
- [Workflow Issue Extraction](../reference/workflow-issue-extraction.md) — three-tier extraction

---

**Metadata**

| Field    | Value                              |
| -------- | ---------------------------------- |
| Contract | Provider-aware workflow routing    |
