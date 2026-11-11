---
title: Transient Failure Classification and Retry
description: Recipe failure classes, retry bounds, reset hints, and recovery ownership.
doc_type: reference
last_updated: 2026-11-11
---

# Transient Failure Classification and Retry — Reference

`amplihack recipe run` classifies failed runner results, preserves bounded
failure evidence, and automatically retries only transient transport failures.
Usage limits receive a separate `usage_limit` verdict with the supplied reset
hint. This reference covers classification, diagnostics, configuration, and
the boundary between transport retry and recovery.

## Contents

- [Classification](#classification)
- [Usage-limit markers](#usage-limit-markers)
- [Session-limit verdict and recovery ownership](#session-limit-verdict-and-recovery-ownership)
- [Recovery mechanisms](#recovery-mechanisms)
- [Durable recipe recovery](#durable-recipe-recovery)
- [Retry bounds](#retry-bounds)
- [Observability](#observability)
- [Where the judgement lives](#where-the-judgement-lives)
- [Source](#source)

## The split: mechanical facts vs. judgement

| Question                                              | Decided by                                          |
| ----------------------------------------------------- | --------------------------------------------------- |
| Was this an HTTP 529 / a reset connection?             | Code. `failure_class.rs` — a table of literal markers |
| Should the identical invocation be tried again?        | Code, bounded. `retry.rs`                            |
| Is this run still making progress?                     | **An agent.** `loop-health-evaluator.yaml` (#1337)   |
| Is this task genuinely impossible?                     | **An agent.** Same evaluator                         |

Transport classification is a mechanical fact about a mechanical fault, so code
does it. Whether continuing is *worthwhile* is a judgement, so code does not:
there is no "give up after N unproductive rounds" integer in this path. That
question is answered by looking at the evidence, which is what the loop-health
evaluator exists to do.

## Classification

`classify_failure_text` is a pure function: evidence text in, class out. It is
unit-tested in isolation
(`crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`).

| Class                 | Examples                                                                                  | Retried by code? |
| --------------------- | ----------------------------------------------------------------------------------------- | ---------------- |
| `transient_transport` | `API Error: 529`, 503, 502, 500, 429, `overloaded_error`, `rate_limit_error`, `ECONNRESET`, `socket hang up`, `request timed out` | **Yes**, bounded |
| `usage_limit`         | `usage limit`, `weekly limit`, `spend limit`; see [Usage-limit markers](#usage-limit-markers) | No               |
| `environmental`       | 401/403, `invalid x-api-key`, `command not found`, `no space left on device`                | No               |
| `work`                | `test result: FAILED`, `assertion failed`, `panicked at`, `error[E0308]`, `merge conflict`, a `BLOCKED_TERMINAL` policy refusal | No               |
| `indeterminate`       | nothing in the evidence identifies the failure                                              | No               |

Two rules matter more than the tables:

- **401 and 403 are not transient.** They are stable server answers. Retrying
  them burns the budget and can never succeed, so they are `environmental`.
- **Ambiguity resolves toward not retrying.** When the same evidence carries a
  work marker *and* a transient marker, the verdict is `work`. A missed retry
  costs one run; an unbounded retry of an impossible step costs the whole budget
  and hides the real failure.

Evidence is drawn from the tail of the **failed** steps — their `error`, their
captured stdout/stderr — plus the runner's own stderr tail. In the reported
incident the step's `error` said only `agent step failed: amplihack claude
failed (exit 1)`; the 529 was in the captured stdout, which is why both are read.
Completed-step output does not contribute to classification. Each captured
output source contributes at most its last 40 lines; combined evidence longer
than 2,000 characters retains its last 2,000 characters with an elision notice.
The verdict is a bounded diagnostic, not a transcript archive.

### Usage-limit markers

The markers are `session limit`, `usage limit`, `usage_limit_reached`,
`quota exceeded`, `monthly limit`, `weekly limit` and `spend limit`.
They are Claude Code's, and are kept in one
table per harness in `limit_patterns.rs`, together with the patterns that
read the reset time. The classifier, the auto-switch supervisor and the
usage-limit wait all read that table. Test fixtures supply synthetic limit
messages separately. `failure_class::usage_limit_reset_hint` is a thin wrapper
over the table's reset reader, and the classifier's precedence is
unchanged: work, then repo-selection and environmental, then usage limit,
then transient. To support another harness, add a `Harness` variant and
its table there.

- **A usage limit is waited out before the runner sees it.** A Claude agent
  step that hits a usage limit inside a recipe run first waits for the reset,
  or for the login to change, and runs once more, inside the step's own
  `amplihack claude -p` launch. The runner classifies only that last
  attempt, so `usage_limit` here means the step still failed after the wait,
  or the wait was turned off. See the
  [usage-limit wait](./profile-command.md#usage-limit-wait).

- **Why `spend limit` is listed.** `monthly limit` alone does not match Claude
  Code's `monthly spend limit` message, so both are markers (#1546).
- **Reset hint.** The reset time the message states is quoted in the verdict:
  `You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)`
  gives `it resets Oct 8, 12am (America/Los_Angeles)` in `reasoning`.
- **Two markers in one message.** The `signal` is the marker that appears
  earliest in the text, not the one listed first. This message:

  ```text
  You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 10, ...
  ```

  reports `spend limit`, and its reset hint is `Oct 10, ...`, the text after the
  first `resets `.
- **Spend limit on a credit-backed profile.** The classifier files it as
  `usage_limit` like any other limit. Separately, when the
  [login rotator](./profile-command.md#rotate-overflow) reads this message in
  `claude -p /usage` for a profile marked as overflow, it blocks that profile
  until the 1st of the next month and moves work elsewhere; see
  [Use usage credits when every plan login is exhausted](../howto/claude-profile-rotation.md#use-usage-credits-when-every-plan-login-is-exhausted).
  The retry rules here do not change, and neither the classifier nor the
  usage-limit wait ever writes an overflow block: only the rotator's probe
  does (see [Overflow spend-limit block](./profile-command.md#overflow-spend-limit-block)).
- **Checked after work and environmental, before transient.** The order is
  `work` → `environmental` → `usage_limit` → `transient_transport`, and the
  first class with a match wins. A usage-limit marker inside a 429 envelope is
  still `usage_limit`, and `weekly limit` text cannot hide a real test failure.
- **`daily limit` is not a marker.** Provider and GitHub API daily request
  quotas use the same phrase. Filing those as `usage_limit` would stop retrying
  failures that clear on their own.
- **A bare `rate limit` is not a marker**, so a provider rate limit is never
  filed as `usage_limit`. `rate limit exceeded`, `rate_limit_error`, a plain
  `API Error: 429` and `429 Too Many Requests` stay `transient_transport`.
- **`credit balance is too low`** is an environmental marker. That check runs
  before usage-limit matching, so the message classifies `environmental`.

### Session-limit verdict and recovery ownership

For this reported failure:

```text
Agent terminated early due to an API error: You've hit your session limit · resets 9:20am (UTC)
```

the classification is `usage_limit`, the deciding signal is `session limit`,
and the verdict's `reasoning` retains the complete supplied reset hint,
`9:20am (UTC)`. The hint is quoted from the failure evidence; it is not a
calculated reset time or a scheduling instruction. A recognized usage limit
without a reset hint still receives `usage_limit`.

The structured run result's `failure_classification` object and the
JSON object after the `amplihack.recipe.failure_class ` stderr prefix contain
the same verdict. With `--format json`, stdout reports `success: false` and
the command exits with code `1`. For a usage limit on the first runner attempt:

| Verdict field | Value or meaning |
| --- | --- |
| `class` | `"usage_limit"` |
| `signal` | `"session limit"` for the message above |
| `retryable` | `false` |
| `attempt` | `1` |
| `action` | `"terminal"` |
| `reasoning` | Names the class and signal and quotes `9:20am (UTC)` |
| `evidence` | Retains the reported message above within the evidence bounds |
| `completed_steps` | IDs of steps reported as completed, in result order |
| `failed_steps` | IDs of steps reported as unsuccessful, in result order |

For a result with a completed `setup` step and a failed `limited-agent` step,
the arrays are `["setup"]` and `["limited-agent"]`. These identities describe
the reported run. A completed-step ID alone does not make that step eligible
for replay on a later invocation. The implemented [durable recipe recovery](#durable-recipe-recovery)
requires separately validated records.

Here **terminal** means the recipe transport mechanism stops automatic retries.
If the first attempt reports this limit, the runner executes exactly once,
with one classification marker. Usage limits do not enter the mechanical
transport retry loop, even when wrapped in an HTTP 429 error. No additional
configuration is needed for classification, and the [transport retry settings](#retry-bounds)
do not enable retries for this class.

Work failures, including policy refusals, and environmental failures, including
authentication errors, retain higher precedence. Bare HTTP 429 errors retain
their transport classification. An unrelated failure without a recognized
marker remains `indeterminate`.

### Recovery mechanisms

`retryable: false` and `action: "terminal"` describe the transport retry
decision for this invocation. A usage limit can still be temporary. The
same-process wait and durable recipe recovery are implemented, with separate
contracts described below. Upstream session lifecycle remains distinct.

| Mechanism | Trigger and behavior | State retained or reused |
| --- | --- | --- |
| Transport retry | Only `transient_transport`; repeats the invocation within the [retry bounds](#retry-bounds) | Bounded failure diagnostics and the run's existing scratch space |
| Usage-limit wait | A recognized Claude limit inside an `amplihack claude -p` step; waits and retries within that launch when enabled | The active step's process-local execution; see [usage-limit wait](./profile-command.md#usage-limit-wait) |
| Durable recipe recovery | A later compatible launch after the latest matching attempt durably stopped on a usage limit and its owner is no longer active | Reuses validated, finalized step results and declared context and artifacts under the [recovery contract](./recipe-recovery.md) |
| Claude Code internal session lifecycle (upstream ownership) | The runtime terminates or resumes one of its own subagents | Transcript retention, reasoning context, and deliberate child cleanup belong to Claude Code; this classifier does not implement or validate them |

Classification requires no configuration. The implemented same-process wait
has its own
[`recipe.usage_limit_wait` and `recipe.usage_limit_max_wait_secs`
settings](./user-config.md#recipeusage_limit_wait), overridden by
`AMPLIHACK_USAGE_LIMIT_WAIT` and `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS`.
Disabling that wait allows the failure to reach classification directly;
increasing the transport retry budget does not make `usage_limit` retryable.

### Durable recipe recovery

Current main supplies repository-owned durable recipe recovery through the
pinned Rust runner. The CLI records the trusted `usage_limit` outcome without
adding a transport retry or reset timer, or changing the existing wait policy.
Replay requires compatible work identity, execution definitions, inputs,
persisted results, and resources. A later ordinary failure or success prevents
falling back to an older usage-limit attempt. An active predecessor cannot be
taken over. Completed-step IDs in the verdict remain diagnostic metadata,
not recovery checkpoints.

Every provider requires `durable_recovery_v1`; Codex additionally requires
`codex_exec`. The CLI durably admits a top-level launch before execution and
passes its recovery descriptor; delegated runners attach through the inherited
recovery channel. A capability string alone does not establish admission or
permit replay. See [runner validation](./recipe-runner-validation.md#recovery-capability-enforcement),
[runner execution](./rust-runner-execution.md), and the full
[recovery reference](./recipe-recovery.md).

The issue #1390 regression fixture uses the pinned runner's descriptor and
private store to reach classification through this boundary. It verifies a
finalized durable `usage_limit` outcome. It supplies controlled step results;
it does not exercise replay, real agent scheduling, or a session lifecycle.
Those recipe recovery contracts have their own `issue_1741_*` scenarios.

Classification and durable recipe records do not preserve a
Claude Code internal subagent's transcript or recover its in-flight children. The
[maintainer's scope clarification](https://github.com/rysweet/amplihack-rs/issues/1390#issuecomment-5449708778)
assigns that lifecycle to the upstream runtime. The upstream requirements are
to preserve reasoning context across involuntary faults, flush transcripts before
termination, and deliberately reap or record in-flight children. Durable
worktree changes alone do not preserve the reasoning behind them. These are
upstream requirements, not guarantees implemented by this change.

## Retry bounds

Only `transient_transport` is retried, with exponential backoff and equal jitter
(`amplihack_utils::backoff::BackoffPolicy`, shared with the Azure remote
pipeline). Two bounds, whichever is reached first:

| Setting        | Default | Override                                   |
| -------------- | ------- | ------------------------------------------ |
| Total attempts | 3       | `AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS` (capped at 10; `1` disables retry) |
| Backoff wait   | 300s    | `AMPLIHACK_RECIPE_TRANSIENT_BUDGET_SECS`   |
| First delay    | 10s, doubling | —                                     |

The budget counts time spent **waiting on backoff**, not the age of the run.
That distinction is the whole point: the fault this exists for arrives hours
into a long workstream, so a budget measured from the start would already be
spent when the first 529 lands and the retry would never happen at all. What is
bounded is how long the runner sits idle hoping an endpoint recovers — not how
long the work itself takes.

These are a backstop on a mechanical retry against a dead endpoint, not a
judgement about progress. Exhausting either produces a terminal error that names
the class, the deciding signal, the attempts made, the time spent waiting, and
the phases that completed.

Between attempts the caller checkout's git state is repaired (issue #964), so a
retry never runs against state the previous attempt corrupted.

## Observability

Every classification decision writes a greppable single-line marker to stderr:

```
amplihack.recipe.failure_class {"schema_version":1,"issue":1267,"class":"transient_transport","signal":"api error: 529","reasoning":"classified `transient_transport` — evidence contains \"api error: 529\"","action":"retry","attempt":1,"retryable":true,"failed_steps":["step-12-run-precommit"],"completed_steps":["workflow-prep","workflow-worktree"],"evidence":"..."}
```

`action` is one of `retry`, `terminal`, or `terminal_budget_exhausted`. The
issue noted that a dead run and a quiet run were indistinguishable from the log
alone; `action: "terminal"` / `"terminal_budget_exhausted"` is the unambiguous
marker that a run ended abnormally.

Strip the marker prefix before parsing its JSON payload:

```sh
sed -n 's/^amplihack\.recipe\.failure_class //p' run.log \
  | jq -r '.class + " " + .action'
# Output: transient_transport retry
```

The same object is attached to the structured run result under
`failure_classification`, so an agentic step consumes it as data rather than
scraping prose:

```sh
amplihack recipe run default-workflow --format json \
  | jq '.failure_classification | {class, action, completed_steps}'
```

## Where the judgement lives

When the class is not `transient_transport`, or the transport budget is spent,
the runner stops and surfaces the classification. It does not decide whether the
work should continue. That decision belongs to
[`loop-health-evaluator.yaml`](../../amplifier-bundle/recipes/loop-health-evaluator.yaml),
which reads the accumulated evidence — what the last round actually produced,
which findings recur, whether test and CI signals moved — and returns
`CONTINUE` / `DONE` / `STUCK`. Feeding it an honest classification instead of an
undifferentiated `exit 1` is the point of this change.

## Source

- [Issue #1390 validation and delivery evidence](../bugfixes/bugfix-1390-validation-status.md) — current verification, limitations, and historical results.
- `crates/amplihack-cli/src/commands/recipe/run/failure_class.rs`
- `crates/amplihack-cli/src/commands/recipe/run/limit_patterns.rs`
- `crates/amplihack-cli/src/commands/recipe/run/retry.rs`
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`
- `crates/amplihack-utils/src/backoff.rs`
