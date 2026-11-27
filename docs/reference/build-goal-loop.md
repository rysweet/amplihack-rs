---
title: Build Goal Loop
description: How default-workflow builds a change and opens its pull request as one goal-driven agent loop. Inputs, outputs, the reading, actions, pull request text, claims, host inputs, run endings, behaviour changes and the release evidence: start kinds, runs in flight and rollback.
last_updated: 2026-11-24
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Build Goal Loop

`default-workflow` builds a change and opens its pull request by running the
shared goal loop described in [Auto-drive Goal Loop](./autodrive-goal-loop.md).
The goal is "build this change and open its pull request". Each turn, an agent
reads a fresh reading of the live state and the task text, then chooses the
next action. Fixed code reads state, commits, pushes, opens and edits the pull
request, and waits for an event. It does not decide that a run ends because of
how the run is going.

This page is the specification for the build side of issue #3129. It closes
#3110, #3102 and #2787. Every caller of `default-workflow` gets the loop:
`smart-execute-routing`, `autodrive-build`, `oxidizer-workflow`,
`quality-loop`, the hand-over from `quick-change`, and `multitask`.

## Contents

- [How a build run goes](#how-a-build-run-goes)
- [Inputs and outputs](#inputs-and-outputs)
- [The reading](#the-reading)
- [Actions](#actions)
- [Pull request text](#pull-request-text)
- [Claims on the issue](#claims-on-the-issue)
- [Host inputs](#host-inputs)
- [Checks already passed](#checks-already-passed)
- [Base merges](#base-merges)
- [Guidance and required outcomes](#guidance-and-required-outcomes)
- [How a build ends](#how-a-build-ends)
- [Quick-change](#quick-change)
- [Changes to tools used by older recipes](#changes-to-tools-used-by-older-recipes)
- [Behaviour changes](#behaviour-changes)
- [Security notes](#security-notes)
- [Notes for the shared loop](#notes-for-the-shared-loop)
- [Release evidence](#release-evidence)
- [Tests](#tests)

## How a build run goes

`default-workflow` version 4.0.0 has five build steps, each with
`recovery: {replay: never}`, and seven Azure DevOps steps that are described
under [Azure DevOps builds](#azure-devops-builds). The build steps run when
`remote_host_type` is `github` or `other`, or empty:

| Step | What it does |
| --- | --- |
| `workflow-prep` | The existing recipe, unchanged. Chooses the tracking issue and runs the claim check as information. |
| `workflow-worktree` | The existing recipe. Step 04 creates the worktree and, for a new branch, links host inputs into it. |
| `build-goal-setup` | Runs `autodrive_build_run.sh setup`. Creates the state folder, records the run's branch and worktree, and links host inputs into the worktree, which also covers a run on an existing branch. |
| `build-loop` | Runs `autodrive_build_run.sh loop`, which calls the shared loop's driver, `tools/autodrive_goal_loop.sh`, the way the `autodrive-goal-loop` recipe's own step calls it, with `autodrive_build_goal.sh` as the goal tool and `skills/default-workflow/goal-build-pr.md` as the goal text. Each turn is then one `amplihack recipe run autodrive-goal-turn` from this step's depth, as under `auto-drive-to-merge`. Output: `build_loop_result`, the loop's end line from `loop-end.json`. A nested run of the loop recipe would put each turn's agent past the session-depth ceiling, and a sub-recipe step would merge the loop's context, `autodrive_state_dir` included, back into the run and its callers. |
| `build-report` | Runs `autodrive_build_run.sh report`. Non-fatal, and it runs once, after the loop's last action. Writes `workflow_result` from the loop's end, the pull request's live state and the outcome ledger. The run makes no file outside its state folder, so there is nothing else to remove. |

The agent in each turn works on the branch the run created. The worktree and
branch are never removed by the build run, because callers such as
`autodrive-build` and continuation runs use them after `default-workflow`
returns. This closes the cleanup-order failure in which a cleanup step removed
the worktree that a later step needed.

The phase recipes (`workflow-design`, `workflow-tdd`, `workflow-publish`,
`workflow-finalize` and the others) are called only on the Azure DevOps route
(see [Azure DevOps builds](#azure-devops-builds)). They stay for one release
with the same names, inputs and step ids, because a 1.2.213 run that has
already loaded the old `default-workflow` calls them by name at step time.

## Inputs and outputs

`default-workflow` accepts every context key it accepted in 1.2.213. Keys the
loop does not use are accepted and ignored for one release, so older launch
commands keep working. The ignored keys are `no_merge` and
`finalization_handoff`, together with any other 1.2.213 key that the table
below does not list.

| Key | Use in the loop |
| --- | --- |
| `task_description`, `repo_path`, `step_recovery_dir` | Passed to the loop |
| `worktree_setup` | Read by `build-goal-setup` for the branch and worktree |
| `base_branch` | The base for `open-pr` and `merge-base` |
| `existing_branch`, `allow_existing_pr`, `pr_number` | Shown in the reading and used to find the run's own pull request |
| `issue_number`, `closing_reference` | Shown in the reading and allowed in pull request text |
| `checks_watch_interval` | Seconds between CI reads during `wait`. Defaults to 120. See [CI check interval](./autodrive-goal-loop.md#ci-check-interval). |
| `strict_delivery`, `allow_no_op` | Passed through |
| `no_merge`, `finalization_handoff` | Accepted and ignored. A build never merges, and the run always ends with the pull request open (see [Behaviour changes](#behaviour-changes)). |

Outputs:

| Output | Content |
| --- | --- |
| `worktree_setup` | Unchanged from `workflow-worktree` |
| `build_goal_setup` | One JSON line: `schema_version`, `state_dir`, `branch`, `worktree`, `source`, `fallback` |
| `build_loop_result` | The content of the loop's `loop-end.json` |
| `workflow_result` | Schema 1. Empty only if `build-report` itself fails, because that step is non-fatal. See below |

`workflow_result` fields: `end` (`ready`, `merged`, `closed`, `handoff`,
`stopped` or `none`), `detail`, `turn`, `pr_number`, `pr_url`, `pr_state` and
`pr_draft` as read from GitHub, `terminal_state`, `terminal_success`,
`terminal_reason`, `handoff` (always `"false"`), `state_dir`, `outcomes` (see
[Guidance and required outcomes](#guidance-and-required-outcomes)), and
`errors`, which lists each read that failed. A failed read is recorded in `errors` and
never changes the end. A GitHub rate-limit exit (79) is recorded the same way.

`terminal_state` keeps the vocabulary that `autodrive-build` step 03 reads
through `workflow_finalization_handoff.sh`:

| Live state and loop end | `terminal_state` | `terminal_success` |
| --- | --- | --- |
| The pull request is merged | `MERGED` | `"true"` |
| Open and not a draft, loop end `ready` | `IMPLEMENTED_VERIFIED` | `"true"` |
| Open as a draft, loop end `ready` | `EXISTING_OPEN_PR` | `"true"` |
| Closed without a merge | `FAILED_CLOSED_UNMERGED` | `"false"` |
| Anything else: a hand-off, a stop, no pull request | `INCOMPLETE` | `"false"` |

Readers of these outputs (`autodrive-build` step 03, `smart-execute-routing`,
`oxidizer-workflow`, `quality-loop` and `multitask`) keep the names and shapes
they read in 1.2.213.

### State folder

Each invocation of `default-workflow` gets a fresh private folder made with
`mktemp -d` (mode 0700, owner checked) under the first usable parent of:

1. `$AMPLIHACK_RUN_DIR/build-goal`
2. `<worktree_setup.runtime_root>/build-goal`, only when it is absolute and
   outside the worktree
3. `${TMPDIR:-/tmp}`

A parent that is relative, contains `..`, is a symlink, belongs to another user
or is world-writable without the sticky bit is skipped. The folder holds
`identity.json` (branch, worktree and where they came from), `pr.json`,
`pr-text/` (the latest intended title and body), `checks.json` and the turn
records. State files are a cache. They are never an authority for a write to
GitHub, which always checks live state first.

The run makes no file outside this folder. Each reading and each action keeps
its scratch files in a private folder inside its turn folder and removes it
when it ends. These include `gh`'s error text before it is redacted, the claim
check's error file and the pull request text read back after `open-pr`.
`publish`, which has no turn folder, keeps its files in the state folder.

A fresh folder is used because `oxidizer-workflow` calls `default-workflow` many
times in one run and `quality-loop` calls it in a loop. A retry on the same
branch finds its pull request through the branch, so it does not need the old
folder.

## The reading

`autodrive_build_goal.sh read <turn_dir>` writes `goal.json` (schema 1) through
`autodrive_build_reading.sh`. It is read fresh every turn. The state folder
supplies only the run's identity and the record of checks the agent reported.

Top-level fields: `summary` (the reading in plain words, at most 600 bytes),
`subject` (`branch <name>`, or `PR #<n>` once a pull request exists),
`guidance_target` (the pull request URL once one exists, otherwise absent),
`worktree` (the recorded path when it is present and valid, otherwise an empty
private folder, so the shared loop's turns still start) and `untrusted` (the
JSON paths that carry third-party text).

| Section | Fields |
| --- | --- |
| `run` | branch, base, worktree (path, present), launch checkout, local head, pushed head, commits ahead of the pushed head, uncommitted changes, merge in progress, `identity_check` |
| `prs` | how the run's pull request was found (`state`, `branch`, `context` or `none`); each candidate's number, URL, state, draft flag, head ref, head SHA, base, cross-repository flag, title and body |
| `pr`, `checks`, `reviews`, `repository`, `base` | With one own open pull request: taken whole from `autodrive_pr_reading.sh`, as in the merge loop. Includes mergeable and mergeStateStatus, each check at the head, review verdicts, strict status checks and allowed merge methods |
| `base` (no pull request yet) | commits behind, whether the base conflicts (from `git merge-tree`), files changed on both sides, the strict-policy reading |
| `issue` | number, source, and each claim with its pull request, state, head ref, `via` and `relation` |
| `host_inputs` | `.venv` and `.venv-*` in the launch checkout; in the worktree, each as a folder, a link with its target, or missing |
| `checks_passed` | Checks reported at the current local head only |
| `pr_text` | The intended title and body, and whether the live text equals them |
| `context` | `existing_branch`, `allow_existing_pr`, `pr_number`, `issue_number` and the other keys the agent may need. Not `no_merge` or `finalization_handoff` |

A field that cannot be read is `null` with the reason in `error`. The read
itself succeeds. A failed `gh pr list` is unknown, never "no pull request", so
the agent cannot open a second pull request because a read failed. Third-party
strings have control characters removed and are capped (titles at 256 bytes,
excerpts at 2,000 bytes).

### How the run's own pull request is found

In order:

1. the number recorded in `pr.json` when `open-pr` ran;
2. `gh pr list --head <run branch> --state open` against the run's repository
   (same-repository pull requests only; the one whose base matches the context
   first, then the lowest number);
3. the context `pr_number`.

The run branch and worktree come from `worktree_setup`, recorded when the run
starts. They never come from the branch checked out at `REPO_PATH`, which is
often `main`. No head SHA filter applies. When more than one pull request
matches, all of them are in the reading. Pull request reads and `mark-ready`
need no worktree. The repository is chosen as described under
[Safety checks before a write](#safety-checks-before-a-write).

## Actions

The goal tool offers five actions. The shared loop adds its own words:
`continue`, `wait-owner`, `handoff`, `ready` and `stop`. The agent edits and
commits code itself in the worktree; the tool does not wrap that in an action.

`act <word> <turn_dir>` always writes `result.json` with an `outcome` and a
`summary` (at most 600 bytes) and exits 0. A non-zero exit means misuse of the
contract: exit 2 for a bad call, 75 and 79 passed up unchanged.

| Word | Needs | What fixed code does | Outcomes |
| --- | --- | --- | --- |
| `open-pr` | pushed branch | Creates the pull request with the title and body from `args.title` and `args.body`, byte for byte, against the base from the context, then the identity, then the repository default. If an open pull request already exists on the branch, records its number. Reads the text back and corrects it at most once | `opened`, `already-open`, `text-differs`, `not-pushed`, `refused`, `invalid-args`, `refused-secret`, `failed` |
| `mark-ready` | pull request | Runs `gh pr ready <n>` and reads back `isDraft`. Refuses a cross-repository pull request | `marked-ready`, `already-ready`, `still-draft`, `not-open`, `refused`, `refused-cross-repo`, `guidance-unknown`, `no-pr`, `failed` |
| `review` | pull request | Runs one review round at the pushed head through the shared review entry point | the verdict, `no-pr`, `failed` |
| `wait` | pull request | Uses the [shared conditional REST wait](./autodrive-goal-loop.md#waiting) at the configured interval | `checks-complete` with `checks_state` of `passed`, `failed`, or `unverified`, `head-changed`, `guidance-arrived`, `wait-error` |
| `prepare-worktree` | recorded branch | Restores a missing worktree, or adopts one, and links host inputs. Never uses `-f`, never resets | Reports whether the worktree is usable and which host inputs were linked |

How the agent commits and pushes: it runs `git commit` and `git push` in the
worktree like any developer, through the repository's hooks and never with
`--no-verify` or force. A hook failure is the command's output. The next turn
reads it as evidence and chooses what to do, for example installing a missing
host input and committing again. No fixed step runs hook repair on the loop
path.

Every action accepts an `args.checks` field, which `act` ignores. The agent records [checks already passed](#checks-already-passed) in `checks.json`.

`mark-ready` honours the shared `AUTODRIVE_GUIDANCE_UNKNOWN` signal. The other
actions do not, because only readiness changes what the owner sees.

### Safety checks before a write

Before `open-pr`, `mark-ready`, `review`, adoption and `publish`, fixed code
derives the run branch again from the context (`worktree_setup.branch_name`,
then `existing_branch`) and reads the pull request live. The head must match,
the pull request must be same-repository, and, for every write except
`publish`, open. Any mismatch gives `refused`.

The repository comes from the launch checkout's configured origin URL
(`git config remote.origin.url`, before any `insteadOf` rewrite) and is
exported as `GH_REPO` for every `gh` call, including the delegated ones. No
call passes `--repo`. A write is refused when the context `pr_repository` differs from the
origin repository. When the origin cannot be parsed, reads record an error in
the reading and writes report `failed`.

### Adoption

`prepare-worktree` with `args.branch` adopts an existing branch only for:

- the pull request recorded in `pr.json`;
- the context `pr_number`;
- a pull request authored by the authenticated `gh` user;
- a pull request whose number appears in the task text.

It never adopts the base branch, the default branch or a protected branch.
Adoption rewrites `identity.json` with `source: adopted` and the previous
identity, and logs the pull request number and the author's login. Under a
GitHub App token, `gh api user` fails, so the author-based source cannot apply.
The recorded pull request, the context `pr_number` and the pull request named in
the task text still apply.

## Pull request text

On every route, the title and description are the task's own. When the task
gives PR lines, such as a required `Refs`, `Fixes` or `Task` line, they reach
the pull request exactly and no template replaces them. Reports go in
comments, never in the description.

- The agent passes `args.title` and `args.body`. The tool writes each to a file
  byte for byte and keeps the latest copy in the state folder.
- Title limits: one line, 1 to 256 bytes. Body limits: 1 to 60,000 bytes, with
  no NUL and no control characters other than tab, carriage return and line
  feed.
- After the pull request opens, the tool reads the text back. It compares after
  converting CRLF to LF and trimming trailing whitespace on each field, since
  GitHub normalises both. A difference gets one `gh pr edit` and one more
  read-back. A difference that remains is recorded in `text.result`
  (`same`, `corrected`, `differs` or `unread`) as evidence. It does not end the
  run.
- `edit-pr` has no action of its own. If a later turn finds that the live text
  differs from `pr_text`, the agent restores it with `gh pr edit`. The
  description is edited only to restore the task's text.
- `@user` and `@org/team` mentions, and closing keywords (`fix`, `fixes`,
  `fixed`, `close`, `closes`, `closed`, `resolve`, `resolves`, `resolved`
  followed by `#N` or an issue URL), are refused with `invalid-args` unless the
  task text, the goal notes, `closing_reference`, the context `issue_number` or
  operator guidance contains them. This stops injected text from notifying
  people or closing other issues at a later merge. The refusal names the token
  in `refused` so the agent can raise it in a comment. See
  [Guidance and required outcomes](#guidance-and-required-outcomes) for how
  guidance is read. Email addresses do not count as
  mentions.
- The shared secret screen covers the title, the body, `publish` text, the
  quick-change report comment and `quick_change_pr_text`. It does not cover
  comments the agent posts itself (see [Security notes](#security-notes)).

## Claims on the issue

A pull request that already names the run's issue is information. The agent
compares it with its task, including `existing_branch` and `allow_existing_pr`,
and either continues that pull request or works beside it with its reason
posted in a pull request comment. Nothing ends the run.

Finding the issue: step 03 chooses a tracking issue from the task text when
`issue_number` is empty, as before. `tracking_issue_reference` in
`workflow_issue_tracking.sh` returns the number and where it came from, so the
agent knows whether the brief quoted its own issue:

| Source | Meaning |
| --- | --- |
| `context` | The run was given `issue_number` |
| `marked-reference` | A marked reference such as `Fixes #463` in the task text |
| `bare-reference` | A bare `#463` in the task text |
| `none` | No issue |

Each claim in `issue.claims` carries a `relation` to the run. The code defines
the full set; the two below are the ones this page relies on.

| Relation | Meaning |
| --- | --- |
| `run-branch` | The claiming pull request's head is the run's own branch |
| `other-branch` | Any other head. The agent judges whether to continue it or work beside it |

The reading also marks when a claim is the recorded pull request or the context
`pr_number`, so the agent can tell its own pull request from another.

`workflow_issue_claim_check.sh` exits 0 for every claim result and every failed
read. A usage error keeps exit 2. The key lines on stdout are unchanged. On
stderr, a claim prints an `INFO` line and the claim list. A new `--json` flag
prints one schema-1 line:

```json
{"issue":463,"repo":"example-org/repo-c","result":"claimed","reason":"open pull request #480 is working on issue #463","claims":[{"number":480,"url":"https://github.com/example-org/repo-c/pull/480","state":"open","head":"feat/issues-463-464-runner-content-follow-ups","via":"closing-ref","source":"search","own":false}]}
```

`--issue` must be numeric, `--head` a valid branch name and `--repo`
`owner/name`. `workflow_publish_pr.sh` no longer finishes with
`FAILED_DUPLICATE_CLAIM`. It prints the claim tool's stderr as information and
carries on. A retry after `workflow-publish` therefore continues the pull
request its first attempt opened (#3110): `open-pr` returns `already-open` with
that pull request's number.

## Host inputs

A new worktree lacks the launch checkout's git-ignored environment folders. A
repository whose pre-commit hooks call `.venv/bin/python` fails in the worktree
unless the folder is present (#3102). The run installs the missing input and
never ends because of it.

Step 04 of `workflow-worktree`, and `prepare-worktree` in the loop, call
`workflow_worktree_host_inputs.sh link <launch> <worktree>`. It considers
top-level `.venv` and `.venv-*` names that:

- match `^\.venv(-[A-Za-z0-9._-]+)?$`;
- are folders, or symlinks to folders (a symlink resolving outside the launch
  checkout qualifies only when its target holds `pyvenv.cfg`);
- are reported ignored by `git -C <launch> check-ignore -q`.

Each qualifying name is linked to `<launch>/<name>` where nothing exists at that
name in the worktree. One line per link, and a `WARNING` per failure, go to
stderr; stdout stays empty. The tool exits 0, or 2 for a usage error. Nothing
else is linked: no `node_modules`, no caches.

A linked name must never be staged or committed. An ignore rule written as
`.venv/` matches folders only, not a symlink. When `git check-ignore` does not
already ignore the link, the tool appends `/<name>` to the file that
`git rev-parse --git-path info/exclude` names. That file lives in the common git
directory, so the exclude applies to every worktree of the repository.

`workflow_worktree_host_inputs.sh facts` prints
`{launch, worktree:[{name, kind, target}], missing}`. The reading's
`host_inputs` section carries the same facts. When a commit fails on a missing
input, the hook output together with `host_inputs` names the cause: "the
worktree has no `.venv`; the launch checkout has one".

A linked `.venv` belongs to the launch checkout. If the tests would import the
repository's own package from it, for example through an editable install, the
agent removes the link with `rm .venv` and creates the worktree's own before
installing into it. The rule is in the goal text and in the hook-repair prompt.
No code detects it.

### Hook repair

`hook-rejection-repair.yaml` lets its agent install a missing host input by the
repository's own instructions, read from the base branch, inside the worktree.
It never uses `sudo`, a system package manager or a global install. Causes that
cannot be fixed from the worktree still return `not_fixable_in_code`.
`workflow_hook_repair.sh` and its return codes are unchanged.

## Checks already passed

A run does not repeat a check it has passed at the same head. The agent reports
checks it runs by writing `checks.json` in the state folder, and the reading
lists those recorded at the current local head under `checks_passed`, each with
`reported_by: agent`, the check, the head, whether changes were uncommitted, the
outcome and the time. After a new commit, nothing is listed at the new head.
Malformed entries are counted in `ignored`. The tool never refuses to run a
check; the agent judges.

`smart-execute-routing` skips `smart-reflect-loop`, and the validation step of
`smart-validate-summarize`, whenever `build_loop_result` is non-empty. The
summary and session completion still run.

## Base merges

No fixed step on the build path merges the base. The reading gives the facts
(commits behind, conflicts, files changed on both sides, strict policy). The
goal text states the rule: merge the base only on a conflict, when the
repository requires a branch level with the base, or for an owner's stated
reason, always with a merge commit, never by a rebase or force-push. The agent
runs `git fetch` and `git merge --no-ff --no-edit origin/<base>` itself, resolves
any conflict, and commits and pushes without force.

## Guidance and required outcomes

Guidance is part of the goal. Where guidance and the task text differ, the later
guidance governs. The shared driver reads guidance at the start of each turn.
The goal text also names three long actions, the full test suite, a review round
and a wait for CI, and tells the agent to read guidance again before each of
them. Nothing in code times this.

When `open-pr` checks mentions and closing keywords, it also reads the guidance
the run has received. `build_allow_text` in `autodrive_build_lib.sh` runs
`amplihack recipe guidance show --operator-only`, passing the pull request's URL
when `pr.json` holds one that matches
`https://github.com/<owner>/<repo>/pull/<n>`, and adds the text to the allow
list. A malformed URL is dropped and the run's own folder is still read.

- `--operator-only` leaves out entries that an agent step added with
  `recipe guide --from-step`, so a build cannot widen its own list by writing
  guidance to itself. It guards against mistakes. It is not a trust boundary:
  the label is written by the CLI, and the turn agent holds the same `gh` login.
- The read fails closed. The guidance text is appended to the allow list only
  when the command exits 0, and the scratch file holding it is removed in every
  case. When the read fails, for example on an older binary that rejects
  `--operator-only` with exit 2, the allow list is the task-only list, and
  `open-pr` adds `guidance_unread` (the exit code) to its result and the
  unread clause to its summary.
- A reference found in none of the sources is refused as an action result. The
  run carries on, and the agent can raise it with `publish`.
- A guidance header of the form `user@domain` would allow `@domain`. The value
  comes from the operator's own account, so this is accepted.
- Guidance text appears only in the plain output of `show` and in a 0600
  scratch file. It never reaches `--json`, argument lists, logs, turn records or
  `workflow_result`.

Every required outcome in the task text or in guidance is listed in the outcome
ledger each turn, with whether it is met. The goal text tells the agent to
choose `mark-ready` once each outcome a build can meet is met, or rejected with
a reason in a pull request comment. An outcome that only a later run can meet,
such as a merge, stays `unmet` and says so in its evidence. Nothing in code
checks this; the ledger is the agent's recorded judgement. The shared `ready`
ending posts the ledger as a pull request comment.

`build-report` copies the ledger into `workflow_result.outcomes` as a list of
`{name, status}` in ledger order, with `status` one of `met`, `unmet` or
`rejected`. The value is `[]` when there is no ledger. It is `null`, with one
entry in `errors`, when the ledger is a symbolic link, is over 524,288 bytes,
is not JSON, has no `outcomes` array, or has an element with an empty name or
another status. Control characters in a name, U+2028 and U+2029 included, become
a space, and a name is cut to 200 characters. `reason` and `evidence` are not
copied. `terminal_state` and `terminal_success` are never changed by the
ledger.

Text from pull requests, issues, checks and reviews is data. Only the task text
and owner guidance are instructions, and a command quoted in other text is never
run. Rule 15 of `skills/default-workflow/goal-build-pr.md` states this
distinction between data and instructions for the agent.

## How a build ends

| Loop end | Run result |
| --- | --- |
| `ready`, `merged`, `closed` | Success |
| `handoff` | A hand-off with its text, naming the owner and the decision. Not a failure |
| `stop` | Stopped by the owner |
| a turn that failed | Never an end. The next turn starts and reads the failure as evidence |
| failure before the first turn (bundle check, missing goal tool) | Failure. It is an install fault that no agent inside the run can judge |
| failure after a turn has started | Not a failure. The step prints a warning, and the loop's end line is `{end: null, exit, turn, error}` with exit 0 |

The loop counts as started when a turn's `reading.json` is a regular file, not a
symbolic link, or when `loop-end.json` is valid. `build-loop` exits like this:

| Driver exit | `build-loop` result |
| --- | --- |
| 0 | Prints `loop-end.json`, or the fallback line when it is absent, and exits 0 |
| 75 or 79 | The code passes up unchanged |
| any other code, before the loop started | Prints an `ERROR` line, leaves stdout empty and exits with that code |
| any other code, after it started | Prints a `WARNING` on stderr and the fallback line, and exits 0 |

The `error` field goes through `goal_clean_line` and is cut to 300 bytes. The
loop's scratch folder is created inside the state folder, so the run leaves no
file outside it. A run whose loop ended with a failure after a turn has run, including a usage
limit when `usage_limit_wait` is `false`, reports `INCOMPLETE` and exits 0, not
a failed step. `build-report` is non-fatal. A run
whose work is pushed and whose pull request is open is never reported as failed
because a later bookkeeping step failed. The same holds for the steps after the loop in
`smart-execute-routing` and `smart-validate-summarize` (`summarize` and
`complete-session` are non-fatal).

The loop normally ends `ready`: the pull request is open, the work is pushed and
the agent has marked it ready for review. The build then stops. Merging belongs
to an `auto-drive-to-merge` run.

### Finding and marking the run's own pull request (#2787)

The agent knows its own pull request from live state: the branch it pushed and
the pull request it opened. `mark-ready` marks it ready, or the outcome says why
not. No path fallback ends a run whose work is done. Mergeability is read from
GitHub, not from a worktree, which may be gone.

`final` prints `merged` or `closed` when the run's own pull request is merged
or closed. Otherwise it prints nothing, including before a pull request exists
and when the read fails.

## Quick-change

`quick-change` stays a short separate route for small new-branch changes. It
gains three things.

1. Task PR lines. A new fast-tier agent step, `quick-change-pr-text`, runs after
   the push and before the pull request is created. When the task gives PR
   lines, it writes the title and body files with those lines exactly.
   Otherwise it writes nothing. The step is non-fatal.
2. `quick_change_create_pr.sh`. When the step gave a non-empty title and it passes every
   check, the pull request is created with the title and body byte for byte and
   the template report (Request, Diff, Checks) goes in one comment. When the
   step gave an empty title with a string body, the task has no PR lines, and
   `qc_title` (the first 72 characters of the task), `qc_request` (the first
   4,000 bytes) and the template body apply, as before. Any other result, such as a
   missing, unparseable or non-object `quick_change_pr_text`, or a non-empty
   title that fails a check, creates nothing and exits 1 with
   `ERROR: quick-change-create-pr: check=<key>: <reason>; nothing was created and the work goes to the build loop`.
   The keys are `pr-text`, `title`, `body` (a title with an empty body),
   `secret-screen` and `references`, with the unread clause when guidance could
   not be read. The work then goes to `quick-change-finish-in-loop`. After creation the text is read back
   and corrected once, as in [Pull request text](#pull-request-text). A
   difference that remains is printed as a `WARNING` and posted as a short
   comment. It does not fail the run. `gh pr create` runs without the former
   `timeout 1800` wrapper.
3. A way out of a failed commit. Commit, push and create-pr are non-fatal.
   When the verdict is `proceed` and no pull request exists after them, the new
   step `quick-change-finish-in-loop` runs `default-workflow` with
   `existing_branch` set from the quick-change worktree's branch, and the task
   followed by a fixed hand-over note. Step 04 of `workflow-worktree` reuses a
   registered worktree on that branch as it stands, uncommitted changes
   included; it checks ownership, not cleanliness. The loop continues from the
   worktree and the failure instead of ending the run.

### Azure DevOps builds

The build tools read through `gh` only. `workflow_publish_pr.sh` in 1.2.213
publishes to Azure DevOps with `az repos pr list` and `az repos pr create`, so
those builds would stop working on the loop. `default-workflow` 4.0.0 therefore
chooses the route from the `remote_host_type` that `workflow-prep` outputs:

| `remote_host_type` | Route |
| --- | --- |
| `github`, `other` or empty | The build loop: `build-goal-setup`, `build-loop`, `build-report` |
| `azdo` | The seven v3 phase steps, unchanged: `workflow-design`, `workflow-tdd`, `workflow-refactor-review`, `workflow-precommit-test`, `workflow-publish`, `workflow-pr-review` and `workflow-finalize` |

The three build steps carry the condition `remote_host_type != 'azdo'` and the
seven v3 steps carry `remote_host_type == 'azdo'`. An Azure DevOps run does not
reach `build-goal-setup` and creates no `build.*` state folder.
`recovery.persist.outputs` lists all twelve step ids, and
`recursion.max_total_steps` is 120. The v3 gates remain on this route for one
release. Moving Azure DevOps builds onto the loop is tracked in a follow-up
issue.

Step 04 links host inputs for `quick-change` as well, so its commit passes a
hook that calls `.venv/bin/python`.

See [Quick-Change Recipe](./quick-change-recipe.md) for the full recipe.

## Changes to tools used by older recipes

These changes also protect runs in flight on earlier releases; see
[Runs in flight](#runs-in-flight) for each caller.

| Tool | Change |
| --- | --- |
| `workflow_issue_claim_check.sh` | Exit 0 for every result; `--json`; strict argument parsing |
| `workflow_issue_tracking.sh` | New function `tracking_issue_reference`. Existing functions unchanged |
| `workflow_publish_pr.sh` | The claim stop is removed. All other `publish_status` values unchanged |
| `workflow_pr_ready.sh` | See below |
| `workflow_pr_scope.sh` | `no_scoped_pr` names the filters that removed each candidate |
| `workflow-finalize.yaml` | Step 20 forbids removing the worktree, the branch or the run state, if the sentence fits without adding a line |
| `workflow-prep.yaml` | An Azure DevOps work-item create failure is a `WARNING` and local metadata, not a stop |

`workflow_pr_ready.sh --base <branch>` now:

1. resolves the pull request's worktree from `worktree_setup.branch_name`, then
   `existing_branch`, then the pull request's `headRefName`, reading the branch
   and head there. It never reads the branch checked out at `REPO_PATH` on
   `main`. An `INFO` line names the
   worktree used;
2. passes `--allow-head-behind` to the scope tool, and accepts a pull request
   head that equals the local head or is its ancestor. This relaxes only the head
   SHA check. The repository, cross-repository, base and state checks are
   unchanged;
3. when the run's pull request is known but scope or identity validation fails,
   prints the reason as a `WARNING` and exits 0 without calling `gh pr ready`.

The exits for a closed unmerged pull request, a failed `gh pr ready`, a failed
`gh auth status` and a usage error are unchanged.

When `workflow_pr_scope.sh` finds no match, its JSON gains
`rejected:[{number, rejected_by[]}]`. The filters are head, base, head SHA,
issue token, created-after and title prefix, among others. After the existing
`ERROR` line, stderr gets one `INFO` line per candidate with the compared values
(refs, SHAs, numbers, repository names and times only, control characters
removed). The reason, message, exit status and first stderr line are unchanged:

```text
ERROR: workflow_pr_scope.sh: no_scoped_pr: no PR matched the explicit workflow scope
INFO: candidate #711 rejected by: head_sha (pr=1c7996e2 local=45817ee2)
```

## Behaviour changes

- Builds no longer merge. In 1.2.213, `no_merge` controlled whether the build
  went on to merge; the loop has no merge action. `no_merge` is accepted and
  ignored. Callers that relied on a merge from `default-workflow` use
  `auto-drive-to-merge`. A build run ends with the pull request open and marked
  ready for review.
- `workflow_issue_claim_check.sh` exits 0 where it exited non-zero for a claim.
  Callers that tested its exit status for a claim read the `claimed` key line
  instead, which is unchanged.
- `workflow_publish_pr.sh` no longer returns `FAILED_DUPLICATE_CLAIM`.
- The JSON from `workflow_pr_scope.sh` has a new `rejected` key.
- A pull request on another branch that names the run's issue no longer stops the
  run, so #1361's refusal is replaced by information and a posted reason when the
  agent works beside it.
- Hook repair on the loop path is gone. A hook failure is the result of the
  agent's own commit.
- Cleanup after the goal no longer removes the worktree.
- A loop failure after the first turn, including a usage limit when
  `usage_limit_wait` is `false`, gives a report of `INCOMPLETE` and a
  `build-loop` exit of 0 instead of a failed step.
- `quick-change` no longer falls back to its template when the task's own PR
  lines fail a check or the PR-text step gives no usable answer. It creates
  nothing and hands the work to the build loop, which writes the task's lines.
- Builds whose origin is Azure DevOps run the seven v3 phase steps, as in
  1.2.213, for this release.

## Security notes

The turn agent has a shell and the operator's credentials. The controls above
make fixed code robust against agent mistakes and injected input. They do not
contain the agent. Branch protection is the enforcing control.

- Merge authority comes only from the operator's own inputs. Nothing passes
  `no_merge` into the loop, and the build makes no merge call.
- `mark-ready` and `workflow_pr_ready.sh` refuse a cross-repository pull
  request.
- Fixed code never uses `--no-verify`, `--force` or `--force-with-lease`.
- Arguments are passed as array elements, with no `eval`. Titles use `--title=`,
  bodies use `--body-file`, and `--` precedes paths. Every branch from identity,
  context or `gh` passes `git check-ref-format --branch` and is refused when it
  starts with `-`.
- `prepare-worktree` checks realpath containment under the worktree root and
  refuses symlinked or occupied paths.
- JSON is built with `jq --arg` and `--argjson`. Logs name identifiers only and
  never carry titles, bodies, guidance or task text. Stderr tails are redacted
  (URL userinfo and token-shaped strings) and capped at 200 bytes.
- The resolved goal tool path must lie under `AMPLIHACK_HOME`.
- Residual risks: the agent can force-push, post unscreened comments or edit
  state files; repository hooks and setup scripts run as the operator; an
  editable `.venv` loads code from the launch checkout; a hung `gh pr create` in
  quick-change is no longer cut by a timeout; an operator ceiling of 2 on session
  depth refuses nested turns.

## Notes for the shared loop

The build goal runs a whole goal that starts with no pull request: no
`guidance_target`, `publish` exiting 1, and `final` printing nothing. The shared
loop files are not edited by this change. What the build side relies on, as
read in PR 1's code:

- The driver reads guidance at the start of each turn, before the agent step,
  and again after the decision, before the action. Guidance comes only from
  local guidance folders that `amplihack recipe guide` writes on the host: the
  run's own folder, and the pull request's folder once `guidance_target` names
  it. It is never read from pull request comments, so no author filter is
  needed for it.
- Only `wait` (and the shared `wait-owner`) is woken when guidance arrives.
- Guidance text never reaches the turn folder or any JSON, by design. The goal
  tool reads the guidance folders itself, at each `open-pr`, with
  `amplihack recipe guidance show --operator-only`; see
  [Guidance and required outcomes](#guidance-and-required-outcomes).
- `autodrive_goal_lib.sh` has no redaction helper, so the build library has
  its own (`build_redact_tail`).
- `failed_turn` starts the next turn after any failed turn run, so a failure
  after the work is done is evidence for the next turn (condition 5).

## Release evidence

This section records what a release of this change needs on file before it
ships: each kind of start run through the real binary and recipes, the tools
and recipes that runs already in flight still call, and the way back to the
previous release. The pull request's "Release evidence" comment gives the CI
result of each start kind at the head being merged.

### Start kinds

Each start kind runs in CI's `Install Smoke Test` job, against the binary that
job installs from the checkout and the pinned recipe runner. `gh`, `claude` and,
in the #2806 scenario, `codex` are stand-ins on a private `PATH`, and each
origin is a local bare repository. A missing binary, runner or host tool fails
the step; nothing is skipped. Each runner prints `PASS: <case>` for every case
and `ALL_CASES_PASSED` at the end. None of them is run on a developer host.

| Start kind | Recipe started | CI step and runner | Cases |
| --- | --- | --- | --- |
| New-branch build | `default-workflow` | "Outside-in: the build goal loop (issue #3129)", `tests/gadugi/run-issue-3129-build-loop-scenario.sh` | `build_opens_its_pr_with_the_task_text`, `build_marks_its_pr_ready`, `build_never_merges`, `build_branch_is_pushed_at_the_worktree_head`, `build_keeps_its_worktree_and_branch`, `build_reports_the_open_pr` |
| New-branch build through the router | `smart-orchestrator`, routed to `default-workflow` | "Outside-in: whole build and whole drive (issue #2806)", `tests/gadugi/run-issue-2806-end-to-end-build-and-drive-scenario.sh` | `part1_build_opens_pr_and_pushes_branch`, `part1_build_loop_ends_ready_without_merge` |
| Continuation of an existing branch and pull request | `smart-orchestrator` | "Outside-in: continuation, quick-change and investigation starts (issue #3129)", section E of `tests/gadugi/run-issue-3129-start-kinds-scenario.sh` | `continuation_runs_to_its_end`, `continuation_pushes_onto_the_existing_branch`, `continuation_opens_no_second_pr`, `continuation_marks_the_existing_pr_ready`, `continuation_keeps_the_pr_text`, `continuation_never_merges` |
| Quick-change | `smart-orchestrator` | The same step, section F | `quick_change_runs_to_its_end`, `quick_change_opens_its_pr_with_the_task_text`, `quick_change_opens_the_pr_without_the_build_loop`, `quick_change_pushes_its_branch`, `quick_change_never_merges` |
| Investigation | `smart-orchestrator` | The same step, section G | `investigation_runs_to_its_end`, `investigation_writes_its_report`, `investigation_changes_nothing_on_origin` |

### Runs in flight

A run started on the previous release keeps the recipe it has already loaded.
When a step runs, it reads two things from the live install:

- a tool in `amplifier-bundle/tools`, found through
  `$AMPLIHACK_HOME/amplifier-bundle`, which is the release link, or the other
  places each step searches;
- a recipe named in a `recipe:` step, looked up by name in the recipe search
  path: the run's repository's `amplifier-bundle/recipes`, then
  `$AMPLIHACK_HOME/amplifier-bundle/recipes`, then
  `~/.amplihack/.claude/recipes`.

`autodrive-*` runs are the exception. Each has a private copy of the bundle,
taken at launch, so it keeps its own release's recipes and tools throughout.
An `autodrive-build` run launched on 1.2.219 therefore keeps running
`default-workflow` 3.0.0.

The previous release at the time of writing is 1.2.219. Its bundle is
identical to `main` at fb97d5119 (`git diff v1.2.219 fb97d5119 --
amplifier-bundle` is empty). Apart from `recipes/_recipe_manifest.json`, every
bundle file this change touches is identical at `v1.2.213` and `v1.2.219`, so
what this page says about 1.2.213 holds for 1.2.219. The callers below were
found with `git grep` at `v1.2.219`, leaving out comments, documents and the
recipe test scripts. Every line that invokes one of the changed tools is the
same at `v1.2.213` and `v1.2.219`, so the invocations that the `old_tools`
tests copy from 1.2.213 are also the 1.2.219 invocations. No tool, recipe,
step id or recipe output is removed or renamed. A release cut from `main`
after fb97d5119 and before this change is checked the same way, with its own
tag in place of `v1.2.219`.

Changed tools:

| Tool | Change | Caller at 1.2.219 and its invocation | What the caller reads | Test |
| --- | --- | --- | --- | --- |
| `workflow_issue_claim_check.sh` | Exit 0 for every result, where a claim exited 1. New `--json`. An unknown argument, a missing value or a malformed `--head` or `--repo` exits 2 | workflow-prep `step-03c-issue-claim-check`: `--issue "$N" --repo-path "$REPO_PATH"`. `workflow_publish_pr.sh`: `--issue --repo --head` | step-03c: the stdout key line as `issue_claim_check`, and a non-zero exit fails the step. Both callers pass only accepted arguments, each with a value. A claim now lets step-03c go on | `old_tools::claim_check_called_as_step_03c_exits_zero_with_one_claimed_key_line`, `old_tools::claim_check_called_as_publish_calls_it_exits_zero_and_names_the_claim` |
| `workflow_issue_tracking.sh` | One function added, `tracking_issue_reference`. No line removed | workflow-prep `step-03-create-issue` sources it and calls `select_github_tracking_issue` and `emit_local_metadata`. `autodrive_followups.sh` and `autodrive_followups_file.sh` source it for `sanitize_cli_output` | The existing functions, unchanged | `old_tools::tracking_functions_step_03_calls_still_answer_as_before` |
| `workflow_publish_pr.sh` | A claim by another branch's pull request is printed and the pull request is opened. `FAILED_DUPLICATE_CLAIM` is no longer returned | workflow-publish `step-16-create-draft-pr`: `bash "$PUBLISH_HELPER"` | The stdout JSON as `pr_publish_result`. Every other `publish_status` is unchanged | `old_tools::publish_called_as_step_16_opens_the_pr_beside_another_branchs_claim` |
| `workflow_pr_scope.sh` | The `no_scoped_pr` JSON gains `rejected`. One `INFO` line per candidate follows the unchanged `ERROR` line | The workflow-terminal-state step, `workflow_final_status.sh`, `workflow_pr_ready.sh` and `workflow_publish_pr.sh`, each with its own `scope_args` | `.reason`, `.url` and `.number`, and the exit status. No caller parses stderr | `old_tools::scope_called_as_terminal_state_keeps_reason_exit_and_first_error_line` |
| `workflow_pr_ready.sh` | Finds the pull request's worktree and passes `--allow-head-behind`. A known pull request that fails validation gives a `WARNING` and exit 0, where it gave exit 1 | workflow-finalize `step-21-pr-ready`: `bash "$READY_HELPER" --base "$base"` | The stdout as `pr_ready_result`, and a non-zero exit fails the step. The other exits are unchanged | `old_tools::pr_ready_called_as_step_21_from_the_launch_checkout_on_main_marks_the_pr_ready`, `old_tools::pr_ready_with_a_failed_validation_records_no_pr_ready_call` |

The `autodrive_build_*` tools, `quick_change_create_pr.sh`,
`workflow_pr_text.sh` and `workflow_worktree_host_inputs.sh` are new and have
no caller at 1.2.219. `quick_change_common.sh`, `workflow_hook_repair.sh` and
`worktree_setup_path.sh` are not changed.

Changed recipes that a run in flight can load by name:

| Recipe | Loaded at 1.2.219 by | Change | What stays the same |
| --- | --- | --- | --- |
| `smart-classify-route` | `smart-orchestrator` | The classifier's prompt sends a small operational step to quick-change or the Operations route | Step ids, outputs and the routing values |
| `smart-execute-routing` | `smart-orchestrator` | `build_loop_result` joins the recovery context list | Step ids and outputs |
| `smart-reflect-loop`, `smart-validate-summarize` | `smart-orchestrator` | New input `build_loop_result`, empty by default; reflection and validation are skipped when it is set. Two summary steps continue on error | Step ids and outputs. A 3.0.0 build sets no `build_loop_result`, so these steps run as before |
| `default-workflow`, 3.0.0 to 4.0.0 | `smart-execute-routing`, `quick-change`, `oxidizer-workflow` and `quality-loop` | The build loop | Every 3.0.0 context key is accepted. `workflow_result` keeps `terminal_state`, `terminal_success`, `handoff` and `pr_number` |
| `quick-change` | `smart-execute-routing` | Two steps added, `quick-change-pr-text` and `quick-change-finish-in-loop`; `quick_change_create_pr.sh` opens the pull request | No step id or output removed |
| `workflow-prep` | `default-workflow` 3.0.0 | An Azure DevOps work-item create failure is a `WARNING`, and the run carries on with local tracking. Recovery context list | Step ids and outputs |
| `workflow-worktree` | `default-workflow` 3.0.0, `quick-change` and `auto-drive-light` | Step 04 links the launch checkout's git-ignored `.venv` folders and cannot fail on them | Inputs and the `worktree_setup` JSON |
| `workflow-design` | `default-workflow` 3.0.0 | Recovery context list only | Step ids and outputs |
| `workflow-finalize` | `default-workflow` 3.0.0 | One sentence in step 20's prompt | Step ids and outputs |
| `hook-rejection-repair` | `workflow_hook_repair.sh`, from the bundle beside it | Point 4 of the prompt: a missing host input is installed in the worktree | The outcome values `fixed` and `not_fixable_in_code` |

`smart-orchestrator` itself changes only in its recovery context list. A
1.2.219 `smart-orchestrator` run that loads the new sub-recipes therefore runs
the same steps as the #2806 scenario above. The one difference is that a
recovery replay of that run would not carry `build_loop_result`, so reflection
would run again after a 4.0.0 build.

### Rollback

The way back is an explicit install from the previous release's own binary.
These steps were run in a sandbox with a temporary `HOME`: the 1.2.218 binary
staged this branch's tree with `install --local`, and the 1.2.217 binary then
rolled it back with `install --force-refresh`. Both exited 0.

1. Before the upgrade, note the release in use:
   `readlink ~/.amplihack/amplifier-bundle`, for example
   `~/.amplihack/releases/1.2.219/amplifier-bundle`. After the upgrade,
   install's cleanup keeps the release each link pointed to before the switch
   (see [Versioned Install Layout](../concepts/versioned-install-layout.md#cleanup-of-old-releases)),
   so that folder and its `bin/amplihack` stay on disk until a later install
   finds them unused.
2. Run that release's binary:
   `~/.amplihack/releases/<previous>/bin/amplihack install --force-refresh`.
   It fetches the bundle at the tag `v<previous>`, builds a new release folder
   (`<previous>-<UTC time>-<pid>` when `releases/<previous>` already exists),
   switches all four links to it, copies the framework folders under
   `~/.amplihack/.claude` again, the recipes among them, and writes
   `<previous>` to `~/.amplihack/.installed-version`. A stamp newer than the
   binary does not stop an explicit install; only the startup self-heal and
   the launch bootstrap decline to stage over a newer stamp. In the sandbox it
   printed no refusal or warning about the newer stamp, and the release that
   held this branch's bundle was kept.
3. Check the result. `amplihack --version` prints the previous version, and
   `grep -m1 '^version:'` on both
   `~/.amplihack/amplifier-bundle/recipes/default-workflow.yaml` and
   `~/.amplihack/.claude/recipes/default-workflow.yaml` prints `3.0.0`. In the
   sandbox, `default-workflow`, `smart-orchestrator`, `smart-execute-routing`,
   `quick-change`, `workflow-prep` and `workflow-worktree` in both folders were
   byte-identical to `v1.2.217`, and the bundle had no
   `autodrive_build_goal.sh`.

Repointing the four links by hand is not enough.
`~/.amplihack/.claude/recipes` is a copy outside the release folders, and the
recipe search path reads it, ahead of everything else installed, whenever
`AMPLIHACK_HOME` is unset. Only an install copies it again.

A `default-workflow` 4.0.0 run still in flight at the rollback keeps going
inside a build step it has already started, because each build step resolves
`autodrive_build_run.sh` to its physical release folder when the step starts,
and cleanup keeps a release that a running process uses. Its next build step
resolves through the link to the previous release, finds no
`autodrive_build_run.sh` and stops: `build-goal-setup` and `build-loop` end the
run with "autodrive_build_run.sh is missing from the launch bundle", and
`build-report` logs a warning and writes no report. Let 4.0.0 runs reach their
end before rolling back, or start them again afterwards, when they run 3.0.0.
`autodrive-*` runs are not affected, because they use their private copy.

## Tests

Behaviour tests run the bundle's tools, or the real binary and runner, against
a stand-in `gh` and real git repositories, with a policy-driven stub agent
where an agent step runs. They assert values in `goal.json`, `result.json`,
`loop-end.json`, the runner's step results and the calls the stand-in `gh`
recorded. No test reads recipe, document or prompt text. They are in
`crates/amplihack-cli/tests/issue_3129_build_loop/`.

| Module | What it shows |
| --- | --- |
| `goal_tool` | Goal contract v1: the five words and no merge word; `final` prints nothing before a pull request exists or when a read fails, and `merged` for the run's own merged one; the reading names the issue and its source and marks claims `run-branch` or `other-branch`; a failed pull request read is recorded and the read succeeds; host inputs and checks at the current head only; `open-pr` refuses an empty title, a secret and a closing keyword the task never gave, and reports `already-open` for a retry (#3110); `mark-ready` marks the run's own pull request, refuses a cross-repository one, and works with the worktree gone; no action calls `pr merge`; `publish` exits 1 with no pull request and never posts on the issue |
| `loop_drive` | The build goal through the real shared loop: the pull request opens with the task's title and body byte for byte, is marked ready, the loop ends `ready` with no merge call; a retry continues the pull request its first attempt opened; guidance sent while turn 1 decides is new in turn 2's reading and readable by turn 2's agent with `amplihack recipe guidance show`, and turn 1 has neither |
| `claim_check` | Every claim result exits 0 and keeps its stdout key line; `--json` lists each claim with its head and whether it is the run's own; usage errors exit 2 |
| `issue_reference` | `tracking_issue_reference` gives `context`, `marked-reference`, `bare-reference` or `none`, and ignores code spans |
| `pr_ready` | #2787: a head one commit behind is marked ready; run from a checkout on `main` the tool reads the worktree; an own pull request that cannot be validated stays draft and the tool exits 0; a cross-repository pull request is never marked ready |
| `host_inputs` | #3102: ignored `.venv` and `.venv-*` folders are linked, logged and never staged; a hook that calls `.venv/bin/python` passes in the new worktree |
| `pr_text` | The read-back: matching text is left alone, CRLF and trailing whitespace are no difference, an altered body is restored with one edit, a difference that remains is reported, a failed read edits nothing |
| `quick_change` | The task's PR lines reach `gh pr create` exactly and the report goes in a comment; a rewritten body is restored once; without PR lines the 72-character title and 4,000-byte body apply; an unlisted closing keyword, a failed `pr-text` step and a title with an empty body each exit 1 with the check key and create nothing; a failed create exits 1 with no URL |
| `run_level` | `default-workflow` run whole through the real runner, with a stand-in `gh` and a stub agent: the worktree exists when `mark-ready` runs and after the run, and `build-report` runs once after the loop; a `build-report` that fails after the pull request opened leaves the recipe exit 0 with the failure in the log; an Azure DevOps origin takes the v3 route and never starts the loop; a branch only behind a non-strict base gets no base merge; another branch's pull request on the issue is information and the run opens its own |
| `prep` | `workflow-prep` run whole: `Fixes #N` in the task with no `issue_number` gives issue N, and the claim check reports an open pull request on it as information while the run goes on |
| `guidance` | Operator guidance allows a closing line the task did not give; guidance from an agent step does not; `show --operator-only` leaves out agent-step entries in plain and JSON output; a failed guidance read does not widen the list and sets `guidance_unread`; a malformed pull request URL still reads the run's folder; guidance text reaches no kept record |
| `run_script` | The loop calls the shared driver with the build goal; the loop's exit table and fallback line, a loop that ends before turn 1, no scratch folder left behind, and the outcome list in the report, including `null` for an unreadable ledger |
| `routing` | With `build_loop_result` set, reflection and outside-in validation do not run and the summary does |
| `old_tools` | One 1.2.213 caller's invocation of each changed tool runs against the new bundle |

A gadugi scenario, `tests/gadugi/scenarios/issue-3129-build-loop.yaml`, covers
the build loop end to end. A second, `tests/gadugi/scenarios/issue-3129-start-kinds.yaml`,
starts a continuation of an existing branch and pull request, a quick-change
and an investigation through `smart-orchestrator` and runs each to its end.
Both run in CI only.

## Related

- [Auto-drive Goal Loop](./autodrive-goal-loop.md): the shared loop, goal contract v1 and the turn records
- [Quick-Change Recipe](./quick-change-recipe.md)
- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md)
