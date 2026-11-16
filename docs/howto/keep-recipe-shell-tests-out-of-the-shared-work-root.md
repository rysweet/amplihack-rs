---
title: Keep Recipe Shell Tests out of the Shared Work Root
type: howto
updated: 2026-11-16
related: issue #2712, issue #1808
---

# Keep Recipe Shell Tests out of the Shared Work Root

A recipe run exports `AMPLIHACK_WORK_ROOT` and `AMPLIHACK_WORK_ROOT_EXPLICIT=1`,
and every process it starts inherits them, including the pre-commit and
pre-push suites. With both set, `workflow_worktree_root.sh base` puts a new
worktree under `<work root>/worktrees/<sanitised name>-<hash8>` rather than
under `<main>/worktrees`. A recipe shell test suite that builds its fixture
repository inside its own scratch directory therefore created its worktrees
outside that scratch, and its `EXIT` trap, which removes only the scratch,
left them behind with a `.git` file pointing at nothing (#2712).

The suites now source one shared setup fragment,
`amplifier-bundle/recipes/tests/recipe-test-env.sh`, which clears the
inherited work root. Rust tests that start a suite go through the `isolated()`
command helper, which clears the whole environment. This guide covers running
a suite from inside a recipe run, adding the setup to a new suite, and the
behaviour test that holds the fix.

## Contents

- [Run a suite from inside a recipe run](#run-a-suite-from-inside-a-recipe-run)
- [What the shared setup clears](#what-the-shared-setup-clears)
- [Add the setup to a new suite](#add-the-setup-to-a-new-suite)
- [Which suites source it](#which-suites-source-it)
- [Start a suite from a Rust test](#start-a-suite-from-a-rust-test)
- [The behaviour test](#the-behaviour-test)
- [Troubleshooting](#troubleshooting)
- [Related](#related)

## Run a suite from inside a recipe run

Nothing extra is needed, and nothing has to be unset by hand. The same command
passes in a clean shell and inside a recipe run, and in both cases the suite's
worktrees live and die inside its own scratch directory:

```bash
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
# Output ends with: PASS
```

To confirm the suite leaves the shared work root alone, compare before and
after:

```bash
WORKTREES="${AMPLIHACK_WORK_ROOT:?not inside a recipe run}/worktrees"
BEFORE="$(ls -1 "${WORKTREES}" | wc -l)"
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
test "$(ls -1 "${WORKTREES}" | wc -l)" = "${BEFORE}" && echo "work root untouched"
# Output: work root untouched
```

## What the shared setup clears

`amplifier-bundle/recipes/tests/recipe-test-env.sh` is sourced, never
executed. It produces no output, creates nothing, and is safe to source twice.

```bash
# shellcheck shell=bash
# recipe-test-env.sh — sourced (never run) by the recipe shell test suites.
#
# No shebang and no execute bit, deliberately: unlike the sibling fixture
# expected-checks-fixtures.sh, which keeps a shebang while also being
# source-only, this file carries nothing that would read as a runnable suite.
#
# A recipe run exports AMPLIHACK_WORK_ROOT and AMPLIHACK_WORK_ROOT_EXPLICIT=1,
# and every process it starts inherits them. workflow_worktree_root.sh base
# then puts a suite's worktrees under <work root>/worktrees/<name>-<hash8>
# instead of under the suite's own scratch, where its EXIT trap would remove
# them (#2712, #1808).
unset AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID
```

| Variable | Why it is cleared |
| --- | --- |
| `AMPLIHACK_WORK_ROOT` | With `_EXPLICIT=1` it moves new worktrees to `<work root>/worktrees`, outside the suite's scratch and outside its `EXIT` trap. |
| `AMPLIHACK_WORK_ROOT_EXPLICIT` | The flag that turns the work root into the worktree base (#1808). Clearing only this one leaves a stale root for anything that reads it directly. |
| `AMPLIHACK_RUN_DIR` | A run's artefact directory holds prompts and step output. A suite writing fixtures there mixes test content into a real run's record. |
| `AMPLIHACK_RUN_ID` | Pairs with the run directory; a suite that reports it would attribute its own artefacts to the enclosing run. |

The list stops there. Credential and git-identity variables such as
`GH_TOKEN`, `SSH_AUTH_SOCK` and `GIT_CONFIG_*` are not touched: dropping them
changes which remote operations a suite can perform, which is a separate
decision from where worktrees land.

Two facts about the file's placement are easy to get wrong. CI does not
discover suites by scanning the directory; `.github/workflows/ci.yml` names
each one in its own step, as `bash amplifier-bundle/recipes/tests/<suite>.sh`.
A new file in that directory is therefore never picked up, whatever it is
called, and the absent execute bit removes the remaining way it could be run
by accident. Directory-wide static guards are a different matter: the scan in
`crates/amplihack-cli/tests/issue_1570_recipe_text.rs:418-427` reads every file under
`amplifier-bundle/recipes/tests` regardless of name, so the fragment is
checked by it. A file containing one `unset` has nothing for it to catch.

### Why clear rather than redirect

Pointing `AMPLIHACK_WORK_ROOT` at a directory inside the suite's scratch also
keeps the worktrees contained, but it changes their layout to the explicit
one, `<root>/worktrees/<name>-<hash8>`. The suites assert on the
non-explicit layout: `run_worktree_case` in
`test-default-workflow-reliability.sh` expects the worktree at
`${case_dir}/worktrees/${branch_name}` (line 919). Clearing both variables
restores exactly the layout those assertions already encode. A suite that
needs explicit-root behaviour sets the variables itself, per case, as
`test-issue-1808-worktree-base.sh` does for its `base` cases.

## Add the setup to a new suite

Put the same four lines in every suite, immediately after the script-directory
resolution and before any step extraction, fixture build, scratch allocation or
execution:

```bash
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
TEST_ENV="${SCRIPT_DIR}/recipe-test-env.sh"
[[ -f "${TEST_ENV}" ]] || { echo "HARNESS-ERROR: missing ${TEST_ENV}" >&2; exit 2; }
# shellcheck source=recipe-test-env.sh
. "${TEST_ENV}"
```

Three details carry weight:

- **Resolve from `${BASH_SOURCE[0]}`, never from the current directory or
  `PATH`.** A `source` argument with no slash is searched along `PATH`, so the
  path must always carry a directory component and always be quoted.
- **Keep the `[[ -f ]]` guard.** Without it a rename or an incomplete install
  silently reverts the suite to the leaking behaviour. Exit 2 is the harness
  error code the suites already use.
- **Keep the block byte-identical across suites.** `grep -n recipe-test-env.sh
  amplifier-bundle/recipes/tests/*.sh` then shows the guard on the adjacent
  line everywhere, which is how a missing guard is spotted.

The fragment stays repository content. Do not generate it at runtime, fetch
it, copy it through `TMPDIR` or read it from `HOME`: sourcing executes with
the suite's full privileges, so provenance is the whole control.

## Which suites source it

The selection criterion is mechanical: a suite adopts the fragment if it runs
`git worktree add`, directly or through a step body it extracts. Thirteen
suites match. Three of them behave differently than before; the other ten take
the source as a standing contract, because they pass an explicit path to
`git worktree add` and so never consult the work root at all.

| Suite | Effect |
| --- | --- |
| `test-default-workflow-reliability.sh` | Behaviour change. It executes the extracted step-04 body with an inherited environment and was the demonstrated leaker. |
| `test-issue-1808-worktree-base.sh` | The inline `unset` it carried, of the same four names, is replaced by the shared source. |
| `test-issue-2061-sweep-no-push.sh` | It cleared only `AMPLIHACK_WORK_ROOT_EXPLICIT` inline; now it clears all four. |
| `test-issue-1121-relative-repo-path.sh`, `test-issue-1494-1560-worktree-safety.sh`, `test-issue-1543-pr-head-worktree.sh`, `test-issue-1565-rerun-preserves-dirty-worktree.sh`, `test-issue-1626-consensus-rerun-preserves-work.sh`, `test-issue-1671-hook-rejection-repair.sh`, `test-issue-840-worktree-leak-proof.sh`, `test-foreign-worktree-deconflict.sh`, `test-issue-962-implementation-evidence-fallback.sh`, `test-checkpoint-scoped-format.sh` | Inert today; they take the contract so a later case that does start a worktree from the inherited environment is covered. |

Several suites mention worktrees without creating one, and they are left
alone:

- `test-issue-1937-no-stray-git.sh` executes three of the adopting suites as
  children. They source the fragment themselves, so adding it to the parent
  would clear the same four names twice and prove nothing.
- `test-issue-1060-existing-pr-is-idempotent.sh` has a `make_worktree()`
  helper, but it is a plain `git init` and clone pair, not a linked worktree.
- `test-issue-1337-loop-health-evaluator.sh`,
  `test-issue-962-runtime-artifact-ladders.sh`,
  `test-issue-2033-qa-git-env.sh`, `test-issue-1516-qa-command-selection.sh`,
  `test-issue-2186-expected-checks.sh`, `test-issue-1763-qa-scope.sh` and
  `test-auto-drive-to-merge.sh` name worktrees in comments or in helper
  filenames and create none.

A fourteenth suite added later does not inherit the contract automatically.
Nothing fails if it omits the source, by design: this work adds no guard
check. Copy the block when you add a suite that calls `git worktree add`.

## Start a suite from a Rust test

A Rust test that executes a recipe shell suite starts `bash` through
`isolated()` from `tests/support/isolated_amplihack.rs`. It calls `env_clear()`
and carries over a short allowlist, so no `AMPLIHACK_WORK_ROOT*` can reach the
child even if the suite forgets to source the setup:

```rust
let output = crate::isolated_amplihack::isolated("bash")
    .arg(suite_path)
    .current_dir(&repo_root)
    .output()
    .expect("run the recipe shell suite");
assert!(output.status.success(), "suite failed:\n{}", String::from_utf8_lossy(&output.stderr));
```

`isolated()` comes first; the test's own `.env()`, `.env_remove()`,
`.current_dir()` and `.stdin()` calls follow and still win, which preserves
deliberate injections such as the `GIT_DIR` and `GIT_COMMON_DIR` values in
`issue_1671_hook_rejection_repair_test.rs`.

Two consequences to expect:

- `isolated()` sets `HOME` to a temp home shared by the test process. A suite
  that needs its own writable home sets `.env("HOME", <tempdir path>)` after
  `isolated()` — never a fixed path under `/tmp`.
- It also applies `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=true`,
  `SSH_ASKPASS=true` and a null stdin, and no `GH_TOKEN` survives the clear. A
  fixture that reached a real remote fails closed instead of waiting on a
  prompt. Point the fixture at a local remote; do not add a token back with a
  trailing `.env()`.

The sites this work changes are the six that started a suite with a bare
`Command::new("bash")`, all under `tests/integration/`: two in
`issue_1337_loop_health_evaluator_test.rs`, one in
`issue_1671_hook_rejection_repair_test.rs` and three in
`auto_drive_to_merge_test.rs`. They now read `isolated("bash")`, which is
also how every other isolated bash spawn in the suite directory is written,
so the whole set is:

```
grep -rn 'isolated("bash")' tests/integration/
```

The #1671 site previously carried its own `env_clear()` and a four-name
allowlist; that is now `isolated()`, with its deliberate `GIT_DIR`,
`GIT_COMMON_DIR` and fixture `HOME` injections unchanged below it.

One site already follows the pattern and is left as it is:
`crates/amplihack-cli/tests/issue_1808_run_cleanup/scripts.rs:355` starts
`test-issue-1808-worktree-base.sh` through `isolated()` and then calls
`.env_remove()` on three of the four names. The removals are now redundant
with both `isolated()` and the suite's own source, but they are harmless and
record the test's intent.

## The behaviour test

`tests/integration/issue_2712_recipe_test_work_root.rs`, registered in
`tests/integration/main.rs`, reproduces the inheritance rather than depending
on the developer's shell:

1. It creates a work root in a `tempfile::TempDir`, outside the suite's own
   scratch.
2. It starts `test-default-workflow-reliability.sh` through `isolated()` with
   `AMPLIHACK_WORK_ROOT` set to that directory, `AMPLIHACK_WORK_ROOT_EXPLICIT=1`,
   and `HOME` and `TMPDIR` inside temp directories.
3. It asserts both conditions with distinct messages, checking the second even
   when the first fails: the suite exited 0, and the outside work root holds no
   entries. A revert produces both symptoms, and the failure message should say
   which.

The integration files are modules of one test binary rooted at
`tests/integration/main.rs`, so the module path is the filter, per the
convention recorded at `bins/amplihack/Cargo.toml:63-72`:

```bash
cargo test -p amplihack --test integration issue_2712_recipe_test_work_root::
# Output includes: test ...::outside_work_root_stays_empty ... ok
```

The test takes roughly 28 seconds, which is the cost of covering the suite that
actually leaked. The four-second `test-issue-1808-worktree-base.sh` was the
cheaper candidate and was rejected: it already cleared the variables, so it
would pin the setup without exercising the path that produced the orphans.

The test deletes nothing. `TempDir` cleans up on drop, and the inherited work
root is read with `read_dir` and `symlink_metadata`, never canonicalised and
then acted on.

## Troubleshooting

**A suite passes in a clean shell and fails inside a recipe run.** Check that
it sources the setup, and that the source runs before the first worktree is
created:

```bash
grep -n -B1 recipe-test-env.sh amplifier-bundle/recipes/tests/test-issue-840-worktree-leak-proof.sh
```

**`HARNESS-ERROR: missing .../recipe-test-env.sh`.** The fragment was not
staged. It lives in `amplifier-bundle/recipes/tests/` beside
`expected-checks-fixtures.sh` and is installed with the rest of the bundle
directory by `amplihack install`; reinstall, or confirm the file is present in
the checkout the suite resolved.

**A helper fails with `AMPLIHACK_WORK_ROOT: unbound variable` under `set -u`.**
It is reading one of the four names without a default. That failure is the
intended direction: it fails closed instead of acting on a stale root. Guard
the read, as `cmd_base` in `workflow_worktree_root.sh` does: its bare
`work="$AMPLIHACK_WORK_ROOT"` at line 189 is reached only through the
`${AMPLIHACK_WORK_ROOT:-}` test at line 181, which returns first when the
variable is unset. Either shape is fine as long as an unset variable cannot
reach a bare expansion.

**A worktree still appears in the shared work root.** Find which process made
it before changing anything. A branch name with a slash, such as
`feat/issue-2712-x`, puts the `.git` file one level deeper than a flat one, so
search by depth rather than by glob:

```bash
find "${AMPLIHACK_WORK_ROOT:?}/worktrees" -maxdepth 4 -name .git -exec cat {} +
# gitdir: /path/to/the/scratch/that/created/it
```

A `gitdir:` under a removed temporary directory names the suite or test that
created it. If the creator is a Rust test, route its `Command::new("bash")`
through `isolated()`; if it is a shell suite, add the source block.

**An empty `<name>-<hash8>` directory is left in the work root.** These are
leftovers from orphans already removed. They hold nothing and can be removed
with `rmdir`.

## Related

- [Run recipe::run Tests Inside a Recipe Session](run-recipe-tests-inside-a-recipe-session.md) - the same leak in the `amplihack-cli` unit tests, cleared with `clear_outer_run_env()`
- [Git Repository Environment Helper](../reference/git-repo-env-helper.md) - the shell lists of `GIT_*` selectors and run-identity names that QA steps clear
- [Keep Tests from Changing PATH and HOME](keep-tests-from-changing-path-and-home.md) - giving a child process its own environment
- [Keep Tests from Prompting for Git Credentials](keep-tests-from-prompting-for-git.md) - the prompt-suppressing settings `isolated()` applies
- [Worktree Support](../worktree-support.md) - how the workflow helpers choose a worktree base
