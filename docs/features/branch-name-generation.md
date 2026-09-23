# Branch Name Generation in default-workflow

**How `default-workflow` decides what to call its branch and its worktree directory.**

---

## Overview

Step 4 of `default-workflow` (`step-04-setup-worktree`, in
`amplifier-bundle/recipes/workflow-worktree.yaml`) has to answer one question before
it can create anything: *what is this branch called?* The answer also names the
worktree directory, at `<main-repo>/worktrees/<branch>`.

Until issue #1426 the answer was always "slugify the first fifty characters of
`task_description`". That is how a run whose task opened with

```
Repository: /Users/ryan/src/mistt-qa/ws/142/jamestown (GitHub mistt-repo/jamestown).
```

ended up on

```
feat/issue-142-repository-usersryansrcmistt-qaws142jamestown-gith
```

— the path separators stripped out, the name cut mid-word at "gith", and created as a
**second** branch beside `fix/142-band-edge-previous-slice`, which the same task had
pinned and told the run not to branch away from. The same shape once produced
`feat/issue-1277-skip-workflow-launch-this-agent-is-already-executi`: a truncated
prompt fragment living in the git ref namespace.

The name is now resolved by a ladder. Rank 2 below (scanning the task for a branch it
names) lives in `amplifier-bundle/tools/workflow_branch_name.sh`; ranks 3 and 4 are
computed **inline** in the recipe — see [Why the derived name is inline](#why-the-derived-name-is-inline).

---

## The resolution ladder

| Rank | Source | Result |
| ---- | ------ | ------ |
| 1 | `existing_branch` / `pr_number` context keys | that branch, unchanged (issue #342 path) |
| 2 | A branch **named in the task description** | that branch, verbatim |
| 3 | The issue number plus a bounded slug | `{branch_prefix}/issue-{issue_number}-{slug}` |
| 4 | Nothing usable | `{branch_prefix}/issue-{issue_number}-{hash}` |

Ranks 1 and 2 are the same rule stated twice: **an explicitly supplied branch wins, and
no competing name is derived alongside it.** Rank 3 keys the name to the work (the
issue), not to the wording of the prompt. Rank 4 uses a short stable hash of the task
rather than its prose.

### Rank 2 — a branch named in the task

Only a fixed set of directives names the branch. Each must open a line, and case is
ignored:

```
Branch: fix/142-band-edge-previous-slice
Branch name: fix/142-band-edge-previous-slice
Branch ref: fix/142-band-edge-previous-slice
BRANCH = fix/142-band-edge-previous-slice

BRANCH — already created and checked out in this worktree:
    fix/142-band-edge-previous-slice
```

The `Branch:`, `Branch name:`, `Branch ref:` and `Branch =` forms take their value
from the same line or, when it is empty, from the next non-blank line. The dash form
(`—`, `–` or `-`, any text, then a closing `:`) is the one from the #1426 incident. It
takes its value only from the next non-blank line. Any other line that merely starts
with "Branch" and contains a colon is prose. For example, `Branch coverage is low in
these files: src/foo-bar.rs` names nothing.

Backticks, quotes, brackets and trailing sentence punctuation are stripped from the
value, which is then validated with `git check-ref-format --branch`.

Detection is deliberately conservative, because a false positive hijacks the run:

- The word `branch` must open the line. `Please use the branch fix/9-foo` names nothing.
- The value must contain a `/` or a `-`. `Branch protection rules: enabled` names
  nothing. A genuinely single-word branch is still reachable through the
  `existing_branch` context key, which needs no guessing.
- `main`, `master`, `develop`, `trunk` and `head` are refused. A task that mentions one
  of those is describing the base, not the branch to commit onto.
- Names starting with `origin/`, `refs/` or `heads/` are refused. Created as local
  branches, they would make git report ambiguous refs.
- At most 20 directive lines are examined, so a task full of `Branch:` lines cannot
  stall the step.

When the named branch already exists (locally or on `origin`), the run takes the
existing-branch path and **reuses** it. When it does not exist, it is created under
exactly that name.

The run **refuses** (exit 3 from the helper, which aborts step-04) if the named branch
is checked out in another worktree, meaning any worktree other than the caller checkout
or this run's own `worktrees/<branch>`. Task text must not be able to move a run onto a
branch another session is using. To adopt such a branch on purpose, pass
`-c existing_branch=<ref>`.

### Rank 3 — the bounded, issue-keyed slug

```
{branch_prefix}/issue-{issue_number}-{slug}
```

The slug is built from the task description and bounded on both ends:

- Tokens containing `/`, `\` or `@` — filesystem paths, URLs, `org/repo` slugs, e-mail
  addresses — are dropped whole. They are where `usersryansrcmistt-qaws142jamestown`
  came from.
- Tokens longer than 20 characters (identifier blobs, hashes) are dropped.
- A leading `issue <N>` is skipped; the number is already in the branch name.
- Words are joined with `-` **only while the result still fits 24 characters**, so the
  slug always ends on a whole word. Nothing is cut mid-word unless the very first word
  is itself over budget and there is nothing else to say.
- Only the first 64 KB of the task is examined, so the cost does not grow with the
  prompt. With words capped at 20 characters and the slug at 24, the finished ref
  cannot exceed roughly 60 characters.

**Examples:**

| `task_description` | `branch_prefix` | `issue_number` | Generated branch |
| ------------------ | --------------- | -------------- | ---------------- |
| `Add user authentication` | `feat` | `123` | `feat/issue-123-add-user-authentication` |
| `child task` | `fix` | `1134` | `fix/issue-1134-child-task` |
| `Repository: /Users/ryan/src/mistt-qa/ws/142/jamestown (GitHub mistt-repo/jamestown).` | `feat` | `142` | `feat/issue-142-repository-github` |
| `  ` (blank) | `feat` | `42` | `feat/issue-42-36a9e7f1` (hash, rank 4) |

### Rank 4 and the last resort

With no usable words at all the tail is a `cksum` of the task — bounded by
construction, stable across re-runs of the same task, and never prose. If the assembled
name still fails `git check-ref-format --branch`, the recipe falls back to
`feat/task-unnamed-<unix-timestamp>`. That last-resort name hardcodes `feat/` rather
than `$branch_prefix` so an invalid or malicious prefix cannot inject through the
fallback (issue #3023 / BL-002).

---

## Configuration

| Context key | Required | Meaning | Default |
| ----------- | -------- | ------- | ------- |
| `task_description` | Yes | The task. Read from the `TASK_DESCRIPTION` environment variable, never from argv. | — |
| `issue_number` | Yes | Issue number, or a local tracking id such as `local-9f2c1a`. | — |
| `branch_prefix` | No | `feat`, `fix`, `docs`, `refactor`, `test`, … | `feat` |
| `existing_branch` | No | Target an existing branch outright (rank 1). | `""` |
| `pr_number` | No | Resolve a PR's head branch (rank 1). | `""` |

`branch_prefix` is **not** inferred from the task text. Reading the commit type out of
the prose would reintroduce exactly the class of bug #1426 is about; the caller states
it, or it stays `feat`. There is also no structured source to read it
from today — see [Item 5 — the kind prefix](#item-5--the-kind-prefix).

---

## Never pipe into an early-exit stage

The derivation bounds its input with a **shell substring**, never `| head -c`. That is
not a style preference; the first version used `head` and it broke CI.

`printf '%s' "$TASK" | head -c 65536 | tr … | sed …` — `head` stops reading once it has
its bytes, so with a task larger than the 64 KiB pipe buffer the producer is left
writing into a closed pipe. It then dies of `SIGPIPE` (status 141), or — where SIGPIPE
is ignored, a disposition that survives `exec` and is common in CI — bash's `printf`
reports `write error: Broken pipe` and returns **1**. `set -o pipefail` promotes either
to the pipeline's status, the command substitution yields `""`, and `set -e` kills
step-04 before it emits any JSON.

```console
$ trap '' PIPE; set -euo pipefail
$ X="$(printf '%s' "$BIG" | head -c 10)"; echo REACHED
bash: printf: write error: Broken pipe          # rc=1, "REACHED" never printed
```

Whether it fires at a given size is a race on the pipe buffer, which is why it was
green on bash 5.3.9 locally and red on the runner's 5.2.21 with a 100 KB task
description. An empty branch name is worse than an ugly one, so: **no pipeline stage
may stop reading before its producer is done.** `tests/issue_1426_branch_name_not_prose.sh`
guards this twice — B6 exercises a 100 KB description end to end, and D4 asserts the
shape, deterministically, on every machine.

---

## Why the derived name is inline

Rank 2 may live in a bundle helper because it is *optional*: when the helper cannot be
found, the run simply does not detect a named branch, which is the pre-#1426 behaviour.
The derived name is not optional. It is the key under which a re-run recognises — and
reuses — the branch and worktree its predecessor registered.

A first cut of this fix put the derivation behind the same helper, with a hash fallback
when it could not be resolved. The phase bricks are executed with **no `amplifier-bundle`
on disk** by design (`amplifier-bundle/recipes/tests/`, and the step-04 unit tests), so
the fallback fired, the name silently became `fix/issue-1121-2052514743` instead of
`fix/issue-1121-reuse-me`, the already-registered worktree no longer matched, and
`test-issue-1121-relative-repo-path.sh` went red on lost idempotency — the same defect
family as #1420 (duplicate issue, branch and PR on relaunch).

The rule that follows: **a load-bearing value must not change with the environment.**
Anything whose absence would produce a *different* name stays inline in the recipe;
only logic whose absence produces *no* name may be extracted.

`tests/issue_1426_branch_name_not_prose.sh` Part B runs every derived-name case with no
bundle reachable, and pins `fix/issue-1121-reuse-me` explicitly so the two tests cannot
drift apart.

---

## Usage

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication with OAuth" \
  -c branch_prefix="feat" \
  -c repo_path="/path/to/repo"
# Creates branch: feat/issue-123-add-user-authentication
```

Pin the branch explicitly and nothing is derived:

```bash
amplihack recipe run default-workflow \
  -c task_description="$(cat task.txt)" \
  -c existing_branch="fix/142-band-edge-previous-slice" \
  -c repo_path="/path/to/repo"
```

The explicit-branch scan can be run on its own:

```bash
TASK_DESCRIPTION="$(cat task.txt)" \
  bash amplifier-bundle/tools/workflow_branch_name.sh explicit \
    --repo-path /path/to/repo --main-repo /path/to/repo
```

`explicit` prints the named branch, or nothing. Exit codes:

| Code | Meaning |
| ---- | ------- |
| 0 | A branch is named and already exists. Reuse it. |
| 10 | A branch is named and does not exist yet. Create it. |
| 1 | No branch is named. Derive one (rank 3). |
| 2 | Usage error: an unknown flag, or a missing or non-directory path. |
| 3 | The named branch is checked out in another session's worktree. Refuse. |

Step-04 aborts on any other exit code. If the helper is missing, step-04 prints a
`WARNING` and skips detection. It never skips it silently.

---

## Security considerations

- **No shell injection.** The task text is read from the `TASK_DESCRIPTION` environment
  variable, never interpolated into a command line, so quoting and word-splitting cannot
  be subverted. (This is also why a task description larger than Linux's 128 KB
  `MAX_ARG_STRLEN` cannot reach the helper — but such a description cannot reach the
  recipe's own bash step either.)
- **No path traversal.** `..`, `//` and a trailing `/` are refused before any name is
  used, and `git check-ref-format --branch` is the authority on the rest.
- **No ref-namespace corruption.** Every branch name, derived or explicit, passes
  `git check-ref-format --branch` before it is used in a path or a git invocation. A
  name recovered from the task text is re-validated exactly like one from `gh`.
- **Bounded by construction.** The word length (20), the slug (24) and the amount of
  task text examined (64 KB) are all capped, so directory names cannot grow with the
  prompt.

---

## What issue #1426 asked for, item by item

The report closed with a six-part "Suggested fix". Three parts are implemented as
written, one is implemented differently, and two are refused. The refusals are the
ones worth reading.

| # | #1426 asked for | Outcome |
| - | --------------- | ------- |
| 1 | a branch named in the task or context, if present | **As asked** — ranks 1–2 |
| 2 | otherwise issue number + the **issue title** slug | **Deviated** — issue number + a slug of the task's own words |
| 3 | otherwise a short stable hash, not prose | **As asked** — rank 4 |
| 4 | cap the slug (~30 chars), cut on a word boundary | **As asked** — 24 chars, word boundary |
| 5 | take the kind prefix from the work type | **Not done** — below |
| 6 | prefer a branch already checked out in the `-w` directory | **Refused** — conflicts with #858 |

### Item 2 — why the slug is the task's words, not the issue title

No issue *title* reaches `step-04-setup-worktree`. `workflow-prep` extracts an issue
*number* (`step-03b-extract-issue-number`) and nothing else about the issue; there is
no `issue_title` key anywhere in the context chain. Supplying one means changing
`workflow-prep.yaml`, `default-workflow.yaml` and a brick already at its 400-line
budget — for a tail bounded to 24 characters either way.

What #1426 objected to was a ref built out of a prose sentence. The issue title was a
suggested *means* to that end, not the end itself, and `issue-<N>` plus a bounded,
word-boundary slug meets the end. If the title is wanted later it is a separate,
self-contained change to the context chain.

### Item 5 — the kind prefix

`branch_prefix` is already a context key and already honoured: a caller who knows the
work is a fix passes `-c branch_prefix=fix` and gets `fix/issue-N-…`. What is not done
is choosing it *automatically*, and the reason is narrower than "prose parsing is bad":

- Inferring it from the task text is refused outright — that is the #1426 defect.
- The one structured classification the orchestrator does produce, `task_type` in
  `smart-classify-route.yaml`, has the vocabulary `Q&A | Operations | Investigation |
  Development`. It carries no commit kind, so it cannot supply the prefix either.

So there is currently no structured source to read the prefix from; one would have to
be built (the tracking issue's labels are the obvious candidate). That is a design
question of its own and is tracked in **#1463** rather than smuggled in here.

### Item 6 — the branch already checked out in the `-w` directory

This is refused, and it is a real conflict rather than an oversight.

#1426 asks: if the directory the recipe was pointed at is already on the branch, use it
instead of creating a second one. Issue **#858** forbids precisely that — the caller
checkout must never be adopted as a recipe task worktree, because it may carry
unrelated commits and uncommitted files from another recipe that a failed run would
then have to salvage. `step-04` therefore **fails closed**, with an error naming #858.

Both positions are right in their own context. The conflict is resolved in #858's
favour because silently committing into a checkout the user was already working in is
the more expensive mistake. The consequence, stated plainly:

> The exact configuration in #1426's report — the pinned branch checked out in the
> `-w` directory — now **stops the run with an error** instead of producing a
> wrongly-named branch.

What #1426 does win even there is that no competing `feat/issue-N-<prose>` branch is
invented while refusing. `E2` and `E2b` in `tests/issue_1426_branch_name_not_prose.sh`
pin both halves, so neither can drift by accident.

A caller who hits this takes the remedy the error prints: run the recipe from a base
checkout, or pass `-c existing_branch=<ref>` from a directory not sitting on that
branch.

Anyone reopening #858 should start here.

---

## Troubleshooting

### The branch is not the one my task named

Check that the directive opens a line (`Branch: …`, not `…use the branch …`), that the
directive is one of the recognised forms above (`Branch:`, `Branch name:`, `Branch ref:`,
`Branch =`, or the dash form), that the value contains a `/` or `-`, and that it is not
`main`/`master`/`develop`. When in doubt, pass `-c existing_branch=<ref>`, which is unambiguous.

### The branch name is shorter than I expected

That is the 24-character slug bound, cut on a word boundary. The full task lives in the
tracking issue and the PR body; the ref only has to be a readable handle.

### Two runs of the same task create conflicting branches

They do not: for a given issue number and task the name is deterministic. Concurrent
runs that would collide are separated by `tools/workflow_worktree_deconflict.sh`
(issues #829/#840), and an issue already claimed by an open PR stops the second run
(`tools/workflow_issue_claim_check.sh`, issue #1361).

### The run stops with "refusing to use the caller checkout as a recipe task worktree"

The branch the task named is checked out in the directory passed to `-w`. That is
refused by design (issue #858) even though issue #1426 asked for the opposite; the
reasoning is under [Item 6](#item-6--the-branch-already-checked-out-in-the--w-directory).
Run the recipe from a base checkout, or pass `-c existing_branch=<ref>` from a
directory not sitting on that branch.

### The name looks like `feat/task-unnamed-1699564800`

`git check-ref-format --branch` rejected the assembled name — most often because
`branch_prefix` is not a valid ref component (a space, a slash, a leading dash), or
because `issue_number` is a local tracking id containing a `:`, which is not legal in
a git ref.

That fallback is keyed to the clock, so it is **not** deterministic: a re-run gets a
different name and therefore a different worktree, and stops recognising the one its
predecessor registered. Tracked in **#1464**; until it is fixed, pass a `branch_prefix`
and an `issue_number` that form a valid ref, or name the branch outright.

---

## Portability

`workflow_branch_name.sh` targets bash 3.2, the system bash on macOS: no `${VAR,,}`, no
`mapfile`, no associative arrays. Issue #1423 was exactly that mistake.

---

## Related documentation

- [Workflow execution guardrails](workflow-execution-guardrails.md)
- `amplifier-bundle/recipes/workflow-worktree.yaml` — `step-04-setup-worktree`, and the
  inline derivation (ranks 3-4)
- `amplifier-bundle/tools/workflow_branch_name.sh` — the explicit-branch scan (rank 2)
- `amplifier-bundle/tools/workflow_worktree_base_ref.sh` — fetch + base-ref resolution
- `tests/issue_1426_branch_name_not_prose.sh` — the regression spec
- #1463 — deriving `branch_prefix` from a structured source (item 5)
- #1464 — the non-deterministic `feat/task-unnamed-<epoch>` fallback
