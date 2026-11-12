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
The marker tables live in `failure_markers.rs` beside it, except the
usage-limit markers, which live in `limit_patterns.rs` (#1618); the evidence
helpers are in `failure_evidence.rs`, and `failure_class.rs` holds the decision
and the marker writer.

| Class                 | Examples                                                                                  | Retried by code? |
| --------------------- | ----------------------------------------------------------------------------------------- | ---------------- |
| `transient_transport` | `API Error: 529`, 503, 502, 500, 429, `overloaded_error`, `rate_limit_error`, `ECONNRESET`, `socket hang up`, `request timed out` | **Yes**, bounded |
| `usage_limit`         | `session limit`, `5-hour limit`, `usage limit`, `weekly limit`, `spend limit`; see [Usage-limit markers](#usage-limit-markers) | No               |
| `environmental`       | 401/403, `invalid x-api-key`, `API Error: 404`, `404 Not Found`, `not_found_error`, `model_not_found`, `command not found`, `no space left on device` | No               |
| `work`                | `test result: FAILED`, `assertion failed`, `panicked at`, `error[E0308]`, `merge conflict`, a failed step terminating with exit `79` | No               |
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
- **A policy refusal requires the reserved terminating status `79`.** The
  guard's phrase, even in full, can be quoted from a descendant refusal the
  agent already handled inline. Captured text cannot set
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

The markers are `session limit`, `5-hour limit`, `usage limit`,
`usage_limit_reached`, `quota exceeded`, `monthly limit`, `weekly limit`,
`hit your opus limit`, `hit your sonnet limit`, `hit your fable limit` and
`spend limit`. Matching
ignores case. They are Claude Code's, and are kept in one
table per harness in `limit_patterns.rs`, together with the patterns that
read the reset time. The classifier, the auto-switch supervisor and the
usage-limit wait all read that table; no other source file holds a Claude
limit message. `failure_class::usage_limit_reset_hint` is a thin wrapper
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

- **`5-hour limit` is the session window's newer name (#2230).** Claude Code
  now reports the session window this way:

  ```text
  You've hit your 5-hour limit · resets 8:20pm (America/Los_Angeles)
  ```

  The classifier files it as `usage_limit` with signal `5-hour limit`, whether
  the line is in the step's stderr, is the last non-empty line of its stdout,
  or both. Its reset hint is `8:20pm (America/Los_Angeles)`. The curly
  apostrophe form (`You’ve`) reads the same way.
- **Per-model limits match only their stopping form.** `hit your opus limit`,
  `hit your sonnet limit` and `hit your fable limit` cover messages such as
  `You've hit your Opus limit · resets 8:20pm (America/Los_Angeles)`, which
  reports signal `hit your opus limit`. These are usage caps with a stated
  reset, so the run waits for the reset or switches login, as for a session
  limit. The names come from Claude Code 2.1.322, which maps its limit
  windows `seven_day_opus`, `seven_day_sonnet` and
  `seven_day_overage_included` to "Opus limit", "Sonnet limit" and "Fable
  limit". A bare `opus limit` is not a marker: Claude Code also prints those
  names in notices that do not stop work, such as
  `Fable limit reached · continuing on <model> …` or a model fallback such as
  `Opus limit reached · now using Sonnet`. Those stay `indeterminate`, so
  auto-drive does not stop as USAGE_LIMIT on them.
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
- **`fast limit`, `hourly limit` and a bare `hit your limit` are not
  markers.** Fast mode falls back to normal mode when its limit is reached and
  does not block the account. `hourly limit` is left out for the same reason as
  `daily limit`. `hit your limit` on its own is common in model prose and tool
  output, so it would file unrelated failures as `usage_limit`; the harness's
  own messages always name the limit.
- **`credit balance is too low` is not a usage-limit marker.** It is an
  environmental marker, so that message classifies `environmental`.

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
an unsuccessful step's terminating error header records the reserved exit code
`79`. The runner decodes its pinned subprocess error envelope, anchored at the
start of `error`, before diagnostics. It does not search captured stdout,
stderr or diagnostic text for refusal phrases or embedded exit-code references.
The class is then `work`, and the failure is never mechanically retried.

```text
Command failed (exit 79): <any diagnostics>                  -> true, work
amplihack copilot failed (exit 79)                            -> true, work
Command failed (exit 1): BLOCKED_TERMINAL orchestration_unavailable -> false
quoted guard phrase followed by Execution failed: 400        -> false, agent_api
```

`class` still reads the failed steps' diagnostics, run status and the last 40
lines of the runner's stderr. `structural_refusal` reads only the failed
steps' terminating status. Completed steps, captured transcripts, runner stderr
and run status cannot set it. The decision is stored before combining and
truncating diagnostic evidence to its 2,000-character bound. Eliding a failed
step's header, or even its entire diagnostic when later runner stderr displaces
it, cannot clear an exit-79 refusal or make it mechanically retryable.

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
- `crates/amplihack-cli/src/commands/recipe/run/limit_patterns.rs`
- `crates/amplihack-cli/src/commands/recipe/run/retry.rs`
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs`
- `crates/amplihack-cli/src/commands/recipe/run/tests_failure_class_1437.rs`
- `crates/amplihack-cli/tests/issue_1437_final_marker.rs`
- `crates/amplihack-utils/src/backoff.rs`
