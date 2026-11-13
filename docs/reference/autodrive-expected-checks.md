---
title: Expected Checks at the Head
description: how auto-drive decides CI green from every expected check at the head SHA (#2186), autodrive_expected_checks.sh, and its limits.
last_updated: 2026-11-12
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Expected checks at the head (#2186)

**CI green means every expected check exists at the head SHA and passed.**
A check that should have run but was never created is named and blocks the
merge. A set of expected checks that cannot be read also blocks.

Merge readiness used to read `gh pr checks`, which lists only the checks that exist.
If a job was never created, for example because a workflow failed to start,
nothing reported it, and "every listed check passed" read as green.
`autodrive_expected_checks.sh` starts from what *should* exist and then looks
for it at one SHA. It counts defined jobs, not matrix legs: a dropped leg is
caught only when the base branch requires it (see
[Limits](#limits-of-the-expected-set)).

## Usage

```bash
/bin/bash -p amplifier-bundle/tools/autodrive_expected_checks.sh <HEAD_SHA> <BASE_REF>
```

Run it from inside a clone of the repository, so `gh` picks the repository
from the clone (or from `GH_REPO`). It is run, never sourced.

| Argument | Rule |
| --- | --- |
| `HEAD_SHA` | Exactly 40 characters of `[0-9a-f]`. |
| `BASE_REF` | Only `A-Z a-z 0-9 . _ / -`; no leading `-` or `/`; no `..` or `//`; no trailing `/` or `.lock`. |

Both are checked before any `gh` call. A bad argument prints a usage line on
stderr and exits `2`.

The helper is read-only. It makes `gh api` GET requests only, writes nothing
outside its own `mktemp -d` directory, and never sleeps, loops to wait or
retries. It runs on Bash 3.2, POSIX awk and jq 1.6 or later.

## The expected set

The expected set is the union of two sources.

**1. Required checks.** The status-check contexts that the base branch
requires:

- `GET repos/{owner}/{repo}/rules/branches/<BASE_REF>`: every
  `required_status_checks` rule, by `context`.
- `GET repos/{owner}/{repo}/branches/<BASE_REF>`: the
  `protection.required_status_checks.contexts` list and the `context` of each
  `protection.required_status_checks.checks` entry.

Neither empty answer is unreadable. `rules/branches/<base>` returns `[]` when no
ruleset applies, and an unprotected branch returns `"protected": false` with no
`protection` object. Both mean no required checks from that source.

The helper never calls `branches/<base>/protection/required_status_checks`. A
token without admin rights gets a 404 there, and a 404 would look like "no
required checks".

**2. Defined jobs.** Every job of every workflow that should run for this pull
request at the head. A workflow is included when all of these hold:

| Rule | Detail |
| --- | --- |
| The file exists at the head | A `.github/workflows/*.yml` or `*.yaml` file listed by `contents/.github/workflows?ref=<HEAD_SHA>`. The path must match `^\.github/workflows/[A-Za-z0-9._-]+\.(yml|yaml)$`. Any other path makes the set unreadable. |
| It is active | `actions/workflows` reports `state: active` for the same `path`. Disabled workflows never run. |
| It runs on `pull_request` | The top-level `on:` (also `"on":`, `'on':` and `true:`) names `pull_request`, in scalar, list or mapping form. `pull_request_target` does **not** count: its runs belong to the base SHA, not the head. |
| Its branch filter matches | `BASE_REF` is matched against each `branches:` and `branches-ignore:` pattern with GitHub's [filter pattern](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#filter-pattern-cheat-sheet) rules: `*` matches any characters except `/`, `**` matches any characters, `?` and `+` apply to the character before them, and `[...]` is a character class. In `branches:` the last pattern that matches decides, so `['**', '!main']` excludes `main` and `['!main', '**']` includes it. A `branches-ignore:` pattern that matches excludes the workflow. |

Branch filters depend only on the base name, so the helper evaluates them
exactly. Three kinds of filter can stop a workflow from running at a head for
reasons the base name does not settle. A workflow with any of them is expected **only if it
ran** at the head. Without a run it gets an `evidence:` note naming the
filter, not a block:

| Filter | Note | Why it is not evaluated |
| --- | --- | --- |
| `paths:` or `paths-ignore:` | `paths filter` | The helper does not match path globs against the pull request's files. |
| `types:` without all of `opened`, `synchronize` and `reopened` | `types filter` | A head gets a run only through one of those three activity types, so a list such as `[labeled]` may never run at a given head. `types:` that names all three is evaluated like no `types:`. |
| A branch pattern jq cannot compile (for example `+main`, with nothing before the `+`), or a `!` pattern in `branches-ignore:` | `branch pattern` | The result is unknown, so the helper does not guess. |

Before this rule, such a workflow could block a merge for ever. For example, a
`types: [labeled]` workflow on a pull request that was never labelled would
stay `MISSING`. The cost is that a workflow with one of these filters that
should have run, but failed to start, is not caught; required checks are the
defence (see [Limits](#limits-of-the-expected-set)).

For each included workflow, the helper reads the top-level `jobs:` keys and
each job's own `name:`. A defined job that is absent from the workflow's
**completed** run blocks as `MISSING`, whatever the run concluded
(`startup_failure`, `failure`, `cancelled` or `success`), and whether or not
the job has a job-level `if:`. GitHub reports a job that its `if:` skipped as a
`skipped` job, and [`skipped` passes](#which-conclusions-pass) for a defined
job, so a job whose condition was false is never absent. A job absent from a
run that is still going is `PENDING`: the run can still create it.

Earlier versions of this helper passed a job with a job-level `if:` that was
absent from a completed run. The only time that was observed was `docs.yml`'s
`deploy` on pull request #2152. That run was created at 18:24Z on 9 November
2026, inside the GitHub Actions incident that #2186 describes. On later heads,
GitHub created `deploy` as `skipped`. The exception also passed a run that
concluded `startup_failure` with no jobs, which is #2186's own failure mode,
so it was removed. The evidence line for each run gives its status and
conclusion (`run 1008 is completed/startup_failure`), so the reason for a
missing job is in the evidence bundle.

The run that decides for a workflow is chosen from its runs at the head, the
ones from `actions/runs?head_sha=<HEAD_SHA>&event=pull_request` whose `path`
matches: it is the run with the highest `id` **that holds evidence**. A run
that concluded `cancelled` with zero jobs holds none and is passed over (see
below). Its jobs come from `actions/runs/<id>/jobs`, latest attempt. Run ids
must be all digits.

The highest `id` is only the order in which GitHub created the runs. It is not
the order in which a concurrency group admitted, ran or cancelled them, so
"highest id" alone does not mean "the run that counts".

**A run cancelled before it created a job does not decide.** Run ids do not
follow the order in which a concurrency group handles runs. On tokio-rs/tokio
pull request #8771, `loom.yml` (`types: [labeled, opened, synchronize,
reopened]`, `cancel-in-progress: true`) has four runs created between
12:38:07Z and 12:38:09Z. Runs 42618653969, 42618656066 and 42618656471
completed as `skipped` with six jobs each. Run 42618656717 has the highest id,
and the concurrency group cancelled it before it created any job. A run that
concluded `cancelled` with zero jobs holds no evidence, so the helper reads
the next run down instead, and records it:

```text
evidence: workflow .github/workflows/loom.yml: run 42618656717 was cancelled before it created any job; run 42618656471 decides
```

The helper skips only a run with **both** conclusion `cancelled` **and** zero
jobs. A cancelled run that created jobs still decides, so its cancelled jobs
block as `RED`. A `startup_failure` run with zero jobs still decides, so its
jobs block as `MISSING`: that is #2186's own failure mode. The next run down
decides even when it is still going, so the result is then `PENDING`. If
every run of the workflow at the head was cancelled before it created a job,
the latest one decides, and the helper prints one blocker for it instead of a
"never created" line for each job:

```text
blocker: expected workflow '.github/workflows/loom.yml' run 42618656717 was cancelled before it created any job at <sha>; re-run it
```

`missing` then names the workflow path.

The other runs of the same workflow at the same head are superseded. `ci.yml`
cancels a running build when a new one starts for the same ref, so closing and
reopening a pull request, or pushing the same SHA again, leaves a cancelled
earlier run at the head. The check-runs in the check suite of every run other
than the one that decides (`check_suite_id` from `actions/runs`) are dropped
before the required checks and the other checks at the head are evaluated.
The deciding run's own check-runs always count, so if that run is cancelled or
failing, it still blocks.

## Matching a defined job to a reported job

The expected display name is the job's `name:`, or its key when it has no
`name:`. The scanner reads a `name:` written in any of these forms:

| Form | Example | Read as |
| --- | --- | --- |
| Plain or quoted, one line | `name: Test`, `name: 'Ubuntu (installed)'` | the value |
| Plain or quoted, continued on deeper lines | `name: Build` then `  all` on the next line | the lines joined by one space: `Build all` |
| Folded block scalar (`>`, `>-`, `>+`, `>2-`) | cpython's `name: >-` then `Windows` and `${{ … }}` on two lines | the lines joined by one space |
| Literal block scalar (`\|`) of one line | `name: \|` then `Build all` | that line |

A comment after the indicator (`name: >- # note`) is not part of the value,
and a line starting with `#` inside a block scalar is. A `name:` value that
keeps a line break **outside a `${{ … }}` expression** cannot be matched to a
reported name, because the whole name is compared (rule 2 below) and the
shown name then holds a line break. These values are refused by name, so the
workflow is `UNREADABLE` and the blocker says why
(`job <key>: name: keeps a line break`):

- a blank line inside the value;
- a line indented deeper than the first content line of a block scalar;
- a literal (`|`) block scalar of two or more lines, including
  `Build ${{ matrix.t }}` followed by `${{ matrix.u }}` on a second line.

A line break **inside** a `${{ … }}` is not refused: it is whitespace in the
expression, not in the shown name, and the whole expression stands for any
text. The lines are joined with one space. cpython's `reusable-san.yml`
splits its name this way:

```yaml
    name: >-
      ${{ inputs.sanitizer }}${{
        inputs.free-threading
        && ' (free-threading)'
        || ''
      }}
```

An empty block scalar (`name: >-` with nothing under it) is refused as
`job <key>: name: is an empty block scalar`.

**A fault under `jobs:` blocks only an expected workflow.** A refused job
name, an unreadable job key, bad indentation under `jobs:` or no jobs at all
makes the workflow `UNREADABLE` only if it is expected (`pull_request` in
`on:` and the base admitted). A workflow that is not expected, such as a
`workflow_call`-only reusable workflow, runs no job of its own for the pull
request, so its job names are never compared. A fault in `on:` always blocks,
because then the helper cannot tell whether the workflow is expected.

A reported job matches a defined job when its name:

1. equals the expected name exactly, `${{ … }}` included. GitHub reports a
   matrix job that was skipped before its matrix expanded under its raw
   name: home-assistant/core run 42858618158 shows
   `Run ${{ matrix.postgresql-group }} tests Python ${{ matrix.python-version }}`
   as `skipped`, byte for byte the `name:` in `ci.yaml`; or
2. matches the whole expected name with each `${{ … }}` read as any text.
   Runs of expressions with only spaces between them count as one. GitHub
   trims the shown name, so the space beside an expression at either end may
   be missing when the expression was empty. A ` (…` or ` / …` tail, as in
   rules 3 and 4, is allowed too. So `Build ${{ matrix.target }}` matches
   `Build x86_64-unknown-linux-gnu` but not `Builds`. On tokio's `ci.yml`,
   `features exclude ${{ matrix.name }}` matches the leg GitHub reports as
   `features exclude`, because the expression was empty and GitHub trims the
   trailing space. On deno's `ci.generated.yml`,
   `test ${{ matrix.test_crate }} ${{ matrix.shard_label }}debug macos-x86_64`
   matches `test libs debug macos-x86_64` but not
   `test libs release macos-x86_64`, which belongs to the `…release
   macos-x86_64` job. This rule also covers a raw name whose expression
   GitHub has rewritten: pydantic/pydantic #14268 reports
   `${{ (matrix.interpreter || 'all') }}` for the `name:` text
   `${{ matrix.interpreter || 'all' }}`; or
3. starts with `<key> (` or `<name> (`, for a matrix job: GitHub adds the
   matrix values in brackets when `name:` does not use them; or
4. starts with `<name> / `, for a reusable workflow.

Rules 3 and 4 apply to a literal `name:`. For a `name:` with an expression,
rule 2 allows the same tails. A literal between two expressions matches at
its leftmost place, atomically. This is the standard glob algorithm, and it
finds a match whenever one exists. It also keeps the regex engine from giving
up on a name with many expressions, which would make the whole evaluation
`UNREADABLE`.

**A reported job satisfies only the defined jobs that match it best.** Rule 1
beats rules 3 and 4, which beat rule 2. Between two rule-2 matches, the name
with more literal text wins. So if every leg of `Test ${{ matrix.os }}` is
never created, a sibling job named `Test docs` does not stand in for them. On
cpython's `build.yml`, `Windows MSI${{ '' }}` belongs to its own job (literal
`Windows MSI`), not to the folded `Windows ${{ … }}` job (literal `Windows`).
Two sibling jobs that are both skipped and reported under their raw names
each match their own name by rule 1. home-assistant's
`Run ${{ matrix.mariadb-group }} tests …` and
`Run ${{ matrix.postgresql-group }} tests …` are an example, so neither is
tied.

**A reported job belongs to at most one defined job.** If two defined jobs
match it equally well, the helper cannot tell whose leg it is. Examples are
`Test ${{ matrix.os }}` and `Test ${{ matrix.py }}` with the leg
`Test ubuntu-latest`, or two jobs both named `Lint`. The helper does not let
one leg satisfy both, because the other job might never have been created.
Each of them blocks as `UNREADABLE` and names the leg:

```text
blocker: expected checks unreadable: job 'Test <matrix.os>' of .github/workflows/m.yml cannot be matched: another of its jobs matches 'Test ubuntu-latest' as well; give them distinct literal names
```

If neither job reported a leg, nothing is tied, and both are `MISSING`.

**A repository with two jobs that share one `name:` cannot be driven.** No
rule that matches by name can tell such jobs apart. home-assistant/core's
`ci.yaml` names `pytest-full` (line 951) and `pytest-partial` (line 1510)
both `Run tests Python ${{ matrix.python-version }} (${{ matrix.group }})`,
and they run under opposite `if:` conditions. Whichever one runs reports a
leg, and the other is reported under the raw name. Both reports match both
jobs, so both jobs are `UNREADABLE` at every head until one is renamed.
Treating jobs with one name as one group that passes when any member reports
would accept, for those jobs, the weakness already accepted for matrix legs
(see [Limits](#limits-of-the-expected-set)). That is the owner's decision,
tracked in #2420. Until it is made, the helper keeps the stricter rule.

**A job whose name is only an expression** (`name: ${{ matrix.target }}`, or
several expressions with nothing literal between them) has no literal text to
match. A name such as `${{ matrix.os }} tests` is not expression-only: rule 2
matches its literal ` tests`. An expression-only job is satisfied by its own
raw name (rule 1, when it was skipped before its matrix expanded) and by the
jobs of its run that no other defined job matches. On tokio's `ci.yml`,
those are `wasm32-wasip1` and `wasm32-wasip1-threads`. While the run is going
and no such job exists, it is `PENDING`. When the run has completed and no
such job exists, it is `MISSING`. No check called `<matrix.target>` can
exist, so the blocker names the job by its key and gives the reason and the
fix, and `missing` holds the key:

```text
blocker: expected job 'wasm32-wasip1' of .github/workflows/ci.yml was never created at <sha>: its name '<matrix.target>' is only an expression and every job of run 1011 matches another job; if it ran, give it a literal name
```

If one workflow has two or more jobs named only by an expression and its run
has a job that no defined job matches, the helper cannot tell whose job that
is. Each of them then blocks as `UNREADABLE`, with a reason that names it:

```text
blocker: expected checks unreadable: job '<matrix.a>' of .github/workflows/m.yml cannot be matched: 2 of its jobs are named only by an expression; give them literal names
```

If every job of the run is claimed, for example because each expression-only
job was skipped and reported under its own raw name, nothing is ambiguous and
they are matched as above.

## Which conclusions pass

| Kind | Passes | Blocks |
| --- | --- | --- |
| Defined job (a required check of the same name included) | `success`, `neutral`; `skipped` when the job exists in the run | not `completed` (pending); `failure`, `cancelled`, `timed_out`, `action_required`, `stale`, `startup_failure`; never created |
| Required check with no defined job behind it (a commit status, or a check-run from another app or workflow) | `success`, `neutral` | anything else, `skipped` and `pending` included; never created |
| Any other check at `HEAD_SHA` | `success`, `neutral`, `skipped` | not `completed`; `failure`, `cancelled`, `timed_out`, `action_required`, `startup_failure`, `stale` |
| All checks at `HEAD_SHA` | at least one | zero check-runs and zero statuses |

`neutral` passes on every check. GitHub
[passes a required check](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches#require-status-checks-before-merging)
that is successful, skipped or neutral, and `gh pr checks`, which merge-evidence
step-03b reads, counts `NEUTRAL` as skipping, not failing. A `neutral` check
ran at the head and reported no failure; blocking it would leave the fixing
agent nothing to fix. The evidence line records it as `neutral`.

`skipped` is the one place the helper is stricter than GitHub. A `skipped`
required check passes only when a defined job at the head explains it. A
required status or check-run that reports `skipped` with no job behind it
blocks, because nothing shows that it ran or was meant to be skipped.

Check-runs are read from `commits/<HEAD_SHA>/check-runs` and statuses from
`commits/<HEAD_SHA>/status`. Nothing from any other SHA counts, so a green run
on an earlier push does not satisfy the current head.

Check-runs are grouped by name **and** `check_suite.id`, keeping the latest
run in each suite. A required name passes only if every remaining run with that
name passes, so a second suite cannot hide a failing run by reusing its name.
A commit status never overrides a failing check-run of the same name.

## Unreadable means blocked

Any of these makes `ci_status` `UNREADABLE`:

- `gh` exits non-zero, including a 404 on rules, branch, workflow or run
  endpoints;
- a response is not valid JSON, or a page of a paginated read fails;
- `jq` is missing or older than 1.6 (no `@base64d`);
- a workflow file has a top-level `on:` the scanner cannot read, or none;
- an expected workflow has a `jobs:` section the scanner cannot read (see
  [Matching a defined job](#matching-a-defined-job-to-a-reported-job));
- a workflow path or run id breaks the rules above;
- a run has no `path` field (some GitHub Enterprise Server versions).

The one 404 read as empty is `contents/.github/workflows?ref=<HEAD_SHA>`: a
repository with no workflows directory has no defined jobs. Its required
checks still apply.

The YAML scanner is an `awk` program that reads a stated subset of YAML: the
top-level `on:` with `pull_request`'s child keys, and each job's key and
`name:`. A form outside that subset is refused with a reason that names it,
so the verdict is `UNREADABLE`. The scanner never guesses a value.

## Output

The helper prints `evidence:` lines, then `blocker:` lines, then one final JSON
line, which is the verdict:

```json
{"ci_status":"GREEN","missing":[],"blockers":0}
```

| `ci_status` | Meaning | Exit |
| --- | --- | --- |
| `GREEN` | Every expected check exists at the head and passed; no other check at the head is pending or failing. | `0` |
| `MISSING` | At least one expected check or workflow run was never created at the head. Listed in `missing`. | `1` |
| `PENDING` | Nothing is missing or failing, but some check is not yet `completed`. | `1` |
| `RED` | A check failed. | `1` |
| `UNREADABLE` | The expected set or the checks at the head could not be read. | `1` |
| — | Bad arguments. | `2` |

When more than one applies, the first in this order wins: `UNREADABLE`,
`MISSING`, `RED`, `PENDING`. All blocker lines are still printed.

Exit `2` is the helper's own usage error. To its caller it is one more
non-`GREEN` result, never a pass.

Blocker wording:

| Case | Line |
| --- | --- |
| Expected check never created | `blocker: expected check '<name>' was never created at <sha>` |
| Defined job absent from a run still going | `blocker: expected check '<name>' is not created yet; run <id> is <status> at <sha>` |
| Workflow with no run at the head | `blocker: expected workflow '<path>' never ran at <sha>` |
| Expected check pending or not passed | `blocker: expected check '<name>' is <state>/<conclusion> at <sha>` |
| Other check failing or pending | `blocker: check '<name>' is <state>/<conclusion> at <sha>` |
| No checks at all | `blocker: no checks at <sha>` |
| Unreadable | `blocker: expected checks unreadable: <endpoint or reason>` |

Every name is cleaned before it is printed: only printable characters are
kept, the characters `` ' " \ $ ` { } `` are removed, and it is cut to 120 characters. The
verdict line is built with `jq -cn --arg`, and callers read only the last line,
so a check name cannot forge a verdict.

## Example: a required check that never started

A pull request against `main` in this repository. `main` has no rulesets
(`rules/branches/main` returns `[]`); its branch protection requires seven
checks: `Lint & Format`, `Test`, `Install Smoke Test` and the four
`Build ${{ matrix.target }}` legs.

Two workflows run on `pull_request` to `main`:

- `ci.yml` defines `Lint & Format`, `Test`, `Install Smoke Test`,
  `Build ${{ matrix.target }}` (four legs), `Prune sccache caches` and
  `Release`.
- `recipe-runner-pin.yml` defines `Recipe runner pin ancestry`.

The `ci.yml` run at the head reported every job except `Install Smoke Test`.
The helper prints one evidence line per reported job, in workflow order:

```console
$ /bin/bash -p amplifier-bundle/tools/autodrive_expected_checks.sh 3f9c2a7e1b4d5c6f708192a3b4c5d6e7f8091a2b main
evidence: required: Lint & Format, Test, Install Smoke Test, Build x86_64-unknown-linux-gnu, Build aarch64-unknown-linux-gnu, Build x86_64-apple-darwin, Build aarch64-apple-darwin
evidence: workflow .github/workflows/ci.yml: run 18234567890 is completed/success
evidence: expected 'Lint & Format' (required): success
evidence: expected 'Test' (required): success
evidence: expected 'Build x86_64-unknown-linux-gnu' (required): success
evidence: expected 'Build aarch64-unknown-linux-gnu' (required): success
evidence: expected 'Build x86_64-apple-darwin' (required): success
evidence: expected 'Build aarch64-apple-darwin' (required): success
evidence: expected 'Prune sccache caches': skipped
evidence: expected 'Release': skipped
evidence: workflow .github/workflows/docs.yml: no run at 3f9c2a7e1b4d5c6f708192a3b4c5d6e7f8091a2b (paths filter; not expected)
evidence: workflow .github/workflows/recipe-runner-pin.yml: run 18234567712 is completed/success
evidence: expected 'Recipe runner pin ancestry': success
blocker: expected check 'Install Smoke Test' was never created at 3f9c2a7e1b4d5c6f708192a3b4c5d6e7f8091a2b
{"ci_status":"MISSING","missing":["Install Smoke Test"],"blockers":1}
$ echo $?
1
```

`Install Smoke Test` is both required and a defined job. It is named once in
`missing` and gets one blocker line. `docs.yml` has a `paths:` filter and no
run, so it gets a note, not a block. `invisible-char-scan.yml` runs on
`pull_request_target`, and `atlas.yml`, `release.yml`, `publish-snapshot.yml`
and `branch-protection-guard.yml` do not run on `pull_request`, so none of them
is expected.

## In the merge evidence step

The merge-ready loop's CI read (`autodrive-merge-evidence.yaml`
`step-03b-expected-checks`) is a bash step that reads the head's checks once
per round, with no wait (#2342; see
[The merge evidence steps](auto-drive-to-merge.md#the-merge-evidence-steps-2342)).
When the rollup is `GREEN`, it runs the trusted read-only
`/bin/bash -p "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_expected_checks.sh" <head-sha> <base-branch>`,
never a copy from the pull request's worktree or the git toplevel, and takes
its final JSON line. It passes only a `headRefOid` of exactly 40 or 64
hexadecimal characters; any other head makes a green rollup `UNREADABLE`
without running the helper.

| Helper says | `ci_status` |
| --- | --- |
| `GREEN` | `GREEN` |
| `MISSING`, `RED` or `PENDING` | the helper's status; `ci_signal` names the missing checks |
| anything else, or no verdict | `UNREADABLE` |

Only `GREEN` passes the merge round's measured downgrade, so `MISSING`,
`PENDING`, `RED` and `UNREADABLE` all keep the round `NOT_MERGE_READY`.
Finalization refreshes the head before any merge request and requires
expected-head protection, so a head that moves after this read is not merged
on stale evidence. `autodrive_platform_facts.sh` still reports a `CHECKS`
fact; it is advisory only.

## Limits of the expected set

These are deliberate. The defence for each is a ruleset of required checks on
the base branch, which the helper always enforces.

- **Path filters, `types:` filters and branch patterns jq cannot compile.**
  A workflow with one of these filters that has a run at the head has all its
  jobs expected. One with **no** run is not expected and gets an `evidence:`
  note naming the filter (see [The expected set](#the-expected-set)). So if
  such a workflow should have run but failed to start, the helper does not
  catch it.
- **Matrix legs are not counted.** A matrix job is one defined job, and any
  one reported leg satisfies it (matching rules 2 and 3). If one leg of
  `Build ${{ matrix.target }}` is dropped and the others are reported, the job
  passes. The helper does not expand matrices. The defence is to require each
  leg on the base branch: this repository requires every `Build` leg, and a
  required leg that was never created blocks as `MISSING`.
- **Name matching.** Defined jobs are matched by exact name, or by the whole
  name with each `${{ … }}` read as any text (see
  [Matching a defined job](#matching-a-defined-job-to-a-reported-job)). An
  expression stands for any text, so a leg whose computed name happens to fit
  another job's pattern better is counted for that other job. A job named
  only by an expression is met by its own raw name and by the jobs of its run
  that no other defined job matches. Two jobs named only by an expression in
  one workflow, when a job of the run is unclaimed, or two defined jobs that
  match one reported job equally, are `UNREADABLE` by name. **A workflow with
  two jobs that share one `name:` is therefore `UNREADABLE` at every head,
  and auto-drive cannot merge in that repository** (home-assistant/core's
  `pytest-full` and `pytest-partial`; the owner's decision on treating them
  as "any one of them" is #2420). Rules 3 (`<key> (` or `<name> (` for a
  matrix job) and 4 (`<name> / ` for a reusable workflow) can match a job
  that the name alone would not, so each has its own test.
- **No app pinning.** Required checks match by context name. The
  `integration_id` in a ruleset is not compared. Pin `integration_id` in the
  ruleset if a check name could be posted by another app.
- **Workflow list read at the head only.** A pull request that deletes or
  renames a workflow removes it from the defined jobs. Required checks from
  the base branch still apply, and crusty reviews workflow changes.
- **Workflows read at the head, but `pull_request` runs use the merge
  commit.** The helper reads each workflow file at `<HEAD_SHA>`. GitHub runs a
  `pull_request` workflow from `refs/pull/<N>/merge`, the test merge of the
  head into the base, so `GITHUB_SHA` is that merge commit (and no such run
  starts while the pull request has merge conflicts). If the base branch
  renames or removes a job after the branch point, the run creates the base's
  jobs while the helper still expects the head's: the old name is reported as
  `was never created`, and the verdict stays `MISSING` until the base is
  merged into the branch. A job added on the base is not expected, but its
  check still has to pass, as every check at the head does. The helper does
  not read the merge commit, because `pulls/<N>.merge_commit_sha` changes
  every time the base moves. Instead, whenever a job or workflow from the
  head's files is missing, it adds one `evidence:` line that says so:

  ```text
  evidence: jobs are read from the workflow files at the head, but pull_request runs build the merge commit with main: if main changed a missing job or workflow since this branch forked, merge main in
  ```

  The merge-round prompt gives the agent the same rule for `MISSING`. A
  required check that was never created gets no such line: required checks
  are read from the base branch, not from the head.
- **Only active workflows are expected.** A workflow that `actions/workflows`
  does not list as `active` is not expected. A workflow that the pull request
  adds, and that has never run, is therefore not required. As with deleted
  workflows, required checks from the base branch are the defence.
- **Required checks from `branches/<base>`.** Classic protection is read from
  the branch object, not the admin-only protection endpoint.
- **`skipped` is taken at GitHub's word.** The helper does not evaluate `if:`
  conditions. A defined job that GitHub reports as `skipped` passes, so if an
  outage made GitHub skip a job whose condition was true, the helper cannot
  tell that from a genuine skip. A job that is *absent* is never excused (see
  [The expected set](#the-expected-set)).
- **jq 1.6 or later** is required; older jq gives `UNREADABLE`.
- **`neutral` passes** on every check, as on GitHub (see
  [Which conclusions pass](#which-conclusions-pass)).
- **`skipped` passes only for a defined job.** A required check that reports
  `skipped` passes when its name matches a job in one of the pull request's
  workflows, as GitHub itself treats a skipped required check. A required
  status or check-run that reports `skipped` with no job behind it blocks.

`GH_REPO` and `GH_HOST` are taken from the operator's environment, as for every
other `gh` call in this workflow.

## Related documentation

- [Auto Drive To Merge Reference](auto-drive-to-merge.md)
- [Evidence and outcomes](auto-drive-evidence.md)
