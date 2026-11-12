---
title: Loop-Health Evaluator Reference
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
---

# Loop-Health Evaluator Reference

`loop-health-evaluator` is a reusable recipe brick that answers one question
after each round of an iterative loop: **is this loop still doing useful
work?** It looks at the accumulated evidence of the loop and emits one
structured verdict — `CONTINUE`, `DONE`, or `STUCK`.

Auto-drive Crusty reviews check objective completion before invoking this
brick. An evidence-valid `CLEAN` or `MINOR_ONLY` ends review iteration without
an evaluator call; required follow-up recording still gates phase completion.
This also applies to Crusty rereviews between merge-ready rounds. The
merge-ready loop's own evaluator policy is unchanged. See the
[Crusty completion contract](autodrive-crusty-completion.md).

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
  - [Round text never decides a refusal](#round-text-never-decides-a-refusal)
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
`DONE` that breaks this rule anyway on evaluated rounds (see
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
`79` or a failed child's final marker with `structural_refusal: true`, and also
evidence that is missing, unparseable or `null`, because `terminal_refusal` defaults to `true`. Second, empty output
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
If an evaluated merge-ready round is clean, the loop ends without the
confirming round that a `CONTINUE` would have run. Completed Crusty reviews
use objective-completion dispatch before this evaluator is invoked.
If the round verdict is anything else, the loop does not advance. It runs
one more round to re-review, and stops as `STUCK` if the round record is
missing or if the next round repeats the same non-clean `DONE` with no
progress. Currently a `DONE` from the merge loop hands over to the shell
merge gate. The [planned authorized finalization](auto-drive-to-merge.md#no-silent-merge)
will replace that gate, refreshing mutable facts and validating accepted
evidence before carrying out the captured action. Invocation-wide authorization
and gate retirement remain [implementation requirements](auto-drive-to-merge.md#current-behavior-and-authority).
Readiness does not itself authorize a merge under the target contract.

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
| `terminal_refusal`, `terminal_reason` | A child returned exit `79`, or failed with a final `structural_refusal: true` marker. Terminal — see below. |
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
| `terminal_policy_refusal` | The evidence showed exit `79` or a failed child's final `structural_refusal: true` marker, or was missing or unparseable (`terminal_refusal` defaults to `true`); the evaluator was skipped. |
| `missing_verdict` | The evaluator produced no output. |
| `unparseable_verdict` | Output existed but held no usable verdict, or held conflicting tokens. |

See [How step-03 resolves the verdict](#how-step-03-resolves-the-verdict) for
the order in which these are tried.

## Using it from a recipe

Invoke the brick for rounds that need convergence judgment and gate the next
round on its verdict. Auto-drive Crusty rounds first apply the
[completion-first dispatch contract](autodrive-crusty-completion.md):

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

Exit code `79` and a failed child's final `structural_refusal: true` marker
are authoritative answers from a structural guard (#1327 / #1332). They are
surfaced and the loop stops. They are **never** retried into, and the agentic step is skipped entirely on a terminal refusal
(`condition: "loop_evidence.terminal_refusal == 'false'"`) — not even a model
call is spent deciding whether to re-enter a sealed guard. A terminal refusal
forces `STUCK` even if an evaluator verdict from an earlier round said
`CONTINUE`. Only the exit status and the runner's final marker count: no
round text, the guard's own phrase included, is a refusal (see
[Round text never decides a refusal](#round-text-never-decides-a-refusal)).

Historically, agents read a `79` refusal as an infrastructure fault and retried
one level deeper with a raised ceiling. That is what #1327 sealed, and it is
what this branch refuses to re-open.

The opposite mistake, reading an ordinary failure as a refusal, is as costly
(issue #1437). `autodrive_loop.sh` treats a child as refused only when its exit
status is `79`, or when it failed and the last `amplihack.recipe.failure_class`
marker in its log has `"structural_refusal": true`. A transcript that quotes
`BLOCKED_TERMINAL orchestration_unavailable` does not stop the loop driver; the
failure reaches this evaluator instead, with `child=rc=<n> class=<class> retryable=<bool>[ signal=<signal>]` on the
round's `loop_history` line. See
[auto-drive-to-merge](auto-drive-to-merge.md#not-every-failure-is-terminal-issue-1437).
The collector sources the same `autodrive_refusal.sh` helper and applies it
only to `loop_child_exit_code` and `loop_last_round_output`. A successful child,
a missing/non-numeric status, quoted guard/exit text and markers in
`loop_history` cannot set `terminal_refusal`. A marker must carry a JSON boolean
`true`; the string `"true"` is ignored. The last anchored marker wins, so the
runner's final marker overrides nested markers in the round output.

`loop-evidence-collector.yaml` sets `infrastructure_fault=true` only for
`transient_transport` and `environmental`, and the evaluator prompt names only
those two. An `agent_api` round, an agent endpoint rejecting the request
(`Execution failed: 400`, `session not found`), therefore arrives with
`infrastructure_fault=false` and `agent_api` in `infrastructure_fault_class`.
That is intended: it is judged as a round that made no progress, so one may be
followed by a fresh round, but a run of them reads as `STUCK`. A 404 that
carries an environmental marker (`API Error: 404`, `404 Not Found`,
`not_found_error`, `model_not_found`) is `environmental`, not `agent_api`,
because the classifier checks environmental markers first (see
[transient-failure-retry](transient-failure-retry.md#classification)), so an
unknown model still counts as an infrastructure fault.

### Round text never decides a refusal

Round text, the guard's own phrase included, never sets `terminal_refusal`.
Before #1437 the collector also read text: it cut the runner's run summary,
dropped agent and `amplihack.recipe.log_pointer` lines, and then looked for
`BLOCKED_TERMINAL orchestration_unavailable` and for `exit 79`-like phrases.
Each filter closed one path by which quoted text stopped a loop (#1510; PR
#1586; rounds 1 and 3 of PR #1632), and each left another open. The #1437
incident was one of those: an agent handled a descendant's refusal inline,
as the refusal tells it to, and later lost its API session, and the phrase
in its transcript ended a run that had pushed its work.

The collector now applies `terminal_refusal` from
`amplifier-bundle/tools/autodrive_refusal.sh`, the function `autodrive_loop.sh`
uses for every child, to `loop_child_exit_code` and `loop_last_round_output`
alone:

| Input | `terminal_refusal` |
| ----- | ------------------ |
| `loop_child_exit_code` = `79`, with any text or none | `"true"` |
| exit `1`, and the last line beginning `amplihack.recipe.failure_class ` has `"structural_refusal": true` | `"true"` |
| exit `1`, `BLOCKED_TERMINAL orchestration_unavailable: …` on its own line at column 0, no marker | `"false"` |
| exit `1`, an agent line or a run summary quoting the phrase or `exit 79` | `"false"` |
| exit `1`, `child exited with status 79` | `"false"` |
| exit `1`, a `log_pointer` line with `"exit_code":79` | `"false"` |
| exit `1`, a `true` marker followed by a later `false` marker | `"false"` |
| exit `1`, `"structural_refusal": "true"` (a string) | `"false"` |
| exit `1`, an indented copy of a `true` marker | `"false"` |
| exit `0`, or no exit code, with a `true` marker | `"false"` |
| a `true` marker in `loop_history` only | `"false"` |

What this costs: a refusal whose exit code was lost and whose runner wrote no
final marker is not seen. For the loop's own rounds neither happens in
practice. A guard refuses before a run starts and `amplihack recipe run` exits
79; a failed step that ended with exit 79 sets `structural_refusal` on the
marker the runner writes last. For a `type: recipe` step that is the first
failed step the runner lists for the sub-recipe; a refusal after a non-fatal
failure in the same sub-recipe is not seen (see the
[Limits](auto-drive-to-merge.md#limits) of the #1437 section).
The duration, still-running and repetition checks still read the unfiltered
history and last round output. `autodrive_round_tail.sh` still prints whole
lines only, and the final marker, being the log's last line, is always inside
its 20000-byte tail.

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

The collector never uses `loop_history` to decide current termination, and
never searches text for an exit code or guard phrase. Feeding a previous
escalation report back into history therefore cannot manufacture another
terminal refusal. Tagged `[loop-health-evaluator]` report lines are excluded
from repetition and waiting measurements as well.

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
  "terminal_reason": "exit 79 [loop-health-evaluator]",
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
| Collector refusals come from the exit code and the runner's final marker only (#1510, #1437): the real step-01 command run through `bash` in an isolated environment, with `AMPLIHACK_HOME` and the built `amplihack` binary for the shared helper. Every #1510 case that quoted the guard (agent prose, an indented or failed-step tail, a bare token, `log_pointer` echoes, a real successful round log whose run summary quotes the marker and "exit 79", a byte tail cut inside an agent line) is still not terminal. Every case #1510 kept terminal on text (the guard's phrase at column 0, indented, after agent prose, after a `log_pointer` line, inside other JSON, before or after a summary header, in history; an unprefixed "exit status 79"; near-miss agent prefixes) is now not terminal with no exit code or exit `1`/`2`, and terminal with the runner's final `structural_refusal: true` marker after it. The marker cases check exit `0`, a missing or non-numeric exit code, an indented copy, the string `"true"`, a later `false` marker and history. `issue_1437_collector_and_loop_share_one_refusal_helper` fails if either reader stops sourcing `autodrive_refusal.sh` or matches the guard's text again. The round-tail tests check that only whole lines are printed and that a long summary keeps its header | `tests/integration/issue_1510_collector_agent_prose_test.rs` |
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
