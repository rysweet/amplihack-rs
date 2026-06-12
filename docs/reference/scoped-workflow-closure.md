# Scoped Workflow Closure Reference

> [Home](../index.md) > Reference > Scoped Workflow Closure

Field-level contract for scoped PR ownership, scoped process ownership,
multitask state persistence, and fail-closed monitor decisions.

## Contents

- [Workflow Context Inputs](#workflow-context-inputs)
- [Environment Inputs](#environment-inputs)
- [Operation Requirements](#operation-requirements)
- [PR Scope Persistence](#pr-scope-persistence)
- [PR Scope Helper](#pr-scope-helper)
- [PR Scope Result Schema](#pr-scope-result-schema)
- [Multitask State Fields](#multitask-state-fields)
- [Process Scope API](#process-scope-api)
- [Process Validation Outcomes](#process-validation-outcomes)
- [Recipe Integration Contract](#recipe-integration-contract)
- [Failure Semantics](#failure-semantics)
- [Deterministic Checks](#deterministic-checks)

## Workflow Context Inputs

`default-workflow` and workflow closure recipes pass these identity fields to
publish, readiness, final-status, and multitask monitor steps.

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `repo_path` | path string | Yes | Repository root or worktree root for the current workflow. |
| `repository` | `owner/name` string | Required for GitHub PR scope | Exact GitHub repository. Resolved from `repo_path` when omitted and a GitHub remote exists. |
| `head_ref` or `branch` | string | Yes | Exact branch owned by the workflow. |
| `base_ref` | string | Required for PR scope | Exact PR target branch. |
| `pr_number` | integer or numeric string | Required when resuming a known PR | Existing pull request number. |
| `pr_url` | URL string | Optional with `pr_number` | Existing pull request URL. Must agree with `repository` and `pr_number`. |
| `issue_number` | integer or numeric string | Required when the workflow is issue-backed | GitHub issue expected in PR metadata or closing references. |
| `work_item_id` | string | Required when the workflow is work-item-backed | Non-GitHub work item expected in PR metadata. |
| `expected_pr_title_prefix` | string | Required for title-based candidate lookup | Prefix that newly created workflow PRs must use. |
| `created_after` | RFC 3339 timestamp | Yes | Workflow start time. Older PRs are rejected. |
| `head_sha` | full Git SHA | Required for readiness, final status, and exact-head no-op paths | PR head SHA that must match before scoped success is reported. |
| `recipe_run_id` | string | Required for multitask monitor authority | Unique recipe run id. |
| `tree_id` | string | Required for nested recipe authority | Recipe execution tree id. |
| `workstream_id` | string | Required for workstream authority | Logical workstream id. |

Known PR identity is authoritative when present. If `pr_number` or `pr_url` is
known, validators check that concrete PR first and fail closed on mismatch.
Scoped lookup without a known PR requires the exact tuple
`repository + head_ref + base_ref + created_after` plus at least one tracking
discriminator: `issue_number`, `work_item_id`, or `expected_pr_title_prefix`.
`head_sha` narrows the candidate for exact-head operations; it never
broadens a match.

## Environment Inputs

| Variable | Required | Meaning |
| --- | --- | --- |
| `AMPLIHACK_HOME` | No | Overrides the framework bundle root used by recipe tools. |
| `AMPLIHACK_AGENT_BINARY` | No | Propagates the active agent binary to nested workflow agents. |

No environment variable disables scoped validation. If scope is incomplete, the
validator returns a named invalid result.

Operational runtime settings such as `NODE_OPTIONS` may be useful for large
nested runs, but they are not identity inputs and do not affect scoped
validation.

## Operation Requirements

| Operation | Required identity |
| --- | --- |
| Publish a new PR | `repository`, `head_ref`, `base_ref`, `created_after`, and `issue_number`, `work_item_id`, or `expected_pr_title_prefix`. |
| Update or resume a known PR | `pr_number` or `pr_url`, plus `repository`, `head_ref`, `base_ref`, and `created_after`. |
| Lookup a PR before identity is persisted | `repository`, `head_ref`, `base_ref`, `created_after`, and at least one of `issue_number`, `work_item_id`, or `expected_pr_title_prefix`. |
| Check readiness | Known PR identity plus `repository`, `head_ref`, `base_ref`, `created_after`, and `head_sha`. |
| Report final status | Known PR identity plus `repository`, `head_ref`, `base_ref`, `created_after`, and `head_sha`. |
| Validate a workstream process | `repo_path`, `workdir`, `branch`, `base_ref`, `recipe_run_id`, `tree_id`, `workstream_id`, `started_at`, `pid`, and process start metadata. |

Issue and work-item fields are mandatory when the workflow was launched from
that tracking system. Title-prefix lookup is a bootstrap path for newly created
PRs; it is not a replacement for persisted PR identity after publish succeeds.

## PR Scope Persistence

PR identity is passed through recipe context and publish outputs. Closure
recipes validate a persisted `pr_number` or `pr_url` first when either exists.

```json
{
  "pr_identity": {
    "repository": "rysweet/amplihack-rs",
    "number": 812,
    "url": "https://github.com/rysweet/amplihack-rs/pull/812",
    "head_ref": "feat/issue-754-scoped-closure",
    "base_ref": "main",
    "issue_number": 754,
    "work_item_id": null,
    "expected_pr_title_prefix": "Fix issue #754:",
    "created_after": "2026-06-12T03:52:44Z",
    "head_sha": "6c8e3b2a4f2e55d0fb65d51d94d8f4c16f37a111",
    "captured_at": "2026-06-12T04:03:24Z"
  }
}
```

Precedence:

1. Validate persisted `number` and `url` when either exists.
2. If no concrete PR identity exists, perform scoped lookup using repository,
   head branch, base branch, start time, and tracking discriminator.
3. Reject mismatches; do not replace persisted identity with a recent, same-author,
   or broad-text candidate.
4. Persist the concrete PR identity immediately after publish succeeds.

## PR Scope Helper

`amplifier-bundle/tools/workflow_pr_scope.sh` is the only helper that determines
current-work PR identity for workflow closure.

```bash
amplifier-bundle/tools/workflow_pr_scope.sh \
  --repo rysweet/amplihack-rs \
  --head feat/issue-754-scoped-closure \
  --base main \
  --created-after 2026-06-12T03:52:44Z \
  --issue 754 \
  --expected-pr-title-prefix "Fix issue #754:" \
  --head-sha "$(git rev-parse HEAD)"
```

When a PR is already known, pass the concrete identity:

```bash
amplifier-bundle/tools/workflow_pr_scope.sh \
  --repo rysweet/amplihack-rs \
  --pr-number 812 \
  --pr-url https://github.com/rysweet/amplihack-rs/pull/812 \
  --head feat/issue-754-scoped-closure \
  --base main \
  --created-after 2026-06-12T03:52:44Z \
  --issue 754
```

Rules:

1. `--repo`, `--head`, `--base`, and `--created-after` are exact match
   fields.
2. `--pr-number` and `--pr-url` must identify the same repository and PR.
3. `--expected-pr-title-prefix` is a prefix check, not a substring search.
4. `--issue` and `--work-item` match structured closing/reference
   fields or exact workflow metadata. They do not run broad text grep.
5. `--head-sha` must equal the PR head SHA when supplied.
6. The PR base repository and head repository must match `--repo` by default;
   fork and cross-repository PRs exit with `cross_repo_pr`.
7. Zero candidates, multiple candidates, stale candidates, and mismatches exit
   non-zero with structured JSON.
8. The helper never falls back to `gh pr list --author @me`, recent PR ordering,
   or broad title/body grep.

## PR Scope Result Schema

Successful validation prints JSON on stdout and exits `0`:

```json
{
  "ok": true,
  "scoped": true,
  "number": 812,
  "url": "https://github.com/rysweet/amplihack-rs/pull/812",
  "headRefName": "feat/issue-754-scoped-closure",
  "baseRefName": "main",
  "headRefOid": "6c8e3b2a4f2e55d0fb65d51d94d8f4c16f37a111",
  "createdAt": "2026-06-12T04:03:19Z"
}
```

Invalid validation prints JSON and exits non-zero:

```json
{
  "ok": false,
  "reason": "no_scoped_pr",
  "message": "no PR matched the explicit workflow scope"
}
```

The `reason` value is stable and intended for recipes, shell tests, and monitor
checks. Human-readable `message` text is diagnostic only.

## Multitask State Fields

`WorkstreamState` persists logical workflow scope and concrete process scope.
Missing fields deserialize successfully so old state files remain readable.

```json
{
  "workstream_id": "issue-754-closure",
  "status": "running",
  "scope": {
    "repository": "rysweet/amplihack-rs",
    "repo_path": "/home/user/src/amplihack-rs",
    "workdir": "/home/user/src/amplihack-rs/worktrees/feat/issue-754-scoped-closure",
    "branch": "feat/issue-754-scoped-closure",
    "base_ref": "main",
    "issue_number": 754,
    "work_item_id": null,
    "recipe_run_id": "run-01JZ7G7R4Z8KX8F2M9S5YH1VKT",
    "tree_id": "tree-01JZ7G7R5A7W1JYQXP3CBB2C2E",
    "workstream_id": "issue-754-closure",
    "started_at": "2026-06-12T03:52:44Z"
  },
  "process_scope": {
    "pid": 41872,
    "process_started_at": "2026-06-12T03:52:47Z",
    "process_start_marker": "41872:74293011",
    "captured_at": "2026-06-12T03:52:48Z"
  },
  "process_scope": {
    "pid": 41872,
    "repository": "rysweet/amplihack-rs",
    "repo_root": "/home/user/src/amplihack-rs/worktrees/feat/issue-754-scoped-closure",
    "workdir": "/home/user/src/amplihack-rs/worktrees/feat/issue-754-scoped-closure",
    "branch": "feat/issue-754-scoped-closure",
    "issue_id": "754",
    "work_item_id": "754",
    "recipe_run_id": "tree-01JZ7G7R5A7W1JYQXP3CBB2C2E",
    "tree_id": "tree-01JZ7G7R5A7W1JYQXP3CBB2C2E",
    "workstream_id": "ws-754",
    "process_started_at": "74293011",
    "recorded_at": "2026-06-12T04:03:24Z"
  }
}
```

Legacy state with missing `scope` or `process_scope` is valid for display. It is
not valid for notifications, closure, readiness, or terminal status decisions.

## Process Scope API

The multitask monitor validates process ownership through
`process_scope.rs`.

```rust
pub struct CurrentWorkflowScope {
    pub repository: String,
    pub repo_root: String,
    pub workdir: String,
    pub branch: String,
    pub issue_id: String,
    pub work_item_id: String,
    pub recipe_run_id: String,
    pub tree_id: String,
    pub workstream_id: String,
}

pub struct ProcessScope {
    pub pid: Option<u32>,
    pub repository: String,
    pub repo_root: String,
    pub workdir: String,
    pub branch: String,
    pub issue_id: String,
    pub work_item_id: String,
    pub recipe_run_id: String,
    pub tree_id: String,
    pub workstream_id: String,
    pub process_started_at: String,
    pub recorded_at: String,
}

pub struct ProcessSnapshot {
    pub pid: u32,
    pub alive: bool,
    pub workdir: String,
    pub process_started_at: String,
}

pub struct ProcessScopeConfig {
    pub max_age_seconds: i64,
}

pub enum ProcessScopeValidation {
    Valid,
    MissingScope,
    Dead,
    PidReused,
    TooOld,
    RepoMismatch,
    WorkdirMismatch,
    BranchMismatch,
    WorkstreamMismatch,
}

pub fn validate_process_scope(
    current: &CurrentWorkflowScope,
    persisted_workstream: &WorkstreamScope,
    persisted_process: &ProcessScope,
    snapshot: &ProcessSnapshot,
    config: &ProcessScopeConfig,
) -> ProcessScopeValidation;
```

All path comparisons use canonical paths when the paths exist. If canonical
resolution fails for a required path, validation returns the corresponding
mismatch instead of guessing.

## Process Validation Outcomes

Rust enum names are internal. CLI and JSON output serializes validation reasons
as snake_case.

| Rust outcome | Serialized reason | Authoritative | Meaning |
| --- | --- | --- |
| `Valid` | `valid` | Yes | Persisted state, current workflow scope, and runtime snapshot match. |
| `MissingScope` | `missing_scope` | No | Old state lacks required scope fields. |
| `Dead` | `dead` | No | PID is not running. |
| `PidReused` | `pid_reused` | No | PID exists, but runtime start metadata differs from the launch record. |
| `TooOld` | `too_old` | No | Process age exceeds `ProcessScopeConfig::max_age_seconds`. |
| `RepoMismatch` | `repo_mismatch` | No | Persisted repository or repo path differs from current workflow. |
| `WorkdirMismatch` | `workdir_mismatch` | No | Persisted workdir differs from current workflow workdir. |
| `BranchMismatch` | `branch_mismatch` | No | Persisted branch differs from current branch. |
| `WorkstreamMismatch` | `workstream_mismatch` | No | Recipe run, tree id, or workstream id differs. |

Only `valid` permits monitor notifications or closure behavior.

## Recipe Integration Contract

These bundled tools and recipes use scoped validation:

| Component | Contract |
| --- | --- |
| `workflow_publish_pr.sh` | Persists PR identity after creation and validates known PR identity before updating an existing PR. |
| `workflow_pr_ready.sh` | Refuses readiness checks until `workflow_pr_scope.sh` returns `ok: true`. |
| `workflow_final_status.sh` | Requires validated PR metadata before reporting terminal PR status. |
| `workflow-terminal-state.yaml` | Uses persisted PR URL or number, then scoped lookup; no recent-PR fallback. |
| `workflow-tdd.yaml` | Carries repository, head branch, base branch, tracking item, recipe run, and start-time scope through test and closure steps. |
| `quality-loop.yaml` | May survey broad PRs for quality reporting, but those surveys are not current-work identity. |
| multitask launcher | Captures workstream and process scope at process launch. |
| multitask orchestrator | Emits notifications and closure decisions only for serialized `valid` process scope. |

## Failure Semantics

Scoped closure fails closed:

- missing PR scope blocks publish/readiness/final-status current-work claims
- unrelated PRs are ignored even when newer or authored by the same account
- cross-repository and fork PRs are rejected by default
- multiple scoped PR candidates block until scope is made more specific
- stale process records are display-only
- PID reuse is rejected when runtime start metadata differs
- missing runtime start metadata is rejected as incomplete process scope
- exact-head readiness blocks when `head_sha` differs from PR head

Failure output names the invalid reason and the checked identifiers. It does
not persist secrets, full environment dumps, auth headers, agent prompts, or
full process command lines.

## Deterministic Checks

The regression suite covers these contracts:

```bash
cargo test -p amplihack-cli --test issue_754_scoped_monitor_contracts

amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh \
  scoped-pr-identity
```

Representative checks:

| Check | Expected result |
| --- | --- |
| newer unrelated PR exists for same author | scoped helper rejects it |
| zero matching PRs | helper exits non-zero with `no_scoped_pr` |
| two matching branch/base candidates | helper exits non-zero with `multiple_scoped_prs` |
| fork or cross-repository PR candidate | helper rejects it during scoped filtering |
| persisted legacy workstream lacks scope | monitor marks display-only and does not notify |
| persisted PID is dead | validation returns `dead` |
| PID exists with different start marker | validation returns `pid_reused` |
| process start metadata is missing | validation returns `missing_scope` |
| repo, workdir, branch, recipe, tracking item, or workstream differs | validation returns the named mismatch |

## Related

- [Scoped Workflow Closure](../concepts/scoped-workflow-closure.md)
- [How to Configure Scoped Workflow Closure](../howto/configure-scoped-workflow-closure.md)
- [Tutorial: Scoped Workflow Closure](../tutorials/scoped-workflow-closure.md)
