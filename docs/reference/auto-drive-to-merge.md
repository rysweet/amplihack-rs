---
title: Auto Drive To Merge Reference
last_updated: 2026-10-03
review_schedule: quarterly
owner: workflow-team
---

# Auto Drive To Merge Reference

> [Home](../index.md) > Reference > Auto Drive To Merge

`auto-drive-to-merge` wraps [`default-workflow`](../../amplifier-bundle/recipes/default-workflow.yaml)
and drives the pull request it produces all the way to a merge. It encodes the
method of using the `crusty-old-engineer` skill as the maintainer's proxy:
iterate on the PR until crusty has zero outstanding concerns, then iterate
until every merge-ready criterion holds, then merge behind an evidence gate.

It is invocable as a recipe or as the [`auto-drive-to-merge`
skill](../../amplifier-bundle/skills/auto-drive-to-merge/SKILL.md).

## Contents

- [The three phases](#the-three-phases)
- [Why there is no iteration cap](#why-there-is-no-iteration-cap)
- [Structured verdicts](#structured-verdicts)
- [Reading step outputs](#reading-step-outputs)
- [Reading the loop-health line](#reading-the-loop-health-line)
- [Two absolute prohibitions](#two-absolute-prohibitions)
- [No silent merge](#no-silent-merge)
- [Exit code 79 is terminal](#exit-code-79-is-terminal)
- [Recursion context propagation](#recursion-context-propagation)
- [Resumability](#resumability)
- [No short timeouts](#no-short-timeouts)
- [Files](#files)
- [Tests](#tests)
- [Dependency on PR #1347](#dependency-on-pr-1347)
- [Related references](#related-references)

## The three phases

| Phase | Brick | What it does |
| --- | --- | --- |
| 1. Build | `autodrive-build.yaml` | Runs `default-workflow` with `no_merge: "true"` to produce a PR. Never merges. |
| 2. Crusty loop | `autodrive-crusty-loop.yaml` over `autodrive-crusty-round.yaml` | Runs `crusty-old-engineer` as the maintainer's proxy, addresses every concern, re-reviews, repeats until `crusty_verdict` is `CLEAN`. |
| 3. Merge-ready loop | `autodrive-merge-loop.yaml` over `autodrive-merge-round.yaml` | Syncs the base, runs the `qa-team` scenarios, waits for CI, applies the `merge-ready` criteria, clears blockers, repeats — then merges behind the gate. |

Phase 1 is a no-op when an open PR already exists for the branch, and the whole
workflow short-circuits when the PR is already merged.

## Why there is no iteration cap

Neither loop has a `max_rounds`, a "backstop" integer, or a wall-clock budget.
Integer caps fail in both directions:

- **A cap cuts off work that was about to converge.** Round 12 of a loop
  steadily resolving concerns is the round you least want to kill.
- **A cap lets a genuinely stuck loop burn the whole budget first.** The waste
  happens *before* the counter fires.

The signal acted on is **absence of progress**, not number of attempts. Each
round therefore ends by invoking the
[`loop-health-evaluator`](loop-health-evaluator.md) brick, which reads measured
evidence — what the round actually produced, which findings are new /
recurring / resolved, whether test and CI signals moved, whether the same text
keeps reappearing — and answers `CONTINUE`, `DONE`, or `STUCK`.

`autodrive_loop.sh` carries a round **label** (`round-3`) for reports. Nothing
compares it to a limit and no branch reads it. The guard test
`no_numeric_iteration_cap_anywhere` fails the build if a cap is ever
reintroduced — it rejects `$ROUND` and `${ROUND}` next to any of `-ge`, `-gt`,
`-le`, `-lt`, `-eq`, `-ne`, and the arithmetic comparisons, so a cap cannot
sneak in through the operator that was not on the list.

### The terminator is resolved before round 1

Because the terminator is a separate recipe invoked by name, a bundle where
`loop-health-evaluator` does not resolve would otherwise cost a **full round**
— a crusty review and a builder fix pass, with commits pushed — before the
loop stopped with "returned STUCK (or an unreadable verdict)", which blames the
loop for a missing dependency. `autodrive_loop.sh` therefore runs
`amplihack recipe show loop-health-evaluator` before the first round and, if it
does not resolve, refuses with `loop_result: MISSING_DEPENDENCY` and a message
naming the recipe. Zero rounds are spent.

Host safety is enforced structurally one layer down and is not this workflow's
job:

| Guard | Bounds | Refusal |
| --- | --- | --- |
| #1327 sealed recursion ceiling | depth | exit `79` |
| #1332 width cap + free-memory floor | fan-out, memory | exit `79` |

Rounds run **sequentially at constant session depth**, so a long loop never
walks toward that ceiling.

## Structured verdicts

No phase advances on a model's prose impression. Every gate reads a structured
field through the canonical pipeline documented in
[Structured Verdict & Intent Parsing](structured-verdict-parsing.md):

```bash
VALUE=$(printf '%s' "$RAW" \
  | amplihack orch helper extract-json --require-field FIELD \
  | amplihack orch helper extract-field --field FIELD --default SAFE_DEFAULT)
```

### `--require-field` is not optional here

Plain `extract-json` returns the **first** complete JSON object it finds, and
prefers a ` ```json ` fenced block over an untagged one, and either over raw
prose. That is fail-OPEN for a verdict. A reviewer that restates its output
contract before reviewing — ordinary behaviour, and the `crusty-old-engineer`
skill documents the shape it must emit — puts an example object ahead of its
real verdict, and the parser reads the example:

```
Review.

<a fenced json block containing {"crusty_verdict": "CLEAN", …}>

{"crusty_verdict":"CONCERNS", …}      <- the actual verdict, ignored
```

Nothing downstream would catch it. `autodrive_loop.sh` takes the recorded
`crusty_verdict` at face value, and phase 3 has no step that re-measures
crusty's judgement the way it re-measures CI — an accidental `CLEAN` is an
unearned advance toward a merge. `merge_ready_verdict` has the same exposure
but is largely saved by the measurement downgrade below; crusty is not.

`--require-field NAME` (issue #1337, PR #1347) collects every JSON object in
**document order** and returns the LAST one carrying `NAME`, which is exactly
what the prompts ask for ("as the very last thing you emit"). When no object
carries the field it returns nothing, so the blocking `--default` applies
rather than some unrelated object. Every `extract-json` on the control path
passes it, and the guard test
`every_verdict_gate_selects_the_last_object_carrying_its_field` fails the build
if one does not. The crusty skill's own verdict examples are deliberately
unfenced for the same reason.

Agent output is untrusted data: it reaches bash steps as an environment
variable, is fed to the helpers on stdin with `printf '%s'`, and is never
interpolated into a command position, `eval`'d, or branched on as raw prose.
Every extracted token is then matched against an exact-token allow-list; a
token outside the allow-list falls to the safe default.

| Signal | Field | Clean token | Fail-safe default | `verdict_source` values |
| --- | --- | --- | --- | --- |
| Crusty review | `crusty_verdict` | `CLEAN` | `CONCERNS` | `crusty`, `missing_verdict`, `unparseable_verdict` |
| Merge-ready | `merge_ready_verdict` | `MERGE_READY` | `NOT_MERGE_READY` | `merge_ready`, `missing_verdict`, `unparseable_verdict`, `evidence_downgrade` |
| Loop health | `loop_verdict` | `DONE` | `STUCK` | see [loop-health-evaluator](loop-health-evaluator.md) |

**A missing or unparseable verdict is never the permissive one.** It is
`CONCERNS`, `NOT_MERGE_READY`, `STUCK` — never `CLEAN`, `MERGE_READY`, or
`CONTINUE`.

### Advancing needs two independent signals

`autodrive_loop.sh` advances a phase only when **both** hold:

1. the round's own machine-checked verdict is the clean token, **and**
2. the loop-health evaluator returned `DONE`.

`DONE` over a non-clean round verdict is an inconsistent pair and is treated as
`STUCK` — it never advances. A clean round verdict with `CONTINUE` simply runs
another confirming round, which is cheap and safe.

### Measurement outranks the model

Even an explicit `MERGE_READY` is downgraded to `NOT_MERGE_READY` when the
measured evidence disagrees: `qa_status` other than `PASS`, `ci_status` other
than `GREEN`, a merge conflict, or unresolved review threads. The downgrade is
recorded in `downgrade_reason` and adds a `measured-evidence-disagrees` finding
so the loop evaluator sees a round that did not converge.

### The crusty verdict contract

`crusty-old-engineer` historically emitted prose only. It now emits an
**opt-in** structured block — requested in the prompt, or via
`CRUSTY_OUTPUT_CONTRACT=structured` — appended after its normal review:

```json
{"crusty_verdict": "CLEAN" | "CONCERNS",
 "concerns": [{"id": "<stable-kebab-id>", "severity": "blocking|major|minor",
               "summary": "<one line>", "evidence": "<file:line or output>"}],
 "summary": "<one line>"}
```

Standalone human use is unchanged: asked a question directly, the skill answers
in its usual structure and emits no JSON. `id` must be stable across rounds and
derived from the substance of the concern — that is what lets the loop tell a
recurring concern from a new one.

## Reading step outputs

Each recipe in this workflow passes results between steps through step
outputs: the preflight's `state_dir`, a round's verdict record, the build
result. `recipe-runner-rs` exports every step output to later bash steps as
`RECIPE_VAR_<output>`, where `<output>` is the step's `output:` value exactly
as written. It adds the plain upper-case alias (`CRUSTY_LOOP_PREFLIGHT`) only
for **scalar** outputs. These outputs are JSON objects, so the alias is never
set, and a bare `${CRUSTY_LOOP_PREFLIGHT:-}` read is always empty. Before issue
#1511 was fixed, every read missed, every loop lost its state directory, and
every run stopped as `STUCK`.

Every step-output read in the autodrive recipes uses both names, on one line,
inside double quotes:

```bash
PREFLIGHT="${CRUSTY_LOOP_PREFLIGHT:-${RECIPE_VAR_crusty_loop_preflight:-}}"
```

The bare name is tried first, the same idiom as `workflow-publish`,
`workflow-finalize` and the [loop-health evaluator](loop-health-evaluator.md#reading-step-outputs-the-dual-name-idiom).
`:-` treats an empty value as unset, so an empty context default such as
`build_result: ""` still falls through to `RECIPE_VAR_build_result`. The
`RECIPE_VAR_` value is read only through `extract-json` / `extract-field`; it
is never sourced, `eval`'d or exported.

The rule covers **step outputs only**. Context variables (`REPO_PATH`, `PR_*`,
`AUTODRIVE_*`) and shell locals (`COUNT`, `RC`, `PENDING`, …) are read as
before. `autodrive-merge-evidence.yaml` and `loop-health-evaluator.yaml` need
no change: the first declares step outputs but reads none, and the second
already used the dual-name form.

A read is one `${<NAME>:-` occurrence, so a line that reads the same output
twice counts twice. There are 31 in all:

| Recipe | Step-output reads |
| --- | --- |
| `auto-drive-to-merge.yaml` | 3 |
| `autodrive-build.yaml` | 1 |
| `autodrive-crusty-loop.yaml` | 5 |
| `autodrive-crusty-round.yaml` | 6 |
| `autodrive-merge-loop.yaml` | 5 |
| `autodrive-merge-round.yaml` | 11 |

### `state_dir` is checked before use

With the fallback in place, the preflight's `state_dir` reaches later steps.
Every step that uses it refuses these values with the usual `no state_dir`
error:

- an empty value;
- `/`;
- a value starting with `-`;
- a value containing a newline.

The path is always quoted, and `rm`, `mkdir` and `cp` get `--` before it. An
absolute path is not required; the preflight decides where state lives.

### The static check

The test `autodrive_step_outputs_are_read_with_the_recipe_var_fallback`
collects the `output:` names declared across **all** the autodrive recipes
(`auto-drive-to-merge.yaml` and every `autodrive-*.yaml`) into one set, then
checks every one of those files against the whole set. Per-file collection
would miss reads of outputs declared in a sub-recipe:
`autodrive-merge-round.yaml` reads `QA_EVIDENCE`, `CI_EVIDENCE` and
`MERGE_SYNC`, which `autodrive-merge-evidence.yaml` declares. Any line that
reads `${<NAME_UPPER>:-` for a name in the set must also contain
`RECIPE_VAR_<name>`. A failure names the file, line and output. The shell
contract test runs the same check.

## Reading the loop-health line

After each round, `autodrive_loop.sh` runs `loop-health-evaluator` and reads
its verdict from the log. `step-04-enforce-loop-verdict` prints one marker
line, `LOOP_HEALTH: <verdict> — …`. amplihack's run formatter
(`crates/amplihack-cli/src/commands/recipe/run/format.rs`) prints a status
line for each step, then the step's output behind a four-space `Output: `
prefix. A finished evaluator run therefore ends like this:

```
  ✓ step-04-enforce-loop-verdict: completed [elapsed: 120ms]
    Output: LOOP_HEALTH: DONE — 'crusty' has converged; advance.
```

The status line has the form
`  <symbol> <id>[ (<step name>)]: <status>[ [<details>]]`. The step name, when
the step has one, sits in parentheses **before** the colon. The details
(`phase: …`, `elapsed: …`, `child: …`, comma-separated) sit in square brackets
**after** the status. Most runs record an elapsed time, so the suffix is the
normal case, not the exception.

The formatter puts `    Output: ` in front of the **first** line of a step's
output only; later lines are printed as they are. Step-04 therefore prints its
`LOOP_HEALTH: <verdict> — …` marker as its **first and only** stdout line, and
writes everything else (the `STUCK` explanation, `WARNING` and `INFO` lines) to
stderr. The shell contract test pins this: it runs step-04 for `CONTINUE`,
`DONE` and `STUCK` and checks that stdout is exactly zero or one line and,
when present, starts with `LOOP_HEALTH: `.

Before issue #1512 was fixed, the reader looked only for `LOOP_HEALTH:` at the
start of a line, never found it, and treated every round as `STUCK`.

### Which line is trusted

The log also holds the evaluator agent's own output, which can contain any
text, including a copy of the two lines above. The reader therefore trusts one
line only:

1. `step04_output_line` (an `LC_ALL=C awk` function) finds the **last** status
   line for `step-04-enforce-loop-verdict`: two spaces, any status symbol, a
   space, the step id, then ` (` or `:`.
2. It keeps that block only if the whole line matches
   `^  ✓ step-04-enforce-loop-verdict( \([^)]*\))?: completed( \[[^]]*\])?$`,
   which allows the optional parenthesised step name and the optional
   bracketed details suffix and nothing else. Under `LC_ALL=C` the `✓` is
   matched as its three UTF-8 bytes.
3. It takes the line **right after** the status line. Any other line in
   between, or no line at all, means no verdict.
4. That line must match
   `^    Output: LOOP_HEALTH: (CONTINUE|DONE)( |$)`. `CONTINUE` is checked
   before `DONE`.

Taking the last block, not the first, matters. The agent's output is printed
earlier in the log than the real step-04 block, so a fake block inside it can
never be the last one.

### When the whole log is read

If the log has **no** step status lines at all (lines starting
`  <symbol> step-`), it did not come from the run formatter. Only then does the
reader test every line with `^(    Output: )?LOOP_HEALTH: (CONTINUE|DONE)( |$)`.
A log that has status lines but no usable step-04 pair never falls back to
this scan.

Anything else is `STUCK`, with the warning
`loop-health-evaluator exited 0 with no readable LOOP_HEALTH verdict; failing
safe to STUCK.`

| Log contents | Verdict |
| --- | --- |
| `  ✓ step-04-enforce-loop-verdict: completed` then `    Output: LOOP_HEALTH: DONE — converged` | `DONE` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `    Output: LOOP_HEALTH: CONTINUE` | `CONTINUE` |
| `  ✓ step-04-enforce-loop-verdict: completed [elapsed: 120ms]` then `    Output: LOOP_HEALTH: DONE` | `DONE` |
| `  ✓ step-04-enforce-loop-verdict (Enforce verdict): completed [phase: loop, elapsed: 2s]` then `    Output: LOOP_HEALTH: CONTINUE` | `CONTINUE` |
| `  ✓ step-04-enforce-loop-verdict: completed extra` then `    Output: LOOP_HEALTH: DONE` | `STUCK` |
| `LOOP_HEALTH: DONE` alone, no status lines anywhere | `DONE` |
| step-02 `    Output: LOOP_HEALTH: CONTINUE`, then the real step-04 block with `DONE` | `DONE` |
| a fake step-04 block inside step-02's output, then the real step-04 block with `CONTINUE` | `CONTINUE` |
| a fake step-04 block inside step-02's output, and no real step-04 block | `STUCK` |
| `  ✗ step-04-enforce-loop-verdict: failed` then `    Output: LOOP_HEALTH: CONTINUE` | `STUCK` |
| a different line between the step-04 status line and its `Output:` line | `STUCK` |
| `note: Output: LOOP_HEALTH: CONTINUE` | `STUCK` |
| `      LOOP_HEALTH: CONTINUE` (six spaces) | `STUCK` |
| garbled or missing | `STUCK` |

The prefix is exact: exactly four spaces and `Output: `, or column 0 in a log
with no status lines. It is not `[[:space:]]*`, because the formatter shows
the agent's recent output indented by six spaces.

The remaining risk is the column-0 scan of a log with no status lines. It can
only pick between `CONTINUE` and `DONE` when the evaluator exited `0`, and
amplihack's own runner always prints status lines, so it applies only to
logs from other runners.

### These greps never decide on a non-zero exit

The reader runs only when the evaluator exited `0`. `LOOP_VERDICT` is set to
`STUCK` first, and a non-zero `HEALTH_RC` leaves it there whatever the log
says. Setting `loop_health_enforce: "false"` on the evaluator makes step-04
exit `0` on `STUCK`; its marker then reads `LOOP_HEALTH: STUCK …`, which
matches neither `CONTINUE` nor `DONE`, so the result is still `STUCK`.

## Two absolute prohibitions

1. **Never skip hooks on a commit.** No `--no-verify` and no `-n` shorthand on
   any commit this workflow makes.
2. **Never bypass branch protection.** No `--admin` on `gh pr merge`, and no
   other bypass of required checks, required reviews, or the strict
   up-to-date policy.

If a hook or a check fails, the cause is fixed. This restates the repository
policy in [Merge Flow](merge-flow.md), which already says these two are never
used, and the pre-tool-use hook that blocks the hook-skipping commit flag
outright.

They are enforced three ways, not just documented:

- **Structurally.** `autodrive_merge_gate.sh` builds its `gh` argv as a fixed
  literal list and accepts no flags from any caller, then asserts the argv is
  unchanged before executing. There is no parameter through which a bypass
  could be threaded.
- **In the prompts.** The commit instructions in both round bricks say to fix
  the cause, never to reach for a skip flag.
- **By a guard test.** `forbidden_flags_never_appear_in_an_executable_position`
  scans every recipe `command:` body, every `autodrive_*.sh` tool, and the
  skill, and fails if either flag appears anywhere but a line that explicitly
  marks it as prohibited.

## No silent merge

The merge gate does not trust the loop that preceded it. In the run that
merges, it re-verifies and records:

| Criterion | Source | Failure condition |
| --- | --- | --- |
| Already merged | `gh pr view --json state,mergedAt` | — (short-circuits to success; merged work is never redone) |
| PR open, not draft | `gh pr view` | any other state; `isDraft: true` |
| Merge conflicts | `mergeable`, `mergeStateStatus` | not `MERGEABLE`; `BEHIND`, `DIRTY`, `UNKNOWN` |
| Reviews | `reviewDecision` | `CHANGES_REQUESTED` |
| Review threads | GraphQL `reviewThreads`, **paginated** | any unresolved, not-outdated thread on any page — **or an unreadable answer** |
| CI | `gh pr checks --json name,state,bucket` | any pending or failing check, zero checks, **or an unreadable rollup** |
| qa-team scenarios | evidence file from this run | `qa_status` other than `PASS`, no evidence file, or evidence whose `head_sha` is missing or is not the SHA being merged |
| merge-ready verdict | round record from this run | not `MERGE_READY`, or captured against a different head SHA |

The review-thread query pages. `reviewThreads(first:100)` with no `pageInfo`
follow-up silently truncates: a PR with 101 threads whose only unresolved one
is the last would report zero unresolved and pass the gate. Both readers —
`autodrive_merge_gate.sh` and `autodrive-merge-round.yaml` — use
`gh api graphql --paginate` with `pageInfo { hasNextPage endCursor }` and sum
the per-page counts; a page that does not come back as a number makes the whole
criterion unreadable, which is a blocker.

The qa-team evidence binds to a SHA like everything else. Existence plus
`qa_status: PASS` is not enough: a PASS left behind by an earlier round
describes a tree that is no longer what would be merged. `autodrive-merge-evidence`
records the `head_sha` it measured, and the gate refuses evidence that carries
none or carries a different one.

Every criterion binds to **one** head SHA, the evidence bundle is written to
disk **before** anything is merged, and the merge passes
`--match-head-commit "$HEAD_SHA"` so GitHub itself refuses the merge if the
head moved after the evidence was captured. That closes the check-then-merge
window without a cooperative lease.

**An unreadable criterion is a failure, never a pass.** After the merge, the
platform must confirm `MERGED`; a `gh` success the platform does not confirm is
reported as `NOT_MERGED`.

Exit codes: `0` merged (or already merged), `1` not merged with the blocker
list and the evidence bundle path, `79` terminal policy refusal. The `gh pr
merge` status is captured on its own line rather than inside the `then` of an
`if ! gh …`, where `$?` is the negation's status — always `0` — which would
make the exit-79 branch dead code and report every failure as "exit 0".

## Exit code 79 is terminal

Exit `79` and `BLOCKED_TERMINAL` are final answers from a structural guard
(#1327 / #1332), not infrastructure hiccups. Every child invocation is checked
for them; when one appears the loop stops, surfaces the refusal, and exits `79`
itself so a parent sees a policy refusal rather than a generic failure. It is
**never** retried into — not at a deeper level, and never with a raised
ceiling.

## Recursion context propagation

`autodrive_loop.sh` exports `AMPLIHACK_TREE_ID` and `AMPLIHACK_SESSION_DEPTH`
to every child unchanged, and re-exports the inherited `AMPLIHACK_MAX_DEPTH`
verbatim. `assert_ceiling_untouched` runs before every round and before every
evaluator call; if `AMPLIHACK_MAX_DEPTH` has changed, the loop aborts rather
than continuing under a ceiling it did not inherit.

Because rounds are sequential rather than nested, depth does not grow with the
number of rounds.

## Resumability

A run that dies partway is re-runnable. Nothing merged is redone and no
resolved concern is reopened.

| Store | Path | Role |
| --- | --- | --- |
| Local | `${AMPLIHACK_STATE_DIR:-~/.amplihack/state}/auto-drive/<key>/` | Phase completions, resolved concern ids, round records, evidence bundles. Written only by the host that ran them. |
| Platform | `gh pr view --json state,mergedAt` | The **authority** on whether the PR is merged. |

Local state is a cache, never a claim: the merge gate re-verifies every
criterion regardless of what any state file says, and `UNKNOWN` platform state
is treated as a failure rather than as "not merged".

### There is no pull-request-comment ledger

An earlier revision of this workflow mirrored the local store into a marked PR
comment and rehydrated an empty local store from it, so a different host could
resume. That was removed, deliberately and permanently.

A PR comment is writable by **anyone who can comment on the pull request**, and
the rehydration ran `awk` over the comment body straight into `phases.tsv` and
`resolved-concerns.txt`. A forged comment carrying the marker and a `phases:`
block naming `crusty-loop` made the phase-2 preflight decide the crusty loop
had already completed: the loop was skipped, the phase-completion step never
ran, and nothing downstream detected it — phase 3 re-measures CI, but nothing
re-measures crusty's judgement. The same channel seeded `resolved-concerns`,
which the round prompt tells crusty not to re-raise without new evidence. The
pull fired only when the local store was empty, i.e. precisely on a fresh
host — the normal case for a fleet — and the selector took the *last* matching
comment, so an attacker's comment beat the workflow's own.

Local state plus platform truth cover everything the ledger was for, minus a
fresh-host optimisation. A fresh host redoes a phase; that is a cheaper mistake
than an unauthenticated input into an automated merge authority. Do not
reintroduce it, with or without author authentication.

Resolved concern ids are handed back to crusty on a resumed run. Crusty may
still re-raise one — but only with new evidence in the current diff, and it is
asked to say what that evidence is.

## No short timeouts

No step in any of these recipes declares `timeout` or `timeout_seconds`, and no
recipe declares a `default_step_timeout` (issue #439 — the runner owns the
ceiling). The CI wait polls on a 60-second interval and stops when CI reaches a
terminal state or becomes unreadable; it is bounded by the build finishing, not
by a stopwatch. Test suites, builds, and model calls run to their natural end.
The guard test `no_short_timeouts_anywhere` fails the build if a seconds-scale
or single-digit-minute bound is introduced.

## Files

| File | Lines | Role |
| --- | --- | --- |
| `amplifier-bundle/recipes/auto-drive-to-merge.yaml` | composer | Three phases, then a summary. |
| `amplifier-bundle/recipes/autodrive-build.yaml` | phase 1 | `default-workflow`, resume-aware. |
| `amplifier-bundle/recipes/autodrive-crusty-round.yaml` | round | Crusty review, verdict, fixes, round record. |
| `amplifier-bundle/recipes/autodrive-crusty-loop.yaml` | phase 2 | Loop driver + phase bookkeeping. |
| `amplifier-bundle/recipes/autodrive-merge-evidence.yaml` | evidence | Base sync, `qa-team` run, CI wait. |
| `amplifier-bundle/recipes/autodrive-merge-round.yaml` | round | Merge-ready criteria, verdict, blocker fixes. |
| `amplifier-bundle/recipes/autodrive-merge-loop.yaml` | phase 3 | Loop driver + merge gate + bookkeeping. |
| `amplifier-bundle/tools/autodrive_loop.sh` | tool | The uncapped, agentically-terminated loop driver. |
| `amplifier-bundle/tools/autodrive_merge_gate.sh` | tool | Evidence gate and the fixed merge argv. |
| `amplifier-bundle/tools/autodrive_state.sh` | tool | Resumable local state; platform truth for merged-ness. |
| `amplifier-bundle/skills/auto-drive-to-merge/SKILL.md` | skill | Invocable entry point. |

Every recipe file stays inside the 400-line brick budget.

## Tests

| Test | Location |
| --- | --- |
| Executable contract test — STUCK path, malformed-verdict path, forbidden-flag guard, merge-gate refusals, a step output read through `RECIPE_VAR_` only, the `state_dir` refusals, the static `RECIPE_VAR_` check, and every row of the [loop-health line table](#reading-the-loop-health-line) | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |
| Structural + wiring, including `autodrive_step_outputs_are_read_with_the_recipe_var_fallback` | `tests/integration/auto_drive_to_merge_test.rs` |

```bash
cargo test -p amplihack --test auto_drive_to_merge
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
```

## Dependency on PR #1347

Both loops invoke the `loop-health-evaluator` recipe **by name**. That brick,
the `amplihack orch helper normalise-loop-verdict` helper, and
[its reference](loop-health-evaluator.md) ship with PR #1347 (issue #1337).
They are deliberately **not** reimplemented or copied here — one loop-health
contract, used by every loop that needs one. Until #1347 lands, the two loops
in this workflow cannot resolve their terminator at runtime.

Nothing else in this workflow depends on #1347: the verdicts it parses itself
use only `extract-json` / `extract-field`, which are already on `main`, so the
tests here run and pass without #1347.

## Related references

- [Loop-Health Evaluator](loop-health-evaluator.md) — the agentic terminator
  both loops use (ships with PR #1347).
- [Structured Verdict & Intent Parsing](structured-verdict-parsing.md) — the
  `extract-json | extract-field` pipeline every gate here uses.
- [Merge Flow](merge-flow.md) — the repository's serial, strict-up-to-date
  merge policy, and the prohibition this workflow enforces.
- [Workflow Terminal State](workflow-terminal-state.md) — the `no_merge` flag
  phase 1 sets to keep `default-workflow` from merging.
- [PR-Ownership Lease](pr-ownership-lease.md) — the cooperative alternative to
  the `--match-head-commit` binding used here.
