# Re-runs Never Destroy an Existing Worktree's Work (Issue #1565)

When a task is re-run, `step-04-setup-worktree` no longer resets, cleans or
deletes a branch or worktree that already holds work. If the derived branch's
worktree has uncommitted changes, untracked files or commits ahead of
`BASE_REF`, the step **keeps** that branch and worktree exactly as they are and
gives the new run its own fresh branch and worktree.

**Affects:**
- `amplifier-bundle/recipes/workflow-worktree.yaml`: `step-04-setup-worktree`
- `amplifier-bundle/recipes/consensus-issue-worktree.yaml`: `step3-setup-worktree` ([issue #1626](#consensus-issue-worktreeyaml-step3-issue-1626))
- `amplifier-bundle/tools/workflow_worktree_deconflict.sh`: `resolve --base`

**Closes:** #1565, #1626

---

## Contents

- [Quick start](#quick-start)
- [The problem](#the-problem)
- [Behaviour by state](#behaviour-by-state)
- [What counts as work](#what-counts-as-work)
- [Messages](#messages)
- [Continuing an earlier branch on purpose](#continuing-an-earlier-branch-on-purpose)
- [Fail-loud guard](#fail-loud-guard)
- [Cleaning up kept worktrees](#cleaning-up-kept-worktrees)
- [Testing](#testing)
- [consensus-issue-worktree.yaml step3 (issue #1626)](#consensus-issue-worktreeyaml-step3-issue-1626)
- [Related](#related)

---

## Quick start

Nothing needs configuring. Run the same task twice:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1565" \
  -c repo_path="$(pwd)"
```

If the first run left work in
`worktrees/feat/issue-1565-fix-issue-1565`, the second run prints:

```text
WARNING: keeping existing_branch=feat/issue-1565-fix-issue-1565 at /repo/worktrees/feat/issue-1565-fix-issue-1565 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1); this run uses feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821. To continue the kept branch, re-run with existing_branch=feat/issue-1565-fix-issue-1565.
```

and its step output names the new branch:

```json
{
  "worktree_path": "/repo/worktrees/feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821",
  "branch_name": "feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821",
  "base_ref": "origin/main",
  "base_branch": "main",
  "runtime_root": "/run/user/1000/amplihack-runtime/feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821",
  "created": true
}
```

The first run's files, modifications and commits are untouched.

---

## The problem

Before this fix, the State 1 reuse path ran the following on any existing
worktree it found dirty or ahead of base:

```bash
git -C "${WORKTREE_PATH}" checkout -- .
git -C "${WORKTREE_PATH}" clean -fd
git -C "${WORKTREE_PATH}" reset --hard "${BASE_REF}"
```

The State 2 path (branch exists, worktree missing) ran `git branch -D` on a
branch with commits ahead of base. Together they deleted uncommitted edits and
untracked files and orphaned unpushed commits. The step cannot tell whether
that work came from an earlier run of the same task or from a person working
in the worktree, so it must not destroy it.

Issue #1565 offered two fixes: keep the work and move the new run to a new
branch (option 1), or archive the work under
`refs/amplihack-archive/<branch>/<timestamp>` before resetting (option 2).
Option 1 is implemented. It reuses the existing deconfliction helper, writes no
refs, and leaves the earlier work exactly where the user left it. Option 2
would still rewrite the user's worktree and need a recovery procedure.

---

## Behaviour by state

`BASE_REF` is the resolved base for this run (for example `origin/main`).

| State | Situation | Result | `created` |
|---|---|---|---|
| 1, clean | Branch and worktree exist at this run's path, no changes, 0 commits ahead | Reused unchanged, same branch and path. Not fast-forwarded even if behind `BASE_REF`. | `false` |
| 1, dirty or ahead | Branch and worktree exist at this run's path, with changes and/or commits ahead | Kept untouched. This run gets a fresh branch from `BASE_REF` in its own worktree. | `true` |
| 2, not ahead | Branch exists, no worktree, 0 commits ahead | Worktree attached to the existing branch. | `true` |
| 2, ahead | Branch exists, no worktree, commits ahead | Branch kept at the same SHA. This run gets a fresh branch. | `true` |
| 3 | Branch does not exist | Branch and worktree created from `BASE_REF`. | `true` |
| Foreign | Branch is checked out by a worktree at a different path | Unchanged from before: this run gets a fresh branch. | `true` |

Step-04 contains no `checkout -- .`, `clean -f`, `reset --hard` or
`branch -D`. The fresh name comes from
[`workflow_worktree_deconflict.sh resolve --base`](../../amplifier-bundle/tools/workflow_worktree_deconflict.md#the---base-flag):
`<branch>-<suffix>`, at most 80 characters, checked with
`git check-ref-format --branch`, and not already used by a local branch, a
local remote-tracking ref (`refs/remotes/origin/<name>`) or any worktree. The
helper does not fetch, so a branch pushed to `origin` since the last fetch is
not seen.

The kept branch is never pushed by the new run. The push at the end of step-04
runs after `BRANCH_NAME` has been reassigned, so it pushes only the new branch.

---

## What counts as work

| Signal | Check | Notes |
|---|---|---|
| Uncommitted or untracked changes | Any output from `git --no-optional-locks -C <worktree> status --porcelain` | Covers modified, staged and untracked files. Ignored files don't count. The old `clean -fd` never touched them either. `--no-optional-locks` avoids taking a live worktree's index lock (#1560). |
| Commits ahead | `git rev-list --count "${BASE_REF}..refs/heads/<branch>"` > 0 | Counted whether or not the commits are pushed. |
| Cannot tell | `status` or `rev-list` fails, the count is not a number, the worktree directory is missing, or `BASE_REF` does not resolve | Treated as work. The run moves to a fresh branch. |

When the check can't give an answer, the step assumes work is there: the cost
is one extra branch, never lost work.

---

## Messages

All messages go to stderr. They contain the branch, the path, `yes`/`no` for
changes, the ahead count and `BASE_REF`. They never contain file names, diffs,
commit messages or the task description.

| Level | When | Meaning |
|---|---|---|
| `WARNING: keeping existing_branch=<branch> …` | Printed by the helper when it moves this run off a branch holding work | Earlier work is kept at the path shown. This run uses the new branch named in the message. |
| `INFO: branch '<branch>' is owned by a foreign worktree …` | Printed by the helper when a different worktree holds the branch | Unchanged foreign-worktree deconfliction. |
| `INFO: using branch '<new>' instead of '<old>'.` | Printed by step-04 after either kind of rename | Step-04 can't tell the two renames apart; the helper line just above it says which it was. |
| `INFO: Branch '<branch>' and worktree '<path>' already exist and are clean — reusing.` | Clean State 1 | Same branch, `created=false`. |
| `ERROR: … refusing to reset existing_branch=<branch> …` | The [fail-loud guard](#fail-loud-guard) fired | Step exits 1. Nothing was changed. |
| `ERROR: worktree branch deconfliction failed for '<branch>' …` | The helper exited non-zero: retries exhausted, misconfigured, or an old copy that rejects `--base` | Step exits 1. Nothing was changed. |
| `ERROR: deconflict helper returned an invalid branch name` | The helper's output failed `git check-ref-format --branch` | Step exits 1 before any ref is created. |

---

## Continuing an earlier branch on purpose

A plain re-run never adopts a branch that holds work. To carry on with it, name
it explicitly through the issue #342 `existing_branch` path:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1565" \
  -c repo_path="$(pwd)" \
  -c existing_branch="feat/issue-1565-fix-issue-1565"
```

The `WARNING` and `ERROR: … refusing to reset …` messages both print the exact
`existing_branch=<branch>` value to use.

A task description that names the branch on its own line (`Branch: <ref>`,
issue #1426) does the same thing: when that branch already exists, step-04
sets `existing_branch` to it and takes the #342 path. Both routes run before
the deconflict helper and the state machine, so a branch you name is adopted
as it stands, work included.

---

## Fail-loud guard

A missing helper fails the step only when work is present. A clean State 1, a
State 2 with no commits ahead, and State 3 proceed as before without it.

The step re-checks for work just before its state machine runs, in case the
helper is missing or the worktree changed between the helper call and the
state machine. If State 1 is
dirty or ahead, or State 2 is ahead, at that point, step-04 prints:

```text
ERROR: refusing to reset existing_branch=feat/issue-1565-fix-issue-1565 at /repo/worktrees/feat/issue-1565-fix-issue-1565 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1). Re-run with existing_branch=feat/issue-1565-fix-issue-1565 to continue it, or move the work aside.
```

and exits `1`. It never falls back to resetting.

An old helper that does not understand `--base` exits `1` on it as a bad
argument, so step-04 prints
`ERROR: worktree branch deconfliction failed for '<branch>' …` and exits `1`.
As a second line of defence, the helper's output is checked with
`git check-ref-format --branch`: a helper that echoes `--base` back as the
branch name makes step-04 print
`ERROR: deconflict helper returned an invalid branch name` and exit `1` before
any ref is created.

---

## Cleaning up kept worktrees

Kept branches and worktrees are not garbage-collected. List and remove the ones
you no longer need yourself:

```bash
git worktree list
git worktree remove worktrees/feat/issue-1565-fix-issue-1565
git branch -d feat/issue-1565-fix-issue-1565   # -d refuses if unmerged
```

The orphan sweep (`workflow_worktree_sweep.sh`, see
[#840](issue-840-leak-proof-worktree.md)) is unchanged and still skips dirty
worktrees.

---

## Testing

`amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh`
extracts the real step-04 body and runs it against a scratch repo with a local
bare `origin`, under `env -i` with a scratch `HOME` and
`GIT_CONFIG_GLOBAL=/dev/null`. Scratch lives under `$TMPDIR`.

| Scenario | Checks |
|---|---|
| S1: dirty State 1 (untracked file, modified file, unpushed commit) | All three survive in the original worktree and branch. The run gets a new branch whose HEAD is `BASE_REF`, its worktree exists under `worktrees/`, `created=true`, and stderr names the kept branch. |
| S2: clean State 1 | Same branch and path, `created=false`. |
| S3: State 2, 1 commit ahead | Old branch still at the same SHA. The run gets a new branch. |
| S4: `resolve` without `--base` | Output unchanged from before. |
| S5: helper missing | Exits non-zero, stderr says `refusing to reset`, all work survives. |
| S6: old helper that prints `--base` | Exits non-zero, no ref created. |
| Static | Step-04 contains no `checkout -- .`, `clean -f`, `reset --hard` or `branch -D`. |

Run it locally:

```bash
bash amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh
```

To confirm it catches the old behaviour, point it at another copy of the YAML:

```bash
git show origin/main:amplifier-bundle/recipes/workflow-worktree.yaml \
  > "$TMPDIR/workflow-worktree.main.yaml"
WORKTREE_YAML="$TMPDIR/workflow-worktree.main.yaml" \
  bash amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh
# exits non-zero against a YAML that still resets
```

`WORKTREE_YAML` must be a regular file. CI runs the test against the in-repo
YAML in the step "Recipes — re-run never destroys an existing worktree's work
(issue #1565)".

---

## consensus-issue-worktree.yaml step3 (issue #1626)

`step3-setup-worktree` in `consensus-issue-worktree.yaml` follows the same
contract as step-04: a re-run never resets, cleans or deletes a branch or
worktree that holds work. When the derived branch is dirty or ahead of
`BASE_REF`, the run moves to a fresh suffixed branch. If the work is still
there after deconfliction, or the helper can't be found and there is work, the
step stops with an error. A clean existing branch is reused as before.

This applies to the slug path only (branch `feat/consensus-<slug>`). The
`existing_branch`/`pr_number` path (#342) adopts the named branch as it stands
and is unchanged.

### Behaviour by state

| State | Situation | Result | `commands_executed` |
|---|---|---|---|
| 1, clean | Branch and worktree exist at this run's path, no changes, 0 commits ahead | Reused unchanged | `["reuse-clean-worktree"]` |
| 1, dirty or ahead | Branch and worktree exist, with changes and/or commits ahead | Kept untouched. This run gets `<branch>-<suffix>` from `BASE_REF` in its own worktree. | `["create-from-base"]` |
| 2, not ahead | Branch exists, no worktree, 0 commits ahead | Worktree attached to the existing branch (no `-b`) | `["attach-clean-branch"]` |
| 2, ahead | Branch exists, no worktree, commits ahead | Branch kept at the same SHA. This run gets a fresh branch. | `["create-from-base"]` |
| 3 | Branch does not exist | Branch and worktree created from `BASE_REF` | `["create-from-base"]` |
| Foreign | Branch is checked out by a worktree at another path | This run gets a fresh branch | `["create-from-base"]` |

Step3 contains no destructive or forced git command. The full list is
checked by test [A1](#testing-step3): `checkout -- .`, `clean` with any `-f`
flag, `reset --hard`, `rm -rf`, `branch -D`, `branch -f`/`--force`,
`worktree add -B`/`--force`, `worktree remove`, `worktree prune`,
`update-ref`, and any forced push (`--force`, `-f`, `--force-with-lease` or a
`+<refspec>`).
Branch existence is tested with
`git show-ref --verify --quiet refs/heads/<branch>`, not the glob-matching
`git branch --list`. A concurrent run that creates the same branch between the
helper call and `git worktree add -b` makes the add fail, and the step exits
non-zero rather than retrying with `-B`.

The push at the end of step3 (`git push -u origin "$BRANCH_NAME"`, never
forced) runs after `BRANCH_NAME` has been reassigned, so the kept branch and
its unpushed commits never reach `origin`.

### Step output

The slug path prints exactly one JSON line on stdout, the same shape as the
#342 path. All git output goes to stderr. The step's prompt tells the agent to
return that line verbatim, so `{{worktree_setup.branch_name}}` and
`{{worktree_setup.worktree_path}}` in later steps follow the resolved branch.

```json
{"branch_name": "feat/consensus-fix-issue-1626-1761300000-1-48213-4821", "worktree_path": "/repo/worktrees/feat/consensus-fix-issue-1626-1761300000-1-48213-4821", "commands_executed": ["create-from-base"], "setup_complete": true}
```

Before printing, step3 refuses a worktree path that contains `"`, `\` or a
control character, so the JSON passed to later steps can't be forged.

### Where the helper is found

Step3 uses the same five-place lookup order as step-04 of
`workflow-worktree.yaml`. It first sets `TOOLS_DIR` to the first of these
directories that contains `workflow_worktree_deconflict.sh` (each path below
is followed by `/amplifier-bundle/tools`):

1. `${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}`
2. `${REPO_PATH:-$(pwd)}`
3. `$(pwd)`
4. `${HOME:-/root}/.copilot`
5. `${HOME:-/root}/.amplihack`

Then it runs step-04's two lines unchanged:

```bash
DECONFLICT_HELPER="$TOOLS_DIR/workflow_worktree_deconflict.sh"
[ -f "$DECONFLICT_HELPER" ] || DECONFLICT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_worktree_deconflict.sh"
```

and calls the helper only when `[ -f "$DECONFLICT_HELPER" ]` holds.

If none of the five contains the helper, `TOOLS_DIR` is left at the last
one (`~/.amplihack/amplifier-bundle/tools`), the `REPO_PATH` fallback line
is tried, and if that file is missing too the step prints the
missing-helper warning shown below.

This is the one deliberate difference from step-04: step-04 tests each
directory for `workflow_worktree_sweep.sh`, step3 tests for
`workflow_worktree_deconflict.sh`. Step3 never runs the sweep helper, and a
directory that holds only a stray sweep helper must not be chosen as the
place to load the deconflict helper from.

Step3 never sets `REPO_PATH` itself, but inherits it when it is set in the
environment. When `AMPLIHACK_HOME` is unset, the order in practice is:
`REPO_PATH` if set in the environment, otherwise the current directory; then
`~/.copilot`; then `~/.amplihack`. When `AMPLIHACK_HOME` is set and contains
the helper, that copy wins.

Every place in this order is a place code is run from. The helper runs as
the user, with the user's privileges, before the guard, so a helper committed
to the repository can run any command. The checks in
[Validating the helper's answer](#validating-the-helpers-answer) limit only
the branch name a helper can hand back to step3; they do not limit what the
helper does while it runs. This is the same exposure step-04 already has.

To pin the helper to an installed copy, set `AMPLIHACK_HOME` to the installed
bundle (the directory that contains `amplifier-bundle/`). Its helper is then
tried first and wins over any copy in the repository; test
[S11](#testing-step3) checks this.

If no helper is found, step3 prints

```text
WARNING: workflow_worktree_deconflict.sh not found — skipping deconfliction; the guard below refuses if '<branch>' holds work.
```

on stderr and the guard decides:

| Branch state without a helper | Result |
|---|---|
| Dirty or ahead (State 1 or State 2) | `refuse_reset`: `ERROR` on stderr, exit 1, nothing changed |
| Clean | Reused unchanged, exit 0 |
| Missing | Created from `BASE_REF`, exit 0 |

A missing helper never blocks a fresh or clean run, and never leads to a reset.

Without the helper, the Foreign state gets no fresh branch. Step3 tries to
attach a worktree to the branch, `git worktree add` fails because another
worktree already has it checked out, and the step exits non-zero without
changing any ref or worktree.

### Validating the helper's answer

Step3 calls:

```bash
bash "$DECONFLICT_HELPER" resolve --base "$BASE_REF" \
  "$BRANCH_NAME" "$WORKTREE_DIR/$BRANCH_NAME" "$(pwd)"
```

and treats the output as untrusted. Each check fails closed with exit 1,
before any ref or worktree is created:

| Check | Failure message |
|---|---|
| Helper exits 0 | `ERROR: worktree branch deconfliction failed for '<branch>' …` |
| `git check-ref-format --branch "$RESOLVED_BRANCH"` passes | `ERROR: deconflict helper returned an invalid branch name …` |
| Only `[A-Za-z0-9._/-]` characters (`check-ref-format` accepts `"`, so this check is needed) | `ERROR: deconflict helper returned an unexpected branch name …` |
| Equals `<branch>` or starts with `<branch>-` | `ERROR: deconflict helper returned an unexpected branch name …` |

Once the branch name is settled, step3 also refuses a worktree path that
contains `"`, `\` or a control character, because that path goes into the JSON
line later steps read. This check runs before any branch, worktree or push is
made:

| Check | Failure message |
|---|---|
| `WORKTREE_DIR/<branch>` has no `"`, `\` or control character | `ERROR: worktree path contains a quote, backslash or control character — refusing to emit setup JSON.` |

After a rename, step3 prints
`INFO: branch '<old>' holds work or is owned by a foreign worktree — deconflicting to '<new>'.`
on stderr.

Step3's own messages differ from step-04's in the [Messages](#messages)
table: step3 prints the `deconflicting to` line above instead of
`INFO: using branch '<new>' instead of '<old>'.`, and adds the
`unexpected branch name` check, which also covers the character check. The helper's `WARNING: keeping …` and
`INFO: … foreign worktree …` lines are the same for both, and the
`refuse_reset` line uses the same format as step-04's.

### Fail-loud guard

The guard re-checks the resolved branch just before acting. If State 1 is
dirty or ahead, or State 2 is ahead, it prints:

```text
ERROR: refusing to reset existing_branch=feat/consensus-fix-issue-1626 at /repo/worktrees/feat/consensus-fix-issue-1626 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1). Re-run with existing_branch=feat/consensus-fix-issue-1626 to continue it, or move the work aside.
```

and exits 1. The format matches step-04's guard message. The changes field is
`yes`, `no` or `unknown`.

When the check can't give an answer, it assumes work and refuses: a failed
`git rev-list --count` is treated as 1 commit ahead, and a failed
`git status --porcelain` is treated as work and printed as
`uncommitted/untracked changes: unknown`.

To continue a kept branch, name it with `existing_branch` (the #342 path), as
in [Continuing an earlier branch on purpose](#continuing-an-earlier-branch-on-purpose).

### Known limitation

A relative `WORKTREE_DIR` is not normalised, so the worktree lookup can miss an
existing worktree and treat State 1 as State 2. The outcome is still safe: the
step attaches a clean branch or refuses, and never resets.

### Testing step3

`amplifier-bundle/recipes/tests/test-issue-1626-consensus-rerun-preserves-work.sh`
extracts the real step3 bash block (the first ` ```bash ` fence after
`- id: "step3-setup-worktree"`) and runs it under `env -i` against scratch
repos with a local bare `origin`. Scratch lives under `$TMPDIR`; nothing
touches the network or `/tmp`. `HOME` points at a scratch directory, and the
test checks that neither `$HOME/.amplihack` nor `$HOME/.copilot` exists before
any scenario runs, so a real installed helper can never be picked up by
lookup steps 4 and 5.

| Case | Checks |
|---|---|
| A0 | The extracted block passes `bash -n` |
| A1 | No destructive or forced command anywhere in the step3 block, with a self-check that the pattern matches `git worktree add -B x` and `git push --force` |
| A2–A6 | The block contains `resolve --base "$BASE_REF"`, `check-ref-format --branch "$RESOLVED_BRANCH"`, `deconfliction failed`, `refuse_reset` and the final `printf` JSON line |
| A7 | Branch existence uses `show-ref --verify`, never `git branch --list` (which matches globs) |
| A8 | The helper lookup matches step-04: the block contains `${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}`, `DECONFLICT_HELPER="$TOOLS_DIR/workflow_worktree_deconflict.sh"`, the repo-local fallback line and `if [ -f "$DECONFLICT_HELPER" ]`, and does not contain `[ -n "$DECONFLICT_HELPER" ]`, `for _candidate` or the old "differs from step-04 on purpose" comment |
| S0 | Helper present, fresh repo: creates `<branch>` at `WORKTREE_DIR/<branch>`; stdout is exactly one JSON line |
| S1 | Helper present, State 1 dirty and ahead: untracked file, modified file and unpushed commit survive byte-for-byte; original branch SHA unchanged; new `branch_name` starts with `<original>-`; its worktree is registered; `origin` lacks the unpushed commit |
| S2 | Helper present, State 2 ahead: original branch at the same SHA; new suffixed branch |
| S3 | Helper present, clean State 1: same `branch_name`, same `worktree_path`, exit 0 |
| S4 | Helper present, clean State 2: same `branch_name`, worktree re-attached to the existing branch at the same SHA, exit 0 |
| S5, S6 | Helper hidden, dirty or ahead in State 1 / State 2: non-zero exit, `ERROR` on stderr, all work, SHAs and branches intact |
| S7 | Helper hidden, clean branch: reused, exit 0 |
| S7b | Helper hidden, fresh repo: created, exit 0 |
| S8 | Stub helper prints `main`: non-zero exit, no ref or worktree changed |
| S9 | Stub helper prints `<original>-a"b`: non-zero exit, no ref changed |
| S10 | Stub helper exits non-zero: non-zero exit, stderr names `deconfliction failed`, no ref changed |
| S11 | `AMPLIHACK_HOME` first: with `AMPLIHACK_HOME` set, its helper is used before the repo-local copy |
| S11b | `AMPLIHACK_HOME` empty, dirty State 1, helper planted in the repo: (a) a helper that prints `<original>-planted` is used: exit 0, `branch_name` is `<original>-planted`, `worktree_path` is `WORKTREE_DIR/<original>-planted` and is registered, and the original worktree's files and `HEAD` are unchanged; (b) a helper that prints `other` (no `<original>` prefix): non-zero exit, `ERROR` on stderr, files, branch heads and `git worktree list` unchanged |
| S12 | `WORKTREE_DIR` contains `"`: non-zero exit with `refusing to emit setup JSON`, and no branch, worktree or pushed ref exists |

Run it:

```bash
bash amplifier-bundle/recipes/tests/test-issue-1626-consensus-rerun-preserves-work.sh
```

Confirm it fails against the old step3:

```bash
git show origin/main:amplifier-bundle/recipes/consensus-issue-worktree.yaml \
  > "$TMPDIR/consensus-issue-worktree.main.yaml"
WORKTREE_YAML="$TMPDIR/consensus-issue-worktree.main.yaml" \
  bash amplifier-bundle/recipes/tests/test-issue-1626-consensus-rerun-preserves-work.sh
# exits 1: 10 passed, 38 failed — including A1 (destructive commands present),
# A8 (old helper lookup), S1–S4 and S11b(a)/(b)
```

CI runs it in the step after the #1565 test.

---

## Related

- [Worktree branch deconfliction](../../amplifier-bundle/tools/workflow_worktree_deconflict.md): the helper and its `--base` flag
- [Relative-path-safe, idempotent worktree setup (#1121)](issue-1121-relative-repo-path.md): the three-state guard
- [Leak-proof worktree setup (#840)](issue-840-leak-proof-worktree.md): orphan sweep
