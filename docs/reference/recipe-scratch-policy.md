---
title: Recipe Scratch Policy
description: Where recipe agent steps and bulk test fixtures put temporary files, how each case is removed, and the tests that keep literal /tmp paths and growing fixtures off the shared /tmp.
last_updated: 2026-11-08
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Recipe Scratch Policy

> [Home](../index.md) > Reference > Recipe Scratch Policy

Recipe agent steps and the bulk shell test suites write scratch output under
`$TMPDIR`, or under a directory the run owns. They never write it under a
literal `/tmp` path, and they remove each case's files when that case
finishes. Two tests enforce this: a guard over the recipe YAML (issue #1978)
and an entry-count check in the hook-repair suite (issue #2043).

## Contents

- [Why the policy exists](#why-the-policy-exists)
- [The scratch order](#the-scratch-order)
- [The policy sentence](#the-policy-sentence)
- [Where the sentence appears](#where-the-sentence-appears)
- [Writing scratch paths in recipes](#writing-scratch-paths-in-recipes)
- [What an agent may remove](#what-an-agent-may-remove)
- [The workflow runtime root](#the-workflow-runtime-root)
- [The workstream file writer and cleaner](#the-workstream-file-writer-and-cleaner)
- [The recipe guard test](#the-recipe-guard-test)
- [Bulk test fixtures](#bulk-test-fixtures)
- [The hook-repair suite: per-case removal](#the-hook-repair-suite-per-case-removal)
- [Other shell suites](#other-shell-suites)
- [Environment variables](#environment-variables)
- [Running the tests](#running-the-tests)
- [Troubleshooting](#troubleshooting)
- [Related documentation](#related-documentation)

## Why the policy exists

`/tmp` is shared by every user and process on the host. On most hosts it is a
`tmpfs` with a fixed number of inodes. Two things used to exhaust it:

- **Agent steps (#1978).** A fixer or test step told to run
  `pytest --basetemp /tmp/x`, or to copy the tree for mutation testing, wrote
  tens of thousands of files under a literal `/tmp/...` path. That bypassed
  the `TMPDIR` the recipe runner sets for each step (see
  [Where a run writes](../howto/clean-up-after-recipe-runs.md#where-a-run-writes)).
- **Bulk test fixtures (#2043).** The hook-repair shell suite built a git
  repository for every case and kept all of them until the script exited,
  about 22,000 entries per run by its end. On 8 Nov seven concurrent runs
  held about 120,600 inodes on the host's `/tmp`. With per-case removal the
  suite holds one case at a time: its largest case, `section_e`, peaks near
  7,300 entries, and only 8 remain between cases.

The fix has two parts: scratch goes somewhere other than the shared `/tmp`,
and nothing is kept longer than the case that made it.

## The scratch order

Every recipe step and test that needs scratch space picks the first of these
that is set:

| Order | Location | Set by |
|---|---|---|
| 1 | `$TMPDIR` | the recipe runner, per step; or the caller of a test |
| 2 | `$AMPLIHACK_RUNTIME_ROOT` | `workflow-worktree.yaml` step-04, for the whole run |
| 3 | a directory inside the recipe's own worktree | removed with the worktree |

A literal `/tmp` is never one of the choices. Shell code that has no other
option writes `"${TMPDIR:-/tmp}"`, so `/tmp` is reached only when the caller
set nothing.

Each case, copy or scratch clone is removed when it finishes, not at the end
of the run.

## The policy sentence

Every prompt that carries the policy contains this text verbatim:

```text
Scratch policy (#1978): put test --basetemp and tmp_path roots, mutation-test and other scratch copies, and any scratch worktree or clone under "$TMPDIR", else "$AMPLIHACK_RUNTIME_ROOT", else this recipe's own worktree, never under a literal /tmp path; remove each one when its case finishes.
```

| Property | Value |
|---|---|
| Marker the guard test looks for | `Scratch policy (#1978)` |
| Template variables | None. No `{{...}}`. |
| Literal `/tmp/` | None. It says `/tmp path`, with no trailing slash. |

The runner does not expand `$TMPDIR` or `$AMPLIHACK_RUNTIME_ROOT` in prompt
text. The agent's own shell resolves them.

## Where the sentence appears

| Recipe (`amplifier-bundle/recipes/`) | Where |
|---|---|
| `default-workflow.yaml` | top-level `description:`, after its first line (the first line is pinned and unchanged) |
| `auto-drive-to-merge.yaml` | top-level `description:` |
| `autodrive-crusty-round.yaml` | `step-04-address-concerns` |
| `autodrive-merge-round.yaml` | `step-04-address-blockers` |
| `workflow-tdd.yaml` | `step-07-write-tests`, `step-08-implement`, `step-08b-integration`, `step-08c-work-verifier` |
| `workflow-precommit-test.yaml` | `step-12-run-precommit`, `step-13-local-testing` |
| `workflow-refactor-review.yaml` | `step-09-refactor`, `step-09b-optimize`, `step-11b-implement-feedback` |
| `workflow-pr-review.yaml` | `step-17e-address-blocking-issues`, `step-18b-implement-feedback` |
| `consensus-test-fix.yaml` | `step7-fix-issues`, `step8-local-testing` |
| `smart-execute-routing.yaml` | `handle-ops-agent` |

`autodrive-crusty-round.yaml`, `autodrive-merge-round.yaml`,
`smart-execute-routing.yaml` and `workflow-pr-review.yaml` are at or one
under the 400-line brick budget (two at 400 lines, two at 399), and none of
them gains a line:

- in `autodrive-crusty-round.yaml`, `autodrive-merge-round.yaml` and
  `workflow-pr-review.yaml` the sentence is joined onto the end of an
  existing prose line of the prompt;
- in `smart-execute-routing.yaml` it is a line of its own, and the
  `cleanup-helper` comment above the `smart-orch-ws` cleaner went from two
  lines to one.

In the other files the sentence is a line of its own: right after the
prompt's `# Step ...` heading, or at the end of a prompt that has no heading.

Steps without the sentence: the read-only reviewer steps
(`autodrive-crusty-round.yaml` `step-02-crusty-review`,
`autodrive-merge-round.yaml` `step-02-merge-ready-assessment`), the
question-and-answer steps of `qa-workflow.yaml`, and all bash steps, which
have no prompt.

Check one recipe by hand:

```bash
grep -c 'Scratch policy (#1978)' amplifier-bundle/recipes/workflow-tdd.yaml
# 4
```

## Writing scratch paths in recipes

| Context | Form | Example |
|---|---|---|
| Shell command | quoted `"${TMPDIR:-/tmp}/..."` | `pytest --basetemp "${TMPDIR:-/tmp}/pytest-step13"` |
| Scratch directory | `mktemp -d` under the same root | `wt=$(mktemp -d "${TMPDIR:-/tmp}/wt-issue.XXXXXX")` |
| Glob cleanup | quoted root, glob outside the quotes | `rm -f "${TMPDIR:-/tmp}"/smart-orch-ws-*.json` |
| Prose | `$TMPDIR/...` (default `/tmp`) | "Logs go to `$TMPDIR/amplihack-workstreams/` (default `/tmp`)." |

Rules:

- Never write a bare `$TMPDIR/x` in a shell line. With `TMPDIR` unset it
  becomes `/x`. Use `${TMPDIR:-/tmp}`.
- Quote every expanded path.
- Keep the `X`s at the end of a `mktemp` template.

## What an agent may remove

An agent removes only directories it created in the same step, by the exact
path `mktemp` returned:

```bash
case_dir=$(mktemp -d "${TMPDIR:-/tmp}/mutants.XXXXXX")
cp -r src "$case_dir/src"
# ... run the mutation case ...
rm -rf -- "${case_dir:?}"
```

It never removes `$TMPDIR`, `$AMPLIHACK_RUNTIME_ROOT` or the worktree root
itself, and never removes by glob or pattern outside a directory it created.

## The workflow runtime root

The runtime root holds a run's locks, reflection, logs, metrics and
provenance. `workflow-worktree.yaml` step-04 computes and exports it as
`AMPLIHACK_RUNTIME_ROOT`; the helper
`amplifier-bundle/tools/workflow_runtime_root.sh` creates the folders.

The root is the first of:

1. `$AMPLIHACK_RUNTIME_ROOT`, when already set;
2. `$XDG_RUNTIME_DIR/amplihack-runtime/<branch>`;
3. `${TMPDIR:-/tmp}/amplihack-runtime/<branch>`.

The inline form in step-04 is:

```bash
AMPLIHACK_RUNTIME_ROOT="${AMPLIHACK_RUNTIME_ROOT:-${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/amplihack-runtime/$BRANCH_NAME}"
```

On the third choice the helper creates `${TMPDIR:-/tmp}/amplihack-runtime`
with `mkdir -p -m 700`, and exits 1 if that directory or the branch root
below it is a symlink or is not owned by the caller. Its other contract is unchanged: one argument, the branch
name; the root printed on stdout; exit 1 when the folders cannot be created.

```bash
TMPDIR=/d0/ryan/amplihack-test-tmp/2043 env -u AMPLIHACK_RUNTIME_ROOT -u XDG_RUNTIME_DIR \
  amplifier-bundle/tools/workflow_runtime_root.sh feat/issue-2043
# /d0/ryan/amplihack-test-tmp/2043/amplihack-runtime/feat/issue-2043
```

The Rust runtime resolver still falls back to `/tmp/amplihack-runtime` when
neither `AMPLIHACK_RUNTIME_ROOT` nor `XDG_RUNTIME_DIR` is set. Inside a
recipe run this makes no difference, because step-04 exports
`AMPLIHACK_RUNTIME_ROOT` before anything reads it.

## The workstream file writer and cleaner

`amplihack orch` writes workstream configs named `smart-orch-ws-*.json` with
`std::env::temp_dir()`, which is `$TMPDIR`, or `/tmp` when it is unset
(`crates/amplihack-cli/src/commands/orch.rs`). The safety-net step
`cleanup-helper` in `smart-execute-routing.yaml` removes them from the same
place:

```bash
rm -f "${TMPDIR:-/tmp}"/smart-orch-ws-*.json 2>/dev/null || true
```

The guard test checks that the writer and the cleaner name the same
location.

## The recipe guard test

`crates/amplihack-cli/tests/issue_1978_recipe_prompts_use_tmpdir.rs` reads
`amplifier-bundle/recipes/*.yaml` and fails when:

- a step in its `POLICY_STEPS` list, or either top-level description, lacks
  the policy sentence;
- the first line of `default-workflow.yaml`'s description changed;
- a line tells an agent to write scratch under a literal `/tmp/` path and no
  allowlist entry covers it;
- an allowlist entry matches nothing;
- the `smart-orch-ws` writer and cleaner disagree on the location.

The test only reads files. Its detector is also exercised by unit tests on
text held in memory, such as `pytest --basetemp /tmp/x`, so no test edits a
file in the repository.

### Allowlist

Each entry is (file, substring, reason).

| File | Substring | Reason |
|---|---|---|
| `quality-audit-cycle.yaml` | `${OUTPUT_DIR:-/tmp}/cycle_` | not on the fixer, test or QA path; the caller sets the output directory |

### Adding or renaming a step

1. Add the sentence to the new step's prompt.
2. Add the step id to `POLICY_STEPS`.
3. Recompute the recipe's entry in `amplifier-bundle/recipes/_recipe_manifest.json`
   (the first 16 hex characters of the file's SHA-256):

   ```bash
   sha256sum amplifier-bundle/recipes/workflow-tdd.yaml | cut -c1-16
   ```

4. Run `scripts/check-brick-budget.sh`.

## Bulk test fixtures

A test that creates many files picks its fixture root with
`pick_fixture_root` in `tests/integration/issue_1671_hook_rejection_repair_test.rs`:

```rust
fn pick_fixture_root(
    tmpdir: Option<&Path>,
    is_tmpfs: &dyn Fn(&Path) -> bool,
    target_tmp: Option<&str>,
) -> PathBuf
```

| `TMPDIR` | `CARGO_TARGET_TMPDIR` | Root used |
|---|---|---|
| set, not on tmpfs | any | `TMPDIR` |
| set, on tmpfs | set | `CARGO_TARGET_TMPDIR` |
| unset | set | `CARGO_TARGET_TMPDIR` |
| unset or on tmpfs | unset | `/tmp` |

- `target_tmp` is `option_env!("CARGO_TARGET_TMPDIR")`, read at compile time.
  Cargo sets it for integration-test targets, so for this test it is a
  directory under `target/`, on the build disk.
- **tmpfs detection, Linux:** `libc::statfs` on the path, then `f_type`
  compared with `0x0102_1994` (`TMPFS_MAGIC`). A path containing a NUL byte,
  or a failed `statfs` call, counts as not tmpfs.
- **tmpfs detection, other platforms:** no path counts as tmpfs.
- After choosing, the harness asserts the root is not on tmpfs, unless the
  choice was the `/tmp` fallback.

The function is pure; four unit tests cover the four rows with `is_tmpfs`
injected, so they do not depend on the host's mounts.

## The hook-repair suite: per-case removal

`amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh`
runs 14 cases: `mutation_canary` and the 13 `section_*` functions
(`a` to `g`, `h_static`, `h_dynamic`, `i` to `l`). Case names are the full
function names, such as `section_j`. Each runs through `run_case <name> <fn>`:

1. sets `CASE_ROOT=$WORK/case-<name>`; the case builds its repositories there,
   including the `mktemp -d` roots in `section_k` and `section_l` and the
   canary's `mutation-canary` directory;
2. runs the function;
3. counts entries in `$WORK` (`find "$WORK" -xdev | wc -l`) as `peak`. This
   is one sample taken when the function returns, before removal, not the
   highest count during the case;
4. removes `$CASE_ROOT` and empties `$WORK/tmp`, unless
   `ISSUE_1671_KEEP_WORK` is set;
5. counts again as `after`.

`$WORK/home`, `$WORK/bin` and `$WORK/tmp` stay for
the whole run. The EXIT trap still removes `$WORK` as a backstop.

### The entry ceiling

`B` is the first case's `after` count (the canary's in a full run). Every
later `after` count must be at most `B + CASE_SLACK`, where `CASE_SLACK=64`
covers small writes to home config and stub files. When a case leaves files behind, the suite fails at that
case boundary and names the case. When `ISSUE_1671_KEEP_WORK` is set nothing
is removed, so the ceiling is not checked.

The counts are taken when each case function returns. There is no timer.

### The boundary log

When `ISSUE_1671_BOUNDARY_LOG` is set (an absolute path; anything else exits
2), the suite appends two lines per case:

```text
peak section_d 3249
after section_d 8
```

The Rust test `hook_rejection_repair_shell_contract_passes` puts this log in
a second temporary directory beside the fixture (prefix
`issue-2043-boundary-`) and checks that:

- there are exactly 14 `after` lines;
- every `peak` is at least its `after`;
- `boundary_violations(log, CASE_SLACK)` finds nothing. The parser is strict:
  a line that is not `<peak|after> <name> <count>` is an error, not skipped;
- neither the fixture nor the boundary-log directory is left in the fixture
  root after the run. Other runs may share that root and hold live fixtures
  of their own, so only this test's own entries are checked.

The #1937 checks are unchanged: nothing new beside the fixture, and its
`tmp`, `home` and `cwd` empty afterwards.

### Running part of the suite

`ISSUE_1671_ONLY` takes a comma-separated list of case names (characters
`a-z0-9_,` only). A list that matches no case exits 2.

```bash
mkdir -p /d0/ryan/amplihack-test-tmp/2043
TMPDIR=/d0/ryan/amplihack-test-tmp/2043 ISSUE_1671_KEEP_WORK=1 \
  ISSUE_1671_ONLY=mutation_canary,section_j \
  bash amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh
ls -d /d0/ryan/amplihack-test-tmp/2043/issue-1671-hook-repair-*/case-*
```

The test `keep_work_leaves_case_directories` runs the same selection under
its own temporary root, checks that the `case-*` directories remain, then
removes them.

## Other shell suites

| Suite | Per-case removal | Keep switch |
|---|---|---|
| `test-default-workflow-reliability.sh` | yes: `drop_case` after the last assertion of each repository-building case (runtime artifacts, terminal status, commit guard, worktree base, JSON escape, publish title, no-merge probe). The non-git `mktemp -d` fixture lives outside `$WORK` and keeps its own `rm -rf --` | `DEFAULT_WORKFLOW_RELIABILITY_KEEP_WORK=1` |
| `test-auto-drive-to-merge.sh` | yes: `drop_case` at three points where nothing later reads the state: the loop state of sections 1 to 2d, the other-repository pair and refused-review-dir loops of section 2e, and the #1597 checkout pair and its loops. The shared `FX`/`FX_ORIGIN` fixture and the launch checkout stay | `AUTODRIVE_TEST_KEEP_WORK=1` |
| `test-issue-1937-no-stray-git.sh` | no: one arena is what lets it detect a stray `.git`, and it holds 5 small repositories | none |
| suites with 3 or fewer repositories (`1543`, `1808`, `1763`, `1494-1560`, `1060`, and suites with 0 or 1) | no: peak use is small and the EXIT trap is enough | none |

`drop_case` refuses any path that is not under `$WORK/` or that contains
`..`. Every removal uses
`${VAR:?}` and `--`, and starts from a `mktemp`-created root.

## Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `TMPDIR` | recipes, every suite, `pick_fixture_root` | first choice for scratch |
| `AMPLIHACK_RUNTIME_ROOT` | recipe agents, `workflow_runtime_root.sh` | second choice for scratch; the run's runtime directory |
| `ISSUE_1671_KEEP_WORK` | 1671 suite | keep `$WORK` and every `case-*` directory; skip the ceiling |
| `ISSUE_1671_ONLY` | 1671 suite | run only the named cases |
| `ISSUE_1671_BOUNDARY_LOG` | 1671 suite | append `peak`/`after` counts per case |
| `DEFAULT_WORKFLOW_RELIABILITY_KEEP_WORK` | `test-default-workflow-reliability.sh` | keep each case's directories |
| `AUTODRIVE_TEST_KEEP_WORK` | `test-auto-drive-to-merge.sh` | keep each case's state directories |

## Running the tests

```bash
mkdir -p /d0/ryan/amplihack-test-tmp/2043
export TMPDIR=/d0/ryan/amplihack-test-tmp/2043 RUST_TEST_THREADS=8
cargo test -p amplihack-cli --test cli_integration issue_1978
cargo test -p amplihack --test integration issue_1671_hook_rejection_repair::
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
bash amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh
```

## Troubleshooting

| Message | Cause | Fix |
|---|---|---|
| `entry ceiling exceeded after section_<x>` | that case wrote outside `$CASE_ROOT`, or created more than `CASE_SLACK` entries in shared directories | build the case's repositories under `${CASE_ROOT}` |
| guard test: `missing Scratch policy (#1978)` in a step | a prompt was rewritten without the sentence | add the sentence back, then recompute the manifest hash |
| guard test: `allowlist entry matches nothing` | the allowlisted line was changed or removed | delete or update the entry |
| guard test: literal `/tmp/` scratch path | a prompt or command names `/tmp/...` | use `"${TMPDIR:-/tmp}/..."` |
| `issue_1740` test: stale manifest hash | a recipe changed after hashing | recompute its 16-character entry |
| harness assertion: fixture root is on tmpfs | `TMPDIR` is unset or on tmpfs, and `CARGO_TARGET_TMPDIR` (the build's `target/` directory) is itself on tmpfs | set `TMPDIR` to a directory on a disk, or move the build target directory off tmpfs |

## Related documentation

- [Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md): where a
  run writes and how run directories are removed
- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md): the feature the
  1671 suite tests
- [Recipe Executor Environment](./recipe-executor-environment.md): the
  `TMPDIR` each step receives
