# Multi-Provider Workflow Architecture

> [Home](../index.md) > Concepts > Multi-Provider Workflow Architecture

Explains the design decisions behind the multi-provider workflow feature
([Issue #684](https://github.com/rysweet/amplihack-rs/issues/684)). For
implementation details, see the
[Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md).

---

## The Problem

`default-workflow` steps 03, 03b, 15, 16, and 21 call `gh` (GitHub CLI)
unconditionally. When the repository remote points to Azure DevOps, these
steps fail with confusing errors. When no remote exists (local-only), they
fail silently or produce broken output.

---

## Detect-Once, Branch-Everywhere

The core pattern: **one detection step** early in the workflow writes a
`remote_provider` context variable. Every downstream step reads it and
branches to the correct code path.

```
Step 01b (once) → remote_provider = "github" | "azdo" | "local"
    ↓
Steps 03, 03b, 15, 16, 21 → case $REMOTE_PROVIDER in ...
```

**Why not adapters?** An adapter/strategy pattern would add a layer of
indirection (provider interface, factory, registration) for three simple
`case` branches. The bash `case` statement is self-documenting, testable in
isolation, and avoids the indirection that would make recipe YAML harder to
read. If a fourth provider is added in the future, the cost of adding
another `case` branch is lower than maintaining an adapter registry.

---

## The Issue Number Contract

All providers produce the same output: a plain integer `issue_number`. This
is the contract that downstream steps depend on.

| Provider | Source                    | Example |
| -------- | ------------------------- | ------- |
| GitHub   | `gh issue create` URL     | `684`   |
| AzDO     | `az boards` work item ID  | `12345` |
| Local    | Synthetic (PID + epoch)   | `4821937` |

By normalizing to an integer at step 03b, no downstream step needs to know
which provider produced it. Branch names, commit messages, and PR
descriptions all use `issue_number` as a plain number with provider-specific
formatting applied only at the point of use (e.g., `#684` for GitHub,
`AB#12345` for AzDO, `[local-4821937]` for local).

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

Automated AzDO PR creation is planned as a follow-up once the AzDO CLI
integration is more robust.

---

## Local Fallback Rationale

The `local` provider is not an error state — it is a first-class mode that
enables the workflow to run in isolated environments (CI containers,
air-gapped systems, fresh `git init` repos). Local mode:

- Generates a synthetic `issue_number` using PID + epoch seconds for
  collision resistance
- Skips all network calls (no `gh`, no `az`)
- Produces valid branch names and commit messages
- Skips PR creation (no remote to push to)

---

## Trade-offs

| Decision                     | Benefit                            | Cost                                  |
| ---------------------------- | ---------------------------------- | ------------------------------------- |
| Detect-once pattern          | Single detection, no repeated work | One step to maintain                  |
| `case` over adapters         | Simpler, no indirection            | Adding providers requires editing steps |
| Numeric issue_number contract | Downstream steps stay unchanged   | Provider URL information is lost       |
| Local as fallback            | Works everywhere                   | No tracking system updated             |
| Manual AzDO PR creation      | Avoids brittle automation          | Extra manual step for AzDO users       |
| Context propagation required | Explicit data flow                 | Must declare vars in parent recipe     |

---

## Related

- [Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md) — full implementation details
- [How to Use the Workflow with Azure DevOps](../howto/use-workflow-with-azure-devops.md) — task-oriented guide
- [Step 03 Idempotency Guards](../reference/recipe-step-03-idempotency.md) — GitHub-specific guards
- [Workflow Issue Extraction](../reference/workflow-issue-extraction.md) — three-tier extraction

---

**Metadata**

| Field  | Value                                                     |
| ------ | --------------------------------------------------------- |
| Status | In Progress (retcon documentation; implementation pending) |
| Issue  | #684                                                      |
