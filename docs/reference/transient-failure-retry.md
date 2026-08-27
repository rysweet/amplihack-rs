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
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`
- `crates/amplihack-utils/src/backoff.rs`
