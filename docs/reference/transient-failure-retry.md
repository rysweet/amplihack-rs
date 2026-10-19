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
(`crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`, and
`tests_failure_class_1437.rs` for `agent_api` and `structural_refusal`).
The marker tables live in `failure_markers.rs` beside it, and the evidence
helpers in `failure_evidence.rs`; `failure_class.rs` holds the decision and
the marker writer.

| Class                 | Examples                                                                                  | Retried by code? |
| --------------------- | ----------------------------------------------------------------------------------------- | ---------------- |
| `transient_transport` | `API Error: 529`, 503, 502, 500, 429, `overloaded_error`, `rate_limit_error`, `ECONNRESET`, `socket hang up`, `request timed out` | **Yes**, bounded |
| `usage_limit`         | `usage limit`, `weekly limit`, `spend limit`; see [Usage-limit markers](#usage-limit-markers) | No               |
| `environmental`       | 401/403, `invalid x-api-key`, `API Error: 404`, `404 Not Found`, `not_found_error`, `model_not_found`, `command not found`, `no space left on device` | No               |
| `work`                | `test result: FAILED`, `assertion failed`, `panicked at`, `error[E0308]`, `merge conflict`, the guard's full `BLOCKED_TERMINAL orchestration_unavailable` phrase | No               |
| `agent_api`           | `Execution failed: 400`, `Execution failed: 404`, `The resource you requested was not found`, `API Error: 400`, `400 Bad Request`, `invalid_request_error`, `session not found`, `conversation not found` | No               |
| `indeterminate`       | nothing in the evidence identifies the failure                                              | No               |

Five rules matter more than the tables:

- **401 and 403 are not transient.** They are stable server answers. Retrying
  them burns the budget and can never succeed, so they are `environmental`.
- **An agent endpoint rejection is `agent_api`, not `indeterminate`** (issue
  #1437). `Execution failed: 400 The resource you requested was not found`
  names its own cause. It is not retried mechanically, because the identical
  request gets the identical rejection, and it is not a policy refusal: a
  supervising loop may start a fresh round.
- **A 404 that names an unknown model is `environmental`, not `agent_api`**
  (issue #1421). `API Error: 404`, `404 Not Found`, `not_found_error` and
  `model_not_found` are environmental markers, and environmental markers are
  checked before agent-API markers, so the same evidence never gets two
  classes depending on which phrase a provider used.
- **A policy refusal is the guard's whole phrase**, `BLOCKED_TERMINAL
  orchestration_unavailable`. Neither word on its own is a marker. Whether the
  refusal ended the run is published separately, as
  [`structural_refusal`](#the-structural_refusal-field).
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
  `work` → `environmental` → `usage_limit` → `agent_api` →
  `transient_transport` → `indeterminate`, and the first class with a match
  wins. A usage-limit marker inside a 429 envelope is still `usage_limit`, and
  `weekly limit` text cannot hide a real test failure.
- **`daily limit` is not a marker.** Provider and GitHub API daily request
  quotas use the same phrase. Filing those as `usage_limit` would stop retrying
  failures that clear on their own.
- **A bare `rate limit` is not a marker**, so a provider rate limit is never
  filed as `usage_limit`. `rate limit exceeded`, `rate_limit_error`, a plain
  `API Error: 429` and `429 Too Many Requests` stay `transient_transport`.
- **`credit balance is too low`** is also on the marker list, but the
  environmental check runs first, so that message classifies `environmental`.

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
amplihack.recipe.failure_class {"schema_version":1,"issue":1267,"class":"transient_transport","signal":"api error: 529","reasoning":"classified `transient_transport` — evidence contains \"api error: 529\"","action":"retry","attempt":1,"retryable":true,"structural_refusal":false,"failed_steps":["step-12-run-precommit"],"completed_steps":["workflow-prep","workflow-worktree"],"evidence":"..."}
```

`action` is one of `retry`, `terminal`, or `terminal_budget_exhausted`. The
issue noted that a dead run and a quiet run were indistinguishable from the log
alone; `action: "terminal"` / `"terminal_budget_exhausted"` is the unambiguous
marker that a run ended abnormally.

```sh
grep amplihack.recipe.failure_class run.log | jq -r '.class + " " + .action'
```

### The `structural_refusal` field

`structural_refusal` is a JSON boolean, never a string. It is `true` only when
the failed step's own evidence contains the guard's full phrase,
`BLOCKED_TERMINAL orchestration_unavailable`, matched without regard to case.
The class is then `work`. Neither word on its own sets it: `blocked_terminal`
and `orchestration_unavailable` appear in transcripts whenever a descendant
was refused and the agent then did what the refusal told it to do. Every other
failure, `agent_api` included, has `false`.

```text
BLOCKED_TERMINAL orchestration_unavailable: depth 4 of max 3   -> true,  work
blocked_terminal orchestration_unavailable                     -> true,  work
the step mentioned blocked_terminal                            -> false
Execution failed: 400 The resource you requested was not found -> false, agent_api
```

`structural_refusal` reads a narrower set of evidence than `class`. `class`
also reads the run status and the last 40 lines of the runner's stderr, and
that stderr tail can hold the transcripts of runs the runner started.
`structural_refusal` reads only the failed steps' own evidence: their `error`
and their captured stdout and stderr. The runner's other steps, its stderr
tail, its run status and earlier output cannot set the field. When the
evidence is longer than its 2,000-character bound and its start was elided,
the lines ahead of the first surviving failed-step header have no known
origin, so they do not count either.

When the runner produced no structured result (a spawn failure, unparseable
stdout, a runner that died before writing its JSON), the class comes from the
error chain, which already includes the stderr tail. That evidence is not a
failed step's own output, so on this path `structural_refusal` is always
`false`, whatever the error says.

The marker is serialised with `serde_json`. If serialisation fails the marker
is `{}`, which a reader treats as no refusal unless the child exited `79`.

A failed run writes the marker twice: when the failure is classified, and
again, unchanged, as the run's last line after the result table. The table
shows each step's output, and a multi-line output can put any text at the
start of a line, including a line that looks like a marker; the second copy
keeps the runner's own marker last.

A supervising loop reads this field from the **last** marker in a child's log
instead of searching the transcript. `autodrive_loop.sh` does so through
`terminal_refusal` in
[`autodrive_refusal.sh`](auto-drive-to-merge.md#autodrive_refusalsh), which
treats a child as refused only when it exited `79` or its last marker has
`"structural_refusal": true`:

```bash
. amplifier-bundle/tools/autodrive_refusal.sh
if terminal_refusal 1 run.log; then echo "terminal: $TERMINAL_WHY"; else echo "not terminal"; fi
# not terminal
child_failure_summary 1 run.log
# rc=1 class=agent_api retryable=false signal=execution failed: 400
```

The field was added without changing `schema_version`, the marker prefix or
any existing key; a consumer that does not read it is unaffected.

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
- `crates/amplihack-cli/src/commands/recipe/run/failure_evidence.rs`
- `crates/amplihack-cli/src/commands/recipe/run/failure_markers.rs`
- `crates/amplihack-cli/src/commands/recipe/run/retry.rs`
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class_1437.rs`
- `crates/amplihack-cli/tests/issue_1437_final_marker.rs`
- `crates/amplihack-utils/src/backoff.rs`
