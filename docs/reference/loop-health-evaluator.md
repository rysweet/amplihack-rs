---
title: Loop-Health Evaluator Reference
last_updated: 2026-10-19
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
- [A usage limit is not a verdict](#a-usage-limit-is-not-a-verdict)
  - [Retry inside the step](#retry-inside-the-step)
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
| `DONE` | The loop's objective is met **and** this round's own verdict (`loop_round_verdict`) is the loop's clean token (`loop_clean_token`). | Advance to the next phase. |
| `STUCK` | No progress is being made. | Stop. Do not proceed. Escalate with the specific evidence of what is not converging. |

### `DONE` needs this round's own verdict to be clean

A loop whose rounds are judged by a reviewer has a clean token: `CLEAN` for
the crusty loop, `MERGE_READY` for the merge-ready loop. The caller passes
that token as `loop_clean_token` and the verdict the reviewer gave **this**
round as `loop_round_verdict`. The prompt's `## Decide` rules make `DONE`
legal only when the two are equal.

A non-clean round (`CONCERNS`, `NOT_MERGE_READY` and the like) is never
`DONE`, even when every finding looks addressed — by a commit or by an edit
to the pull request description. Only another review round can confirm a
fix, so the evaluator answers `CONTINUE` if the round acted on its findings
and `STUCK` only if it neither changed the code nor acted on a finding. A fix
made outside the diff counts as movement **even with zero commits**: a pull
request description edit or a reply on a review thread that the round output
shows is `CONTINUE`, and the re-review is the next step. Issue #1597 is the
case this rule closes: a crusty round said `CONCERNS`, the concerns were fixed
by a description edit with no commit, and the evaluator answered `DONE`.
Without the zero-commit clause, the measured `commits_since_baseline = 0` and
the "if you are not sure, answer `STUCK`" fallback would steer the same round
to `STUCK`, which the loop honours, and the run would end one round short.

The prompt also defines the `done_overridden=1` marker. When `autodrive_loop.sh`
refuses a `DONE` because the round's verdict was not clean, it appends that
marker to the round's line in `loop_history`. The `## Decide` rule tells the
evaluator that a history line ending in `done_overridden=1` is a `DONE` that was
refused as non-clean, so the model does not have to guess what the marker means.

When `loop_clean_token` is empty, the evaluator applies the same rule to the
reviewer verdict shown for this round in `loop_history`, if there is one;
otherwise it decides as before. The quality, self-improvement and
smart-reflect loops pass neither input, so their behaviour does not change.

The rule lives in the prompt only. Steps 03 and 04 do not compare the two
inputs, and nothing about how the verdict is parsed changes: the
deterministic check is `ROUND_CLEAN` in `autodrive_loop.sh`, which handles a
`DONE` that breaks this rule anyway (see
[Advancing needs two independent signals](auto-drive-to-merge.md#advancing-needs-two-independent-signals)).

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

The step-02 prompt states the output contract **twice**, under its first and
its last `##` heading, and shows the same example line both times:

```
{"loop_verdict":"STUCK","not_converging":[]}
```

1. **First**, directly after the `# Loop-health evaluation` title:
   `## OUTPUT CONTRACT`, the first `##` heading in the prompt. Its paragraph
   says the answer ends with one JSON object carrying `loop_verdict` and
   `not_converging`, that the gate reads the LAST such object, and that
   `loop_verdict` is exactly one of `CONTINUE`, `DONE` or `STUCK`. The
   example line follows. The two role paragraphs ("You are the loop-health
   evaluator…") come after the example line, and `{{loop_evidence}}` comes
   after them.
2. **Last**, after `{{loop_last_round_output}}`:
   `## OUTPUT CONTRACT (repeated)`, the last `##` heading in the prompt. It
   gives the full schema, the LAST-object rule and the rule that an
   unrecognised `loop_verdict` is `STUCK` and never `CONTINUE`. The example
   line and the legal-tokens sentence close the prompt, and no `{{…}}`
   placeholder follows the heading:

   ```
   {"loop_verdict":"STUCK","not_converging":[]}

   Only CONTINUE, DONE and STUCK are legal; any other word is STUCK.
   ```

The role paragraphs have no heading of their own, so they sit under
`## OUTPUT CONTRACT`. A separate heading would take the recipe past its
400-line budget.

The example shows the shape of the answer, not a default verdict. It uses
`STUCK` on purpose: an evaluator that copies the example instead of deciding
stops the loop, which is the fail-safe direction. An earlier version showed
`CONTINUE`, and a copied example was a well-formed `CONTINUE` that passed
every gate; the loops have no iteration cap, so that could keep a stalled
loop running.

The order is checked in two places. The shell test reads the step-02 prompt
on its own, not the whole recipe file, and runs three checks:

- `PROMPT-contract-first`: the first `##` heading is `## OUTPUT CONTRACT`,
  and that heading, the first example line, the role line
  (`You are the loop-health evaluator`) and `{{loop_evidence}}` appear in
  that order.
- `PROMPT-contract-last`: the last `##` heading is
  `## OUTPUT CONTRACT (repeated)`, it comes after
  `{{loop_last_round_output}}`, and the last example line comes after it.
- `PROMPT-contract-ends-prompt`: the last non-blank line of the prompt
  contains `any other word is STUCK`, and no `{{` follows the last heading.

Each failure message prints the line numbers it compared. The Rust test
`evaluator_prompt_states_the_contract_first_and_last` checks the same
headings, order and closing lines, so CI enforces them too.

The order exists so that the model finds the contract. It is not a defence
against round output that imitates a verdict; see the residual risk under
[The alternate key is final](#the-alternate-key-is-final).

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
`loop_health` object. Two cases are settled before any source is tried. First,
a [terminal refusal](#exit-code-79-is-terminal) gives `STUCK` with
`verdict_source=terminal_policy_refusal`. That covers evidence showing exit
`79` / `BLOCKED_TERMINAL`, and also evidence that is missing, unparseable or
`null`, because `terminal_refusal` defaults to `true`. Second, empty output
gives `STUCK` with `verdict_source=missing_verdict`. Otherwise step-03 tries
four sources in a fixed order. Each source runs only if every source before it
found nothing.

| Order | Source | What it looks for | `verdict_source` |
| ----- | ------ | ----------------- | ---------------- |
| 1 | Primary key | `extract-json --require-field loop_verdict`: the last JSON object carrying `loop_verdict`. | `evaluator` |
| 2 | Alternate key | `extract-json --require-field verdict` on the **last non-blank line** only: a JSON object carrying `verdict` that ends the output. | `evaluator_alt_key` |
| 3 | Prose token | One line-leading `CONTINUE`, `DONE` or `STUCK` (rules below). | `evaluator_prose_token` |
| 4 | Nothing usable | No token, or conflicting tokens. | `unparseable_verdict` (verdict `STUCK`) |

Every token, from every source, then goes through
[`normalise-loop-verdict`](#amplihack-orch-helper-normalise-loop-verdict) and
the existing `case` guard. Structured data always beats prose, and the
primary key beats the alternate key: a `loop_verdict` object is used even
when the text around it contains a different prose token, and even when a
`verdict` object follows it on the last line.

The prompt is stricter than the gate on purpose. It tells the evaluator a
missing `loop_verdict` is `STUCK` to keep pressure on it to emit the object.
The gate still accepts sources 2 and 3, because #1513 showed evaluators that
answered clearly without it.

### The alternate key is final

The rule applies only when the **last non-blank line** of the output is a JSON
object carrying `verdict`. Then its normalised value is the answer, even if
that value is unknown. `{"verdict":"MAYBE"}` gives `STUCK` with
`verdict_source=evaluator_alt_key`; step-03 does not go on to scan the prose
for a second answer. The evaluator did answer, so looking for a different
answer elsewhere would only add ways to fail open. When the object has no
`not_converging` array, it defaults to `[]`.

A `verdict` object anywhere else in the output is quoted evidence and is
ignored. That matches what the prompt tells the evaluator: quoting the
evidence back is safe, and objects without a `loop_verdict` do not count. The
round output the evaluator reads holds PR comments and CI logs. Reading the
last `verdict` object from anywhere would let a quoted
`{"verdict":"CONTINUE"}` from them beat the evaluator's own `STUCK` and keep
the loop running.

A pretty-printed or fenced `verdict` object is not one line, so it is not
read. It falls through to the prose scan, and with no clear prose token the
result is `STUCK`, which is what every `verdict` object gave before #1513.

**Residual risk:** output that *ends* with a quoted object is still read as
the answer. If the evaluator gives no answer of its own and its last line is a
`verdict` object copied from the round output, that object is its verdict.
This is the same risk as a `loop_verdict` object planted in the round output:
nothing in the gate can tell the two apart.

### Prose token rules

The prose scan accepts a verdict token only when there is no doubt about it:

- Lines inside a fenced code block (between two ```` ``` ```` lines) are
  dropped first, so a quoted example is never read as the answer.
- The token is at the **start of a line**, after optional leading whitespace.
- It may follow one optional prefix: a markdown heading (`#`, `##`, …), then
  `Verdict:` or `LOOP_HEALTH:`. The prefixes match in any case.
- Markdown bold around the prefix or the token is allowed:
  `**Verdict: CONTINUE**`, `**Verdict:** CONTINUE` and `**DONE**` all match.
  Only `*` is allowed this way; a `>` quote is still rejected.
- A leading `*` is read the same as bold, so a `* CONTINUE` bullet matches
  and a `- CONTINUE` bullet does not.
- Only the words `Verdict` and `LOOP_HEALTH` count as a prefix. A bold
  heading such as `**What would make the next verdict STUCK:**` does not
  match, because the line starts with `What`.
- The token itself must be **upper case**: `CONTINUE`, `DONE` or `STUCK`.
  "We should continue" in ordinary English never matches. After a
  `Verdict:` or `LOOP_HEALTH:` prefix the token may be in any case, so
  `## Verdict: continue` matches.
- After the token (and any closing `*`) there may be whitespace, and then
  the line must end or the next character must be `:`, `.`, `–` or `—`. A
  hyphen counts only after whitespace (`DONE - all clear`); a hyphen
  straight after the token (`DONE-ish`) does not match. So `DISCONTINUE`,
  `NOT_DONE`, `CONTINUED` and `DONE-ish` never match.
- A token followed by an ordinary word is a sentence, not a verdict, and
  does not match. `CONTINUE is not warranted; the loop has stalled.` and
  `DONE is premature: two blockers remain.` both give `STUCK` with
  `verdict_source=unparseable_verdict`. This is the rule proposed in the
  issue #1513 discussion; a plain space after the token is not an ending.
- A legal ending does not rescue a negated qualifier. When the text after
  the ending, with leading punctuation and spaces removed, starts with the
  word `not`, `no` or `never` (any case), a `CONTINUE` or `DONE` line counts
  as a conflicting token, so the whole answer is `STUCK` with
  `verdict_source=unparseable_verdict`, whatever other lines say. It is not
  simply skipped, so it can never leave a different token standing alone. A
  `STUCK` line keeps its token (`STUCK — no diff`, `STUCK — not converging`):
  either reading stops the loop, and next to a `CONTINUE` line it is still a
  conflict. `CONTINUE: not warranted`, `DONE: not yet`,
  `CONTINUE -> not really`, `**DONE:** never reached` and
  `DONE: no blockers remain` all give `STUCK`; the last one is a false
  positive that stops the loop, which is the fail-safe direction. Only whole
  words count: `DONE: nothing left to fix` and `CONTINUE — notable progress`
  still match.
- Later in the line a contradiction is caught too, weighted by which mistake
  is dangerous. A `DONE` line is a conflicting token when the rest of the
  line contains the whole word `not`, `never`, `cannot`, `continue` or `stuck`, or any
  `n't` contraction: `DONE: the loop is not done yet`,
  `DONE — wait, CI is red, so this is STUCK really` and
  `DONE: the review isn't clean` all give `STUCK`. A `CONTINUE` line is a
  conflicting token only when the rest of the line contains the whole word
  `done` or `stuck` (`CONTINUE: pointless, the loop is stuck`), because a
  reason to continue is often negative: `CONTINUE — CI is not green yet` still
  gives `CONTINUE`. A later `no` is not a negation for either token, so
  `DONE: all clear, no blockers remain` gives `DONE`. Other negations later in
  the line (`CONTINUE — the loop made no progress`) are not detected; the
  prose path is a fallback, and the prompt asks for JSON.
- A line quoted with `>` does not match; the token must not follow any other
  character.
- The same token on several lines counts once.
- Two or more **different** tokens give `STUCK` with
  `verdict_source=unparseable_verdict`.

The rule as an extended regular expression, matched under `LC_ALL=C`. The
em-dash and en-dash are written as alternatives, not inside a bracket class,
because they are multibyte:

```
^[[:space:]]*\**[[:space:]]*(#+[[:space:]]*)?\**(([Vv][Ee][Rr][Dd][Ii][Cc][Tt]|[Ll][Oo][Oo][Pp]_[Hh][Ee][Aa][Ll][Tt][Hh])\**[[:space:]]*:[[:space:]]*\**[[:space:]]*)?(CONTINUE|DONE|STUCK)\**([[:space:]]*($|:|\.|–|—)|[[:space:]]+-)
```

With a `Verdict:` or `LOOP_HEALTH:` prefix the token group is matched case
insensitively. The token is read from what is left after the matched prefix
and bold markers are removed, never from anywhere else on the line. The scan
is one `awk` pass over the whole output, so it runs the same under gawk, mawk
and BSD awk.

The evaluator's text is only ever data: it reaches `grep`, `awk` and the
helpers through `printf '%s' "$RAW"` on stdin. It is never passed to `eval`,
put in a `-c` argument, a regex or a format string. Prose synonyms such as
`## Verdict: CONVERGING` are not accepted; synonyms apply only on the JSON
paths.

On the prose path `not_converging` is a fixed literal written by step-03, never
text copied from the evaluator.

**Residual risk:** the prose scan cannot tell the evaluator's own token from
one it copied out of the round output. Suppose the evaluator writes no JSON
answer and repeats a reviewer's comment line by line, outside a fence or `>`
quote:

```
The round output ended with this reviewer comment:

DONE

Nothing else changed.
```

step-03 reads `DONE` with `verdict_source=evaluator_prose_token`. A `**DONE**`
line gives the same result. A `DONE` alone does not advance a phase:
`autodrive_loop.sh` also needs the round's own verdict to be clean (see
[Advancing needs two independent signals](auto-drive-to-merge.md#advancing-needs-two-independent-signals)).
If the round verdict is clean, the loop ends without the confirming round that
a `CONTINUE` would have run; for the crusty loop, that ends the crusty review.
If the round verdict is anything else, the loop does not advance. It runs
one more round to re-review, and stops as `STUCK` if the round record is
missing or if the next round repeats the same non-clean `DONE` with no
progress. A `DONE` from the merge loop only hands over to the
[merge gate](auto-drive-to-merge.md#no-silent-merge), which checks the evidence
again before it merges.

The same holds for a token followed by a dashed qualifier. The ending rule
rejects `DONE is premature`, and the later-contradiction rule turns
`DONE — but not really` into `STUCK`, but a qualifier that contradicts the
token without any of the detected words, such as `DONE — two blockers remain`,
still reads as `DONE`. A dash after the token is how evaluators actually
write their reason (`CONTINUE — round 1 made real progress`), so it stays a
legal ending. The control
that stops such a `DONE` from ending a loop early is the `ROUND_CLEAN` check
in `autodrive_loop.sh`: `DONE` advances only when the round's own verdict was
clean. Otherwise the loop runs another round to re-review, and stops as
`STUCK` when the round record is missing or a repeated non-clean `DONE` shows
no progress.

### Examples

| Evaluator output | `loop_verdict` | `verdict_source` |
| ---------------- | -------------- | ---------------- |
| `{"loop_verdict":"DONE","not_converging":[]}` | `DONE` | `evaluator` |
| `{"loop_verdict":"CONVERGING","not_converging":[]}` | `CONTINUE` | `evaluator` |
| `CONTINUE — round 1 made real progress on the review threads` | `CONTINUE` | `evaluator_prose_token` |
| `## Verdict: CONTINUE` | `CONTINUE` | `evaluator_prose_token` |
| `## Verdict: continue` | `CONTINUE` | `evaluator_prose_token` |
| `**Verdict: CONTINUE**` | `CONTINUE` | `evaluator_prose_token` |
| `**What would make the next verdict STUCK:**` on one line, `CONTINUE — progress` on another | `CONTINUE` | `evaluator_prose_token` |
| `LOOP_HEALTH: DONE` | `DONE` | `evaluator_prose_token` |
| `{"verdict":"CONVERGING"}` | `CONTINUE` | `evaluator_alt_key` |
| `{"verdict":"MAYBE"}` | `STUCK` | `evaluator_alt_key` |
| `{"verdict":"MAYBE"}` followed by a `CONTINUE` line | `CONTINUE` | `evaluator_prose_token` |
| A `CONTINUE` line followed by `{"verdict":"NOT_CONVERGING"}` | `STUCK` | `evaluator_alt_key` |
| A quoted `{"verdict":"CONTINUE"}`, then `STUCK — nothing moved.` | `STUCK` | `evaluator_prose_token` |
| `{"verdict":"CONTINUE"}` then the mistyped `{"loop_verdict ":"STUCK"}` | `STUCK` | `unparseable_verdict` |
| `{"loop_verdict":"DONE"}` then `{"loop_verdict":"STUCK"}` | `STUCK` | `evaluator` |
| `The loop looks healthy and should probably keep going.` | `STUCK` | `unparseable_verdict` |
| `DISCONTINUE` | `STUCK` | `unparseable_verdict` |
| `CONTINUE` on one line and `STUCK` on another | `STUCK` | `unparseable_verdict` |
| `DONE.` on two separate lines | `DONE` | `evaluator_prose_token` |
| `CONTINUE is not warranted; the loop has stalled.` | `STUCK` | `unparseable_verdict` |
| `DONE is premature: two blockers remain.` | `STUCK` | `unparseable_verdict` |
| `DONE-ish, mostly` | `STUCK` | `unparseable_verdict` |
| `I cannot CONTINUE` | `STUCK` | `unparseable_verdict` |
| `CONTINUE: not warranted, the loop has stalled` | `STUCK` | `unparseable_verdict` |
| `DONE: not yet` | `STUCK` | `unparseable_verdict` |
| `DONE: the loop is not done yet, two concerns remain.` | `STUCK` | `unparseable_verdict` |
| `CONTINUE: pointless, the loop is stuck.` | `STUCK` | `unparseable_verdict` |
| `CONTINUE — CI is not green yet, one more round` | `CONTINUE` | `evaluator_prose_token` |
| `STUCK — not converging` on one line, `CONTINUE` on another | `STUCK` | `unparseable_verdict` |
| `DONE: nothing left to fix` | `DONE` | `evaluator_prose_token` |
| `continue reading below` | `STUCK` | `unparseable_verdict` |
| `> CONTINUE` | `STUCK` | `unparseable_verdict` |
| `DONE` only inside a ```` ``` ```` fenced block | `STUCK` | `unparseable_verdict` |
| *(empty output)* | `STUCK` | `missing_verdict` |
| `{"verdict":"DONE"}` quoted, then `{"loop_verdict":"CONTINUE","not_converging":[]}` | `CONTINUE` | `evaluator` |
| Prose `STUCK` line followed by `{"loop_verdict":"CONTINUE","not_converging":[]}` | `CONTINUE` | `evaluator` |
| `{"loop_verdict":"CONTINUE","not_converging":[]}`, then `{"verdict":"DONE"}` as the last line | `CONTINUE` | `evaluator` |
| `{"loop_verdict":"CONTINUE","not_converging":[]}`, then a `DONE` line | `CONTINUE` | `evaluator` |
| `{"loop_verdict":"STUCK","not_converging":[]}`, then a `CONTINUE` line | `STUCK` | `evaluator` |

When nothing usable is found, step-03 prints a `WARNING` on stderr that says
which case it hit (no token, or which tokens conflicted) and sets a one-item
`not_converging` explaining why. The warning never prints the evaluator's raw
output.

### Verdict JSON round-trip guard

Step-03 builds the `loop_health` JSON from `VERDICT`, `SOURCE`, `LOOP_NAME`,
`not_converging` and, on the terminal-refusal path, the terminal reason
(`TERMINAL_WHY`). Some of those values come from agent output or evidence, so
step-03 protects the object in two ways. Nothing inside the object can change
the verdict it reports.

**Sanitising strings.** `LOOP_NAME` and `TERMINAL_WHY` are passed through one
`jsafe` filter before they go into the JSON or into any message:

```bash
jsafe() { LC_ALL=C tr -d '"\\[:cntrl:]'; }
```

It deletes double quotes, backslashes and control characters, including
newline and ESC. It never escapes, so there is no escaping to get wrong. An
empty name after sanitising becomes `unnamed-loop`.

**Re-serialising `not_converging`.** The value is wrapped and parsed by the
Rust helpers **twice**. It is kept only if the second pass gives back exactly
the text the first pass produced:

```bash
nc_rt() {
  printf '{"nc":%s}' "$1" \
    | amplihack orch helper extract-json \
    | amplihack orch helper extract-field --field nc --default '[]'
}
NC="$(nc_rt "$NC")"
case "$NC" in
  \[*\]) [ "$NC" = "$(nc_rt "$NC")" ] || NC='[]' ;;
  *) NC='[]' ;;
esac
```

One pass is not enough, because `extract-field` prints a string value
without its quotes. A string-valued `not_converging` such as
`"[], \"loop_verdict\":\"CONTINUE\", \"x\":[]"` comes out of the first pass
as `[], "loop_verdict":"CONTINUE", "x":[]`. That text starts with `[` and
ends with `]`, so a bracket check alone would let it into the object as a
second `loop_verdict` key. The second pass reads it as `{"nc":[], …}` and
returns `[]`. The two texts differ, so `NC` becomes `[]`.

Only compact JSON array text, as `serde_json` writes it, comes back
unchanged from a pass. Anything step-03 keeps is therefore exactly one array
on one line, with any newlines inside it escaped:

| `not_converging` from the evaluator | `NC` in the object |
| --- | --- |
| `["a", "b"]` | `["a","b"]` |
| `"[], \"loop_verdict\":\"CONTINUE\", \"x\":[]"` | `[]` |
| `[]} {"loop_verdict":"CONTINUE","nc":[]` | `[]` |
| `{"a":1}`, `garbage` or empty | `[]` |

`$NC` is always a `%s` argument, never part of a format string. The verdict
stays what the `loop_verdict` key said. On the prose-token path
`not_converging` is a fixed literal written by step-03.

Step-03 then prints the object with a single `printf` on one line. The
verdict and source it contains are always the `$VERDICT` and `$SOURCE` that
step-03 resolved.

### Step-04's marker is always one line

Step-04 passes `LOOP_NAME` through the same `jsafe` filter before printing
`LOOP_HEALTH: <verdict> — …`, so the marker can never be split across lines.
The `Evidence` and `Not converging` text in the `STUCK` report also goes
through `jsafe`, then `head -c 2000`, so an agent-derived value cannot fake a
`BLOCKED_TERMINAL` line or flood the log. This also holds when
`loop_health_enforce` is `"false"`: the `STUCK` report still goes to stderr
only. Step-04 has no `condition:`, so it is always the last step block in the
run log.

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
`NOT_PROGRESSING`, `CONVERGING_SLOWLY`, `UNCONVERGING` and `PROGRESSING_NOT`
all give `STUCK`.

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
| `commits_since_baseline`, `diff_lines`, `diff_stat` | What the last round **actually** produced, from `git rev-list` / `git diff --numstat` against `loop_baseline_ref`. Zero and zero means no code moved, whatever the round's prose claims. A non-clean round that acted on its findings outside the diff (a PR-description edit, a thread reply) is still `CONTINUE`; see the `DONE` rule. |
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
| `loop_agent_working_dir` | `.` | Where the `step-02` analyzer agent starts. Kept separate from `loop_repo_path` so a caller that measures the tree under judgement (auto-drive passes the pull request worktree) can start the agent elsewhere: the agent CLI loads `AGENTS.md`, `CLAUDE.md` and `.claude/` hooks from its working directory, and the judged tree must not instruct its judge (issue #1528). |
| `loop_baseline_ref` | `""` | Git ref at the start of the round just finished. Falls back to the working-tree diff with a visible `WARNING` when unresolvable. |
| `loop_child_exit_code` | `""` | Exit code of the last child. `79` is a terminal policy refusal. |
| `loop_findings_current` / `loop_findings_previous` | `""` | Newline-separated finding identifiers for the last two rounds. |
| `loop_test_signal` / `loop_test_signal_previous` | `""` | Test result summaries for the last two rounds. |
| `loop_ci_signal` / `loop_ci_signal_previous` | `""` | CI result summaries for the last two rounds. |
| `loop_clean_token` | `""` | The verdict that makes a round clean for this loop: `CLEAN` (crusty) or `MERGE_READY` (merge-ready). `DONE` is legal only when `loop_round_verdict` equals it. Empty means: use the reviewer verdict for this round in `loop_history` if there is one, otherwise decide as before. |
| `loop_round_verdict` | `""` | The reviewer's verdict for the round just finished, for example `CONCERNS` or `MERGE_READY`, or `MISSING` when the round wrote no record. `autodrive_loop.sh` passes it with every character outside `A-Za-z0-9_` removed; a non-clean verdict that would then equal the clean token (`CLEAN!`) arrives as `CLEAN_NONCANONICAL`. |
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
| `evaluator_alt_key` | No `loop_verdict` object; the evaluator emitted a `verdict` object as its last non-blank line instead. |
| `evaluator_prose_token` | No JSON verdict; one clear line-leading token was found in the prose. |
| `terminal_policy_refusal` | The evidence showed exit `79` / `BLOCKED_TERMINAL`, or was missing or unparseable (`terminal_refusal` defaults to `true`); the evaluator was skipped. |
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
`CONTINUE`. Only the guard's own output counts: an agent that quotes the token
or the exit code is not refusing anything (see
[Agent output lines are not refusals](#agent-output-lines-are-not-refusals)).

Historically, agents read a `79` refusal as an infrastructure fault and retried
one level deeper with a raised ceiling. That is what #1327 sealed, and it is
what this branch refuses to re-open.

### Agent output lines are not refusals

A line that only quotes the refusal is not one. Agent output reaches the
round log as `[HH:MM:SS] [amplihack:...]` lines, and a crusty review of
PR #1586 that explained the stop rule ("the loop only stops for good on the
policy-refusal exit code or BLOCKED_TERMINAL") once stopped the loop as a
terminal policy refusal (#1510).

`step-01-collect-loop-evidence` therefore applies the same rule as
`terminal_refusal()` in `amplifier-bundle/tools/autodrive_loop.sh`, all of
it, so the two detectors agree on which logs are refusals:

1. Before either text detector runs, it removes text that comes from an input
   rather than from the guard. The awk and the ERE are byte-identical in both
   files:
   - the runner's run summary, from its header to the end of the last
     round's output (the awk). See
     [The runner's run summary is not guard output](#the-runners-run-summary-is-not-guard-output);
   - agent output lines, `[HH:MM:SS] [amplihack:...] ...` (the ERE);
   - the runner's `amplihack.recipe.log_pointer {...}` bookkeeping lines
     (`crates/amplihack-cli/src/commands/recipe/run/correlation.rs`), which
     echo the run's `task_description` (the ERE). See
     [Runner bookkeeping lines are not refusals](#runner-bookkeeping-lines-are-not-refusals).
2. The token check requires the guard's full marker,
   `BLOCKED_TERMINAL orchestration_unavailable`. The bare word
   `BLOCKED_TERMINAL` is not a refusal.

The first rule:

```bash
{ printf '%s\n' "$HIST"; printf '%s\n' "$LAST" | awk 'NR > 1 { if (prev ~ /^Recipe: / && $0 ~ /^(Run ID|Status): /) { cut = 1; exit } print prev } { prev = $0 } END { if (NR > 0 && !cut) print prev }'; } \
  | grep -vF "$SELF" > "$WORK/terminal_input" || true
grep -vE '^[[:space:]]*(\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:|amplihack\.recipe\.log_pointer \{)' "$WORK/terminal_input" > "$WORK/terminal_blob" || true
```

Both the `BLOCKED_TERMINAL orchestration_unavailable` check and the
`(exit|status|code)…79` check read `$WORK/terminal_blob`.

The second rule is needed because the first cannot see every agent line. When
an agent step fails, the recipe runner reprints the agent's stdout without the
prefix, indented under `--- stdout (tail) ---`. The summary cut drops it when
it is printed inside the run summary; the full-marker rule holds wherever it
is printed. A review that names the token and then fails, for instance on a
session limit, would otherwise stop the loop for the reason #1510 describes,
while `autodrive_loop.sh` would not. The duration, still-running and failure-class checks
keep reading the unfiltered `$WORK/blob`, so an agent line reporting
`step took 10m 0s` still counts toward `repeated_duration_count`.

| Round text | `terminal_refusal` | `terminal_reason` contains |
| ---------- | ------------------ | -------------------------- |
| `[10:13:17] [amplihack:claude:3] … or BLOCKED_TERMINAL (see terminal_refusal in autodrive_loop.sh)` | `"false"` | — |
| `[10:13:18] [amplihack:claude:3] the guard returns exit 79 on refusal`, in `loop_last_round_output` or `loop_history` | `"false"` | — |
| `    --- stdout (tail) ---` then `    the loop only stops on BLOCKED_TERMINAL` (runner failure tail, unprefixed) | `"false"` | — |
| `child returned BLOCKED_TERMINAL at depth 4/3` (bare token, no marker) | `"false"` | — |
| `BLOCKED_TERMINAL orchestration_unavailable: nested orchestration refused`, at column 0, after spaces, or after a tab | `"true"` | `BLOCKED_TERMINAL reported by a child` |
| `child exited with status 79` (no prefix) | `"true"` | `exit code 79 present in round output` |
| `loop_child_exit_code` = `79`, with any text or none | `"true"` | `child process exited 79` |
| `[loop-health-evaluator] child process exited 79` | `"false"` | — ([self-poisoning](#the-brick-does-not-poison-itself)) |
| `amplihack.recipe.log_pointer {…"task_description":"… BLOCKED_TERMINAL orchestration_unavailable … exit 79 …","exit_code":79…}`, at column 0 or indented | `"false"` | — |
| the same log_pointer line, then `BLOCKED_TERMINAL orchestration_unavailable: …` on its own line | `"true"` | `BLOCKED_TERMINAL reported by a child` |
| a real round log whose run summary reprints a review starting `CLEAN: echoed BLOCKED_TERMINAL orchestration_unavailable and exit 79 are ignored.` at column 0, and step-01 output echoing `resolved_concerns` id `exit-79-regex-reads-unprefixed-prose` | `"false"` | — |
| the same log with `BLOCKED_TERMINAL orchestration_unavailable: …` on its own line before the summary | `"true"` | `BLOCKED_TERMINAL reported by a child` |
| `Recipe: x` then `Run ID: 1` (the summary header), then `BLOCKED_TERMINAL orchestration_unavailable: …` | `"false"` | — |
| `Recipe: x` not followed by `Run ID:`/`Status:` at column 0, then `BLOCKED_TERMINAL orchestration_unavailable: …` | `"true"` | `BLOCKED_TERMINAL reported by a child` |

Rules that keep real refusals visible:

- `loop_child_exit_code` = `79` is checked first and reads no text. It is the
  guard's structural signal, and no filter can hide it.
- The summary cut needs the exact header pair: `Recipe: ` at column 0 and,
  on the very next line, `Run ID: ` or `Status: ` at column 0. An indented,
  lower-case, split or unpaired `Recipe:` line cuts nothing. It is applied to
  `loop_last_round_output` only; `loop_history` holds round records, never a
  runner summary, and is not cut.
- The line filter matches only those two exact line starts: no case folding, no
  partial timestamps, no other tags, and no JSON other than a line that
  begins with `amplihack.recipe.log_pointer {`. An unprefixed guard line is
  always seen, and so is a guard line echoed inside any other JSON.
- Every guard refusal in `crates/amplihack-cli/src/commands/recipe/run/execute.rs`
  begins with `BLOCKED_TERMINAL orchestration_unavailable:`, so requiring the
  full marker loses no real refusal. A false stop is not a safe direction: it
  is the #1510 failure, and it ended the PR #1586 drive.
- `|| true` is there for a log made only of agent lines. In that case
  `grep -v` leaves nothing and exits with status 1. `|| true` keeps that
  status 1 from counting as a failure, so `terminal_blob` is just empty and no
  refusal is reported.

### Runner bookkeeping lines are not refusals

Round 1 of PR #1632 stopped as a terminal policy refusal although no guard
had refused. The recipe runner writes two `amplihack.recipe.log_pointer`
lines per run to stderr, an `early` and a `final` event, each a single-line
JSON object after that prefix. Each carries the run's `task_description`.
The task for #1510 names the full marker and "exit 79", so the runner's
own bookkeeping put both into the round log, unprefixed, and both detectors
read them as the guard.

The shared ERE drops those lines, at column 0 or indented, before the text
checks run. A `final` log_pointer line also carries the child's `exit_code`.
When that is `79`, it is dropped with the rest of the line. **This is
deliberate.** The exit code of the round itself is the structural signal:
`$1` of `terminal_refusal()` in `autodrive_loop.sh`, and
`loop_child_exit_code` here. Both are checked before any text is read, and
no input can fake them. Text matching is only a fallback for a refusal whose
exit code was lost, and a fallback that inputs can trip is worse than none.

### The runner's run summary is not guard output

Round 3 of PR #1632 found a third input-echo path, and this one is in every
successful round log. After a run, `amplihack recipe run` prints a summary
(`format_recipe_run_table` in
`crates/amplihack-cli/src/commands/recipe/run/format.rs`):

```text
Recipe: autodrive-crusty-round
Run ID: 5cc1a446-…
Status: ✓ Success
Log pointer: status=success … exit_code=0 …

Steps:
  ✓ step-01-round-context: completed
    Output: {"head_sha":"…","pr":"1632","resolved_concerns":"exit-79-regex-reads-unprefixed-prose",…}
  ✓ step-02-crusty-review: completed
    Output: ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). …
CLEAN: echoed BLOCKED_TERMINAL orchestration_unavailable and exit 79 are ignored.
```

Each step's `    Output: ` line carries the first 200 characters of that
step's output. An agent step's output is multi-line, and its first line is
usually the `ℹ NODE_OPTIONS` notice, so the agent's own prose follows **at
column 0, with no prefix**. `step-01-round-context` echoes the ids of
earlier concerns there too, and an id like
`exit-79-regex-reads-unprefixed-prose` matches the exit-79 regex. The summary
is the last thing in the log, so it is always inside the 20000-byte tail.
A clean round whose review opened by summarising these detectors would stop
as a terminal policy refusal.

The summary is not filtered line by line. Both detectors cut the log at the
summary header, a `Recipe: <name>` line followed by a `Run ID: ` or
`Status: ` line, and read nothing after it. Whatever else the summary echoes,
now or later, is cut with it. This loses no refusal:

- the guard refuses before a run starts. A refused `amplihack recipe run`
  prints its `BLOCKED_TERMINAL orchestration_unavailable:` line to stderr and
  exits 79, with no summary at all;
- the round recipes run their sub-recipes inside the runner, not through a
  nested `amplihack recipe run`, so no guard refusal is reprinted in a step's
  `Error:` or `Recent stderr:` lines;
- a refusal an agent meets inside its own step reaches the log through the
  agent's prefixed stream, which was never read (see below).

`autodrive_round_tail.sh` keeps the header in view. If a summary is longer
than the 20000-byte window, the window starts inside it and would lose the
header. When the log has a summary header and the tail does not, the script
prints the header's two lines first, so the collector cuts the whole tail,
all of which is summary.

Known limits, accepted rather than patched with heuristics:

- A real guard refusal that reaches the log **only** through an agent's
  prefixed stream, or only inside the run summary, is not seen by the text
  check. The exit-code check still catches a refusal of the round itself, and
  `autodrive_loop.sh` makes the same trade-off.
- Agent prose at column 0 **before** the summary that forges the header pair
  (`Recipe: x`, then `Status: …`) cuts the log early. That can only hide text,
  never invent a refusal, and the only guard line that reaches the log
  unprefixed before the summary belongs to a run that never started and so
  exited 79.
- The exit-79 regex still reads unprefixed text before the summary. Agent
  prose that says "exit 79" and reaches the log there without a prefix (a
  multi-line message's continuation lines) is still read as a refusal. The
  full-marker rule closes this path for the token, not for the regex.
  `loop_child_exit_code` = `79` remains the authoritative signal; the text
  checks are a fallback for a refusal whose exit code was lost.
- Outside the summary, runner bookkeeping lines are recognised by one key
  only, `amplihack.recipe.log_pointer`. The input-echo class is handled by
  region, not by shape, only from the summary header on. Before it, a new
  runner line that echoes an input and names the full marker or `exit 79`
  would still read as a refusal; it must be added to the shared ERE, in both
  files, with a test. Today the log_pointer line is the only such line before
  the summary.
- `loop_last_round_output` is a byte tail of the round log. `autodrive_loop.sh`
  builds it with `autodrive_round_tail.sh`, which drops the partial first line
  when the log is longer than 20000 bytes, so a fragment that has lost its
  `[HH:MM:SS]` prefix never reaches the detectors, and which keeps the summary
  header as described above. A caller that builds `loop_last_round_output`
  some other way must do the same.

## A usage limit is not a verdict

The evaluator's agent step (step-02) is a Claude call like any other, so it
can fail on a usage limit. A usage limit says nothing about whether the loop
is converging, so it never becomes a `STUCK` verdict.

### Retry inside the step

Step-02 runs through `amplihack claude -p`, so its supervisor applies the
[usage-limit wait](profile-command.md#usage-limit-wait) before the recipe
runner sees the failure:

1. The first attempt fails on a usage limit.
2. The supervisor waits. A limit that resets within five hours gets a reset
   wait. A weekly limit, or one with no readable reset, gets a
   [capacity wait](profile-command.md#capacity-wait) when another stored
   login could take over and a rotator is running, for up to six hours by
   default. Inside a recipe run `AMPLIHACK_RECIPE_RUN_ID` stops launches
   from starting the rotator, so start one before auto-drive; without one
   the step fails at once on the usage limit.
3. The wait ends on `rotator capacity`, `login changed`, `login capacity`
   or `reset reached`, and step-02 runs once more with the same prompt under
   whichever login is now live.
4. The second attempt answers as usual. Step-02 exits `0`, and step-03 and
   step-04 resolve and enforce its verdict exactly as if the limit had never
   happened.

The retry happens inside the step, so `loop-health-evaluator.yaml` is
unchanged. The caller sees one completed evaluator run with an ordinary
`LOOP_HEALTH:` line, and the step's stderr holds the
`amplihack.recipe.usage_limit_wait` `start`, `end` and `resume` lines. There
is at most one wait and one retry per step.

### When the wait does not lift the limit

When the wait is skipped (for example `reset_too_far` with a single stored
login), expires (`capacity expired`, `step timeout`) or was already used,
step-02 fails and the recipe runner writes its
`amplihack.recipe.failure_class` marker with `"class":"usage_limit"`.
Step-02 does not set `continue_on_error`, so the runner stops the recipe
there: step-03 and step-04 do not run and no verdict is produced. The
recipe's shell test checks that step-02 never gains `continue_on_error`,
because step-03 would then resolve the missing verdict as `STUCK` with
`verdict_source=missing_verdict`. The caller does not read the failed recipe
as `STUCK`:

- `autodrive_loop.sh` stops as `USAGE_LIMIT`, exit `1`; see
  [Usage limits are not STUCK](auto-drive-to-merge.md#usage-limits-are-not-stuck);
- `hook_repair_evaluate` reports `USAGE_LIMIT` and the repair ends with
  status `usage_limit`; see
  [`hook_repair_run`](recipe-commit-hook-repair.md#hook_repair_run).

The verdict rules in this recipe are unchanged. A missing or unparseable
verdict from an evaluator that **ran** is still `STUCK`. Only the runner's
own marker, on a non-zero exit, counts as a usage limit: an evaluator or a
round agent that writes about usage limits, or quotes the marker in its
output, does not change the verdict.

## Fail-safe guarantees

Every branch fails toward stopping:

- Missing evaluator output → `STUCK` (`verdict_source=missing_verdict`).
- Unparseable evaluator output → no `loop_verdict` object, no `verdict`
  object on the last non-blank line, and no single clear prose token →
  `STUCK` (`verdict_source=unparseable_verdict`).
- Conflicting prose tokens (`CONTINUE` on one line, `STUCK` on another) →
  `STUCK` (`verdict_source=unparseable_verdict`).
- A `verdict` object quoted before the evaluator's answer is ignored. Only
  one on the last non-blank line is read.
- A `not_converging` value that is not a real JSON array → replaced with `[]`
  by the [round-trip guard](#verdict-json-round-trip-guard); the verdict is
  unchanged.
- A verdict token outside the three canonical outcomes → `STUCK`.
- Unparseable **evidence** → `terminal_refusal` defaults to `true` → `STUCK`.
  An absent evidence object is never read as "the guard did not fire".
- Enforcement on an empty or malformed `loop_health` → `STUCK`, exit non-zero.
- An explicit JSON `null` (`{"terminal_refusal": null}`) takes the `--default`
  too. `extract-field` treats a null exactly like an absent field, so the
  `--default true` fail-safe cannot be walked past with a null.

A verdict copied from the prompt's example is `STUCK`, so it stops the loop
(see [Where the prompt states the contract](#where-the-prompt-states-the-contract)).
None of these catches a verdict copied from the round output. See the residual
risks under [The alternate key is final](#the-alternate-key-is-final) and
[Prose token rules](#prose-token-rules).

### Reading step outputs: RECIPE_VAR only

`recipe-runner-rs` exports every step output as `RECIPE_VAR_<name>`, but it
only adds the plain-uppercase alias (`LOOP_EVIDENCE`) for **scalar** outputs.
`loop_evidence` and `loop_health` declare `parse_json: true`, and an evaluator
that obeys the "emit only the JSON object" contract makes
`loop_health_assessment` an object too. All three plain names are therefore
**absent** at runtime, and a bare `${LOOP_EVIDENCE:-}` read is silently empty,
or worse, holds a stale value inherited from whatever launched the runner.

Every read in these bricks uses the `RECIPE_VAR_` name alone:

```bash
EV="${RECIPE_VAR_loop_evidence:-}"
```

The bare name is never read, so an inherited `LOOP_EVIDENCE` or `LOOP_HEALTH`
cannot replace the brick's own evidence or verdict (see
[Step outputs ignore inherited bare variables](auto-drive-to-merge.md#step-outputs-ignore-inherited-bare-variables)).
Dropping the `RECIPE_VAR_` read makes the recipe inert: the reads miss,
`--default true` fires, and every run reports a `terminal_policy_refusal` that
never happened. `tests/integration/issue_1337_loop_health_evaluator_test.rs`
fails on any read of `LOOP_EVIDENCE`, `LOOP_HEALTH` or
`LOOP_HEALTH_ASSESSMENT` in a step command, and on a missing `RECIPE_VAR_`
read of any of the three; the contract test proves it end to end through the
real runner.

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
  "terminal_reason": "exit code 79 present in round output",
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
arrives, and a terminal `79` refusal. The evidence paraphrases the child's
refusal ("child BLOCKED_TERMINAL at depth 4/3, exit 79"), so only the exit-79
check fires; had the guard's own `BLOCKED_TERMINAL orchestration_unavailable`
line been present, the reason would also cite it. Verdict: **`STUCK`** — on the first round
the evidence becomes visible, not after some number of attempts. The loop stops
and reports what is not converging instead of consuming the remaining budget.

## Tests

| Test | Location |
| ---- | -------- |
| Helper unit tests (synonyms including `CONVERGING` / `PROGRESSING`, canonical pass-through, malformed → `STUCK`, negation-adjacent equality regression including `NOT_PROGRESSING`, opposite-default guard) | `crates/amplihack-cli/src/commands/orch.rs` |
| Executable contract test (STUCK path, malformed-verdict path, exit-79 terminal path, the 2h47m worked example, no-cap and no-timeout guards, and a hand-written `check_rs` line, run through `resolve_verdict_and_source`, for every row of the [step-03 examples](#examples) — the test does not read the table, so a new row needs its own `check_rs` line — plus the round-trip guard, `LOOP_NAME` cleaning, prompt-marker and [contract-position](#where-the-prompt-states-the-contract) cases, and step-04's stdout being at most one `LOOP_HEALTH: ` line for `CONTINUE`, `DONE` and `STUCK`) | `amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh` |
| **End-to-end probe** — the real recipe files run through the real `recipe-runner-rs` with only step-02 stubbed as a bash step: `CONTINUE` → exit 0, `STUCK` → exit 1, verdict selection, exit-79 terminal, self-poisoning | same file, section 7 (skipped with a loud notice when `recipe-runner-rs` is not installed; set `RECIPE_RUNNER_RS_PATH` to force it) |
| Structural + end-to-end wiring, including `evaluator_prompt_states_the_contract_first_and_last` and the [RECIPE_VAR-only read check](#reading-step-outputs-recipe_var-only) for `loop_evidence`, `loop_health` and `loop_health_assessment` | `tests/integration/issue_1337_loop_health_evaluator_test.rs` |
| Collector terminal detectors ignore agent output lines: the real step-01 command run through `bash` in an isolated environment, one test per row of the [agent output table](#agent-output-lines-are-not-refusals), each checking both `terminal_refusal` and, where the table lists one, the `terminal_reason` substring, plus the unfiltered duration count and an agent line followed by a real guard line. Two tests guard the filter itself: `issue_1510_near_miss_prefixes_are_not_filtered` checks that a short timestamp, another tag or a different case is not dropped, and `issue_1510_filter_ere_matches_autodrive_loop_byte_for_byte` fails if the collector and `terminal_refusal()` in `autodrive_loop.sh` drift apart on either half of the rule (the agent-line ERE or the full marker). The round-tail tests run `amplifier-bundle/tools/autodrive_round_tail.sh` and check that a log cut inside an agent line yields only whole lines and no refusal, and that a summary longer than the window keeps its header. The run-summary tests feed a copy of a real successful round log (`tests/integration/fixtures/issue_1510_crusty_round_tail.txt`, with the review quoting the marker and "exit 79" and a `resolved_concerns` id containing `exit-79`) through both detectors and expect no refusal; they keep a guard line placed before the summary terminal, pin the header pair the cut needs, and `issue_1510_summary_cut_awk_is_shared_byte_for_byte` fails if the cut drifts between the collector, `terminal_refusal()` and `autodrive_round_tail.sh` | `tests/integration/issue_1510_collector_agent_prose_test.rs` |
| Usage limit on step-02, retried: a fake `claude` that prints the weekly-limit message on its first call and a valid verdict on its second, a fake rotator cycle written during the wait, run through a stub `recipe-runner-rs` under an isolated `HOME`. Checks exit `0`, the verdict once, two calls, the `start` and `end reason=rotator capacity` lines, and no `usage_limit` failure class | `crates/amplihack-cli/tests/issue_1801_evaluator_retry.rs` |
| Usage limit on step-02, retried and expired, as seen by `autodrive_loop.sh`: `1801-evaluator-retry-verdict` records the verdict with no `STUCK`; `1801-evaluator-usage-limit` stops as `USAGE_LIMIT`; negative cases for a nested `class`, the marker with exit `0`, an indented or echoed marker, and wait lines alone | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |

Run them with:

```bash
cargo test -p amplihack-cli normalise_loop_verdict
cargo test -p amplihack --test integration issue_1337_loop_health_evaluator::
cargo test -p amplihack --test integration issue_1510
cargo test -p amplihack-cli --test cli_integration issue_1801_evaluator_retry::
bash amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh
```

### Which `amplihack` binary the shell test uses

The shell test runs the real step bodies, which call
`amplihack orch helper`. The test tries these candidates in order:

1. `target/release/amplihack` in this checkout
2. `target/debug/amplihack` in this checkout
3. `$CARGO_TARGET_DIR/release/amplihack`, only when `CARGO_TARGET_DIR` is set
   and not empty
4. `$CARGO_TARGET_DIR/debug/amplihack`, under the same condition
5. `amplihack` on `PATH`

It uses the first candidate that exists, is executable, and passes this
probe, which must print exactly `CONTINUE`:

```bash
printf converging | "$candidate" orch helper normalise-loop-verdict
```

A binary built before the `CONVERGING` synonym existed (issue #1513) prints
`STUCK`, so it is skipped even when it comes first on `PATH`. If no candidate
passes, the test prints a `HARNESS-ERROR` saying that no candidate maps
`converging` to `CONTINUE` (each one is missing or stale), and exits `2`.

The test prints the chosen binary before its first check. With a target
directory outside the checkout it looks like this:

```console
$ export CARGO_TARGET_DIR="$HOME/.cache/cargo-target"
$ cargo build -p amplihack --bin amplihack
$ bash amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh
amplihack binary: /home/dev/.cache/cargo-target/debug/amplihack
=== Issue #1337: agentic loop-health evaluator contract ===
...
```

Under `cargo test`, the `loop_health_contract_shell_test_passes` wrapper puts
the binary cargo just built first on `PATH`. Candidates 1 to 4 are still
tried before `PATH`, so an older binary in one of them that passes the probe
is tested instead. The `amplihack binary:` line shows which one ran. CI runs
this test through that wrapper as part of `cargo nextest run --workspace`,
with no `CARGO_TARGET_DIR` and no cached `target/`, so it uses the
`target/debug/amplihack` it built from the commit under test.

The step bodies call bare `amplihack`, so the test puts the chosen binary's
directory first on `PATH` for the whole run, and every executable in that
directory shadows system commands of the same name. Point `CARGO_TARGET_DIR`
only at a directory that no one but you can write, where every executable
comes from this workspace. A predictable path in a shared directory such as
`/tmp` or `/var/tmp` lets another user plant a binary that the test then runs.

## Related references

- [Structured Verdict & Intent Parsing](structured-verdict-parsing.md) — the
  `extract-json | extract-field | normalise-*` pipeline this contract fits into.
- [Session Tree Recursion Control](session-tree-recursion-control.md) — the
  sealed depth ceiling behind exit code `79`.
- [Recipe Executor Environment](recipe-executor-environment.md) — how context
  vars and step outputs reach bash steps and engine conditions.
- [Recipe Quick Reference](recipe-quick-reference.md) — recipe authoring basics.
- [Usage-limit wait](profile-command.md#usage-limit-wait): how an agent step
  waits for a reset or for another login, and retries once.
