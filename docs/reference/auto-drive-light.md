---
title: Auto-Drive-Light Recipe
description: The smaller drive-to-merge recipe for small pull requests, its eligibility rules, escalation to auto-drive-to-merge, and the minor-only review rule.
last_updated: 2026-11-01
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-Drive-Light Recipe

> [Home](../index.md) > Reference > Auto-Drive-Light

`auto-drive-light` builds a small change in one agent session, opens a pull request, and drives that PR to merge through the same crusty loop, merge loop and evidence gate as [`auto-drive-to-merge`](auto-drive-to-merge.md). It has no workflow-design, consensus-design or architect steps and never calls `default-workflow`.

A bash step measures the PR after it is opened. If the change exceeds any eligibility rule, the run hands the same PR to `auto-drive-to-merge`. In light mode, a crusty round that raises only minor concerns defers them to a follow-up issue in place of starting a fix round.

The recipe is `amplifier-bundle/recipes/auto-drive-light.yaml`. It was added for issue #1802.

## Contents

- [When to use it](#when-to-use-it)
- [Running it](#running-it)
- [Context variables](#context-variables)
- [Steps](#steps)
- [Eligibility rules](#eligibility-rules)
- [The eligibility tool](#the-eligibility-tool)
- [Escalation to auto-drive-to-merge](#escalation-to-auto-drive-to-merge)
- [The light-mode marker](#the-light-mode-marker)
- [The minor-only rule](#the-minor-only-rule)
- [The light CLEAN record](#the-light-clean-record)
- [Confirming rounds](#confirming-rounds)
- [Returning concern ids](#returning-concern-ids)
- [The follow-up issue](#the-follow-up-issue)
- [Merge criteria](#merge-criteria)
- [State files](#state-files)
- [Recursion limits](#recursion-limits)
- [Security limits](#security-limits)
- [Limitations](#limitations)
- [Files](#files)
- [Related](#related)

## When to use it

Use `auto-drive-light` for a change that should be reviewed and merged without supervision, but is too small to warrant the design phases of `default-workflow`. Typical examples are a bug fix inside one private function, a new test, a documentation correction with a code example, or a dependency bump that needs a code adjustment.

smart-orchestrator does not select this recipe. Run it by name. See [Choosing a drive recipe](../recipes/README.md#choosing-between-quick-change-auto-drive-light-and-auto-drive-to-merge) for a comparison with `quick-change` and `auto-drive-to-merge`.

## Running it

A new change:

```bash
amplihack recipe run auto-drive-light \
  -c task_description="Return an error instead of panicking when the round record is empty in autodrive_last_round.sh" \
  -c repo_path=.
```

A PR that already exists:

```bash
amplihack recipe run auto-drive-light \
  -c pr_number=1850 \
  -c repo_path=.
```

A different QA command and a higher line limit:

```bash
amplihack recipe run auto-drive-light \
  -c task_description="Add the missing --end-of-options to the rev-parse calls in autodrive_pr_head.sh" \
  -c repo_path=. \
  -c qa_command="cargo test -p amplihack-cli --test main issue_1570" \
  -c light_max_lines=400
```

A dry run that verifies everything and reports what it would merge, without merging:

```bash
amplihack recipe run auto-drive-light \
  -c pr_number=1850 \
  -c repo_path=. \
  -c autodrive_dry_run=true
```

The eligibility decision appears on stderr in the `light-eligibility` step:

```text
AUTODRIVE_LIGHT_ELIGIBILITY=light reason=within-limits
```

## Context variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `task_description` | `""` | The change to make. Required unless `pr_number` or `existing_branch` is set. |
| `repo_path` | `"."` | Repository to work in. |
| `issue_number` | `""` | Optional. 1 to 10 digits. The PR body gets `Refs #N` and the branch name uses it. |
| `existing_branch` | `""` | A branch whose PR already exists. Validated with `git check-ref-format` and refused if it begins with `-`. A branch without an open PR is refused with `light-existing-branch-no-pr`. |
| `pr_number` | `""` | An open PR to drive. 1 to 10 digits. The build steps are skipped. |
| `qa_command` | `""` | When set, run as `bash -c "$RECIPE_VAR_qa_command"` in the worktree in place of `quick_change_checks.sh`. |
| `light_max_lines` | `"300"` | Line limit. See [Eligibility rules](#eligibility-rules). |
| `light_max_new_modules` | `"0"` | New-module limit. |
| `light_max_public_api` | `"0"` | Public API line limit. |
| `light_max_schema` | `"0"` | Schema file limit. |
| `light_max_recipe_contract` | `"0"` | Limit on recipes whose `context:` key set changed. |
| `autodrive_state_dir` | `""` | Durable state root. Empty means derive it from the repository remote and the PR, as `auto-drive-to-merge` does. Must be an absolute path with no `..` component and no control characters. |
| `autodrive_dry_run` | `"false"` | Passed to the merge loop unchanged. |
| `no_merge` | `"true"` | Applies to the build only. See [The no_merge value](#the-no_merge-value). |

Every limit must match `^[0-9]{1,6}$` and is read as base 10, so `010` means ten. The `light-eligibility` step validates all five limits itself, before it calls the eligibility tool, and fails with exit code 2 on any other value. An invalid limit therefore stops the run; it is never turned into an escalation. There is no option that skips the eligibility check and no force-light override: selecting `auto-drive-light` by name is still subject to the check.

### The no_merge value

`no_merge` governs the build only. `light-implement` never merges, and its prompt carries `[amplihack:no-merge]` whatever the value. On escalation the value is passed to the nested `auto-drive-to-merge`, where it again governs only that recipe's build phase. It does not stop either merge loop from merging through the gate. To verify a PR without merging it, set `autodrive_dry_run=true`.

The recipe also fills these values as it runs: `light_base`, `build_result`, `light_eligibility`, `light_escalation_result`, `crusty_loop_result` and `merge_gate_result`.

## Steps

| # | Step | Kind | Runs when |
| --- | --- | --- | --- |
| 1 | `light-prereqs` | bash | always |
| 2 | `light-pr-input` | bash | always |
| 3 | `light-worktree` | recipe `workflow-worktree` | new change |
| 4 | `light-base-pin` | bash | new change |
| 5 | `light-implement` | agent | new change |
| 6 | `light-qa` | bash | new change |
| 7 | `light-precommit` | bash | new change |
| 8 | `light-commit-push` | bash | new change |
| 9 | `light-open-pr` | bash | new change |
| 10 | `light-resolve-pr` | bash | always |
| 11 | `light-eligibility` | bash | always |
| 12 | `light-escalate` | recipe `auto-drive-to-merge` | decision is `escalate` |
| 13 | `light-mode-marker` | bash | decision is `light` |
| 14 | `light-crusty-loop` | recipe `autodrive-crusty-loop` | decision is `light` |
| 15 | `light-check-unmerged` | bash | decision is `light` |
| 16 | `light-merge-loop` | recipe `autodrive-merge-loop` | decision is `light` |
| 17 | `light-report` | bash | always |

"New change" means neither `pr_number` nor `existing_branch` was given.

**Build (steps 3 to 9).** The worktree and branch come from `workflow-worktree` and `quick_change_common.sh`, so the branch is named `quick/<issue-or-slug>`. `light-base-pin` records the base commit before the agent runs; that commit is `light_base`. `light-implement` is a single agent session whose prompt carries the quick-change leaf-step constraints and the line `[amplihack:no-merge]`. It never tells the agent to start an orchestrator. `light-qa` runs `qa_command` or `quick_change_checks.sh`; `light-precommit` runs the repository's pre-commit hooks. A failing check stops the run before anything is pushed.

**PR resolution (step 10).** `light-resolve-pr` writes `build_result` in the same shape as `autodrive-build`, with `pr`, `branch`, `head_sha`, `base_sha` and `state_dir`. For an existing PR it reads `gh pr view` and refuses a PR whose `isCrossRepository` is true (`light-cross-repo`). For an existing PR, `light_base` is the merge base of the PR head and `origin/<baseRefName>`. PR code fetched as `refs/pull/<N>/head` is not checked out, sourced or run before review.

**Light path (steps 13 to 16).** `light-mode-marker` is described in [The light-mode marker](#the-light-mode-marker). `light-crusty-loop` and `light-merge-loop` run the unchanged `autodrive-crusty-loop` and `autodrive-merge-loop` sub-recipes that `auto-drive-to-merge` uses, on the same state directory. There is no `autodrive-check-unmerged` recipe: in `auto-drive-to-merge.yaml` that check is an inline bash step. `light-check-unmerged` is a bash step with the same body. It reads `build_result`, sources `autodrive_state.sh` from `AMPLIHACK_HOME` and calls `autodrive_require_unmerged`, so a PR merged outside the gate during the crusty loop stops the run with exit 1 (#1551).

**Report (step 17).** Prints which path ran (`light`, `escalate` or `light-escalated`), the eligibility or escalation reason, the follow-up issue number if one was filed, and the merge gate result.

## Eligibility rules

The rules measure `git diff <light_base>..<head>` in the worktree. Each rule escalates when its measurement is above its limit; a measurement equal to the limit passes.

| Rule | `reason` token | Measurement | Default limit |
| --- | --- | --- | --- |
| Lines | `lines` | Sum of added and deleted lines from `git diff --numstat`. `Cargo.lock`, `*.lock` and `package-lock.json` are excluded. Binary files count as 0. | 300 |
| New modules | `new-module` | Added files matching `crates/*/src/**/*.rs`, any added `Cargo.toml`, any added `amplifier-bundle/recipes/*.yaml`, any added file under `amplifier-bundle/tools/`. Added test files (`crates/*/tests/**`, `tests/**`) and added documentation do not count. | 0 |
| Public API | `public-api` | Added or removed lines in `crates/*/src/lib.rs` matching `^\s*pub[\s(]`, excluding `pub(crate)`, `pub(super)` and `pub(in …)`. | 0 |
| Schema | `schema` | Changed `*.schema.json` files and changed files under any `schemas/` directory. | 0 |
| Recipe contract | `recipe-contract` | Modified recipe YAML files whose set of top-level `context:` keys differs between base and head. A changed default value does not count. | 0 |

When several rules fire, `reason` names the first in table order; the JSON line carries every measurement.

An edit to an existing recipe or tool file does not escalate unless it changes that recipe's `context:` key set or crosses the line limit. The quick-change `sensitive-path` rule is not applied. `quick_change_diff_limits.sh` is not used and is unchanged.

### When the rules are measured

- `light-eligibility` runs once the PR exists and its head is known. The decision is written to `light-eligibility.json` with the base and head it measured and the limits it used.
- A resumed run whose PR head equals the recorded head reuses the decision. When the PR head has moved, `light-eligibility` measures again on `light_base..<current head>`.
- Inside the loops, every crusty round that finds a valid marker compares the head it reviewed with the marker head. When they differ, `autodrive_light_round.sh` measures again on `<base>..<reviewed head>`, taking the base and limits from `light-eligibility.json`. This covers commits made by crusty address steps, commits pushed by anyone else, and crusty re-reviews in the merge phase. See [Escalation inside the loop](#escalation-inside-the-loop).

An earlier escalation is permanent for the state directory. When `escalation.json` is present, `light-eligibility` still measures and writes `light-eligibility.json`, but the decision is `escalate` with reason `previously-escalated`, and the file is never removed. To try light mode again on the same PR, pass a new `autodrive_state_dir` or delete `escalation.json` by hand.

## The eligibility tool

`amplifier-bundle/tools/autodrive_light_eligibility.sh` is a stateless measurement. It uses git plumbing only and writes nothing to the repository or the state directory; each diff goes to a private temporary directory, which is removed on exit. A diff that fails is a `bad-rev` fault, never a zero measurement.

```text
autodrive_light_eligibility.sh --repo <dir> --base <rev> --head <rev>
    [--max-lines N] [--max-new-modules N] [--max-public-api N]
    [--max-schema N] [--max-recipe-contract N]
```

`--base` must be an ancestor of `--head`. Revisions are resolved with `git rev-parse --verify --end-of-options`, the diff runs with `--no-ext-diff --no-textconv`, path lists are read with `-z`, and every pathspec follows `--`. File names containing newlines are measured correctly.

Output on success (exit 0), two lines:

```text
AUTODRIVE_LIGHT_ELIGIBILITY=escalate reason=lines
{"decision":"escalate","reason":"lines","base":"9f2c1e0a4b7d3e8f1a2b3c4d5e6f708192a3b4c5","head":"4e1d2c3b4a5f60718293a4b5c6d7e8f901a2b3c4","lines":412,"new_modules":0,"public_api":0,"schema":0,"recipe_contract":0,"limits":{"lines":300,"new_modules":0,"public_api":0,"schema":0,"recipe_contract":0}}
```

A change within every limit prints `decision` `light` and `reason` `within-limits`.

Faults exit 2 and print one line on stderr, `AUTODRIVE_LIGHT_ELIGIBILITY=fault reason=<token>`:

| Token | Cause |
| --- | --- |
| `bad-arg` | Unknown option, missing value, or a limit that does not match `^[0-9]{1,6}$`. A base or head beginning with `-` is refused here. |
| `not-a-repo` | `--repo` is not a git work tree. |
| `bad-rev` | A revision does not resolve to a commit, or a `git diff` or recipe read fails. |
| `not-ancestor` | The base is not an ancestor of the head. |
| `jq-missing` | `jq` is not on PATH. |

The `light-eligibility` step treats a fault as `escalate` with reason `fault-<token>`, so a PR that cannot be measured gets full review. The step has already validated the limits and passes only digits, so `fault-bad-arg` indicates a fault in the tool or its call, not bad user input.

## Escalation to auto-drive-to-merge

When the decision is `escalate`, `light-eligibility` does three things before `light-escalate` runs:

1. Removes any `light-mode` marker from the state directory.
2. Writes `escalation.json` with the reason, the measurements, `base_sha` and `head_sha`.
3. Sets `light_eligibility.decision` to `escalate`.

`light-escalate` is a `type: recipe` step that runs `auto-drive-to-merge` with `pr_number` taken from `build_result`, `existing_branch` empty, and `autodrive_state_dir` set to the same state directory. `task_description`, `repo_path`, `autodrive_dry_run` and `no_merge` are passed unchanged. Only the PR number is passed because `autodrive-build` uses `pr_number` whenever it is set and looks a branch up only when it is empty. Because the PR already exists, the nested build phase is a no-op: no second branch, build or PR is created. The nested recipe then runs its full crusty loop, merge loop and gate.

Every later light step has a condition on `light_eligibility.decision == 'light'` and is skipped. `light-escalate` is the only step in the recipe that names `auto-drive-to-merge`. Exit code 79 from the nested run is passed through and never retried.

## The light-mode marker

`light-mode-marker` writes `<state>/light-mode` after a `light` decision, and only then. The file holds one line, the head SHA that was measured. It is created under `umask 077` through `mktemp` and `mv -f`.

`autodrive-crusty-round` looks for the marker in `$(dirname "$AUTODRIVE_ROUND_RECORD")`. No context key is added to `autodrive-crusty-loop`. Crusty re-reviews in the merge phase (`autodrive_crusty_rereview.sh`) use the same state directory, so they apply light rules too.

The round accepts the marker only when all of these hold:

- it is a regular file, not a symlink, and private to the current user;
- its content is a 40 or 64 character lowercase hex SHA;
- `light-eligibility.json` is a private regular file with `decision` `light` and the same head;
- `escalation.json` is absent.

A marker that fails these checks gives `marker-invalid` or `state-unsafe`. A reviewed head that does not descend from the marker head gives `marker-stale`. In every one of those cases the round takes the normal path. A reviewed head that descends from the marker head but differs from it is measured again, as described below.

### Escalation inside the loop

When the reviewed head differs from the marker head, `autodrive_light_round.sh` runs `autodrive_light_eligibility.sh` on `<base>..<reviewed head>` with the limits recorded in `light-eligibility.json`, before it applies the minor-only rule.

The base is the merge-base of the reviewed head and the PR's current base tip, so a head that merged the base branch in (the usual fix for a branch that is behind) is measured on the PR's own change only, as GitHub shows it. The tool reads `baseRefName` and `baseRefOid` with `gh pr view`, checks both, fetches `refs/heads/<baseRefName>` from `origin` if the tip is not present locally, and uses the merge-base only when it descends from the base recorded in `light-eligibility.json`. In every other case (a `gh` failure, an invalid name or SHA, a failed fetch, or a merge-base that does not descend from the recorded base) it logs one `INFO:` line and measures from the recorded base. For a failed fetch, that line includes the first line of the `git fetch` error, cut to 200 characters. That measurement counts any upstream changes the head has merged, so it can only escalate more often. Exit code 79 from `gh` is passed through.

- **`light`.** The tool rewrites `light-eligibility.json` with the new head and the base it measured from, then rewrites `light-mode` with the same head, and goes on to the minor-only rule.
- **`escalate` or a fault.** The tool removes `light-mode`, writes `escalation.json` with reason `in-loop-<reason>` or `in-loop-fault-<token>`, and prints `light_exit` `none` with reason `re-measured-escalate`. The round takes the normal path and addresses every concern.

After an escalation inside the loop, every later round in that state directory, including merge-phase re-reviews, finds no marker and runs with full rules. The run stays in the same crusty and merge loops; it does not start a nested `auto-drive-to-merge`, because those loops are the ones that recipe would run. `light-report` prints the path as `light-escalated` with the recorded reason.

Without the marker, `autodrive-crusty-round` behaves exactly as it did before #1802: the same steps run, step-03 and step-06 write the same bytes, and no new file is written. `autodrive_light_round.sh` is not called. Only step-03 and step-06 change; step-04 and step-05 are untouched.

## The minor-only rule

In step-03 of `autodrive-crusty-round`, when the marker is present, the parsed concerns go to `autodrive_light_round.sh defer`, loaded from the launch-side tools (`AUTODRIVE_LAUNCH_TOOLS`). The round recipe is at its 400-line brick budget, so the light branch of step-03 and step-06 lives in `amplifier-bundle/tools/autodrive_light_steps.sh`, which each step sources in one line, and only under the light condition. Both steps load it from `AUTODRIVE_LAUNCH_TOOLS`, the tool root that step-00 has already checked (#1543), so the helper and the round tool always come from the same install. If that file cannot be loaded, step-03 keeps `CONCERNS` and step-06 writes `CONCERNS` with `WARNING: light-side-file-invalid`. Step-06 calls the helper only when the verdict text names `light-minor-rule`; if the verdict's `verdict_source` is anything else, the helper returns an error and step-06 records `CONCERNS`. The tool works out an effective severity for each concern and fails closed:

| Concern | Effective severity |
| --- | --- |
| `severity` is exactly `minor` and no rule below applies | `minor` |
| `severity` missing, empty or not one of `minor`, `major`, `blocking` | `major` |
| `id` missing or not matching `^[A-Za-z0-9._-]{1,64}$` | `major` |
| `id` is a [returning id](#returning-concern-ids) | `major` |
| `id` contains a security keyword: `sec`, `auth`, `secret`, `token`, `credential`, `inject`, `xss`, `csrf`, `cve`, `vuln`, `unsafe`, `privilege`, `perm` (case-insensitive substring) | `major` |
| `summary` or `evidence` contains a security word (case-insensitive, whole words; see below) | `major` |

In the id, keywords match as substrings, not whole tokens. An id such as `section-1`, `author-name` or `permalink-2` is therefore promoted to `major`. This is intended: a false promotion costs one fix round, while a missed security concern would be deferred to an issue.

In the `summary` and `evidence`, the tool matches whole words that begin with one of these stems: `secur` (and `insecure`), `authenticat`, `authoris`/`authoriz`, `authn`, `authz`, `secret`, `token`, `credential`, `inject`, `xss`, `csrf`, `cve`, `vuln`, `unsafe`, `privilege`, `permission`, `sanitis`/`sanitiz`, `escape`, `traversal`, `ssrf`, `rce` and `password`. Whole words are used here because free text would otherwise match almost every concern (`sec` in "second", `auth` in "author"). This check covers a security concern that crusty grades `minor` under a neutral id such as `c3`. Some harmless concerns are also promoted, for example one about a parser's tokens or file permissions; each costs one fix round.

The rule applies only when there is at least one concern and every effective severity is `minor`. A `CONCERNS` verdict with an empty `concerns` array is not minor-only. A `CLEAN` verdict from crusty is unchanged.

When the rule applies, the tool:

1. Writes `<record>.concerns.tsv` with one row per concern: id, effective severity, summary.
2. Files or reuses the [follow-up issue](#the-follow-up-issue) and links it from the PR.
3. Writes `<record>.light.json` with exactly two keys, `follow_up_issue` and `deferred_ids`.
4. Truncates `<record>.findings` to empty.
5. Prints one JSON line with `light_exit` `minor-deferred`.

Step-03 then emits `{"crusty_verdict":"CLEAN","verdict_source":"light-minor-rule","concern_count":N}`, where N is the tool's `concern_count`. Step-03 normally counts the lines of `.findings`; that file is now empty, so in light mode it takes the count from the tool instead. Step-04 (address) and step-05 (fix check) are skipped through their existing `CONCERNS` conditions. The light rule does not start another round.

The tool always prints one JSON line:

| `light_exit` | Meaning | Step-03 verdict |
| --- | --- | --- |
| `off` | No marker. Not printed in practice, because step-03 does not call the tool without one. | unchanged |
| `none` | Marker present but the rule does not apply. A `reason` field gives `no-concerns`, `not-minor-only`, `marker-invalid`, `marker-stale`, `state-unsafe`, `re-measured-escalate` or `follow-up-failed`. | unchanged |
| `minor-deferred` | The rule applied. Carries `follow_up_issue`, `deferred_ids` and `concern_count`. | `CLEAN` |

If `gh` fails while filing or linking the issue, the tool prints `none` with `follow-up-failed`, writes the `follow-up-failed` state file, and leaves `.findings` untouched, so the round takes the normal path and addresses the concerns.

### Why `.findings` is emptied

When the crusty loop ends DONE, `autodrive_record_crusty_loop_done` in `autodrive_state.sh` writes every id from every `crusty-round-*.json.findings` file in the state directory to `resolved-concerns.txt`. A deferred id left in `.findings` would be recorded there as resolved, when it was only deferred to an issue. Emptying `.findings` keeps `resolved-concerns.txt` limited to concerns that were addressed. The id and severity rows live in `<record>.concerns.tsv` instead.

Two other readers of `.findings` are affected:

- Step-03 counts concerns from `.findings`. In light mode it takes `concern_count` from the tool, so the record still reports how many concerns crusty raised.
- `autodrive_loop.sh` copies `.findings` for loop-health's recurring-findings signal. A light CLEAN round contributes no ids to that signal.

## The light CLEAN record

This CLEAN is produced by a bash rule after crusty raised only minor concerns. It is not crusty's own verdict, and the record says so.

Step-06 reads `<record>.light.json` when `verdict_source` is `light-minor-rule`, and checks that:

- it is a private regular file with exactly the keys `follow_up_issue` and `deferred_ids`;
- `follow_up_issue` is 1 to 10 digits;
- every deferred id has a `minor` row in `<record>.concerns.tsv`, and the TSV has no row that is not `minor`;
- `<record>.findings` is empty.

It then writes the normal record with a suffix built by `jq`:

```json
{"crusty_verdict":"CLEAN","concern_count":2,"commits_this_round":0,"head_sha":"4e1d2c3b4a5f60718293a4b5c6d7e8f901a2b3c4","reviewed_head_sha":"4e1d2c3b4a5f60718293a4b5c6d7e8f901a2b3c4","round_label":"crusty-round-1","test_signal":"","ci_signal":"","verdict_source":"light-minor-rule","follow_up_issue":"1851","deferred_ids":["naming-1","doc-comment-2"]}
```

The record is one line, begins with `{"crusty_verdict":"CLEAN",` and has exactly one `reviewed_head_sha`, so `autodrive_crusty_final`, merge gate section 6b and the `crusty-evidence` step accept it with no change.

If any check fails, step-06 writes a `CONCERNS` record with no light suffix and prints `WARNING: light-side-file-invalid` on stderr. The loop then runs another round.

## Confirming rounds

`autodrive_loop.sh` is unchanged. After a light CLEAN round, loop-health decides whether the loop stops, as it does after any CLEAN round. If it answers `CONTINUE`, the shared loop runs one more confirming round:

- New minor-only concerns are posted to the existing follow-up issue as one comment. Ids already listed in `follow-up-posted-ids` are not posted again; if every id was already posted, no comment is made. No second issue is filed. The round writes another light CLEAN record.
- Any `major` or `blocking` concern, or any concern that becomes `major` under the rules above, takes the normal path: step-04 addresses every concern in that round, minor ones included.

## Returning concern ids

A concern id is returning when it appears in `resolved-concerns.txt` or in the `.findings` file of an earlier `crusty-round-*.json` record in the same state directory. A returning id is promoted to `major`.

No marker file is needed. A light round empties its `.findings`, so a non-empty `.findings` file belongs to a round that kept its `CONCERNS` verdict, and step-04 addressed every concern in such a round. An id found there was therefore addressed once and has come back.

Deferred ids are never in `.findings`, so a deferred minor id raised again stays `minor` and is not posted to the issue again.

## The follow-up issue

| Property | Value |
| --- | --- |
| Repository | The PR's repository, checked against `^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$` on github.com. |
| Title | `Follow-up: minor review concerns from PR #<pr>` |
| Label | `follow-up`, only when `gh label list` shows that it exists. |
| Body | A table of id, severity, summary and evidence, and the hidden marker `<!-- auto-drive-light:pr-<pr> -->`. |
| Delivery | `gh issue create --body-file`, through `workflow_gh_retry.sh`. The body file is removed after a successful create. |
| Recorded in | `<state>/follow-up-issue`, written atomically straight after creation. |
| PR comment | `Minor review concerns deferred to #<N>.`, posted once and recorded in `<state>/follow-up-linked`. |

Example body:

```markdown
<!-- auto-drive-light:pr-1850 -->
Minor review concerns raised by crusty on PR #1850 and deferred by the auto-drive-light minor-only rule.

| id | severity | summary | evidence |
| --- | --- | --- | --- |
| naming-1 | minor | Rename `tmp_rec` to `record_copy` | autodrive_last_round.sh line 41 |
| doc-comment-2 | minor | The header comment still names the old exit code | autodrive_last_round.sh line 3 |
```

### Finding an existing issue

1. A number in `<state>/follow-up-issue` is always reused.
2. Otherwise the tool searches open issues in the repository for the marker. This covers a crash between creating the issue and recording it. A result is reused only if it is open, was written by the login that `gh api user` returns, and contains the marker exactly once.
3. Otherwise it creates a new issue.

Issue numbers are validated as 1 to 10 digits before they are written or reused.

### Cell sanitising

Each table cell has control characters removed, passes through `sanitize_cli_output`, has `&`, `<` and `>` HTML-encoded, has `|` and backticks replaced by the numeric entities `&#x7C;` and `&#x60;`, has `@` mentions, `#N` references and full `github.com/` URLs neutralised with U+200B (after `@`, after `#`, and after `github.com/`, matched without regard to case), and is truncated to 200 characters. The marker therefore appears exactly once, however the concern text is written. Titles and PR comments contain only fixed text and digits.

## Merge criteria

The merge criteria are those of `auto-drive-to-merge`, unchanged. `autodrive-merge-loop`, `autodrive-merge-round`, `autodrive-merge-evidence` and `autodrive_merge_gate.sh` are the same files. A light run merges only through the gate, which requires the crusty loop to have ended DONE with a CLEAN latest record, qa-team scenarios, green CI, no conflicts and resolved review threads. See [How the merge round applies merge-ready](auto-drive-to-merge.md#how-the-merge-round-applies-merge-ready).

## State files

All files live in the autodrive state directory. They are written under `umask 077` with `mktemp` and `mv -f`, after a symlink check.

| File | Written by | Content |
| --- | --- | --- |
| `light-eligibility.json` | `light-eligibility`; rewritten by `autodrive_light_round.sh` after a light re-measurement | The tool's JSON line: decision, reason, base, head, measurements, limits. |
| `escalation.json` | `light-eligibility` or `autodrive_light_round.sh` | Reason, measurements, `base_sha`, `head_sha`. Present only after an escalation, and never removed by the recipe. |
| `light-mode` | `light-mode-marker`; rewritten or removed by `autodrive_light_round.sh` | The measured head SHA. Present only while the decision is `light`. |
| `<record>.concerns.tsv` | `autodrive_light_round.sh` | id, effective severity, summary. |
| `<record>.light.json` | `autodrive_light_round.sh` | `follow_up_issue` and `deferred_ids`. |
| `follow-up-issue` | `autodrive_light_round.sh` | The issue number. |
| `follow-up-linked` | `autodrive_light_round.sh` | Present once the PR comment is posted. |
| `follow-up-posted-ids` | `autodrive_light_round.sh` | One id per line. Lines that do not match `^[A-Za-z0-9._-]{1,64}$` are ignored when the file is read and dropped when it is next written. |
| `follow-up-failed` | `autodrive_light_round.sh` | A fixed token. No free text. |

Logs written by the light tools contain only counts, ids, digits and fixed tokens. `gh` stderr goes through `gh_sanitize_stderr`.

## Recursion limits

The recipe header sets `recursion.max_depth: 6` and `recursion.max_total_steps: 200`. The step budget allows for an escalation, where the nested `auto-drive-to-merge` run counts against the same total. There are no `timeout:` keys and no `default_step_timeout`.

## Security limits

- No step tells an agent to start an orchestrator or recipe. The guard in `issue_1333_leaf_steps_do_not_orchestrate.rs` covers this recipe.
- `--admin`, `--no-verify`, `SKIP=` and `gh pr merge` do not appear in the recipe or either tool. Merging happens only through the unchanged merge loop and gate.
- Bash steps read context only as `RECIPE_VAR_*` values and contain no `{{...}}` templates. Every value is validated before use.
- Agent text reaches `gh` only through files (`--body-file`).
- Judging agents run from the review directory. Round and loop recipes, and the light tools, are loaded from the tool root. `auto-drive-light.yaml` loads every tool, including `quick_change_common.sh`, only from `$AMPLIHACK_HOME/amplifier-bundle/tools`; `light-prereqs` checks that each one exists there, and a missing file stops the step rather than falling back to the worktree or another install. Inside `autodrive-crusty-round`, `autodrive_light_steps.sh` and `autodrive_light_round.sh` both come from `AUTODRIVE_LAUNCH_TOOLS`.
- Exit code 79 is final at every level and is never retried.
- `light-implement` and the crusty loop agents carry `[amplihack:no-merge]` in their prompts.

## Limitations

- **Measured on head change only.** Eligibility is measured when the PR is resolved and again in any crusty round whose reviewed head differs from the marker head. A run can therefore start in light mode and escalate part-way through the crusty loop or a merge-phase re-review, after some minor concerns have already been deferred to the follow-up issue. Those deferrals stand; later rounds address every concern.
- **Escalation is permanent.** Once `escalation.json` exists, the state directory never returns to light mode. A new `autodrive_state_dir`, or deleting the file by hand, is needed.
- **The marker persists.** If `auto-drive-to-merge` is later run by hand on a PR whose state directory still holds a valid `light-mode` marker, its crusty rounds apply light rules. An escalation always removes the marker and writes `escalation.json`, and the round cross-checks the marker against `light-eligibility.json`.
- **The marker can be forged by the same user.** The state directory is private, so only processes running as the same user can write a marker. Such a process could also write any other state file.
- **Existing branch without a PR.** `existing_branch` without an open PR is refused with `light-existing-branch-no-pr`.
- **Fork PRs.** PRs from another repository are refused with `light-cross-repo`.
- **`qa_command` runs PR code.** For a new change it runs the agent's own changes. Set it only to commands you would run on the change yourself.
- **The security word list is fixed.** A `minor` concern with a neutral id whose summary and evidence describe a security problem without using any word on the list is still deferred. The rule relies on crusty grading security concerns as `major` or `blocking`, and the word list is a second check, not a replacement.
- **Disclosure: deferred concerns are published.** The follow-up issue copies each deferred concern's summary and evidence verbatim. In a public repository this publishes them. A security concern that crusty graded `minor` and that the word list does not catch is therefore made public before it is fixed. Do not use `auto-drive-light` on a public repository where that is unacceptable.
- **The CLEAN is a rule, not a review.** A light CLEAN record means crusty raised only minor concerns, and they were deferred to an issue. The record's `verdict_source` and `deferred_ids` show this.
- **Branch prefix.** New changes use the `quick/` branch prefix from `quick_change_common.sh`. `auto-drive-light` has no equivalent of the `branch_prefix` key that `auto-drive-to-merge` offers (default `feat`). Repositories with other branch-naming conventions need `existing_branch` or `pr_number`.
- **Eligibility heuristics.** The public API rule reads `lib.rs` only, so a `pub` item added in another module and re-exported through an existing `pub use` is not counted. The recipe contract rule compares top-level `context:` keys only.
- **Inline `context:` mappings.** The recipe contract rule reads the keys of a block-style `context:` mapping. An inline mapping such as `context: { a: "1" }` is compared as written, so any change to it, including a changed default value, counts as a contract change. `context: {}` counts as no keys.
- **Re-measurement base.** After the head moves, the change is measured from the merge-base with the PR's current base tip. When that tip cannot be read, fetched or checked, the recorded base is used instead, and a head that merged the base branch then usually escalates with `in-loop-lines`.
- **Rejected side files leave `.findings` empty.** `autodrive_light_round.sh` empties `<record>.findings` before step-06 checks the side files. If step-06 rejects them, the round is recorded as `CONCERNS`, which blocks merge, but the address step of that round has no ids to work on. The next round reviews the PR again.

## Files

| Path | Kind | Purpose |
| --- | --- | --- |
| `amplifier-bundle/recipes/auto-drive-light.yaml` | recipe | The 17-step recipe described here. |
| `amplifier-bundle/tools/autodrive_light_eligibility.sh` | tool | Stateless eligibility measurement. |
| `amplifier-bundle/tools/autodrive_light_round.sh` | tool | Re-measurement on head change, the minor-only rule, the follow-up issue and side files. |
| `amplifier-bundle/tools/autodrive_light_steps.sh` | tool | The light branch of step-03 and step-06, sourced by the round recipe. Functions only. |
| `amplifier-bundle/recipes/autodrive-crusty-round.yaml` | recipe | Step-03 and step-06 carry the marker-gated light branch. No other step changes. |
| `amplifier-bundle/recipes/_recipe_manifest.json` | manifest | Hash entries for `auto-drive-light` and `autodrive-crusty-round`. |
| `crates/amplihack-cli/tests/issue_1802_auto_drive_light_eligibility.rs` | test | Eligibility rules, boundaries and faults. |
| `crates/amplihack-cli/tests/issue_1802_auto_drive_light_round.rs` | test | The minor-only rule, step-03 and step-06, and byte-identical output without the marker. |
| `crates/amplihack-cli/tests/issue_1802_auto_drive_light_recipe.rs` | test | Recipe structure, conditions and prohibited strings. |

## Related

- [Auto Drive To Merge Reference](auto-drive-to-merge.md): the full recipe, its loops and the merge gate.
- [Quick-Change Recipe](quick-change-recipe.md): the single-session recipe that stops at an open PR.
- [Recipe Runner](../recipes/README.md#choosing-between-quick-change-auto-drive-light-and-auto-drive-to-merge): choosing between the three recipes.
- [Structured Verdict & Intent Parsing Reference](structured-verdict-parsing.md): how step-03 parses crusty's output.
