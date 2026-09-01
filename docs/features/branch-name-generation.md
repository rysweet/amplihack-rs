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

The name is now resolved by a ladder, in `amplifier-bundle/tools/workflow_branch_name.sh`.

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

A directive **anchored at the start of a line** names the branch. Both the value on the
same line and the value on the following line are recognised:

```
Branch: fix/142-band-edge-previous-slice
Branch name = fix/142-band-edge-previous-slice

BRANCH — already created and checked out in this worktree:
    fix/142-band-edge-previous-slice
```

Backticks, quotes, brackets and trailing sentence punctuation are stripped from the
value, which is then validated with `git check-ref-format --branch`.

Detection is deliberately conservative, because a false positive hijacks the run:

- The word `branch` must open the line. `Please use the branch fix/9-foo` names nothing.
- The value must contain a `/` or a `-`. `Branch protection rules: enabled` names
  nothing. A genuinely single-word branch is still reachable through the
  `existing_branch` context key, which needs no guessing.
- `main`, `master`, `develop`, `trunk` and `head` are refused. A task that mentions one
  of those is describing the base, not the branch to commit onto.

When the named branch already exists (locally or on `origin`), the run takes the
existing-branch path and **reuses** it. When it does not exist, it is created under
exactly that name.

### Rank 3 — the bounded, issue-keyed slug

```
{branch_prefix}/issue-{issue_number}-{slug}
```

The slug is built from the issue title when the caller supplies one (`--title`),
otherwise from the task description, and it is bounded on both ends:

- Tokens containing `/`, `\` or `@` — filesystem paths, URLs, `org/repo` slugs, e-mail
  addresses — are dropped whole. They are where `usersryansrcmistt-qaws142jamestown`
  came from.
- Tokens longer than 20 characters (identifier blobs, hashes) are dropped.
- A leading `issue <N>` is skipped; the number is already in the branch name.
- Words are joined with `-` **only while the result still fits 24 characters**, so the
  slug always ends on a whole word. Nothing is cut mid-word unless the very first word
  is itself over budget and there is nothing else to say.
- The finished ref is capped at 72 characters, again trimmed back to a word boundary.

**Examples:**

| `task_description` | `branch_prefix` | `issue_number` | Generated branch |
| ------------------ | --------------- | -------------- | ---------------- |
| `Add user authentication` | `feat` | `123` | `feat/issue-123-add-user-authentication` |
| `child task` | `fix` | `1134` | `fix/issue-1134-child-task` |
| `Repository: /Users/ryan/src/mistt-qa/ws/142/jamestown (GitHub mistt-repo/jamestown).` | `feat` | `142` | `feat/issue-142-repository-github` |
| `  ` (blank) | `feat` | `42` | `feat/issue-42-36a9e7f1` (hash, rank 4) |

### Rank 4 and the last resort

With no usable words at all the tail is `sha256(task_description)[0:8]` — bounded by
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

Bounds are overridable for testing but the defaults are the contract:
`AMPLIHACK_BRANCH_SLUG_MAX` (24), `AMPLIHACK_BRANCH_NAME_MAX` (72),
`AMPLIHACK_BRANCH_INPUT_MAX` (65536 bytes of task text examined).

`branch_prefix` is **not** inferred from the task text. Reading the commit type out of
the prose would reintroduce exactly the class of bug #1426 is about; the caller states
it, or it stays `feat`.

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

The helper can also be run on its own:

```bash
TASK_DESCRIPTION="$(cat task.txt)" \
  bash amplifier-bundle/tools/workflow_branch_name.sh explicit
TASK_DESCRIPTION="$(cat task.txt)" \
  bash amplifier-bundle/tools/workflow_branch_name.sh derive --issue 142 --prefix fix
```

`explicit` prints the named branch, or nothing. With `--repo-path`, its exit code
answers "does it already exist?": 0 for yes (reuse it), 10 for no (create it).

---

## Security considerations

- **No shell injection.** The task text reaches the helper through the environment, not
  through a command line, so quoting and word-splitting cannot be subverted. It is also
  why a task description larger than Linux's 128 KB `MAX_ARG_STRLEN` cannot reach the
  helper at all — but such a description cannot reach the recipe's own bash step either.
- **No path traversal.** `..`, `//` and a trailing `/` are refused before any name is
  used, and `git check-ref-format --branch` is the authority on the rest.
- **No ref-namespace corruption.** Every branch name, derived or explicit, passes
  `git check-ref-format --branch` before it is used in a path or a git invocation. A
  name recovered from the task text is re-validated exactly like one from `gh`.
- **Bounded by construction.** The slug (24), the ref (72) and the amount of task text
  examined (64 KB) are all capped, so directory names cannot grow with the prompt.

---

## Troubleshooting

### The branch is not the one my task named

Check that the directive opens a line (`Branch: …`, not `…use the branch …`), that the
value contains a `/` or `-`, and that it is not `main`/`master`/`develop`. When in
doubt, pass `-c existing_branch=<ref>`, which is unambiguous.

### The branch name is shorter than I expected

That is the 24-character slug bound, cut on a word boundary. The full task lives in the
tracking issue and the PR body; the ref only has to be a readable handle.

### Two runs of the same task create conflicting branches

They do not: for a given issue number and task the name is deterministic. Concurrent
runs that would collide are separated by `tools/workflow_worktree_deconflict.sh`
(issues #829/#840), and an issue already claimed by an open PR stops the second run
(`tools/workflow_issue_claim_check.sh`, issue #1361).

### The name looks like `feat/task-unnamed-1699564800`

`git check-ref-format --branch` rejected the assembled name — most often because
`branch_prefix` is not a valid ref component (a space, a slash, a leading dash).

---

## Portability

`workflow_branch_name.sh` targets bash 3.2, the system bash on macOS: no `${VAR,,}`, no
`mapfile`, no associative arrays. Issue #1423 was exactly that mistake.

---

## Related documentation

- [Workflow execution guardrails](workflow-execution-guardrails.md)
- `amplifier-bundle/recipes/workflow-worktree.yaml` — `step-04-setup-worktree`
- `amplifier-bundle/tools/workflow_branch_name.sh` — the ladder
- `amplifier-bundle/tools/workflow_worktree_base_ref.sh` — fetch + base-ref resolution
- `tests/issue_1426_branch_name_not_prose.sh` — the regression spec
