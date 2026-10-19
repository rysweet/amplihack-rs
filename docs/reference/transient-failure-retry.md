# Transient Failure Classification and Retry — Reference

_Issue [#1267](https://github.com/rysweet/amplihack-rs/issues/1267)._

`amplihack recipe run` used to treat every non-zero exit from `recipe-runner-rs`
as terminal. A single `API Error: 529 Overloaded` — a server-side blip the error
text itself calls "usually temporary" — unwound a six-hour workstream and
discarded five completed phases.

This page describes what the runner does now, and, more importantly, what it
deliberately does **not** decide for itself.

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
| `usage_limit`         | `usage limit`, `weekly limit`, `spend limit`; see [Usage-limit markers](#usage-limit-markers) | Once, after a login swap, for login limits only; see [Retry after rotating the login](#retry-after-rotating-the-login) |
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

### Usage-limit markers

The markers are `session limit`, `usage limit`, `usage_limit_reached`,
`quota exceeded`, `monthly limit`, `weekly limit`, `spend limit` and
`credit balance is too low`.

- **Why `spend limit` is listed.** `monthly limit` alone does not match Claude
  Code's `monthly spend limit` message, so both are markers (#1546).
- **Reset hint.** The reset time the message states is quoted in the verdict:
  `You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)`
  gives `it resets Oct 8, 12am (America/Los_Angeles)`.
- **Two markers in one message.** The `signal` is the marker that appears
  earliest in the text, not the one listed first. This message:

  ```text
  You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 10, ...
  ```

  reports `spend limit`, and its reset hint is `Oct 10, ...`, the text after the
  first `resets `.
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
- **`credit balance is too low`** is also on the marker list, but the
  environmental check runs first, so that message classifies `environmental`.

## Retry bounds

Only `transient_transport` is retried by this loop (a login-limit
`usage_limit` gets the separate, single
[retry after rotating the login](#retry-after-rotating-the-login)), with exponential backoff and equal jitter
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

## Retry after rotating the login

_Issue [#1608](https://github.com/rysweet/amplihack-rs/issues/1608)._

A `usage_limit` failure is not retried by the transport retry: the same login
would fail the same way. When it is a login limit and
[rotation](profile-command.md#rotate-on-a-usage-limit) is enabled, the
runner swaps `~/.claude` to a profile with capacity and runs the recipe once
more.

### Which signals rotate

| `signal` | Rotates |
| -------- | ------- |
| `session limit`, `weekly limit`, `usage limit`, `usage_limit_reached` | Yes |
| `spend limit`, `monthly limit`, `credit balance is too low`, `quota exceeded` | No. These are billing or API quotas that another login does not clear; the run stays terminal and nothing is swapped. |

### Turning it on

| `AMPLIHACK_ROTATE_ON_LIMIT` | Behaviour |
| --------------------------- | --------- |
| `1` | Rotate on every rotating signal. |
| `0` | Never rotate. Output is the same as before this feature. |
| unset or empty | Rotate only while the background rotator (`amplihack profile rotate start`) is running. |
| anything else | One warning on stderr, then as if unset. |

On Windows and other platforms without Unix file semantics nothing is
rotated, whatever the value, and stderr is what it was before this feature.

The variable is read from the environment of the `amplihack recipe run`
process. A recipe context key cannot set it: `--set amplihack_rotate_on_limit=1`
is refused like every other `AMPLIHACK_` key. The rotation settings come from
the `AMPLIHACK_ROTATE_*` variables in the same environment and their
defaults, never from the flags given to `rotate start`.
`AMPLIHACK_ROTATE_MIN_HEADROOM` and `AMPLIHACK_ROTATE_MARGIN` decide which
profiles can take over, and `AMPLIHACK_ROTATE_FAST_AT` decides whether the
live login is near enough to its limit for a swap. An invalid value in any
of the [rotate setting variables](profile-command.md#rotate-settings) means
no swap, with reason `invalid rotate settings: <error>`.

### What happens

1. The attempt fails and is classified `usage_limit` with a rotating signal.
2. The runner repairs the caller checkout's git state, as for every failed
   attempt, then runs the
   [on-limit rotate cycle](profile-command.md#rotate-on-a-usage-limit). It
   waits at most 30 seconds for the state lock.
3. If the cycle swapped, or another run or the rotator already swapped after
   this attempt started, the runner writes the marker with
   `action: "retry_after_rotate"` and a `WARNING` line, then runs the recipe
   again with a fresh transport retry budget.
4. Otherwise it writes the `no_swap` line and the usual terminal marker and
   `ERROR` line, and stops.

At most one rotate-retry happens per `recipe run`, whatever the transport
retry settings are. A second usage-limit failure is terminal. Attempt numbers
continue across both runs: if the first run used attempt 1, the retried run
starts at attempt 2, in the markers and in the `WARNING` lines alike.

When five runs fail on the same limit within seconds, the first to take the
lock swaps. The others find a `swapped_at` at or after the start of their
failed attempt, do not probe or swap again, and retry on the new login.

### stderr

Exactly one `amplihack.recipe.failure_class` marker is written per failed
attempt. A swap gives:

```
amplihack.recipe.failure_class {"schema_version":1,"issue":1267,"class":"usage_limit","signal":"session limit","reasoning":"...","action":"retry_after_rotate","attempt":1,"retryable":false,...}
WARNING: recipe run attempt 1 hit a usage limit — rotated work->personal; retrying once. [issue #1608]
```

`retryable` stays `false`. It says whether the transport retry would run
the attempt again, and it never does for a usage limit; the second run comes
from the rotation, which `action` records.

When another run had already swapped, the `WARNING` line reads
`... hit a usage limit — the login was already rotated to personal; retrying once. [issue #1608]`.

When rotation is enabled but nothing was swapped, one line comes before the
usual terminal marker and `ERROR` line, which are unchanged:

```
amplihack.recipe.rotate_on_limit outcome=no_swap reason=no profile has capacity
amplihack.recipe.failure_class {..."class":"usage_limit",...,"action":"terminal","attempt":1,...}
ERROR: recipe run terminated on attempt 1 — classified `usage_limit` ... [issue #1267]
```

The reasons are listed in
[Rotate on a usage limit](profile-command.md#rotate-on-a-usage-limit).
Control characters are removed from the reason, so the line is always one
line. With rotation disabled, or for a non-rotating signal, stderr is
exactly what it was before this feature.

Nothing from rotation is written to stdout; with `--format json`, stdout
holds only the result.

### Result JSON

When a rotate-retry happened, the result carries a `rotate_retry` object
beside `failure_classification`, whether the retried run succeeded or failed:

```json
{
  "rotate_retry": {
    "schema_version": 1,
    "issue": 1608,
    "from": "work",
    "to": "personal",
    "outcome": "swapped",
    "signal": "session limit",
    "failed_attempt": 1
  }
}
```

| Field | Value |
| ----- | ----- |
| `from` | The profile this run swapped away from. `null` when `outcome` is `already_swapped`, because another run made that swap. |
| `to` | The profile that is live for the retry. |
| `outcome` | `swapped` (this run swapped) or `already_swapped` (another run or the rotator had). |
| `signal` | The usage-limit marker that matched. |
| `failed_attempt` | The attempt number that failed on the limit. |

If the retried run fails too, `failure_classification` describes that
failure with `action: "terminal"`. `rotate_retry` is absent when the cycle
returned `no_swap` or rotation is disabled; an `already_swapped` outcome
carries it. If the retried run cannot produce a result at
all (the runner fails to start, for example), there is no result JSON and the
`WARNING` line is the only record of the rotation.

```sh
amplihack recipe run default-workflow --format json \
  | jq '{retry: .rotate_retry, class: .failure_classification.class}'
```

## Observability

Every classification decision writes a greppable single-line marker to stderr:

```
amplihack.recipe.failure_class {"schema_version":1,"issue":1267,"class":"transient_transport","signal":"api error: 529","reasoning":"classified `transient_transport` — evidence contains \"api error: 529\"","action":"retry","attempt":1,"retryable":true,"failed_steps":["step-12-run-precommit"],"completed_steps":["workflow-prep","workflow-worktree"],"evidence":"..."}
```

`action` is one of `retry`, `retry_after_rotate`, `terminal`, or
`terminal_budget_exhausted`. The
issue noted that a dead run and a quiet run were indistinguishable from the log
alone; `action: "terminal"` / `"terminal_budget_exhausted"` is the unambiguous
marker that a run ended abnormally.

```sh
grep amplihack.recipe.failure_class run.log | jq -r '.class + " " + .action'
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

- `crates/amplihack-cli/src/commands/recipe/run/failure_class.rs`
- `crates/amplihack-cli/src/commands/recipe/run/retry.rs`
- `crates/amplihack-cli/src/commands/recipe/run/rotate_retry.rs`
- `crates/amplihack-cli/src/commands/profile/rotate/on_limit.rs`
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`
- `crates/amplihack-utils/src/backoff.rs`
