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

`qa_status` is `PASS` only when every repository suite command **and**
`gadugi-test validate` and every per-scenario `gadugi-test run` passed, so a
gadugi failure downgrades the verdict through `qa_status`. See [Criterion 1](#criterion-1-qa-team-scenarios-run-with-gadugi-test)
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
`/merge-ready` command a person runs, with an `argument-hint`, and Claude Code
refuses the skill when an agent calls it. The flag stays set. No auto-drive
recipe calls the skill. The merge round reads its files instead.

Step `step-00-merge-ready-files` of `autodrive-merge-round.yaml` is a bash step
that runs before the merge evidence. It runs
`amplifier-bundle/tools/autodrive_merge_ready_files.sh`, found through the same
tool lookup the other auto-drive steps use. The script checks these
directories in order, skips any whose variable is empty, and uses the first
one in which `SKILL.md` is a regular file:

| Order | Directory |
| --- | --- |
| 1 | `$AMPLIHACK_HOME/amplifier-bundle/skills/merge-ready` |
| 2 | `$REPO_PATH/amplifier-bundle/skills/merge-ready` |
| 3 | `<git toplevel of $REPO_PATH>/amplifier-bundle/skills/merge-ready` |
| 4 | `~/.copilot/skills/merge-ready` |
| 5 | `~/.amplihack/amplifier-bundle/skills/merge-ready` |

The order is the one the other auto-drive steps use to find bundle files, so
the criteria come from the same install as the recipes that read them.
`~/.copilot` holds skills directly under `skills/`, which is where `amplihack
install` stages them for Copilot CLI.

The two files are resolved together. The template must be in the directory
that supplied `SKILL.md`; the script never takes one file from one install and
the other file from another.

| Situation | Result |
| --- | --- |
| A directory has both files | stdout is one line of JSON; the step succeeds |
| The first directory with `SKILL.md` has no `pr-description-template.md` | `ERROR: merge-ready-template-not-found: <path>` on stderr, exit 1. The next directory is not checked. |
| No directory has `SKILL.md` | `ERROR: merge-ready-skill-files-not-found: searched <every path checked>` on stderr, exit 1 |
| The resolver script itself is not found | `ERROR: merge-ready-skill-files-not-found: resolver autodrive_merge_ready_files.sh not found (searched ...)`, exit 1 |
| `REPO_PATH` names a directory the step cannot enter | `ERROR: cannot cd to REPO_PATH` on stderr, exit 1. The step never resolves from whatever directory the recipe happens to run in. Step `step-01b-crusty-evidence` and the qa evidence step fail the same way. |

A failure fails the recipe step, so the round fails and its log carries the
named error. It is not turned into a `NOT_MERGE_READY` blocker: a missing
install is not something a blocker-clearing agent can fix, and treating it as
one is what made every round of issue #1517 return the same verdict until the
loop went `STUCK`, with nothing in the log saying why.

A failed round writes no round record, so `autodrive_loop.sh` reads it as not
clean and adds no manifest row. The loop-health evaluator then gets the
round's log, with the named error, and its non-zero exit code. The same error
in round after round is no progress, so the loop still ends `STUCK`. The
change is that the log names the cause, and no blocker-fix round is spent on
it. A failed crusty `step-06-write-round-record`
(`crusty-head-sha-unavailable`) ends the same way.

On success the output looks like this:

```json
{"skill_dir":"/home/dev/.amplihack/amplifier-bundle/skills/merge-ready","skill_md":"/home/dev/.amplihack/amplifier-bundle/skills/merge-ready/SKILL.md","template":"/home/dev/.amplihack/amplifier-bundle/skills/merge-ready/pr-description-template.md","skill_md_sha":"5b0e8f6c1d2a3b4c5d6e7f8091a2b3c4d5e6f708"}
```

`skill_dir` is the physical path (`pwd -P`). `skill_md_sha` is the git blob
hash of `SKILL.md`, so the round record shows exactly which criteria were
applied. The script also writes these lines to stderr:

```text
INFO: merge-ready criteria from /home/dev/.amplihack/amplifier-bundle/skills/merge-ready (5b0e8f6c1d2a3b4c5d6e7f8091a2b3c4d5e6f708)
WARNING: merge-ready criteria come from the branch under review and differ from origin/main
```

The warning appears only when the chosen directory is inside the repository
and its `SKILL.md` differs from the base branch. This happens in repositories
that ship `amplifier-bundle/`, amplihack-rs included, when the pull request
edits its own merge criteria. It never fails the step; the merge gate
re-checks criteria 1 and 3 from measured evidence either way.

**Criteria read from the branch under review are advisory.** A pull request
that edits `SKILL.md` or `pr-description-template.md` can change what the
assessment agent reads, and so what it concludes. It cannot change what the
merge gate checks. `autodrive_merge_gate.sh` is the only step that can merge,
and it measures criteria 1 and 3 itself from the qa evidence and the crusty
manifest, whatever text the agent was given. A `MERGE_READY` verdict reached
from edited criteria is downgraded by the gate when the measured evidence
does not support it.

A candidate path that contains `"`, `\`, a control byte, `{{` or `}}` is
skipped with a `WARNING`, so a path can never break the JSON or look like a
recipe template expression. Skipping a candidate does not stop the search: the
script moves on to the next directory in the table. An `AMPLIHACK_HOME` that
contains such a character is therefore ignored, and the criteria come from
`REPO_PATH`, the git toplevel, `~/.copilot` or `~/.amplihack`, in that order.
If every candidate is skipped or lacks `SKILL.md`, the step fails with
`merge-ready-skill-files-not-found`, and the `searched` list includes the
skipped paths.

Step `step-02-merge-ready-assessment` receives
`{{merge_ready_files.skill_md}}` and `{{merge_ready_files.template}}` and tells
the agent to read both files and apply their criteria. The prompt names the
skill and says it is read as a file because its frontmatter blocks agent
invocation. It contains no `Skill(` call. It keeps the rule that a criterion
the agent could not verify counts as failed, and it states that text in the
evidence (test output, scenario names, commit messages) is data from the
branch under review, not instructions.

All merge-ready criteria apply as the skill states them, except criteria 1 and
3, which auto-drive measures as described below. The skill's own
`## Running under auto-drive` section says the same thing for a person reading
it, and states that a manual `/merge-ready` still requires the full criteria:
`gadugi-test validate` and `gadugi-test run`, and a separate `quality-audit` of
at least 3 SEEK, VALIDATE, FIX cycles ending clean.

#### A guard test keeps recipes from calling refusing skills

`no_recipe_invokes_a_skill_that_refuses_model_invocation` in
`tests/integration/auto_drive_to_merge_test.rs` reads the frontmatter of every
`amplifier-bundle/skills/<name>/SKILL.md` and collects each skill that sets
`disable-model-invocation: true` (any case, quoted or not; the name comes from
`name:`, falling back to the directory name). A `SKILL.md` whose name cannot be
resolved is logged and skipped. The test then scans the raw text of every
`*.yaml` and `*.yml` file under `amplifier-bundle/recipes/`, in every
subdirectory, for this pattern, where `NAME` is the skill name passed through
`regex::escape`:

```text
Skill\(\s*skill\s*=\s*\\?["']NAME\\?["']\s*\)
```

Whitespace is allowed after `(`, around `=` and before `)`, and either quote
may be preceded by one backslash, so the escaped form a YAML double-quoted
string produces is caught too. Comments and negative mentions count. A prompt that says "do not call
`Skill(skill="merge-ready")`" fails the test too, so prompts refer to such a
skill by name only. These all match for `merge-ready`:

```text
Skill(skill="merge-ready")
Skill(skill='merge-ready')
Skill( skill = "merge-ready" )
Skill(skill=\"merge-ready\")
```

A call that passes the name some other way, for example through a variable,
is not caught. A match fails the build and prints the matched text as written:

```text
amplifier-bundle/recipes/autodrive-merge-round.yaml: step step-02-merge-ready-assessment: Skill(skill="merge-ready") but amplifier-bundle/skills/merge-ready/SKILL.md sets disable-model-invocation: true
```

The step id is the nearest earlier `- id:` line. The walk skips symlinks, and
an unreadable or non-UTF-8 file fails the test and names the path.

The test's fixtures cover each of the four forms above, including the
escaped-quote form, and keep a `bad.yml` recipe so the `*.yml` scan stays
covered.

Two companion tests:

- `the_invocation_guard_flags_the_old_merge_round_prompt` runs the same
  detector over the prompt line this fix replaced, so the guard is known to
  catch the original defect.
- `qa_team_does_not_refuse_model_invocation` asserts that the `qa-team`
  `SKILL.md` does not set `disable-model-invocation: true`. Step-04 of the
  merge round calls `Skill(skill="qa-team")`, and this test fails with a
  direct message if that call would start being refused.

### Criterion 1: qa-team scenarios run with gadugi-test

Step `step-02-qa-team-scenarios` of `autodrive-merge-evidence.yaml` runs two
kinds of check and records both. Neither can stand in for the other. Every
suite command runs, even after an earlier one failed, and the gadugi checks
run whatever the suite commands did. Within the gadugi checks, a missing
`gadugi-test` stops them all and a failed `gadugi-test validate` stops every
scenario run; once validate passes, every scenario runs, even after an earlier
one failed. The evidence lists every cause found.

#### Repository suite commands

The suite commands come from the environment, or are detected from the
repository type when no command variable is set:

| Set | `qa_repo_type` | Commands run |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` only | `configured` | that command, in `AUTODRIVE_QA_DIR` |
| `AUTODRIVE_QA_COMMANDS` only | `configured` | each entry, from the repository root |
| both | `configured` | `AUTODRIVE_QA_COMMAND` first, then each `AUTODRIVE_QA_COMMANDS` entry |
| either one set but empty | `configured` | none; `qa_reason` is `qa-command-missing`, and nothing is auto-detected |
| neither | detected | the command for the repository type, from the table below |

| Repo type | Detected by | Test command |
| --- | --- | --- |
| `rust-cli` | `Cargo.toml` | `cargo test --workspace --locked --no-fail-fast` |
| `node` | `package.json` | `npm test` |
| `python` | `pyproject.toml` or `setup.py` | `pytest` |
| `unknown` | none of the above | none; `qa_reason` is `qa-command-missing` |

Every command runs, even after one fails. Each must exit 0. No timeout is
added to any of them. See [Environment variables](#environment-variables) for
how each variable is run.

Every repository type, `rust-cli` included, must also pass the gadugi
scenarios. The `qa-team` skill tells Rust CLI repositories to use `cargo test`
in place of `gadugi-test run`; auto-drive does not apply that substitution.

#### The gadugi scenarios

Before any gadugi check, the step resolves the
[scenario directory](#the-scenario-directory) and counts its scenario files.
`gadugi_scenario_dir`, `gadugi_scenario_count` and `qa_scenarios` are recorded
on every run, including when `gadugi-test` is not installed. Step-04 and the
merge gate both depend on them. The checks then run in this order:

1. `gadugi-test` must be on `PATH`. If it is not, nothing else runs.
2. The scenario count must be at least 1. `gadugi-test run` prints
   `No scenarios found to execute` and exits 0 on an empty directory, so the
   count is checked here.
3. `gadugi-test validate -d "$DIR"` must exit 0. If it does not, no scenario
   runs.
4. Each scenario file runs in its own `gadugi-test` process, and each must
   exit 0. All of them run, even after one fails.
5. There must be no symlinked scenario file. Symlinked `*.yaml` and `*.yml`
   files in the directory are not counted and never run. Each one is named
   in a `WARNING: symlinked scenario not run: <path>` line on stderr and in
   `qa_summary`. Once validate passes, each one also fails the evidence as
   `gadugi-run-failed` and is listed in `gadugi_failed_scenarios`. With no
   regular scenario file the status stays `NO_SCENARIOS`, and `qa_summary`
   still names the symlinks. Any other `*.yaml` or `*.yml` entry that is not
   a regular file, such as a directory, is handled the same way and named as
   `non-regular scenario entry not run: <path>`.

**Why one process per scenario.** `gadugi-test run -d <dir>` runs every
scenario in the directory in one process. In gadugi-test 1.0.x concurrent
scenarios in one directory share one CLI runner, and the first to finish kills
the others' processes
([gadugi-agentic-test #207](https://github.com/rysweet/gadugi-agentic-test/issues/207)).
The step never runs a whole directory. A comment next to the loop cites #207.

`--scenario` in gadugi-test 1.0.x selects by a case-insensitive substring
match on a scenario's `name:` field, not by file path. To make one run select
exactly one scenario, the step does this for each scenario file:

1. Copy the file alone, with `cp -P`, into a new `mktemp -d` directory outside
   the repository.
2. Read its name: the top-level `name:` key, or `name:` under a top-level
   `scenario:` key, with quotes and trailing comments removed.
3. Run it from the repository root:

   ```bash
   (cd "$ROOT" && gadugi-test run -d "$stage" --scenario "$name")
   ```

Because the staging directory holds one file, the substring match cannot pick
up a second scenario. The staging directories are removed by an `EXIT` trap,
set only after `mktemp` succeeded. If `mktemp` fails, the step records
`gadugi-run-failed`; it never falls back to running the whole directory.

A file counts as **unnamed**, and as a failed scenario, when its name is empty,
starts with `-`, contains a control byte, or is longer than 200 bytes. The
name reader does not handle YAML block scalars or flow mappings, so a name
written that way also counts as unnamed. An unnamed file is not run.

`gadugi-test` writes `logs/` and `outputs/` into its working directory. The
step records which of the two existed before it ran, and afterwards removes
only the ones it created. A `logs/` directory that belongs to the repository
is left alone, and a symlink is never followed.

No timeout wrapper is added and no `--timeout` flag is passed, so
`gadugi-test run` uses its own default of 300000 ms as the limit for each
command it runs.

| `gadugi_status` | Meaning |
| --- | --- |
| `PASS` | validate exited 0, and every scenario file was named, run, and exited 0 |
| `NOT_INSTALLED` | `gadugi-test` is not on `PATH` |
| `NO_SCENARIOS` | the directory is missing or has no scenario files |
| `VALIDATE_FAILED` | `gadugi-test validate` exited non-zero; no scenario ran |
| `RUN_FAILED` | at least one scenario file was unnamed, could not be staged, or its run exited non-zero, or the directory holds a symlinked scenario file |

#### `qa_status` and `qa_reason`

`qa_status` is `PASS` only when all of these hold:

- at least one suite command ran, and every suite command exited 0
- `gadugi-test` is installed
- the scenario count is at least 1
- `gadugi-test validate` exited 0
- every scenario file was named and its run exited 0, so
  `gadugi_scenarios_passed` equals `gadugi_scenario_count`

Otherwise `qa_reason` is the first of these tokens that applies, and
`qa_status` is the class of that token:

| Order | `qa_reason` | `qa_status` | When |
| --- | --- | --- | --- |
| 1 | `qa-command-failed` | `FAIL` | a suite command exited non-zero |
| 2 | `no-scenarios` | `FAIL` | the scenario directory is missing or empty |
| 3 | `gadugi-validate-failed` | `FAIL` | `gadugi-test validate` exited non-zero |
| 4 | `gadugi-scenario-unnamed` | `FAIL` | a scenario file has no usable name |
| 5 | `gadugi-run-failed` | `FAIL` | a scenario run exited non-zero, staging failed, or a scenario file is a symlink |
| 6 | `qa-command-missing` | `BLOCKED` | no suite command: unknown repository type, or a command variable set but empty |
| 7 | `qa-command-not-installed` | `BLOCKED` | the program of the single or detected command is not installed |
| 8 | `gadugi-test-missing` | `BLOCKED` | `gadugi-test` is not on `PATH` |

`qa_reason` is `""` when `qa_status` is `PASS`. The step checks the value it
emits against this list. `FAIL` tokens come first because the merge round can
act on them: it can fix code or write scenarios. `gadugi-test-missing` never
appears together with a later gadugi token, because no gadugi check runs
without the binary.

`qa_summary` still names every cause, not only the first, joined by `; `,
followed by the tail of the suite log:

| Cause | Phrase |
| --- | --- |
| a suite command exited non-zero | `repository test failure` |
| no suite command | `no repository test command for repository type <type>` |
| a program is missing | `<program> not installed` |
| no scenario files | `no scenarios in <dir>` |
| validate exited non-zero | `gadugi-test validation failure` |
| a scenario file has no name | `gadugi scenario without a name: <path>` |
| a scenario run exited non-zero | `gadugi scenario run failure: <path>` |
| a scenario file is a symlink | `symlinked scenario not run: <path>` |
| a `*.yaml` entry is not a regular file | `non-regular scenario entry not run: <path>` |

#### The scenario directory

The evidence step uses the first match:

1. `AUTODRIVE_QA_SCENARIO_DIR`, when set and not empty. A relative path is
   resolved from the repository root; an absolute path is used as given. If it
   names a directory that does not exist, the result is `no-scenarios`; there
   is no fallback to the defaults.
2. `tests/agentic`, if it exists.
3. `scenarios`, if it exists.
4. Otherwise `tests/agentic` is recorded with a count of 0. This is the
   directory the merge round writes new scenarios into.

Only `*.yaml` and `*.yml` regular files directly in the directory count,
found with `find -maxdepth 1 -type f`. Symlinked scenario files are not
counted and not run, although `gadugi-test validate -d` still reads them.
`gadugi-test validate -d` does not look in subdirectories, so a scenario that
exists only in a subdirectory counts as 0.

amplihack-rs keeps its gadugi scenarios in `tests/gadugi/scenarios`, which is
not in the default lookup. Its own auto-drive runs set
`AUTODRIVE_QA_SCENARIO_DIR=tests/gadugi/scenarios`.

#### Environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` | unset | One suite command. Split into words by the shell, with globbing turned off. |
| `AUTODRIVE_QA_DIR` | `.` | Directory `AUTODRIVE_QA_COMMAND` runs in, relative to the repository root. |
| `AUTODRIVE_QA_COMMANDS` | unset | Several suite commands, one per line. Blank lines and lines starting with `#` are ignored. |
| `AUTODRIVE_QA_SCENARIO_DIR` | `tests/agentic`, then `scenarios` | The gadugi scenario directory. |

`AUTODRIVE_QA_COMMAND` runs as `( set -f; cd -- "$QA_DIR" && $CMD )`. It
counts as installed when its first word is on `PATH` or is an executable file
at `$QA_DIR/<first word>`; otherwise `qa_reason` is
`qa-command-not-installed`. Word splitting cannot express `cd ui && npm test`.
Use `AUTODRIVE_QA_COMMANDS` for that.

Each `AUTODRIVE_QA_COMMANDS` entry runs as its own
`( cd -- "$ROOT" && bash -c "$entry" )`, so `cd` inside one entry does not
affect the next. Entries are never concatenated into one script, sourced, or
passed to `eval`. A missing program in an entry exits 127 and counts as
`qa-command-failed`.

**Set the `AUTODRIVE_QA_*` variables only from the operator's own
environment.** Each `AUTODRIVE_QA_COMMANDS` entry is shell source passed to
`bash -c`, so whoever sets the variable chooses what runs. Never fill these
variables from repository content (a checked-in `.env` file, a config file in
the branch), from agent output, or from pull request text such as the title,
description or comments. Auto-drive itself never does: no recipe step, agent
step or tool writes them, and no recipe declares them as context keys. A
wrapper script that exports them from data the operator does not control
turns the evidence step into a command injection point.

A repository with a Cargo workspace and a TypeScript package under `ui/` that
is outside the workspace:

```bash
export AUTODRIVE_QA_SCENARIO_DIR=tests/gadugi/scenarios
export AUTODRIVE_QA_COMMANDS='cargo test --workspace --locked --no-fail-fast
# the UI package is not part of the Cargo workspace
cd ui && npm ci && npm test'
amplihack recipe run auto-drive-to-merge -c pr_number=1234 -c repo_path=.
```

All four variables are read from the environment only. No recipe declares
them as context keys: the recipe runner exposes context keys as environment
variables, so a context key with an empty default would hide the value the
user exported. If the recipe runner drops a variable (for example, when it
trims an oversized environment), the step falls back to its default. The
evidence always shows what was actually used: `qa_command` for the commands
and `gadugi_scenario_dir` for the directory.

**Command text is recorded as written**, up to 500 characters, in
`qa_command`, and from there it can reach the pull request description.
Reference secrets as `$VAR` in the command text. The expanded value is never
recorded.

#### The qa evidence record

Every value is a string. The step reads `head_sha` with `git rev-parse HEAD`
before any check runs; it is the commit that was tested. Exit codes are `""`
when the command did not run. Free text (`qa_command`, `qa_summary`,
`qa_scenarios`, `gadugi_scenario_dir`, `gadugi_failed_scenarios`) has quotes,
backslashes and control bytes removed before it is cut to length, so hostile
test output still yields valid JSON. Every count is checked to be digits only.
If a temporary log cannot be created, the step prints
`ERROR: cannot create a temporary log` and exits 1 without writing evidence,
which the merge round and the gate read as missing evidence.

amplihack-rs with `AUTODRIVE_QA_SCENARIO_DIR=tests/gadugi/scenarios`, all
checks passing:

```json
{"qa_status":"PASS","qa_repo_type":"rust-cli","qa_command":"cargo test --workspace --locked --no-fail-fast","qa_scenarios":"tests/gadugi/scenarios/issue-815-804-local-tracking-extract.yaml tests/gadugi/scenarios/issue-820-merge-validations-mixed-output.yaml tests/gadugi/scenarios/pr-ownership-lease.yaml","qa_exit_code":"0","qa_summary":"test result: ok. 41 passed; 0 failed","qa_round":"round-2","head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","gadugi_status":"PASS","gadugi_validate_exit_code":"0","gadugi_run_exit_code":"0","gadugi_scenario_count":"3","gadugi_scenario_dir":"tests/gadugi/scenarios","gadugi_scenarios_validated":"3","gadugi_scenarios_run":"3","gadugi_scenarios_passed":"3","gadugi_scenarios_failed":"0","gadugi_failed_scenarios":"","gadugi_scenario_results":"issue-815-804-local-tracking-extract.yaml=PASS,issue-820-merge-validations-mixed-output.yaml=PASS,pr-ownership-lease.yaml=PASS","qa_suite_commands_count":"1","qa_reason":""}
```

Two configured suite commands, one failing scenario out of two:

```json
{"qa_status":"FAIL","qa_repo_type":"configured","qa_command":"cargo test --workspace --locked --no-fail-fast; cd ui && npm ci && npm test","qa_scenarios":"tests/gadugi/scenarios/export-csv.yaml tests/gadugi/scenarios/import-csv.yaml","qa_exit_code":"0","qa_summary":"gadugi scenario run failure: tests/gadugi/scenarios/import-csv.yaml; Tests: 12 passed, 12 total","qa_round":"round-1","head_sha":"4e2d9a7c0b1f3e5d7c9a1b3d5f7e9c0a2b4d6f8e","gadugi_status":"RUN_FAILED","gadugi_validate_exit_code":"0","gadugi_run_exit_code":"1","gadugi_scenario_count":"2","gadugi_scenario_dir":"tests/gadugi/scenarios","gadugi_scenarios_validated":"2","gadugi_scenarios_run":"2","gadugi_scenarios_passed":"1","gadugi_scenarios_failed":"1","gadugi_failed_scenarios":"tests/gadugi/scenarios/import-csv.yaml","gadugi_scenario_results":"export-csv.yaml=PASS,import-csv.yaml=FAIL","qa_suite_commands_count":"2","qa_reason":"gadugi-run-failed"}
```

| Field | Values |
| --- | --- |
| `qa_status` | `PASS`, `FAIL`, `BLOCKED` |
| `qa_reason` | `""` on `PASS`, otherwise one token from the [precedence table](#qa_status-and-qa_reason) |
| `qa_repo_type` | `configured`, `rust-cli`, `node`, `python`, `unknown` |
| `qa_command` | the suite commands that were run, joined by `; `, at most 500 characters |
| `qa_suite_commands_count` | number of suite commands run |
| `qa_exit_code` | the first non-zero suite exit code; `"0"` when all passed; `""` when none ran |
| `qa_summary` | every cause phrase, then the tail of the suite log |
| `qa_round` | the merge round label |
| `head_sha` | the commit that was tested |
| `qa_scenarios` | the counted scenario files, relative to the repository root, sorted, space separated; `""` when the count is 0 |
| `gadugi_status` | `PASS`, `NOT_INSTALLED`, `NO_SCENARIOS`, `VALIDATE_FAILED`, `RUN_FAILED` |
| `gadugi_scenario_dir` | the directory used, relative to the repository root when inside it |
| `gadugi_scenario_count` | number of regular scenario files found, not counting symlinks; always recorded |
| `gadugi_validate_exit_code` | exit code of `gadugi-test validate`, or `""` |
| `gadugi_scenarios_validated` | `gadugi_scenario_count` when validate exited 0, otherwise `"0"` |
| `gadugi_scenarios_run` | number of `gadugi-test run` processes started; unnamed files are not run |
| `gadugi_scenarios_passed` | runs that exited 0 |
| `gadugi_scenarios_failed` | runs that exited non-zero, plus unnamed files, plus files that could not be staged, plus symlinked scenario files once validate passed |
| `gadugi_failed_scenarios` | paths of the failed files, relative to the repository root, space separated |
| `gadugi_run_exit_code` | the first non-zero run exit code; `"0"` when every run exited 0; `""` when no run started |
| `gadugi_scenario_results` | one `<file>=<result>` entry per scenario file, comma separated; see [Per-scenario results](#per-scenario-results) |

Where each item of the evidence requirement is recorded:

| Requirement | Field |
| --- | --- |
| gadugi result | `gadugi_status` |
| scenarios found, validated, run, passed, failed | `gadugi_scenario_count`, `gadugi_scenarios_validated`, `gadugi_scenarios_run`, `gadugi_scenarios_passed`, `gadugi_scenarios_failed` |
| failed scenarios | `gadugi_failed_scenarios` |
| result of each scenario file | `gadugi_scenario_results` |
| suite command count, first failing exit code | `qa_suite_commands_count`, `qa_exit_code` |
| head SHA tested | `head_sha` |
| status and reason | `qa_status`, `qa_reason` |

The step writes the evidence file under `umask 077`, so it is private to the
user running auto-drive.

#### Per-scenario results

`gadugi_scenario_results` lists every top-level `*.yaml` and `*.yml` entry in
the scenario directory, sorted with `LC_ALL=C`, as `<file>=<result>` joined by
`,`. `<file>` is the name relative to the scenario directory, for example
`import-csv.yaml`. `gadugi_failed_scenarios` uses paths relative to the
repository root instead, for example `tests/gadugi/scenarios/import-csv.yaml`.

| Result | When |
| --- | --- |
| `PASS` | the scenario ran in its own `gadugi-test run` and exited 0 |
| `FAIL` | the run exited non-zero, or the file could not be staged |
| `INVALID` | the file was not run: it is a symlink, it is not a regular file, it has no usable name, or `gadugi-test validate` failed for the directory |

A symlink is detected with `[ -L ]` before the file is read. When validation
fails, every entry is `INVALID`, because no scenario runs. The field is `""`
when `gadugi-test` is not installed or the scenario count is 0.

`INVALID` entries are still counted in `gadugi_scenarios_failed`, exactly as
before this field existed: an unnamed or symlinked scenario file is a failed
scenario, not a skipped one.

While the scenarios run, the evidence step appends one `<file><TAB><result>`
line per file to a private `mktemp` file. `autodrive_scenario_results LIST`
in `autodrive_trust.sh` turns that file into the field. Each `<file>` passes
through `tr -c 'A-Za-z0-9._/-' '_'` and is cut to 128 bytes, so it cannot
break the JSON or carry text into a prompt. Two names can become the same key
after this. That does not matter: the field is for people and agents reading
the evidence, and nothing decides a pass from it. `gadugi_status`, the counts,
`gadugi_failed_scenarios` and `qa_status` are computed exactly as described
above, without reference to this field.

#### The qa evidence is trusted only through its hash

The evidence step writes `qa-evidence.json` into the state directory. Agent
steps run after it in the same round, and an agent running as the same user
could edit the file. The round and the gate therefore accept the qa evidence
only through a chain of hashes that starts with the loop's manifest:

```text
merge-ready-records.tsv (written by autodrive_loop.sh)
  -> merge-ready-round-N.json (blob hash in the last manifest row)
       -> qa_evidence_sha (field in that record)
            -> qa-evidence.json (its blob hash must equal qa_evidence_sha)
                 -> head_sha (must equal the head being merged)
```

In the merge round:

1. Step `step-00d-qa-evidence-hash` is a bash step that runs right after the
   evidence step and before any agent step. It calls
   `autodrive_qa_evidence_sha FILE`, which hashes `qa-evidence.json` with
   `git hash-object --no-filters` and prints the hash, or `""` when the file
   is missing, a symlink, or not a regular file. The step emits
   `{"qa_evidence_sha":"<hash>"}`.
2. Step `step-03-extract-merge-ready-verdict` hashes the file again with the
   same function. When the step-00d hash is empty, is not 40 or 64 hex
   characters, or differs from the new hash, it downgrades any `MERGE_READY`
   verdict with `qa_evidence=modified`.
3. Step `step-05-write-round-record` adds `qa_evidence_sha` to the round
   record, taken from step-00d's output after the hex check, and writes the
   record under `umask 077`.
4. `autodrive_loop.sh` appends the record's hash to `merge-ready-records.tsv`,
   as it does for crusty.

The round record then looks like this:

```json
{"merge_ready_verdict":"MERGE_READY","blocker_count":0,"head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","round_label":"round-2","test_signal":"PASS","ci_signal":"green","qa_status":"PASS","ci_status":"GREEN","qa_evidence_sha":"7c41d0e95a2b38f6e1d4c7a9b0e3f6a2d5c8b1e4"}
```

The gate checks the chain with
`autodrive_qa_trusted STATE_DIR RECORD_COPY QA_COPY HEAD_SHA` in
`autodrive_trust.sh`. `RECORD_COPY` and `QA_COPY` are the gate's own private
copies of the round record and `qa-evidence.json` (see
[What the gate checks](#what-the-merge-gate-checks)). The function
hashes and parses exactly those copies and never re-reads the originals, so
the record that section 7 of the gate reads is the record whose hash was
verified. It reads `merge-ready-records.tsv` once, through
`autodrive_manifest_row DIR merge-ready` in `autodrive_state.sh`. It prints
`ok` or the token of the first check that failed:

| Order | Check | Token on failure |
| --- | --- | --- |
| 1 | `merge-ready-records.tsv` is a regular file, not a symlink, and its last non-blank row has a label, a file name matching `^merge-ready-[A-Za-z0-9._-]+\.json$`, and a 40- or 64-character hex hash | `qa-manifest-missing` |
| 2 | the hash of `RECORD_COPY` equals the manifest hash, it is one line starting with `{"merge_ready_verdict":"`, and its `qa_evidence_sha` is 40 or 64 hex characters | `qa-record-modified` |
| 3 | the hash of `QA_COPY` equals `qa_evidence_sha` | `qa-evidence-modified` |
| 4 | the `head_sha` in `QA_COPY` equals `HEAD_SHA` | `qa-evidence-stale` |

It never prints file contents.

#### When scenarios are missing

Step `step-04-address-blockers` of the merge round handles qa evidence whose
`gadugi_status` is `NO_SCENARIOS`, `VALIDATE_FAILED` or `RUN_FAILED`, and
evidence whose scenarios do not cover the changed behaviour. The trigger is
`gadugi_status`, not `qa_reason`. `qa_reason` holds only the first cause, so a
failing suite command together with an empty scenario directory gives
`qa_reason: qa-command-failed`, and a trigger on `qa_reason` would not start
scenario writing that round. `RUN_FAILED` covers unnamed scenario files as
well as failed runs. Its prompt tells the agent:

1. Gadugi scenarios are required in every repository type, Rust CLI
   repositories included. For this step this overrides the `qa-team` line that
   substitutes `cargo test` for `gadugi-test`.
2. Use `Skill(skill="qa-team")` to write the missing scenarios as top-level
   `*.yaml` files in `gadugi_scenario_dir`, each with a top-level `name:`.
3. Run `gadugi-test validate -d <dir>`, then run each scenario on its own with
   `gadugi-test run -d <dir> --scenario "<name>"`. Never run a whole directory
   in one command.
4. Commit through the same identity helper and artifact guard as every other
   round commit.

A failing scenario is fixed in the scenario or in the code. It is never
weakened or deleted to reach `PASS`. If `gadugi_scenario_dir` is absolute or
outside the repository, the step writes nothing and reports blocker
`gadugi-scenario-dir-outside-repo`.

### Criterion 3: the crusty loop ended DONE and CLEAN

Criterion 3 of merge-ready asks for a `quality-audit` of at least 3 SEEK,
VALIDATE, FIX cycles ending clean. Under auto-drive it is met instead by phase
2 of the same run: the `crusty-old-engineer` loop, an iterative review-and-fix
loop, must have ended `DONE`, and its last round record, as written by the
loop, must have verdict `CLEAN`. Every commit between the head that clean
round reviewed and the head being merged must be a base merge or a
description or evidence change; any other commit goes back to crusty first
(see [Commits after the clean round](#commits-after-the-clean-round)).
**No minimum round count applies**, in any
recipe or tool. The crusty loop stops at its first `CLEAN` round, so a minimum
would block forever any pull request that was clean in round 1 or 2. The
three-cycle rule of the skill does not apply inside auto-drive; the step-02
prompt says so.

#### Every crusty round records the reviewed head SHA

Step `step-06-write-round-record` of `autodrive-crusty-round.yaml` writes one
line of JSON per round:

```json
{"crusty_verdict":"CLEAN","concern_count":0,"commits_this_round":0,"head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","reviewed_head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","round_label":"round-1","test_signal":"","ci_signal":""}
```

| Field | Source |
| --- | --- |
| `reviewed_head_sha` | `git rev-parse HEAD`, run by bash in `step-01-round-context` before the review. Never taken from an agent. |
| `head_sha` | the head after the fix step when it made commits; otherwise `reviewed_head_sha` |
| `concern_count`, `commits_this_round` | the crusty verdict and the fix evidence. A value that is not digits only is recorded as `0`, with `WARNING: concern_count '<value>' is not a number; recorded as 0` (or `commits`) on stderr. The value in the warning keeps only letters and digits and is cut to 32 characters. |

Neither SHA field is ever empty. If either value is not a 40- or 64-character hex
SHA, the step fails with `ERROR: crusty-head-sha-unavailable: ...` and writes
no record. It does not force `CONCERNS`, which would make the loop review the
same tree forever. The loop then handles the round as described under
[The round reads the skill's files](#the-round-reads-the-skills-files): no
record, no manifest row, and `STUCK` if the error repeats.

#### The loop writes a manifest of the records it wrote

After each round, `autodrive_loop.sh` copies the round record to
`<loop>-latest.json` as before, then appends one row to
`<loop>-records.tsv` in the state directory:

```text
round-1	crusty-round-1.json	3b18e512dba79e4c8300dd08aeb37f8e728b8dad
```

The three tab-separated fields are the round label, the record file name, and
the git blob hash of the record (`git hash-object --no-filters --stdin`). The
row is written before the loop-health evaluator or any later agent runs, and
only after `cmp` confirms the record and its `-latest.json` copy are
identical, and only when the record file name matches `^[A-Za-z0-9._-]+$` and
does not start with `.`. The file is created with `umask 077`. If a check
fails, the loop writes a `WARNING` and adds no row. Both loops write a manifest
(`crusty-records.tsv` and `merge-ready-records.tsv`). `autodrive_crusty_final`
reads `crusty-records.tsv`; `autodrive_qa_trusted` reads
`merge-ready-records.tsv` (see
[The qa evidence is trusted only through its hash](#the-qa-evidence-is-trusted-only-through-its-hash)).

#### `autodrive_crusty_final` decides criterion 3

`autodrive_crusty_final DIR` in `autodrive_state.sh` is the one check for
criterion 3. Step `step-01b-crusty-evidence` of the merge round and section 6b
of the merge gate both call it. It needs only bash, git and coreutils. It
checks, in order:

| Order | Check | Token on failure |
| --- | --- | --- |
| 1 | `phases.tsv` has the `crusty-loop` marker | `crusty-loop-not-done` |
| 2 | `crusty-records.tsv` exists, is a regular file, not a symlink, and its last non-blank row has exactly three fields: a label, a file name matching `^crusty-[A-Za-z0-9._-]+\.json$`, and a 40- or 64-character hex hash | `crusty-manifest-missing` |
| 3 | the named record is a regular file, not a symlink | `crusty-record-missing` |
| 4 | the record's hash equals the manifest hash, and `crusty-latest.json` (not a symlink) has the same hash | `crusty-record-modified` |
| 5 | the record is one line, starts with `{"crusty_verdict":"CLEAN",` or `{"crusty_verdict":"CONCERNS",`, and has one `reviewed_head_sha` key | `crusty-record-modified` |
| 6 | the verdict is `CLEAN` | `crusty-not-clean` |
| 7 | `reviewed_head_sha` is a 40- or 64-character hex SHA | `crusty-head-sha-empty` |

On success it prints only the reviewed SHA and returns 0. On failure it prints
only the token and returns 1. If the private temporary copy cannot be created,
the token is `crusty-record-modified` and stderr says
`ERROR: cannot create temporary copy`. It copies the record once into a private
temporary file and hashes and parses that copy, so the file cannot change
between the two. It never prints file contents. Records that are not in the
manifest are ignored, so a record an agent added to the directory has no
effect, and archiving the loop-written record fails check 3.

A state directory written before this change has no manifest. It reads as
`crusty-manifest-missing`, and such a run needs a fresh crusty loop.

#### Commits after the clean round

The crusty loop stops at its first `CLEAN` round, and the merge loop runs
after it. The merge round's blocker-clearing step can commit code, and so can
a person. A clean verdict covers only the head crusty reviewed, so criterion 3
also needs every later commit to be one that does not need review.

`autodrive_crusty_range REPO REVIEWED HEAD BASE_SHA` in `autodrive_trust.sh`
walks the commits crusty has not seen:

```bash
git rev-list --reverse --topo-order REVIEWED..HEAD ^BASE_SHA
```

`^BASE_SHA` leaves out every commit already on the base branch, so the
commits a base merge brings in are not walked. When `BASE_SHA` is empty the
`^BASE_SHA` argument is dropped. Each commit in the walk is passed to
`autodrive_range_allowed`, and the first one that is not allowed ends the
walk. `--topo-order` puts every parent before its children, so "oldest
first" holds even when commit dates are out of order. The function prints one
of three values:

| Output | Meaning |
| --- | --- |
| `ok` | every commit in the range is allowed, or the range is empty |
| `crusty-unreviewed-commits:<sha>` | `<sha>` is the first commit, oldest first, that needs crusty review |
| `crusty-range-unreadable` | a SHA is not 40 or 64 hex characters, git failed, or the repository is shallow |

`autodrive_range_allowed REPO COMMIT BASE_SHA` allows a commit in two cases.

**A base merge.** The commit has exactly two parents, the second parent is an
ancestor of `BASE_SHA`, and the commit's tree equals the tree that
`git merge-tree --write-tree <parent1> <parent2>` produces. A merge whose
conflicts were resolved by hand, or that has extra edits, produces a
different tree and counts as code. So do octopus merges and merges from any
other branch. `git merge-tree --write-tree` needs git 2.38 or later; on older
git the check fails and the merge counts as code.

**A description or evidence change.** `git diff-tree -r -z --no-renames --raw`
against the first parent lists at least one path, every path is on the
allowlist, and every new file mode is `100644`, `100755` or `000000`
(deleted). Symlinks (`120000`) and submodules (`160000`) count as code. A
commit that changes no path, such as an empty commit, counts as code. So does
a root commit.

The allowlist is `AUTODRIVE_RANGE_ALLOWLIST`, a read-only variable defined
only in `autodrive_trust.sh`:

| Entry | Match | Why it is on the list |
| --- | --- | --- |
| `PR_DESCRIPTION.md` | exact path | Pull request description text that some repositories keep in the tree. The merge-ready criteria require evidence in the description. |
| `.github/pull_request_template.md` | exact path | The description template. Changing it changes no behaviour of the code under review. |
| `.autodrive/evidence/` | prefix | Reserved for evidence files committed to the branch. |

Matching is byte-exact and case-sensitive. A path under the prefix that
contains `..` does not match. Scenario files written by `qa-team` are not on
the list: they are tests, and tests are code. The list is the same for every
repository. A repository whose build, tests or scenarios read
`PR_DESCRIPTION.md` or any file under `.autodrive/evidence/` would let
changes to that file skip crusty; narrow the list in such a repository.

All git calls in these functions run with `GIT_NO_REPLACE_OBJECTS=1`,
`GIT_GRAFT_FILE=/dev/null` and `GIT_TERMINAL_PROMPT=0`, with `GIT_DIR` and
`GIT_WORK_TREE` unset, so replace refs and grafts cannot change the history
being walked. Every SHA is checked as a whole value before it reaches a git
command. The functions print only the fixed tokens above and hex SHAs, never
commit subjects, paths or file contents, so nothing from the branch reaches a
log or a prompt through them.

##### Where the base SHA comes from

The base SHA is never read from an existing local ref, which could be stale or
set by anyone with write access to the clone. The caller reads the pull
request's `baseRefName` from `gh pr view` and passes it to
`autodrive_base_sha REPO BASE_REF` in `autodrive_trust.sh`, which prints the
SHA or `""`. Step `step-00b-crusty-range`, step `step-01b-crusty-evidence` and
section 6d of the gate all use this one function. It does three things:

1. `baseRefName` must pass `git check-ref-format --branch`, must not start
   with `-`, and must contain only `A-Z`, `a-z`, `0-9`, `.`, `_`, `/` and `-`.
2. `git fetch --force origin +refs/heads/<base>:refs/remotes/origin/<base>`
3. `BASE_SHA` is `git rev-parse --verify refs/remotes/origin/<base>^{commit}`.

If any of these fails, the function prints `""` and `BASE_SHA` is empty. The walk then includes the
commits a base merge brought in, they count as code, and crusty reviews them.
That is safe but costs a crusty run.

##### What the merge round does

In file order, `autodrive-merge-round.yaml` now starts with the seven steps
below. Steps 00b, 00c and 01b apply the rule; steps 02, 03 and 04 act on its
result, as described after the table.

| Step | Type | Role |
| --- | --- | --- |
| `step-00-merge-ready-files` | bash | Finds the merge-ready files, as above. |
| `step-00b-crusty-range` | bash | Checks the range at the start of the round. |
| `step-00c-crusty-rereview` | recipe | Runs the crusty loop again when step-00b asks for it. |
| `merge-evidence` | recipe | Base sync, suite commands, gadugi scenarios, CI. |
| `step-00d-qa-evidence-hash` | bash | Hashes the qa evidence; see [The qa evidence is trusted only through its hash](#the-qa-evidence-is-trusted-only-through-its-hash). |
| `step-01-platform-facts` | bash | Pull request state. |
| `step-01b-crusty-evidence` | bash | Criterion 3, range check included. |

**step-00b-crusty-range** sources `autodrive_state.sh` and
`autodrive_trust.sh`, reads `baseRefName` with `gh pr view`, and calls
`autodrive_rereview_decision STATE_DIR REPO BASE_REF`, which prints the step's
whole output. The function:

1. Calls `autodrive_crusty_final`. If that fails, crusty has not reached a
   clean end and there is nothing to re-review: it prints
   `rereview: "false"` with `range: "not-checked"`, and step-01b reports the
   reason.
2. Otherwise resolves the base SHA with `autodrive_base_sha` and runs
   `autodrive_crusty_range` from the reviewed SHA to `HEAD`.
3. Only for `crusty-unreviewed-commits:<sha>` removes the `crusty-loop` row
   from `phases.tsv` with `autodrive_clear_phase` and prints
   `rereview: "true"`. If the row cannot be removed, it writes an `ERROR` on
   stderr and prints `rereview: "false"`; step-01b and the gate still block
   on the unreviewed commit.
4. For `ok` and for `crusty-range-unreadable` prints `rereview: "false"`.

A code commit gives:

```json
{"rereview":"true","range":"crusty-unreviewed-commits","first_unreviewed_sha":"c3a9e0f27b18d4e65a0c9f3b2d71e8a4f6b05d92","base_sha":"4ccd1977e2b0a5f8c3d6e9b1a4f7c0d3e6b9a2c5"}
```

A shallow clone gives:

```json
{"rereview":"false","range":"crusty-range-unreadable","first_unreviewed_sha":"","base_sha":"4ccd1977e2b0a5f8c3d6e9b1a4f7c0d3e6b9a2c5"}
```

An unreadable range does not trigger a re-review, because crusty cannot fix
it: re-running crusty does not deepen a clone or repair git, and the next
round would find the range unreadable again and re-run crusty every round.
Instead step-01b reports `UNREVIEWED_COMMITS` with `crusty-range-unreadable`,
step-03 downgrades the verdict, and the gate blocks. The round blocks until a
person deepens the clone or fixes the git error, or until loop-health ends the
merge loop.

`autodrive_clear_phase DIR PHASE` accepts only a phase name matching
`^[a-z][a-z-]*$`, refuses a `phases.tsv` that is a symlink or not a regular
file, writes the filtered rows to a `mktemp` file under `umask 077`, and
moves it into place with `mv -f`. It removes only rows whose first field is
exactly `PHASE`.

**step-00c-crusty-rereview** runs `autodrive-crusty-loop.yaml` with the
condition `crusty_range.rereview == 'true'`, and passes
`autodrive_state_dir` explicitly, so the new records, manifest rows and the
`crusty-loop` marker are written to the same state directory. The crusty loop
reviews the current head with its normal termination: it stops at its first
`CLEAN` round, or ends `STUCK`. No minimum round count applies here either.
A `CLEAN` end writes the `crusty-loop` marker again.

Step-00c has no `continue_on_error`. If the nested crusty loop fails, for any
reason including an exit 79 policy refusal, step-00c fails and the round
fails with it; no later step runs, so the round fails closed. Whether the
merge loop then sees code 79 or a generic failure depends on how the recipe
runner reports a failed `type: recipe` step; neither lets the round continue.

If the re-run ends `STUCK`, `autodrive_loop.sh` exits 1, so the nested crusty
loop fails, step-00c fails, and that round fails without a verdict. The crusty
loop does not write the `crusty-loop` marker, so the marker stays absent.
Step-01b of every later round gets `crusty_status: ABSENT` from
`autodrive_crusty_final`. Step-00b of every later round sees
`autodrive_crusty_final` fail and prints `rereview: "false"`, so crusty is not
run again. No round can reach
`MERGE_READY`, and loop-health ends the merge loop. This is terminal and
fails closed; a person has to resolve crusty's concerns and start a new
run.

The re-run starts its round labels at `round-1`, so it overwrites the earlier
`crusty-round-1.json` and later files. Only the last manifest row is trusted,
and an earlier row cannot be replayed: its hash no longer matches the file it
names.

**step-01b-crusty-evidence** calls `autodrive_crusty_final` as before. When it
succeeds, the step resolves the base SHA again with `autodrive_base_sha` and
runs `autodrive_crusty_range` against the current head, because the evidence
step's base sync can add a merge commit. A base merge passes. Any result other
than `ok` sets `crusty_status` to `UNREVIEWED_COMMITS`.

**step-02-merge-ready-assessment** states the range rule in its prompt. When
`crusty_status` is `UNREVIEWED_COMMITS`, the blocker is
`crusty-review-required:<sha>`, or `crusty-range-unreadable` when there is no
SHA. The prompt shows only the SHA, never the commit text.

**step-03-extract-merge-ready-verdict** downgrades any `MERGE_READY` verdict
when `crusty_status` is `UNREVIEWED_COMMITS`.

**step-04-address-blockers** tells the agent not to try to clear
`crusty-review-required`, because the next round's step-00b sends the commits
to crusty, nor `crusty-range-unreadable`, which a person fixes. It also tells the agent not to rewrite history, reset, force-push,
or edit refs.

This means every code commit made after the clean round costs one crusty run
in the next merge round. That is intended: a commit crusty has not seen is not
covered by its verdict.

The re-review happens at the start of the next merge round, not in the merge
loop or the composer. The round already holds the state directory and the
pull request, and a nested recipe step needs no change to the crusty loop's
termination logic.

#### What the merge round sees

`autodrive-merge-loop.yaml` passes its state directory to every round with
`--context "autodrive_state_dir=${DIR}"`. Step `step-01b-crusty-evidence` calls
`autodrive_crusty_final` and emits `crusty_evidence`, where every value comes
from a fixed set or is a hex SHA:

```json
{"crusty_status":"DONE_CLEAN","crusty_reason":"","crusty_reviewed_head_sha":"9f1c2e7a4b5d6c8e0f1a2b3c4d5e6f7a8b9c0d1e","crusty_first_unreviewed_sha":""}
```

| `crusty_status` | `crusty_reason` |
| --- | --- |
| `DONE_CLEAN` | `""` |
| `ABSENT` | `crusty-loop-not-done` |
| `NOT_CLEAN` | `crusty-not-clean` |
| `UNTRUSTED` | `crusty-manifest-missing`, `crusty-record-missing`, `crusty-record-modified`, `crusty-head-sha-empty` |
| `UNREVIEWED_COMMITS` | `crusty-unreviewed-commits` or `crusty-range-unreadable` |

Any other token from the helpers becomes `crusty-other` with status
`UNTRUSTED`. `crusty_first_unreviewed_sha` is the SHA from
`crusty-unreviewed-commits:<sha>`, and `""` otherwise.

The step-02 prompt treats `DONE_CLEAN` as criterion 3 met, with no round
minimum, and records `crusty_reviewed_head_sha` in its evidence. The prompt
states the range rule: the head being merged may differ from the reviewed
head only by base merges and description or evidence changes, which step-01b
has already checked. When `crusty_status` is `UNREVIEWED_COMMITS`, the blocker
is `crusty-review-required:<crusty_first_unreviewed_sha>`, or
`crusty-range-unreadable` when that SHA is `""`. For the other
statuses that are not `DONE_CLEAN`, the blocker is
`quality-audit-convergence-crusty-not-done-clean` with the `crusty_reason`.
`step-03-extract-merge-ready-verdict` downgrades any `MERGE_READY` verdict
for every status other than `DONE_CLEAN`.

The crusty evidence is read from the state directory rather than from the
composer's `crusty_loop_result`. On a resumed run the crusty loop is skipped
and that result is empty; the state directory persists, so a resumed run
accepts the crusty records the earlier run wrote.

A merge loop run on its own, with no earlier crusty loop in the same state
directory, gets `crusty_status: ABSENT`. Its rounds never reach `MERGE_READY`
and the loop ends `STUCK`. That is intended: criterion 3 has not been met.

#### What the merge gate checks

Section 6b of `autodrive_merge_gate.sh` keeps every earlier check unchanged
and adds these, after the existing `crusty_verdict` check (the range checks
are in section 6d, which runs only after `autodrive_crusty_final` succeeded):

- `crusty-records.tsv` must exist. If it is absent the gate blocks with
  `crusty-manifest-missing`, as it does for a state directory written before
  this change.
- `crusty-records.tsv` must not be a symlink and must be private to this user.
- The file name in its last row must match `^crusty-[A-Za-z0-9._-]+\.json$`
  before any path is built from it. The named record, if present, must be
  private to this user.
- `autodrive_crusty_final` must succeed. On failure the gate blocks and quotes
  the token, for example
  `crusty records in <dir> are not loop-written evidence (crusty-record-modified)`.
- On success the gate notes `crusty_reviewed_head_sha=<sha>`.
- `HEAD_SHA`, the head the gate binds the merge to, must exist in the local
  clone (`git cat-file -e`). If it does not, the gate blocks with
  `head <sha> is not in the local clone, so the commits after the clean crusty round cannot be read; criterion 3 is not met`;
  it never skips the range check.
- The gate reads `baseRefName` in the same `gh pr view --json` call as the
  other pull request fields, resolves the base SHA with `autodrive_base_sha`
  as described in [Where the base SHA comes from](#where-the-base-sha-comes-from),
  and runs `autodrive_crusty_range` from `crusty_reviewed_head_sha` to
  `HEAD_SHA`. Any result other than `ok` blocks:

  | Token | Block message |
  | --- | --- |
  | `crusty-unreviewed-commits:<sha>` | `commit <sha> after the clean crusty round is not a base merge or a description or evidence change; criterion 3 is not met` |
  | `crusty-range-unreadable` | `the commits after the clean crusty round cannot be read; criterion 3 is not met` |
  | anything else | the same message with `crusty-range-other` |

Section 6 also notes `qa_reason`. Section 6c checks the qa evidence chain
described in [The qa evidence is trusted only through its hash](#the-qa-evidence-is-trusted-only-through-its-hash),
whenever `--state-dir` was given:

- `merge-ready-records.tsv`, the record its last row names,
  `merge-ready-latest.json` and `qa-evidence.json` must each be private to
  this user: a regular file, not a symlink, owned by this user, and not group-
  or world-writable.
- At the start of section 6 the gate copies the round record it was given
  with `--round-record` (`merge-ready-latest.json`) and the file given with
  `--qa-evidence` (`qa-evidence.json`) once each into a private `mktemp -d`
  directory, removed on exit. If that directory cannot be created, the gate
  blocks.
- It passes those copies as `RECORD_COPY` and `QA_COPY` to
  `autodrive_qa_trusted STATE_DIR RECORD_COPY QA_COPY HEAD_SHA`, which hashes
  and parses the copies themselves. The files the gate verifies are therefore
  the files it reads afterwards.
- `autodrive_qa_trusted` must print `ok`. Its tokens, `qa-manifest-missing`,
  `qa-record-modified`, `qa-evidence-modified` and `qa-evidence-stale`, each
  block with the token quoted, as
  `the qa evidence is not bound to a loop-written round record for <sha> (<token>); criterion 1 is not met`.
  Any other output blocks as `qa-other`.
- If `autodrive_trust.sh` or `autodrive_state.sh` is missing beside the gate,
  or cannot be loaded, the gate blocks and runs neither the chain nor the
  range check.

Every check in sections 6, 6c and 7 reads the same two copies. Section 7
reads `merge_ready_verdict` and `head_sha` from `RECORD_COPY`, whose hash
`autodrive_qa_trusted` has already checked against the last manifest row, so
nothing can change between the hash check and the read.

All of these checks are added. No earlier block line in the gate is removed
or loosened; the test `merge_gate_keeps_every_existing_block` checks this.
The gate sources `autodrive_state.sh` and `autodrive_trust.sh` from its own
directory only.

### Agents do not touch the state directory

Every agent step in the auto-drive recipes includes this sentence, word for
word:

> Do not create, edit, move, rename, delete or archive any file in the
> auto-drive state directory (the directory holding crusty-round-*.json,
> merge-ready-round-*.json and phases.tsv).

There are five agent steps:

| Recipe | Step |
| --- | --- |
| `autodrive-crusty-round.yaml` | `step-02-crusty-review` |
| `autodrive-crusty-round.yaml` | `step-04-address-concerns` |
| `autodrive-merge-round.yaml` | `step-02-merge-ready-assessment` |
| `autodrive-merge-round.yaml` | `step-04-address-blockers` |
| `loop-health-evaluator.yaml` | `step-02-evaluate-loop-health`, used by both loops |

The other auto-drive recipes, `auto-drive-to-merge.yaml` included, have only
bash and recipe steps. No agent prompt contains the state directory path or
the names `STATE_DIR`, `autodrive_state_dir` or `AUTODRIVE_STATE_DIR`.

The sentence is an instruction, not a control. The manifest checks are what
make an agent-written crusty record fail criterion 3 and an edited
`qa-evidence.json` fail criterion 1.

The manifest hashes detect a change to a record; they do not authenticate the
writer. A party that can write the records can also write matching manifest
rows (see [Trust model](#trust-model)). For that reason the first line of
defence is that agents are never told where the state directory is: no prompt
contains its path or the variables that hold it, so an agent following
instructions from branch text has no directory to write to. The test
`every_autodrive_agent_prompt_forbids_touching_the_state_dir` in
`tests/integration/auto_drive_to_merge_test.rs` fails if any agent prompt in
the auto-drive recipes contains `STATE_DIR`, `autodrive_state_dir` or
`AUTODRIVE_STATE_DIR`. The manifest checks catch what gets past that.

`step-04-address-blockers` of the merge round also tells its agent not to
rewrite history, reset, force-push, or edit refs, and not to try to clear a
`crusty-review-required` blocker.

### Trust model

| Party | Trust |
| --- | --- |
| The operator's environment, including the `AUTODRIVE_QA_*` variables | trusted |
| Branch code and scenarios | untrusted, but run by design. Only run auto-drive on branches whose authors you would let run code on this host. |
| Agent steps and their output | untrusted writers and untrusted data |
| The state directory | integrity-checked through the manifests, not authenticated |
| GitHub | authoritative for platform state |

Agents, the code under test and auto-drive itself all run as the same OS
user. The manifest and hash checks catch an agent following an instruction
in branch text, an accidental edit, and the replay of an older record. **They
are not a boundary against a deliberate forger running as the same user**,
who can rewrite every file in the state directory consistently. The checks
that such a user cannot satisfy locally are on the platform: required CI,
review state, and `gh pr merge --match-head-commit`, which binds the merge to
the head the gate checked.

The base-branch fetch in the range check is the only network call this
change adds. It uses the clone's existing `origin` remote and credentials,
with `GIT_TERMINAL_PROMPT=0`.

### Known limits and accepted residual risks

- **Full forgery.** An agent running as the same user can rewrite a crusty
  record, `crusty-records.tsv` and `phases.tsv` together. The manifest catches
  injected, replaced and archived records, not a consistent rewrite of all
  three.
- **Self-hosted criteria.** In a repository that ships `amplifier-bundle/`,
  the resolver can pick a `SKILL.md` that the pull request under review
  edited. The `skill_md_sha` record, the base-branch warning and the merge
  gate limit the effect.
- **Legacy multi-scenario files.** A file with a `scenarios:` list counts as
  one file and still runs all its scenarios in one process, where they share
  one CLI runner and the first to finish can kill the others (#207), or fails
  with `SCENARIO_NOT_FOUND`.
- **Paths relative to the scenario file.** A scenario is copied to a staging
  directory before it runs, so a scenario that resolves paths relative to its
  own file fails. It fails loudly; it never passes by accident.
- **Names the reader cannot parse.** Block scalars and flow mappings make a
  file unnamed, which fails as `gadugi-scenario-unnamed`.
- **Symlinked scenario files** are validated by gadugi but neither counted nor
  run. They fail the evidence as `gadugi-run-failed`, or are named in the
  `NO_SCENARIOS` cause when there is no regular scenario file, so a failing
  symlinked scenario can never pass by being skipped.
- **Trivial scenarios.** A scenario that always passes satisfies
  `gadugi_status: PASS`. Review scenarios like any other test.
- **Same-user forgery.** See [Trust model](#trust-model). The hash chains
  catch partial edits, not a consistent rewrite of the state directory.
- **Each code commit after the clean round costs a crusty run.** A
  blocker-clearing commit in one merge round sends the range to crusty at the
  start of the next.
- **Cases that fail closed and cost a crusty run**: a failed base fetch
  (offline, or no access), an empty commit, a root commit, an octopus merge,
  a base merge with hand-resolved conflicts, and git older than 2.38.
- **An unreadable range blocks without a re-review.** A shallow clone or a
  git error gives `crusty-range-unreadable`. Crusty is not re-run for it;
  every round blocks until a person deepens the clone or fixes git, or until
  loop-health ends the merge loop.
- **A re-review that ends `STUCK` is terminal.** The `crusty-loop` marker
  stays absent, later rounds report `ABSENT` and do not re-run crusty, and
  loop-health ends the merge loop. A person has to resolve crusty's concerns
  and start a new run.
- **One allowlist for every repository.** A repository whose build, tests or
  scenarios read `PR_DESCRIPTION.md` or a file under `.autodrive/evidence/`
  would let changes to that file skip crusty.
- **Re-run labels restart at `round-1`** and overwrite the earlier crusty
  round files. Only the last manifest row is trusted, so this loses history
  in the state directory, not evidence.
- **The evidence step runs the pull request's code**, as `cargo test` already
  did. Suite commands and gadugi scenarios run as the operator's OS user, with
  the operator's environment, credentials and network access. There is no
  sandbox, container or separate user. Run auto-drive only on branches whose
  code you would run by hand on the same host.
- **`git` must be on `PATH`** for the loop and the merge gate. The manifest
  row, `autodrive_crusty_final` and the gate all hash records with `git
  hash-object`. Without `git` they fail closed: the loop adds no manifest row,
  and the check and the gate report `crusty-manifest-missing` or
  `crusty-record-modified`, never success.

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
| qa-team scenarios | evidence file from this run, trusted through `merge-ready-records.tsv` | `qa_status` other than `PASS`, no evidence file, or evidence whose `head_sha` is missing or is not the SHA being merged; `autodrive_qa_trusted` failing with any token (see [The qa evidence is trusted only through its hash](#the-qa-evidence-is-trusted-only-through-its-hash)) |
| gadugi scenarios | same evidence file | `gadugi_status` other than `PASS`, or `gadugi_scenario_count` missing or not a positive integer; `qa_reason` is noted |
| Crusty loop | `phases.tsv`, `crusty-latest.json`, `crusty-records.tsv` and the record it names, in `--state-dir` | no `--state-dir` or an empty one; the directory not owned by the current user; the directory or any of these files with the group-write bit or the world-write bit set (either bit alone blocks); any of these files a symlink or not a regular file; `autodrive_state.sh` or `autodrive_trust.sh` missing beside the gate; no `crusty-loop` marker; `crusty_verdict` other than `CLEAN`; `autodrive_crusty_final` failing with any token; `HEAD_SHA` missing from the clone; `autodrive_crusty_range` printing anything other than `ok` (see [What the merge gate checks](#what-the-merge-gate-checks)) |
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
or when the directory, `phases.tsv`, `crusty-latest.json`,
`crusty-records.tsv` or the record it names has the group-write bit or the world-write bit set. Either bit alone blocks: `0770`,
`0702` and `0777` all fail. A failure is reported as
`not private to this user`. The gate sources `autodrive_state.sh` and
`autodrive_trust.sh` from its own directory only, never from the merge-ready
search roots.

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
The exception is the crusty state: the `crusty-loop` marker, `crusty-latest.json`,
`crusty-records.tsv` and the round records it lists. Nothing on the platform
records crusty's judgement, so the gate reads them as criterion-3 evidence (see
[Criterion 3](#criterion-3-the-crusty-loop-ended-done-and-clean)). Only
`autodrive-crusty-loop.yaml` (the marker), `autodrive-crusty-round.yaml` (the
round records) and `autodrive_loop.sh` (`crusty-latest.json` and the manifest)
write them. The gate accepts them only from a directory private to the current
user, and only when the manifest hash matches the record. The qa evidence is
re-measured each merge round, and the gate accepts it only through
`merge-ready-records.tsv` (see
[The qa evidence is trusted only through its hash](#the-qa-evidence-is-trusted-only-through-its-hash)).

The `crusty-loop` marker is the one phase row a later step removes. Step
`step-00b-crusty-range` removes it when commits after the clean round need
review, and the re-run crusty loop writes it again when it ends clean. A run
that dies between the two resumes with the marker absent, so the crusty loop
runs again; it is never skipped.

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
| `amplifier-bundle/recipes/autodrive-crusty-round.yaml` | round | Crusty review, verdict, fixes, round record with `reviewed_head_sha`. |
| `amplifier-bundle/recipes/autodrive-crusty-loop.yaml` | phase 2 | Loop driver + phase bookkeeping. |
| `amplifier-bundle/recipes/autodrive-merge-evidence.yaml` | evidence | Base sync, repository suite commands plus `gadugi-test validate` and one `gadugi-test run` per scenario, CI wait. |
| `amplifier-bundle/recipes/autodrive-merge-round.yaml` | round | Merge-ready file resolution, the crusty range check and re-review, the qa evidence hash, crusty evidence, merge-ready criteria read from the skill's files, verdict, blocker fixes. |
| `amplifier-bundle/recipes/autodrive-merge-loop.yaml` | phase 3 | Loop driver (passes the state dir to rounds) + merge gate + bookkeeping. |
| `amplifier-bundle/recipes/loop-health-evaluator.yaml` | terminator | Agentic loop-health verdict; its prompt carries the state-directory sentence. |
| `amplifier-bundle/tools/autodrive_loop.sh` | tool | The uncapped, agentically-terminated loop driver; writes `<loop>-records.tsv`. |
| `amplifier-bundle/tools/autodrive_merge_gate.sh` | tool | Evidence gate, including the gadugi, qa evidence hash, crusty and range checks, and the fixed merge argv. |
| `amplifier-bundle/tools/autodrive_merge_ready_files.sh` | tool | Finds the merge-ready `SKILL.md` and template; read-only. |
| `amplifier-bundle/tools/autodrive_state.sh` | tool | Resumable local state, the crusty-loop marker, `autodrive_clear_phase`, `autodrive_crusty_final`, platform truth for merged-ness. |
| `amplifier-bundle/tools/autodrive_trust.sh` | tool | Checks on commits and evidence written after the clean round: `AUTODRIVE_RANGE_ALLOWLIST`, `autodrive_base_sha`, `autodrive_range_allowed`, `autodrive_crusty_range`, `autodrive_rereview_decision`, `autodrive_qa_evidence_sha`, `autodrive_qa_trusted`, `autodrive_scenario_results`. |
| `amplifier-bundle/skills/auto-drive-to-merge/SKILL.md` | skill | Invocable entry point. |
| `amplifier-bundle/skills/merge-ready/SKILL.md` | skill | Criteria the merge round reads as a file; its `Running under auto-drive` section. |

Every recipe and tool in this table stays inside the 400-line brick budget.
The budget test covers each recipe in `AUTODRIVE_RECIPES` and each tool in
`AUTODRIVE_TOOLS`; `autodrive_trust.sh` is added to `AUTODRIVE_TOOLS`. The
range and qa evidence functions are a tool of their own, not part of
`autodrive_state.sh`, so that both files fit the budget. Each new recipe step
calls one of these functions and stays a few lines long, which keeps
`autodrive-merge-round.yaml` and `autodrive-merge-evidence.yaml` within the
budget, and the gate's new checks in sections 6c and 6d are calls to the same
functions.

## Tests

| Test | Location |
| --- | --- |
| Executable contract test: STUCK path, malformed-verdict path, forbidden-flag guard, merge-gate refusals including the gadugi and crusty blocks, the qa evidence step against stub `cargo` and `gadugi-test`, the merge-ready file resolver, `autodrive_crusty_final`, the loop manifest, crusty step-06, and `step-01b-crusty-evidence` | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |
| Structural and wiring guards, including the 400-line budget for every recipe and tool, recipes calling skills that refuse agents (escaped-quote and `*.yml` fixtures included), the state-directory sentence in the five agent steps, and the absence of a crusty round minimum (`no_round_minimum_in_any_recipe_or_tool`, which scans `*.yaml`, `*.yml` and the tools) | `tests/integration/auto_drive_to_merge_test.rs` |
| The merge-ready skill stays platform-neutral | `tests/integration/merge_ready_platform_contract_test.rs` |

```bash
AMPLIHACK_SKIP_AUTO_INSTALL=1 cargo test -p amplihack --test auto_drive_to_merge
AMPLIHACK_SKIP_AUTO_INSTALL=1 cargo test -p amplihack --test merge_ready_platform_contract
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
```

The shell tests run each recipe step with `HOME` set to a temporary directory.
The `gadugi-test` stub logs its arguments, so the tests can assert exactly one
`run` per scenario file, each with `--scenario`, and none with only `-d`.

| Area | Cases |
| --- | --- |
| qa evidence | no scenarios; validate failing; one of two scenario runs failing; an unnamed scenario; all passing, with names in both supported formats; `gadugi-test` missing; `AUTODRIVE_QA_COMMAND` with `AUTODRIVE_QA_DIR`; several `AUTODRIVE_QA_COMMANDS` with one failing; both set; a repository `logs/` directory left in place; hostile output that must still parse as JSON; a symlinked scenario next to a passing one failing as `gadugi-run-failed`; a failed `mktemp` named on stderr with no evidence written |
| qa evidence, hostile input | a scenario named `-d /` counted as unnamed; a `logs` symlink left in place; `echo *` recorded literally with a file named `--evil` present; `cd sub && false` followed by `pwd` running from the repository root |
| merge-ready files | each of the five directories; `AMPLIHACK_HOME` winning over the others; `merge-ready-template-not-found`; `merge-ready-skill-files-not-found`; a candidate path containing `"` or `{{` skipped; a `REPO_PATH` that does not exist failing step-00 with `ERROR: cannot cd to REPO_PATH` |
| per-scenario results | `PASS`, `FAIL` and `INVALID` entries in sorted order; a symlink and a non-regular file both `INVALID` and both counted in `gadugi_scenarios_failed`; every entry `INVALID` after a validate failure; a hostile file name sanitised |
| crusty range | a base merge only gives `ok`; a description or evidence change only gives `ok`; a code commit gives `crusty-unreviewed-commits:<sha>`; a hand-resolved merge, an empty commit, a symlink or submodule mode, a `..` path under the prefix, a shallow clone, a replace ref, and an empty base SHA |
| crusty re-review | step-00b clears the `crusty-loop` row and emits `rereview: "true"` for a code commit; step-00b leaves the row and emits `rereview: "false"` for `crusty-range-unreadable`; step-01b gives `UNREVIEWED_COMMITS`; step-03 downgrades it; a re-review ending `STUCK` leaves the marker absent and the next round's step-00b emits `rereview: "false"`; `autodrive_clear_phase` refuses a symlinked `phases.tsv` and an invalid phase name |
| qa evidence hash | an edited `qa-evidence.json` gives `qa-evidence-modified`; a stale `head_sha` gives `qa-evidence-stale`; an edited `merge-ready-latest.json` gives `qa-record-modified`; a replayed older manifest row; a record replaced after the gate copied it does not change what section 7 reads; step-03 downgrades an empty or changed hash |
| crusty evidence | DONE and CLEAN in round 1 gives `DONE_CLEAN`; an injected record and an archived record are both rejected; a manifest row naming `../phases.tsv`; a duplicate `reviewed_head_sha` key; a two-line record; a CLEAN round with no commits records a non-empty `head_sha`; a missing SHA fails step-06; a non-numeric count recorded as 0 with a sanitised `WARNING`; a failed `mktemp` in `autodrive_crusty_final` keeping its token |
| merge gate | the new 6b and 6d refusals, including a group-writable record, a code commit after the clean round, and a `HEAD_SHA` missing from the clone; the section 6c qa evidence hash refusals |
| recipe text | step-02 reads both files and contains no `Skill(`; step-02 no longer says the reviewed SHA `is not compared with the current head`; step-00c is a `type: recipe` step naming `autodrive-crusty-loop` with the condition on `crusty_range.rereview`; `AUTODRIVE_RANGE_ALLOWLIST` is defined only in `autodrive_trust.sh`; the state-directory sentence is in every agent prompt |

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
