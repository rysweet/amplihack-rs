# Provider-Neutral Workflow Architecture

> [Home](../index.md) > Concepts > Provider-Neutral Workflow Architecture

> [PLANNED - Implementation Pending]
>
> This concept page explains the target architecture for the provider-neutral
> workflow feature. Helper commands named here are planned implementation
> surfaces.

Amplihack workflows use a provider-neutral domain contract for tracking items,
change requests, publication, stale cleanup, and terminal state. Recipes do not
hard-code GitHub or Azure DevOps command logic. They call typed helper commands,
consume structured JSON, and let provider adapters translate the domain contract
to the active repository host.

## Contents

- [The model](#the-model)
- [Provider boundary](#provider-boundary)
- [Deterministic helpers](#deterministic-helpers)
- [Agentic judgment with validation](#agentic-judgment-with-validation)
- [Provider states](#provider-states)
- [Why this shape](#why-this-shape)
- [Related documentation](#related-documentation)

## The model

The pure `amplihack-workflows` domain layer owns provider-neutral concepts:

| Concept | Meaning |
| --- | --- |
| `RepositoryProvider` | The classified host for the repository: GitHub, Azure DevOps, local, or unsupported. |
| `TrackingItem` | A provider issue, Azure Boards work item, or local workflow reference. |
| `ChangeRequest` | A reviewable change such as a GitHub pull request, Azure Repos pull request, or manual change request. |
| `ProviderOperation` | A deterministic action such as detect provider, create tracking item, publish change request, query status, or clean stale work. |
| `TerminalState` | The explicit final workflow state, including success, blocked, manual, and failure states. |

The CLI owns adapters because adapters depend on host tools and credentials:

```text
recipe YAML
  -> amplihack workflow <helper> --format json
  -> amplihack-cli provider adapter
  -> amplihack-workflows domain model
  -> structured JSON result
  -> deterministic recipe validation
```

## Provider boundary

The provider boundary is narrow. Recipes pass repository context and task data to
helper commands; helpers return JSON with stable field names.

| Provider | Automated behavior | Manual or blocked behavior |
| --- | --- | --- |
| GitHub | Issues, pull requests, PR status, merge status, and stale/superseded cleanup through `gh` when authenticated. | Missing `gh` or auth returns `BlockedManualProvider` with `next_action`. |
| Azure DevOps | Azure Boards work item reuse/create when `az` is configured. | Azure Repos PR publication and cleanup return `ManualRequired`; `default-workflow` does not automate `az repos pr create` or Azure Repos cleanup mutation. Missing Boards support returns local tracking or `BlockedManualProvider`, depending on the requested operation. |
| Local or unsupported | Local tracking references and local Git evidence only. | Publication and remote cleanup return `ManualRequired` with a provider-neutral next action. |

Adapters never pretend a provider action succeeded. When live automation is not
available, the serialized result is explicit:

```json
{
  "schema_version": 1,
  "status": "ManualRequired",
  "provider": "AzureDevOps",
  "operation": "CreateChangeRequest",
  "next_action": "Create an Azure Repos pull request from feat/auth-timeout to main and include AB#12345 in the description.",
  "warnings": [],
  "data": {
    "change_request": null,
    "manual_action": {
      "kind": "CreateChangeRequest",
      "source_branch": "feat/auth-timeout",
      "base_branch": "main",
      "tracking_item_ref": "AB#12345"
    }
  }
}
```

## Deterministic helpers

Stable parsing and decision logic lives in typed Rust helpers instead of inline
shell snippets. Recipes call these helpers and validate their JSON.

| Helper | Deterministic responsibility |
| --- | --- |
| `amplihack workflow detect-provider` | Normalize the Git remote and classify the provider. |
| `amplihack workflow tracking-item ensure` | Reuse or create a tracking item, or return local/manual state. |
| `amplihack workflow change-request publish` | Publish or describe the required manual publication action. |
| `amplihack workflow change-request status` | Query provider status and normalize it into `ChangeRequestStatus`. |
| `amplihack workflow terminal-state` | Validate final workflow evidence and emit one terminal state. |
| `amplihack workflow cleanup-stale` | Dry-run or apply stale/superseded cleanup through the provider abstraction. |
| `amplihack workflow validate-agent-contract` | Validate structured agentic step output against a named contract. |
| `amplihack workflow simulate-recipe` | Run deterministic recipe simulations with fake providers, tools, and agents. |

Shell remains useful for orchestration glue, but not for durable parsing rules,
provider classification, terminal-state decisions, stale cleanup decisions, or
agent-output validation.

## Agentic judgment with validation

Judgment-heavy work stays agentic. Examples include finalization assessment,
review-readiness interpretation, stale/superseded classification, and choosing a
next action when provider metadata is incomplete.

Those steps must return a deterministic validation contract:

```json
{
  "schema_version": 1,
  "decision": "Superseded",
  "confidence": "high",
  "reason": "PR #812 has the same workflow scope and newer head SHA.",
  "required_next_action": "Close PR #791 as superseded by PR #812.",
  "evidence_used": [
    "scope.repository",
    "scope.head_branch",
    "candidate_pr.head_sha",
    "candidate_pr.created_at"
  ]
}
```

The recipe succeeds only after deterministic validation accepts the JSON schema,
known enum values, confidence rules, required evidence, and provider-safe next
action. Free-form prose is diagnostic text; it cannot prove success.

## Provider states

Provider helpers use these stable state values:

| State | Meaning |
| --- | --- |
| `Succeeded` | The provider operation completed and returned durable evidence. |
| `NoOp` | Nothing needed to change, and the reason is explicit. |
| `ManualRequired` | Automation is intentionally unavailable; `next_action` tells the operator what to do. |
| `BlockedManualProvider` | The provider path is blocked by missing credentials, permissions, tooling, or unsupported API behavior. |
| `Failed` | The operation failed and cannot be treated as terminal success. |

`ManualRequired` and `BlockedManualProvider` are not success-shaped fallbacks.
They are auditable terminal or intermediate states that downstream recipes must
surface in final output.

## Why this shape

The architecture keeps the simple parts simple and the judgment-heavy parts
adaptable:

| Decision | Benefit |
| --- | --- |
| Pure domain layer | Provider concepts are tested without live GitHub or Azure DevOps calls. |
| CLI-owned adapters | Host tools, auth, and filesystem access stay at the edge. |
| Typed JSON helpers | Recipes consume stable contracts instead of parsing fragile command text. |
| Agentic steps with schemas | Reasoning remains available without letting prose drive workflow state. |
| Simulation tests | Success, failure, manual, and blocked paths are reproducible without external services. |

## Related documentation

- [Provider-Neutral Workflow Reference](../reference/workflow-provider-contract.md)
- [Multi-Provider Workflow Reference](../reference/multi-provider-workflow.md)
- [Configure Provider-Neutral Workflows](../howto/configure-provider-neutral-workflows.md)
- [Recipe Simulation Reference](../reference/workflow-simulation.md)
- [Workflow Terminal-State Reference](../reference/workflow-terminal-state.md)
