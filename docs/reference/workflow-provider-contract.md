# Provider-Neutral Workflow API

> [Home](../index.md) > Reference > Provider-Neutral Workflow API

This reference defines the provider-neutral workflow domain model, JSON helper
schemas, provider adapter behavior, and validation contracts used by
`default-workflow`.

## Contents

- [Rust domain models](#rust-domain-models)
- [Provider capability states](#provider-capability-states)
- [Serialized enum names](#serialized-enum-names)
- [Helper command output envelope](#helper-command-output-envelope)
- [Change-request model](#change-request-model)
- [Terminal states](#terminal-states)
- [Agent validation contracts](#agent-validation-contracts)
- [Stale cleanup model](#stale-cleanup-model)
- [Manual and blocked transitions](#manual-and-blocked-transitions)
- [Exit behavior](#exit-behavior)

## Rust domain models

The pure `amplihack-workflows` crate owns the stable domain types. Adapters
convert provider data into these models.

```rust
pub enum RepositoryProvider {
    GitHub,
    AzureDevOps,
    Manual,
}

pub enum ProviderCapabilityState {
    Automated,
    ManualRequired,
    BlockedManualProvider,
    Unsupported,
}

pub struct ProviderContext {
    pub schema_version: u32,
    pub provider: RepositoryProvider,
    pub repository: RepositoryIdentity,
    pub capabilities: ProviderCapabilities,
    pub status: ProviderOperationStatus,
    pub next_action: String,
}

pub struct ChangeRequest {
    pub kind: ChangeRequestKind,
    pub id: String,
    pub url: String,
    pub state: ChangeRequestStatus,
    pub source_branch: String,
    pub base_branch: String,
    pub head_sha: Option<String>,
}

pub enum TerminalState {
    FollowupCreated,
    ManualRequired,
    BlockedManualProvider,
    HollowSuccess,
    FailedInvalidEvidence,
    FailedFinalizerOutput,
    Failed,
}
```

## Provider capability states

| State | Meaning |
| --- | --- |
| `Automated` | The adapter can perform the operation with current tools and credentials. |
| `ManualRequired` | Amplihack intentionally does not automate this operation for the provider. |
| `BlockedManualProvider` | Automation would be possible only after credentials, permissions, tools, or provider configuration are fixed. |
| `Unsupported` | No adapter supports the operation. |

Capability states appear in provider detection and operation results.

## Serialized enum names

Rust types keep Rust-style variant names. JSON uses two explicit naming
families:

| Enum family | JSON format | Examples |
| --- | --- | --- |
| Provider names | Rust variant names | `GitHub`, `AzureDevOps`, `Manual` |
| Provider capability and operation status values | Rust variant names | `Automated`, `ManualRequired`, `BlockedManualProvider`, `Succeeded`, `Failed` |
| Terminal states | `SCREAMING_SNAKE_CASE` | `FOLLOWUP_CREATED`, `MANUAL_REQUIRED`, `BLOCKED_MANUAL_PROVIDER`, `HOLLOW_SUCCESS` |

Terminal-state validators may accept older Rust-style terminal names during a
migration window, but they must normalize emitted JSON to the canonical
`SCREAMING_SNAKE_CASE` names:

| Legacy terminal value | Canonical terminal value |
| --- | --- |
| `FollowupCreated` | `FOLLOWUP_CREATED` |
| `ManualRequired` | `MANUAL_REQUIRED` |
| `BlockedManualProvider` | `BLOCKED_MANUAL_PROVIDER` |
| `HollowSuccess` | `HOLLOW_SUCCESS` |

## Helper command output envelope

Every `amplihack workflow ... --format json` command emits the same top-level
envelope:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "DetectProvider",
  "status": "Succeeded",
  "next_action": "No further provider setup is required.",
  "warnings": [],
  "data": {
    "repository": {
      "remote_url": "https://github.com/acme/service.git",
      "owner": "acme",
      "name": "service",
      "default_base": "main"
    },
    "capabilities": {
      "tracking_items": "Automated",
      "change_requests": "Automated",
      "stale_cleanup": "Automated"
    }
  }
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `schema_version` | Yes | Current schema version. Unknown versions fail validation. |
| `provider` | Yes | `GitHub`, `AzureDevOps`, or `Manual`. |
| `operation` | Yes | Stable operation name. |
| `status` | Yes | `Succeeded`, `ManualRequired`, `BlockedManualProvider`, or `Failed`. |
| `next_action` | Yes | Actionable text. Success states explain why no action is needed. Manual/blocked states explain exactly what to do. |
| `warnings` | Yes | Array of non-fatal diagnostic strings. |
| `data` | Yes | Operation-specific object. |

Operation-specific fields always live under `data`. Do not put
`tracking_item`, `change_request`, `manual_action`, `recipe`, `scenario`, or
assertion details at the top level.

Shell helpers and recipes must parse this JSON. They must not infer state from
stderr or provider CLI prose. Stderr is diagnostic only.

## Change-request model

`change-request publish` and `change-request status` return
`data.change_request` when a provider-native change request exists:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "PublishChangeRequest",
  "status": "Succeeded",
  "next_action": "Wait for required checks and review.",
  "warnings": [],
  "data": {
    "change_request": {
      "kind": "PullRequest",
      "id": "812",
      "url": "https://github.com/acme/service/pull/812",
      "state": "Open",
      "source_branch": "feat/auth-timeout",
      "base_branch": "main",
      "head_sha": "1d2c3b4a"
    }
  }
}
```

Status values:

| Value | Meaning |
| --- | --- |
| `Draft` | Created but not ready for review. |
| `Open` | Active and reviewable. |
| `Merged` | Provider reports merge evidence. |
| `Closed` | Closed without merge evidence. |

Manual-provider results include `data.manual_action` and `data.change_request:
null`. Azure Repos pull-request publication is an automated Azure DevOps
capability when the `az` CLI and provider context are available; provider or
authentication failures must return blocked-provider evidence instead of manual
success.

## Terminal states

Terminal-state JSON appears in recipe results, logs, and
`amplihack workflow terminal-state` output.

```json
{
  "schema_version": 1,
  "terminal_success": true,
  "terminal_state": "FOLLOWUP_CREATED",
  "terminal_reason": "Azure Repos pull-request creation succeeded.",
  "required_next_action": "Monitor Azure Repos PR validation.",
  "provider": "AzureDevOps",
  "evidence_used": [
    "provider=AzureDevOps",
    "change_requests=Automated",
    "git.clean=true"
  ]
}
```

`MANUAL_REQUIRED` and `BLOCKED_MANUAL_PROVIDER` are non-success states unless a
later deterministic provider status check proves that the manual action is
complete.

## Agent validation contracts

Agentic steps use named contracts:

| Contract | Purpose |
| --- | --- |
| `finalization` | Classify one terminal state from structured workflow evidence. |
| `stale-cleanup-decision` | Decide whether a candidate change request is stale, active, superseded, or unsafe to touch. |
| `review-readiness` | Decide whether evidence is enough to mark a workflow-owned change request ready. |

Validate finalizer output through the `amplihack-workflows::agent_contract`
Rust API. Recipe simulation also exercises this validator with fake agent
outputs.

Validation rules:

1. Output must be a single JSON object.
2. `schema_version` must be supported.
3. Decision and state fields must be known enums.
4. `confidence=high` is required for success or mutation decisions.
5. `reason`, `required_next_action`, and `evidence_used` must be non-empty.
6. Provider-specific actions must match the detected provider capability.
7. Secrets and raw environment dumps are rejected.

## Stale cleanup model

`cleanup-stale` supports dry-run and apply modes:

```bash
amplihack workflow cleanup-stale \
  --provider github \
  --dry-run \
  --format json
```

Dry-run result:

```json
{
  "schema_version": 1,
  "provider": "GitHub",
  "operation": "CleanupStale",
  "status": "Succeeded",
  "next_action": "Run again without --dry-run to close 2 superseded pull requests.",
  "warnings": [],
  "data": {
    "provider": "GitHub",
    "mode": "DryRun",
    "actions": [
      {
        "change_request_id": "791",
        "action": "WouldCloseAsSuperseded",
        "reason": "dry-run: workflow-owned superseded change request is eligible"
      }
    ],
    "mutations_executed": 0
  }
}
```

The cleanup helper mutates provider state only when:

1. the provider adapter supports the action,
2. the candidate matches scoped workflow identity,
3. the agentic stale decision contract validates with high confidence,
4. dry-run output has already identified the same candidate/action pair, and
5. provider mutation succeeds and returns durable evidence.

## Manual and blocked transitions

Manual and blocked states use one transition contract across helpers, recipes,
and terminal output:

| Helper result | Helper exit | Recipe behavior | Terminal-state mapping |
| --- | --- | --- | --- |
| `status=Succeeded` with durable evidence | `0` | Continue and persist `data`. | A later gate may emit a success terminal state if all required evidence is present. |
| `status=ManualRequired` | `0` for operation helpers | Persist `data.manual_action`, surface `next_action`, and stop any automated provider mutation path. | Required publication/finalization gates emit `MANUAL_REQUIRED`, `terminal_success=false`, and nonzero gate exit. |
| `status=BlockedManualProvider` | `2` | Surface the blocker and do not switch to another provider. Recipes should still parse JSON before failing. | Required gates emit `BLOCKED_MANUAL_PROVIDER`, `terminal_success=false`, and nonzero gate exit. |
| `status=Failed` | `1` | Fail closed. Recipes should still parse JSON before failing. | Required gates emit the most specific `FAILED_*` state. |

`ManualRequired` means the operation is intentionally manual for this provider
contract. `BlockedManualProvider` means the requested provider path could not
complete because tooling, credentials, permissions, repository metadata, or APIs
are unavailable.

## Exit behavior

Operation helpers return `0` when they produced valid JSON for `Succeeded` or
`ManualRequired`; `2` for `BlockedManualProvider`; and `1` for `Failed` or
invalid invocation. Terminal-state gates return `0` only for
`terminal_success=true`, `3` for known non-success terminal states, and `1` for
invalid input or malformed evidence.

Recipes must read helper JSON on both zero and nonzero exits. The exit code
chooses control flow; the JSON chooses provider state, next action, and terminal
classification.
