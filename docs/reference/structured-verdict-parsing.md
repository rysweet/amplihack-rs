---
title: Structured Verdict & Intent Parsing Reference
last_updated: 2026-07-26
review_schedule: quarterly
owner: workflow-team
---

# Structured Verdict & Intent Parsing Reference

The default-workflow recipe family reads every agent verdict, intent, and
outcome signal through the tested `amplihack orch helper` toolchain and through
agent-emitted `parse_json` fields evaluated by engine conditions. There is no
bash text-scraping of agent prose in the control path.

This reference documents the contract that recipe steps and the backing Rust
CLI honour: the `normalise-verdict` helper, the `session-tree register --json`
flag, and the structured fields (`verdict`, `no_merge`, `goal_status`,
`status`, `tree_id`, `depth`) that drive workflow control flow.

## Contents

- [Why this exists](#why-this-exists)
- [The canonical extraction pipeline](#the-canonical-extraction-pipeline)
- [`amplihack orch helper normalise-verdict`](#amplihack-orch-helper-normalise-verdict)
- [`amplihack orch helper normalise-loop-verdict`](#amplihack-orch-helper-normalise-loop-verdict)
- [`amplihack session-tree register --json`](#amplihack-session-tree-register---json)
- [Structured fields by recipe](#structured-fields-by-recipe)
- [Fail-safe guarantees](#fail-safe-guarantees)
- [What is intentionally NOT parsed this way](#what-is-intentionally-not-parsed-this-way)
- [Related references](#related-references)

## Why this exists

Agent output is free text. Three problems used to be solved three different
brittle ways:

- A verdict was read with line-anchored `grep -E '^{.*"verdict"'`, an `awk`
  "last line" heuristic, a `jq` fallback, and an inline `case` synonym block.
- A no-merge directive was guessed from user prose with a regex.
- A goal status was matched by substring `'PARTIAL' in reflection_1` on prose.

Each degraded silently when an agent emitted a slightly different shape or
phrasing. The fix routes every one of these signals through one tested tool
(`orch helper extract-json | extract-field`) plus a small centralised
normaliser, and moves intent/outcome signals into agent-emitted `parse_json`
fields that engine `condition:` expressions read directly.

The parsing mechanism changed. The fail-safe defaults, loud `WARNING` stderr
messages, and issue-referenced defensive branches did not.

## The canonical extraction pipeline

Every recipe step that reads a field from agent output uses this one pattern
(the template lives at `smart-classify-route.yaml`):

```bash
VALUE=$(printf '%s' "$RAW" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field FIELD --default SAFE_DEFAULT)
```

- `extract-json` reads stdin and prints the first complete JSON object it finds,
  trying ` ```json ` fenced blocks, untagged ` ``` ` blocks, then a
  balanced-brace scan over raw prose. It prints `{}` when nothing parseable is
  found.
- `extract-json --require-field FIELD` changes that selection deliberately: it
  collects **every** JSON object in document order and returns the **last** one
  carrying `FIELD`, printing `{}` when none does so the `--default` below
  applies. Use it for anything that decides control flow. First-object-wins is
  fail-OPEN for a verdict — an agent that quotes an example of its own output
  contract, or that drafts one object and reconsiders in prose before emitting
  another, has the wrong object read, and a ` ```json ` fence anywhere in the
  output outranks the unfenced verdict the prompt asked for. Taking the last
  object that carries the field is what "emit the verdict as the very last
  thing" actually means.
- `extract-field --field FIELD --default SAFE_DEFAULT` reads that JSON object
  and prints the string value of `FIELD`, or `SAFE_DEFAULT` if the field is
  absent or the input is not a JSON object.

Verdicts add a normalisation stage to collapse LLM synonyms:

```bash
VERDICT=$(printf '%s' "$RAW" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field verdict --default INSUFFICIENT_EVIDENCE \
  | amplihack orch helper normalise-verdict)
```

### Security: agent output is untrusted data, never code

Agent output is attacker-influenceable text. Route it through the pipeline
above as **data only**:

- Always feed the raw output to the helper via stdin with `printf '%s' "$RAW"`
  (never string-interpolate it into the command line).
- Never `eval`, `source`, or `bash -c` agent output, and never expand it into a
  command position or an indirect/`${!var}` reference.
- Branch on the *canonical token* the helper returns (a fixed allow-list such as
  `WORK_VERIFIED` / `HOLLOW_SUCCESS` / `INSUFFICIENT_EVIDENCE`), not on the raw
  prose. Any unrecognised input already collapses to the safe default, so a
  crafted verdict string cannot smuggle a shell metacharacter into a decision.

## `amplihack orch helper normalise-verdict`

Collapses a free-text or synonym verdict label into one canonical token. It
mirrors the existing [`normalise-type`](#related-references) helper: reads the
label from stdin, prints the canonical token to stdout, exits `0`.

### Synopsis

```
amplihack orch helper normalise-verdict
```

Reads one already-extracted verdict token from stdin (the output of
`extract-field`, not raw agent prose). Matching is case-insensitive **exact-token
equality**: the label must equal one of the tokens below. Anything else —
including negation-adjacent labels and empty input — resolves to the
`INSUFFICIENT_EVIDENCE` default.

### Canonical mapping

| Input token (case-insensitive, exact match)                | Canonical output        |
| ---------------------------------------------------------- | ----------------------- |
| `VERIFIED`, `WORK_VERIFIED`, `SUCCESS`, `APPROVED`, `PASS`, `PASSED` | `WORK_VERIFIED`         |
| `HOLLOW`, `FAILED`, `FAIL`, `NO_WORK`, `NO_ARTIFACTS`, `EMPTY`       | `HOLLOW_SUCCESS`        |
| `INSUFFICIENT`, `INCONCLUSIVE`, `PARTIAL`, `UNKNOWN`, `UNCLEAR`, `NEEDS` | `INSUFFICIENT_EVIDENCE` |
| _(anything else, including empty input)_                   | `INSUFFICIENT_EVIDENCE` |

The default is `INSUFFICIENT_EVIDENCE`: a malformed or unrecognised verdict
neither auto-passes nor hard-aborts a gate. Because matching is exact-token
equality (not containment), negation-adjacent labels such as `UNVERIFIED`,
`NOT_APPROVED`, or `NOT_ACHIEVED` never collide with a pass token — they fall to
the `INSUFFICIENT_EVIDENCE` default. This is the security-critical property from
the design (R2: equality, not containment — `PASS ⊄ PASSED`, `ACHIEVED ⊄
NOT_ACHIEVED`); a `str::contains` implementation would fail **open**.

The mapping reproduces the bash `case` block that previously lived inline in
`workflow-tdd.yaml` (lines 265–268), whose patterns
(`VERIFIED|SUCCESS|APPROVED|PASS|PASSED)` …) are **exact globs** — token
equality, not substring. Implement with an exact-match comparison, **not**
`str::contains`, even though the sibling `normalise_type` uses `contains`:
task-type labels have no negation-adjacent tokens, verdict labels do. The added
synonyms (`FAIL`, `HOLLOW`, `INSUFFICIENT`, `NEEDS`, …) are additive exact tokens
and remain safe under equality. Issue #615 work-verifier behaviour is preserved.

### Examples

```bash
echo "APPROVED" | amplihack orch helper normalise-verdict
# WORK_VERIFIED

echo "FAILED" | amplihack orch helper normalise-verdict
# HOLLOW_SUCCESS
# (input is the extracted token from `extract-field --field verdict`, not a prose sentence)

echo "PARTIAL" | amplihack orch helper normalise-verdict
# INSUFFICIENT_EVIDENCE

echo "UNVERIFIED" | amplihack orch helper normalise-verdict
# INSUFFICIENT_EVIDENCE  (exact-match: does NOT collide with VERIFIED)

printf '' | amplihack orch helper normalise-verdict
# INSUFFICIENT_EVIDENCE
```

### Behaviour contract

- Input: one line on stdin (trailing whitespace trimmed).
- Output: exactly one canonical token followed by a newline.
- Exit code: `0` in all cases; there is no error path for an unrecognised
  verdict — that resolves to the default.
- Unit tests live next to `normalise_type`'s tests in
  `crates/amplihack-cli/src/commands/orch.rs` and cover each synonym cluster,
  the default, and the R2 equality regression: negation-adjacent tokens
  (`UNVERIFIED`, `NOT_APPROVED`, `NOT_ACHIEVED`) must resolve to
  `INSUFFICIENT_EVIDENCE`, never `WORK_VERIFIED` (a substring match would fail
  open).

## `amplihack orch helper normalise-loop-verdict`

The sibling normaliser for **loop-health** verdicts (issue #1337). Same shape —
one already-extracted token on stdin, one canonical token on stdout, exit `0`
always, case-insensitive **exact-token equality** — but a different token set
and, critically, a different fail-safe direction.

```
amplihack orch helper normalise-loop-verdict
```

| Input token (case-insensitive, exact match) | Canonical output |
| ------------------------------------------- | ---------------- |
| `CONTINUE`, `CONTINUING`, `PROCEED`, `KEEP_GOING`, `ANOTHER_ROUND`, `ITERATE` | `CONTINUE` |
| `DONE`, `COMPLETE`, `COMPLETED`, `FINISHED`, `CONVERGED`, `ADVANCE` | `DONE` |
| _(anything else, including empty input)_ | `STUCK` |

> **The two normalisers fail in opposite directions, deliberately.** An
> unreadable *work* verdict resolves to `INSUFFICIENT_EVIDENCE`, which is
> non-fatal — a recipe with real artifacts must never be hard-failed by a
> verifier-formatting bug. An unreadable *loop* verdict resolves to `STUCK`,
> which stops the loop — because the fail-open alternative authorises another
> round of a loop that is already burning budget for nothing. Do not collapse
> them into one shared default.

The R2 equality property matters even more here: `DISCONTINUE`,
`CANNOT_CONTINUE`, `DO_NOT_CONTINUE` and `SHOULD_NOT_CONTINUE` all contain
`CONTINUE`, and `NOT_DONE` contains `DONE`. Under `str::contains` every one of
them would fail **open**. Unit tests live beside `normalise_verdict`'s in
`crates/amplihack-cli/src/commands/orch.rs`.

Full contract: [Loop-Health Evaluator Reference](loop-health-evaluator.md).

## `amplihack session-tree register --json`

`session-tree register` records a session in the session tree and reports the
assigned `tree_id` and `depth`. It supports two output formats:

### Default (text) output — unchanged

```bash
amplihack session-tree register
# TREE_ID=a1b2c3d4 DEPTH=0
```

The `TREE_ID=… DEPTH=…` line is byte-for-byte the historical contract. Both
`smart-classify-route.yaml` and `smart-orchestrator.yaml` still read it, so it
is never removed or reordered.

### Opt-in JSON output

```bash
amplihack session-tree register --json
# {"tree_id":"a1b2c3d4","depth":0}
```

`--json` is additive. When present, `register` emits a single-line JSON object
instead of the text line, letting consumers use the same `extract-field`
pipeline as everywhere else:

```bash
INFO=$(amplihack session-tree register --json)
TREE_ID=$(printf '%s' "$INFO" | amplihack orch helper extract-field --field tree_id --default "")
DEPTH=$(printf '%s' "$INFO" | amplihack orch helper extract-field --field depth --default 0)
```

The `tree_id` charset validation and the `registration_failed` fallback print
are unchanged; only the success-line format is selectable.

## Structured fields by recipe

Each control signal below is either extracted with the pipeline above or
emitted by an agent step as a `parse_json` field and read by an engine
`condition:`.

| Finding | Recipe / step                                   | Signal                | Source                                                                 |
| ------- | ----------------------------------------------- | --------------------- | --------------------------------------------------------------------- |
| A1      | `workflow-tdd.yaml` step-08c work-verifier gate | `verdict`             | `extract-json \| extract-field --field verdict \| normalise-verdict`  |
| A2      | `workflow-pr-review.yaml` step-17a testing gate | `verdict` (+ fail-safe) | JSON verdict via helper; prose `VERDICT: FAILED` retained as fatal token |
| A3      | `workflow-design.yaml` doc-review checkpoint    | `verdict` / `status`  | agent `parse_json`, read via `extract-field` + `normalise-verdict`    |
| B       | `workflow-terminal-state.yaml`                  | `no_merge`            | classifier `parse_json` boolean → `NO_MERGE` env → engine condition   |
| C       | `smart-reflect-loop.yaml`                       | `goal_status`         | reviewer `parse_json` field, `reflection_N.goal_status == '…'`        |
| D1      | `smart-execute-routing.yaml`                    | `status`              | `extract-json \| extract-field --field status --default unknown`      |
| D2      | `smart-classify-route.yaml`                     | `tree_id`, `depth`    | `session-tree register --json` → `extract-field`                      |
| E       | `loop-health-evaluator.yaml`                     | `loop_verdict`        | `extract-json --require-field loop_verdict \| extract-field --field loop_verdict --default STUCK \| normalise-loop-verdict` |
| F1      | `autodrive-crusty-round.yaml`                    | `crusty_verdict`      | `extract-json --require-field crusty_verdict \| extract-field --field crusty_verdict --default CONCERNS` |
| F2      | `autodrive-merge-round.yaml`                     | `merge_ready_verdict` | `extract-json --require-field merge_ready_verdict \| extract-field --field merge_ready_verdict --default NOT_MERGE_READY` |

### A1 — TDD work-verifier gate (`verdict`)

The verifier's JSON verdict flows through the canonical verdict pipeline. The
opt-out guard, the empty-input guard, and the four verdict branches
(`WORK_VERIFIED`, `HOLLOW_SUCCESS`, `INSUFFICIENT_EVIDENCE`, and the fail-safe
default) are unchanged — only the extraction mechanism moved from
grep/awk/jq/case to the helper toolchain.

### A2 — PR-review testing-evidence gate (`verdict` + prose fail-safe)

The testing-evidence gate reads semi-structured prose that may embed **either**
a JSON `"verdict"` **or** a bare prose `VERDICT: FAILED` token:

- **JSON verdict**: extracted via `extract-json | extract-field --field verdict`
  then normalised; `HOLLOW_SUCCESS` / failure is fatal.
- **Prose `VERDICT: FAILED`**: a narrow literal match is **retained** as a
  documented defensive fatal branch (issue #962). This is not the brittle
  mechanism — the removed mechanism was the ad-hoc combined dual-format regex.
  The structured JSON path now goes through the helper; only the prose-failure
  fail-safe token remains.

Contract preserved: prose `VERDICT: FAILED` → exit 1; empty evidence → exit 0
with a visible `WARNING` degrade; populated benign evidence → exit 0.

### A3 — Design doc-review checkpoint (`verdict` / `status`)

The doc-review agent emits `parse_json` `{"verdict": …}` (or `{"status": …}`),
read via `extract-field --field verdict --default NEEDS_ATTENTION` and
normalised. The English-keyword `grep -qiE 'fail|error|cannot|…'` NLU is gone.
Empty output still defaults to `NEEDS_ATTENTION`, the `WARNING` stderr and the
stdout markers are unchanged, and the checkpoint remains non-fatal (issue #834).

> **Note:** `normalise-verdict` has no `NEEDS_ATTENTION` output — an empty or
> unknown doc-review verdict normalises to `INSUFFICIENT_EVIDENCE`. The
> `NEEDS_ATTENTION` marker asserted by `test-bug-834-doc-review-non-fatal.sh`
> must therefore be emitted from the checkpoint's own status/marker path (the
> preserved lines 287–298), independently of the normalised control token. Keep
> the two concerns separate: the marker is presentation, the normalised token is
> control flow.

### B — Terminal-state no-merge intent (`no_merge`)

The classifier (`smart-classify-route.yaml`) emits a `no_merge` boolean in its
`parse_json` output, derived from the task description it already reads. It
threads down as the context var `no_merge` / env `NO_MERGE` that
`workflow-terminal-state.yaml` already reads. Terminal-state consumes the
structured flag via an engine `condition:` (or `[ "$NO_MERGE" = … ]` on the
exported env), never a prose regex. The "only suppress merge, never fabricate"
semantics and the explicit-flag path are unchanged.

Because intent is now a structured field, previously unmatched phrasings — "hold
off merging", "keep it as a draft", "wait for my review" — are honoured by the
classifier instead of silently auto-merging.

### C — Reflect-loop goal status (`goal_status`)

The reviewer steps that produce `reflection_1` / `reflection_2` emit
`parse_json` `{"goal_status": "ACHIEVED|PARTIAL|NOT_ACHIEVED"}`. Loop-control
conditions test the field for equality:

```yaml
condition: "reflection_1.goal_status == 'PARTIAL' or reflection_1.goal_status == 'NOT_ACHIEVED'"
```

Equality on a structured field replaces substring `'PARTIAL' in reflection_1`,
so a stray "partial" in a reviewer's narrative can no longer false-trigger the
loop. The `GOAL_STATUS:` prose lines remain in the prompts as documentation. A
`normalise-goal-status` helper is optional and only added if a reviewer emits
synonyms; equality on the field is the baseline.

### D1 — Execute-routing recursion guard (`status`)

The recursion guard reads `session_info` with the helper instead of
`grep -qE '"status" *: *"ok"'`:

```bash
STATUS=$(printf '%s' "$SESSION_INFO" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field status --default unknown)
[ "$STATUS" = "ok" ] && ...   # ALLOWED / BLOCKED branches unchanged
```

### D2 — Classify-route session tree (`tree_id`, `depth`)

`smart-classify-route.yaml` calls `session-tree register --json` and reads
`tree_id` / `depth` with `extract-field`, replacing
`grep -oE 'TREE_ID=…' / 'DEPTH=…'`. See
[`session-tree register --json`](#amplihack-session-tree-register---json).

## Fail-safe guarantees

Every gate remains fail-closed. The parsing change never weakens these:

- Missing / malformed JSON → the step's documented safe default
  (`INSUFFICIENT_EVIDENCE`, `NEEDS_ATTENTION`, `unknown`, `no_merge=false`).
- `extract-json` on non-JSON prints `{}`; `extract-field` then returns the
  supplied `--default`.
- `normalise-verdict` resolves any unrecognised or empty verdict to
  `INSUFFICIENT_EVIDENCE`.
- `normalise-loop-verdict` resolves any unrecognised or empty loop verdict to
  `STUCK` — never `CONTINUE`. Failing safe on a loop means stopping, not
  spending another round (issue #1337).
- The `WARNING` stderr messages, `continue_on_error` settings, additive
  "suppress-only" gate semantics, and `# issue #NNN` defensive branches are
  preserved byte-for-behaviour.

## What is intentionally NOT parsed this way

These are correct structured-tool usages or format validation, not brittle
agent-prose scraping, and are left unchanged:

- `jq` over `gh --json` output.
- SHA validation (`^[0-9a-f]{40}$`), `git check-ref-format`, and `tree_id`
  charset checks.
- Error-classification greps of `gh` / `git` **stderr** for rate-limit / auth /
  5xx / transient failures.
- Token-redaction `sed` and slug-building `tr`.
- The exact `grep -qF` orchestration sentinel.

## Related references

- [`amplihack orch run`](orch-run-command.md) — native workstream orchestrator
  and the `orch helper` subcommand family (`extract-json`, `extract-field`,
  `normalise-type`, `count-workstreams`, `reclassify-task-type`).
- [Recipe Executor Environment](recipe-executor-environment.md) — how context
  vars and `parse_json` outputs reach bash steps and engine conditions.
- [Doc-Review Non-Fatal Checkpoint](doc-review-non-fatal-checkpoint.md) — the
  non-fatal semantics preserved by finding A3.
- [Workflow Terminal State](workflow-terminal-state.md) — no-merge and
  auto-merge semantics consumed by finding B.
- [Loop-Health Evaluator](loop-health-evaluator.md) — the reusable
  `CONTINUE`/`DONE`/`STUCK` loop-health contract built on this pipeline.
- How-to: [Read agent verdicts with `orch helper`](../howto/parse-agent-verdicts-with-orch-helper.md).
