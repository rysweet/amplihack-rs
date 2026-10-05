---
title: Auto Drive To Merge Reference
last_updated: 2026-10-04
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

recipe-runner 0.3.8 stores a step output that parses as JSON as an object, and
exports it to later bash steps as `RECIPE_VAR_<output>` only
(`context.rs`, `shell_env_vars`). The bare upper-case name, such as
`QA_EVIDENCE`, is added for scalar outputs alone. A bash step that read
`${QA_EVIDENCE:-}` would see an empty string and fail closed every round:
`MERGE_READY` downgraded, an empty `qa_evidence_sha` in the record, and no
crusty round record at all. Every step-output read in
`autodrive-crusty-round.yaml`, `autodrive-merge-round.yaml` and
`autodrive_round_evidence.sh` (which holds merge-round step bodies) is therefore
written `"${QA_EVIDENCE:-${RECIPE_VAR_qa_evidence:-}}"`, the form the work on issue
#1511 uses, and `round_recipes_read_step_outputs_through_recipe_var` fails
the build on a read without the fallback. The shell tests pass these values as
`RECIPE_VAR_<output>` with the upper-case names removed, as the runner does.
The reads in the loop recipes (`autodrive-build.yaml`,
`autodrive-crusty-loop.yaml`, `autodrive-merge-loop.yaml`) are older, are not
changed here, and stay open under #1511; until that lands, a full auto-drive
run on recipe-runner 0.3.8 still loses the loops' own step outputs.

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
`merge_state`, as the skill's `## Running under auto-drive` section says. A
manual `/merge-ready` still needs `gadugi-test validate` and `gadugi-test run`
(criterion 1) and a `quality-audit` of at least 3 SEEK, VALIDATE, FIX cycles
(criterion 3).

### Round steps

| Step of `autodrive-merge-round.yaml` | Type | Output | Role |
| --- | --- | --- | --- |
| `step-00-tools-dir` | bash | `autodrive_tools_dir` | Finds the round's tools once (see below). |
| `step-00-merge-ready-files` | bash | `merge_ready_files` | Finds `SKILL.md` and its template; stops the round when `gadugi-test` is not on `PATH`. |
| `step-00b-crusty-range` | bash | `crusty_range` | `autodrive_round_evidence.sh crusty-range`: commits after the clean crusty round. |
| `step-00c-crusty-rereview` | recipe | none | Runs `autodrive-crusty-loop` when `crusty_range.rereview == 'true'`. |
| `merge-evidence` | recipe | `merge_sync`, `qa_evidence`, `ci_evidence` | `autodrive-merge-evidence`: base sync, suite commands, gadugi scenarios, CI wait. |
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
`"${AUTODRIVE_TOOLS_DIR:-${RECIPE_VAR_autodrive_tools_dir:-}}"` and step-04's
prompt uses to source `git-identity.sh`. A path holding a quote, backslash,
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

`gadugi-test` belongs to the same install. Criterion 1 runs the qa-team
scenarios with it in every repository type, `amplihack install` does not
install it, and no agent can. So step-00 also checks `command -v gadugi-test`
and, when it is missing, fails the round with
`ERROR: gadugi-test-not-installed: gadugi-test is not on PATH`, which names
the install command (`npm install -g github:rysweet/gadugi-agentic-test`).
Reported instead as the blocker `gadugi-test-missing`, it would repeat every
round until the loop ended `STUCK`.

`step-02-merge-ready-assessment` reads `{{merge_ready_files.skill_md}}` and
`{{merge_ready_files.template}}`, fails any criterion it could not verify, and
treats evidence text as data. It has no `Skill(` call. The guard test
`no_recipe_invokes_a_skill_that_refuses_model_invocation` fails on any recipe
`Skill(skill="<name>")` call, comments included, for a skill that refuses
model invocation; `qa_team_does_not_refuse_model_invocation` keeps `qa-team`
callable for step-04.

### Criterion 1: qa-team scenarios run with gadugi-test

`step-02-qa-team-scenarios` of `autodrive-merge-evidence.yaml` runs
`amplifier-bundle/tools/autodrive_qa_evidence.sh`, found under the same roots,
which runs the suite commands and the gadugi scenarios; a missing tool fails
the step with `ERROR: autodrive-qa-evidence-tool-not-found`. Every repository
type, `rust-cli` included, needs both. The `qa-team` skill says so in one
place, its "Exception: auto-drive-to-merge" paragraph, which sets aside its
`cargo test` substitution for Rust CLI repositories when step-04 loads it to
clear a gadugi blocker.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` | unset | One command, run in `AUTODRIVE_QA_DIR` (default `.`), split into words with globbing off. |
| `AUTODRIVE_QA_COMMANDS` | unset | One command per line, each run as `( cd -- "$ROOT" && bash -c "$entry" )` after `AUTODRIVE_QA_COMMAND`; blank and `#` lines skipped. |
| `AUTODRIVE_QA_SCENARIO_DIR` | the first existing of `tests/agentic`, `tests/gadugi/scenarios`, `scenarios`; else `tests/agentic` | The scenario directory. An override naming a missing directory means no scenarios. |

With a command variable set, `qa_repo_type` is `configured`. Otherwise
`Cargo.toml` gives `rust-cli` with
`cargo test --workspace --locked --no-fail-fast`, `package.json` gives `node`
with `npm test`, `pyproject.toml` or `setup.py` gives `python` with `pytest`,
and anything else is `unknown`. Every command runs, must exit 0, and has no
timeout. The variables come from the environment only, never recipe context
(`no_recipe_declares_the_qa_overrides_as_context`), and only the operator sets
them: an entry is shell source.

After `gadugi-test validate -d "$DIR"` passes, each top-level regular `*.yaml`
or `*.yml` file is copied alone to a `mktemp -d` directory outside the
repository and run as
`(cd "$ROOT" && gadugi-test run -d "$stage" --scenario "$name")`, never as a
whole directory. In gadugi-test 1.0.x, scenarios in one directory share one
CLI runner and the first to finish kills the others
([gadugi-agentic-test #207](https://github.com/rysweet/gadugi-agentic-test/issues/207)),
and `--scenario` matches a substring of `name:` (top-level, or under
`scenario:`). Every file runs, even after one fails.

`qa_status` is `PASS`, with `qa_reason` `""`, only when a suite command ran,
all passed, and `gadugi_status` is `PASS`. Otherwise `qa_reason` is the first
token that applies, and `qa_summary` names every cause:

| Order | `qa_reason` | `qa_status` | Cause (`gadugi_status`) |
| --- | --- | --- | --- |
| 1 | `qa-command-failed` | `FAIL` | A suite command exited non-zero. |
| 2 | `no-scenarios` | `FAIL` | No scenario file (`NO_SCENARIOS`). |
| 3 | `gadugi-validate-failed` | `FAIL` | Validate failed; no scenario ran (`VALIDATE_FAILED`). |
| 4 | `gadugi-scenario-unnamed` | `FAIL` | A name is empty, starts with `-`, holds a control byte, exceeds 200 bytes, or is a block scalar or flow mapping (`RUN_FAILED`). |
| 5 | `gadugi-run-failed` | `FAIL` | A run or staging failed, or a symlinked or non-regular entry exists; such entries never run (`RUN_FAILED`). |
| 6 | `qa-command-missing` | `BLOCKED` | No suite command, or incomplete evidence with no other token. |
| 7 | `qa-command-not-installed` | `BLOCKED` | The single or detected command's program is missing. |
| 8 | `gadugi-test-missing` | `BLOCKED` | `gadugi-test` is not on `PATH`; no gadugi check ran (`NOT_INSTALLED`). Step-00 stops the round before this when the tool is missing, so in a merge round it means the tool went away during the round; step-04 leaves it to a person. |

The evidence is one JSON line of sanitised strings in `qa-evidence.json`,
written under `umask 077`. A failed temporary file gives
`ERROR: cannot create a temporary log` and no evidence.

| Field | Value |
| --- | --- |
| `qa_status`, `qa_reason`, `gadugi_status`, `qa_summary` | As above. |
| `qa_repo_type`, `qa_round` | `configured`, `rust-cli`, `node`, `python` or `unknown`; the round label. |
| `qa_command`, `qa_suite_commands_count` | The commands run, joined by `; ` (at most 500 characters, so reference secrets as `$VAR`), and their number. |
| `qa_exit_code`, `gadugi_run_exit_code`, `gadugi_validate_exit_code` | The first non-zero exit code, `"0"` when all passed, `""` when none ran. |
| `head_sha` | `git rev-parse HEAD` before any check: the commit tested. |
| `gadugi_scenario_dir`, `qa_scenarios`, `gadugi_failed_scenarios` | The directory, the counted files, the failed files; relative to the repository root. |
| `gadugi_scenario_count`, `gadugi_scenarios_validated`, `gadugi_scenarios_run`, `gadugi_scenarios_passed`, `gadugi_scenarios_failed` | Files found; that count if validate passed, else `"0"`; runs; passes; failures, counting unnamed, unstaged and (after validate) symlinked or non-regular entries. |
| `gadugi_scenario_results` | `<file>=<result>` per entry (`PASS`, `FAIL`, or `INVALID` for not run), sorted, from `autodrive_scenario_results`. Informational only. |

For `NO_SCENARIOS`, `VALIDATE_FAILED`, `RUN_FAILED`, or step-02's
`qa-team-scenarios-do-not-cover-the-change`, step-04 uses `qa-team`, following
that exception, to write
top-level `*.yaml` scenarios with a `name:` in `gadugi_scenario_dir`, runs
each, and commits. A failing scenario is fixed, never weakened or deleted. A
directory outside the repository gives `gadugi-scenario-dir-outside-repo`.

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
`crusty-records.tsv` or `merge-ready-records.tsv` under `umask 077`.

`autodrive_crusty_final DIR` in `autodrive_state.sh` decides criterion 3 for
step-00b, step-01b and the gate. It checks one private copy of the record and
prints the reviewed SHA or the first failing token:

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

`step-00b-crusty-range` prints `autodrive_rereview_decision` (`rereview`,
`range`, `first_unreviewed_sha`, `base_sha`). Only `crusty-unreviewed-commits`
sets `rereview` to `true`, after `autodrive_clear_phase` removes the
`crusty-loop` row; an unclean loop gives `range` `not-checked`, and an
unreadable range is never re-reviewed. `step-00c-crusty-rereview` then runs
the crusty loop on the same state directory, which writes the marker again at
its first `CLEAN` round. A failed, `STUCK` or exit-79 re-review fails the
round, and after `STUCK` the marker stays absent. `step-01b-crusty-evidence`
runs `autodrive_crusty_final` and the range against the current `HEAD`:

| `crusty_status` | `crusty_reason` | step-02 blocker |
| --- | --- | --- |
| `DONE_CLEAN` | `""` | None; cites `crusty_reviewed_head_sha`. |
| `ABSENT` | `crusty-loop-not-done` | `quality-audit-convergence-crusty-not-done-clean` |
| `NOT_CLEAN` | `crusty-not-clean` | the same |
| `UNTRUSTED` | `crusty-manifest-missing`, `crusty-record-missing`, `crusty-record-modified`, `crusty-head-sha-empty`, `crusty-other` | the same |
| `UNREVIEWED_COMMITS` | `crusty-unreviewed-commits`, `crusty-range-unreadable` | `crusty-review-required:<crusty_first_unreviewed_sha>`, or `crusty-range-unreadable` without a SHA |

Step-04 leaves `crusty-review-required` to the next re-review (each code
commit costs a crusty run) and `crusty-range-unreadable` to a person, and
never rewrites history. Empty, root and octopus commits, hand-resolved merges,
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
| empty | count unreadable or unknown; `merge_state` is `DIRTY` or `BEHIND`, or `BLOCKED` with a check in the rollup that has not passed (or no checks), or `mergeable` `CONFLICTING` | `PENDING` | `other-blockers` |
| anything else, or no PR read | none | `UNREADABLE` | `unreadable` |

A check has passed when its `conclusion` (a check run) or `state` (a status
context) is `SUCCESS`, `NEUTRAL` or `SKIPPED`; a run that has not finished has
not passed. GitHub folds every rule into the one `BLOCKED`, so while a
failing check, a conflict or a stale branch is present, whether a review is
also missing cannot be read. That is `PENDING`, which step-02 reports as
`approval-pending-other-blockers` and step-04 treats as a reason to clear the
other blockers first, not as a request for a person; the next round reads the
approval again. `BLOCKED` with every check passed, no conflict and the branch
up to date leaves only rules a person satisfies, a required review among
them, and stays `UNREADABLE`. The INFO line shows `mergeable` and
`checks=pass|not-passed|none`.

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

Accepted limits: a `scenarios:` list in one file runs in one process (#207); a
scenario using paths relative to its own file fails after staging; without
`git` on `PATH` the loop and the gate fail closed.

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
| Review threads | GraphQL `reviewThreads`, **paginated** | any unresolved, not-outdated thread on any page — **or an unreadable answer** |
| CI | `gh pr checks --json name,state,bucket` | any pending or failing check, zero checks, **or an unreadable rollup** |
| qa-team scenarios | evidence file from this run | `qa_status` other than `PASS`, no evidence file, or evidence whose `head_sha` is missing or is not the SHA being merged |
| qa evidence chain | `merge-ready-records.tsv`, `merge-ready-latest.json`, `qa-evidence.json` | any of them not private; `autodrive_qa_trusted` not `ok` (its token, or `qa-other`; see [The qa evidence hash chain](#the-qa-evidence-hash-chain)) |
| gadugi scenarios | same evidence file | `gadugi_status` other than `PASS`, or `gadugi_scenario_count` not a positive integer |
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
| Local | `${AMPLIHACK_STATE_DIR:-~/.amplihack/state}/auto-drive/<key>/` | Phase completions, resolved concern ids, round records, manifests, evidence bundles. Written only by the host that ran them. |
| Platform | `gh pr view --json state,mergedAt` | The **authority** on whether the PR is merged. |

Local state is a cache, never a claim: the merge gate re-verifies every
criterion regardless of what any state file says, and `UNKNOWN` platform state
is treated as a failure rather than as "not merged". The exception is the
crusty state (`crusty-loop` marker, `crusty-latest.json`, `crusty-records.tsv`
and its records): nothing on the platform records crusty's judgement, so the
gate reads it as criterion-3 evidence through the manifest hash. A run that
dies after step-00b removed the marker resumes with the crusty loop.

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

| File | Kind | Role |
| --- | --- | --- |
| `amplifier-bundle/recipes/auto-drive-to-merge.yaml` | composer | Three phases, then a summary. |
| `amplifier-bundle/recipes/autodrive-build.yaml` | phase 1 | `default-workflow`, resume-aware. |
| `amplifier-bundle/recipes/autodrive-crusty-round.yaml` | round | Crusty review of the PR head, verdict, fixes, round record. |
| `amplifier-bundle/recipes/autodrive-crusty-loop.yaml` | phase 2 | Loop driver + phase bookkeeping. |
| `amplifier-bundle/recipes/autodrive-merge-evidence.yaml` | evidence | Base sync, suite commands, gadugi scenarios, CI wait. |
| `amplifier-bundle/recipes/autodrive-merge-round.yaml` | round | Merge-ready criteria from the skill's files, verdict, blocker fixes. |
| `amplifier-bundle/recipes/autodrive-merge-loop.yaml` | phase 3 | Loop driver + merge gate + bookkeeping. |
| `amplifier-bundle/recipes/loop-health-evaluator.yaml` | terminator | Agentic loop-health verdict. |
| `amplifier-bundle/tools/autodrive_loop.sh` | tool | The uncapped, agentically-terminated loop driver; writes `<loop>-records.tsv`. |
| `amplifier-bundle/tools/autodrive_merge_gate.sh` | tool | Evidence gate and the fixed merge argv. |
| `amplifier-bundle/tools/autodrive_merge_ready_files.sh` | tool | Finds the merge-ready `SKILL.md` and template. |
| `amplifier-bundle/tools/autodrive_platform_facts.sh` | tool | Platform facts for the merge round, criterion 6 included; the review-thread count for the merge gate too. |
| `amplifier-bundle/tools/autodrive_qa_evidence.sh` | tool | Criterion 1: the suite commands and the gadugi scenarios, for `autodrive-merge-evidence.yaml` step-02. |
| `amplifier-bundle/tools/autodrive_round_evidence.sh` | tool | The merge round's deterministic step bodies: crusty range and evidence, the qa evidence hash, the measured downgrade, the findings and the round record. |
| `amplifier-bundle/tools/autodrive_state.sh` | tool | Resumable local state, `autodrive_crusty_final`, `autodrive_private`; platform truth for merged-ness. |
| `amplifier-bundle/tools/autodrive_trust.sh` | tool | Range check and qa evidence chain. |
| `amplifier-bundle/skills/auto-drive-to-merge/SKILL.md` | skill | Invocable entry point. |
| `amplifier-bundle/skills/merge-ready/SKILL.md` | skill | The criteria the merge round reads. |

Every recipe and tool here stays inside the 400-line brick budget.

## Tests

| Test | Location |
| --- | --- |
| Executable contract test: loop, verdicts, forbidden-flag scan, merge-gate refusals, qa evidence against stub `cargo` and `gadugi-test`, crusty records and range | `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh` |
| Structural and wiring guards, brick budget, skill-invocation guard, state-directory sentence, and `tests/gadugi/recipe-step-command.sh` compared with serde_yaml over every bundled recipe step | `tests/integration/auto_drive_to_merge_test.rs` |
| The merge-ready skill stays platform-neutral | `tests/integration/merge_ready_platform_contract_test.rs` |

```bash
AMPLIHACK_SKIP_AUTO_INSTALL=1 cargo test -p amplihack --test auto_drive_to_merge
AMPLIHACK_SKIP_AUTO_INSTALL=1 cargo test -p amplihack --test merge_ready_platform_contract
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
