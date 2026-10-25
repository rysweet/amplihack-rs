---
title: Auto Drive To Merge Reference
last_updated: 2026-10-19
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
- [PR head and worktree resolution](#pr-head-and-worktree-resolution)
- [Why there is no iteration cap](#why-there-is-no-iteration-cap)
- [Structured verdicts](#structured-verdicts)
- [How the merge round applies merge-ready](#how-the-merge-round-applies-merge-ready)
- [Reading step outputs](#reading-step-outputs)
- [Reading the loop-health line](#reading-the-loop-health-line)
- [Two absolute prohibitions](#two-absolute-prohibitions)
- [No silent merge](#no-silent-merge)
- [Merges outside the gate are refused (#1551)](#merges-outside-the-gate-are-refused-1551)
- [Exit code 79 is terminal](#exit-code-79-is-terminal)
- [Recursion context propagation](#recursion-context-propagation)
- [Resumability](#resumability)
- [The run report](#the-run-report)
- [No short timeouts](#no-short-timeouts)
- [Where phases 2 and 3 run](#where-phases-2-and-3-run)
- [Files](#files)
- [Tests](#tests)
- [Dependency on PR #1347](#dependency-on-pr-1347)
- [Related references](#related-references)

## The three phases

| Phase | Brick | What it does |
| --- | --- | --- |
| 0. Prerequisites | `auto-drive-to-merge.yaml` step `autodrive-prerequisites` | Where criterion 1 needs `gadugi-test` (a Node repository, or `AUTODRIVE_QA_SCENARIO_DIR` set), stops the run with `ERROR: gadugi-test-not-installed` when it is not on `PATH`, before the build and the crusty loop spend hours. A Rust CLI or Python repository does not need it. |
| 1. Build | `autodrive-build.yaml` | Runs `default-workflow` with `no_merge: "true"` to produce a PR. Never merges. |
| 2. Crusty loop | `autodrive-crusty-loop.yaml` over `autodrive-crusty-round.yaml` | Runs `crusty-old-engineer` as the maintainer's proxy, addresses every concern, re-reviews, repeats until `crusty_verdict` is `CLEAN`. |
| 3. Merge-ready loop | `autodrive-merge-loop.yaml` over `autodrive-merge-round.yaml` | Before each round, sends code commits made after the clean crusty round back to the crusty loop (`autodrive_crusty_rereview.sh`). Syncs the base, runs the repository tests and, where criterion 1 needs them, the `gadugi-test` scenarios, waits for CI, applies the `merge-ready` criteria read from the skill's files, clears blockers, repeats, then merges behind the gate. |

Phase 1 is a no-op when an open PR already exists for the branch, and the whole
workflow short-circuits when the PR is already merged.

Phases 2 and 3 resolve the pull request's linked worktree before their first
round, and every round edits, commits and pushes there, never in the launch
checkout. See [Where phases 2 and 3 run](#where-phases-2-and-3-run).

## PR head and worktree resolution

Every evidence step measures the PR head, whichever directory the run starts
from. A run launched from the repository's main checkout does not record the
main checkout's HEAD as `head_sha`, and does not run QA on the main
checkout's tree. That holds whether the operator named the PR with
`pr_number` or phase 1 built it.

```bash
# Launched from the main checkout; feat/x is checked out in worktrees/feat/x.
amplihack recipe run auto-drive-to-merge -c pr_number=1543 -c existing_branch=feat/x -c repo_path=.
# Launched from the main checkout; default-workflow builds the PR in its own
# worktree (workflow-worktree, step-04) and leaves the main checkout on main.
amplihack recipe run auto-drive-to-merge -c task_description="..." -c repo_path=.
```

### When resolution runs

Resolution runs in step-01 (the preflight) of `autodrive-crusty-loop.yaml` and
`autodrive-merge-loop.yaml` when both of these hold:

- the loop was handed a PR number. The composer passes the build phase's PR
  as `pr_number` (`{{build_result.pr}}`) and the operator's own `pr_number`
  as `autodrive_pr_input`, which its first step, `autodrive-pr-input`, records
  before any phase runs. Either one is enough. Both name the PR from the
  platform's side, and the worktree is then found through the PR's
  `headRefName`, not through the launch checkout;
- the phase is going to run (`should_run` would be `true`).

The PR number used is `PR_NUMBER`, or `autodrive_pr_input` when `PR_NUMBER`
is empty or not numeric. A `pr_number` the operator gave that is not a
positive number, with no `PR_NUMBER` to override it, is refused before
resolution:

```
ERROR: pr_number 'abc' is not a pull request number; refusing to enter the crusty loop (#1543)
```

Only when the loop was handed no PR number at all, which happens when a loop
recipe is run on its own without `pr_number`, does the preflight fall back to
`gh pr view` in `${REPO_PATH:-.}`. That number is a guess from the launch
checkout, so it never turns resolution on: the loop uses `${REPO_PATH:-.}`,
calls no `gh` head lookup, and never refuses on these grounds.

The operator's value travels separately as `autodrive_pr_input` because the
recipe runner copies a sub-recipe's whole context back into the parent when
it returns, and the crusty loop is given `pr_number={{build_result.pr}}`, so
after phase 2 `pr_number` holds the build phase's PR. `autodrive_pr_input`,
which no sub-recipe changes, keeps the operator's value, and is what makes a
non-numeric operator `pr_number` a refusal rather than a silent fallback.

When resolution does run, `REPO_PATH` may be absolute or relative to the
directory the run started in. The preflight changes into `REPO_PATH` and
passes the resulting `$PWD` to the resolver. A `REPO_PATH` the preflight
cannot enter is refused before resolution; the loop never falls back to the
current directory:

```
ERROR: cannot cd to REPO_PATH='sub'; refusing to enter the crusty loop (#1543)
```

### The resolver

`amplifier-bundle/tools/autodrive_pr_head.sh` is read-only. It never creates,
fetches into, checks out, commits in, or otherwise changes any worktree.

```bash
amplifier-bundle/tools/autodrive_pr_head.sh --pr 1543 --branch feat/x --repo .
```

On success it prints one JSON line on stdout. Values in angle brackets are
placeholders:

```json
{"pr":"1543","head_sha":"<40-hex headRefOid>","head_ref":"feat/x","worktree":"<repo>/worktrees/feat/x","worktree_head":"<40-hex HEAD>","head_matches":"true","worktree_relation":"same","remedy":"","reason":"ok"}
```

| Argument | Required | Meaning |
| --- | --- | --- |
| `--pr N` | yes | PR number. Must match `^[1-9][0-9]*$`. |
| `--branch B` | no | The run's `existing_branch`. When given it must equal the PR's `headRefName`, or the tool refuses with exit `5`. |
| `--repo R` | no | Repository to search. Defaults to `.`. |

The tool reads `headRefOid`, `headRefName` and `state` with one
`gh pr view N --json` call, then checks and picks the worktree in this order:

1. If `B` is given and is not `headRefName`, refuse (`branch-mismatch`). A
   worktree on another branch would be measured, and the round would push
   from it.
2. `R` itself, if `git branch --show-current` there equals `headRefName`.
   This covers a main checkout that has the PR branch checked out.
3. The `git -C R worktree list --porcelain` entry whose branch is
   `refs/heads/<headRefName>`. Prunable and bare entries are skipped, and so
   is any entry whose git common dir is not `R`'s.
4. Otherwise, refuse.

Branch names are compared as literal strings. A branch called `--force` is
compared, not passed to `git` as an option.

On success the tool prints exactly one JSON line on stdout and exits `0`.
On failure, stdout is **empty**. The tool writes one line to stderr in the form
`autodrive_pr_head: <reason>: <detail>`, sometimes followed by more lines, and
exits non-zero. The preflight acts on the exit code alone. It never parses a
failure from stdout, so there is no failure JSON for the resolver and the
preflight to disagree about.

| Field | Meaning |
| --- | --- |
| `head_sha` | The PR's `headRefOid` on GitHub. |
| `head_ref` | The PR's `headRefName`. |
| `worktree` | Absolute, symlink-resolved path of the chosen worktree. |
| `worktree_head` | `git rev-parse HEAD` in that worktree. |
| `head_matches` | `"true"` when `worktree_head` equals `head_sha`, else `"false"`. |
| `worktree_relation` | `same`, `behind`, `ahead`, `diverged` or `unfetched`; see [A worktree that is not at the PR head](#a-worktree-that-is-not-at-the-pr-head). |
| `remedy` | The command that brings the worktree to the PR head for that relation, with the worktree path and branch shell-quoted (`printf %q`) so it can be pasted as printed even when the path contains a space. Empty when `head_matches` is `"true"`. |

| Exit | `reason` | Cause |
| --- | --- | --- |
| `0` | `ok` (in the JSON) | Worktree found. |
| `2` | `usage` (on stderr) | Bad or missing arguments. |
| `3` | `no-pr-worktree` (on stderr) | No worktree has the PR branch checked out. |
| `3` | `ambiguous-pr-worktree` (on stderr) | More than one worktree matches. Each matching path follows on its own stderr line. |
| `4` | `pr-lookup-failed` (on stderr) | `gh pr view` failed, or returned an empty `headRefName` or a `headRefOid` that is not 40 or 64 hex characters. A `gh` failure is summarised as `gh pr view failed (rc=N)`. `gh`'s own output is never echoed, so auth errors and tokens do not reach the log. |
| `5` | `branch-mismatch` (on stderr) | `--branch` is not the PR's `headRefName`. |

### Refusal

When resolution fails, the preflight exits non-zero before it emits
`should_run`. Neither the round steps nor the merge gate run. There is no
fallback to the current directory. Each message names the loop that refused:
`merge loop` from `autodrive-merge-loop.yaml`, `crusty loop` from
`autodrive-crusty-loop.yaml`. The crusty loop runs first, so on a fresh run you
normally see the crusty-loop wording.

The resolver's own stderr line comes first, then the preflight's `ERROR`.

**No worktree** (resolver exit `3`, `no-pr-worktree`):

```
autodrive_pr_head: no-pr-worktree: PR #1543 head branch 'feat/x' has no worktree (searched --repo '<repo>' and git worktree list)
ERROR: PR #1543 head branch 'feat/x' has no single worktree (searched REPO_PATH=<repo> and git worktree list); refusing to enter the crusty loop (#1543)
```

To recover, check the branch out in a worktree and re-run. Local state is
kept, so finished phases are not redone:

```bash
git worktree add worktrees/feat/x feat/x
amplihack recipe run auto-drive-to-merge -c pr_number=1543 -c existing_branch=feat/x -c repo_path=.
```

**Ambiguous worktree** (resolver exit `3`, `ambiguous-pr-worktree`). Each
matching path is listed:

```
autodrive_pr_head: ambiguous-pr-worktree: PR #1543 branch 'feat/x' is checked out in 2 worktrees; check another branch out in all but one, or remove the extras
  <repo>/worktrees/feat/x
  <repo>/worktrees/feat/x-copy
ERROR: PR #1543 head branch 'feat/x' has no single worktree (searched REPO_PATH=<repo> and git worktree list); refusing to enter the crusty loop (#1543)
```

To recover, check another branch out in all but one of the listed worktrees,
or remove the extras with `git worktree remove <path>`, then re-run.

**Branch mismatch** (resolver exit `5`, `branch-mismatch`). `existing_branch`
names a branch other than the PR's head branch:

```
autodrive_pr_head: branch-mismatch: --branch 'feat/x-2' is not PR #1543's head branch 'feat/x'; pass existing_branch=feat/x or leave it empty
ERROR: PR #1543 head resolution refused (autodrive_pr_head exit 5; reason on the line above); refusing to enter the crusty loop (#1543)
```

To recover, re-run with `existing_branch` set to the PR's head branch, or
without it.

**Any other resolver failure** (fail closed, exits non-zero with the reason):

- the resolver is missing from the tools directory the state helper was found in;
- it exits non-zero for any other reason, including `2` (usage) and `4`
  (`pr-lookup-failed`, reported as `gh pr view failed (rc=N)` with no `gh` or
  auth output);
- it exits `0` but stdout is not one parseable JSON line, `worktree` is empty,
  or `head_sha` is not 40 or 64 hex characters.

```
autodrive_pr_head: pr-lookup-failed: gh pr view failed (rc=1) for PR #1543
ERROR: PR #1543 head resolution refused (autodrive_pr_head exit 4; reason on the line above); refusing to enter the crusty loop (#1543)
```

### A worktree that is not at the PR head

If the worktree's HEAD is not `headRefOid`, the resolver does not refuse. It
sets `head_matches` to `"false"`, works out which way the worktree differs
from the PR head with read-only commands (`git cat-file -e` and
`git merge-base --is-ancestor`), and prints both SHAs, the relation and the
remedy for that relation on stderr. The decision belongs to its caller, and
both loop preflights refuse by name before the loop starts:

```
WARNING: worktree <repo>/worktrees/feat/x is at <worktree_head>, PR #1543 head is <head_sha> (worktree behind); run: git -C <repo>/worktrees/feat/x pull --ff-only
ERROR: pr-worktree-stale: worktree <repo>/worktrees/feat/x is at <worktree_head>, PR #1543 head is <head_sha> (worktree behind); run: git -C <repo>/worktrees/feat/x pull --ff-only, then re-run. Refusing to enter the crusty loop: every crusty round would refuse with crusty-local-head-not-pr-head (#1543)
```

The remedy depends on `worktree_relation`:

| `worktree_relation` | Meaning | Remedy printed |
| --- | --- | --- |
| `behind` | The worktree's HEAD is an ancestor of the PR head. | `git -C <wt> pull --ff-only` |
| `ahead` | The PR head is an ancestor of the worktree's HEAD: the worktree holds commits the PR does not have. | `git -C <wt> push origin <head_ref>` |
| `diverged` | Each side has commits the other lacks. | `git -C <wt> pull --no-rebase`, then `git -C <wt> push origin <head_ref>` |
| `unfetched` | The PR head is not in the local object store, so the worktree cannot be ahead of it; it is behind or diverged. | `git -C <wt> pull --ff-only`; if git refuses, follow the `diverged` remedy |

`ahead` is the usual result of a previous run whose `git push` failed. The
round treats a failed push as a warning (see below), and a push that fails on
authentication or the network has the same effect: the round's fix commits
stay in the worktree only. For that case `pull --ff-only` prints
"Already up to date" and changes nothing, so the preflight tells the operator
to push instead.

The loop is not entered because it cannot succeed on that tree. Every crusty
round refuses with `crusty-local-head-not-pr-head`, and the merge gate refuses
every merge round's evidence, so the loop would end in an opaque `STUCK`
after paying for a `loop-health-evaluator` call per round. The rest of this
section explains why nothing in the run would close the gap if the preflight
let it in.

Evidence records the worktree's real `git rev-parse HEAD`, never `headRefOid`.

Nothing in the run closes the gap. The round's base-sync step fetches and
merges only the **base** branch and then runs `git push`. It never pulls the PR
branch. On a stale worktree that push fails as a non-fast-forward, which is
only a warning. Base-merge commits and crusty-fix commits then land on the
stale head, so the worktree diverges further from the PR.

The merge gate also refuses evidence whose `head_sha` is not the PR head it
is about to merge (see [No silent merge](#no-silent-merge)). It is the last
line, not the first: the preflight refusal comes before any round runs. Run
the printed remedy and re-run. For a worktree behind the PR head:

```bash
git -C worktrees/feat/x pull --ff-only
amplihack recipe run auto-drive-to-merge -c pr_number=1543 -c existing_branch=feat/x -c repo_path=.
```

The remedies never discard local commits. Resetting the worktree onto
`origin/feat/x` also clears the refusal, but it throws away every commit the
worktree has that the PR does not, including unpushed crusty-fix commits. Use
it only when those commits are not wanted.

### What uses the resolved worktree

The preflight adds `repo` (the worktree) to its JSON. It does not emit
`head_sha`: nothing downstream would read it, because each round takes its own
`git rev-parse HEAD` in `repo`. What binds `repo` to the PR head is the
`pr-worktree-stale` refusal above. Every path string in that JSON is escaped, so a worktree path containing `"` cannot forge
`should_run` or any other field.

| Step | Uses `repo` for |
| --- | --- |
| step-02, both loops | The starting point of the [#1528 resolution](#where-phases-2-and-3-run): `review-dir "$REPO"` and `autodrive_pr_worktree.sh resolve --repo "$REPO"`, whose result is `autodrive_loop.sh --repo` and `--context repo_path=`. It reads `repo` as `${RECIPE_VAR_repo:-}`, the same way as `state_dir`. If `repo` is missing, the step refuses to run; it never falls back to `REPO_PATH`. Every round step therefore runs in the PR worktree: base sync, `qa-team` run and its `head_sha`, CI wait, crusty review and fixes, round records, loop baseline. |
| merge-loop step-04, crusty-loop step-03 | **Nothing.** These steps write only inside `state_dir`, so they do not depend on the working directory. |
| merge-loop step-03 (merge gate) | **Nothing.** The gate stays on the launch side and binds to `headRefOid` from `gh`. Resolver output never reaches it. |

Tools are still looked up from the launch side. If `AMPLIHACK_HOME` is unset,
step-02 sets it to the tree it took `autodrive_loop.sh` from: the launch
checkout, `~/.copilot` or `~/.amplihack`. The rounds' tool lookups therefore
never fall through to an `amplifier-bundle/tools/` that the PR branch adds,
and tools on the PR branch cannot produce the QA evidence the gate reads. An
`AMPLIHACK_HOME` that is already set is never changed.

**Recipes come from the launch side too.** `amplihack recipe run <name>
--working-dir <dir>` searches `<dir>`'s own `amplifier-bundle/recipes/` before
`AMPLIHACK_HOME`. With the PR worktree as the working directory, a PR that
ships `amplifier-bundle/recipes/` (any PR to amplihack-rs) would otherwise run
its own `autodrive-merge-round.yaml`, which sets `merge_ready_verdict`, and its
own `loop-health-evaluator.yaml`, which decides when the loop stops. The PR
would judge its own readiness (PR #1588 review). So:

- Step-02 of each loop passes the round recipe as an absolute path,
  `$(dirname "$LOOP_HELPER")/../recipes/autodrive-<x>-round.yaml`, the tree it
  took `autodrive_loop.sh` from. If that file is missing, the step refuses with
  `autodrive-recipe-not-found` before the loop starts.
- `autodrive_loop.sh` resolves the round recipe with `autodrive_launch_recipe`
  (in `autodrive_state.sh`). A bare name, as the crusty re-review passes,
  resolves only to the `recipes/` directory beside the driver. An absolute path
  is accepted only when it is that same file. Anything else exits 2, with
  `"loop_result":"UNTRUSTED_RECIPE"` on stdout, before a round runs.
- The driver's `loop-preflight` (`autodrive_worktree_guard.sh`, #1528) then
  resolves the round recipe again by name and `loop-health-evaluator`, and
  refuses with `UNTRUSTED_RECIPE` when that bundle lies inside `--repo` (the
  driver was itself taken from the tree under review).
- The runner searches a recipe's own directory first (`build_search_dirs`), so
  sub-recipes reached from a round also come from the launch side.

`test-issue-1543-pr-head-worktree.sh` T23 runs the real driver against a PR
worktree whose round and evaluator always pass, and checks that neither runs.
The round steps still run the PR's own code by design (tests, hooks,
`qa-team`), so drive only PRs whose code you are prepared to run.

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

`autodrive_loop.sh` carries a round **label** (`round-3`) for reports and the
round's file names. Nothing compares it to a limit and no branch reads it. A
loop's first label is one past the highest `round-N` the same loop already has
in the state directory, counting both files named `<loop>-round-N…` and the
labels in `<loop>-records.tsv`. So when the crusty re-review starts the crusty
loop again on phase 2's directory, or a resumed run starts a loop on the
directory a dead run left, no earlier round's record, findings or logs are
rewritten, and every manifest row still names the record it hashed. Starting
again at `round-1` rewrote phase 2's `crusty-round-1.json` and left its row
pointing at a different file (PR #1520 review). The guard test
`no_numeric_iteration_cap_anywhere` fails the build if a cap is ever
reintroduced — it rejects `$ROUND` and `${ROUND}` next to any of `-ge`, `-gt`,
`-le`, `-lt`, `-eq`, `-ne`, and the arithmetic comparisons, so a cap cannot
sneak in through the operator that was not on the list.

### The terminator is resolved before round 1

Because the terminator is a separate recipe, a bundle where
`loop-health-evaluator` does not resolve would otherwise cost a **full round**
— a crusty review and a builder fix pass, with commits pushed — before the
loop stopped with "returned STUCK (or an unreadable verdict)", which blames the
loop for a missing dependency. `autodrive_loop.sh` therefore resolves
`loop-health-evaluator.yaml` beside itself and runs `amplihack recipe show` on
that path before the first round and, if it does not resolve, refuses with `loop_result: MISSING_DEPENDENCY` and a message
naming the recipe. Zero rounds are spent.

Host safety is enforced structurally one layer down and is not this workflow's
job:

| Guard | Bounds | Refusal |
| --- | --- | --- |
| #1327 sealed recursion ceiling | depth | exit `79` |
| #1332 width cap + free-memory floor | fan-out, memory | exit `79` |

Rounds run **sequentially at constant session depth**, so a long loop never
walks toward that ceiling. No round recipe starts a loop; see
[Recursion context propagation](#recursion-context-propagation) for the
depths, and for why the crusty re-review runs between merge rounds rather than
inside one.

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

A clean round verdict with `CONTINUE` simply runs another confirming round,
which is cheap and safe. `DONE` over a round that is not clean never advances.
What happens instead depends on the round:

| Evaluator | Round verdict | Result |
| --- | --- | --- |
| `DONE` | the clean token | Exit `0`. The only `exit 0` in the loop. |
| `DONE` | `MISSING` (no round record) | `STUCK`, exit `1`. |
| `DONE` | non-clean, first time, or the round made progress | One more round to re-review. |
| `DONE` | non-clean, the previous round was also overridden, and nothing moved | `STUCK`, exit `1`. |
| `CONTINUE` | any | Another round. |
| `STUCK` or unreadable | any | `STUCK`, exit `1`. |

A non-clean `DONE` usually means the findings were addressed without a new
review: in PR #1587 crusty said `CONCERNS` and the concerns were fixed by a
pull request description edit with no commit; in PR #1586 round 5 the
merge-ready loop got `DONE` against `NOT_MERGE_READY` (issue #1597). Only a
review round can confirm such a fix, so the loop runs one and prints:

```
INFO: loop-health said DONE but crusty_verdict=CONCERNS, not CLEAN; running another round to re-review.
```

The overridden round's line in `loop_history` gains ` done_overridden=1`, so
the next evaluator call sees it. The evaluator itself is told the same rule
through `loop_clean_token` and `loop_round_verdict` (see
[`DONE` needs this round's own verdict to be clean](loop-health-evaluator.md#done-needs-this-rounds-own-verdict-to-be-clean)).

**No progress** means all three hold, compared with the previous round:

- the round verdict is the same;
- the round's findings text is the same, byte for byte after whitespace is
  collapsed and trimmed;
- `HEAD` after the round equals the round's baseline, so nothing was
  committed. A commit made by the merge-ready loop's `--before-round` re-review
  counts as progress, because the baseline is taken before that step runs. If
  `git rev-parse HEAD` fails after the round, `HEAD` counts as unchanged, so a
  repeated `DONE` stops as STUCK and fails safe. An unreadable baseline with a
  readable `HEAD` afterwards counts as changed.

If any one differs, the loop keeps going, exactly as for `CONTINUE`. A
`CONTINUE` or a clean round in between resets the override, so a `DONE` over
an unchanged non-clean round runs at most two rounds. That repeat stops
with this on stderr (`<key>` is the run's state key):

```
AUTO_DRIVE_LOOP: STUCK — loop 'crusty' stops at round label 'round-2'.
  Reason: loop-health said DONE while the round verdict is 'CONCERNS', not 'CLEAN': repeated DONE over non-clean 'CONCERNS' with no progress (same verdict, same findings, HEAD unchanged). An inconsistent pair never advances a phase.
  Round record: ~/.amplihack/state/auto-drive/<key>/crusty-round-2.json
  This is a judgement about absence of progress, not an attempt count.
```

and this one line on stdout:

```json
{"loop":"crusty","loop_result":"STUCK","round_label":"round-2","reason":"loop-health said DONE while the round verdict is 'CONCERNS', not 'CLEAN': repeated DONE over non-clean 'CONCERNS' with no progress (same verdict, same findings, HEAD unchanged). An inconsistent pair never advances a phase."}
```

A `DONE` over a missing round record keeps its existing reason, which also
ends in `An inconsistent pair never advances a phase.`

There is no counter behind this: the stop comes from the absence of progress,
like every other stop in the loop. The loop asks
`amplifier-bundle/tools/autodrive_done_override.sh` for this decision. The verdict and token are reduced to
`A-Za-z0-9_` before they reach the evaluator's `-c` context arguments, the
`loop_history` line, and the override `INFO` and `STUCK` messages above, so an
agent-written verdict cannot forge a `done_overridden=1` marker or a fake
history line. A non-clean verdict that reduces to the clean token, such as
`CLEAN!` or `**CLEAN**`, is passed on as `CLEAN_NONCANONICAL`: the evaluator
is never told such a round is clean, and a STUCK message reads
`the round verdict is 'CLEAN_NONCANONICAL', not 'CLEAN'` rather than
contradicting itself. Other existing messages, such as the `DONE` line and the clean
`CONTINUE` notice, still print the raw values.

**Known limit:** a reviewer that rewrites its findings every round while
the verdict stays non-clean and nothing is committed passes the no-progress
check, so the evaluator's `DONE` can be overridden round after round. This
is the same exposure as an ordinary run of `CONTINUE` answers, and it is
visible: every overridden round carries `done_overridden=1` in the history.

### Measurement outranks the model

Step `step-03-extract-merge-ready-verdict` downgrades an explicit
`MERGE_READY` to `NOT_MERGE_READY` when measured evidence disagrees, and lists
each disagreement in `downgrade_reason`: `qa_status=<value>` unless `PASS`,
`ci_status=<value>` unless `GREEN`, `conflict=<value>` unless `false`,
`unresolved_threads=<value>` unless `0`, `crusty_status=<value>` unless
`DONE_CLEAN`, `approval_status=<value>` unless `MET`, and
`qa_evidence=modified` unless `qa-evidence.json` still has the hash step-00d
took. A missing value never passes. The comparison is
`autodrive_round_evidence.sh measured-downgrade`; a step that cannot run it
downgrades with `measured_evidence=unreadable`. A downgrade adds a
`measured-evidence-disagrees` finding so the loop evaluator sees a round that
did not converge.

### How a step output reaches bash

The pinned runner (`claude-plugin/recipe-runner.rev`, rev `4d5187ea`) stores a
step output that parses as JSON as an object, and exports it to later bash
steps as `RECIPE_VAR_<output>` only (`src/context.rs:357`, `shell_env_vars`). The bare upper-case name, such as
`QA_EVIDENCE`, is added for scalar outputs alone. A bash step that read
`${QA_EVIDENCE:-}` would see an empty string and fail closed every round:
`MERGE_READY` downgraded, an empty `qa_evidence_sha` in the record, and no
crusty round record at all. Every step-output read in
`autodrive-crusty-round.yaml`, `autodrive-merge-round.yaml` and
`autodrive_round_evidence.sh` (which holds merge-round step bodies) is therefore
written `"${RECIPE_VAR_qa_evidence:-}"`, and
`round_recipes_read_step_outputs_through_recipe_var` fails the build on a read
that does not go through `RECIPE_VAR_`. The bare name is not read at all, so a
stale `QA_EVIDENCE` inherited from whatever launched the runner cannot replace
the round's own evidence (#1561; see
[Step outputs ignore inherited bare variables](#step-outputs-ignore-inherited-bare-variables)).
The shell tests pass these values as `RECIPE_VAR_<output>` with the upper-case
names removed, as the runner does.
The step-output reads in the loop recipes (`autodrive-build.yaml`,
`autodrive-crusty-loop.yaml`, `autodrive-merge-loop.yaml`) were converted to
a dual-name form by issue #1511 (PR #1563), and to the `RECIPE_VAR_`-only
form by #1561; see [Reading step outputs](#reading-step-outputs).

**Overlap with #1511.** PR #1520 changed the reads in the two round recipes
before #1511 landed, because its rebuilt steps read outputs that did not exist
before, and it used the dual-name `${UPPER:-${RECIPE_VAR_lower:-}}` form of the
time, which #1561 later replaced. Merging
`main` after #1563 conflicted in six files (the three round and loop recipes,
`test-auto-drive-to-merge.sh`, this page, and
`tests/integration/auto_drive_to_merge_test.rs`). Both sides used the same
read form, so the resolution keeps one copy of each read; #1520's rebuilt
merge-round steps, which call `autodrive_round_evidence.sh`, keep their
fallbacks there. `autodrive-merge-loop.yaml` keeps #1511's unsafe
`state_dir` refusal ahead of #1520's crusty re-review lookup. The shell test
keeps both sides' sections; #1511's are numbered 14 to 16.

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

## How the merge round applies merge-ready

The [`merge-ready`](../../amplifier-bundle/skills/merge-ready/SKILL.md) skill
sets `disable-model-invocation: true`, so no auto-drive recipe invokes it; the
merge round reads its two files. Every criterion applies as written, except
that auto-drive measures criteria 1, 3 and 6 and reads criterion 8 from
`merge_state`, as the skill's `## Running under auto-drive` section says.
Inside auto-drive, criterion 1 follows qa-team's repo-type table (see
[Criterion 1](#criterion-1-qa-team-scenarios-by-repository-type)). A manual
`/merge-ready` still needs `gadugi-test validate` and `gadugi-test run`
(criterion 1) and a `quality-audit` of at least 3 SEEK, VALIDATE, FIX cycles
(criterion 3).

### Round steps

| Step of `autodrive-merge-round.yaml` | Type | Output | Role |
| --- | --- | --- | --- |
| `step-00-tools-dir` | bash | `autodrive_tools_dir` | Finds the round's tools once (see below). |
| `step-00-merge-ready-files` | bash | `merge_ready_files` | Finds `SKILL.md` and its template; where criterion 1 needs `gadugi-test`, stops the round when it is not on `PATH`. |
| `merge-evidence` | recipe | `merge_sync`, `qa_evidence`, `ci_evidence` | `autodrive-merge-evidence`: base sync, suite commands, gadugi scenarios where required, CI wait. |
| `step-00d-qa-evidence-hash` | bash | `qa_evidence_hash` | `autodrive_round_evidence.sh qa-evidence-sha`: hashes `qa-evidence.json` before any agent runs. |
| `step-01-platform-facts` | bash | `platform_facts` | Runs `autodrive_platform_facts.sh`. |
| `step-01b-crusty-evidence` | bash | `crusty_evidence` | `autodrive_round_evidence.sh crusty-evidence`: criterion 3. |
| `step-02-merge-ready-assessment` | agent | `merge_ready_review` | Applies the criteria; emits `merge_ready_verdict`. |
| `step-03-extract-merge-ready-verdict` | bash | `merge_ready_verdict` | Parses the verdict; applies the [measured downgrade](#measurement-outranks-the-model) (`measured-downgrade`) and writes the findings (`findings`). |
| `step-04-address-blockers` | agent | `merge_ready_fixes` | On `NOT_MERGE_READY`: clears blockers, commits, pushes. |
| `step-05-write-round-record` | bash | `merge_round_record_written` | `autodrive_round_evidence.sh round-record` writes the round record under `umask 077`: `merge_ready_verdict`, `blocker_count`, `head_sha`, `round_label`, `test_signal`, `ci_signal`, `qa_status`, `ci_status`, `qa_evidence_sha`. |

`step-00-tools-dir` finds the round's tools once: the first
`amplifier-bundle/tools/` under `$AMPLIHACK_HOME`, `$REPO_PATH`, the git
toplevel, `~/.copilot`, then `~/.amplihack` that holds every one of
`autodrive_merge_ready_files.sh`, `autodrive_platform_facts.sh`,
`autodrive_round_evidence.sh`, `autodrive_state.sh`, `autodrive_trust.sh` and
`git-identity.sh`, so the tools never come from two installs. It prints the
physical path, which later bash steps read as
`"${RECIPE_VAR_autodrive_tools_dir:-}"` and step-04's prompt uses to source
`git-identity.sh`. An `AUTODRIVE_TOOLS_DIR` inherited from an outer run is
never read, so a nested auto-drive run cannot take its tools from the outer
run's tree (#1607). If `RECIPE_VAR_autodrive_tools_dir` is empty, the step
stops before it builds any path, with
`ERROR: autodrive-tools-not-found: autodrive_tools_dir is empty`;
it never falls back to another tree (see
[The tools directory comes from step-00 only](#the-tools-directory-comes-from-step-00-only)). A path holding a quote, backslash,
`$`, backtick, control byte or template brace is skipped, because it reaches
that prompt. No complete root fails the step with
`ERROR: autodrive-tools-not-found: ... (searched <paths>)`, and a later step
whose tool is missing fails with `ERROR: autodrive-tools-not-found: <path>`.
`step-01` also fails, with `ERROR: autodrive-platform-facts-failed`, when the
tool prints no facts. The deterministic step bodies live in
`autodrive_round_evidence.sh` (subcommands `crusty-range`, `qa-evidence-sha`,
`crusty-evidence`, `measured-downgrade`, `findings` and `round-record`), which
sources `autodrive_state.sh` and `autodrive_trust.sh` from its own directory
only. A step that cannot enter `REPO_PATH` fails with
`ERROR: cannot cd to REPO_PATH`. A failed step fails the round with no record
and no manifest row; repeated, it ends the loop `STUCK`. An install error is
never reported as a blocker.

### The merge-ready files

`step-00-merge-ready-files` runs the read-only `autodrive_merge_ready_files.sh`.
It takes the first `skills/merge-ready` directory with a regular `SKILL.md`
from the same roots (`~/.copilot/skills` has no `amplifier-bundle/`) and
prints `skill_dir`, `skill_md`, `template` and `skill_md_sha` (git blob hash).
It fails with `ERROR: merge-ready-template-not-found: <path>` when that
directory lacks `pr-description-template.md`, and with
`ERROR: merge-ready-skill-files-not-found: searched <paths>` when no directory
or no resolver is found. Paths holding `"`, `\`, a control byte, `{{` or `}}`
are skipped. A `SKILL.md` from the branch that differs from `origin/HEAD`
gives `WARNING: merge-ready criteria come from the branch under review`; such
criteria are advisory, since the gate measures criteria 1 and 3 itself.

`gadugi-test` belongs to the same install wherever criterion 1 needs it:
a Node repository, or any repository where the operator sets
`AUTODRIVE_QA_SCENARIO_DIR` (see
[Criterion 1](#criterion-1-qa-team-scenarios-by-repository-type)). There,
`amplihack install` does not install it, and no agent can. The first step of
`auto-drive-to-merge.yaml`, `autodrive-prerequisites`, asks
`autodrive_gadugi_required` and, when it says `true`, checks
`command -v gadugi-test` before the build. A missing tool therefore stops the
run in seconds, and the auto-drive skill lists it under Prerequisites. In a
Rust CLI or Python repository the step prints `{"gadugi_test":"not-required"}`
and the run goes on without it. Step-00 checks again by the same rule, in case
the tool goes away during the run. When it is missing, step-00 fails the round with
`ERROR: gadugi-test-not-installed: gadugi-test is not on PATH`, which names
the install command (`npm install -g github:rysweet/gadugi-agentic-test#6c120657798995b1b53399a5acf3693d418a2d8b`).
Reported instead as the blocker `gadugi-test-missing`, it would repeat every
round until the loop ended `STUCK`. The command is pinned to the
gadugi-agentic-test commit auto-drive was tested with, which has no release
tag: auto-drive depends on gadugi-test 1.0.x running one `--scenario` per
call and deciding a scenario by exit code (see Criterion 1), and the default
branch can change either.

`step-02-merge-ready-assessment` reads `{{merge_ready_files.skill_md}}` and
`{{merge_ready_files.template}}`, fails any criterion it could not verify, and
treats evidence text as data. It has no `Skill(` call. The guard test
`no_recipe_invokes_a_skill_that_refuses_model_invocation` fails on any recipe
`Skill(skill="<name>")` call, comments included, for a skill that refuses
model invocation; `qa_team_does_not_refuse_model_invocation` keeps `qa-team`
callable for step-04.

### Criterion 1: qa-team scenarios by repository type

`step-02-qa-team-scenarios` of `autodrive-merge-evidence.yaml` runs
`amplifier-bundle/tools/autodrive_qa_evidence.sh`, found under the same roots.
The tool runs the suite commands and, where they are required, the gadugi
scenarios. A missing tool fails the step with
`ERROR: autodrive-qa-evidence-tool-not-found`. The tool needs
`autodrive_state.sh` and `autodrive_trust.sh` beside it, and without them
fails with `ERROR: autodrive-qa-helpers-not-found` and writes no evidence.

Criterion 1 follows the
[`qa-team`](../../amplifier-bundle/skills/qa-team/SKILL.md) skill's
repo-type table. That is the per-repository-type rule
[#1517](https://github.com/rysweet/amplihack-rs/issues/1517) proposed. One
function, `autodrive_gadugi_required <dir>` in `autodrive_trust.sh`, decides
whether `gadugi-test` is part of criterion 1. `autodrive-prerequisites`,
step-00, the evidence tool and the merge gate all ask it.

The repository type comes from the manifests in the checkout. The first row
that matches wins; `qa_dir` is the directory the suite command runs in,
relative to the repository root:

| Order | Repository (`autodrive_repo_type`) | Matches when | Suite command when none is configured | `qa_dir` | gadugi scenarios |
| --- | --- | --- | --- | --- | --- |
| 1 | `rust-cli` | Root `Cargo.toml` | `cargo test --workspace --locked --no-fail-fast` | `.` | Not required: qa-team's Rust CLI line substitutes `cargo test` and says not to require gadugi. `qa_scenarios` lists `tests/parity/scenarios/*.yaml`, which `cargo test` runs. |
| 2 | `python` | Root `pyproject.toml` or `setup.py`, with or without a root `package.json` | `pytest` | `.` | Not required: qa-team lists `pytest` for Python. |
| 3 | `node` | Root `package.json`, and none of the files above | `npm test` | `.` | Required. |
| 4 | `rust-cli` (nested) | No root manifest above, no other root project file (see below), and exactly one nested Cargo workspace (see [Nested Cargo workspaces](#nested-cargo-workspaces)) | `cargo test --workspace --locked --no-fail-fast` | The workspace directory, for example `x/y` | Not required. `qa_scenarios` lists `x/y/tests/parity/scenarios/*.yaml`: the detected workspace directory, even when `AUTODRIVE_QA_DIR` sends `AUTODRIVE_QA_COMMAND` elsewhere. |
| 5 | `unknown` | Nothing above, another root project file, or ambiguous nested candidates | none (`qa-command-missing`, `BLOCKED`, unless one is configured) | `.` | Not required. |
| any | any | `AUTODRIVE_QA_SCENARIO_DIR` set | as above | as above | Required: the operator asked for gadugi scenarios. |

Only a root `Cargo.toml`, `pyproject.toml`, `setup.py` or `package.json`
counts as a root manifest that selects a command. Any other root project file
(`requirements.txt`, `setup.cfg`, `tox.ini`, `Pipfile`, `go.mod`, `pom.xml`,
`build.gradle`, `build.gradle.kts`, `Makefile`, `makefile`, `GNUmakefile`,
`CMakeLists.txt`, `meson.build`, `build.sbt`, `Gemfile`, `composer.json`,
`mix.exs` or `flake.nix`, listed in `AUTODRIVE_ROOT_PROJECT_FILES`) gives
`unknown`, and the round is `BLOCKED` unless `AUTODRIVE_QA_COMMAND` is set: a
nested crate beside such a root is a helper, and passing its `cargo test`
would not show that the root project's own tests ran. A root with `Cargo.toml`
and `pyproject.toml` (a maturin project, say) is `rust-cli`. A root with
`package.json` and `pyproject.toml` is `python`: it runs `pytest`, never `npm
test`, and `gadugi_required` is `false`. An operator who still wants gadugi
scenarios there sets `AUTODRIVE_QA_SCENARIO_DIR`. A mixed repository whose
real suite is `npm test` sets `AUTODRIVE_QA_COMMAND='npm test'`.

A root Python or Node project always wins over a nested crate. A repository
with a root `pyproject.toml`, a root `tests/` suite and a helper crate in
`grid-encode-decode/Cargo.toml` runs `pytest` at the root, and `cargo` is
never called. A root `package.json` beside `sub/Cargo.toml` runs `npm test`.
A root `requirements.txt` or `go.mod` beside `c/Cargo.toml` is `BLOCKED`.

This departs from the literal order in
[#1516](https://github.com/rysweet/amplihack-rs/issues/1516), which put the
nested Cargo search before `pyproject.toml` and `package.json`. With that
order, a Python repository with one helper crate ran `cargo test` in the
crate and never ran its own root suite. Searching for nested crates only when
the root has no manifest keeps the issue's fix for a repository whose only
Rust code is nested, such as `WorldModelsExplorations/src/Cargo.toml`.

#### Nested Cargo workspaces

Row 4 searches only when no root manifest matched. It looks for regular files
named `Cargo.toml` at most three levels below the root (`a/b/Cargo.toml` is
found, `a/b/c/Cargo.toml` is not). It skips every directory named `target`,
`worktrees` or `node_modules` and every hidden directory (`.git`, `.venv`), at
any level, and never follows a symlink. It reads no file contents.

The hits are sorted in the C locale, then:

| Hits | Result |
| --- | --- |
| One, in `x/y` | `rust-cli`, `qa_dir` `x/y`. |
| Several, where the first sorted hit's directory is an ancestor of every other hit (`ws/Cargo.toml` with `ws/a/Cargo.toml` and `ws/b/Cargo.toml`) | `rust-cli`, `qa_dir` `ws`. Cargo resolves the members itself, so the root does not need a `[workspace]` table. |
| Several otherwise (`a/Cargo.toml` and `b/Cargo.toml`) | `unknown`, `BLOCKED` with `qa-command-missing`. `qa_summary` reads `ambiguous nested Cargo workspace candidates: a b`. |
| None | `unknown`, `BLOCKED` with `qa-command-missing`. |

Auto-drive does not guess between unrelated crates. To unblock a repository
with several of them, name the suite:
`AUTODRIVE_QA_COMMAND='cargo test --workspace --locked' AUTODRIVE_QA_DIR=a`.

The candidate list in that summary is data, never instructions. Each byte
outside `A-Z a-z 0-9 . _ / -` is replaced with `_`, each candidate is cut to
128 bytes, and after ten candidates the rest are counted as `(+N more)`. The
whole list in the summary is cut to 1500 bytes. The
chosen directory must be relative, use only those characters, and contain no
`..` or empty component. A directory named `my crate` or `$(x)` is never run:
it makes the result ambiguous, so the round is `BLOCKED`.

The command is the same `--locked` command as at the root, so a nested
workspace without a `Cargo.lock` fails with `qa-command-failed`. That is a
real QA failure; commit the lock file.

`autodrive_qa_target <dir>` in `autodrive_trust.sh` makes this choice, and
`autodrive_repo_type` and `autodrive_gadugi_required` are built on it, so the
evidence tool, the prerequisite check and the merge gate always agree. It
prints one line, `<type>\t<field>`: `rust-cli`, `python`, `node` or `unknown`
with the relative directory, or `ambiguous` with the sanitised candidates
separated by single spaces (the BLOCKED summary uses the same list).
`autodrive_repo_type` reports `ambiguous` as `unknown`. The evidence tool
checks the answer again. If it is empty, not one of those tokens, names an
unsafe directory, or disagrees with `autodrive_repo_type`, the tool detects no
command: without `AUTODRIVE_QA_COMMAND(S)` the round is `BLOCKED` with
`qa-command-missing`, and an operator-configured command still runs. Detection
never falls back to running at the root.

```bash
$ . amplifier-bundle/tools/autodrive_trust.sh
$ autodrive_qa_target "$PWD"          # root pyproject.toml + grid-encode-decode/Cargo.toml
python	.
$ autodrive_qa_target "$PWD"          # only ws/Cargo.toml, ws/a/Cargo.toml and ws/b/Cargo.toml
rust-cli	ws
$ autodrive_qa_target "$PWD"          # only a/Cargo.toml and b/Cargo.toml
ambiguous	a b
```

Where gadugi is not required, `gadugi_required` is `false`,
`gadugi_status` is `NOT_REQUIRED`, `gadugi-test` is never called (installed
or not), no scenario directory is searched, and `qa_status` depends on the
suite commands alone. Requiring gadugi in every repository type would stop
a run without `gadugi-test` before the build. In a Rust CLI or Python
repository it would also report `NO_SCENARIOS` until the round committed
gadugi scenario files, and each of those commits would cost a crusty
re-review.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` | unset | One command, run in `AUTODRIVE_QA_DIR`, split into words with globbing off. |
| `AUTODRIVE_QA_DIR` | `.` | The directory `AUTODRIVE_QA_COMMAND` runs in: relative to the repository root, or absolute. Used only with `AUTODRIVE_QA_COMMAND`; detection picks its own directory and ignores it. Recorded as `qa_dir`. A directory that does not exist gives `BLOCKED` with `qa-command-missing` and the summary `AUTODRIVE_QA_DIR <dir> not found`. |
| `AUTODRIVE_QA_COMMANDS` | unset | One command per line, each run as `( cd -- "$ROOT" && bash -c "$entry" )` after `AUTODRIVE_QA_COMMAND`; blank and `#` lines skipped. |
| `AUTODRIVE_QA_SCENARIO_DIR` | the first existing of `tests/agentic`, `tests/gadugi/scenarios`, `scenarios`; else `tests/agentic` | The scenario directory, searched only where gadugi is required. Setting it requires gadugi scenarios in any repository type. An override naming a missing directory means no scenarios. |

With a command variable set, `qa_repo_type` is `configured` and command
detection is skipped; a variable set to the empty string still means no
command. `autodrive_qa_target` and `autodrive_repo_type` still run, because
they decide `gadugi_required` and `qa_scenarios`. In a repository whose only
crate is `ws/Cargo.toml`, `AUTODRIVE_QA_COMMAND=make AUTODRIVE_QA_DIR=sub`
runs `make` in `sub` and records `qa_dir` `sub`, while `qa_scenarios` still
lists `ws/tests/parity/scenarios/*.yaml`.

With no command variable set, the
[repo-type table](#criterion-1-qa-team-scenarios-by-repository-type) decides:

- a root `Cargo.toml` gives `rust-cli` with
  `cargo test --workspace --locked --no-fail-fast`;
- a root `pyproject.toml` or `setup.py` gives `python` with `pytest`, even
  beside a `package.json`;
- a root `package.json` alone gives `node` with `npm test`;
- a single nested Cargo workspace gives `rust-cli`, run in its directory;
- anything else is `unknown`.

`AUTODRIVE_QA_COMMAND` and detected commands run as
`( cd -- "$QA_DIR" && <command> )`; `AUTODRIVE_QA_COMMANDS` entries run from
the root. A detected command counts as installed when `command -v`, run in
`qa_dir`, finds it on `PATH`; where it resolves is not checked. A project
`.venv/bin/pytest` (uv, poetry with in-project virtualenvs,
`python -m venv .venv`) is used when `.venv/bin` is on `PATH`.
`AUTODRIVE_QA_COMMAND` also accepts a `./<first word>` executable relative to
`AUTODRIVE_QA_DIR`, so `AUTODRIVE_QA_COMMAND=bin/check AUTODRIVE_QA_DIR=sub`
runs `sub/bin/check`. That is the way to use a repository-local wrapper:
`AUTODRIVE_QA_COMMAND='./scripts/cargo-wrapper test'`.

**This is not a sandbox, and the installed check is not a security control.**
The suite runs code from the branch whichever binary starts it: `npm test`
runs `package.json` `scripts.test`, pytest imports `conftest.py`, and
`cargo test` compiles and runs `build.rs` and the tests. Refusing a `cargo`
or `pytest` found inside the repository would contain nothing the suite does
not already run, so the tool does not do it. Run auto-drive only on branches
whose code you are willing to execute.

When `AUTODRIVE_QA_DIR` names a directory that does not exist, the command
does not run and is not reported as missing from `PATH`:

```bash
$ EVIDENCE="$(mktemp)"
$ REPO_PATH="$PWD" AUTODRIVE_QA_EVIDENCE="$EVIDENCE" \
    AUTODRIVE_QA_COMMAND='cargo test' AUTODRIVE_QA_DIR=nope \
    bash amplifier-bundle/tools/autodrive_qa_evidence.sh >/dev/null
$ jq -r '.qa_status, .qa_reason, .qa_dir' "$EVIDENCE"
BLOCKED
qa-command-missing
nope
```

`qa_summary` contains `AUTODRIVE_QA_DIR nope not found`, never
`not installed`. The directory name in it is sanitised and cut to 120 bytes.
The suite counts as not run, so the round can never pass this way.
Every command runs, must exit 0, and has no timeout. The variables come from
the environment only, never recipe context
(`no_recipe_declares_the_qa_overrides_as_context`), and only the operator sets
them: an entry is shell source.

Where gadugi is required, after `gadugi-test validate -d "$DIR"` passes, each top-level regular `*.yaml`
or `*.yml` file is copied alone to a `mktemp -d` directory outside the
repository and run as
`(cd "$ROOT" && gadugi-test run -d "$stage" --scenario "$name")`, never as a
whole directory. In gadugi-test 1.0.x, scenarios in one directory share one
CLI runner and the first to finish kills the others
([gadugi-agentic-test #207](https://github.com/rysweet/gadugi-agentic-test/issues/207)),
and `--scenario` matches a substring of `name:` (top-level, or under
`scenario:`). Every file runs, even after one fails.

gadugi-test 1.0.x passes or fails a scenario on the exit codes of its
commands alone. Its CLI agent records a `validate_output` or
`validate_exit_code` step as passed whatever the check returned, and does not
apply the scenario's `assertions:` block (`src/agents/CLIAgent.ts`,
`executeStep`); a command that exits non-zero fails the scenario. So
`gadugi_status` `PASS` means every scenario's commands exited 0, and a
scenario harness must exit non-zero when any case fails. The harnesses in
this repository's `tests/gadugi/` also print each line their scenario's
`validate_output` step and assertions expect, and each `validate_output` step
uses gadugi's `contains:` form, because a plain `expected` string is compared
with the whole trimmed output (`CLIOutputParser.validateOutput`). So the
scenarios still hold if gadugi starts checking them.

`qa_status` is `PASS`, with `qa_reason` `""`, only when a suite command ran,
all passed, and `gadugi_status` is `PASS` or, where gadugi is not required,
`NOT_REQUIRED`. Otherwise `qa_reason` is the first token that applies, and
`qa_summary` names every cause:

| Order | `qa_reason` | `qa_status` | Cause (`gadugi_status`) |
| --- | --- | --- | --- |
| 1 | `qa-command-failed` | `FAIL` | A suite command exited non-zero. |
| 2 | `no-scenarios` | `FAIL` | Gadugi is required and there is no scenario file (`NO_SCENARIOS`). |
| 3 | `gadugi-validate-failed` | `FAIL` | Validate failed; no scenario ran (`VALIDATE_FAILED`). |
| 4 | `gadugi-scenario-unnamed` | `FAIL` | A name is empty, starts with `-`, holds a control byte, exceeds 200 bytes, or is a block scalar or flow mapping (`RUN_FAILED`). |
| 5 | `gadugi-run-failed` | `FAIL` | A run or staging failed, or a symlinked or non-regular entry exists; such entries never run (`RUN_FAILED`). |
| 6 | `qa-command-missing` | `BLOCKED` | No suite command, or incomplete evidence with no other token. |
| 7 | `qa-command-not-installed` | `BLOCKED` | The single or detected command's program is missing. |
| 8 | `gadugi-test-missing` | `BLOCKED` | Gadugi is required and `gadugi-test` is not on `PATH`; no gadugi check ran (`NOT_INSTALLED`). Step-00 stops the round before this when the tool is missing, so in a merge round it means the tool went away during the round; step-04 leaves it to a person. |

The evidence is one JSON line of sanitised strings in `qa-evidence.json`,
written under `umask 077`. A failed temporary file gives
`ERROR: cannot create a temporary log` and no evidence.

| Field | Value |
| --- | --- |
| `qa_status`, `qa_reason`, `gadugi_status`, `qa_summary` | As above. `gadugi_status` is `NOT_REQUIRED` where gadugi is not required. |
| `gadugi_required` | `true` or `false`, from `autodrive_gadugi_required`. The gate decides it again from the checkout's files and does not read this field. |
| `qa_repo_type`, `qa_round` | `configured`, `rust-cli`, `node`, `python` or `unknown`; the round label. |
| `qa_dir` | The directory the suite command ran in, relative to the repository root, with no trailing slash: `.` for a root manifest, `x/y` for a nested Cargo workspace, the `AUTODRIVE_QA_DIR` value with `AUTODRIVE_QA_COMMAND`, and `.` with `AUTODRIVE_QA_COMMANDS` alone or when no command was selected (`unknown`, ambiguous candidates, an empty override). A selected command that could not run (`qa-command-not-installed`, or an `AUTODRIVE_QA_DIR` that does not exist) still records the directory it would have run in. Detection never records an absolute path; an absolute `AUTODRIVE_QA_DIR` set by the operator is recorded as given, sanitised. |
| `qa_command`, `qa_suite_commands_count` | The commands run, joined by `; ` (at most 500 characters, so reference secrets as `$VAR`), and their number. |
| `qa_exit_code`, `gadugi_run_exit_code`, `gadugi_validate_exit_code` | The first non-zero exit code, `"0"` when all passed, `""` when none ran. |
| `head_sha` | `git rev-parse HEAD` before any check: the commit tested. |
| `gadugi_scenario_dir`, `qa_scenarios`, `gadugi_failed_scenarios` | The directory, the counted files, the failed files; relative to the repository root. Where gadugi is not required, the directory is `""`, and in a `rust-cli` repository `qa_scenarios` lists `tests/parity/scenarios/*.yaml` under the detected workspace directory (`.` at the root, `x/y` when nested), not under `qa_dir`. |
| `gadugi_scenario_count`, `gadugi_scenarios_validated`, `gadugi_scenarios_run`, `gadugi_scenarios_passed`, `gadugi_scenarios_failed` | Files found; that count if validate passed, else `"0"`; runs; passes; failures, counting unnamed, unstaged and (after validate) symlinked or non-regular entries. |
| `gadugi_scenario_results` | `<file>=<result>` per entry (`PASS`, `FAIL`, or `INVALID` for not run), sorted, from `autodrive_scenario_results`. Informational only. |

For `NO_SCENARIOS`, `VALIDATE_FAILED`, `RUN_FAILED`, or step-02's
`qa-team-scenarios-do-not-cover-the-change`, step-04 uses `qa-team` and
follows its repo-type table. Where gadugi is required, it writes top-level
`*.yaml` scenarios with a `name:` in `gadugi_scenario_dir`, runs each, and
commits. In a Rust CLI or Python repository it adds the missing coverage to
`cargo test` or `pytest`, because auto-drive runs no gadugi scenario there. A
failing scenario or test is fixed, never weakened or deleted. A directory
outside the repository gives `gadugi-scenario-dir-outside-repo`.

### Criterion 3: the crusty loop ended DONE and CLEAN

Criterion 3 is met when phase 2's `crusty-old-engineer` loop ended `DONE`, its
last loop-written record is `CLEAN`, and every later commit is a base merge or
a description or evidence change. No minimum round count applies
(`no_round_minimum_in_any_recipe_or_tool`), nor does the skill's three-cycle
rule.

Crusty reads `gh pr diff`, the PR head on GitHub, so `step-01-round-context`
of `autodrive-crusty-round.yaml` records the PR's `headRefOid` as the reviewed
commit (`head_sha`, and `reviewed_head_sha` in the record). It fails the round
with `ERROR: crusty-pr-head-unreadable` if that cannot be read, with
`ERROR: crusty-local-head-not-pr-head` when the local `HEAD` differs (the
message says whether local commits have not reached GitHub, #1519, or another
commit is checked out), or with `ERROR: cannot cd to REPO_PATH`. Without a PR
it uses the local `HEAD`. `step-06-write-round-record` writes
`crusty_verdict`, `concern_count`, `commits_this_round`, `head_sha` (post-fix,
else reviewed), `reviewed_head_sha`, `round_label`, `test_signal` and
`ci_signal`, or fails with `ERROR: crusty-head-sha-unavailable`. `autodrive_loop.sh` then copies the
record to `<loop>-latest.json` and, before any later agent runs and only when
the copy matches, appends `label<TAB>file<TAB>git-blob-hash` to
`crusty-records.tsv` or `merge-ready-records.tsv` under `umask 077`. The
manifest is append-only: one row per round, across every loop run on the
directory, and no round reuses an earlier round's label (see
[Why there is no iteration cap](#why-there-is-no-iteration-cap)).

`autodrive_crusty_final DIR` in `autodrive_state.sh` decides criterion 3 for
the crusty re-review, step-01b and the gate. It checks one private copy of the
record and prints the reviewed SHA or the first failing token:

| Order | Check | Token |
| --- | --- | --- |
| 1 | `phases.tsv` has the `crusty-loop` marker. | `crusty-loop-not-done` |
| 2 | `crusty-records.tsv` is a regular file, not a symlink; its last row has a label, a name matching `^crusty-[A-Za-z0-9._-]+\.json$`, and a 40- or 64-hex hash. | `crusty-manifest-missing` |
| 3 | The named record is a regular file, not a symlink. | `crusty-record-missing` |
| 4 | The record and `crusty-latest.json` hash to the manifest hash; the record is one line starting `{"crusty_verdict":"CLEAN",` or `{"crusty_verdict":"CONCERNS",` with one `reviewed_head_sha`. | `crusty-record-modified` |
| 5 | The verdict is `CLEAN`. | `crusty-not-clean` |
| 6 | `reviewed_head_sha` is 40 or 64 hex characters. | `crusty-head-sha-empty` |

`autodrive_crusty_range` in `autodrive_trust.sh` walks
`git rev-list --reverse --topo-order REVIEWED..HEAD ^BASE_SHA` and prints `ok`,
`crusty-unreviewed-commits:<sha>` (the oldest commit needing review, or `HEAD`
when `REVIEWED` is not its ancestor), or `crusty-range-unreadable` (bad SHA,
git error, shallow clone). It allows a two-parent merge of the base whose tree
equals `git merge-tree --write-tree` (git 2.38 or later), and a one-parent
commit whose paths, with modes `100644`, `100755` or `000000`, are all on the
read-only `AUTODRIVE_RANGE_ALLOWLIST`: `PR_DESCRIPTION.md`,
`.github/pull_request_template.md` and the prefix `.autodrive/evidence/`.
Everything else is code, scenario files included. `autodrive_base_sha`
validates the PR's `baseRefName` and force-fetches it, never trusting a local
ref; if that fails, base-merge commits count as code.

Before every merge round, round 1 included, the merge-ready loop runs
`autodrive_crusty_rereview.sh` as its `--before-round` step. It runs
`autodrive_round_evidence.sh crusty-range` in the repository, which prints
`autodrive_rereview_decision` (`rereview`, `range`, `first_unreviewed_sha`,
`base_sha`). Only `crusty-unreviewed-commits` sets `rereview` to `true`, after
`autodrive_clear_phase` removes the `crusty-loop` row; an unclean loop gives
`range` `not-checked`, and an unreadable range is never re-reviewed. On
`true` the tool runs the crusty loop (`autodrive_loop.sh --loop-name crusty`)
on the same state directory. That loop labels its rounds after phase 2's: if
phase 2 ended at `round-2`, the re-review's first round is `round-3`. Phase 2's
records, findings, logs and manifest rows stay as they were, and the
manifest's last row is the re-review's last round. When that loop ends `DONE`,
`autodrive_record_crusty_loop_done` writes the resolved concern ids and the
marker back, exactly as phase 2's step-03 does. The tool prints one JSON line
whose `before_round_result` is one of:

| `before_round_result` | Exit | Merge-ready loop |
| --- | --- | --- |
| `crusty-rereview-not-needed` | `0` | Runs the round. |
| `crusty-rereview-done` | `0` | Runs the round; crusty reviewed the current head. |
| `crusty-rereview-not-done` | `1` | Stops with `loop_result` `BEFORE_ROUND_FAILED`; the marker stays absent, so a resumed run starts with the crusty loop. |
| `crusty-rereview-unavailable` | `1` | The same: the range gave no decision or a tool is missing. |
| `crusty-rereview-refused` | `79` | Stops with `TERMINAL_POLICY_REFUSAL`, exit `79`, never retried. |

The result joins the round's line in the history the loop-health evaluator
reads (`before=crusty-rereview-done`), and the baseline is taken before the
step, so crusty's fixes count as that round's work.

This used to be merge round steps `step-00b-crusty-range` and
`step-00c-crusty-rereview`, a crusty loop nested in the round. The recursion
guard refused it: the round's steps run one recipe runner deeper than the loop
drivers, so the nested crusty round's `recipe run` was issued at depth 4
against the default ceiling of 3 and returned exit `79` on the first code
commit after the clean crusty round (PR #1520 review). See
[Recursion context propagation](#recursion-context-propagation).

`step-01b-crusty-evidence` runs `autodrive_crusty_final` and the range against
the current `HEAD`:

| `crusty_status` | `crusty_reason` | step-02 blocker |
| --- | --- | --- |
| `DONE_CLEAN` | `""` | None; cites `crusty_reviewed_head_sha`. |
| `ABSENT` | `crusty-loop-not-done` | `quality-audit-convergence-crusty-not-done-clean` |
| `NOT_CLEAN` | `crusty-not-clean` | the same |
| `UNTRUSTED` | `crusty-manifest-missing`, `crusty-record-missing`, `crusty-record-modified`, `crusty-head-sha-empty`, `crusty-other` | the same |
| `UNREVIEWED_COMMITS` | `crusty-unreviewed-commits`, `crusty-range-unreadable` | `crusty-review-required:<crusty_first_unreviewed_sha>`, or `crusty-range-unreadable` without a SHA |

Step-04 leaves `crusty-review-required` to the re-review before the next
round (each code commit costs a crusty run) and `crusty-range-unreadable` to a
person, and never rewrites history. Empty, root and octopus commits, hand-resolved merges,
git older than 2.38 and a failed base fetch all fail closed into a re-review.

### Criterion 6: reviews and approvals

`step-01-platform-facts` runs `autodrive_platform_facts.sh [pr]` (#1518). Its
one JSON line holds `pr`, `state`, `is_draft`, `mergeable`, `merge_state`,
`review_decision`, `head_sha`, `base_ref`, `unresolved_threads`,
`required_approvals`, `approval_status` and `approval_source`; a malformed
value is reported as unreadable.

| `reviewDecision` | Other input | `approval_status` | `approval_source` |
| --- | --- | --- | --- |
| `APPROVED` | none | `MET` | `review-decision` |
| `CHANGES_REQUESTED`, `REVIEW_REQUIRED` | none | `NOT_MET` | `review-decision` |
| empty | `required_approvals` is `0`, and no review rule makes it unknown | `MET` | `required-count` |
| empty | `required_approvals` is above `0` | `NOT_MET` | `required-count` |
| empty | count unreadable or unknown; `merge_state` is `CLEAN`, `HAS_HOOKS` or `UNSTABLE` | `MET`; `required_approvals` stays empty, never `0` | `merge-state` |
| empty | count unreadable or unknown; `merge_state` is `DIRTY` or `BEHIND`, or `BLOCKED` with a check in the rollup that has not passed (or no checks), `mergeable` `CONFLICTING`, or an unresolved review thread, outdated or not | `PENDING` | `other-blockers` |
| anything else, or no PR read | none | `UNREADABLE` | `unreadable` |

A check has passed when its `conclusion` (a check run) or `state` (a status
context) is `SUCCESS`, `NEUTRAL` or `SKIPPED`; a run that has not finished has
not passed. GitHub folds every rule into the one `BLOCKED`, so while a
failing check, a conflict, a stale branch or an unresolved review thread is
present, whether a review is also missing cannot be read. A thread counts
because a rule can require conversations to be resolved, and an agent answers
and resolves threads; an unreadable thread count is not a thread. An outdated
thread counts too: it went outdated because the lines it was left on changed,
which does not answer it, and GitHub's conversation-resolution rule requires
every conversation resolved, outdated ones included
([About protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches#require-conversation-resolution-before-merging)).
`unresolved_threads` is that one count, for the merge round and the gate
alike. That is
`PENDING`, which step-02 reports as `approval-pending-other-blockers` and
step-04 treats as a reason to clear the other blockers first, not as a request
for a person; the next round reads the approval again. `BLOCKED` with every
check passed, no conflict, the branch up to date and no unresolved thread
leaves only rules a person satisfies, a required review among them, and stays
`UNREADABLE`. The INFO line shows `mergeable`, `checks=pass|not-passed|none`
and `threads=<count>|unreadable`.

An empty `reviewDecision` does not mean that no review is required. GitHub
publishes a decision only when a rule on the base branch requires at least one
approving review; at a required count of `0` the field stays empty even while
a code-owner, last-push or required-reviewer rule keeps the merge blocked
([Mergify, GitHub Rulesets Compatibility](https://docs.mergify.com/merge-queue/github-rulesets/)).
A count of `0` that comes with such a rule is therefore unknown, not `0`:
`require_code_owner_reviews` or `require_last_push_approval` in classic
protection, and `require_code_owner_review`, `require_last_push_approval` or a
non-empty `required_reviewers` in a ruleset `pull_request` rule. The INFO line
shows it as `unknown(<rule>)`, `required_approvals` stays empty, and the
`merge-state` and `other-blockers` rows decide, so `BLOCKED` gives `PENDING`
or `UNREADABLE`, never `MET`. A count above `0` is kept whatever else the
rule requires, because GitHub publishes a decision for it.

`required_approvals` is the higher of `required_approving_review_count` from
`branches/<base>/protection/required_pull_request_reviews` (`0` when
`branches/<base>` says `"protected": false`) and the highest among
`pull_request` rules in `rules/branches/<base>`; a lone readable count decides
only above `0`. A 404 from the protection endpoint is never read as zero:
GitHub returns it for no rule and for a token without admin rights, as on
rysweet/amplihack-rs (404 `Not Found` while `branches/main` says
`"protected": true`). The `merge-state` fallback holds because GitHub reports
`BLOCKED` while a required review is missing.

Step-02 counts criterion 6 met only for `MET`, otherwise reporting
`changes-requested`, `required-approval-missing`,
`approval-pending-other-blockers` or `approval-state-unreadable`. It reads criterion 8 from `merge_state`: `CLEAN`,
`HAS_HOOKS` or `UNSTABLE` means no protection rule blocks, and a protection-API
404 is not a blocker. Step-03 downgrades with `approval_status=<value>`.
Step-04 cannot approve; it answers requested changes and a person approves.
Without the tool, or with a tool that prints no facts, step-01 fails by name.
The gate's criterion 6 check is unchanged: it refuses `CHANGES_REQUESTED`,
and GitHub refuses a merge missing a required review.

### The qa evidence hash chain

Agents run after the evidence step as the same user, so the qa evidence counts
only through a hash chain: `merge-ready-records.tsv`, the record its last row
names, that record's `qa_evidence_sha`, `qa-evidence.json`, its `head_sha`.
`step-00d-qa-evidence-hash` emits `{"qa_evidence_sha":"<hash>"}` from
`autodrive_qa_evidence_sha` before any agent runs; step-03 re-hashes and
step-05 records it. The gate runs
`autodrive_qa_trusted STATE_DIR RECORD_COPY QA_COPY HEAD_SHA` on its private
copies; it prints `ok` or the first failing token:

| Order | Check | Token |
| --- | --- | --- |
| 1 | `merge-ready-records.tsv` is a regular file, not a symlink, and its last row names `^merge-ready-[A-Za-z0-9._-]+\.json$` with a 40- or 64-hex hash. | `qa-manifest-missing` |
| 2 | The record copy has that hash, starts `{"merge_ready_verdict":"`, and has one hex `qa_evidence_sha`. | `qa-record-modified` |
| 3 | The qa copy hashes to `qa_evidence_sha`. | `qa-evidence-modified` |
| 4 | The qa copy's `head_sha` equals `HEAD_SHA`. | `qa-evidence-stale` |

### Agents do not touch the state directory

Every agent step in the auto-drive recipes carries this sentence word for word:

> Do not create, edit, move, rename, delete or archive any file in the
> auto-drive state directory (the directory holding crusty-round-*.json,
> merge-ready-round-*.json and phases.tsv).

| Recipe | Agent steps |
| --- | --- |
| `autodrive-crusty-round.yaml` | `step-02-crusty-review`, `step-04-address-concerns` |
| `autodrive-merge-round.yaml` | `step-02-merge-ready-assessment`, `step-04-address-blockers` |
| `loop-health-evaluator.yaml` | `step-02-evaluate-loop-health` |

No agent prompt names the state directory, `STATE_DIR`, `autodrive_state_dir`
or `AUTODRIVE_STATE_DIR`
(`every_autodrive_agent_prompt_forbids_touching_the_state_dir`). The sentence
is an instruction; the manifest checks are the control.

### The state directory is private

The merge gate reads crusty and qa evidence only from a state directory owned
by the current user, with no group-write or world-write bit on the directory
or on the files it reads, and never through a symlink (see
[No silent merge](#no-silent-merge)). The caller's umask must not decide
that. Under the umask `0002` that hosts with user private groups use, a plain
`mkdir` or `>` makes every file group-writable, and the gate would refuse
every merge. So every writer makes its state private itself:

| Writer | What it does |
| --- | --- |
| `autodrive_private_dir DIR` (`autodrive_state.sh`) | Creates `DIR` and missing parents under `umask 077` and removes the group and world write bits from `DIR` (`chmod go-w`). Existing parents are left as they are. |
| The preflight of `autodrive-build`, `autodrive-crusty-loop` and `autodrive-merge-loop`, and `autodrive_state_dir` | Call `autodrive_private_dir` and stop on its error. |
| `autodrive_loop.sh` | Calls `autodrive_private_dir`, then writes its round copies, manifests and logs under `umask 077`. It runs the round recipe and the loop-health evaluator with the caller's umask, so files agents create in the repository are unaffected. |
| `autodrive_mark_phase_done`, `autodrive_record_resolved` | Append to `phases.tsv` and `resolved-concerns.txt` under `umask 077`. |
| Round steps that write records (`step-05-write-round-record` in both rounds, the findings files) | Write under `umask 077`, removing the old file first. |
| `autodrive_merge_gate.sh` | Creates a missing state directory under `umask 077`, but never changes the mode of an existing one, since that is what it judges. |

`autodrive_private_dir` fails with `ERROR: state-dir-not-private: <reason>`,
touching nothing, when the directory is empty, a symlink, not a directory, not
owned by this user, or cannot be created or changed.

An entry directly in the directory that the gate would refuse (a symlink, an
entry another user owns, or one with a group or world write bit) was written
by a version of auto-drive that did not make its state private, or by someone
else. It is not evidence, and making it private now would not make it
evidence. `autodrive_private_dir` moves each such entry, unchanged, into a new
private subdirectory `untrusted-<UTC time>.XXXXXX` and prints
`WARNING: state-dir-untrusted-entries-set-aside` naming it. No reader looks in
that subdirectory, so the run redoes what those entries recorded: with
`phases.tsv` set aside, for example, the crusty loop runs again. An entry that
cannot be moved gives `ERROR: state-dir-not-private`.

Section 13 of `test-auto-drive-to-merge.sh` runs these writers under
`umask 0002` and checks that the gate accepts what they wrote.

### Trust model

The operator's environment, including the `AUTODRIVE_QA_*` variables, is
trusted. Branch code, scenarios and agent output are untrusted; branch code
still runs as the operator's user with no sandbox, so only run auto-drive on
branches whose authors you would let run code on this host. The state
directory is integrity-checked through the manifests, not authenticated.
GitHub is authoritative for platform state. The hash checks catch an agent
following instructions in branch text, an accidental edit, and a replayed
record. **They are not a boundary against a deliberate forger running as the
same user**, who still cannot satisfy required CI, review state, or
`gh pr merge --match-head-commit`.

The branch's own manifests decide the repository type. A branch can turn a
Node repository into a Python one by adding `pyproject.toml`, and so turn the
gadugi requirement off. `qa_repo_type` and `qa_dir` in the evidence show the
choice; set `AUTODRIVE_QA_SCENARIO_DIR` to require gadugi whatever the
branch's manifests say. If the nested crate search hits an error, such as an
unreadable directory that could hide a second crate, the result is
`ambiguous` and the round is BLOCKED; it never runs the crate it could read.
The candidate list then names the error as `(search-error)`.
Checking the type at the merge base is tracked in
[#1640](https://github.com/rysweet/amplihack-rs/issues/1640). Detected
commands run in `qa_dir` and are found through the operator's `PATH`; where
the program resolves, inside or outside the repository, is not checked. That
is deliberate: the suite runs the branch's code anyway (`build.rs`,
`conftest.py`, `scripts.test`), so refusing an in-repository `cargo` or
`pytest` would isolate nothing and would block a project `.venv`. Nothing here
is a sandbox, and no part of the installed check should be cited as one.
`cargo` also reads the branch's `.cargo/config.toml` and
`rust-toolchain.toml`. Both are accepted limits: the suite is the branch's own
code, and running it is the point of criterion 1. The tool unsets `CDPATH`, so
`qa_dir` and a relative `AUTODRIVE_QA_DIR` resolve from the repository root,
never in a `CDPATH` directory of the same name. The environment override does
not change the repository type: with `AUTODRIVE_QA_COMMAND` set,
`gadugi_required` still follows the detected type.

Accepted limits: a `scenarios:` list in one file runs in one process (#207); a
scenario using paths relative to its own file fails after staging; without
`git` on `PATH` the loop and the gate fail closed.

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

Every step-output read in the autodrive recipes uses the `RECIPE_VAR_` name
alone, on one line, inside double quotes:

```bash
PREFLIGHT="${RECIPE_VAR_crusty_loop_preflight:-}"
```

If a read ever needs a default, it goes after `:-` as a literal. There are no
quotes inside the braces, so the integration test can match the
`${RECIPE_VAR_<name>:-` text exactly.

No step reads a bare upper-case name in place of its `RECIPE_VAR_` partner,
in these recipes, anywhere else in `amplifier-bundle/recipes` or in
`amplifier-bundle/tools`. The few bare names that are still read are listed
under [What is still read through a bare name](#what-is-still-read-through-a-bare-name). The same holds for
`workflow-publish`, `workflow-finalize` and the [loop-health evaluator](loop-health-evaluator.md#reading-step-outputs-recipe_var-only).
For an object output the runner never sets the bare name, so reading it could
only ever pick up a value inherited from outside the run; see
[Step outputs ignore inherited bare variables](#step-outputs-ignore-inherited-bare-variables).
`:-` treats an empty value as unset, so an empty context default such as
`build_result: ""` reads as empty and the step fails closed. The
`RECIPE_VAR_` value is read only through `extract-json` / `extract-field`; it
is never sourced, `eval`'d or exported.

The difference shows without the runner. With only the `RECIPE_VAR_` name
set, as the runner leaves an object output, the bare read is empty and the
`RECIPE_VAR_` read returns the object, which the helpers accept:

```console
$ unset MY_OUT; export RECIPE_VAR_my_out='{"a":"1"}'
$ echo "[${MY_OUT:-}] [${RECIPE_VAR_my_out:-}]"
[] [{"a":"1"}]
$ printf '%s' "${RECIPE_VAR_my_out:-}" | amplihack orch helper extract-json --require-field a
{"a":"1"}
$ printf '%s' "${RECIPE_VAR_my_out:-}" | amplihack orch helper extract-field --field a --default MISSING
1
```

The shell test `1511-repro` runs these reads and both helpers, and passes only
when all of this output matches and `extract-json` exits `0`. It also sets a
conflicting `MY_OUT` and checks that the `RECIPE_VAR_` read still returns
`{"a":"1"}`.

The rule covers **step outputs only**. Context variables (`REPO_PATH`, `PR_*`,
`AUTODRIVE_*`) and shell locals (`COUNT`, `RC`, `PENDING`, …) that are read
through their bare name alone, never paired with a `RECIPE_VAR_` read, are
read as before. `AUTODRIVE_TOOLS_DIR` is the exception: it is never read (see
[The tools directory comes from step-00 only](#the-tools-directory-comes-from-step-00-only)). The exceptions are `AUTODRIVE_PR_INPUT`, which the composer's
`autodrive-pr-input` step produces as a step output (#1543), so each loop
preflight reads it once as `${RECIPE_VAR_autodrive_pr_input:-}`, and
`build_result`, which the composer's `autodrive-check-unmerged` step reads
(#1551). `autodrive-build.yaml`'s `no_merge_directive` output (#1551) is read only
as a template in step-02's sub-recipe `context:`, never in a shell, so it adds
no row here. Its name joins the checked output set, so a later shell read
must use the `RECIPE_VAR_` form, and [check 3](#the-static-check) forbids
templating it into a `command:`. `autodrive-merge-evidence.yaml` declares
step outputs but reads none, so it has no row.

A read is one `${RECIPE_VAR_<name>:-` occurrence for a name in the checked
output set, so a line that reads the same output twice counts twice. Reads
made inside a bundled tool, such as the merge round's evidence reads in
`autodrive_round_evidence.sh`, are not counted here; that file uses the same
form. There are 35 in all:

| Recipe | Step-output reads |
| --- | --- |
| `auto-drive-to-merge.yaml` | 4 |
| `autodrive-build.yaml` | 1 |
| `autodrive-crusty-loop.yaml` | 7 |
| `autodrive-crusty-round.yaml` | 8 |
| `autodrive-merge-loop.yaml` | 6 |
| `autodrive-merge-round.yaml` | 9 |

### Step outputs ignore inherited bare variables

A recipe's own value always wins over a variable of the same upper-case name
inherited from whatever launched the runner: a parent recipe run, an
interactive shell, or a CI job. Every step body in
`amplifier-bundle/recipes/*.yaml` and every bundled tool in
`amplifier-bundle/tools/*.sh` reads a recipe value as
`${RECIPE_VAR_<key>:-<default>}`, and never reads a bare name in place of its
`RECIPE_VAR_` partner. A stale bare value is ignored even when the recipe's own
value is empty. The only bare name still read next to its partner is the
[`NO_MERGE` override](#no_merge-override), which can only stop a merge. Bare
names with no partner in the same read, such as the `$AMPLIHACK_HOME` and
`$REPO_PATH` roots that step-00 searches, are listed under
[What is still read through a bare name](#what-is-still-read-through-a-bare-name).
`REPO_PATH` has no partner only in step-00; where a step pairs it with
`RECIPE_VAR_repo_path`, as `probe-terminal-state` did, the bare read is
removed like any other.

The example sets both variables for one child shell only, so your shell is
left unchanged:

```console
$ CRUSTY_LOOP_PREFLIGHT='{"state_dir":"/old"}' \
  RECIPE_VAR_crusty_loop_preflight='{"state_dir":"/new"}' \
  bash -c 'echo "${RECIPE_VAR_crusty_loop_preflight:-}"'
{"state_dir":"/new"}
$ CRUSTY_LOOP_PREFLIGHT='{"state_dir":"/old"}' \
  RECIPE_VAR_crusty_loop_preflight='' \
  bash -c 'echo "[${RECIPE_VAR_crusty_loop_preflight:-}]"'
[]
```

#### Setting a value on purpose

A bare variable in the launching shell never sets a recipe value. To set one,
pass it as context:

```bash
amplihack recipe run amplifier-bundle/recipes/auto-drive-to-merge.yaml \
  -c task_description="Fix issue #1561" -c no_merge=true
```

`-c` (long form `--context`) overrides `--context-file` and
`--context-from-file`. Called directly, the runner takes the same pair as
`recipe-runner-rs <recipe> --set no_merge=true`. A parent recipe passes a value
to a sub-recipe through that sub-recipe's `context:` block, never through an
exported bare variable.

#### Why the bare read is dropped, not moved

The bare read was removed rather than moved after the `RECIPE_VAR_` read.
With `${RECIPE_VAR_x:-${UPPER:-}}`, an empty step output would still fall
through to the stale value. Without the bare read, an empty value takes the
read's own default, and every default that a gate depends on fails closed.

The pinned runner (`claude-plugin/recipe-runner.rev`, rev `4d5187ea`) shows
that the bare name never carries anything `RECIPE_VAR_<key>` does not already
hold. It holds a stale value only when the key is undeclared, or when a step
body runs outside the runner, as the shell tests run it:

- `src/context.rs:357` always exports `RECIPE_VAR_<key>` for every context
  value, and `RECIPE_VAR_<key>__<field>` for each nested field of an object.
- `src/context.rs:372-379` and `:399` add the bare upper-case alias only for a
  top-level, non-reserved **scalar**. An object output, such as `qa_evidence`
  or `worktree_setup`, never gets one.
- `src/context.rs:383` never sets a flattened nested name such as
  `PR_PUBLISH_RESULT_PR_URL` or `TERMINAL_STATE_TERMINAL_STATE`. Before #1561,
  any read of such a name could only pick up an inherited value.
- `src/adapters/cli_subprocess.rs:1361-1371` and `:603` start each step from
  the runner's inherited environment and lay the recipe's values over it. A
  bare name the recipe did not set is therefore whatever the launcher had.

So inside a run, for a key the recipe declares in its `context:`,
`RECIPE_VAR_<key>` always holds the recipe's value, and a step that reads only
`RECIPE_VAR_<key>` loses no input it could legitimately receive. For a declared
scalar the bare alias holds the same value, so dropping the bare read changes
nothing; for an object, or an undeclared key, the bare name could only ever be
inherited. Undeclared keys are covered
[below](#every-key-read-is-declared-in-context).

#### How each spelling was rewritten

| Before #1561 | After |
| --- | --- |
| `${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}` | `${RECIPE_VAR_qa_evidence:-}` |
| `${PR_URL:-${RECIPE_VAR_pr_url:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}` | `${RECIPE_VAR_pr_url:-${RECIPE_VAR_pr_publish_result__pr_url:-}}` |
| `${BRANCH_NAME:-${RECIPE_VAR_worktree_setup__branch_name:-}}` | `${RECIPE_VAR_branch_name:-${RECIPE_VAR_worktree_setup__branch_name:-}}` |
| `${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-}}}` | `${RECIPE_VAR_terminal_state__terminal_state:-}` |
| `X="${UPPER:-}"` then `[ -n "$X" ] \|\| X="${RECIPE_VAR_x:-}"` | `X="${RECIPE_VAR_x:-}"` |
| `${UPPER:=${RECIPE_VAR_x:-}}`, `${UPPER-${RECIPE_VAR_x:-}}` | `${RECIPE_VAR_x:-}` |

The rules behind the table:

- A flattened nested bare name (`PR_PUBLISH_RESULT_PR_URL`) is removed; the
  runner never sets it.
- A top-level bare alias (`PR_URL`, `BRANCH_NAME`, `REPO_PATH`,
  `TASK_DESCRIPTION`) that is paired with its `RECIPE_VAR_` partner in the same
  read becomes that partner (`RECIPE_VAR_pr_url`), and the recipe declares the
  key in its `context:`. For example, step-01b of `autodrive-build.yaml` reads
  `${TASK_DESCRIPTION:-${RECIPE_VAR_task_description:-}}`, which becomes
  `${RECIPE_VAR_task_description:-}`.
- A bare name that nothing in the bundle produces (`TERMINAL_STATE`,
  `TERMINAL_REASON`, `ALLOW_NO_OP`, `WORKFLOW_TERMINAL_AGENT_VERDICT`) is
  removed. It is not renamed, so it does not survive as an undocumented input.
- `RECIPE_VAR_` names in a chain keep their order, and a trailing literal
  default (`MISSING`, `false`, `TERMINAL_FAILURE`) stays.
- Every output check after the read stays (`extract-json --require-field`,
  hashes, verdict parsing).
- A `"{{key}}"` fallback after a `RECIPE_VAR_key` read is removed, together
  with any test that compares a value with `"{{key}}"`. Outside a heredoc the
  runner renders `"{{key}}"` as a reference to the same `RECIPE_VAR_key`, so the
  fallback adds nothing and the comparison is always against the value itself;
  see [Template fallbacks compared a value with itself](#template-fallbacks-compared-a-value-with-itself).

#### An empty recipe value wins

An empty `RECIPE_VAR_<key>` no longer falls through to a bare value. This is
deliberate. `autodrive-merge-round.yaml` declares its gate keys with `""`
defaults precisely to shadow a parent's values, and the read now honours that
shadow. No parent recipe in the bundle passes a value only as a bare
environment variable; every one passes it through `context:` or `-c`.

#### Template fallbacks compared a value with itself

Before #1561, the `probe-terminal-state` step of `workflow-terminal-state.yaml`
read each of its inputs through a bare name first and `RECIPE_VAR_<key>`
second. `WORKTREE_INPUT` read `WORKTREE_SETUP_WORKTREE_PATH`, then
`RECIPE_VAR_worktree_setup__worktree_path`, and stopped there. `REPO_DIR` read
`REPO_PATH`, then `RECIPE_VAR_repo_path`, then `"{{repo_path}}"`, and was
replaced by `.` if it still equalled the template. The other five inputs and
`no_merge` added the same template step and were blanked if they still
equalled the template:

```bash
# Before #1561 (workflow-terminal-state.yaml, probe-terminal-state)
BRANCH_INPUT="${BRANCH_NAME:-}"
[ -n "$BRANCH_INPUT" ] || BRANCH_INPUT="${RECIPE_VAR_branch_name:-}"
[ -n "$BRANCH_INPUT" ] || BRANCH_INPUT="{{branch_name}}"
case "$BRANCH_INPUT" in "{{branch_name}}"|"''") BRANCH_INPUT="" ;; esac
```

The pinned runner does not leave `{{branch_name}}` in the script. Outside a
heredoc, `render_shell` (`src/context.rs:225`) passes each line through
`replace_vars_quoted` (`src/context.rs:286`), which replaces `{{key}}` with
`"$RECIPE_VAR_key"`. The `case` line therefore ran as:

```bash
case "$BRANCH_INPUT" in ""$RECIPE_VAR_branch_name""|"''") BRANCH_INPUT="" ;; esac
```

The pattern matches whenever the input equals the recipe's own value, so every
recipe-supplied value was discarded. The same shape affected five inputs and
the failure mode:

| Input | Recipe value | Before #1561 | After |
| --- | --- | --- | --- |
| `repo_path` | a path | replaced by `.` | used |
| `branch_name`, `base_ref`, `pr_number`, `pr_url` | non-empty | blanked, so the git-derived fallback ran | used; the git fallback runs only when the value is empty |
| `goal_already_met` | `true` | reset to `false` | honoured: if meaningful unmerged diff remains, the probe fails with `FAILED_MEANINGFUL_DIFF` |
| `terminal_fail_mode` | `report` | reset to `exit`, so a failure exited `1` | honoured: a failure is reported and the step exits `0` |

The probe now reads every input once from `RECIPE_VAR_<key>`, with no bare
read and no template, and blanks it only when it is empty or the literal `''`:

```bash
WORKTREE_INPUT="${RECIPE_VAR_worktree_setup__worktree_path:-}"
REPO_DIR="${RECIPE_VAR_repo_path:-}"
case "$REPO_DIR" in ""|"''") REPO_DIR="." ;; esac
BRANCH_INPUT="${RECIPE_VAR_branch_name:-}"
case "$BRANCH_INPUT" in ""|"''") BRANCH_INPUT="" ;; esac
GOAL_ALREADY_MET_INPUT="${RECIPE_VAR_goal_already_met:-}"
case "$GOAL_ALREADY_MET_INPUT" in ""|"''") GOAL_ALREADY_MET_INPUT="false" ;; esac
fail_mode="${RECIPE_VAR_terminal_fail_mode:-}"
[ "$fail_mode" = "report" ] || fail_mode="exit"
```

An unset or empty `repo_path` still falls back to `.`, and a non-empty
`worktree_setup__worktree_path` still takes precedence over it as the target
checkout. An inherited `WORKTREE_SETUP_WORKTREE_PATH` or `REPO_PATH` no longer
redirects the probe to another checkout. The
[bundle-wide guard](#the-bundle-wide-guard) rejects a `case` pattern or a
`[ … != … ]` test whose operand is `"{{key}}"`, so the self-comparison cannot
return.

A caller that relied on the old behaviour sees a different outcome: a caller
that passes `terminal_fail_mode=report` now gets exit `0` on a terminal
failure, and a caller that passes `goal_already_met=true` while meaningful
unmerged diff remains now gets `FAILED_MEANINGFUL_DIFF`.

#### What was exposed before #1561

Before #1561, reads were `${UPPER:-${RECIPE_VAR_lower:-}}`, so a non-empty
bare variable hid the recipe's value:

- **Object outputs were exposed in production.** `CI_EVIDENCE`,
  `QA_EVIDENCE`, `QA_EVIDENCE_HASH`, `MERGE_SYNC`, `PLATFORM_FACTS`,
  `CRUSTY_EVIDENCE`, `MERGE_READY_VERDICT` and `WORKTREE_SETUP_WORKTREE_PATH`
  never get a runner alias, so an inherited value of any of them reached the
  step unchanged. This included the merge round's evidence reads in
  `autodrive_round_evidence.sh` and the terminal-evidence reads in
  `workflow_final_status.sh`.
- **Scalar outputs leaked in two cases.** For a scalar such as
  `autodrive_tools_dir`, the runner's alias overwrites a stale inherited
  `AUTODRIVE_TOOLS_DIR` once the recipe sets the key. The stale value leaked
  only when the key was missing from the context, or when a step body ran
  outside the runner, as the shell tests run it. That is the #1607 case: a
  round whose own tools directory had no round tool kept the outer run's
  `AUTODRIVE_TOOLS_DIR` and reported `MERGE_READY` (PR #1588's QA case
  `MERGEREADY-no-round-tool`).

#### The tools directory comes from step-00 only

`AUTODRIVE_TOOLS_DIR` decides which code the merge round runs, so it is never
read. The six steps of `autodrive-merge-round.yaml` that run a tool
(`step-00-merge-ready-files`, `step-00d-qa-evidence-hash`,
`step-01-platform-facts`, `step-01b-crusty-evidence`,
`step-03-extract-merge-ready-verdict` and `step-05-write-round-record`) read
`RECIPE_VAR_autodrive_tools_dir`, which only `step-00-tools-dir` produces after
its path checks. Each step copies it into a local named `TOOLS_D`, never
`AUTODRIVE_TOOLS_DIR`, and checks it before it builds any path:

```bash
TOOLS_D="${RECIPE_VAR_autodrive_tools_dir:-}"
[ -n "$TOOLS_D" ] || { echo "ERROR: autodrive-tools-not-found: autodrive_tools_dir is empty [autodrive-merge-round/step-05-write-round-record]" >&2; exit 1; }
[ -d "$TOOLS_D" ] || { echo "ERROR: autodrive-tools-not-found: autodrive_tools_dir is not a directory [autodrive-merge-round/step-05-write-round-record]" >&2; exit 1; }
T="${TOOLS_D}/autodrive_round_evidence.sh"
[ -f "$T" ] || { echo "ERROR: autodrive-tools-not-found: ${T}" >&2; exit 1; }
```

A failed check exits non-zero, writes no round record and reports no
`MERGE_READY`. The empty and not-a-directory messages name the key and the
step, never the value; a missing tool's message names the tool's path, which
step-00 has already checked. The step never tries another tree (`$PWD`,
`$HOME`, `/` or a bare variable), and it never builds `/autodrive_<tool>.sh`
from an empty directory. `step-00-merge-ready-files` checks
`autodrive_trust.sh` with `[ -f ]` before it sources it.

`step-03-extract-merge-ready-verdict` is the one step where a missing round
tool is not an exit: it still downgrades `MERGE_READY` with
`measured_evidence=unreadable` (`MERGEREADY-no-round-tool`). An empty or
missing tools directory now stops that step too. Before #1561 it reached the
same downgrade by building a path from the empty value.

`step-00-merge-ready-files` also asks `autodrive_trust.sh` whether gadugi-test
is required, and treats a failed call as `G=""`. That stays fail-closed: any
answer other than `false` requires gadugi-test.

#### What is still read through a bare name

- **Context variables with no `RECIPE_VAR_` partner in the read.** For example,
  `${AMPLIHACK_HOME:-${REPO_PATH:-}}` (and the `$AMPLIHACK_HOME`, `$REPO_PATH`
  roots that step-00 searches; `probe-terminal-state` no longer reads
  `REPO_PATH`), `${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-}}`,
  `EXPECTED_PR_TITLE_PREFIX`, `${PR:-${BRANCH:-}}`, `REMOTE_HOST_TYPE`, and a
  lone `$TASK_DESCRIPTION` or `${TASK_DESCRIPTION:-}` as in
  `workflow-prep.yaml`, `workflow-worktree.yaml` and
  `workflow_branch_name.sh`. No recipe value competes with them in the same
  read, so #1561 does not change them. For a declared scalar such as
  `task_description`, the runner's alias already holds the recipe's value.
  Where `TASK_DESCRIPTION` is paired with `RECIPE_VAR_task_description` in one
  read, the read is rewritten as in the table above.
- **A tool's own locals.** `workflow_final_status.sh` assigns `PUBLISH_STATE`
  from `RECIPE_VAR_pr_publish_result__state` and `ISSUE_NUMBER` from
  `RECIPE_VAR_issue_number` near the top of the script, then reuses them as
  defaults further down. They are computed in the same script, not inherited,
  and each carries a comment saying so. `TASK_DESC` is the same kind of local:
  every recipe step and tool that uses it assigns it from `$TASK_DESCRIPTION`
  or `{{task_description}}` first, and `workflow_issue_tracking.sh` reads the
  `TASK_DESC` its caller in `workflow-prep.yaml` set. It is never a
  `RECIPE_VAR_` alias, so #1561 does not touch it.
- **`NO_MERGE`**, as the [override](#no_merge-override) below.
- **Test harnesses.** `amplifier-bundle/recipes/tests/` stands in for the
  runner and sets names on purpose; the guard does not scan it.

#### Every key read is declared in `context:`

The runner also passes inherited `RECIPE_VAR_*` variables through. A nested run
that does not declare a key in its `context:` still sees a `RECIPE_VAR_<key>`
inherited from its parent. Only a `context:` declaration replaces that value
when the run starts. A key declared only as a step's `output:` is replaced when
that step runs, not before, so two reads still see the parent's value:

- a read in a step that runs before the producer;
- any read after a producer that was skipped by its `condition:` or did not
  run.

Each recipe therefore declares every `RECIPE_VAR_` key it reads in its
`context:`, with a `""` default when it has no better one. An `output:` alone
is accepted only when every read of the key comes after its producer in the
same recipe and the producer has no `condition:`. `autodrive-merge-round.yaml`, for example, declares
`ci_evidence`, `qa_evidence`, `merge_sync`, `qa_evidence_hash`,
`merge_ready_verdict`, `platform_facts`, `crusty_evidence` and
`autodrive_tools_dir` as `""`. A key that no step fills stays empty and takes
the read's deny default (`MISSING` or `unreadable`), never the parent's value.
The [bundle-wide guard](#the-bundle-wide-guard) checks these declarations.

The bounds that applied before still apply. A value still goes through
`extract-json --require-field` or `extract-field`, a `state_dir` still goes
through the [check below](#state_dir-is-checked-before-use), and
`autodrive-merge-evidence.yaml` reads no step outputs, so the
[merge gate](#no-silent-merge) cannot be fed through this path.

### `NO_MERGE` override

`NO_MERGE` is the one bare name that is still read, and only as an explicit
override. A bare `NO_MERGE` is combined with the recipe's `no_merge` value as a
logical OR, after the recipe value has been resolved. It can switch merging
off, but it can never switch it on. A stale `NO_MERGE=true` can only stop a
merge. Ignoring an operator's no-merge request would be a worse failure than
the one #1561 fixes.

The override is read in three places:

| Recipe | Step |
| --- | --- |
| `autodrive-build.yaml` | `step-01b-no-merge-directive` |
| `autodrive-build.yaml` | `step-03-resolve-pr` |
| `workflow-terminal-state.yaml` | `probe-terminal-state`, the no-merge block |

Each site normalises both values the same way: it lower-cases the value,
removes all whitespace, and treats `true`, `1`, `yes` and `on` as true.
Anything else, including an unset, empty or unknown value such as `maybe`, is
false. The value reaches the normaliser only as a quoted `printf '%s'`
argument: no `eval`, no `declare -n`, and no command built from the value.
Each step runs in its own process, so each one defines the normaliser locally.
The three definitions are byte-identical, copied from
`workflow-terminal-state.yaml`, and the
[bundle-wide guard](#the-bundle-wide-guard) fails if they drift:

```bash
normalise_no_merge_flag() {
  # $1 structured flag value. Truthy tokens suppress auto-merge; anything
  # else (including empty/false) preserves the default merge behavior.
  local flag
  flag="$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  case "$flag" in true|1|yes|on) printf 'true\n' ;; *) printf 'false\n' ;; esac
}
NM="$(normalise_no_merge_flag "${RECIPE_VAR_no_merge:-}")"
# NO_MERGE override (#1561): a bare NO_MERGE can only switch merging off.
# See docs/reference/auto-drive-to-merge.md#no_merge-override
[ "$(normalise_no_merge_flag "${NO_MERGE:-}")" = "true" ] && { NM=true; echo "INFO: autodrive-build/step-01b-no-merge-directive: no_merge=true from bare NO_MERGE env (override, #1561)" >&2; }
```

The `INFO` line goes to stderr whenever the bare value normalises to true. It
is a fixed string that names the recipe and step and contains
`bare NO_MERGE env`; it prints no inherited value. The three sites print:

| Site | `INFO` line |
| --- | --- |
| `autodrive-build.yaml`, `step-01b-no-merge-directive` | `INFO: autodrive-build/step-01b-no-merge-directive: no_merge=true from bare NO_MERGE env (override, #1561)` |
| `autodrive-build.yaml`, `step-03-resolve-pr` | `INFO: autodrive-build/step-03-resolve-pr: no_merge=true from bare NO_MERGE env (override, #1561)` |
| `workflow-terminal-state.yaml`, `probe-terminal-state` | `INFO: workflow-terminal-state/probe-terminal-state: no_merge=true from bare NO_MERGE env (override, #1561)` |

In `probe-terminal-state`, the normaliser is defined above the input reads,
the result is held in `NO_MERGE_DIRECTIVE` rather than `NM`, because
`active_should_merge` and the rest of the probe already read that name, and the
`{{no_merge}}` template fallback and its `case` line are gone (they compared
the value with itself; see
[Template fallbacks compared a value with itself](#template-fallbacks-compared-a-value-with-itself)).
After the override line, the probe still prints its existing
`INFO: workflow-terminal-state: explicit no-merge directive detected; auto-merge will be suppressed (should_merge=false)`
line whenever the combined directive is true, and `should_merge` is `false`.
Before #1561 the probe read the bare `NO_MERGE` first and the recipe value only
when the bare value was empty, so an inherited `NO_MERGE=false` cleared a
recipe `no_merge=true` and re-enabled merging.

| Recipe `no_merge` | Bare `NO_MERGE` | Merging |
| --- | --- | --- |
| `true` | unset | off, no `INFO` line |
| `true` | `false` | off: a bare `false` cannot re-enable it |
| `true` | `true` | off, with the `INFO` line |
| `false` | `true` | off, with the `INFO` line |
| `false` | `maybe` | on: an unknown value is false |
| `false` | `$(touch "$TMPDIR/pwned")` | on: the value is never run, and no file is created |
| `false` | unset | on, no `INFO` line |
| `""` | `true` | off, with the `INFO` line |
| `""` | ` On ` | off, with the `INFO` line |
| `""` | `false` | on |
| `""` | unset | on |

`autodrive-build.yaml` sets `no_merge: "true"`, so phase 1 never merges,
whatever `NO_MERGE` holds; the override in its two steps is defensive, for a
caller that passes `-c no_merge=` empty. To turn no-merge on for a run, prefer
`-c no_merge=true`. The bare variable exists for operators and CI jobs that
already export it.

The [bundle-wide guard](#the-bundle-wide-guard) allows `NO_MERGE` only in this
exact shape: a separate statement after the `RECIPE_VAR_no_merge` read that
passes `"${NO_MERGE:-}"` to the normaliser, tests `= true`, and acts only
through `&&` by assigning `true` to the directive. The directive is `NM` in
`autodrive-build.yaml` and `NO_MERGE_DIRECTIVE` in `probe-terminal-state`; the
guard accepts either name and no other. The line may not contain `RECIPE_VAR_`,
`false`, `||` or `!=`, and no later line in the step may assign `false` to the
directive.
The `# ... override (#1561)` comment linking this anchor sits on the two lines
above it, and the `INFO` line naming the bare `NO_MERGE` env is within three
lines.

`autodrive-build.yaml` resolves `NM` once per step; the later merge decision in
`step-03-resolve-pr` tests `"$NM"` and does not read `NO_MERGE` again.

### `state_dir` is checked before use

With the `RECIPE_VAR_` read in place, the preflight's `state_dir` reaches later steps
for the first time. Every step that uses it checks it before any `mkdir`,
`rm` or write, and refuses:

- an empty value;
- `/`;
- a value starting with `-`;
- a value containing a newline.

A refused value exits non-zero with this message on stderr:

```
ERROR: no state_dir (empty or unsafe: refusing to touch it) [autodrive-merge-loop/step-03]
```

The bracketed part names the recipe and step. A safe, non-empty value passes
without any `no state_dir` text on stderr; the shell test
`1511-crusty-loop-02` asserts this for `autodrive-crusty-loop.yaml` step-02.
The check runs in:

| Recipe | Steps |
| --- | --- |
| `autodrive-crusty-loop.yaml` | step-02, step-03 |
| `autodrive-merge-loop.yaml` | step-02, step-03, step-04 |
| `autodrive-build.yaml` | step-03 |

`autodrive-build.yaml` step-03 differs in one way: an **empty** `state_dir`
means the preflight supplied none. The step prints
`INFO: no state_dir from the preflight; nothing to record.` on stderr, marks
no phase, and carries on. It still resolves and reports the PR: it exits `0`
when it finds one, and otherwise exits `1` with the usual
`ERROR: phase 1 finished without a pull request.` A non-empty unsafe value
still exits non-zero, and nothing is marked.

The path is always quoted, and because the check refuses a leading `-`, it
can never be read as an option. An absolute path is not required; the
preflight decides where state lives. No step deletes `state_dir` recursively.
Any future step that does must also require an absolute path under the
`autodrive_state_dir` root with no `..` component.

### The static check

The test `autodrive_step_outputs_are_read_from_recipe_var_only` in
`tests/integration/auto_drive_to_merge_test.rs` enforces this section. It
collects the `output:` names declared across all seven autodrive recipes
(`auto-drive-to-merge.yaml` and the six `autodrive-*.yaml` files) into one
set, then checks the files against the whole set. Per-file collection would
miss reads of outputs declared in a sub-recipe: `autodrive-merge-round.yaml`
reads `QA_EVIDENCE`, `CI_EVIDENCE` and `MERGE_SYNC`, which
`autodrive-merge-evidence.yaml` declares.

The checks cover two groups of files:

- **The six reader recipes** are the rows of the read table under
  [Reading step outputs](#reading-step-outputs): every autodrive recipe
  except `autodrive-merge-evidence.yaml`.
- **All seven recipes** adds `autodrive-merge-evidence.yaml`, which declares
  step outputs but reads none.

A name only matches at a name boundary: the next character must not be a
letter, digit or `_`. `QA` never matches `QA_EVIDENCE`, and `qa` never
matches `qa_evidence`.

Two sanity checks come first, so the test cannot pass by looking at nothing:

- fewer than 20 output names fails;
- a reader recipe with no `${RECIPE_VAR_<name>:-` read fails, and the
  failure names the recipe. This runs while the reads are counted, before
  check 1.

Then four checks run, in this order, so a failure reports its most specific
cause:

1. **No read uses the bare name.** On every line of the six reader recipes,
   `${<NAME_UPPER>` in any form, `${#<NAME_UPPER>`, `${!<NAME_UPPER>` and
   `$<NAME_UPPER>` must not appear. Each of these is a violation, because it
   can read a value inherited from outside the run instead of
   `RECIPE_VAR_qa_evidence`:

   ```bash
   ${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}  ${QA_EVIDENCE:-}  ${QA_EVIDENCE}
   ${QA_EVIDENCE-}  ${QA_EVIDENCE:=}  ${QA_EVIDENCE:+x}  ${QA_EVIDENCE:?}
   ${QA_EVIDENCE#*}  ${QA_EVIDENCE%x}  ${QA_EVIDENCE/a/b}
   ${#QA_EVIDENCE}  ${!QA_EVIDENCE}  $QA_EVIDENCE
   ```

   The failure lists each bare read by file, line and output.
2. **Every `RECIPE_VAR_` read has a default and no bare tail.** Each
   `${RECIPE_VAR_<name>` must be followed by `:-`, and its default must not
   be a bare read of the same output: `${RECIPE_VAR_qa_evidence:-${QA_EVIDENCE:-}}`
   is a violation, because an empty step output would fall through to the
   inherited value. A literal default such as `${RECIPE_VAR_x:-MISSING}` is
   allowed. The failure names the file, line and output.
3. **No step output is templated into a `command:`.** This check walks the
   parsed YAML of all seven recipes, not their text lines. In each step
   whose `command:` is a string, `{{` followed by optional spaces and an
   output name fails. The name-boundary rule applies, so a `.`, `}` or space
   after the name ends it: `{{qa_evidence}}`, `{{ qa_evidence }}` and
   `{{platform_facts.pr}}` are all caught. The failure names the file, the
   step `id` and the output. It has no line number, because the parsed YAML
   has none.

   The runner pastes a template value into the script text before bash
   parses it, so an LLM-written output would run as shell code (see [direct
   interpolation in
   bash](quality-audit-cycle-recipe.md#unsafe-pattern-direct-interpolation-in-bash)).
   A step output reaches a `command:` only as an environment variable. A
   `prompt:`, a sub-recipe `context:` value and the recipe-level output
   template are not `command:` text, so `{{build_result.pr}}` there is
   allowed. The check does not follow a value through a sub-recipe's
   `context:` into that sub-recipe's commands. The sub-recipes read such a
   value from their environment (`PR="${PR_NUMBER:-}"`) and never template
   it into a `command:`. Keep it that way: this check would not catch
   `{{pr_number}}` there.
4. **The counts match this page.** Only after checks 1–3 pass does the test
   read the table under [Reading step outputs](#reading-step-outputs) from
   this file: the total from the sentence just above the table, then each
   row. Each reader recipe's counted reads must equal its row, and the
   counted sum must equal the stated total. The rows must name exactly the
   six reader recipes, so a missing row, or a row for any other recipe, is a
   mismatch. The failure prints the counted and documented per-recipe maps
   side by side, with both totals. The page is the only source of truth; the
   test holds no count of its own.

A change that adds or removes a read updates the table and the total in the
same commit. Copy the new numbers from the counted map in the check 4
failure.

The table parse fails closed: the test fails, rather than skipping the count
check, when

- the total sentence is missing or appears on more than one line;
- the total is not a number, or is `0`;
- no table row follows the sentence;
- a row's count is not a number;
- a recipe appears in two rows;
- the rows do not add up to the total.

The table ends at the first line that does not start with `|`. Keep its shape
when editing it: the file name in backticks, then the count. Reword the total
sentence only together with the parser.

The shell contract test `test-auto-drive-to-merge.sh` runs the two sanity
checks (`1511-output-set`, `1511-reads-<recipe>`) and check 1
(`1511-static-audit`). Checks 2–4 run only in the Rust test.

### The bundle-wide guard

The checks above cover the seven autodrive recipes and their declared outputs.
`crates/amplihack-cli/tests/issue_1561_step_outputs_prefer_recipe_var.rs`
covers the whole bundle, with no output list. It flags any spelling in which a
bare upper-case name can win over the `RECIPE_VAR_` name it is paired with. Run
it with:

```bash
cargo test -p amplihack-cli --test cli_integration issue_1561
```

It scans, read-only and non-recursively:

- every `amplifier-bundle/recipes/*.yaml`, and fails if it finds fewer than 20.
  `recipes/tests/` is not scanned: it holds harnesses that stand in for the
  runner and set bare names on purpose;
- every `amplifier-bundle/tools/*.sh`, and fails if it finds fewer than 40.

The minimum counts stop the guard from passing because it scanned nothing.
Six detectors run on each line. In the table, `UPPER` is the upper-case form
of any `RECIPE_VAR_` name on the same line (for `two-line-fallback`, on the
fallback line), so
`RECIPE_VAR_worktree_setup__worktree_path` pairs with
`WORKTREE_SETUP_WORKTREE_PATH`:

| Detector | Flags |
| --- | --- |
| `nested-default` | `${UPPER:-${RECIPE_VAR_x:-…}}`, `${UPPER:=${RECIPE_VAR_x:-…}}` and `${UPPER-${RECIPE_VAR_x:-…}}`, with any default, at any depth of a chain |
| `candidate-loop` | a `for` list with `"${UPPER:-}"` ahead of `"${RECIPE_VAR_x:-}"` |
| `two-line-fallback` | `X="${UPPER:-}"` followed within three lines, for the same local `X`, by `[ -n "$X" ] \|\| X="${RECIPE_VAR_x:-}"`. The three [`NO_MERGE` override](#no_merge-override) sites are exempt only in the override shape |
| `single-line-or` | `X="${UPPER:-}" \|\| X="${RECIPE_VAR_x:-}"` and other `\|\|` chains that try `UPPER` first |
| `bare-last` | `${RECIPE_VAR_x:-${UPPER:-}}`, where an empty recipe value would still fall through |
| `template-self-compare` | a `case` pattern, or an operand of `[ … != … ]` or `[ … = … ]`, written `"{{key}}"` in a shell `command:`; the runner renders it as `"$RECIPE_VAR_key"`, so it compares a value with itself (see [Template fallbacks compared a value with itself](#template-fallbacks-compared-a-value-with-itself)) |

A bare read with no `RECIPE_VAR_` partner is not flagged. Examples are
`${REPO_PATH:-}`, `: "${TMPDIR:=/tmp}"`, a lone `X="${UPPER:-}"` that no
`RECIPE_VAR_` fallback follows, and a tool local such as `${PUBLISH_STATE:-…}`
in `workflow_final_status.sh`, whose name is not the upper-case form of the
`RECIPE_VAR_` name it follows.

Self-tests run each detector on inline strings, one for each flagged spelling
and one for each accepted spelling, so a detector that stops matching fails the
test. They include a four-level chain that interleaves bare and `RECIPE_VAR_`
names, and a `:=` read followed by `${REPO_PATH:-}`. A captured bare name
ends at a word boundary, so the `RECIPE_VAR_` prefix of a lower-case name is
never taken for a bare name: `${RECIPE_VAR_x:-${RECIPE_VAR_y:-}}` is accepted,
and `${FOO_BAR:-${RECIPE_VAR_x:-}}` is flagged.

The guard also checks:

- **The bundle-wide grep finds nothing.** This command must print no line,
  comments included:

  ```bash
  grep -nE '\$\{[A-Z_][A-Z0-9_]*:-\$\{RECIPE_VAR_' \
    amplifier-bundle/recipes/*.yaml amplifier-bundle/tools/*.sh
  ```

  The globs are non-recursive, the same two sets the detectors scan. The
  per-line detectors skip lines that start with `#`; this grep does not, so a
  comment in a recipe or tool that quotes the old form describes it in words
  instead ("bare name first, `RECIPE_VAR_` second"). The grep does not cover
  `docs/` or `tests/`: this page and the `tests/integration/*.rs` fixtures
  quote the old form on purpose, as in
  [How each spelling was rewritten](#how-each-spelling-was-rewritten).
- **No `AUTODRIVE_TOOLS_DIR` read is left.** The name may appear in a comment,
  but no recipe or tool may read it.
- **Every `RECIPE_VAR_<key>` a recipe reads is declared in `context:`.** The
  key must be in the recipe's `context:`. A key that is only a step's `output:`
  passes only when every reading step comes after its producer and the
  producer has no `condition:`; see
  [Every key read is declared in `context:`](#every-key-read-is-declared-in-context).
  Negative fixtures prove that each of these fails: an undeclared read, a read
  in a step before the only producer, and a read after a producer with a
  `condition:`.
- **`ALLOWED_OVERRIDES` holds `NO_MERGE` only.** Each entry is a name and a
  justification, and the justification must name a page under `docs/` that
  exists and mentions the variable. The entry exempts only the
  [override shape](#no_merge-override).
- **`no_merge_override_can_only_tighten`.** Each `NO_MERGE` read must have
  the [override shape](#no_merge-override) exactly, in one of the three steps
  listed there, assigning `NM` or `NO_MERGE_DIRECTIVE`, with that site's
  `INFO` line. The three `normalise_no_merge_flag` definitions must be
  byte-identical. Negative fixtures prove that each of these fails: a nested
  `${NO_MERGE:-${RECIPE_VAR_no_merge:-}}`, a `${NO_MERGE:-false}` default, a
  `${NO_MERGE:=…}` read, a bare-last `${RECIPE_VAR_no_merge:-${NO_MERGE:-}}`,
  and the pre-#1561 terminal-state cascade (`NO_MERGE_FLAG_INPUT="${NO_MERGE:-}"`
  followed by `[ -n "$NO_MERGE_FLAG_INPUT" ] || NO_MERGE_FLAG_INPUT="${RECIPE_VAR_no_merge:-}"`).
- **The test-only opt-out.** `AUTODRIVE_TEST_KEEP_INHERITED` may appear only
  under `amplifier-bundle/recipes/tests/` and `tests/`; see
  [The inherited-variable rows](#the-inherited-variable-rows).

A failure prints one line per read, as source text only, never an environment
value:

```
amplifier-bundle/tools/workflow_pr_ready.sh:5: nested-default: PR_URL="${PR_URL:-${RECIPE_VAR_pr_url:-}}"
```

### The inherited-variable rows

`test-auto-drive-to-merge.sh` proves the precedence by running the step bodies
with a conflicting bare variable set:

| Row | Sets | Asserts |
| --- | --- | --- |
| `1561-stale-CI_EVIDENCE`, `1561-stale-QA_EVIDENCE`, `1561-stale-MERGE_READY_REVIEW`, each with `-reverse` and `-empty` (stale values carry `# deliberately stale (#1561)`) | the bare name to a passing value while the `RECIPE_VAR_` value fails; then the reverse; then a passing bare value with an empty `RECIPE_VAR_` value | `step-03-extract-merge-ready-verdict` reports the `RECIPE_VAR_` value in every direction. An empty `RECIPE_VAR_` value gives the deny default (`NOT_MERGE_READY`, `MISSING`, `unknown` or `unreadable`), never `MERGE_READY` |
| `1561-record-stale-CI_EVIDENCE`, `1561-record-stale-QA_EVIDENCE`, `1561-stale-MERGE_READY_VERDICT`, each with `-empty` (and `-reverse` for the verdict) | the same stale bare values, with the round record written by `step-05-write-round-record` through `autodrive_round_evidence.sh` | the record's verdict, blocker count and evidence fields come from `RECIPE_VAR_`; a passing stale bare value with an empty `RECIPE_VAR_` value never yields a passing gate |
| `1561-stale-MERGE_SYNC-empty`, `1561-stale-PLATFORM_FACTS-empty`, `1561-stale-CRUSTY_EVIDENCE-empty`, `1561-stale-QA_EVIDENCE_HASH-empty` | a passing bare value and an empty `RECIPE_VAR_` value | `NOT_MERGE_READY`, with the deny default named in the downgrade reason |
| `1607-stale-tools-dir-<step>`, one per tool-running step | `AUTODRIVE_TOOLS_DIR` to a decoy directory whose tools append their name to a marker file, print a sentinel and exit `0` | the marker file is never written and the sentinel is never printed; the step runs its tool from `RECIPE_VAR_autodrive_tools_dir`. A positive control first sets the decoy as `RECIPE_VAR_autodrive_tools_dir` and requires the marker file, so a step that stopped early cannot pass. Because a decoy exits `0`, a step's exit status shows nothing; only the marker does |
| `1607-stale-tools-dir-empty-<step>`, `1561-empty-tools-dir-fails-closed` | the decoy as `AUTODRIVE_TOOLS_DIR` and an empty `RECIPE_VAR_autodrive_tools_dir` | each step exits non-zero with `autodrive-tools-not-found`, runs no decoy tool, reports no `MERGE_READY`, and never builds an `/autodrive_*.sh` path |
| `MERGEREADY-no-round-tool-inherited` | the outer run's real `amplifier-bundle/tools` as `AUTODRIVE_TOOLS_DIR`, not scrubbed, and a round tools directory with no round tool | `NOT_MERGE_READY` with `measured_evidence=unreadable`; the inherited tools do not rescue the round |
| `1561-NO_MERGE-bare-true-blocks`, `1561-NO_MERGE-bare-1-blocks`, `1561-NO_MERGE-bare-On-blocks` | bare `NO_MERGE` true in some spelling, recipe `no_merge` `false` or empty | merging is off |
| `1561-NO_MERGE-bare-false-does-not-override-recipe-true`, `1561-NO_MERGE-bare-empty-does-not-override-recipe-true` | recipe `no_merge=true`, bare `NO_MERGE` `false` or empty | merging is off |
| `1561-NO_MERGE-recipe-true`, `1561-NO_MERGE-recipe-YES-normalised` | recipe `no_merge` true in some spelling, no bare value | merging is off |
| `1561-NO_MERGE-recipe-true-no-info` | recipe `no_merge=true`, no bare value | no `INFO` line is printed |
| `1561-NO_MERGE-both-true-blocks` | recipe `no_merge=true` and bare `NO_MERGE=true` | merging is off |
| `1561-NO_MERGE-override-info`, `1561-NO_MERGE-override-info-line` | recipe `no_merge=false`, bare `NO_MERGE=true` | merging is off, and stderr has the `INFO` line naming the bare `NO_MERGE` env |
| `1561-NO_MERGE-neither`, `1561-NO_MERGE-neither-empty`, `1561-NO_MERGE-bare-false-alone` | neither value true | merging is on |
| `1561-NO_MERGE-bare-junk-ignored` | recipe `no_merge=false`, bare `NO_MERGE=maybe` | merging is on: an unknown value is false |
| `1561-NO_MERGE-no-info-when-unset`, `1561-NO_MERGE-no-info-when-unset-line` | recipe `no_merge=false`, no bare value | merging is on and no `INFO` line is printed |

The test normally removes every inherited `AUTODRIVE_*` and `RECIPE_VAR_*`
variable before it starts, so a developer's shell cannot change its result.
`AUTODRIVE_TEST_KEEP_INHERITED=1` skips that removal only. The #1551 merge
guard and the `gh` stubs stay in place, so no row can merge a real PR. Every
scratch file goes under `$TMPDIR`. The decoy tools directory and its marker
file sit in the test's work tree, which is itself `mktemp -d` under
`${TMPDIR:?}`; the decoy is mode `0700` and is removed with the work tree by
the `EXIT` trap. Every line that sets a stale bare value carries the comment
`# deliberately stale (#1561)`, so a reader cannot mistake it for a fixture
leak. With the opt-out, the whole suite still
passes when the outer variables point at the real tools; this is the #1607
reproduction:

```bash
T="$PWD/amplifier-bundle/tools"
AUTODRIVE_TEST_KEEP_INHERITED=1 AUTODRIVE_TOOLS_DIR="$T" RECIPE_VAR_autodrive_tools_dir="$T" \
  bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
```

`tests/integration/auto_drive_to_merge_test.rs` runs the suite in CI three
times, once per `Inherited` mode. None of the three tests is `#[ignore]`.
Each sets the child's environment only through `.env()` and `.env_remove()`,
and asserts exit `0`, no `FAIL` line, and `PASS: MERGEREADY-no-round-tool`:

| Mode | Test | Environment |
| --- | --- | --- |
| `Inherited::Scrubbed` | `auto_drive_contract_shell_test_passes` | the script's own scrub |
| `Inherited::Conflicting` | `auto_drive_contract_shell_test_passes_with_conflicting_inherited_tools_dir` | `AUTODRIVE_TEST_KEEP_INHERITED=1`, `AUTODRIVE_TOOLS_DIR` at a decoy made with `tempfile::Builder::tempdir_in(env!("CARGO_TARGET_TMPDIR"))`, and `RECIPE_VAR_autodrive_tools_dir` at the round's own tools |
| `Inherited::Same` | `auto_drive_contract_shell_test_passes_with_same_inherited_tools_dir` | `AUTODRIVE_TEST_KEEP_INHERITED=1` and both variables at the real `amplifier-bundle/tools` |

#### The terminal-state probe rows

`amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh` runs
`probe-terminal-state` from `workflow-terminal-state.yaml` as the runner
renders it: the harness replaces each `{{key}}` with `"$RECIPE_VAR_key"`, as
`render_shell` does outside a heredoc, and unsets every empty `RECIPE_VAR_`
variable. The probe has no heredoc around its input block, so the harness
emulates only the outside-heredoc rule. Its sixth argument sets the recipe's
`no_merge`. The bare `NO_MERGE` is applied after that unset step, so a row
that passes it as empty exports `NO_MERGE=` and the `bare-empty` row differs
from `bare-unset`.

| Row | Sets | Asserts |
| --- | --- | --- |
| `1561-terminal-recipe-true-bare-unset`, `1561-terminal-recipe-true-bare-false`, `1561-terminal-recipe-true-bare-empty` | recipe `no_merge=true`; bare `NO_MERGE` unset, `false` or empty | `should_merge=false` |
| `1561-terminal-recipe-false-bare-true` | recipe `no_merge=false`, bare `NO_MERGE=true` | `should_merge=false`, and stderr has `INFO: workflow-terminal-state/probe-terminal-state: no_merge=true from bare NO_MERGE env (override, #1561)` |
| `1561-terminal-stale-repo-path` | `RECIPE_VAR_repo_path` at the test repository; bare `REPO_PATH` and `WORKTREE_SETUP_WORKTREE_PATH` at a decoy directory that is not a git repository | the probe runs in the test repository and does not fail with `FAILED_INVALID_INPUT` |
| `1561-terminal-bare-injection` | recipe `no_merge=false`, bare `NO_MERGE='$(touch $TMPDIR/pwned)'` | `should_merge=true`, and `$TMPDIR/pwned` does not exist |
| `1561-terminal-rendered-inputs` | `RECIPE_VAR_pr_number` and `RECIPE_VAR_branch_name` | both values reach the terminal state; neither is blanked |
| `1561-terminal-goal-already-met` | `RECIPE_VAR_goal_already_met=true` with meaningful unmerged diff | `FAILED_MEANINGFUL_DIFF` |
| `1561-terminal-fail-mode-report` | `RECIPE_VAR_terminal_fail_mode=report`, `RECIPE_VAR_repo_path` at a path under `$TMPDIR` that does not exist, and no worktree path | `terminal_state=FAILED_INVALID_INPUT` with `target checkout does not exist or is not accessible`, and the step exits `0` |

Run it with:

```bash
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
```

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

1. An `LC_ALL=C awk` pass finds the **last** step status line in the log, for
   any step: two spaces, any status symbol, a space, a `step-` id, an
   optional ` (<name>)`, then `: `. Step-04 always runs last, so a status
   line of any step after a step-04 block cancels that block.
2. It keeps the last status line only if the whole line matches
   `^  ✓ step-04-enforce-loop-verdict( \([^)]*\))?: completed( \[[^]]*\])?$`,
   which allows the optional parenthesised step name and the optional
   bracketed details suffix and nothing else. Under `LC_ALL=C` the `✓` is
   matched as its three UTF-8 bytes.
3. It takes the line **right after** the status line. Any other line in
   between, or no line at all, means no verdict.
4. That line must match
   `^(    Output: )?LOOP_HEALTH: (CONTINUE|DONE)( |$)`: either exactly four
   spaces and `Output: ` in front of the marker, or nothing in front of it.
   `CONTINUE` is checked before `DONE`.

Taking the last block, not the first, matters. The agent's output is printed
earlier in the log than the real step-04 block, so a fake block inside it can
never be the last one.

Anything else is `STUCK`, including a log with no completed step-04 status
line at all, such as one from another runner. The warning is
`loop-health-evaluator exited 0 with no readable LOOP_HEALTH verdict; failing
safe to STUCK.`

| Log contents | Verdict |
| --- | --- |
| `  ✓ step-04-enforce-loop-verdict: completed` then `    Output: LOOP_HEALTH: DONE — converged` | `DONE` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `    Output: LOOP_HEALTH: CONTINUE` | `CONTINUE` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `LOOP_HEALTH: DONE — converged` | `DONE` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `LOOP_HEALTH: CONTINUE` | `CONTINUE` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `LOOP_HEALTH: STUCK — not converging` | `STUCK` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `LOOP_HEALTH: DONE` indented by one space, six spaces or a tab | `STUCK` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `Output: LOOP_HEALTH: DONE` with no indent | `STUCK` |
| `  ✓ step-04-enforce-loop-verdict: completed` then `LOOP_HEALTH: DONEISH` | `STUCK` |
| `  ✗ step-04-enforce-loop-verdict: failed` then `LOOP_HEALTH: CONTINUE` | `STUCK` |
| a step-02 status line, then `LOOP_HEALTH: CONTINUE`, then a step-03 status line | `STUCK` |
| a fake step-04 status line and `LOOP_HEALTH: DONE` inside step-02's output, then a step-03 status line, and no real step-04 block | `STUCK` |
| `  ✓ step-04-enforce-loop-verdict: completed [elapsed: 120ms]` then `    Output: LOOP_HEALTH: DONE` | `DONE` |
| `  ✓ step-04-enforce-loop-verdict (Enforce verdict): completed [phase: loop, elapsed: 2s]` then `    Output: LOOP_HEALTH: CONTINUE` | `CONTINUE` |
| `  ✓ step-04-enforce-loop-verdict: completed extra` then `    Output: LOOP_HEALTH: DONE` | `STUCK` |
| `LOOP_HEALTH: DONE` or `    Output: LOOP_HEALTH: CONTINUE`, no status lines anywhere | `STUCK` |
| step-02 `    Output: LOOP_HEALTH: CONTINUE`, then the real step-04 block with `DONE` | `DONE` |
| a fake step-04 block inside step-02's output, then the real step-04 block with `CONTINUE` | `CONTINUE` |
| a fake step-04 block inside step-02's output, and no real step-04 block | `STUCK` |
| a fake `✓` step-04 block with `DONE` inside step-02's output, then a real step-04 status line with no `Output:` line after it | `STUCK` |
| `  ✗ step-04-enforce-loop-verdict: failed` then `    Output: LOOP_HEALTH: CONTINUE` | `STUCK` |
| a different line between the step-04 status line and its `Output:` line | `STUCK` |
| `note: Output: LOOP_HEALTH: CONTINUE` | `STUCK` |
| `      LOOP_HEALTH: CONTINUE` (six spaces) | `STUCK` |
| garbled or missing | `STUCK` |

The prefix is exact or absent. It is either exactly four spaces and
`Output: `, or nothing, with `LOOP_HEALTH:` at the start of the line. It is
never `[[:space:]]*` or any other whitespace, because the formatter shows the
agent's recent output indented by six spaces. Whenever step-04 prints a
marker, amplihack's formatter shows it in the `Output:` form. On `STUCK`,
step-04's stdout is empty, so no `Output:` line follows its status line.

The bare form covers a runner that prints amplihack's step status lines but
prints step-04's stdout line unchanged (issue #1512). It adds no way to forge
a verdict. The slot is still the one line right after the last completed
step-04 status line, and anything that could put a bare marker there could
already put the `Output:` form there. A log with no such status line is still
`STUCK` with the warning above, however many bare `LOOP_HEALTH:` lines it
holds. A `DONE` still advances nothing unless the round's own verdict is
clean (see [Advancing needs two independent
signals](#advancing-needs-two-independent-signals)), and a real `STUCK` exits
non-zero, so the reader never runs on it.

In the shell test's loop scenarios, the stub evaluator prints the same two
lines a real run prints. Every row of the table above, including the
bare-form rows, also goes through the real driver: `check_health` in
section 13 of `test-auto-drive-to-merge.sh` has the stub evaluator print the
row's log, runs `autodrive_loop.sh` against it, and checks the verdict the
loop acted on. For every `STUCK` row it also checks that the warning above
was printed.

The reader uses only `awk` features that gawk, mawk and BSD awk share: it
runs under `LC_ALL=C`, matches `✓` as its literal three bytes, and uses
only `index()`, `==` and `~` against fixed regexes, never `gensub` or
`IGNORECASE`. It runs the same under bash 3.2 on macOS.

### These greps never decide on a non-zero exit

The reader runs only when the evaluator exited `0`. `LOOP_VERDICT` is set to
`STUCK` first, and a non-zero `HEALTH_RC` leaves it there whatever the log
says. Setting `loop_health_enforce: "false"` on the evaluator makes step-04
exit `0` on `STUCK`. Its `LOOP_HEALTH: STUCK …` marker goes to stderr, not
stdout, so no `Output:` line follows the step-04 status line and the reader
still gives `STUCK`.
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
| Merge conflicts | `mergeable`, `mergeStateStatus` | not `MERGEABLE`; a `mergeStateStatus` other than `CLEAN`, `HAS_HOOKS` or `UNSTABLE` (`BEHIND`, `BLOCKED`, `DIRTY`, `UNKNOWN` all block) |
| Reviews | `reviewDecision` | `CHANGES_REQUESTED` (a missing required review shows as `BLOCKED` above) |
| Review threads | GraphQL `reviewThreads`, **paginated** | any unresolved thread on any page, outdated or not — **or an unreadable answer** |
| CI | `gh pr checks --json name,state,bucket` | any pending or failing check, zero checks, **or an unreadable rollup** |
| qa-team scenarios | evidence file from this run | `qa_status` other than `PASS`, no evidence file, or evidence whose `head_sha` is missing or is not the SHA being merged |
| qa evidence chain | `merge-ready-records.tsv`, `merge-ready-latest.json`, `qa-evidence.json` | any of them not private; `autodrive_qa_trusted` not `ok` (its token, or `qa-other`; see [The qa evidence hash chain](#the-qa-evidence-hash-chain)) |
| gadugi scenarios | same evidence file, and `autodrive_gadugi_required .` on the checkout's own files, the same files the evidence step measured | Unless the gate's own `autodrive_gadugi_required` says `false` and `gadugi_status` is `NOT_REQUIRED`: `gadugi_status` other than `PASS`, or `gadugi_scenario_count` not a positive integer. An unreadable answer counts as required. |
| Crusty loop | `phases.tsv`, `crusty-latest.json`, `crusty-records.tsv` and its record, in `--state-dir` | no `--state-dir`; the directory or a file not private; no `crusty-loop` marker; `crusty_verdict` not `CLEAN`; `autodrive_crusty_final` failing (its token, or `crusty-other`); `HEAD_SHA` not in the clone; `autodrive_crusty_range` not `ok` (its token, or `crusty-range-other`) |
| merge-ready verdict | round record from this run | not `MERGE_READY`, or captured against a different head SHA |

The review-thread query pages. `reviewThreads(first:100)` with no `pageInfo`
follow-up silently truncates: a PR with 101 threads whose only unresolved one
is the last would report zero unresolved and pass the gate. The count is
written once, in `autodrive_platform_facts.sh`: `gh api graphql --paginate`
with `pageInfo { hasNextPage endCursor }`, the per-page counts summed, and a
page that does not come back as a number makes the whole criterion
unreadable. The merge round reads it in step-01, and the gate runs the copy
beside itself and reads `unresolved_threads`; unreadable, or no tool beside
the gate, is a blocker.

The gate copies `--round-record` and `--qa-evidence` once into a private
`mktemp -d` directory, and every later check reads only the copies. It reads
local state only from an explicit `--state-dir` owned by the current user,
with no group-write or world-write bit on the directory or its files; either
bit alone blocks as `not private to this user`. The workflow's own writers
keep it private whatever the umask
([The state directory is private](#the-state-directory-is-private)). Without `--state-dir` it
writes its evidence bundle to `${TMPDIR:-/tmp}` but reads no state there. It
sources `autodrive_state.sh` and `autodrive_trust.sh` from its own directory
only. The qa-chain, gadugi and crusty rows only add checks;
`merge_gate_keeps_every_existing_block` fails if an earlier block is removed or
loosened.

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

## Merges outside the gate are refused (#1551)

`autodrive_merge_gate.sh` is the only place in this workflow allowed to merge,
so a phase-1 agent cannot skip the crusty and merge-ready phases. A merge
attempted anywhere else is refused by two independent layers: a `gh` shim
first on `PATH`, and a check in the pre-tool-use hook that reads the command
an agent types and does not depend on `PATH`. The agents are also told not to
merge, and a merge that happens anyway ends the run.

### How the guard is applied

`amplihack recipe run` applies the guard before any step runs when the
recipe's `name:` or file stem is `auto-drive-to-merge` or starts with
`autodrive-` (not `autodrivex` or `my-autodrive-build`), or when the
environment carries a non-empty `AMPLIHACK_MERGE_GUARD`; any value, `0`
included, switches it on and none switches it off. The runner puts
`$AMPLIHACK_HOME/amplifier-bundle/tools/gh-merge-guard` first on `PATH` once
and sets `AMPLIHACK_MERGE_GUARD=1`, which every step, agent CLI and nested run
inherits. `amplihack claude` prepends the claude binary's directory, which can
hold the real `gh`, so under the marker the launcher puts the guard back at
`PATH` index 0, and stops with the same errors if it cannot find it.

The run stops before any step when `AMPLIHACK_HOME` is unset or not absolute
(the error names which), the host is not Unix (the bash shim cannot be the
`gh` an agent runs there), the host has no `/proc/self/stat` (macOS, the
BSDs: the shim could never trust the gate, so the run would fail only at the
final merge; tracked in [#1646](https://github.com/rysweet/amplihack-rs/issues/1646)),
the host has no executable `/bin/bash` (NixOS: the shim could not start, so
every `gh` call would fail), the shim is missing or not a regular executable, or
anything the shim trusts is group- or other-writable: the shim, its
directory, `tools/` above it, and the gate and the helpers it sources there
(`autodrive_state.sh`, `autodrive_trust.sh`, `autodrive_unmerged.sh`). The install sets the shim and its directory to
`0755` and removes group and other write from the rest. Nothing skips this:

```text
amplihack: merge guard not found at /home/ryan/.amplihack/amplifier-bundle/tools/gh-merge-guard; refusing to run auto-drive without it (#1551)
```

### Keeping the shim first after a shell profile

The shim works only if it is the first `gh` the agent's shell finds. A
profile that prepends `~/.local/bin`, `~/.npm-global/bin` or Homebrew's
directory (`brew shellenv`) would put a real `gh` ahead of it. So the runner
also exports `BASH_ENV` pointing at `gh-merge-guard/bash-env`. Every
non-interactive bash in the run (a recipe step, an agent's shell tool,
`bash -c`, `bash -lc`) sources that file before its command, and a login
shell does so after its profile. The file puts the guard directory back at
the front of `PATH`. Some agent runtimes reset `PATH` inside the command
itself, after `BASH_ENV` has run. Claude Code's Bash tool is one: it runs
`bash -c 'source <snapshot> && eval …'`, and the snapshot exports the
profile's `PATH`. So the file also defines a `gh` shell function that runs
the shim. Bash finds a function before searching `PATH`. A `BASH_ENV` the
user already had is kept in
`AMPLIHACK_MERGE_GUARD_USER_BASH_ENV` and sourced first. The run stops at
launch when `bash-env` is missing or writable by group or others, like a
missing shim.

`agent_login_shell_resolves_gh_to_the_shim_after_a_profile_reorders_path` in
`crates/amplihack-cli/tests/issue_1551_merge_guard.rs` checks this. It runs
an `autodrive-*` recipe whose step starts `bash -lc` with a `HOME` whose
`.bash_profile`, `.bashrc` and `.profile` prepend a directory holding another
`gh`. It asserts that `type -P gh` is the shim, that `gh` is the guard's
function, and that `gh pr merge 1` exits `3`. The last check is repeated in
a `bash -c` that re-exports that `PATH` inside the command, the way Claude
Code's snapshot does. The same test also pins the limit below: after that
re-export, `type -P gh` is the profile's `gh`, and `env gh pr view 7`
reaches it.

Not covered: an interactive shell (`bash -i`), which does not read
`BASH_ENV`, and `command gh` or a `gh` by absolute path, which skip the
function. The hook check below still refuses the direct forms in all three.

Also not covered: a `gh` run by a program that is not bash after the
command has re-exported the profile's `PATH`. Examples are `xargs`,
`find -exec`, `make`, `env`, `timeout` and a `subprocess.run(["gh", …])`.
The function exists only inside bash. Such a program finds `gh` by
searching `PATH`, and it starts no new bash, so `BASH_ENV` is not read
again. It gets the shim only when no real `gh` comes earlier on the
profile's `PATH`. So with a real `gh` in `~/.local/bin`,
`gh pr list … | xargs -n1 gh pr merge` merges. The hook still refuses the
wrapper forms (`env`, `timeout`, `nice`, `nohup`, `exec`, `command`, `sudo`
… `gh pr merge`), because it skips those wrappers to reach the program word.
It does not see a `gh` started by `xargs`, `find -exec`, `make` or a
subprocess, since the program word there is not `gh`. The phase 1 and phase 2
`MERGED` checks are what catch those.

`BASH_ENV` reaches a repository's own test suite too: an agent's
`cargo test` and the merge round's QA command run inside the guarded run.
A test that runs a gate against a stub `gh` must start without the guard, or
the shim refuses the stub merge. `test-auto-drive-to-merge.sh` unsets
`BASH_ENV`, `AMPLIHACK_MERGE_GUARD` and the `gh` function at its top and
takes the guard directory off `PATH`; its #1551 cases put the guard back on
their own command. `auto_drive_contract_shell_test_passes` runs it under the
guard's environment so that `cargo test` fails if this regresses.

### The pre-tool-use hook check

When `AMPLIHACK_MERGE_GUARD` is non-empty, the pre-tool-use hook
(`crates/amplihack-hooks/src/pre_tool_use/merge_guard.rs`) is a second,
deliberately small layer for the same ordinary forms. It reads the command
of the agent's shell tool (`Bash` in Claude Code, `bash` in Copilot), so it
does not depend on `PATH`. It refuses only these:

- `gh … pr … merge` where `gh` (any path) is the program being run;
- `gh api` on a merge endpoint, forbidden outside the gate: an endpoint
  of `…/pulls/<n>/merge`, `repos/<o>/<r>/merges` or
  `repositories/<id>/merges` (any case, path or URL).

Running `autodrive_merge_gate.sh` is not one of these forms, and the hook
reads no shell options. Running the gate from an agent shell is refused by
the shim's parent and environment check: the shim trusts only a parent that
is `/bin/bash -p <gate>` with a clean environment, and the runner exports
`BASH_ENV` into every agent shell, so the gate's merge is refused (T2-G6 and
`S-G6-*` in `amplifier-bundle/recipes/tests/test-issue-1551-no-merge-outside-gate.sh`).
Doing it with a cleaned environment (`env -i /bin/bash -p <gate>`) is
deliberate evasion and a [non-goal](#what-this-does-not-stop).
`leaves_merge_gate_runs_to_the_shim` asserts that the hook passes gate runs
and inspections alike (`bash -n`, `command -v`, `bash -c '… gate'`).

The program is the first word of each command after splitting at unquoted
`;`, `&`, `|`, parentheses and newlines. Reserved words (`if`, `then`, `{`,
`!`, …), `VAR=value` assignments and the wrappers `env`, `exec`, `command`,
`nohup`, `time`, `nice`, `timeout` and `sudo` (with their options) are
skipped. Nothing else is read. Quoted text (with `\'` escaped inside
`$'…'`), the arguments of other programs, heredoc bodies and comments are
data. So these pass with the guard on, alone
or after a command that runs an interpreter (`bash test.sh && …`,
`cargo test 2>&1 | awk '{print}'; …`):

- `grep -rn 'gh pr merge' .`
- `rg 'pulls/1/merge'`
- `echo gh pr merge 38 > notes`
- `cat <<'EOF' … gh pr merge … EOF`
- `git add …/autodrive_merge_gate.sh`
- `git commit -m '… gh pr merge …'`
- `gh pr create --body …`

`data_mentions_pass_in_every_context` in
`crates/amplihack-hooks/src/pre_tool_use/tests/merge_guard_tests.rs` asserts
this.

The hook is not a shell parser and does not try to be one. Shell text cannot
be read completely without running it: expansion and aliases are resolved at
run time. Code that is not the program word is outside what the hook reads:
`bash -c '…'`, a script file, `eval`, an alias, a variable holding `gh`, and
command substitution (`$(…)` or backticks). The hook keeps a substitution
inside the word it sits in, so in `out=$(gh pr merge 38 --squash)` the program
word is the assignment and the hook does not refuse it. The hook also takes
any `<<` as the start of a heredoc, including a shift inside bare arithmetic
such as `(( n = 1 << 2 ))`, and then does not read the lines after it. The
shim refuses the merge in all of these when it is the `gh` that runs, which
covers the ordinary forms. Deliberate evasion is a non-goal; see the next
section.

### What the shim refuses

`gh-merge-guard/gh` skips leading global flags (`-R`, `--repo` and
`--hostname` take a value; `--` is skipped) and classifies the call:

| Form | Examples | Label |
| --- | --- | --- |
| `pr merge`, any flags | `gh pr merge 38 --squash`; `gh -R o/r pr merge 38`; with `--admin` too, a flag never used anywhere (prohibited) | `gh pr merge` |
| REST merge endpoints, any method (never outside the gate) | `gh api -X PUT repos/o/r/pulls/38/merge`; `gh api repos/o/r/merges` | `gh api merge endpoint` |
| A merge endpoint in another spelling | a full `https://` URL (any host or port), an `api/v3/` prefix, a leading or doubled `/`, a `?query` or `#fragment`, `{owner}/{repo}` placeholders, the `repositories/<id>/` form, any letter case (`PULLS`, `Merge`) | `gh api merge endpoint` |
| An ambiguous endpoint that mentions `pull` or `merge` | `%` escapes, `.` or `..` segments, backslashes, whitespace or control bytes; any endpoint holding `{branch}`, which gh fills in unescaped | `gh api merge endpoint` |
| GraphQL merge mutations | `mergePullRequest`, `enablePullRequestAutoMerge`, `enqueuePullRequest`, `mergeBranch` as whole names, any case | `gh api graphql merge mutation` |
| A GraphQL body the shim cannot read | any endpoint that mentions `graphql` (`graphql?`, a full `https://` URL, GHES `/api/graphql`, any case) with `--input` or `=@` in any argument, combined short flags (`-iF query=@f`) included | `gh api graphql unreadable query` |
| Aliases | `gh alias set`/`import` whose expansion mentions `merge` or `graphql` or holds a `$N` placeholder; a call through an alias that expands to a merge form or holds a placeholder or quotes; any alias lookup while `gh alias list` fails | `gh alias` |
| Extensions | `gh extension install`/`upgrade`/`exec` | `gh extension` |
| Unknown first words | any first word that is neither in the shim's [built-in allowlist](#the-built-in-command-allowlist) nor an alias, or an alias expanding to one: an extension, or a command a newer `gh` added | `unknown gh command or extension` |

An unknown first word gets its own line, because it is not necessarily a
merge attempt. The line names the allowlist to extend:

```text
REFUSED (amplihack #1551): unknown gh command or extension — the first word is not a built-in command in the merge guard's allowlist (classify() in amplifier-bundle/tools/gh-merge-guard/gh) and not a gh alias. Extensions run their own code and could merge past this guard, so auto-drive refuses them. If a newer gh added this command, add it to that allowlist.
```

#### The built-in command allowlist

`classify()` in `gh-merge-guard/gh` lists the `gh` top-level commands and
help topics that cannot merge, checked against `gh` 2.102 (`gh help`). This
includes `environment`, `exit-codes`, `formatting`, `mintty`, `reference` and
`telemetry`. A word missing from the list fails safe: it is refused as
`unknown gh command or extension`, never passed through. So after a `gh`
upgrade, a new command is refused in auto-drive runs until it is added. Adding
one is a one-word change to that `case` line, plus a `parity` line in
`test-issue-1551-no-merge-outside-gate.sh`. Add a command only after checking
that it cannot merge.

Only `gh api` arguments are inspected, so a `gh pr create --body` that
mentions `mergePullRequest` passes through. A refused call never reaches the
real `gh`: the shim prints one fixed line, with the label and never the
arguments, and exits `3`:

```text
REFUSED (amplihack #1551): gh pr merge is only allowed inside autodrive_merge_gate.sh; leave the PR open — auto-drive's merge gate merges after Crusty CLEAN and the merge-ready loop. Do not retry with another gh path, PATH, alias, API client or token.
```

Every other call (`gh pr view`, `gh api repos/o/r/pulls/38/files`, a GraphQL
query that reads `mergeable`) runs `exec "$REAL_GH" "$@"` unchanged. The shim
(`#!/bin/bash -p`, bash 3.2 included, as on macOS) takes the first executable `gh` on an absolute `PATH`
entry, skipping its own directory and any file with the marker line
`# amplihack-gh-merge-guard`. It exits `127` when there is none, or when
`AMPLIHACK_GH_GUARD_DEPTH` is not 1–3 digits or passes 4.

### When the gate's own merge is allowed

A merge form is allowed only when the shim's **direct parent** is the trusted
gate, `gh-merge-guard/../autodrive_merge_gate.sh`. Using `/proc`, all of these
must hold:

1. `/proc/self/stat` field 4 still equals `$PPID`.
2. `/proc/$PPID/cmdline` is exactly `[<bash>, -p, <gate>, …]`, `<gate>` being
   the trusted gate's physical path (no symlink, relative path or `..`).
3. `/proc/$PPID/fd/255` is the same file as the trusted gate.
4. `/proc/$PPID/exe` is a `bash` owned by root and writable by no one else.
5. `/proc/$PPID/environ` has no `BASH_FUNC_*` and no non-empty `BASH_ENV`,
   `ENV`, `SHELLOPTS`, `BASHOPTS`, `PS4`, `LD_PRELOAD`, `LD_AUDIT` or
   `LD_LIBRARY_PATH`. Only key names are compared.

Any check the shim cannot evaluate refuses. A subshell of the gate passes; a
grandparent, a copy of the gate, `exec -a`, `bash -c` and no `-p` are refused.

The gate starts with `#!/bin/bash -p` (never `#!/usr/bin/env bash -p`, which
Linux passes to `env` as one argument) and runs `builtin unset -f gh git jq`.
Phase 3's `autodrive-merge-loop.yaml` loads the gate and `autodrive_state.sh`
from `$AMPLIHACK_HOME` only. Step-03 resolves the gate's
physical path with `pwd -P` and, in a subshell that drops everything check 5
names, runs `exec env -u LD_… /bin/bash -p "$GATE_PHYS" …`, so a session
exporting `which` (Fedora), Lmod's `module` or `LD_LIBRARY_PATH` still merges.

### What the agents are told

The shim is the control; the directive saves agents a wasted round. Its
fixed text is repeated in each prompt below, and the contract tests check
every copy:

```text
[amplihack:no-merge] Do NOT run "gh pr merge" or call the pulls/<N>/merge API, and do not list merging in requirements or acceptance criteria. Leave the PR open. Auto-drive's merge gate merges it after a Crusty CLEAN and the merge-ready loop; any other merge attempt is refused.
```

| Where | How |
| --- | --- |
| `autodrive-build.yaml` step-01b | Builds `no_merge_directive` with `jq -n --arg` (`jq` is required), appending the directive when `no_merge` is `true`, and exits non-zero if `jq` fails or the directive is missing. Step-02 passes it to `default-workflow`. |
| `default-workflow.yaml`, `workflow-prep.yaml` | `no_merge` defaults to `""`. When `true`, acceptance criteria end at "PR open, merge-ready". |
| `workflow-finalize.yaml` | When `no_merge` is `true`: do not merge; stop at merge-ready. |
| `autodrive-crusty-round.yaml`, `autodrive-merge-round.yaml` | The directive line, then a blank line. |

None of these tells an agent to orchestrate ([#1333 guard test](../../crates/amplihack-cli/tests/issue_1333_leaf_steps_do_not_orchestrate.rs)).

### A merge outside the gate ends the run

After phase 1 (`autodrive-build.yaml` step-03, when it built under
`no_merge`) and after the crusty loop (`auto-drive-to-merge.yaml`
`autodrive-check-unmerged`, skipped when the PR was already merged when the
run started), `autodrive_require_unmerged` (in `autodrive_unmerged.sh`, which
`autodrive_state.sh` sources from its own directory) re-reads the PR with
`autodrive_pr_state`. Both checks load `autodrive_state.sh` from
`$AMPLIHACK_HOME` only and exit `1` if it, `autodrive_unmerged.sh` or
`REPO_PATH` is unusable. The PR number must match `^[0-9]+$`. `MERGED` exits
`1` with `#1551 merged outside autodrive_merge_gate.sh`; an unreadable state
is re-read once and then exits `1`.

### What this does not stop

**Non-goal: deliberate evasion with the same token.** The guard covers the
ordinary means an agent uses on its own initiative: `gh pr merge` and
`gh api` on `…/pulls/N/merge` (both forbidden outside the gate), through any
`gh` that `PATH` resolves. It does not
try to stop an agent that sets out to evade it with the same GitHub token
the run uses. Examples are `curl` or another HTTP client sending the token to
the merge endpoint, a script file the agent writes and then runs, and an
inline alias or variable that hides `gh`. Those routes are out of scope for
the shim and the hook. Branch protection on the base branch is the
boundary. The `MERGED` checks after phases 1 and 2 are the backstop: they end
the run when the PR is merged outside the gate.

| Route | Outcome |
| --- | --- |
| An agent CLI profile that reorders `PATH` so a real `gh` comes first (`~/.local/bin`, `~/.npm-global/bin`, `brew shellenv`) | The runner's [`BASH_ENV` file](#keeping-the-shim-first-after-a-shell-profile) puts the shim back first in every non-interactive bash, after a login shell's profile, and its `gh` function answers a `gh` that bash runs when the command resets `PATH` itself (Claude Code's shell snapshot). **Not covered:** after that reset, a `gh` run by a program that is not bash (`xargs`, `find -exec`, `make`, `env`, `timeout`, a subprocess) finds the first `gh` on the profile's `PATH`. It gets the shim only if no real `gh` comes earlier there. The hook still refuses the wrapper forms (`env`, `timeout`, `nice`, `nohup`, `exec`, `command`, `sudo` … `gh pr merge`), since it skips those wrappers. It does not see a `gh` started by `xargs`, `find -exec`, `make` or a subprocess; the `MERGED` checks catch those. In an interactive shell, or through `command gh`, the [hook check](#the-pre-tool-use-hook-check) still refuses `gh pr merge` and `gh api` on a merge endpoint, since it reads the program word, not `PATH`. |
| `gh` by absolute path or `PATH=… gh`, as the program | Refused by the hook check, which matches `gh` at any path. |
| `curl`, `wget` or another HTTP client sending the token to the merge endpoint | Out of scope (the non-goal above). Branch protection and the `MERGED` checks. |
| A merge inside a script file the agent writes and then runs, `bash -c '…'`, `eval`, an alias, a variable holding `gh`, a command substitution (`out=$(gh pr merge …)`), or a line after a `<<` the hook took for a heredoc | The shim refuses it when it is the `gh` that runs, which `BASH_ENV` keeps true in non-interactive bash. Otherwise out of scope: the hook reads only the program word. Caught by the phase 1 and phase 2 `MERGED` checks. |
| An agent CLI that runs no amplihack hooks | Only the shim applies. |
| A same-user edit to the gate or the shim | Not prevented. Use a `CODEOWNERS` entry for both and review changes. |
| Forged `--round-record` or evidence files | Not prevented. The gate re-checks platform facts. |
| `git push` to the base branch, or `gh api -X PATCH …/git/refs/heads/<base>` | Pushes, not merges: branch protection is the boundary. In phases 1–2 the `MERGED` checks catch them. |
| A same-user process that preloads a library into, or attaches a debugger to, a gate it launched itself | Not prevented. As deliberate as calling `gh` by absolute path. |
| An agent running the installed gate itself, as `/bin/bash -p <gate> --pr N` | Refused by the shim: an agent shell carries the runner's `BASH_ENV`, so the gate's parent environment is not clean and the shim refuses its merge. The [hook check](#the-pre-tool-use-hook-check) does not read gate runs. Clearing the environment first (`env -i /bin/bash -p <gate>`) gets past the shim, which then trusts the gate like phase 3; that is deliberate evasion, the stated non-goal. The gate still re-reads CI, conflicts and review threads from GitHub, so it merges only a PR that passes them, but its Crusty and QA records are files and can be forged (below). |
| A merge during phase 3 | The gate treats an already-`MERGED` PR as success, so it is not reported. |
| An extension binary run directly from `~/.local/share/gh/extensions` | Not refused, like `gh` by absolute path. Caught by the `MERGED` checks. |
| A fake `jq`, `git` or `gh` planted in a `PATH` entry ahead of the system one | The gate finds its tools through `PATH`, so it could misread CI. Not prevented; the gate's facts come from GitHub, and branch protection is the boundary. |
| No `/proc` (macOS, BSDs) | `amplihack recipe run` stops the run at launch, before any step: the shim could never trust the gate's merge. Auto-drive does not run on macOS until [#1646](https://github.com/rysweet/amplihack-rs/issues/1646) adds a parent check that works there. |
| A `/proc` where the parent's `environ` cannot be read, or a `/bin/bash` not owned by root | Fails closed: every merge is refused, the gate's included. |
| No `/bin/bash` at all (NixOS) | `amplihack recipe run` stops the run at launch: the shim cannot start, so every `gh` call in the run would fail. |
| A top-level command a newer `gh` added | Refused as `unknown gh command or extension` until it is added to the [allowlist](#the-built-in-command-allowlist). |

### Troubleshooting

- **`merge guard not found` before step 1.** Set `AMPLIHACK_HOME` to an
  absolute path, rerun `amplihack install`, and check
  `stat -c %a "$AMPLIHACK_HOME/amplifier-bundle/tools/gh-merge-guard/gh"`
  prints `755`.
- **The gate ends `NOT_MERGED — gh pr merge failed (exit 3)`.** The shim
  did not trust the gate. Check that `readlink -f "$(command -v gh)"` and the
  gate path step-03 resolved come from one bundle tree and that `/bin/bash`
  is root-owned. Then rerun phase 3.
- **An agent's non-merge `gh api` call is refused.** It mentions `pull` or
  `merge` with `%`, `.`/`..`, a backslash or whitespace (use the plain
  `repos/<owner>/<repo>/…` path), or it is a `gh` extension.
- **`unknown gh command or extension` after a `gh` upgrade.** The new command
  is missing from the [allowlist](#the-built-in-command-allowlist). Check that
  it cannot merge, then add it.
- **`auto-drive needs /proc/self/stat` before step 1.** The host is macOS or
  a BSD. See [#1646](https://github.com/rysweet/amplihack-rs/issues/1646).
- **The run exits `1` with `#1551 merged outside`.** Something merged the PR
  before phase 3; review the merge in the PR's timeline.

## Exit code 79 is terminal

Exit `79` and `BLOCKED_TERMINAL` are final answers from a structural guard
(#1327 / #1332), not infrastructure hiccups. Every child invocation is checked
for them; when one appears the loop stops, surfaces the refusal, and exits `79`
itself so a parent sees a policy refusal rather than a generic failure. It is
**never** retried into — not at a deeper level, and never with a raised
ceiling.

A log line that only quotes the refusal is not one. Agent output reaches a
round log as `[HH:MM:SS] [amplihack:...]` lines, and crusty's review of PR #1520
quoted `BLOCKED_TERMINAL orchestration_unavailable: depth 4 of max 3` in its
verdict. `terminal_refusal` in `autodrive_loop.sh` drops those lines, then
looks for the guard's own `BLOCKED_TERMINAL orchestration_unavailable` text;
reading any `BLOCKED_TERMINAL` as a refusal ended the loop on a review that
merely discussed one.

`terminal_refusal()` and the loop-health evidence collector both remove text
that comes from inputs before their text checks (#1510). They cut the log at
the runner's run summary header (`Recipe: <name>` then `Run ID:` or
`Status:`), because the summary's `Steps:` block reprints each step's output,
agent prose included, unprefixed at column 0. Then, with one ERE, they drop
agent lines and the runner's `amplihack.recipe.log_pointer {...}` lines,
which echo the run's `task_description`. A log_pointer `exit_code` of 79 is dropped with its
line on purpose, because the round's own exit code is the structural signal.
The collector's token check, like `terminal_refusal()`'s,
requires the full `BLOCKED_TERMINAL orchestration_unavailable` marker. The
round log is handed to it through `autodrive_round_tail.sh`, which drops the
partial first line of the 20000-byte tail and keeps the summary header when
a long summary would push it out of the window. See
[Agent output lines are not refusals](loop-health-evaluator.md#agent-output-lines-are-not-refusals).

## Recursion context propagation

`autodrive_loop.sh` exports `AMPLIHACK_TREE_ID` and `AMPLIHACK_SESSION_DEPTH`
to every child unchanged, and re-exports the inherited `AMPLIHACK_MAX_DEPTH`
verbatim. `assert_ceiling_untouched` runs before every round and before every
evaluator call; if `AMPLIHACK_MAX_DEPTH` has changed, the loop aborts rather
than continuing under a ceiling it did not inherit.

Because rounds are sequential rather than nested, depth does not grow with the
number of rounds.

Every `amplihack recipe run` starts a runner one session level deeper, and the
runner hands every step, bash or agent, one level more (`build_child_env` in
recipe-runner-rs). A `type: recipe` step runs its sub-recipe inside the same
runner and adds nothing. The guard in `amplihack recipe run` refuses at
`depth >= max_depth`, 3 by default, and the environment can only lower that.
Started from a shell at depth 0:

| Process | `AMPLIHACK_SESSION_DEPTH` | May it issue `recipe run`? |
| --- | --- | --- |
| `amplihack recipe run auto-drive-to-merge` | 0 | yes |
| its runner | 1 | |
| the loop drivers' bash steps, `autodrive_loop.sh`, its `--before-round` step and the crusty loop that step starts | 2 | yes, this is where every round and every evaluator is started |
| a round's runner (`autodrive-crusty-round`, `autodrive-merge-round`) | 3 | |
| that round's steps, bash and agent | 4 | **no**: refused with exit `79` |

So no round recipe, and no recipe a round reaches through `type: recipe`
steps, may start a loop or any other `recipe run`. The guard test
`no_round_recipe_starts_a_loop_or_a_recipe_run` checks that, and
`a_recipe_run_from_a_round_step_is_refused_by_the_real_guard` runs the real
`amplihack recipe run` against a sealed ceiling of 3: refused with exit `79`
at depth 4, admitted at depth 2 with its runner handed depth 3.

## Resumability

A run that dies partway is re-runnable. Nothing merged is redone and no
resolved concern is reopened.

| Store | Path | Role |
| --- | --- | --- |
| Local | `${AMPLIHACK_STATE_DIR:-~/.amplihack/state}/auto-drive/<key>/` | Phase completions, resolved concern ids, round records, manifests, evidence bundles. Written only by the host that ran them. |
| Platform | `gh pr view --json state,mergedAt` | The **authority** on whether the PR is merged. |

Local state is a cache, never a claim: the merge gate re-verifies every
criterion regardless of what any state file says, and `UNKNOWN` platform state
is treated as a failure rather than as "not merged". The exception is the
crusty state (`crusty-loop` marker, `crusty-latest.json`, `crusty-records.tsv`
and its records): nothing on the platform records crusty's judgement, so the
gate reads it as criterion-3 evidence through the manifest hash. A run that
dies after the crusty re-review removed the marker resumes with the crusty
loop. A loop started on a directory that already holds its rounds labels its
own after them, so a resumed run adds records and manifest rows and rewrites
none.

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

## The run report

`autodrive-report` reports the merge the run made, even when earlier phases
were skipped as already done. A run given `pr_number` for an open PR whose
build and crusty phases are already complete writes the human-readable line to
**stderr** and only the JSON to **stdout**. The JSON is the step's output:

```
AUTO_DRIVE_TO_MERGE: MERGED — PR #27 (crusty loop: unknown, merge gate: MERGED)
```

```json
{"outcome":"MERGED","pr":"27","crusty_loop":"unknown","merge_result":"MERGED"}
```

Each phase result is read from its `RECIPE_VAR_` name only:
`${RECIPE_VAR_build_result:-}`, and the same for `merge_gate_result` and
`crusty_loop_result`. An inherited `BUILD_RESULT`, `MERGE_GATE_RESULT` or
`CRUSTY_LOOP_RESULT` is ignored. A skipped phase leaves
its own result empty, so the report does not depend on it. These fallbacks,
and with them the `merge_result` read that the #1543 incident showed as
`NOT_MERGED` after a real merge, came from #1511 (PR #1563). #1543 changes
only where `pr` comes from. Cases T20 and T21 of
`test-issue-1543-pr-head-worktree.sh` run the composed recipe through
`recipe-runner-rs` and check the report: T21 with the build phase skipped
because the PR is open, the crusty loop skipped because it is recorded done,
and a merge gate that merges.

`pr` is the first non-empty value of:

1. `RECIPE_VAR_pr_number`. By this step it holds the build phase's PR,
   because the loops hand back the `build_result.pr` they were given; on a
   run given `pr_number` that is the same PR. An inherited bare `PR_NUMBER`
   is not a candidate;
2. the merge gate result's `pr`;
3. the build result's `pr`.

The chosen `pr` must be all digits; anything else is reported as `""`.
`merge_result` and `crusty_loop` must match `^[A-Za-z0-9_-]*$`; anything else
becomes `NOT_MERGED` or `unknown`. The outcome mapping is:

| `merge_result` | `outcome` |
| --- | --- |
| `MERGED`, `ALREADY_MERGED` | `MERGED` |
| `DRY_RUN` | `DRY_RUN` |
| anything else | `NOT_MERGED` |

## No short timeouts

No step in any of these recipes declares `timeout` or `timeout_seconds`, and no
recipe declares a `default_step_timeout` (issue #439 — the runner owns the
ceiling). The CI wait polls on a 60-second interval and stops when CI reaches a
terminal state or becomes unreadable; it is bounded by the build finishing, not
by a stopwatch. Test suites, builds, and model calls run to their natural end.
The guard test `no_short_timeouts_anywhere` fails the build if a seconds-scale
or single-digit-minute bound is introduced.

## Where phases 2 and 3 run

Rounds edit, stage, commit and push. They do all of that in the pull request's
own **linked worktree**, never in the checkout the workflow was launched from,
so the launch checkout's index is never touched (issue #1528).

| Work | Runs in | Commits? |
| --- | --- | --- |
| Phase 1, `autodrive-build` | Launch checkout | Not there. `default-workflow` commits in its own task worktree (issue #858); its commit steps refuse to stage or commit anywhere other than the top of a linked worktree (issue #1528). |
| Loop preflights, `autodrive_pr_worktree.sh resolve`, the merge gate, phase bookkeeping | Launch checkout | No. The resolver may run `git worktree add`; the merge happens on GitHub. |
| The judging agents: `step-02-crusty-review`, `step-02-merge-ready-assessment`, and the `loop-health-evaluator` analyzer | Start in the main checkout of the preflight repo's repository (`review_dir`); read the pull request through `gh pr diff` and `git -C <worktree>` | No |
| Round bash steps, the `step-04` fix agents, `autodrive-merge-evidence`, the evaluator's measurements (recipe files are always loaded from the launch-side bundle; see below) | Pull request worktree | Yes: the round fixes in `step-04`, and the base-branch merge and push in `autodrive-merge-evidence` |

The loop state directory stays keyed to the launch repository, so resuming is
unchanged.

### Why the launch checkout was wrong

`auto-drive-to-merge.yaml` passes `repo_path: "."` to both loops. Before this
change each loop handed that path to `autodrive_loop.sh --repo` and to every
round as `repo_path`, so `step-04` ran its agent in the launch checkout, which
is usually the repository's main checkout on `main`. The prompt told the agent
to run the guard on `.`, then `git add -A`, then commit and push.

The pull request's files live under that checkout's ignored `worktrees/<branch>/`.
There, `git add -A` stages nothing, and naming a path is refused as ignored. The
only way forward is a forced add, which records the nested checkout as a
mode-`160000` gitlink, or sweeps up every ignored file under the pathspec. The
guard ran only before staging, so it never saw either.

### How the worktree is resolved

Each loop's `step-02` resolves the worktree once, after it reads the pull
request number and before the loop starts. `$REPO` is the repo the loop's
preflight (`step-01`) reports: the pull request head's worktree when a pull
request number was supplied (#1543), else `repo_path`:

```bash
PR_WORKTREE="$(bash "$(dirname "$LOOP_HELPER")/autodrive_pr_worktree.sh" resolve --repo "$REPO" --pr "$PR")" || { echo "STOP: cannot resolve the PR's linked worktree; rounds never run in the launch checkout (#1528)" >&2; exit 1; }
```

and starts the loop with:

```bash
bash "$LOOP_HELPER" ... \
  --repo "$PR_WORKTREE" \
  --context "repo_path=$PR_WORKTREE" \
  --review-dir "$REVIEW_DIR" \
  ...
```

Every round step that uses `{{repo_path}}` therefore runs in the pull request
worktree. So does `autodrive-merge-evidence`, which enters `repo_path` before it
merges the base branch and pushes, and `autodrive_loop.sh` reads each round's
baseline `HEAD` there. The judging agents use `{{review_dir}}` instead; see
[Where the judging agents start](#where-the-judging-agents-start). When resolution fails, `step-02` exits `1` and no round
runs. There is no fallback to `REPO_PATH`.

### Where the round and evaluator recipes come from

Running in the pull request worktree does not mean loading recipes from it.
`amplihack recipe run <name> --working-dir DIR` looks the name up in
`DIR`'s repository's `amplifier-bundle/recipes` first (`recipe_search_dirs` in
`crates/amplihack-cli/src/commands/recipe/resolve.rs`). In a repository that
ships these recipes, amplihack-rs itself, a by-name call with the pull request
worktree as `--working-dir` would run the pull request's own
`autodrive-crusty-round`, `autodrive-merge-round` and `loop-health-evaluator`.
The change under review would then supply its own review prompt and its own
stop rule, and a round's fix-up commit would change how the next round runs.

`autodrive_loop.sh` therefore never passes a bare recipe name to `recipe run`.
Before round 1 it runs `autodrive_worktree_guard.sh loop-preflight`, from its
own `tools/` directory, which resolves `--round-recipe` and
`loop-health-evaluator` to absolute paths in the `amplifier-bundle/recipes`
beside that directory (the launch-side bundle), and the loop runs those paths. A recipe run by path searches its
own directory first for sub-recipes, so `loop-evidence-collector` and
`autodrive-merge-evidence` come from that bundle too. Only `--working-dir`,
`repo_path` and the baseline `HEAD` point at the pull request worktree.

The loop refuses, with `"loop_result":"UNTRUSTED_RECIPE"` and before any round
runs, when:

- `--round-recipe` is a path to any file other than the launch-side bundle's
  own copy (step-02 of both loops passes that copy by absolute path, #1543;
  any other path exits 2); or
- the resolved recipe lies inside `--repo`, which means the loop helper itself
  was picked up from the tree under review (exit 1).

`amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` covers both refusals
and checks that a pull request worktree whose own copies of these recipes have
been tampered with still gets the launch-side copies
(`1528-launch-side-recipes`).

### Where the judging agents start

Recipe files are not the only instructions an agent takes from where it
starts. The agent CLI also loads `AGENTS.md`, `CLAUDE.md` and
`.claude/settings.json` hooks from its working directory, and in this
repository `AGENTS.md` is tracked. An agent started in the pull request
worktree is therefore instructed by the branch it is judging, and a round's
fix-up commit could change the instructions given to the next round's
reviewer. That is the same self-approval path as loading the recipes from the
worktree, so it is closed the same way.

Only the commit step moves into the pull request worktree. The read-only
judging agents — the crusty review, the merge-ready assessment and the
loop-health analyzer, which together decide the Crusty `CLEAN`, `MERGE_READY`
and when a loop stops — start in the **review dir**: the main checkout of
the repository that the preflight repo belongs to.

- Each loop's `step-02` sets `REVIEW_DIR` with
  `autodrive_worktree_guard.sh review-dir "$REPO"`, which prints the
  real path of that repository's main checkout (through
  `workflow_worktree_root.sh root`, the same anchor the resolver uses), and
  passes it to `autodrive_loop.sh` as `--review-dir`. `$REPO` is the repo the
  loop's preflight (`step-01`) reports (#1543): the pull request head's
  worktree when a pull request number was supplied, else `repo_path`. The
  review dir is never the step's working directory (`pwd`): that, like
  `$REPO` itself, may be the pull request worktree, because the preflight
  answers the head's worktree and because an operator may pass
  `-c repo_path=<pull request worktree>` from the main checkout or
  `-c repo_path=.` from inside the worktree. In each case `review-dir` answers
  the main checkout of the same repository and the loop preflight passes. A
  linked worktree is never the main checkout, so the review dir cannot be the
  tree under review.
- The guard unsets the repository-selecting variables (`GIT_DIR`,
  `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and the rest of the list
  `autodrive_pr_worktree.sh` unsets) before its first git call. An inherited
  `GIT_DIR`, which a git hook runs with (githooks(5)), would otherwise make
  `review-dir` answer for another repository while the resolver, which unsets
  them, answers for the preflight repo's.
- `review-dir`, the loop preflight and `round-guard` all accept a review dir
  only when it is the **main checkout of the repository under review**: the
  top of a work tree whose git dir is its common dir, and whose
  `--git-common-dir` resolves to the same real path as that of the repo it is
  checked against (`review-dir`: the preflight repo; the loop preflight:
  `--repo`; `round-guard`: `repo_path`, all in the same repository).
  Outside the worktree is not enough. A checkout of another repository would
  have the judging agent load that repository's `AGENTS.md`, and its
  `gh pr diff N`, which has no `-R`, would read that repository's pull request
  N. For a bare repository `workflow_worktree_root.sh root` answers the
  directory beside the bare git dir, which is no checkout, so `review-dir`
  stops there with a `STOP:` line: auto-drive needs a repository with a main
  checkout to start its judging agents in. Another linked worktree of the
  same repository is refused too; it carries its own branch's instruction
  files.
- `autodrive_loop.sh` hands it to every round as `review_dir`, after the
  caller's `--context` values, and runs `loop-health-evaluator` with
  `--working-dir` set to it and `loop_agent_working_dir` set to it.
  `loop_repo_path` stays the pull request worktree, where the evidence is
  measured.
- The merge-ready loop's `--before-round` step, `autodrive_crusty_rereview.sh`,
  gets the same checked review dir as `--review-dir` and passes it to the
  crusty loop it starts, so a re-review's judging agents start there too.
- `autodrive_loop.sh` also hands every round its own tools directory as
  `autodrive_launch_tools`, so the rounds run `autodrive_worktree_guard.sh`
  from the launch-side bundle, never from the tree they judge. Both
  `review_dir` and `autodrive_launch_tools` are set only by the loop: passed as
  `--context`, the loop refuses them before any round runs.
- Both round recipes run `step-00-review-dir-guard`
  (`autodrive_worktree_guard.sh round-guard`) before any agent or sub-recipe
  step; the merge round finds its other tools first (`step-00-tools-dir`). It
  stops the round when `review_dir` is empty, missing, or equal to or inside
  `repo_path` (symlinks resolved), or when `autodrive_launch_tools` holds no
  guard. Neither has a default, because `.` would resolve to the pull request
  worktree.
- The judging agents are full-tool agents, read-only only by their prompts,
  and #1528 was an agent staging a forced gitlink in the main checkout.
  `round-guard` therefore prints a stamp of the review dir's `HEAD`, branch
  and staged entries (`index-stamp`; it writes no object), which `step-00`
  keeps as `review_index`. `step-02b-review-dir-unchanged`, the step straight
  after each judging step, runs `index-check` and fails the round with a
  `STOP:` line naming the judging step when the stamp has changed.
- `step-04`, the agent that stages and commits, runs in the pull request's
  linked worktree, and its commit block refuses to run anywhere else. Both are
  instructions to a full-tool agent: one that improvises
  `git -C <main checkout> add -f ...` never reaches that block, and the
  artifact guard in it sees only `--repo .`, the worktree. The #1528 agent was
  this step.
- A failed round is not terminal on its own: it goes to the loop-health
  evaluator, which may continue. So `autodrive_loop.sh` stamps the review dir
  itself before each round (and before its `--before-round` step), and
  compares after the round, whatever the round's exit status, and again after
  `loop-health-evaluator`. Any change stops the loop with
  `"loop_result":"REVIEW_DIR_CHANGED"`; the next round never starts from it.
  That covers `step-04`, including a `step-04` that fails or times out after
  staging there, so the rounds carry no separate check after it (an earlier
  `step-04b` did, and was removed as a duplicate of this one). When a stamp
  cannot be taken (an index lock, a corrupt index, another git error) the loop
  stops with `"loop_result":"REVIEW_DIR_UNSTAMPED"` and claims no change:
  `index-check` exits 3 for that case and says "could not be stamped again
  ... no change is claimed", never "changed the index".
- A stamp records the main checkout as it is found. `round-guard` therefore
  refuses, before stamping, a review dir whose index holds a run worktree as
  a gitlink (one under `worktrees/`, or any registered worktree of the
  repository), staged or committed. That is the rule
  `amplihack hygiene artifact-guard` calls `run-worktree-gitlink`, and the
  #1528 state: a gitlink left there by an earlier run, or by a person, never
  becomes a round's baseline. The `STOP:` line names the path and how to drop
  it from the index alone, keeping the worktree and its files
  ([artifact guard](../artifact-guard.md)).
- A change is therefore caught by the round that made it, not by the next
  human commit in the main checkout. The check cannot tell who made the
  change: a person committing, staging or switching branch in the main
  checkout while `step-02`, `step-04` or the evaluator runs fails the round
  too, and the `STOP:` line says so. Run the round again; do not work in the
  main checkout while auto-drive runs.
- The judging prompts tell the agent where it is and to read the pull request
  as data — `gh pr diff`, `gh pr view`, `git -C <worktree> show HEAD:<path>` —
  never by `cd`-ing into the worktree or opening its files with file tools,
  which can load the branch's nested instruction files. Instruction files in
  the diff are code under review.

The loop refuses, with `"loop_result":"UNTRUSTED_REVIEW_DIR"` and before any
round runs, when the review dir is the tree under review or inside it — given
by `--review-dir`, defaulted from the loop's own working directory, or passed as
`--context review_dir=...` — or is not the main checkout of the pull request's
repository. The launch directory does not decide the review
dir in the loop recipes; the preflight repo (`$REPO` in `step-02`: the pull
request head's worktree when a pull request number was supplied, else
`repo_path`) does, through `review-dir "$REPO"`. A refusal
there therefore means the main checkout is itself the tree under review: keep
the pull request in a linked worktree. A direct call to `autodrive_loop.sh`
must pass `--review-dir` the main checkout.

`test-auto-drive-to-merge.sh` checks, against the tampered worktree, that the
round gets the launch checkout as `review_dir` and the evaluator starts there
(`1528-judging-agents-launch-side`), and covers each refusal
(`1528-review-dir-refused-*`, including another repository's checkout, a
directory that is no checkout and a sibling worktree). Every loop it runs works
on a linked worktree with its real main checkout as the review dir. It runs
both rounds' real `step-00` and `step-02b` bodies around a judging step that
stages a file in the review dir and requires the round to fail
(`1528-autodrive-*-round-review-dir-index`), and requires the loop to stop when
the evaluator does the same (`1528-loop-evaluator-review-dir-index`). Its end-to-end runs of both loops' `step-02`
(`1528-1570-*-loop-e2e`) set `repo_path` to the pull request's linked
worktree and check that the real preflight passes with the main checkout as
`review_dir`. `autodrive_pr_worktree_tests.rs` checks the same for `step-02`
with `repo_path` set to the worktree and to the main checkout. The same file
also checks that in both round recipes every agent step except `step-04` takes
`working_dir` from `review_dir`, that the evaluator's analyzer takes it from
`loop_agent_working_dir`, and it runs `step-00-review-dir-guard` against each
bad review dir. `review_dir_ignores_an_inherited_git_dir_and_the_preflight_refuses_another_repository`
runs `review-dir` with `GIT_DIR` pointing at another repository, and
`review_dir_stops_on_a_bare_repository_with_a_linked_worktree` runs it on a
bare repository's linked worktree.

The resolver sits next to `autodrive_loop.sh` in the launch-side bundle, never
in the pull request worktree. The loops always resolve by `--pr`. The merge
gate, `step-03-merge-gate`, keeps `--repo "${REPO_PATH:-.}"` and never sees the
pull request worktree.

Because the resolver runs inside each loop, it works on every entry path: the
full workflow, an `existing_branch=` run that skips phase 1, and a loop recipe
run on its own:

```bash
amplihack recipe run autodrive-crusty-loop \
  -c repo_path=/home/dev/src/shop-api \
  -c pr_number=1530
```

### `autodrive_pr_worktree.sh resolve`

```text
autodrive_pr_worktree.sh resolve --repo DIR (--pr N | --head BRANCH)
```

| Option | Meaning |
| --- | --- |
| `--repo DIR` | Required. Any checkout of the repository, main or linked. A relative path is resolved from the current directory, never through `CDPATH`. |
| `--pr N` | Read the head branch with `gh pr view N --json headRefName,isCrossRepository`, run inside `--repo`. `N` must be a positive number without a leading zero; anything else is a usage error and `gh` never runs. |
| `--head BRANCH` | Use `BRANCH` directly. `gh` is never called. |

Exactly one of `--pr` and `--head` is required. Each option may appear once,
and an option value that is empty or starts with `-` counts as missing.

First the resolver runs `git ls-remote --exit-code origin refs/heads/<branch>`.
If origin has the branch, `git fetch --no-tags origin` updates
`refs/remotes/origin/<branch>`, and every choice below is checked against that
tip. A resumed run could otherwise review and edit a stale tree for a whole
round, only to stop on a non-fast-forward push.

The resolver then picks the worktree in this order:

1. A linked worktree already registered on the branch, wherever it lives. It
   is used only if its `HEAD` contains `origin/<branch>`. The resolver never
   updates an existing worktree in place.
2. Otherwise, `<main>/worktrees/<branch>`, created with
   `git worktree add <path> <branch>` from the local branch. If the local
   branch is behind origin, it is first fast-forwarded with
   `git fetch origin refs/heads/<branch>:refs/heads/<branch>`. That refspec
   has no `+`, so git refuses anything other than a fast-forward. A local
   branch that is ahead of origin, holding unpushed work, is used as it is.
3. Otherwise, the same path created with
   `git worktree add --track -b <branch> <path> origin/<branch>`.

If origin reports that it has no such branch, a local branch is the only copy
and is used as it is.

| Exit | stdout | stderr |
| --- | --- | --- |
| `0` | Exactly one line: the worktree's absolute, canonical path | Nothing required |
| `1` (refusal) | Empty | One line ending in `(#1528)`. It never echoes the rejected branch name. |
| `2` (usage error) | Empty | The usage message |

It refuses, with exit `1`, when:

- the branch is checked out only in the **main checkout**, which is never a
  place to commit a round;
- the pull request comes from a **fork** or another repository, or
  `isCrossRepository` is missing or `null`. A fork's head branch name can match
  an unrelated branch on `origin`;
- `gh` cannot resolve the pull request;
- the match is **prunable** (its directory is gone). Run `git worktree prune`
  yourself; the resolver never prunes;
- more than one worktree matches;
- the target path already exists but is not a worktree registered on this
  branch of this repository, including a dangling symlink, or a component under
  `worktrees/` is a symlink or a file;
- a registered worktree at the target path belongs to another repository;
- the branch fails `git check-ref-format` or contains anything outside
  `A-Z a-z 0-9 . _ / -`;
- the worktree path contains `"`, `$`, `` ` ``, `\` or a control character.
  Round prompts embed it as `cd "{{repo_path}}"`;
- the matched worktree is **behind or has diverged from** `origin/<branch>`.
  Fast-forward it (`git -C <worktree> merge --ff-only origin/<branch>`) and
  resume;
- the local branch has **diverged** from `origin/<branch>`, so it cannot be
  fast-forwarded. Reconcile it yourself;
- origin cannot be asked whether it has the branch (no `origin` remote, or
  origin unreachable), so the tip cannot be checked for currency;
- the branch is neither local nor on origin.

Before its first `git` call the resolver unsets every repository-selecting
variable (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR` and the
rest of `amplihack_git::REPOSITORY_ENV_VARS`) and `CDPATH`, so an inherited
environment cannot point it at another repository.

The resolver never forces, rewinds, stages, commits, resets or prunes. Its
only writes are the fetch into the remote-tracking ref, a fast-forward-only
fetch into a local branch that is checked out nowhere, and `git worktree add`. The main
checkout's `HEAD` and index are unchanged by every call.

### The round commit block

`step-04-address-concerns` (crusty round) and `step-04-address-blockers` (merge
round) give their agent one fenced block that runs as one shell command. The
order is fixed: enter the worktree, prove it is the pull request's linked
worktree on its head branch, set the identity, guard, stage, guard again,
commit, push.

```bash
cd "{{repo_path}}" || { echo "STOP: cannot enter the PR worktree (#1528)" >&2; exit 1; }
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GH_REPO
bash "{{autodrive_launch_tools}}/autodrive_worktree_guard.sh" commit-dir {{pr_number}} || { echo "STOP: not the PR's linked worktree on its head branch; never stage or commit in the main checkout or another checkout (#1528)" >&2; exit 1; }
# … source git-identity.sh (issue #1394) …
amplihack_prepare_git_commit_identity || { echo "ERROR: git identity helper not found; stop rather than commit under a guessed identity" >&2; exit 2; }
amplihack hygiene artifact-guard --repo . --mode pre-commit || { echo "STOP: artifact guard refused before staging (#1528)" >&2; exit 1; }
git add -A
amplihack hygiene artifact-guard --repo . --mode pre-commit || { echo "STOP: artifact guard refused after staging; fix the cause, never bypass (#1528)" >&2; exit 1; }
git commit -m "address crusty review: <what changed>" || { echo "STOP: the commit failed; fix what the hook reports, never bypass it (#1528)" >&2; exit 1; }
git push || { echo "STOP: the push failed (#1528)" >&2; exit 1; }
```

| Line | Why |
| --- | --- |
| `unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GH_REPO` | An inherited `GIT_DIR` would make the main checkout the current repository, and `GIT_INDEX_FILE` would make `git add` and `git commit` use an index the guard never read. An inherited `GH_REPO` would make `gh pr view` ask a different repository than the `origin` the push goes to. |
| `commit-dir`: `--show-toplevel` equals `pwd -P` | Refuses a subdirectory of some other checkout. |
| `commit-dir`: git directory differs from the absolute common directory | True only for a linked worktree. Refuses the main checkout. Needs Git 2.31 or later. |
| `commit-dir`: current branch equals the pull request's head (`gh pr view <pr> --json headRefName`, empty never matches) | Refuses a linked worktree on some other branch. |
| Guard before `git add -A` | Keeps a stray runtime artifact from being staged at all. |
| Guard after `git add -A` | Sees exactly what the commit will contain, including any gitlink. See [Run worktrees in the index](../artifact-guard.md#run-worktrees-in-the-index). |

No check is silenced with `2>/dev/null` or `|| true`; each one stops the block.
`commit-dir` runs in the block's own shell, after the `unset`, and prints its
own `STOP: … (#1528)` line naming which check failed. The `STOP:` messages
above, and the resolver's `STOP:` line in `step-02`, are the exact text the
recipes print. Tests check only that each guard has the
`|| { … STOP … exit 1; }` shape, so keep this page and the recipes in step when
either changes.
The merge round's commit message is
`clear merge-ready blockers: <what changed>`. Everything else is the same in
both recipes.

The prompt prose around the block tells the agent that:

- the block is the only place it may stage or commit;
- it must never stage from another checkout, never stage an ignored path, and
  never stage anything under `worktrees/`;
- it must never force a staging or a commit, and must stop if the guard refuses
  after staging.

Started from the launch checkout, the block stops at the linked-worktree check
and leaves that checkout's `HEAD` and index byte-for-byte unchanged.

### Limitations

- **Fork pull requests are not driven.** The resolver refuses them.
- **Branch names outside `A-Z a-z 0-9 . _ / -`** are refused. Rename the
  branch. Generated branch names use only lower-case letters, digits, `-` and
  `/`.
- **`gh` must work inside the round.** `step-04` reads the head branch again; if
  `gh` fails, the block stops, and the loop-health evaluator sees a stalled
  round instead of a commit.
- **The `step-04` fix agent is the one agent that starts in the pull request
  worktree**, so it loads that branch's `AGENTS.md` and `.claude/` hooks, and
  its `git commit` runs that branch's `.pre-commit-config.yaml`. That is the
  same as any developer committing on the branch. It does not reach a verdict:
  the review, the merge-ready assessment and the loop-health analyzer start in
  the launch checkout and judge the fix-up commit as code.
- **The block is a prompt.** An agent can still deviate from it. The structural
  fix (rounds start in the pull request worktree) and the guard after staging,
  which refuses a run-worktree gitlink however it got there, cover that case.

## Files

| File | Kind | Role |
| --- | --- | --- |
| `amplifier-bundle/recipes/auto-drive-to-merge.yaml` | composer | Three phases, then a summary. |
| `amplifier-bundle/recipes/autodrive-build.yaml` | phase 1 | `default-workflow`, resume-aware. |
| `amplifier-bundle/recipes/autodrive-crusty-round.yaml` | round | Crusty review of the PR head, verdict, fixes, round record. |
| `amplifier-bundle/recipes/autodrive-crusty-loop.yaml` | phase 2 | Loop driver + phase bookkeeping. |
| `amplifier-bundle/recipes/autodrive-merge-evidence.yaml` | evidence | Base sync, suite commands, gadugi scenarios where required, CI wait. |
| `amplifier-bundle/recipes/autodrive-merge-round.yaml` | round | Merge-ready criteria from the skill's files, verdict, blocker fixes. |
| `amplifier-bundle/recipes/autodrive-merge-loop.yaml` | phase 3 | Loop driver + merge gate + bookkeeping. |
| `amplifier-bundle/recipes/loop-health-evaluator.yaml` | terminator | Agentic loop-health verdict. |
| `amplifier-bundle/tools/autodrive_loop.sh` | tool | The uncapped, agentically-terminated loop driver; writes `<loop>-records.tsv`; runs a `--before-round` step at its own depth. |
| `amplifier-bundle/tools/autodrive_last_round.sh` | tool | The highest round number a loop already has in the state dir, so a second or resumed loop labels its rounds after it. Writes nothing. |
| `amplifier-bundle/tools/autodrive_done_override.sh` | tool | What the loop does with a loop-health `DONE` over a non-clean round (#1597): exit `0` runs a re-review round; exit `1` is `STUCK` for a missing record or a repeat with no progress. Writes nothing. |
| `amplifier-bundle/tools/autodrive_round_tail.sh` | tool | The last 20000 bytes of a round log for the loop-health evaluator, without the partial first line a byte cut leaves, so no prefix-less agent fragment reaches the evidence collector, and with the runner summary's header kept when a long summary pushes it out of the window (#1510). Writes nothing. |
| `amplifier-bundle/tools/autodrive_crusty_rereview.sh` | tool | The merge-ready loop's `--before-round` step: sends code commits made after the clean crusty round back to the crusty loop. |
| `amplifier-bundle/tools/autodrive_pr_worktree.sh` | tool | Resolves the pull request's linked worktree for phases 2 and 3. Read-only apart from `git worktree add` and fast-forward-only fetches. Refuses a tip that is stale against origin. |
| `amplifier-bundle/tools/autodrive_worktree_guard.sh` | tool | The #1528 checks: the loop preflight (launch-side recipes, review dir is the main checkout of the PR's repository), each round's `step-00-review-dir-guard` (which also refuses a run-worktree gitlink in the review dir's index), `step-02b-review-dir-unchanged` (index stamp and check), the loop's stamp around each round and the evaluator, and the commit block's linked-worktree and head-branch check. Writes nothing. |
| `amplifier-bundle/tools/autodrive_merge_gate.sh` | tool | Evidence gate and the fixed merge argv; runs as `/bin/bash -p`, the only process the merge guard trusts. |
| `amplifier-bundle/tools/gh-merge-guard/gh` | tool | The `gh` shim that refuses a merge outside the gate ([#1551](#merges-outside-the-gate-are-refused-1551)). |
| `crates/amplihack-cli/src/commands/recipe/run/merge_guard.rs` | launcher | Decides when the shim goes first on `PATH` and exports its `BASH_ENV` file; fails closed when either is missing, the shim is not executable, or either is writable by group or others. |
| `amplifier-bundle/tools/gh-merge-guard/bash-env` | tool | Sourced through `BASH_ENV`; puts the shim back first on `PATH` after a login shell's profile. |
| `crates/amplihack-hooks/src/pre_tool_use/merge_guard.rs` | hook | Refuses the prohibited forms `gh pr merge`, `gh api` on a merge endpoint and a run of the gate, by program word only. |
| `amplifier-bundle/tools/autodrive_merge_ready_files.sh` | tool | Finds the merge-ready `SKILL.md` and template. |
| `amplifier-bundle/tools/autodrive_platform_facts.sh` | tool | Platform facts for the merge round, criterion 6 included; the review-thread count for the merge gate too. |
| `amplifier-bundle/tools/autodrive_qa_evidence.sh` | tool | Criterion 1: the suite commands and, where `autodrive_gadugi_required` says so, the gadugi scenarios, for `autodrive-merge-evidence.yaml` step-02. |
| `amplifier-bundle/tools/autodrive_round_evidence.sh` | tool | The merge round's deterministic step bodies, and the crusty range the re-review reads: crusty range and evidence, the qa evidence hash, the measured downgrade, the findings and the round record. |
| `amplifier-bundle/tools/autodrive_state.sh` | tool | Resumable local state, `autodrive_crusty_final`, `autodrive_private`; platform truth for merged-ness. |
| `amplifier-bundle/tools/autodrive_trust.sh` | tool | Range check and qa evidence chain. |
| `amplifier-bundle/tools/autodrive_pr_head.sh` | tool | Read-only PR head and worktree resolver. |
| `amplifier-bundle/skills/auto-drive-to-merge/SKILL.md` | skill | Invocable entry point. |
| `amplifier-bundle/skills/merge-ready/SKILL.md` | skill | The criteria the merge round reads. |

Every recipe and tool here stays inside the 400-line brick budget.

## Tests

| Test | Location |
| --- | --- |
| Executable contract test: loop, verdicts, forbidden-flag scan, merge-gate refusals, qa evidence against stub `cargo` and `gadugi-test`, crusty records and range; a step output read through `RECIPE_VAR_` only, the [#1511 repro](#reading-step-outputs) (`1511-repro`), the `state_dir` refusals and their silence on a safe value, the static `RECIPE_VAR_` check, the [inherited-variable rows](#the-inherited-variable-rows) (`1561-stale-*`, `1561-empty-tools-dir-fails-closed`, `1561-no-merge-*`, `MERGEREADY-no-round-tool-inherited`), and every row of the [loop-health line table](#reading-the-loop-health-line) | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |
| PR head and worktree resolution, refusal, relative `repo_path`, run report, and two runs of the composed recipe through `recipe-runner-rs` (a fresh build whose stand-in default-workflow builds in its own worktree, as the real one does, where both loops must get that worktree and not the launch checkout; skipped build and crusty phases followed by a merge), and the `pr-worktree-stale` refusal (#1543). It needs `recipe-runner-rs` on `PATH` or in `RECIPE_RUNNER_RS_PATH`, and stops with a harness error without it | `amplifier-bundle/recipes/tests/test-issue-1543-pr-head-worktree.sh` |
| QA command selection (#1516): `AUTODRIVE_QA_DIR` with `AUTODRIVE_QA_COMMAND`, root Cargo, nested and nested-workspace Cargo, pruned `target`/`worktrees`/`node_modules`/`.git` crates, ambiguous and depth-4 crates, mixed Node/Python roots, the jamestown shape (root `pytest` beside a nested crate), `package.json` beside a nested crate, a root `requirements.txt`, `go.mod` or other unhandled project file beside a nested crate (BLOCKED, never `cargo test` in the crate), the `./<word>` installed check, an `AUTODRIVE_QA_DIR` that does not exist, parity scenarios under the detected workspace directory (including `AUTODRIVE_QA_COMMAND` with `AUTODRIVE_QA_DIR=sub` in a nested-Cargo repository), the space-separated ambiguous candidate list, symlinked and unsafely named directories, a `cargo` in the workspace that is off `PATH` and never runs, an in-repository `cargo` on `PATH` that runs (the check is a `PATH` lookup, not a sandbox), a `.venv/bin/pytest` on `PATH` that runs whether detected or set with `AUTODRIVE_QA_COMMAND=pytest`, and a crate in a directory named `-` or an exported `CDPATH` that never moves the suite outside the checkout. Stub `cargo`, `pytest`, `npm` and `gadugi-test` record their directory and arguments | `amplifier-bundle/recipes/tests/test-issue-1516-qa-command-selection.sh` |
| Structural and wiring guards, brick budget, skill-invocation guard, state-directory sentence, `tests/gadugi/recipe-step-command.sh` compared with serde_yaml over every bundled recipe step, the PR-head resolver read-only guard, `autodrive_step_outputs_are_read_from_recipe_var_only` (the four [static checks](#the-static-check), with the read counts taken from this page), `autodrive_round_with_inherited_stale_tools_dir_uses_its_own` (#1607), and the contract shell suite run once per [`Inherited` mode](#the-inherited-variable-rows): scrubbed, conflicting and same | `tests/integration/auto_drive_to_merge_test.rs` |
| [The bundle-wide guard](#the-bundle-wide-guard) (#1561): no recipe in `amplifier-bundle/recipes/*.yaml` and no tool in `amplifier-bundle/tools/*.sh` lets a bare upper-case name win over its `RECIPE_VAR_` name, with minimum file counts, self-tests for each detector, the repository-wide grep, no `AUTODRIVE_TOOLS_DIR` read, declared `RECIPE_VAR_` keys, the `NO_MERGE`-only allowlist with `no_merge_override_can_only_tighten`, and the test-only opt-out | `crates/amplihack-cli/tests/issue_1561_step_outputs_prefer_recipe_var.rs` |
| Where phases 2 and 3 run (issue #1528): resolver behaviour and refusals (including a local branch or worktree behind, ahead of or diverged from origin), loop `step-02` resolution and forwarding, round `step-04` commit blocks, a forced-add reproduction the guard refuses, and a commit from the pull request worktree that leaves the launch checkout's index clean | `tests/integration/autodrive_pr_worktree_tests.rs` |
| [Merges outside the gate](#merges-outside-the-gate-are-refused-1551) (#1551): every merge form refused with exit `3` and the PR left open (T1); forged gates, a copied gate and the real gate run without `-p` refused (T2, T3); the real gate run as `/bin/bash -p` still merges (T3); pass-through of non-merge calls (T4); exit `127` cases (T5); `export -f gh`, `BASH_ENV`, a `PPID` mismatch, a non-digit depth and every non-canonical endpoint form, mixed-case `Pulls` and `Merge` included, refused (SEC); the step-01b directive and its fail-closed branches, the phase-1 `MERGED` check and the unreadable-state exit (T6); static checks (S). CI runs it in a Linux job. | `amplifier-bundle/recipes/tests/test-issue-1551-no-merge-outside-gate.sh` |
| When the launcher applies the guard, idempotent `PATH` with the guard first, `no_merge` alone not applying it, the inherited marker, the fail-closed error for an unset, empty or relative `AMPLIHACK_HOME` and a missing or non-executable shim, and (Linux only) a phase-1 `autodrive-*` bash step whose `gh pr merge 1` gets exit `3` and `REFUSED` while the stub `gh` logs no merge, a login shell whose profile prepends another `gh` still resolving `gh` to the shim, the shipped `bash-env` sourcing a user `BASH_ENV` first; off Linux, a companion test shows the merge is refused | `crates/amplihack-cli/tests/issue_1551_merge_guard.rs` |
| `guard_applies` truth table (`0` and `false` still apply it) and `locate_guard_dir` errors, a group- or other-writable shim or directory included | `crates/amplihack-cli/src/commands/recipe/run/tests_merge_guard.rs` |
| The real gate's `MERGED` path with the guard first on `PATH`, the phase-2 `MERGED` check, and the directive reaching the agent prompts (§5i-1551) | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |
| The merge-ready skill stays platform-neutral | `tests/integration/merge_ready_platform_contract_test.rs` |
| gadugi-test scenarios for this change: criterion 6 approval measurement (#1518), the merge-ready file lookup and the repo-type gadugi rule (#1517), and the crusty range and re-review at the loop's depth, with the real recursion guard, leaving phase 2's crusty records and manifest rows unchanged. This repository is a Rust CLI repository, so auto-drive runs them only with `AUTODRIVE_QA_SCENARIO_DIR=tests/gadugi/scenarios`. | `tests/gadugi/scenarios/issue-1518-*.yaml`, `tests/gadugi/scenarios/issue-1517-*.yaml` |

```bash
cargo test -p amplihack --test integration auto_drive_to_merge::
cargo test -p amplihack --test integration merge_ready_platform_contract::
cargo test -p amplihack --test integration autodrive_pr_worktree::
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
bash amplifier-bundle/recipes/tests/test-issue-1543-pr-head-worktree.sh
bash amplifier-bundle/recipes/tests/test-issue-1551-no-merge-outside-gate.sh
cargo test -p amplihack-cli --test cli_integration issue_1551
cargo test -p amplihack-cli --test cli_integration issue_1561
bash amplifier-bundle/recipes/tests/test-issue-1516-qa-command-selection.sh
gadugi-test validate -d tests/gadugi/scenarios
bash tests/gadugi/run-autodrive-crusty-rereview-scenario.sh   # and the other run-autodrive-*-scenario.sh
```

### Which `amplihack` binary the shell test uses

The shell test runs `amplihack` through a stub that scripts `recipe run` per
scenario and passes every `orch` call to a real binary. It picks that binary
from the same candidates, in the same order, as the
[loop-health evaluator test](loop-health-evaluator.md#which-amplihack-binary-the-shell-test-uses),
and prints it first as `amplihack binary: <path>`. Only the probe differs: a
candidate qualifies when it supports `orch helper extract-field` and
`extract-json --require-field`. If none does, the test prints a
`HARNESS-ERROR` and exits `2`.

The test runs whatever binary it picks, so point `CARGO_TARGET_DIR` only at a
directory that no one but you can write. Under `cargo test`, candidates in
`target/` and `$CARGO_TARGET_DIR` are tried before the fresh build that the
wrapper puts first on `PATH`. An older build there that passes the probe is
tested instead, and the `amplihack binary:` line shows which one ran.

## Dependency on PR #1347

Both loops invoke the shared `loop-health-evaluator` recipe, taken from the
`recipes/` directory beside `autodrive_loop.sh`. That brick,
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
- [Artifact Guard](../artifact-guard.md#run-worktrees-in-the-index) — the
  `run-worktree-gitlink` rule both round commit blocks rely on.
