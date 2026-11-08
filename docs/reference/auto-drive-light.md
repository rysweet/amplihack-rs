---
title: Auto-Drive-Light Recipe
description: The smaller drive-to-merge recipe for small pull requests, its eligibility rules, escalation to auto-drive-to-merge, the task id its follow-up issues name, and how it shares auto-drive-to-merge's per-finding follow-up model.
last_updated: 2026-11-07
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-Drive-Light Recipe

> [Home](../index.md) > Reference > Auto-Drive-Light

`auto-drive-light` builds a small change in one agent session, opens a pull request, and drives that PR to merge through the same crusty loop, merge loop and evidence gate as [`auto-drive-to-merge`](auto-drive-to-merge.md). It has no workflow-design, consensus-design or architect steps and never calls `default-workflow`.

A bash step measures the PR after it is opened. If the change exceeds any eligibility rule, the run hands the same PR to `auto-drive-to-merge`. Minor review findings are handled exactly as in `auto-drive-to-merge`: a crusty round whose findings are all minor ends the crusty loop, and each finding is filed as its own follow-up issue, linked to the task, before the PR can merge.

The recipe is `amplifier-bundle/recipes/auto-drive-light.yaml`. It was added for issue #1802. Issue #1982 moved its minor findings to the follow-up model of #1979.

## Contents

- [When to use it](#when-to-use-it)
- [Running it](#running-it)
- [Context variables](#context-variables)
- [Steps](#steps)
- [Eligibility rules](#eligibility-rules)
- [The eligibility tool](#the-eligibility-tool)
- [Escalation to auto-drive-to-merge](#escalation-to-auto-drive-to-merge)
- [The light-mode marker](#the-light-mode-marker)
- [The round tool](#the-round-tool)
- [Minor findings and follow-up issues](#minor-findings-and-follow-up-issues)
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

A new change for an existing issue. The issue number is the task id that any follow-up issue names:

```bash
amplihack recipe run auto-drive-light \
  -c task_description="Return an error instead of panicking when the round record is empty in autodrive_last_round.sh" \
  -c issue_number=1846 \
  -c repo_path=.
```

A PR that already exists, with a plan task id:

```bash
amplihack recipe run auto-drive-light \
  -c pr_number=1850 \
  -c plan_task_id=plan/17.104 \
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
| `issue_number` | `""` | Optional. 1 to 10 digits. The PR body gets `Refs #N` and the branch name uses it. When `plan_task_id` is empty, `#N` is the task id for follow-up issues. See [The task id](#the-task-id). |
| `plan_task_id` | `""` | Optional. The plan task that follow-up issues name: `N`, `#N`, or 1 to 64 characters from `A-Za-z0-9._:/-`. Checked by `light-prereqs`, which exits 2 on any other value. |
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

The recipe also fills these values as it runs: `light_base`, `light_task_id`, `build_result`, `light_eligibility`, `light_escalation_result`, `crusty_loop_result`, `merge_gate_result` and `light_summary`.

## Steps

| # | Step | Kind | Runs when |
| --- | --- | --- | --- |
| 1 | `light-prereqs` | bash | always |
| 2 | `light-pr-input` | bash | always |
| 3 | `light-task-id` | bash | always |
| 4 | `light-worktree` | recipe `workflow-worktree` | new change |
| 5 | `light-base-pin` | bash | new change |
| 6 | `light-implement` | agent | new change |
| 7 | `light-qa` | bash | new change |
| 8 | `light-precommit` | bash | new change |
| 9 | `light-commit-push` | bash | new change |
| 10 | `light-open-pr` | bash | new change |
| 11 | `light-resolve-pr` | bash | always |
| 12 | `light-eligibility` | bash | always |
| 13 | `light-escalate` | recipe `auto-drive-to-merge` | decision is `escalate` |
| 14 | `light-mode-marker` | bash | decision is `light` |
| 15 | `light-crusty-loop` | recipe `autodrive-crusty-loop` | decision is `light` |
| 16 | `light-check-unmerged` | bash | decision is `light` |
| 17 | `light-merge-loop` | recipe `autodrive-merge-loop` | decision is `light` |
| 18 | `light-report` | bash | always |

"New change" means neither `pr_number` nor `existing_branch` was given.

**Prerequisites (steps 1 to 3).** `light-prereqs` checks that every tool the run loads exists under `$AMPLIHACK_HOME/amplifier-bundle/tools`. The list includes the light tools, `autodrive_finding_rules.sh`, and the follow-up tools `autodrive_followups.sh`, `autodrive_followups_check.sh` and `workflow_issue_tracking.sh`, which the loops load from the same root. It also validates `plan_task_id`. `light-task-id` works out the effective task id; see [The task id](#the-task-id).

**Build (steps 4 to 10).** The worktree and branch come from `workflow-worktree` and `quick_change_common.sh`, so the branch is named `quick/<issue-or-slug>`. `light-base-pin` records the base commit before the agent runs; that commit is `light_base`. `light-implement` is a single agent session whose prompt carries the quick-change leaf-step constraints and the line `[amplihack:no-merge]`. It never tells the agent to start an orchestrator. `light-qa` runs `qa_command` or `quick_change_checks.sh`; `light-precommit` runs the repository's pre-commit hooks. A failing check stops the run before anything is pushed.

**PR resolution (step 11).** `light-resolve-pr` writes `build_result` in the same shape as `autodrive-build`, with `pr`, `branch`, `head_sha`, `base_sha` and `state_dir`. For an existing PR it reads `gh pr view` and refuses a PR whose `isCrossRepository` is true (`light-cross-repo`). For an existing PR, `light_base` is the merge base of the PR head and `origin/<baseRefName>`. PR code fetched as `refs/pull/<N>/head` is not checked out, sourced or run before review.

**Light path (steps 14 to 17).** `light-mode-marker` is described in [The light-mode marker](#the-light-mode-marker). `light-crusty-loop` and `light-merge-loop` run the unchanged `autodrive-crusty-loop` and `autodrive-merge-loop` sub-recipes that `auto-drive-to-merge` uses, on the same state directory, with `plan_task_id` set to the effective task id. There is no `autodrive-check-unmerged` recipe: in `auto-drive-to-merge.yaml` that check is an inline bash step. `light-check-unmerged` is a bash step with the same body. It reads `build_result`, sources `autodrive_state.sh` from `AMPLIHACK_HOME` and calls `autodrive_require_unmerged`, so a PR merged outside the gate during the crusty loop stops the run with exit 1 (#1551).

**Report (step 18).** `light-report` prints which path ran (`light`, `escalate` or `light-escalated`), the eligibility or escalation reason, the follow-up issues filed for the PR, and the merge gate result:

```text
AUTO_DRIVE_LIGHT: MERGED - PR #1850 (path: light, reason: within-limits, follow-up issues: #1851,#1852, merge gate: MERGED)
```

Its output, `light_summary`, is one JSON line:

```json
{"outcome":"MERGED","pr":"1850","path":"light","reason":"within-limits","follow_up_issues":"#1851,#1852","merge_result":"MERGED"}
```

The recipe's markdown report shows the list in a "Follow-up issues" row. The list comes from column 2 of `<state>/followups.tsv`, which is read only when it is a regular file, not a symlink, and owned by the current user. Values that do not match `^[0-9]{1,10}$` are dropped, a repeated number is listed once, and the numbers keep the order of the rows. When the file is absent, unsafe or holds no valid number, the value is `none`. The report does not read any other follow-up file.

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
- Inside the loops, a crusty round whose verdict is `CONCERNS` and that finds a valid marker compares the head it reviewed with the marker head. When they differ, `autodrive_light_round.sh check` measures again on `<base>..<reviewed head>`, taking the base and limits from `light-eligibility.json`. This covers commits made by crusty address steps, commits pushed by anyone else, and crusty re-reviews in the merge phase. A `CLEAN` round is not measured again. See [Escalation inside the loop](#escalation-inside-the-loop).

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

`light-escalate` is a `type: recipe` step that runs `auto-drive-to-merge` with `pr_number` taken from `build_result`, `existing_branch` empty, `autodrive_state_dir` set to the same state directory, and `plan_task_id` set to the effective task id. `task_description`, `repo_path`, `autodrive_dry_run` and `no_merge` are passed unchanged. Only the PR number is passed because `autodrive-build` uses `pr_number` whenever it is set and looks a branch up only when it is empty. Because the PR already exists, the nested build phase is a no-op: no second branch, build or PR is created. The nested recipe then runs its full crusty loop, merge loop and gate, and files follow-up issues in the same way as a light run.

Every later light step has a condition on `light_eligibility.decision == 'light'` and is skipped. `light-escalate` is the only step in the recipe that names `auto-drive-to-merge`. Exit code 79 from the nested run is passed through and never retried.

## The light-mode marker

`light-mode-marker` writes `<state>/light-mode` after a `light` decision, and only then. The file holds one line, the head SHA that was measured. It is created under `umask 077` through `mktemp` and `mv -f`.

The marker has one effect: it makes a crusty round measure a moved head again, and escalate when the change is no longer light. It never changes a round's verdict, its findings or its count.

`autodrive-crusty-round` looks for the marker in `$(dirname "$AUTODRIVE_ROUND_RECORD")`. No context key is added to `autodrive-crusty-loop`. Crusty re-reviews in the merge phase (`autodrive_crusty_rereview.sh`) use the same state directory, so they measure a moved head too.

The round tool accepts the marker only when all of these hold:

- it is a regular file, not a symlink, and private to the current user;
- its content is a 40 or 64 character lowercase hex SHA;
- `light-eligibility.json` is a private regular file with `decision` `light` and the same head;
- `escalation.json` is absent.

A marker that fails these checks gives `marker-invalid` or `state-unsafe`. A reviewed head that does not descend from the marker head gives `marker-stale`. In every one of those cases the tool prints `off` and changes nothing. A reviewed head that descends from the marker head but differs from it is measured again, as described below.

### Step-03 of the crusty round

Step-03 of `autodrive-crusty-round` writes the verdict, the parsed concerns and `<record>.findings` first. Then, only when the marker is a regular file, it sources `amplifier-bundle/tools/autodrive_light_steps.sh` from `AUTODRIVE_LAUNCH_TOOLS`, the tool root that step-00 has already checked (#1543), and calls `autodrive_light_step03`. That function:

1. returns at once unless the verdict is `CONCERNS`, its `verdict_source` is `crusty` and the round record path is set;
2. runs `autodrive_light_round.sh check` from the same root, with stdout sent to `/dev/null`;
3. returns 79 when the tool exits 79;
4. on any other non-zero exit, prints `WARNING: autodrive_light_round.sh check exited <rc>; the round is classified without re-measurement.` on stderr and returns 0.

It never assigns `VERDICT`, `SOURCE` or `COUNT`. Step-03 then runs the #1979 `classify` branch unchanged, so a light round whose findings are all minor becomes `MINOR_ONLY`; see [Minor findings and follow-up issues](#minor-findings-and-follow-up-issues).

If `autodrive_light_steps.sh` cannot be loaded, step-03 prints `WARNING: autodrive_light_steps.sh not found under AUTODRIVE_LAUNCH_TOOLS; the moved head is not re-measured.` and continues to `classify`. Exit 79 from the helper is passed through. Step-04, step-05 and step-06 have no light branch.

Without the marker, `autodrive-crusty-round` behaves exactly as it does for `auto-drive-to-merge`: the same steps run, step-03 and step-06 write the same bytes for `CLEAN`, `CONCERNS` and `MINOR_ONLY` rounds, and neither light tool is loaded.

### Escalation inside the loop

When the reviewed head differs from the marker head, `autodrive_light_round.sh check` runs `autodrive_light_eligibility.sh` on `<base>..<reviewed head>` with the limits recorded in `light-eligibility.json`.

The base is the merge-base of the reviewed head and the PR's current base tip, so a head that merged the base branch in (the usual fix for a branch that is behind) is measured on the PR's own change only, as GitHub shows it. The tool reads `baseRefName` and `baseRefOid` with `gh pr view`, checks both, fetches `refs/heads/<baseRefName>` from `origin` if the tip is not present locally, and uses the merge-base only when it descends from the base recorded in `light-eligibility.json`. In every other case (a `gh` failure, an invalid name or SHA, a failed fetch, or a merge-base that does not descend from the recorded base) it logs one `INFO:` line and measures from the recorded base. For a failed fetch, that line includes the first line of the `git fetch` error, cut to 200 characters. That measurement counts any upstream changes the head has merged, so it can only escalate more often. Exit code 79 from `gh` is passed through.

- **`light`.** The tool rewrites `light-eligibility.json` with the new head and the base it measured from, then rewrites `light-mode` with the same head, and prints `{"light_exit":"light"}`.
- **`escalate` or a fault.** The tool removes `light-mode`, writes `escalation.json` with reason `in-loop-<reason>` or `in-loop-fault-<token>`, and prints `{"light_exit":"escalated","reason":"in-loop-lines"}` (with the matching reason).

Either way the round is then classified like any other round. After an escalation inside the loop, every later round in that state directory, including merge-phase re-reviews, finds no marker and is not measured again. The run stays in the same crusty and merge loops; it does not start a nested `auto-drive-to-merge`, because those loops are the ones that recipe would run. `light-report` prints the path as `light-escalated` with the recorded reason.

## The round tool

`amplifier-bundle/tools/autodrive_light_round.sh` has one command:

```text
autodrive_light_round.sh check --record <abs path> --pr <N>
    --reviewed-head <sha> --repo <dir>
```

| Option | Value |
| --- | --- |
| `--record` | The round record path. The state directory is its directory. |
| `--pr` | The PR number, 1 to 10 digits. |
| `--reviewed-head` | The head crusty reviewed, a 40 or 64 character lowercase hex SHA. |
| `--repo` | The worktree used for `git` and `gh`. |

`check` reads no stdin. It never writes, truncates or reads `<record>.findings`, and writes no file next to the round record. The only files it writes are `light-eligibility.json`, `light-mode` and `escalation.json`, as described in [Escalation inside the loop](#escalation-inside-the-loop).

Stdout is exactly one JSON line:

| `light_exit` | `reason` | Meaning |
| --- | --- | --- |
| `off` | absent | No marker. |
| `off` | `marker-invalid`, `marker-stale` or `state-unsafe` | The marker was not accepted. Nothing was measured or written. |
| `light` | absent | The marker is valid, and the head is unchanged or was measured again within the limits. |
| `escalated` | `in-loop-<rule>` or `in-loop-fault-<token>` | The head was measured again and is no longer light. The marker is removed and `escalation.json` is written. |

The escalation reason always matches `^in-loop-[a-z][a-z-]{0,54}$`. Any other value is replaced by `in-loop-fault-unknown` before it is written or printed, so stdout and `escalation.json` hold only fixed tokens, SHAs and digits.

| Exit | Meaning |
| --- | --- |
| `0` | A decision was printed. |
| `2` | Usage error: an unknown command (including the retired `defer`), a missing or invalid option. Stdout is empty; one `ERROR:` line goes to stderr. |
| `79` | `gh` exited 79. The code is final and is passed through, never retried. |

```bash
bash "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_light_round.sh" check \
  --record "$STATE/crusty-round-2.json" --pr 1850 \
  --reviewed-head 4e1d2c3b4a5f60718293a4b5c6d7e8f901a2b3c4 --repo "$WORKTREE"
# {"light_exit":"light"}
```

The tool loads only `workflow_gh_retry.sh`, from its own directory, for its `gh` calls. It does not load `autodrive_finding_rules.sh` or `workflow_issue_tracking.sh`, handles no finding text, and makes no `gh issue` or `gh pr comment` call.

## Minor findings and follow-up issues

Light mode has no minor-finding rule of its own. A light drive classifies, collects, files and gates minor findings through `autodrive_followups.sh`, exactly as `auto-drive-to-merge` does:

- crusty-round step-03 runs `classify`, so a `CONCERNS` round whose findings are all minor is recorded as `MINOR_ONLY`, with `verdict_source` `crusty` and `concern_count` equal to the number of lines in `<record>.minor`;
- crusty-loop step-03 and merge-loop step-02b run `collect` and `file`, which create one issue per finding and one comment on the parent task;
- the merge gate runs `gate`, which blocks a `MINOR_ONLY` drive until every follow-up is filed.

The severity rules, the recurrence rule, the word lists, the issue format, the parent comment, the state files and the failure tokens are all documented in [Minor findings end the crusty loop (#1979)](auto-drive-to-merge.md#minor-findings-end-the-crusty-loop-1979). A light drive that escalates part-way therefore produces follow-ups of the same shape as one that does not.

Issue #1982 asked whether light mode should keep its own deferral (one issue per drive, filed inside the round, which turned the round `CLEAN`) or use the per-finding model of #1979. Light mode now uses the per-finding model, so one drive produces one shape of follow-up, whichever recipe ran it. The light-mode marker no longer affects the verdict; only `classify` decides whether a round is minor-only.

A round record written by an earlier version, which begins `{"crusty_verdict":"CLEAN",` and has `verdict_source` `light-minor-rule`, is still read as `CLEAN`; there is no code that treats it differently.

### The task id

Every follow-up issue names a plan task. `light-task-id` works out the effective task id once and passes it as `plan_task_id` to `light-escalate`, `light-crusty-loop` and `light-merge-loop`:

1. `plan_task_id`, when it is set;
2. otherwise `#<issue_number>`, when `issue_number` matches `^0*[1-9][0-9]{0,9}$`;
3. otherwise empty.

| `plan_task_id` | `issue_number` | Effective task id | Follow-up issues name |
| --- | --- | --- | --- |
| `plan/17.104` | `1846` | `plan/17.104` | `plan/17.104` |
| `1846` | `""` | `1846` | `#1846` |
| `""` | `1846` | `#1846` | `#1846` |
| `""` | `007` | `#007` | `#7` |
| `""` | `0` or `""` | empty | the first `Closes`, `Fixes` or `Resolves #N` in the PR body, then in `task_description` |

When the effective task id is empty, `autodrive_followups.sh file` resolves the task in its usual order. The PR body that `light-open-pr` writes says `Refs #N`, which that fallback does not match, so the issue number reaches `file` only through `issue_number` or `plan_task_id`.

A non-empty `plan_task_id` must satisfy the rule of `resolve_task` in `autodrive_followups.sh`: digits with an optional leading `#` that name an issue from 1 to 10 digits, or 1 to 64 characters from `A-Za-z0-9._:/-`. `light-prereqs` checks this before any work starts, and `light-task-id` checks it again. Either step exits 2 on any other value, such as `#0` or `task 17`, with:

```text
ERROR: auto-drive-light: plan_task_id must be #N or 1 to 64 characters from A-Za-z0-9._:/-
```

The value itself is never logged. `light-task-id` prints the effective task id as a plain string, `light_task_id`.

A drive that ends `MINOR_ONLY` with no task id that resolves stops with `followup-task-unresolved` before the merge gate. A drive that ends `CLEAN` needs no task id.

## Merge criteria

The merge criteria are those of `auto-drive-to-merge`, unchanged. `autodrive-merge-loop`, `autodrive-merge-round`, `autodrive-merge-evidence` and `autodrive_merge_gate.sh` are the same files, and the gate has no light exemption. A light run merges only through the gate, which requires:

- the crusty loop to have ended DONE with a latest record that is `CLEAN`, or `MINOR_ONLY` with every follow-up filed and the parent task comment recorded (`autodrive_followups.sh check`; see [The follow-up check in the merge gate](auto-drive-to-merge.md#the-follow-up-check-in-the-merge-gate));
- qa-team scenarios;
- green CI;
- no conflicts;
- resolved review threads.

See [How the merge round applies merge-ready](auto-drive-to-merge.md#how-the-merge-round-applies-merge-ready).

## State files

All files live in the autodrive state directory. They are written under `umask 077` with `mktemp` and `mv -f`, after a symlink check.

| File | Written by | Content |
| --- | --- | --- |
| `light-eligibility.json` | `light-eligibility`; rewritten by `autodrive_light_round.sh check` after a light re-measurement | The tool's JSON line: decision, reason, base, head, measurements, limits. |
| `escalation.json` | `light-eligibility` or `autodrive_light_round.sh check` | Reason, measurements, `base_sha`, `head_sha`. Present only after an escalation, and never removed by the recipe. |
| `light-mode` | `light-mode-marker`; rewritten or removed by `autodrive_light_round.sh check` | The measured head SHA. Present only while the decision is `light`. |

The follow-up files (`<record>.minor`, `<record>.severity`, `followups.jsonl`, `followups.tsv`, `followups.error` and `followups.parent`) are those of [auto-drive-to-merge's state files](auto-drive-to-merge.md#state-files). `light-report` reads `followups.tsv` and no other follow-up file.

Files left by an earlier version in a reused state directory (`follow-up-issue`, `follow-up-linked`, `follow-up-posted-ids`, `follow-up-failed`, `<record>.concerns.tsv` and `<record>.light.json`) are ignored. Nothing reads them and nothing migrates them.

Logs written by the light tools contain only fixed tokens, SHAs and digits. `gh` stderr goes through `gh_sanitize_stderr`.

## Recursion limits

The recipe header sets `recursion.max_depth: 6` and `recursion.max_total_steps: 200`. The step budget allows for an escalation, where the nested `auto-drive-to-merge` run counts against the same total. There are no `timeout:` keys and no `default_step_timeout`.

## Security limits

- No step tells an agent to start an orchestrator or recipe. The guard in `issue_1333_leaf_steps_do_not_orchestrate.rs` covers this recipe.
- `--admin`, `--no-verify`, `SKIP=` and `gh pr merge` do not appear in the recipe or either light tool. Merging happens only through the unchanged merge loop and gate.
- Bash steps read context only as `RECIPE_VAR_*` values and contain no `{{...}}` templates. Every value is validated before use.
- `plan_task_id` is validated in `light-prereqs` and again in `light-task-id`, and `file` validates the forwarded value a third time. Its character set contains no quote, brace, newline or `$`, so it cannot inject template or YAML text into the context of the three child recipes.
- The light tools make one GitHub call, the read-only `gh pr view --json baseRefName,baseRefOid` used for re-measurement. They create no issue and post no comment. `autodrive_followups.sh file` is the only code that writes follow-up issues and parent comments.
- Agent text reaches `gh` only through files (`--body-file`), after `cell()`; see [Cell sanitising](auto-drive-to-merge.md#cell-sanitising).
- Judging agents run from the review directory. Round and loop recipes, and the light tools, are loaded from the tool root. `auto-drive-light.yaml` loads every tool, including `quick_change_common.sh`, only from `$AMPLIHACK_HOME/amplifier-bundle/tools`; `light-prereqs` checks that each one exists there, and a missing file stops the step rather than falling back to the worktree or another install. Inside `autodrive-crusty-round`, `autodrive_light_steps.sh` and `autodrive_light_round.sh` both come from `AUTODRIVE_LAUNCH_TOOLS`. `autodrive_light_round.sh` sources `workflow_gh_retry.sh` from its own directory only.
- Exit code 79 is final at every level and is never retried.
- `light-implement` and the crusty loop agents carry `[amplihack:no-merge]` in their prompts.

## Limitations

- **Measured on head change only.** Eligibility is measured when the PR is resolved and again in any `CONCERNS` crusty round whose reviewed head differs from the marker head. A run can therefore start in light mode and escalate part-way through the crusty loop or a merge-phase re-review.
- **Follow-ups come from the last round only.** As in `auto-drive-to-merge`, `collect` reads only the round that ended the loop. Minor findings raised in rounds before an escalation, or in any earlier round, are filed only when the final record is `MINOR_ONLY` and still lists them.
- **Filing happens only after DONE.** Follow-up issues are filed when the crusty loop ends DONE, and again by merge-loop step-02b. A run that stops earlier, for example `STUCK` or `BUDGET_EXHAUSTED`, files none, and a dry run files none.
- **A task id is needed for minor findings.** A drive that ends `MINOR_ONLY` with no `plan_task_id`, no `issue_number` and no `Closes`, `Fixes` or `Resolves #N` in the PR body or `task_description` stops with `followup-task-unresolved` and does not merge.
- **The parent comment says the PR closes the issue.** The comment that `file` posts on the task issue says "Merging that PR closes this issue". A light PR's body says `Refs #N`, so merging it does not close the issue; the comment is inaccurate for light runs. The comment format is shared with `auto-drive-to-merge` and is not changed for light mode.
- **Escalation is permanent.** Once `escalation.json` exists, the state directory never returns to light mode. A new `autodrive_state_dir`, or deleting the file by hand, is needed.
- **The marker persists.** If `auto-drive-to-merge` is later run by hand on a PR whose state directory still holds a valid `light-mode` marker, its `CONCERNS` rounds measure a moved head again and may escalate. The marker never changes a verdict. An escalation always removes the marker and writes `escalation.json`, and the round cross-checks the marker against `light-eligibility.json`.
- **The marker can be forged by the same user.** The state directory is private, so only processes running as the same user can write a marker. Such a process could also write any other state file.
- **Existing branch without a PR.** `existing_branch` without an open PR is refused with `light-existing-branch-no-pr`.
- **Fork PRs.** PRs from another repository are refused with `light-cross-repo`.
- **`qa_command` runs PR code.** For a new change it runs the agent's own changes. Set it only to commands you would run on the change yourself.
- **The security word list is fixed.** A `minor` finding with a neutral id whose summary and evidence describe a security problem without using any word on the list is still filed as a follow-up issue rather than fixed. The rule relies on crusty grading security findings as `major` or `blocking`, and the word list is a second check, not a replacement.
- **Disclosure: minor findings are published.** Each follow-up issue copies the finding's sanitised summary and evidence, up to 200 characters each. In a public repository this publishes them. A security finding that crusty graded `minor` and that the word list does not catch is therefore made public before it is fixed. Do not use `auto-drive-light` on a public repository where that is unacceptable.
- **A task id may look like a URL.** `plan_task_id` may contain `/` and `:`, so a value such as `https://example.org/t/1` is accepted and shown as `Task: https://example.org/t/1` in each follow-up issue. It cannot close the issue's HTML comment marker, because `>` is not allowed.
- **Branch prefix.** New changes use the `quick/` branch prefix from `quick_change_common.sh`. `auto-drive-light` has no equivalent of the `branch_prefix` key that `auto-drive-to-merge` offers (default `feat`). Repositories with other branch-naming conventions need `existing_branch` or `pr_number`.
- **Eligibility heuristics.** The public API rule reads `lib.rs` only, so a `pub` item added in another module and re-exported through an existing `pub use` is not counted. The recipe contract rule compares top-level `context:` keys only.
- **Inline `context:` mappings.** The recipe contract rule reads the keys of a block-style `context:` mapping. An inline mapping such as `context: { a: "1" }` is compared as written, so any change to it, including a changed default value, counts as a contract change. `context: {}` counts as no keys.
- **Re-measurement base.** After the head moves, the change is measured from the merge-base with the PR's current base tip. When that tip cannot be read, fetched or checked, the recorded base is used instead, and a head that merged the base branch then usually escalates with `in-loop-lines`.

## Files

| Path | Kind | Purpose |
| --- | --- | --- |
| `amplifier-bundle/recipes/auto-drive-light.yaml` | recipe | The 18-step recipe described here. |
| `amplifier-bundle/tools/autodrive_light_eligibility.sh` | tool | Stateless eligibility measurement. |
| `amplifier-bundle/tools/autodrive_light_round.sh` | tool | `check`: marker validation, and re-measurement or escalation when the head has moved. Sources `workflow_gh_retry.sh` only. |
| `amplifier-bundle/tools/autodrive_light_steps.sh` | tool | `autodrive_light_step03`, the light branch of crusty-round step-03, sourced by the round recipe. Functions only. |
| `amplifier-bundle/tools/autodrive_followups.sh`, `autodrive_followups_check.sh` | tool | `classify`, `collect`, `file`, `check` and `gate`: the only follow-up path, shared with `auto-drive-to-merge` (#1979). Checked by `light-prereqs`. |
| `amplifier-bundle/tools/autodrive_finding_rules.sh` | tool | Sourced only. The security word lists, `cell()` and the minor-finding predicate, used by `autodrive_followups.sh`. Checked by `light-prereqs`. |
| `amplifier-bundle/recipes/autodrive-crusty-round.yaml` | recipe | Step-03 calls `autodrive_light_step03` when the marker is present, then runs `autodrive_followups.sh classify` on a crusty `CONCERNS` verdict. Step-06 has no light branch. |
| `amplifier-bundle/recipes/_recipe_manifest.json` | manifest | Hash entries for `auto-drive-light` and `autodrive-crusty-round`. |
| `crates/amplihack-cli/tests/issue_1802_auto_drive_light_eligibility.rs` | test | Eligibility rules, boundaries and faults. |
| `crates/amplihack-cli/tests/issue_1802_auto_drive_light_round.rs` | test | `check` and its marker, staleness, re-measurement, escalation and exit-79 cases; step-03 leaving the verdict unchanged; byte-identical output without the marker; an end-to-end light round through the real follow-up tools with a `gh` stub that fails if step-03 calls it. |
| `crates/amplihack-cli/tests/issue_1802_auto_drive_light_recipe.rs` | test | Recipe structure, conditions, `plan_task_id` validation and forwarding, the report, and prohibited strings. |

## Related

- [Auto Drive To Merge Reference](auto-drive-to-merge.md): the full recipe, its loops, the follow-up issues and the merge gate.
- [Quick-Change Recipe](quick-change-recipe.md): the single-session recipe that stops at an open PR.
- [Recipe Runner](../recipes/README.md#choosing-between-quick-change-auto-drive-light-and-auto-drive-to-merge): choosing between the three recipes.
- [Structured Verdict & Intent Parsing Reference](structured-verdict-parsing.md): how step-03 parses crusty's output.
