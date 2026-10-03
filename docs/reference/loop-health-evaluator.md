---
title: Loop-Health Evaluator Reference
last_updated: 2026-10-03
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
- [How step-03 resolves the verdict](#how-step-03-resolves-the-verdict)
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

### Where the prompt states the contract

The step-02 prompt states the output contract **twice**:

1. **First**, before any evidence: an `OUTPUT CONTRACT` block saying the
   answer is one final JSON object with `loop_verdict` and `not_converging`,
   and that the gate reads the LAST JSON object carrying `loop_verdict`.
2. **Last**, after all evidence: the contract again, with this example and the
   legal tokens:

   ```
   {"loop_verdict":"CONTINUE","not_converging":[]}
   CONTINUE | DONE | STUCK are the only legal values. Any other word is STUCK.
   ```

A contract that appears only once, between pages of evidence, is easy for a
model to lose. Real evaluators answered with prose such as
`CONTINUE — round 1 made real progress…`, or with the wrong key
(`{"verdict":"converging"}`), and every such answer stopped the loop as
`STUCK` even when the loop was moving (issue #1513). Stating the contract at
both ends makes the JSON answer the normal case. The fallbacks in the next
section handle the rest.

No prompt line, indented or not, starts with `LOOP_HEALTH: CONTINUE`,
`LOOP_HEALTH: DONE` or `Output: LOOP_HEALTH:`. Those strings are what
`autodrive_loop.sh` reads as the health marker, and an example of one in the
prompt could be echoed back into the log (see
[Reading the loop-health line](auto-drive-to-merge.md#reading-the-loop-health-line)).
A shell assertion in the contract test enforces this.

## How step-03 resolves the verdict

`step-03-resolve-loop-verdict` turns the evaluator's raw output into the
`loop_health` object. It starts from `STUCK` with
`verdict_source=unparseable_verdict` and tries four sources in a fixed order.
Each source runs only if every source before it found nothing.

| Order | Source | What it looks for | `verdict_source` |
| ----- | ------ | ----------------- | ---------------- |
| 1 | Primary key | `extract-json --require-field loop_verdict`: the last JSON object carrying `loop_verdict`. | `evaluator` |
| 2 | Alternate key | `extract-json --require-field verdict`: the last JSON object carrying `verdict`. | `evaluator_alt_key` |
| 3 | Prose token | One line-leading `CONTINUE`, `DONE` or `STUCK` (rules below). | `evaluator_prose_token` |
| 4 | Nothing usable | No token, or conflicting tokens. | `unparseable_verdict` (verdict `STUCK`) |

Every token, from every source, then goes through
[`normalise-loop-verdict`](#amplihack-orch-helper-normalise-loop-verdict) and
the existing `case` guard. Structured data always beats prose: a
`loop_verdict` object is used even when the text around it contains a
different prose token.

### The alternate key is final

When an object with a `verdict` key exists, its normalised value is the answer,
even if that value is unknown. `{"verdict":"MAYBE"}` gives `STUCK` with
`verdict_source=evaluator_alt_key`; step-03 does not go on to scan the prose
for a second answer. The evaluator did answer, so looking for a different
answer elsewhere would only add ways to fail open. When the object has no
`not_converging` array, it defaults to `[]`.

### Prose token rules

The prose scan accepts a verdict token only when there is no doubt about it:

- Lines inside a fenced code block (between two ```` ``` ```` lines) are
  dropped first, so a quoted example is never read as the answer.
- The token is at the **start of a line**, after optional leading whitespace.
- It may follow one optional prefix: a markdown heading (`#`, `##`, …), then
  `Verdict:` or `LOOP_HEALTH:`. The prefixes match in any case.
- The token itself must be **upper case**: `CONTINUE`, `DONE` or `STUCK`.
  "We should continue" in ordinary English never matches.
- The token must end the line or be followed by a space, `:`, `.`, `–` or
  `—`. A hyphen counts only after whitespace (`DONE - all clear`); a hyphen
  straight after the token (`DONE-ish`) does not match. So `DISCONTINUE`,
  `NOT_DONE`, `CONTINUED` and `DONE-ish` never match.
- A line quoted with `>` does not match; the token must not follow any other
  character.
- The same token on several lines counts once.
- Two or more **different** tokens give `STUCK` with
  `verdict_source=unparseable_verdict`.

The rule as an extended regular expression, matched under `LC_ALL=C`. The
em-dash and en-dash are written as alternatives, not inside a bracket class,
because they are multibyte:

```
^[[:space:]]*(#+[[:space:]]*)?(([Vv][Ee][Rr][Dd][Ii][Cc][Tt]|[Ll][Oo][Oo][Pp]_[Hh][Ee][Aa][Ll][Tt][Hh])[[:space:]]*:[[:space:]]*)?(CONTINUE|DONE|STUCK)( |$|:|\.|[[:space:]]-|–|—)
```

The evaluator's text is only ever data: it reaches `grep`, `awk` and the
helpers through `printf '%s' "$RAW"` on stdin. It is never passed to `eval`,
put in a `-c` argument, a regex or a format string. Prose synonyms such as
`## Verdict: CONVERGING` are not accepted; synonyms apply only on the JSON
paths.

On the prose path `not_converging` is a fixed literal written by step-03, never
text copied from the evaluator.

### Examples

| Evaluator output | `loop_verdict` | `verdict_source` |
| ---------------- | -------------- | ---------------- |
| `{"loop_verdict":"DONE","not_converging":[]}` | `DONE` | `evaluator` |
| `CONTINUE — round 1 made real progress on the review threads` | `CONTINUE` | `evaluator_prose_token` |
| `## Verdict: CONTINUE` | `CONTINUE` | `evaluator_prose_token` |
| `## Verdict: continue` | `CONTINUE` | `evaluator_prose_token` |
| `LOOP_HEALTH: DONE` | `DONE` | `evaluator_prose_token` |
| `{"verdict":"CONVERGING"}` | `CONTINUE` | `evaluator_alt_key` |
| `{"verdict":"MAYBE"}` | `STUCK` | `evaluator_alt_key` |
| `The loop looks healthy and should probably keep going.` | `STUCK` | `unparseable_verdict` |
| `DISCONTINUE` | `STUCK` | `unparseable_verdict` |
| `CONTINUE` on one line and `STUCK` on another | `STUCK` | `unparseable_verdict` |
| `DONE.` on two separate lines | `DONE` | `evaluator_prose_token` |
| `DONE-ish, mostly` | `STUCK` | `unparseable_verdict` |
| `I cannot CONTINUE` | `STUCK` | `unparseable_verdict` |
| `continue reading below` | `STUCK` | `unparseable_verdict` |
| `> CONTINUE` | `STUCK` | `unparseable_verdict` |
| `DONE` only inside a ```` ``` ```` fenced block | `STUCK` | `unparseable_verdict` |
| *(empty output)* | `STUCK` | `missing_verdict` |
| `{"verdict":"DONE"}` quoted, then `{"loop_verdict":"CONTINUE","not_converging":[]}` | `CONTINUE` | `evaluator` |
| Prose `STUCK` line followed by `{"loop_verdict":"CONTINUE","not_converging":[]}` | `CONTINUE` | `evaluator` |

When nothing usable is found, step-03 prints a `WARNING` on stderr that says
which case it hit (no token, or which tokens conflicted) and sets a one-item
`not_converging` explaining why. The warning never prints the evaluator's raw
output.

### Verdict JSON round-trip guard

Step-03 builds the `loop_health` JSON from `VERDICT`, `SOURCE`, `LOOP_NAME`,
`not_converging` and, on the terminal-refusal path, the terminal reason. Some
of those values come from agent output or evidence, so step-03 protects the
object in two ways.

**Cleaning.** `LOOP_NAME`, the terminal reason and the `WHY` text are passed
through `tr -d '"\\\000-\037'` before they go into the JSON. That removes
double quotes, backslashes and control characters, including newlines.

**Parsing it back.** Before it prints the object, step-03 reads it back with
exactly the pipeline step-04 uses:

```bash
printf '%s' "$OUT" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field loop_verdict --default STUCK \
  | amplihack orch helper normalise-loop-verdict
```

and reads `verdict_source` from the same object with `extract-field`. If either
value differs from `$VERDICT` or `$SOURCE`, step-03:

1. replaces `not_converging` with the fixed
   `["not_converging rejected: not a JSON array"]`, rebuilds the object and
   prints a `WARNING`;
2. checks again, and if the rebuilt object still does not read back correctly,
   prints this fixed object instead:

   ```json
   {"loop_verdict":"STUCK","verdict_source":"unparseable_verdict","loop_name":"unnamed-loop","not_converging":["round-trip guard failed"]}
   ```

The guard runs on every branch, including the terminal-refusal and
empty-output branches. A value inside the object, such as a string-valued
`not_converging` that tries to add a second `loop_verdict`, can never change
the verdict the object reports.

### Step-04's marker is always one line

Step-04 cleans `LOOP_NAME` the same way before printing
`LOOP_HEALTH: <verdict> — …`, so the marker can never be split across lines.
Any agent-derived value printed in a `WARNING` or `INFO` line has its control
characters removed, so it cannot fake a `BLOCKED_TERMINAL` line.

On `CONTINUE` and `DONE` the marker is step-04's **first and only** stdout
line. Everything else, including the whole `STUCK` report, goes to stderr, so
on `STUCK` stdout is empty. This matters because amplihack's run formatter
puts `    Output: ` in front of the first line of a step's output only, and
[`autodrive_loop.sh` reads exactly that line](auto-drive-to-merge.md#which-line-is-trusted).
A new `echo` to stdout ahead of the marker would make every round read as
`STUCK`; the contract test fails if step-04's stdout is ever more than one
line or does not start with `LOOP_HEALTH: `.

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
| `CONTINUE`, `CONTINUING`, `PROCEED`, `KEEP_GOING`, `ANOTHER_ROUND`, `ITERATE`, `CONVERGING`, `PROGRESSING` | `CONTINUE` |
| `DONE`, `COMPLETE`, `COMPLETED`, `FINISHED`, `CONVERGED`, `ADVANCE` | `DONE` |
| `STUCK`, `STOP`, `BLOCKED`, `NO_PROGRESS`, `ESCALATE`, `LOOPING`, `NOT_CONVERGING` | `STUCK` |
| *(anything else, including empty input)* | `STUCK` |

Matching is **exact-token equality, never `str::contains`**. Every one of
`DISCONTINUE`, `CANNOT_CONTINUE`, `DO_NOT_CONTINUE` and `SHOULD_NOT_CONTINUE`
contains `CONTINUE` as a substring, and `NOT_DONE` contains `DONE`; a
containment implementation would fail **open** on all of them and authorise
another round of a dead loop. Under equality they fall through to `STUCK`.

The `CONTINUE` cluster is kept deliberately tight. Anything doubtful belongs in
the `STUCK` default, not in the permissive one. `CONVERGING` and `PROGRESSING`
are in it because evaluators use them to mean "the loop is moving" (issue
#1513); `CONVERGED` is the finished form and maps to `DONE`. Their negations and
variants are not synonyms: `NOT_CONVERGING`, `NOT_CONVERGED`,
`NOT_PROGRESSING`, `UNCONVERGING` and `PROGRESSING_NOT` all give `STUCK`.

```bash
echo "CONTINUE"       | amplihack orch helper normalise-loop-verdict   # CONTINUE
echo "converged"      | amplihack orch helper normalise-loop-verdict   # DONE
echo "converging"     | amplihack orch helper normalise-loop-verdict   # CONTINUE
echo "NOT_PROGRESSING" | amplihack orch helper normalise-loop-verdict  # STUCK
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

`verdict_source` is always one of these six values, so every verdict, and
every forced `STUCK`, is attributable:

| `verdict_source` | Meaning |
| ---------------- | ------- |
| `evaluator` | The evaluator emitted a JSON object with `loop_verdict`. |
| `evaluator_alt_key` | No `loop_verdict` object; the evaluator emitted an object with `verdict` instead. |
| `evaluator_prose_token` | No JSON verdict; one clear line-leading token was found in the prose. |
| `terminal_policy_refusal` | The evidence showed exit `79` / `BLOCKED_TERMINAL`; the evaluator was skipped. |
| `missing_verdict` | The evaluator produced no output. |
| `unparseable_verdict` | Output existed but held no usable verdict, or held conflicting tokens. |

See [How step-03 resolves the verdict](#how-step-03-resolves-the-verdict) for
the order in which these are tried.

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
- Unparseable evaluator output → neither JSON key is found and the prose
  scan finds no single clear token → `STUCK`
  (`verdict_source=unparseable_verdict`).
- Conflicting prose tokens (`CONTINUE` on one line, `STUCK` on another) →
  `STUCK` (`verdict_source=unparseable_verdict`).
- A value inside the verdict object that would change the parsed verdict or
  source → rejected by the [round-trip guard](#verdict-json-round-trip-guard).
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
| Helper unit tests (synonyms including `CONVERGING` / `PROGRESSING`, canonical pass-through, malformed → `STUCK`, negation-adjacent equality regression including `NOT_PROGRESSING`, opposite-default guard) | `crates/amplihack-cli/src/commands/orch.rs` |
| Executable contract test (STUCK path, malformed-verdict path, exit-79 terminal path, the 2h47m worked example, no-cap and no-timeout guards, and `resolve_verdict_and_source` over every row of the [step-03 examples](#examples) plus the round-trip guard, `LOOP_NAME` cleaning and prompt-marker cases, and step-04's stdout being at most one `LOOP_HEALTH: ` line for `CONTINUE`, `DONE` and `STUCK`) | `amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh` |
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
