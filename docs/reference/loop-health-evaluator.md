---
title: Loop-Health Evaluator Reference
last_updated: 2026-08-27
review_schedule: quarterly
owner: workflow-team
---

# Loop-Health Evaluator Reference

`loop-health-evaluator` is a reusable recipe brick that answers one question
after each round of an iterative loop: **is this loop still doing useful
work?** It looks at the accumulated evidence of the loop and emits one
structured verdict — `CONTINUE`, `DONE`, or `STUCK`.

It is deliberately **not** an iteration counter. See
[Why not a numeric iteration cap](#why-not-a-numeric-iteration-cap).

## Contents

- [Why this exists](#why-this-exists)
- [Why not a numeric iteration cap](#why-not-a-numeric-iteration-cap)
- [The verdict contract](#the-verdict-contract)
- [`amplihack orch helper normalise-loop-verdict`](#amplihack-orch-helper-normalise-loop-verdict)
- [Evidence the evaluator receives](#evidence-the-evaluator-receives)
- [Context inputs](#context-inputs)
- [Outputs](#outputs)
- [Using it from a recipe](#using-it-from-a-recipe)
- [Exit code 79 is terminal](#exit-code-79-is-terminal)
- [Fail-safe guarantees](#fail-safe-guarantees)
- [Worked example — the 2h47m run](#worked-example--the-2h47m-run)
- [Tests](#tests)
- [Related references](#related-references)

## Why this exists

A measured default-workflow run spent **2h47m and produced zero commits**.
Inside that run:

- seven consecutive steps each reported exactly `10m 0s` with no artifacts
  produced — a ceiling being hit seven times, not seven units of work;
- one child returned `BLOCKED_TERMINAL` at depth 4/3 with exit code `79`;
- the run itself said *"The review workflow is still running; I'm waiting for
  its structured findings."*

Every one of those was a visible signal in the run log, and nothing acted on
any of them. Fixed control flow cannot notice this shape; judgement can. This
brick is where that judgement lives, and it is one contract shared by every
loop that needs it rather than a bespoke check per recipe.

## Why not a numeric iteration cap

There is no `max_iterations` integer in this brick, and none is coming — not
even "as a backstop". Two independent failures make integer caps the wrong
terminator:

- **A cap cuts off work that was about to converge.** Round 12 of a loop that
  is steadily resolving findings is exactly the round you least want to kill.
- **A cap lets a genuinely stuck loop burn the whole budget first.** The 2h47m
  run above would have satisfied any cap of five or more. The waste happened
  *before* the counter would have fired.

The signal to act on is **absence of progress**, not number of attempts. A
loop on round 2 that produced nothing twice is `STUCK`; a loop on round 12
that is still moving is `CONTINUE`.

Host safety does not depend on this brick and never did. It is enforced
structurally one layer down:

| Guard | Bounds | Refusal |
| ----- | ------ | ------- |
| [#1327 sealed recursion ceiling](session-tree-recursion-control.md) | depth | exit `79` |
| #1332 width cap + free-memory floor | fan-out, memory | exit `79` |

Both refuse with exit code `79` before a child ever runs. This brick is about
not wasting hours; it is not runaway protection, and it must not be given that
job.

## The verdict contract

Exactly three outcomes. There is no fourth, and no "maybe".

| Verdict | Meaning | Effect |
| ------- | ------- | ------ |
| `CONTINUE` | Concrete evidence that the last round moved something, and a plausible next step exists. | Another round is authorised. |
| `DONE` | The loop's objective is met. | Advance to the next phase. |
| `STUCK` | No progress is being made. | Stop. Do not proceed. Escalate with the specific evidence of what is not converging. |

**A missing or unparseable verdict is `STUCK`, never `CONTINUE`.** Failing safe
here means stopping, not looping: a fail-open default would authorise exactly
the runaway this contract exists to catch. This is the opposite direction from
[`normalise-verdict`](structured-verdict-parsing.md), whose
`INSUFFICIENT_EVIDENCE` default is deliberately non-fatal — the two normalisers
fail in opposite directions on purpose.

The evaluator agent emits, as the last thing on stdout:

```json
{"loop_verdict": "CONTINUE" | "DONE" | "STUCK", "not_converging": ["<specific evidence>", ...], "moved": ["<what demonstrably changed>", ...], "recommended_action": "<one sentence>"}
```

## `amplihack orch helper normalise-loop-verdict`

Collapses a free-text or synonym loop verdict into one canonical token. It
mirrors [`normalise-verdict`](structured-verdict-parsing.md#amplihack-orch-helper-normalise-verdict):
reads one already-extracted token from stdin, prints the canonical token to
stdout, exits `0` in all cases.

```
amplihack orch helper normalise-loop-verdict
```

### Canonical mapping

| Input token (case-insensitive, **exact match**) | Canonical output |
| ----------------------------------------------- | ---------------- |
| `CONTINUE`, `CONTINUING`, `PROCEED`, `KEEP_GOING`, `ANOTHER_ROUND`, `ITERATE` | `CONTINUE` |
| `DONE`, `COMPLETE`, `COMPLETED`, `FINISHED`, `CONVERGED`, `ADVANCE` | `DONE` |
| `STUCK`, `STOP`, `BLOCKED`, `NO_PROGRESS`, `ESCALATE`, `LOOPING`, `NOT_CONVERGING` | `STUCK` |
| *(anything else, including empty input)* | `STUCK` |

Matching is **exact-token equality, never `str::contains`**. Every one of
`DISCONTINUE`, `CANNOT_CONTINUE`, `DO_NOT_CONTINUE` and `SHOULD_NOT_CONTINUE`
contains `CONTINUE` as a substring, and `NOT_DONE` contains `DONE`; a
containment implementation would fail **open** on all of them and authorise
another round of a dead loop. Under equality they fall through to `STUCK`.

The `CONTINUE` cluster is kept deliberately tight. Anything doubtful belongs in
the `STUCK` default, not in the permissive one.

```bash
echo "CONTINUE"       | amplihack orch helper normalise-loop-verdict   # CONTINUE
echo "converged"      | amplihack orch helper normalise-loop-verdict   # DONE
echo "DISCONTINUE"    | amplihack orch helper normalise-loop-verdict   # STUCK
printf ''             | amplihack orch helper normalise-loop-verdict   # STUCK
```

## Evidence the evaluator receives

`step-01-collect-loop-evidence` computes these **deterministically in bash**
and hands them to the judge as data, alongside the raw round output. The judge
reasons over observations, not over a prose summary of them.

The measurement half is its own brick,
`amplifier-bundle/recipes/loop-evidence-collector.yaml`, composed by the
evaluator as a `type: "recipe"` step. Measurement and judgement are different
jobs with different failure modes, and only one of them needs a model — a
caller that wants the numbers alone can invoke the collector directly. Its
`loop_evidence` output lands in the evaluator's context, so step-02's
`condition:` sees it exactly as it would a local step's output.

| Field | Signal |
| ----- | ------ |
| `terminal_refusal`, `terminal_reason` | A child returned exit `79` / `BLOCKED_TERMINAL`. Terminal — see below. |
| `commits_since_baseline`, `diff_lines`, `diff_stat` | What the last round **actually** produced, from `git rev-list` / `git diff --numstat` against `loop_baseline_ref`. Zero and zero means it produced nothing, whatever the round's prose claims. |
| `repeated_duration_count`, `repeated_duration_value` | How many steps reported the *same* duration. Identical durations repeating are a cap, not work. |
| `repeated_output` | Whether the last round's output is textually a repeat of something already in the history (compared after normalising away digits, punctuation and whitespace). |
| `findings_new`, `findings_recurring`, `findings_resolved` | Set difference over `loop_findings_current` / `loop_findings_previous`. A round that resolves nothing and re-raises the same findings has not moved. |
| `tests_observed`, `tests_moved` | Whether the test signal changed at all. Unobserved is **not** progress. |
| `ci_observed`, `ci_moved` | Same, for CI. |
| `waiting_on_output` | The loop reporting it is waiting on output that never arrives. |
| `git_observed` | Whether diff evidence was available at all (a non-git path reports unobserved, never "progress"). |

## Context inputs

| Var | Default | Meaning |
| --- | ------- | ------- |
| `loop_name` | `unnamed-loop` | Identity for reporting. |
| `loop_round_label` | `""` | A **label** printed in the escalation report. Nothing compares it to a limit and nothing branches on it. |
| `loop_history` | `""` | Accumulated structured verdicts from every round so far, oldest first. |
| `loop_last_round_output` | `""` | Raw output of the round just finished. |
| `loop_repo_path` | `.` | Repo to measure diff/commit evidence in. |
| `loop_baseline_ref` | `""` | Git ref at the start of the round just finished. Falls back to the working-tree diff with a visible `WARNING` when unresolvable. |
| `loop_child_exit_code` | `""` | Exit code of the last child. `79` is a terminal policy refusal. |
| `loop_findings_current` / `loop_findings_previous` | `""` | Newline-separated finding identifiers for the last two rounds. |
| `loop_test_signal` / `loop_test_signal_previous` | `""` | Test result summaries for the last two rounds. |
| `loop_ci_signal` / `loop_ci_signal_previous` | `""` | CI result summaries for the last two rounds. |
| `loop_health_enforce` | `"true"` | When `"false"`, `STUCK` is reported loudly but exits `0`, leaving the stop entirely to the caller's `condition:`. |

## Outputs

| Output | Shape |
| ------ | ----- |
| `loop_evidence` | The measured-evidence JSON object above (`parse_json: true`), produced by the `loop-evidence-collector` sub-recipe. |
| `loop_health_assessment` | The evaluator agent's raw output. |
| `loop_health` | `{"loop_verdict": …, "verdict_source": …, "loop_name": …, "not_converging": [...]}` (`parse_json: true`). |
| `loop_health_enforcement` | The enforcement step's report. |

`verdict_source` is one of `evaluator`, `terminal_policy_refusal`,
`missing_verdict`, or `unparseable_verdict`, so a forced `STUCK` is always
attributable.

## Using it from a recipe

Invoke the brick after each round and gate the next round on the verdict:

```yaml
  - id: "round-2"
    type: "recipe"
    recipe: "loop-health-evaluator"
    context:
      loop_name: "pr-review"
      loop_round_label: "round-1"
      loop_history: "{{review_history}}"
      loop_last_round_output: "{{review_round_1}}"
      loop_baseline_ref: "{{round_1_base_sha}}"
      loop_child_exit_code: "{{round_1_exit_code}}"
      loop_findings_current: "{{round_1_findings}}"
      loop_findings_previous: "{{round_0_findings}}"

  - id: "round-2-work"
    condition: "loop_health.loop_verdict == 'CONTINUE'"
    agent: "amplihack:reviewer"
    prompt: |
      …

  - id: "advance"
    condition: "loop_health.loop_verdict == 'DONE'"
    …
```

`STUCK` needs no condition of its own. Both gates above are false, **and**
`step-04-enforce-loop-verdict` exits non-zero — so a caller who forgets to
write the `CONTINUE` condition still stops. Set `loop_health_enforce: "false"`
only when the caller genuinely owns the stop decision.

### Precedent this deliberately does not follow

[`auto-workflow.yaml`](../../amplifier-bundle/recipes/auto-workflow.yaml) is the
existing precedent for an iterative loop. It uses a fixed `max_iterations: 5`
with five hand-unrolled `execute-iteration-N` steps, each gated on
`'CONTINUE' in iteration_{N-1}` — a prose substring search over free agent
text. Both mechanisms are exactly what this contract replaces: the counter is
the wrong terminator, and a substring match over prose fails open the moment an
agent writes "I will not continue". `auto-workflow.yaml` is left as-is here;
this brick is the pattern new loops should use.

## Exit code 79 is terminal

Exit code `79` and `BLOCKED_TERMINAL` are final answers from a structural
guard (#1327 / #1332). They are surfaced and the loop stops. They are **never**
retried into, and the agentic step is skipped entirely on a terminal refusal
(`condition: "loop_evidence.terminal_refusal == 'false'"`) — not even a model
call is spent deciding whether to re-enter a sealed guard. A terminal refusal
forces `STUCK` even if an evaluator verdict from an earlier round said
`CONTINUE`.

Historically, agents read a `79` refusal as an infrastructure fault and retried
one level deeper with a raised ceiling. That is what #1327 sealed, and it is
what this branch refuses to re-open.

## Fail-safe guarantees

Every branch fails toward stopping:

- Missing evaluator output → `STUCK` (`verdict_source=missing_verdict`).
- Unparseable evaluator output → `extract-json` yields `{}` → the `STUCK`
  default → `STUCK` (`verdict_source=unparseable_verdict`).
- A verdict token outside the three canonical outcomes → `STUCK`.
- Unparseable **evidence** → `terminal_refusal` defaults to `true` → `STUCK`.
  An absent evidence object is never read as "the guard did not fire".
- Enforcement on an empty or malformed `loop_health` → `STUCK`, exit non-zero.
- An explicit JSON `null` (`{"terminal_refusal": null}`) takes the `--default`
  too. `extract-field` treats a null exactly like an absent field, so the
  `--default true` fail-safe cannot be walked past with a null.

### Reading step outputs: the dual-name idiom

`recipe-runner-rs` exports every step output as `RECIPE_VAR_<name>`, but it
only adds the plain-uppercase alias (`LOOP_EVIDENCE`) for **scalar** outputs.
`loop_evidence` and `loop_health` declare `parse_json: true`, and an evaluator
that obeys the "emit only the JSON object" contract makes
`loop_health_assessment` an object too. All three plain names are therefore
**absent** at runtime, and a bare `${LOOP_EVIDENCE:-}` read is silently empty.

Every read in these bricks uses the dual-name form, the same idiom as
`workflow-publish` / `workflow-finalize` / `workflow-terminal-state`:

```bash
EV="${LOOP_EVIDENCE:-${RECIPE_VAR_loop_evidence:-}}"
```

Dropping a fallback makes the recipe inert: the reads miss, `--default true`
fires, and every run reports a `terminal_policy_refusal` that never happened.
An integration test guards the invariant statically and the contract test
proves it end to end through the real runner.

### Which JSON object is the verdict

The evaluator's output is selected with
`extract-json --require-field loop_verdict`: of every JSON object in the
output, the **last one carrying `loop_verdict`** is the verdict. Plain
first-parseable-object-wins is fail-open in both directions — it reads a draft
verdict over the reconsidered one, and it reads evidence the model quoted back
inside a ```json fence over the real verdict that follows. The prompt states
the same rule, so prompt and extractor agree.

### The brick does not poison itself

Every line these bricks author is tagged `[loop-health-evaluator]`, and the
collector drops tagged lines before running its terminal detectors. Without
that, the brick's own reason string ("child process exited 79 …") re-matches
its own exit-79 regex, so feeding one escalation report back into
`loop_history` would make the loop permanently terminal on evidence it
invented.

Agent output is treated as untrusted **data** throughout, per
[Structured Verdict & Intent Parsing](structured-verdict-parsing.md#security-agent-output-is-untrusted-data-never-code):
it reaches bash steps as an environment variable, is fed to the helpers on
stdin with `printf '%s'`, and is never interpolated into a command position,
`eval`'d, or branched on as raw prose.

### No short timeouts

No step in this brick declares a `timeout` or `timeout_seconds`, and the recipe
declares no `default_step_timeout`. Per issue #439 the runner owns the ceiling;
nothing here is bounded at seconds or single-digit-minute scale. A false
timeout costs more than a slow run — and a step that keeps hitting a ceiling is
precisely what `repeated_duration_count` is for.

## Worked example — the 2h47m run

Feeding the real run's evidence to `step-01-collect-loop-evidence`:

```json
{
  "terminal_refusal": "true",
  "terminal_reason": "BLOCKED_TERMINAL reported by a child; exit code 79 present in round output",
  "commits_since_baseline": 0,
  "diff_lines": 0,
  "repeated_duration_count": 7,
  "repeated_duration_value": "10m 0s",
  "findings_new": 0,
  "findings_recurring": 2,
  "findings_resolved": 0,
  "tests_moved": "false",
  "waiting_on_output": "true"
}
```

Seven identical `10m 0s` durations, zero commits, zero diff lines, nothing
resolved, the test signal unmoved, the run waiting on output that never
arrives, and a terminal `79` refusal. Verdict: **`STUCK`** — on the first round
the evidence becomes visible, not after some number of attempts. The loop stops
and reports what is not converging instead of consuming the remaining budget.

## Tests

| Test | Location |
| ---- | -------- |
| Helper unit tests (synonyms, canonical pass-through, malformed → `STUCK`, negation-adjacent equality regression, opposite-default guard) | `crates/amplihack-cli/src/commands/orch.rs` |
| Executable contract test (STUCK path, malformed-verdict path, exit-79 terminal path, the 2h47m worked example, no-cap and no-timeout guards) | `amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh` |
| **End-to-end probe** — the real recipe files run through the real `recipe-runner-rs` with only step-02 stubbed as a bash step: `CONTINUE` → exit 0, `STUCK` → exit 1, verdict selection, exit-79 terminal, self-poisoning | same file, section 7 (skipped with a loud notice when `recipe-runner-rs` is not installed; set `RECIPE_RUNNER_RS_PATH` to force it) |
| Structural + end-to-end wiring | `tests/integration/issue_1337_loop_health_evaluator_test.rs` |

Run them with:

```bash
cargo test -p amplihack-cli normalise_loop_verdict
cargo test -p amplihack --test issue_1337_loop_health_evaluator
bash amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh
```

## Related references

- [Structured Verdict & Intent Parsing](structured-verdict-parsing.md) — the
  `extract-json | extract-field | normalise-*` pipeline this contract fits into.
- [Session Tree Recursion Control](session-tree-recursion-control.md) — the
  sealed depth ceiling behind exit code `79`.
- [Recipe Executor Environment](recipe-executor-environment.md) — how context
  vars and step outputs reach bash steps and engine conditions.
- [Recipe Quick Reference](recipe-quick-reference.md) — recipe authoring basics.
