# Recipe Simulation Reference

> [Home](../index.md) > Reference > Recipe Simulation

Recipe simulation runs representative workflow paths with fake providers, fake
tools, and fake agents. It validates recipe behavior without live GitHub, Azure
DevOps, network, or model calls.

## Purpose

Simulation tests protect workflow contracts that are difficult or expensive to
exercise against live services:

- provider detection and routing
- typed helper JSON consumption
- agentic contract validation
- terminal/finalization state
- runtime isolation
- stale/superseded cleanup
- manual and blocked provider paths

Simulation is deterministic. If a test needs adaptation or judgment, the fake
agent returns structured JSON and the deterministic validator decides whether it
is acceptable.

## Command

```bash
amplihack workflow simulate-recipe <RECIPE> \
  --scenario <SCENARIO> \
  --repo-fixture <PATH> \
  --format json
```

Example:

```bash
amplihack workflow simulate-recipe default-workflow \
  --scenario github-success \
  --repo-fixture tests/fixtures/workflows/repos/github-repo \
  --format json
```

## Scenario file

Scenario files live under `tests/fixtures/workflows/scenarios/*.yaml`.
Repository fixtures live under `tests/fixtures/workflows/repos/<name>/`.
Snapshot expectations live beside scenarios as
`tests/fixtures/workflows/snapshots/<scenario>.json`.

Scenario files describe fake provider, fake tool, fake agent, expected terminal
state, and forbidden-call behavior:

```yaml
name: github-success
recipe: default-workflow
repo_fixture: tests/fixtures/workflows/repos/github-repo
provider:
  kind: GitHub
  capabilities:
    tracking_items: Automated
    change_requests: Automated
    stale_cleanup: Automated
tools:
  git:
    status: clean
    base_ref: main
  gh:
    issue_create:
      id: "123"
      url: "https://github.com/acme/service/issues/123"
    pr_create:
      id: "812"
      url: "https://github.com/acme/service/pull/812"
agents:
  finalizer:
    output:
      schema_version: 1
      terminal_state: FOLLOWUP_CREATED
      terminal_success: true
      confidence: high
      reason: "Workflow published PR #812 with verification evidence."
      required_next_action: "Wait for review and required checks."
      hollow_success_detected: false
      evidence_used:
        - "change_request.id=812"
        - "verification.completed=true"
expect:
  terminal_state: FOLLOWUP_CREATED
  terminal_success: true
  forbidden_calls:
    - az.boards.work-item.create
    - az.repos.pr.create
```

Use canonical terminal-state names in fixtures:

```yaml
expect:
  terminal_state: FOLLOWUP_CREATED
```

Legacy Rust-style terminal names may be accepted by fixture loaders during the
migration window, but snapshots must store canonical `SCREAMING_SNAKE_CASE`
values.

## Fake runtime contracts

Simulation replaces live dependencies with deterministic fakes:

| Fake | Contract |
| --- | --- |
| Fake provider | Implements the same provider adapter trait as live adapters, returns helper-envelope JSON, and records each provider operation. |
| Fake tool runner | Replaces `git`, `gh`, `az`, network, and filesystem-sensitive provider calls with scripted outputs from the scenario file. |
| Fake agent | Returns structured JSON from the `agents` section. It never calls a model and never emits unstructured prose unless the scenario is testing invalid output. |
| Forbidden-call monitor | Fails the simulation immediately if a command listed in `expect.forbidden_calls` or derived from a provider boundary is invoked. |

Forbidden calls are not merely reported. A scenario fails if a forbidden command
is observed, even when the final terminal state otherwise matches.

## Required scenarios

Every material workflow behavior has at least one simulation or unit test. The
standard recipe simulation suite includes:

| Scenario | Expected behavior |
| --- | --- |
| `github-success` | GitHub issue and PR paths succeed through adapters. |
| `github-blocked-auth` | Missing GitHub auth returns `BlockedManualProvider`. |
| `azdo-work-item-manual-pr` | Azure Boards work item succeeds; Azure Repos PR creation returns `ManualRequired`. |
| `azdo-blocked-boards` | Missing Azure Boards setup returns manual or blocked state without calling GitHub. |
| `manual-provider` | Provider commands are not invoked; next action is provider-neutral. |
| `agent-invalid-json` | Agentic step fails closed through contract validation. |
| `hollow-success` | Empty or generic agent output becomes `HOLLOW_SUCCESS` or missing terminal evidence. |
| `runtime-isolation` | Runtime artifacts are written outside the worktree and known leftovers are cleaned before guard-sensitive steps. |
| `stale-cleanup-dry-run` | Superseded candidates are reported without mutation. |
| `stale-cleanup-apply` | Cleanup mutates only validated scoped candidates. |

## Output

`amplihack workflow simulate-recipe ... --format json` emits the normal helper
envelope with the simulation result under `data`:

```json
{
  "schema_version": 1,
  "provider": "AzureDevOps",
  "operation": "SimulateRecipe",
  "status": "Succeeded",
  "next_action": "Create an Azure Repos pull request from the pushed branch.",
  "warnings": [],
  "data": {
    "recipe": "default-workflow",
    "scenario": "azdo-work-item-manual-pr",
    "terminal_state": "MANUAL_REQUIRED",
    "terminal_success": false,
    "assertions": {
      "failed": 0,
      "items": ["TerminalStateMatched", "TerminalSuccessMatched", "ForbiddenCallsAbsent"]
    },
    "provider_calls": [
      "az.boards.work-item.show",
      "az.boards.work-item.create"
    ],
    "forbidden_calls": [
      "gh.issue.create",
      "gh.pr.create"
    ],
    "agent_contracts": {
      "finalizer": {
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Azure Boards tracking and Azure Repos PR creation succeeded.",
        "required_next_action": "Monitor Azure Repos PR validation.",
        "hollow_success_detected": false,
        "evidence_used": [
          "provider=AzureDevOps",
          "change_requests=Automated"
        ]
      }
    }
  }
}
```

## Snapshot expectations

Simulation snapshots store the helper envelope after volatile fields are removed.
Snapshots must include:

1. provider,
2. operation,
3. status,
4. next action,
5. terminal state and success,
6. provider calls,
7. forbidden-call assertions,
8. agent contract validation results, and
9. runtime artifact locations relative to the simulation runtime root.

Snapshot updates are intentional maintenance. Use an explicit update flag such
as `--update-snapshots`; ordinary simulation runs must fail on snapshot drift.

## Regression contract

Simulation tests fail when:

1. a recipe calls a provider command outside its adapter,
2. a recipe parses provider prose instead of helper JSON,
3. an agentic step emits non-JSON or unsupported states,
4. a manual/blocked provider path is reported as success,
5. runtime artifacts appear in the commit worktree,
6. terminal state is missing from final output, or
7. stale cleanup mutates a candidate that was not validated by scope and dry-run
   evidence.

## See also

- [Tutorial: Simulate Provider-Neutral Workflows](../tutorials/provider-neutral-workflow-simulation.md)
- [Provider-Neutral Workflow API](workflow-provider-contract.md)
- [Workflow Runtime Artifacts Reference](workflow-runtime-artifacts.md)
