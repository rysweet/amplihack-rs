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
- [How the merge round applies merge-ready](#how-the-merge-round-applies-merge-ready)
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
| 3. Merge-ready loop | `autodrive-merge-loop.yaml` over `autodrive-merge-round.yaml` | Syncs the base, runs the repository tests and the `gadugi-test` scenarios, waits for CI, applies the `merge-ready` criteria read from the skill's files, clears blockers, repeats, then merges behind the gate. |

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
than `GREEN`, a merge conflict, unresolved review threads, or a `crusty_status`
other than `DONE_CLEAN`. A missing `crusty_status` counts as not
`DONE_CLEAN`. The downgrade is recorded in `downgrade_reason` and adds a
`measured-evidence-disagrees` finding so the loop evaluator sees a round that
did not converge.

`qa_status` is `PASS` only when the repository's own test command **and** the
`gadugi-test` validate and run both passed, so a gadugi failure downgrades the
verdict through `qa_status`. See [Criterion 1](#criterion-1-qa-team-scenarios-run-with-gadugi-test)
and [Criterion 3](#criterion-3-the-crusty-loop-ended-done-and-clean).

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

### The round reads the skill's files

The [`merge-ready`](../../amplifier-bundle/skills/merge-ready/SKILL.md) skill
sets `disable-model-invocation: true` in its frontmatter: it is the
`/merge-ready` command a person runs, and Claude Code refuses the skill when an
agent calls it. The flag stays set. Step `step-02-merge-ready-assessment` of
`autodrive-merge-round.yaml` therefore never calls the skill. Its prompt tells
the agent to read `SKILL.md` and `pr-description-template.md` as files and
apply the criteria they state.

The agent checks these roots in order and uses the first one that has
`amplifier-bundle/skills/merge-ready/SKILL.md`:

| Order | Root |
| --- | --- |
| 1 | `$AMPLIHACK_HOME` |
| 2 | `$REPO_PATH` |
| 3 | `git rev-parse --show-toplevel` |
| 4 | `~/.copilot` |
| 5 | `~/.amplihack` |

`~/.copilot` comes before `~/.amplihack`, the same order the merge loop uses to
find `autodrive_merge_gate.sh`. The template is read from the same directory. The agent cites the resolved
`SKILL.md` path in its evidence, so a PR that edits its own criteria file
shows up in the round record. When no root has the file, the verdict is
`NOT_MERGE_READY` with blocker `merge-ready-skill-files-not-found`, which lists
every path checked; the agent does not invent criteria. When only the template
is missing, the blocker is `merge-ready-template-not-found`.

All merge-ready criteria apply as the skill states them, except criteria 1 and
3, which auto-drive measures as described below. The skill's own
`## Running under auto-drive` section says the same thing for a person reading
it, and states that a manual `/merge-ready` still requires the full criteria,
including a separate `quality-audit` of at least 3 cycles.

#### A guard test keeps recipes from calling refusing skills

`no_recipe_invokes_a_skill_that_refuses_model_invocation` in
`tests/integration/auto_drive_to_merge_test.rs` walks
`amplifier-bundle/skills/**/SKILL.md`, collects every skill whose frontmatter
sets `disable-model-invocation: true` (any case, quoted or not; the name comes
from `name:`, falling back to the directory name), then scans the full text of
every `*.yaml` and `*.yml` file under `amplifier-bundle/recipes/` for a call
to one of those skills. Comments count.

The match accepts double or single quotes around the name and whitespace
around `(`, `=` and `)`. These all match for `merge-ready`:

```text
Skill(skill="merge-ready")
Skill(skill='merge-ready')
Skill( skill = "merge-ready" )
```

A call that passes the name some other way, for example through a variable,
is not caught. A match fails the build and prints the matched text as written:

```text
amplifier-bundle/recipes/autodrive-merge-round.yaml: step step-02-merge-ready-assessment: Skill(skill="merge-ready") but amplifier-bundle/skills/merge-ready/SKILL.md sets disable-model-invocation: true
```

The step id is the nearest earlier `- id:` line. The walk skips symlinks, and
an unreadable or non-UTF-8 file fails the test and names the path rather than
being skipped. The companion test
`the_invocation_guard_flags_the_old_merge_round_prompt` runs the same detector
over the prompt line this fix replaced, so the guard is known to catch the
original defect.

### Criterion 1: qa-team scenarios run with gadugi-test

Step `step-02-qa-team-scenarios` of `autodrive-merge-evidence.yaml` runs two
checks and records both. Neither can stand in for the other.

**The repository's own test command**, chosen by repository type:

| Repo type | Detected by | Test command |
| --- | --- | --- |
| `rust-cli` | `Cargo.toml` | `cargo test --workspace --locked --no-fail-fast` |
| `python` | `pyproject.toml` or `setup.py` | `pytest` |
| `node` | `package.json` | `npm test` |
| `unknown` | none of the above | none; `qa_status` is `BLOCKED` |

Every repository type, `rust-cli` included, must also pass the gadugi
scenarios. The step no longer reads `tests/parity/scenarios`.

**The gadugi scenarios**, which run even when the repository test failed, so
the evidence lists every cause at once. Before any gadugi check, the step
resolves the [scenario directory](#the-scenario-directory) and counts its
scenario files. `gadugi_scenario_dir`, `gadugi_scenario_count` and
`qa_scenarios` are therefore recorded on every run, including when
`gadugi_status` is `NOT_INSTALLED`. Step-04 and the merge gate both depend on
them.

The checks then run in this order and stop at the first failure:

1. `gadugi-test` must be on `PATH`.
2. The scenario count must be at least 1.
3. `gadugi-test validate -d "$DIR"` must exit 0.
4. `gadugi-test run -d "$DIR"` must exit 0.

The count check comes before `run` because `gadugi-test run` prints
`No scenarios found to execute` and exits 0 on an empty directory.

`$DIR` is always quoted and absolute. No timeout wrapper is added and no
`--timeout` flag is passed, so `gadugi-test run` uses its own default of
300000 ms (`gadugi-test run --help`, `@gadugi/agentic-test` 1.0.1). It applies
that value as the default time limit for each command and execution, not as a
cap on the whole run.

| `gadugi_status` | Meaning |
| --- | --- |
| `PASS` | validate and run both exited 0 |
| `NOT_INSTALLED` | `gadugi-test` is not on `PATH` |
| `NO_SCENARIOS` | the directory has no `*.yaml` or `*.yml` file at its top level |
| `VALIDATE_FAILED` | `gadugi-test validate` exited non-zero |
| `RUN_FAILED` | `gadugi-test run` exited non-zero |

`qa_status` combines the two:

| `qa_status` | When |
| --- | --- |
| `PASS` | the repository test exited 0 **and** `gadugi_status` is `PASS` |
| `FAIL` | otherwise, when something ran and failed: the repository test, `NO_SCENARIOS`, `VALIDATE_FAILED`, or `RUN_FAILED` |
| `BLOCKED` | otherwise: a binary is missing or the repository type is unknown |

`NO_SCENARIOS` is `FAIL` rather than `BLOCKED` because the merge round can fix
it by writing scenarios.

`qa_summary` starts with one fixed phrase per cause, joined by `; `, followed
by the tail of the repository test log. Only the log tail is cut to the length
limit; the cause phrases always survive.

| Cause | Phrase |
| --- | --- |
| repository test exited non-zero | `repository test failure` |
| no scenario files | `no scenarios in <dir>` |
| `gadugi-test` missing | `gadugi-test not installed` |
| validate exited non-zero | `gadugi-test validation failure` |
| run exited non-zero | `gadugi-test run failure` |

#### The scenario directory

The evidence step uses the first match:

1. `AUTODRIVE_QA_SCENARIO_DIR`, when set and not empty. A relative path is
   resolved from the repository root. If it names a directory that does not
   exist, the result is `NO_SCENARIOS`; there is no fallback to the defaults.
2. `tests/agentic`, if it exists.
3. `scenarios`, if it exists.
4. Otherwise `tests/agentic` is recorded with a count of 0. This is the
   directory the merge round writes new scenarios into.

Only files directly in the directory count, found with `find -maxdepth 1
-type f` and without following symlinks. `gadugi-test validate -d` does not
look in subdirectories, so a scenario that exists only in a subdirectory
counts as 0 and gives `NO_SCENARIOS`.

`AUTODRIVE_QA_SCENARIO_DIR` is read from the environment only. No recipe
declares it as a context key: the recipe runner exposes context keys as
environment variables, so a context key with an empty default would hide the
value the user exported. Set it before starting the run:

```bash
AUTODRIVE_QA_SCENARIO_DIR=tests/gadugi/scenarios \
  amplihack recipe run auto-drive-to-merge -c pr_number=1234 -c repo_path=.
```

If the recipe runner drops the variable (for example, when it trims an
oversized environment), the step falls back to the default lookup. The
`gadugi_scenario_dir` field in the evidence always shows the directory that was
actually used.

amplihack-rs itself keeps its gadugi scenarios in `tests/gadugi/scenarios`,
which is not in the default lookup. Its own auto-drive runs need the variable
above, or they report `NO_SCENARIOS` until the merge round writes scenarios to
`tests/agentic`.

#### The qa evidence record

The record keeps its 8 earlier fields and adds 5. Every value is a string. An
exit code is `""` when its command did not run. `head_sha` is read before any
check runs. Free text (`qa_summary`, `qa_scenarios`, `gadugi_scenario_dir`) has
quotes, backslashes and control bytes stripped before it is cut, so hostile
test output still yields valid JSON. `gadugi_scenario_dir` is relative to the
repository root when it is inside it.

A rust-cli repository with no scenario files, in the default directory:

```json
{"qa_status":"FAIL","qa_repo_type":"rust-cli","qa_command":"cargo test --workspace --locked --no-fail-fast","qa_scenarios":"","qa_exit_code":"0","qa_summary":"no scenarios in tests/agentic; test result: ok. 41 passed; 0 failed","qa_round":"round-1","head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","gadugi_status":"NO_SCENARIOS","gadugi_validate_exit_code":"","gadugi_run_exit_code":"","gadugi_scenario_count":"0","gadugi_scenario_dir":"tests/agentic"}
```

amplihack-rs run with `AUTODRIVE_QA_SCENARIO_DIR=tests/gadugi/scenarios`, all
checks passing:

```json
{"qa_status":"PASS","qa_repo_type":"rust-cli","qa_command":"cargo test --workspace --locked --no-fail-fast","qa_scenarios":"tests/gadugi/scenarios/issue-815-804-local-tracking-extract.yaml tests/gadugi/scenarios/issue-820-merge-validations-mixed-output.yaml tests/gadugi/scenarios/pr-ownership-lease.yaml","qa_exit_code":"0","qa_summary":"test result: ok. 41 passed; 0 failed","qa_round":"round-2","head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","gadugi_status":"PASS","gadugi_validate_exit_code":"0","gadugi_run_exit_code":"0","gadugi_scenario_count":"3","gadugi_scenario_dir":"tests/gadugi/scenarios"}
```

| Field | Values |
| --- | --- |
| `qa_scenarios` | the scenario files counted in `gadugi_scenario_dir`, as paths relative to the repository root, sorted and separated by single spaces; `""` when the count is 0. Recorded even when `gadugi_status` is `NOT_INSTALLED`. |
| `gadugi_status` | `PASS`, `NOT_INSTALLED`, `NO_SCENARIOS`, `VALIDATE_FAILED`, `RUN_FAILED` |
| `gadugi_validate_exit_code` | exit code of `gadugi-test validate`, or `""` |
| `gadugi_run_exit_code` | exit code of `gadugi-test run`, or `""` |
| `gadugi_scenario_count` | number of top-level `*.yaml` and `*.yml` files; always recorded |
| `gadugi_scenario_dir` | the directory used; always recorded |

#### When scenarios are missing

Step `step-04-address-blockers` of the merge round handles qa evidence whose `gadugi_status` is
`NO_SCENARIOS`, or whose scenarios do not cover the changed behaviour:

1. It uses the `qa-team` skill to write top-level `*.yaml` scenarios in
   `gadugi_scenario_dir`.
2. It runs `gadugi-test validate -d` and then `gadugi-test run -d` on that
   directory.
3. It commits through the same identity helper and artifact guard as every
   other round commit.

`VALIDATE_FAILED` and `RUN_FAILED` are fixed in the scenarios or in the code. A
failing scenario is never deleted to reach `PASS`. If `gadugi_scenario_dir` is
absolute or outside the repository, the step writes nothing and reports blocker
`gadugi-scenario-dir-outside-repo`.

### Criterion 3: the crusty loop ended DONE and CLEAN

Criterion 3 of merge-ready asks for a `quality-audit` of at least 3 SEEK,
VALIDATE, FIX cycles ending clean. Under auto-drive it is met instead by phase
2 of the same run: the `crusty-old-engineer` loop, an iterative review-and-fix
loop, must have ended `DONE` with a final `CLEAN` verdict. **No minimum round
count applies.** The crusty loop stops at its first `CLEAN` round, so a minimum
would block forever any PR that was clean in round 1 or 2.

`autodrive-merge-loop.yaml` passes its state directory to every round with
`--context "autodrive_state_dir=${DIR}"`. Step `step-01b-crusty-evidence` of
the merge round reads two things from it:

- the `crusty-loop` marker in `phases.tsv`, through `autodrive_phase_done`
- `crusty_verdict` in `crusty-latest.json`

It emits `crusty_evidence`, where every field comes from a fixed set:

```json
{"crusty_status":"DONE_CLEAN","crusty_phase_done":"true","crusty_verdict":"CLEAN"}
```

| Field | Values |
| --- | --- |
| `crusty_status` | `DONE_CLEAN` (marker present and verdict `CLEAN`), `ABSENT` (no marker), `NOT_CLEAN` (marker present, verdict not `CLEAN`) |
| `crusty_phase_done` | `true`, `false` |
| `crusty_verdict` | `CLEAN`, `CONCERNS`, `MISSING`, `OTHER` |

Any verdict text outside `CLEAN`, `CONCERNS` and `MISSING` becomes `OTHER`, so
an agent-written crusty record cannot inject text into the next prompt or
break the JSON.

The assessment prompt treats `crusty_evidence` as criterion 3. When
`crusty_status` is not `DONE_CLEAN`, the blocker is
`quality-audit-convergence-crusty-not-done-clean`, and
`step-03-extract-merge-ready-verdict` downgrades any `MERGE_READY` verdict.

The crusty evidence is read from the state directory rather than from the
composer's `crusty_loop_result`. On a resumed run the crusty loop is skipped
and that result is empty; the state directory persists. A resumed run therefore
accepts the crusty marker recorded by the earlier run. The evidence is not
bound to the head SHA, because merge-round fixes are expected to move the head.

Agent steps never see the state directory path. The step-02 and step-04
prompts do not contain it, and step-04 forbids creating, editing or deleting `phases.tsv`
or any `*-latest.json`. The guard test
`merge_round_agent_prompts_never_touch_crusty_state` enforces both.

A merge loop run on its own, with no earlier crusty loop in the same state
directory, gets `crusty_status: ABSENT`. Its rounds never reach `MERGE_READY`
and the loop ends `STUCK`. That is intended: criterion 3 has not been met.

### Accepted residual risks

- A trivial scenario that always passes satisfies `gadugi_status: PASS`.
  Review the scenarios like any other test.
- A process running as the same user can write the state files the gate reads.
  The gate rejects state that other users could have written, not state the
  user's own processes wrote.
- The evidence step runs the PR's code, as `cargo test` already did.

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
| gadugi scenarios | same evidence file | `gadugi_status` other than `PASS`, or `gadugi_scenario_count` missing or not a positive integer |
| Crusty loop | `phases.tsv` and `crusty-latest.json` in `--state-dir` | no `--state-dir` or an empty one; the directory not owned by the current user; the directory or either file with the group-write bit or the world-write bit set (either bit alone blocks); either file a symlink or not a regular file; `autodrive_state.sh` missing beside the gate; no `crusty-loop` marker; `crusty_verdict` other than `CLEAN` |
| merge-ready verdict | round record from this run | not `MERGE_READY`, or captured against a different head SHA |

The review-thread query pages. `reviewThreads(first:100)` with no `pageInfo`
follow-up silently truncates: a PR with 101 threads whose only unresolved one
is the last would report zero unresolved and pass the gate. Both readers —
`autodrive_merge_gate.sh` and `autodrive-merge-round.yaml` — use
`gh api graphql --paginate` with `pageInfo { hasNextPage endCursor }` and sum
the per-page counts; a page that does not come back as a number makes the whole
criterion unreadable, which is a blocker.

The gadugi and crusty rows only add checks. No earlier row was changed, so
evidence written before these fields existed now blocks.

The crusty row reads state the run wrote locally, so it accepts that state only
from a directory private to the current user. When `--state-dir` is missing or
`""`, the gate still falls back to `${TMPDIR:-/tmp}` for writing its evidence
bundle, but the crusty row treats it as no state directory and blocks. It never
reads crusty state from that fallback, so it cannot be pointed at files someone
else planted in a shared temporary directory.

The privacy check fails when the directory is not owned by the current user,
or when the directory, `phases.tsv` or `crusty-latest.json` has the
group-write bit or the world-write bit set. Either bit alone blocks: `0770`,
`0702` and `0777` all fail. A failure is reported as
`not private to this user`. The gate sources `autodrive_state.sh` from its own
directory only, never from the merge-ready search roots.

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

Local state is a cache, never a claim, with one exception. The merge gate
re-verifies every criterion regardless of what any state file says, and
`UNKNOWN` platform state is treated as a failure rather than as "not merged".
The exception is the `crusty-loop` marker plus `crusty-latest.json`: nothing on
the platform records crusty's judgement, so the gate reads them as criterion-3
evidence (see [Criterion 3](#criterion-3-the-crusty-loop-ended-done-and-clean)).
Only `autodrive-crusty-loop.yaml` and `autodrive_loop.sh` write them, and the
gate accepts them only from a directory private to the current user.

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
| `amplifier-bundle/recipes/autodrive-merge-evidence.yaml` | evidence | Base sync, repository tests plus `gadugi-test` validate and run, CI wait. |
| `amplifier-bundle/recipes/autodrive-merge-round.yaml` | round | Crusty evidence, merge-ready criteria read from the skill's files, verdict, blocker fixes. |
| `amplifier-bundle/recipes/autodrive-merge-loop.yaml` | phase 3 | Loop driver (passes the state dir to rounds) + merge gate + bookkeeping. |
| `amplifier-bundle/tools/autodrive_loop.sh` | tool | The uncapped, agentically-terminated loop driver. |
| `amplifier-bundle/tools/autodrive_merge_gate.sh` | tool | Evidence gate, including the gadugi and crusty checks, and the fixed merge argv. |
| `amplifier-bundle/tools/autodrive_state.sh` | tool | Resumable local state, the crusty-loop marker, platform truth for merged-ness. |
| `amplifier-bundle/skills/auto-drive-to-merge/SKILL.md` | skill | Invocable entry point. |
| `amplifier-bundle/skills/merge-ready/SKILL.md` | skill | Criteria the merge round reads as a file; its `Running under auto-drive` section. |

Every recipe file stays inside the 400-line brick budget.

## Tests

| Test | Location |
| --- | --- |
| Executable contract test: STUCK path, malformed-verdict path, forbidden-flag guard, merge-gate refusals including the gadugi and crusty blocks (with separate group-writable `0770` and world-writable `0777` state-directory cases), the qa evidence step against stub `cargo` and `gadugi-test`, the `step-01b-crusty-evidence` cases, the crusty downgrade, and the absence of `Skill(skill="merge-ready")` | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |
| Structural + wiring, including the guard against recipes calling skills that refuse agents | `tests/integration/auto_drive_to_merge_test.rs` |
| The merge-ready skill stays platform-neutral | `tests/integration/merge_ready_platform_contract_test.rs` |

```bash
cargo test -p amplihack --test auto_drive_to_merge
cargo test -p amplihack --test merge_ready_platform_contract
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
```

The qa evidence cases cover: everything passing, an empty scenario directory
(`NO_SCENARIOS`), `gadugi-test validate` exiting 1 (`VALIDATE_FAILED`),
`gadugi-test run` exiting 1 (`RUN_FAILED`), `gadugi-test` missing
(`qa_status: BLOCKED`), `cargo test` exiting 1 (`FAIL`), the
`AUTODRIVE_QA_SCENARIO_DIR` override, a scenario only in a subdirectory
(counted as 0), and hostile output that must still parse as JSON. Each case
asserts the new fields and `head_sha`.

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
