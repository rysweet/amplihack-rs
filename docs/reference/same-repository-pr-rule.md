---
title: Same-Repository PR Rule
description: How amplihack recipes and tools decide that a pull request comes from the same repository, and what each reader does when it does not.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Same-Repository PR Rule

> [Home](../index.md) > Reference > Same-Repository PR Rule

Several recipes and tools act on a pull request only when it comes from a
branch in the same repository, not from a fork. They all decide this the same
way: a PR is same-repository only when GitHub's `isCrossRepository` field is
the JSON boolean `false`. Any other value, or no readable value, is treated as
cross-repository and refused.

## Contents

- [The rule](#the-rule)
- [Values and outcomes](#values-and-outcomes)
- [Readers](#readers)
- [REST fallback](#rest-fallback)
- [Files that are not readers](#files-that-are-not-readers)
- [Checking a PR by hand](#checking-a-pr-by-hand)
- [Tests](#tests)

## The rule

Every reader applies this jq test to the parsed `gh` output:

```sh
jq -e '.isCrossRepository == false'
```

The comparison runs inside jq on the typed value. Readers do not convert the
field to a string first (`jq -r`, `tostring`), do not supply a default
(`.isCrossRepository // false`), and do not compare strings in bash. Each of
those would let a missing or `null` field pass as same-repository.

## Values and outcomes

| `gh` output | `jq -e` exit status | Outcome |
| --- | --- | --- |
| `{"isCrossRepository":false}` | 0 | Same-repository; accepted |
| `{"isCrossRepository":true}` | 1 | Refused |
| `{}` (field missing) | 1 | Refused |
| `{"isCrossRepository":null}` | 1 | Refused |
| `{"isCrossRepository":"false"}` (string) | 1 | Refused |
| Empty output | 4 | Refused |
| JSON `null` | 1 | Refused |
| Invalid JSON, or a JSON array or string | 5 | Refused |
| `gh` exits non-zero with no output | 4 | Refused |

In a pipeline such as `gh pr view ... | jq -e ...`, jq still runs when `gh`
fails; it reads empty input and exits 4. Readers that call `gh` through
`gh_with_retry`, such as `workflow_pr_ready.sh`, return on the `gh` failure
before jq runs. Either way the PR is refused.

`isCrossRepository` cannot change after a PR is opened, so one read at each
decision point is enough.

## Readers

| Reader | Where | Effect of a refusal |
| --- | --- | --- |
| `light-eligibility` step (on `build_result.is_cross_repository`, read by `light-resolve-pr`) | `amplifier-bundle/recipes/auto-drive-light.yaml` | The step stops with `light-cross-repo` (see below). |
| PR facts in `workflow-terminal-state` | `amplifier-bundle/recipes/workflow-terminal-state.yaml` | The PR is not accepted as the run's same-repository PR; the run takes the existing cross-repository failure branch and is not reported as a success. |
| `autodrive_open_pr_for_run_branch` | `amplifier-bundle/tools/autodrive_run_pr.sh` | The PR is not selected; no PR number is printed. |
| Branch PR lookup | `amplifier-bundle/tools/workflow_final_status.sh` | The PR is not selected as the branch's PR. |
| Primary-key pass (the discovery `select` filter) | `amplifier-bundle/tools/workflow_pr_scope.sh` | The PR is not selected as the scoped PR. |
| Claim check | `amplifier-bundle/tools/workflow_issue_claim_check.sh` | The PR is not classed as ours (`ok-own-pr`); it counts as another claim, so the check fails closed. |
| Readiness | `amplifier-bundle/tools/workflow_pr_ready.sh` | The check reports `refusing to mutate PR` and exits non-zero; `gh pr ready` is never called. |

When `gh` returns empty or invalid output, a selecting reader takes its
existing failure or "no PR" path. No reader reports such a PR as the
same-repository PR.

### light-eligibility

`light-eligibility` makes no `gh` call: #2329 keeps platform reads in agent
steps. The `light-resolve-pr` agent step reads the PR and returns the observed
`isCrossRepository` as `build_result.is_cross_repository`, a JSON boolean.
Immediately after it validates the PR number, head and base from the build
result, `light-eligibility` checks that value:

```sh
printf '%s' "${RECIPE_VAR_build_result:-}" | jq -e '.is_cross_repository == false' >/dev/null 2>&1 \
  || die "light-cross-repo: PR #$PR is not a same-repository PR (build_result.is_cross_repository is not exactly false)"
```

- The check runs before the step creates private state or reuses a cached
  `light-eligibility.json` or `escalation.json`. A cached `light` decision
  cannot bypass it.
- `true`, a missing field, `null`, the string `"false"` and any other
  non-boolean are refused, the same table as the other readers apply to
  `isCrossRepository`.
- A refusal is a hard stop with exit status 1. It is never an `escalate`
  decision, because escalation would still hand the PR to
  `auto-drive-to-merge`.
- A refusal writes nothing. The message names only the validated PR number.
- If the agent cannot read the PR, it returns no `is_cross_repository`, and
  the light drive is refused.
- Before reading any field, the step checks that `build_result` is exactly
  one JSON object, so a second object cannot override a refused value.

#### Remaining risk

The check is only as reliable as the value the agent reports. If
`light-resolve-pr` wrongly reports `false` for a fork PR, through a mistake or
through text planted in the PR, the light drive builds and tests the fork's
code on this host. `workflow_pr_ready.sh` still reads `isCrossRepository` from
GitHub before any write, so the PR cannot be changed or merged. Reading the
field from GitHub in `light-eligibility`, as #2538 first proposed, would need
an exception to the #2329 rule; that decision is tracked in #2615.

The `light-resolve-pr` prompt also tells the agent to refuse a
cross-repository PR, or one whose `isCrossRepository` is missing or `null`.
See [Auto-Drive-Light Recipe](auto-drive-light.md#same-repository-check).

### workflow-terminal-state

The PR facts line encodes field 9 as
`(if .isCrossRepository == false then "false" else "true" end)`, and the shell
default for that field is `true`. Later checks accept only the exact token
`false`. All four places change together; leaving one on the old form would
reopen the gap. See
[Workflow Terminal-State Provider Safety](workflow-terminal-state-provider-safety.md).

### workflow_pr_ready.sh

```sh
if printf '%s' "$pr_json" | jq -e '.isCrossRepository == false' >/dev/null 2>&1; then
  pr_cross_repo=false
else
  pr_cross_repo=true
fi
```

The later guard refuses whenever `pr_cross_repo` is not `false`. The refusal
message does not echo the raw field value.

## REST fallback

When GraphQL is rate-limited, `gh_pr_exists_rest` and `gh_pr_view_rest` in
`amplifier-bundle/tools/workflow_gh_retry.sh` build `isCrossRepository` from
the REST response:

```jq
(.head.repo.full_name // "") as $h
| (.base.repo.full_name // "") as $b
| (($h | type) != "string" or $h == "" or $h != $b)
```

The fallback emits `false` only when the head name is a non-empty string equal
to the base name. A deleted fork (`head.repo` is `null`), a missing or
non-string name, or any mismatch gives `true`.

| REST response | `isCrossRepository` |
| --- | --- |
| Head and base `full_name` equal | `false` |
| Head and base `full_name` differ | `true` |
| `head.repo` is `null` | `true` |
| Both `full_name` values missing | `true` |
| Head `full_name` is not a string | `true` |

See [Workflow gh rate-limit tolerance](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md)
for when the fallback is used.

## Files that are not readers

These files request `isCrossRepository` but leave the decision to a reader
listed above, so they need no change:

- `amplifier-bundle/tools/workflow_publish_pr.sh` passes the PR to the
  terminal-state proof.
- `amplifier-bundle/tools/workflow_terminal_evidence.sh` records the field as
  evidence only.
- `amplifier-bundle/recipes/workflow-tdd.yaml` tells the agent to call
  `workflow_pr_scope.sh` with `isCrossRepository=false`.

These files already decided with `== false` before the rule was written down:

- `amplifier-bundle/tools/workflow_finalization_pr_signal.sh`
- `amplifier-bundle/tools/autodrive_pr_worktree.sh`

No file in `amplifier-bundle`, `crates` or `tests` uses the
`isCrossRepository //` default form. This search prints nothing:

```sh
git grep -nE 'isCrossRepository[[:space:]]*//' -- amplifier-bundle crates tests
```

## Checking a PR by hand

Run the same test the readers run. For PR 2557 in this repository:

```sh
gh pr view 2557 --repo rysweet/amplihack-rs --json isCrossRepository \
  | jq -e '.isCrossRepository == false' >/dev/null && echo same-repository || echo refused
```

Output for a PR opened from a branch of `rysweet/amplihack-rs`:

```
same-repository
```

The exit status of `jq -e` alone shows why a value is refused:

```sh
printf '%s' '{"isCrossRepository":null}' | jq -e '.isCrossRepository == false' >/dev/null; echo $?
# Output: 1
printf '%s' '' | jq -e '.isCrossRepository == false' >/dev/null; echo $?
# Output: 4
printf '%s' '[]' | jq -e '.isCrossRepository == false' >/dev/null 2>&1; echo $?
# Output: 5
```

## Tests

Each reader has behaviour tests that put a fake `gh` first on `PATH` and run
the real step or script. Unless the table below says otherwise, the six refused cases in
[Values and outcomes](#values-and-outcomes) (`true`, missing, `null`, the
string `"false"`, empty output, invalid JSON) are refused, and `false` is
accepted.

| Test | Covers |
| --- | --- |
| `crates/amplihack-cli/tests/issue_2538_cross_repo_refusal.rs` | `light-eligibility` on `build_result.is_cross_repository`, including a cached `light` decision, a cached escalation, a `build_result` holding two objects and a check that the step makes no `gh` call; `autodrive_run_pr.sh` |
| `tests/integration/issue_2538_cross_repo_readers.rs` | `workflow_pr_scope.sh` discovery, `workflow-terminal-state` field 9, `workflow_pr_ready.sh` (which must never call `gh pr ready` on a refusal), and the REST fallback in `workflow_gh_retry.sh` |
| `tests/issue_1596_final_status_branch_pr_lookup.sh` | `workflow_final_status.sh` |
| `tests/issue_1448_review_is_not_a_duplicate.sh` | `workflow_issue_claim_check.sh`: missing, `null` and the string `"false"` (with `true` covered by the existing fork cases). Empty or invalid output already makes the check report `unknown` (`claim-filter-unreadable`), never `ok-own-pr` |

Run the Rust tests with:

```sh
cargo test -p amplihack-cli --test cli_integration issue_2538_cross_repo_refusal::
cargo test -p amplihack --test integration issue_2538_cross_repo_readers::
```

The tests use a temporary `HOME`, a private `TMPDIR`, no tokens and no
network.

## Related

- [Auto-Drive-Light Recipe](auto-drive-light.md) - where `light-cross-repo` stops a light drive
- [Scoped Workflow Closure](scoped-workflow-closure.md) - the primary-key pass that rejects fork PRs
- [Default Workflow Agentic Finalization](default-workflow-agentic-finalization.md) - the live PR state check that requires `isCrossRepository=false`
