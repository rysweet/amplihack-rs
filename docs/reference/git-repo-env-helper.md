---
title: Git Repository Environment Helper
type: reference
updated: 2026-11-09
related: issue #2033, issue #1984, issue #1278, issue #1764
---

# Git Repository Environment Helper

> [Home](../index.md) > Reference > Git Repository Environment Helper

`amplifier-bundle/tools/git-repo-env.sh` is the one shell copy of the git
variables that pick a repository (`GIT_DIR`, `GIT_WORK_TREE` and ten more)
and of the variables that name an outer recipe run. Every shell step that runs
a repository's tests or hooks sources it and clears those variables first, so
the tests act on their own checkout and their own run state.

## Contents

- [Why it exists](#why-it-exists)
- [Quick start](#quick-start)
- [What the helper defines](#what-the-helper-defines)
- [The two lists](#the-two-lists)
- [Who sources it](#who-sources-it)
- [Finding the helper](#finding-the-helper)
- [The QA evidence fields](#the-qa-evidence-fields)
- [The drift guard](#the-drift-guard)
- [The end-to-end test](#the-end-to-end-test)
- [What is not cleared](#what-is-not-cleared)
- [Limits](#limits)
- [Related](#related)

## Why it exists

Git reads `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and similar variables
before it looks at the current directory. A process that inherits them and
then runs `git add` or `git commit` acts on the repository they name, not on
the one it was started in. Git sets several of them for the hooks it runs.

auto-drive runs other repositories' tests in its QA and merge-readiness
steps. In mistt-repo/Product-Management an auto-drive merge-readiness phase
ran that repository's tests with `GIT_*` set; the tests' git commands
committed into the real PR worktree, about 1,900 deletions on PR #742
(#2033). amplihack-rs's own pre-push hook already stripped these variables,
but other repositories' hooks do not, so auto-drive now clears them itself.

The same kind of leak broke `recipe::run` unit tests run inside a recipe-run
session: the outer run's `AMPLIHACK_RECIPE_RUN_ID`, `AMPLIHACK_RUN_DIR` and
related variables reached the fake runner the tests start (#1984). The Rust
side of that fix is in
[Run recipe::run Tests Inside a Recipe Session](../howto/run-recipe-tests-inside-a-recipe-session.md).

## Quick start

Source the helper, then call the clear functions before the first `git`,
`cargo`, `npm` or `pytest` command:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/git-repo-env.sh"
amplihack_clear_git_repo_env
amplihack_clear_run_identity_env
printf 'git: %s\nrun: %s\n' "$AMPLIHACK_GIT_ENV_CLEARED" "$AMPLIHACK_RUN_ENV_CLEARED"
```

Example output from a shell where git set `GIT_DIR` and `GIT_INDEX_FILE` (a
hook), inside a recipe run. The `run:` names depend on which variables the
session has set, so yours may differ:

```text
git: GIT_DIR GIT_INDEX_FILE
run: AMPLIHACK_RECIPE_RUN_ID AMPLIHACK_RUN_DIR AMPLIHACK_SESSION_TREE_DIR AMPLIHACK_TREE_ID
```

From a clean shell both lines read `none`.

## What the helper defines

Sourcing the file only defines the names below. It changes no variable, runs
no command and prints nothing. It is bash 3.2 safe (no arrays, `local`,
`mapfile` or `declare -g`) and works under `set -euo pipefail`. Its last
command is a function definition, so sourcing it always returns 0.

| Name | Kind | Value or effect |
| --- | --- | --- |
| `AMPLIHACK_GIT_REPO_ENV_VARS` | variable | The 12 git names, separated by spaces, in the order of [the git list](#the-git-list). |
| `AMPLIHACK_RUN_IDENTITY_VARS` | variable | The 5 run-identity names, separated by spaces, in the order of [the run-identity list](#the-run-identity-list). |
| `amplihack_clear_git_repo_env` | function | Unsets every name in `AMPLIHACK_GIT_REPO_ENV_VARS` that is set, and sets `AMPLIHACK_GIT_ENV_CLEARED`. Returns 0. |
| `amplihack_clear_run_identity_env` | function | Unsets every name in `AMPLIHACK_RUN_IDENTITY_VARS` that is set, and sets `AMPLIHACK_RUN_ENV_CLEARED`. Returns 0. |
| `AMPLIHACK_GIT_ENV_CLEARED` | variable, set by the function | The names that were set and are now unset, in list order, separated by spaces; or `none`. |
| `AMPLIHACK_RUN_ENV_CLEARED` | variable, set by the function | The same for the run-identity names. |

Behaviour that callers rely on:

- **Set means set.** A name counts as set when `${!name+x}` is non-empty, so
  a variable set to the empty string is cleared and recorded.
- **The current shell.** The functions use `builtin unset` in the calling
  shell. Call them directly, never as `$(amplihack_clear_git_repo_env)`: a
  command substitution clears only its own subshell.
- **Names, never values.** The `*_CLEARED` variables hold variable names.
  Values, which can be paths, are never recorded.
- **A second call records `none`.** Calling a function twice is harmless, but
  the second call overwrites `*_CLEARED` with `none`. Read the result straight
  after the first call (see [the QA evidence fields](#the-qa-evidence-fields)).
- **The environment cannot change the lists.** Both list variables are
  assigned unconditionally, never as `${VAR:-default}`. Sourcing the file
  redefines both functions, replacing any exported `BASH_FUNC_` import of the
  same name, and `builtin unset` is used so an exported `unset` function has
  no effect. This guards against accidental overrides only: an exported
  `builtin` function, `BASH_ENV` or any other `BASH_FUNC_*` import can defeat
  it, and whoever can set those can replace `git` as well.

## The two lists

### The git list

The same 12 names, in the same order, as
`amplihack_git::REPOSITORY_ENV_VARS` in
[`crates/amplihack-git/src/lib.rs`](../../crates/amplihack-git/src/lib.rs):

```text
GIT_ALTERNATE_OBJECT_DIRECTORIES
GIT_CEILING_DIRECTORIES
GIT_COMMON_DIR
GIT_DIR
GIT_DISCOVERY_ACROSS_FILESYSTEM
GIT_INDEX_FILE
GIT_INDEX_VERSION
GIT_NAMESPACE
GIT_OBJECT_DIRECTORY
GIT_PREFIX
GIT_QUARANTINE_PATH
GIT_WORK_TREE
```

To add or remove a name, change the Rust constant and the helper in the same
commit. [The drift guard](#the-drift-guard) fails CI if they differ.

### The run-identity list

```text
AMPLIHACK_RECIPE_RUN_ID
AMPLIHACK_RUN_DIR
AMPLIHACK_RUN_ID
AMPLIHACK_SESSION_TREE_DIR
AMPLIHACK_TREE_ID
```

These name the outer recipe run. A repository's tests that start their own
recipe run, as amplihack-rs's do, must not join or write into the outer one.

## Who sources it

| Caller | Clears git list | Clears run-identity list | When |
| --- | --- | --- | --- |
| [`scripts/git-hook-cargo.sh`](../../scripts/git-hook-cargo.sh) (amplihack-rs `pre-commit` and `pre-push`) | yes | no | First thing after `set -euo pipefail`. |
| [`autodrive_qa_evidence.sh`](../../amplifier-bundle/tools/autodrive_qa_evidence.sh) | yes | yes | Straight after its helper directory `H` is found, before `cd "$REPO_PATH"`, the scan subshell and `git rev-parse HEAD`. |
| `autodrive-merge-evidence.yaml` `step-01-sync-with-base` | yes | yes | Before its first `git` command. Its `git push` runs the target repository's pre-push hook and tests. |
| `autodrive-merge-evidence.yaml` `step-02-qa-team-scenarios` | yes | yes | Before `git rev-parse` and before it runs `autodrive_qa_evidence.sh`. |
| `autodrive-merge-round.yaml` `step-04` (fix and commit) | yes | yes | Twice, because each of the agent's shell calls is a new process that inherits the agent's environment. The "Running tests or hooks" instruction tells the agent to start every shell call that runs tests, hooks, `cargo`, `npm` or `pytest` with the helper line, so the tests it runs while fixing blockers see no inherited `GIT_*`. The bash block that commits and pushes loads it again; there it replaces the old partial `unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE`, and `unset GH_REPO` stays. The prompt is advice to an agent, not a shell wrapper: a test command the agent runs without the line is not covered. |

`autodrive-merge-round.yaml` `step-00-tools-dir` lists `git-repo-env.sh` in
`NEED`, so a tools directory without the helper is refused before the round
starts. Step-00 runs no git command and does not search the git toplevel, so
an inherited `GIT_WORK_TREE` cannot choose the directory step-04 loads the
helper from.

[`autodrive_qa_sources.sh`](../../amplifier-bundle/tools/autodrive_qa_sources.sh)
does not load the helper. Its only loader, `autodrive_qa_evidence.sh`, has
cleared both lists before it sources the file.

Steps that run no repository command are unchanged:
the steps of `auto-drive-to-merge.yaml`, `autodrive-prerequisites`,
`autodrive-check-unmerged`, and the `type: recipe` delegations (the delegated
recipe's own steps do the clearing).

`autodrive_trust_git` in `autodrive_trust.sh` does not source the helper.
`autodrive_trust.sh` is one of the pinned trusted auto-drive files, so
loading another helper there would widen that trusted set. It holds the one recorded copy instead:
`AUTODRIVE_TRUST_GIT_UNSET_VARS` is the helper's 12 names, in the helper's
order, followed by `GIT_REPLACE_REF_BASE`, and `autodrive_trust_git` removes
each of them with `env -u` before git runs. The
[drift guard](#the-drift-guard) fails CI when that copy differs from the
helper's list.

## Finding the helper

No caller asks git where the helper is: `GIT_DIR` or `GIT_WORK_TREE` may still
be set at that point, and a planted worktree could then supply the file.

- **`git-hook-cargo.sh`** finds it from its own path:
  `"$(unset CDPATH; cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)/amplifier-bundle/tools/git-repo-env.sh"`.
- **`autodrive_qa_evidence.sh`** loads it from its own directory `$H`, like
  its other helpers.
- **`autodrive-merge-round.yaml` step-04** uses `{{autodrive_tools_dir}}`,
  which step-00 already checked. The load is in the bash block of the step's
  agent prompt, so the agent runs it.
- **`autodrive-merge-evidence.yaml` steps 01 and 02** use the same tools
  directory: merge-round passes `autodrive_tools_dir` into the sub-recipe,
  so the helper is looked up once per round, in step-00. When
  merge-evidence runs on its own, that value is empty and the steps use
  `$AMPLIHACK_HOME/amplifier-bundle/tools`. The load line is the same as
  step-04's. Step-02 finds `autodrive_qa_evidence.sh` by searching
  `$AMPLIHACK_HOME`, `$REPO_PATH`, `~/.copilot` and `~/.amplihack`. It no
  longer searches the directory from `git rev-parse --show-toplevel` (`$TOP`).

Every loader refuses a helper that is missing or is a symbolic link, and
stops with a non-zero exit:

| Caller | Message (stderr) | Exit |
| --- | --- | --- |
| `git-hook-cargo.sh` | `ERROR: git-repo-env.sh missing or a symlink at <path>; refusing to run tests with inherited GIT_* (#2033)` | 1 |
| `autodrive_qa_evidence.sh` | `ERROR: autodrive-qa-helpers-not-found: autodrive_state.sh, autodrive_trust.sh or git-repo-env.sh is missing` | 1, and no evidence file is written |
| merge-evidence steps 01 and 02 | `STOP: cannot load git-repo-env.sh from the tools directory (#2033)` | 1 |
| merge-round step-04 | the same message | 1, from the agent's bash block; the step itself has no exit of its own |

The tests check these exact strings, so the code and this table cannot drift
apart: the [drift guard](#the-drift-guard) checks the hook's message and the
three `STOP` lines, and the [end-to-end test](#the-end-to-end-test) checks the
`autodrive-qa-helpers-not-found` message.

## The QA evidence fields

`qa-evidence.json` has two fields at the end of its one JSON line. Both are
strings and pass through the same `san` and `ascii` filters as the other
fields. Programs that read the evidence look fields up by key, so existing
readers (`autodrive_round_evidence.sh` and the Rust evidence reducer) are
unaffected.

| Field | Value |
| --- | --- |
| `git_env_cleared` | `AMPLIHACK_GIT_ENV_CLEARED` from the tool's first call: the git names that were set when the tool started, in list order, or `none`. |
| `run_env_cleared` | `AMPLIHACK_RUN_ENV_CLEARED` from the tool's first call, or `none`. |

The tool captures both values straight after its own call.

A run started from a hook, with every git name pointing at another
repository, ends like this (other fields left out):

```json
{"qa_status":"PASS", "...":"...", "git_env_cleared":"GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE","run_env_cleared":"none"}
```

To check a round's evidence by hand:

```bash
jq -r '.git_env_cleared, .run_env_cleared' "$AUTODRIVE_STATE_DIR/qa-evidence.json"
# Output from a clean environment:
# none
# none
```

`qa_status`, `qa_reason`, `head_sha` and every other field mean what they
meant before; see the [evidence table](auto-drive-to-merge.md#criterion-1-qa-team-scenarios-by-repository-type)
in the auto-drive reference.

## The drift guard

[`tests/issue_1278_git_env_isolation.sh`](../../tests/issue_1278_git_env_isolation.sh)
runs in CI and fails when the shell list and the Rust list part ways, or when
a refusal message changes. It checks:

1. **Same names, same order.** The helper's `AMPLIHACK_GIT_REPO_ENV_VARS` and
   `REPOSITORY_ENV_VARS` in the Rust source, compared in both directions with
   `comm -3`, then compared in order.
2. **Each git name clears.** For each name: set it, source the helper, call
   `amplihack_clear_git_repo_env`, and check the name is unset and
   `AMPLIHACK_GIT_ENV_CLEARED` is exactly that name. With nothing set it must
   be `none`.
3. **Each run-identity name clears**, the same way, and
   `AMPLIHACK_HOME`, `AMPLIHACK_SESSION_DEPTH` and `AMPLIHACK_MAX_DEPTH`
   survive the call.
4. **No hand-written copies.** `scripts/git-hook-cargo.sh`,
   `autodrive_qa_evidence.sh` and `autodrive_qa_sources.sh` have no line naming
   `GIT_QUARANTINE_PATH`; only the helper may list the names. The one
   exception is `autodrive_trust.sh` (see [Who sources it](#who-sources-it)):
   its `AUTODRIVE_TRUST_GIT_UNSET_VARS` must equal the helper's list followed
   by `GIT_REPLACE_REF_BASE`, and `autodrive_trust_git`, run with all 13 names
   set, must start git with none of them in its environment.
5. **The hook still clears.** The existing behaviour check runs
   `git-hook-cargo.sh` with every name set and confirms its children see none.
   It also runs the hook with the helper missing and with the helper replaced
   by a symbolic link, and checks for the exact `ERROR: git-repo-env.sh missing
   or a symlink` message in [the refusal table](#finding-the-helper) and exit 1.
   It checks that `autodrive-merge-evidence.yaml` steps 01 and 02 and
   `autodrive-merge-round.yaml` step-04 all contain the exact `STOP: cannot
   load git-repo-env.sh from the tools directory (#2033)` line.

Before the change is merged, each check is shown to fail on a broken copy of
the helper or list (a mutation check), and that failure is recorded in the
pull request.

Run it locally:

```bash
bash tests/issue_1278_git_env_isolation.sh
# Output ends: PASS: git env isolation (12 git names, 5 run-identity names)
```

## The end-to-end test

[`amplifier-bundle/recipes/tests/test-issue-2033-qa-git-env.sh`](../../amplifier-bundle/recipes/tests/test-issue-2033-qa-git-env.sh)
runs `autodrive_qa_evidence.sh` against a caller repository and a decoy
repository, both under one `mktemp -d` directory. It sets a temporary `HOME`,
`GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL=/dev/null`, so no real git
configuration, hook path or `~/.amplihack` is read.

The acceptance suite behaves like a normal test suite: it creates a scratch
repository under `TMPDIR`, runs `git init` and `git commit --allow-empty` there
with no `-C`, and fails unless `git rev-parse --show-toplevel` is that scratch
directory. With the variables inherited, the commit lands in whichever
repository they name. A *snapshot* of a repository is the `sha256sum` of its
index, `.git/HEAD`, `git rev-parse HEAD`, `git for-each-ref`, `git ls-files -s`,
the hash of every worktree file, and the list of object files.

| Case | Setup | Expected |
| --- | --- | --- |
| Control | The suite run directly, without the tool, with all 12 git names at a throwaway repository. | The suite fails and the throwaway repository's snapshot changes. This shows the suite can detect a leak. |
| 1. Clean | No git or run-identity variable set. | `qa_status` is `PASS`; `git_env_cleared` and `run_env_cleared` are `none`; the caller's snapshot is unchanged. |
| 2. Decoy | All 12 git names point into the decoy; all 5 run-identity names hold `outer-…` values. | Exit status and `qa_status` equal case 1. `head_sha` is the caller's commit. The caller's and the decoy's snapshots are both unchanged. `git_env_cleared` lists all 12 names in order; `run_env_cleared` lists all 5. |
| 3. Self | All 12 git names point at `REPO_PATH`'s own `.git`, as in the Product-Management #742 damage. | `qa_status` is `PASS` and the caller's snapshot is unchanged. |
| 4. Commit here (extra) | A suite that commits in `REPO_PATH` itself; git names at the decoy. | The commit lands in the caller and the decoy's snapshot is unchanged. This shows where git wrote, so it is not the acceptance proof. |
| 5. Helper missing | `git-repo-env.sh` removed from a copy of the tools directory. | Exit 1, stderr contains the exact message `ERROR: autodrive-qa-helpers-not-found: autodrive_state.sh, autodrive_trust.sh or git-repo-env.sh is missing`, no evidence file, the caller's snapshot is unchanged. |
| 6. Helper is a symlink | `git-repo-env.sh` replaced by a symbolic link. | The same as case 5. |

The test asserts the exact `PASS` line below as well as each case. It uses
only exit statuses and before-and-after snapshots; it has no
sleep, timeout or clock read. CI runs it next to
`test-issue-1531-qa-evidence-json.sh`:

```bash
bash amplifier-bundle/recipes/tests/test-issue-2033-qa-git-env.sh
# Output ends: PASS: 6/6 cases (issue #2033)
```

## What is not cleared

| Variable | Why it stays |
| --- | --- |
| `AMPLIHACK_SESSION_DEPTH`, `AMPLIHACK_MAX_DEPTH` | They feed the recursion guard. Resetting them in a QA child would let it nest deeper than the outer run allows. (The Rust test helper does clear them; see the [how-to](../howto/run-recipe-tests-inside-a-recipe-session.md).) |
| `AMPLIHACK_HOME`, `AUTODRIVE_*`, `RECIPE_VAR_*` | Inputs the QA tool and the steps read. |
| `GIT_AUTHOR_*`, `GIT_COMMITTER_*` | Commit identity, set by `git-identity.sh`. |
| `GIT_TERMINAL_PROMPT` | Keeps git from asking for credentials ([#1764](../howto/keep-tests-from-prompting-for-git.md)). |
| `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_<n>` | Not repository selectors. Adding them is a follow-up that changes the Rust constant and the helper together. |

## Limits

- **Not a sandbox.** Clearing the variables stops tests from acting on another
  repository by accident. The suite still runs the branch's code, as
  described under [Running the suite commands](auto-drive-to-merge.md#running-the-suite-commands).
- **Equal trust for `REPO_PATH`.** Step-00 of the merge round may pick the
  tools directory from the PR's own tree. That trusts the tree no more than
  running its QA tool and tests already does, and `$AMPLIHACK_HOME` is
  searched first.
- **Symlinked parent directories** are not detected, the same as for the other
  autodrive helpers.
- **Other repositories** need amplihack installed: `amplihack install` copies
  the whole `tools/` directory, the helper included.

## Related

- [Auto Drive To Merge Reference](auto-drive-to-merge.md) - the merge round and the QA evidence
- [Run recipe::run Tests Inside a Recipe Session](../howto/run-recipe-tests-inside-a-recipe-session.md) - the Rust side of #1984
- [Keep Tests from Prompting for Git Credentials](../howto/keep-tests-from-prompting-for-git.md) - `GIT_TERMINAL_PROMPT` and the `[env]` table
- [Git Utils API](git-utils-api.md) - the Rust git helpers
