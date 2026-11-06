---
title: Recipe Scratch Policy
description: Where recipe agent steps put test temporary folders, mutation-test copies, scratch worktrees and clones, and the guard test that keeps literal /tmp paths out of recipe prompts and skills.
last_updated: 2026-11-06
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Recipe Scratch Policy

> [Home](../index.md) > Reference > Recipe Scratch Policy

Agent steps in the bundled recipes write test and scratch output under `$TMPDIR`, or under the recipe's own worktree. They never write it under a literal `/tmp` path. Recipe prompts state this in one fixed sentence, skill examples use `"${TMPDIR:-/tmp}/..."` in place of `/tmp/...`, and an integration test fails if either drifts.

The policy was added for issue #1978.

## Contents

- [Why the policy exists](#why-the-policy-exists)
- [The policy sentence](#the-policy-sentence)
- [Where the sentence appears](#where-the-sentence-appears)
- [Steps that do not carry the sentence](#steps-that-do-not-carry-the-sentence)
- [Writing scratch paths in skills and recipes](#writing-scratch-paths-in-skills-and-recipes)
- [The workflow runtime root](#the-workflow-runtime-root)
- [The guard test](#the-guard-test)
- [Detection rule](#detection-rule)
- [Allowlist](#allowlist)
- [Adding or renaming a step](#adding-or-renaming-a-step)
- [Troubleshooting](#troubleshooting)
- [Related documentation](#related-documentation)

## Why the policy exists

`/tmp` is shared by every user and process on the host, and it has a fixed number of inodes. Test runners that create a folder per test, such as `pytest --basetemp`, and mutation-test tools that copy the whole source tree can create well over 100,000 files in one run. When an agent step pointed that output at a literal `/tmp/...` path, it filled the inodes on `/tmp` for the whole host.

The recipe runner already gives every step its own `TMPDIR`, `TMP` and `TEMP` (see [Where a run writes](../howto/clean-up-after-recipe-runs.md#where-a-run-writes)). A literal `/tmp/...` path in a command bypasses that setting. The policy closes the gap by telling agents to use `$TMPDIR`, and by removing literal `/tmp/` write targets from the prompts and skills that agents read.

## The policy sentence

Every prompt that carries the policy uses this text verbatim:

```text
Scratch policy (#1978): put test --basetemp and tmp_path roots, mutation-test and other scratch copies, and any scratch worktree or clone under "$TMPDIR" (or this recipe's own worktree), never under a literal /tmp path.
```

Properties of the sentence:

| Property | Value |
|---|---|
| Marker the test looks for | `Scratch policy (#1978)` |
| Terms the test also requires | `$TMPDIR`, `--basetemp`, `tmp_path`, `mutation`, `worktree`, `clone` |
| Template variables | None. It contains no `{{...}}` and is not placed next to one. |
| Literal `/tmp/` | None. It says `/tmp path`, without a trailing slash, so it does not trip the scanner. |
| Host-specific paths | None. |

The runner does not substitute `$TMPDIR` in prompt text; it passes the variable to the agent unchanged. The agent's own shell resolves it to the folder the runner set for that step.

## Where the sentence appears

The sentence is a single line near the end of each listed step's `prompt`, before any final output-format instruction. It is a separate paragraph where the recipe's line budget allows. `autodrive-crusty-round.yaml`, `autodrive-merge-round.yaml`, `smart-execute-routing.yaml` and `workflow-pr-review.yaml` are at the 400-line brick budget, so there it is a line of its own with no blank line around it.

| Recipe (`amplifier-bundle/recipes/`) | Step ids |
|---|---|
| `autodrive-crusty-round.yaml` | `step-04-address-concerns` |
| `autodrive-merge-round.yaml` | `step-04-address-blockers` |
| `workflow-tdd.yaml` | `step-07-write-tests`, `step-08-implement`, `step-08b-integration`, `step-08c-work-verifier` |
| `workflow-precommit-test.yaml` | `step-12-run-precommit`, `step-13-local-testing` |
| `workflow-refactor-review.yaml` | `step-09-refactor`, `step-09b-optimize`, `step-11b-implement-feedback` |
| `workflow-pr-review.yaml` | `step-17e-address-blocking-issues`, `step-18b-implement-feedback` |
| `consensus-test-fix.yaml` | `step7-fix-issues`, `step8-local-testing` |
| `smart-execute-routing.yaml` | `handle-ops-agent` |

The two top-level recipes, `auto-drive-to-merge.yaml` and `default-workflow.yaml`, contain no agent prompts of their own; they run the recipes above. Each also carries the sentence as the last paragraph of its `description:` so that a reader of the top-level file sees the policy. `default-workflow.yaml` has a block-scalar description for this reason; its first line is unchanged.

To check a recipe by hand:

```bash
grep -n 'Scratch policy (#1978)' amplifier-bundle/recipes/workflow-tdd.yaml
```

The command prints four lines, one for each listed step in that recipe.

## Steps that do not carry the sentence

| Recipe or step | Reason |
|---|---|
| `qa-workflow.yaml` (all three architect steps) | Question-and-answer steps; they run no tests and make no scratch copies. |
| `autodrive-crusty-round.yaml` `step-02-crusty-review` | Read-only reviewer. |
| `autodrive-merge-round.yaml` `step-02-merge-ready-assessment` | Read-only reviewer. |
| Bash steps in `autodrive-merge-evidence.yaml` | Bash steps have no prompt. |

The guard test records these exclusions in its module comment.

## Writing scratch paths in skills and recipes

Use these forms in any recipe or skill file under `amplifier-bundle/` that an agent reads.

| Context | Form | Example |
|---|---|---|
| Shell command | Quoted `"${TMPDIR:-/tmp}/..."` | `pytest --basetemp "${TMPDIR:-/tmp}/pytest-smart-test"` |
| Single output file | `mktemp` under the same root | `out=$(mktemp "${TMPDIR:-/tmp}/qa-shadow.XXXXXX")` |
| Scratch worktree | `mktemp -d` under the same root | `wt=$(mktemp -d "${TMPDIR:-/tmp}/wt-issue.XXXXXX")` then `git worktree add "$wt/tree" -b fix/issue` |
| Cleanup with a glob | Quoted root, glob outside the quotes | `rm -rf "${TMPDIR:-/tmp}/amplihack-workstreams/"ws-*` |
| Prose and tables | `$TMPDIR/...` (default `/tmp`) | "Logs go to `$TMPDIR/amplihack-workstreams/` (default `/tmp`)." |

Rules:

- Never write a bare `$TMPDIR/...` in a shell line. If `TMPDIR` is unset, a bare `$TMPDIR/x` becomes `/x`, a write at the filesystem root. The `:-/tmp` default prevents that.
- Quote every expanded path. The one exception is the single-quoted YAML scenario in `qa-team/README.md`, which uses an unquoted `${TMPDIR:-/tmp}/test-student` inside the single quotes.
- When an example writes to a path, change every later read of the same path in that example (`diff`, `cat`, `mkdir`, `rm -rf`) in the same edit, so the example stays consistent.
- Prefer `mktemp` when an example creates one file and nothing later needs its fixed name. It avoids predictable names under the `/tmp` fallback.

The multitask skill documents match the runtime. `utils.rs` places workstream folders under `std::env::temp_dir()`, which honours `TMPDIR`, so the skill now gives the location as `$TMPDIR/amplihack-workstreams/` (default `/tmp`), and its cleanup lines remove that folder.

### Files rewritten under this policy

| File (`amplifier-bundle/`) | Change |
|---|---|
| `skills/dev-orchestrator/reference.md` | `mktemp` lines use `"${TMPDIR:-/tmp}/..."` |
| `skills/smart-test/SKILL.md` | Test temporary roots use `"${TMPDIR:-/tmp}/..."` |
| `skills/qa-team/SKILL.md`, `skills/qa-team/README.md` | `--shadow-log` targets use `"${TMPDIR:-/tmp}/..."` |
| `skills/multitask/SKILL.md`, `reference.md`, `examples.md`, `TIMEOUT_LIFECYCLE.md` | Workstream location and its `rm -rf` cleanup lines use `$TMPDIR` |
| `skills/transcript-viewer/SKILL.md` | Output files use `"${TMPDIR:-/tmp}/..."` |
| `skills/gh-aw-adoption/GENERIC_ADOPTION_PROMPT.md`, `examples.md` | Scratch clones and output use `"${TMPDIR:-/tmp}/..."` |
| `skills/claude-agent-sdk/drift-detection.md` | Scratch output uses `"${TMPDIR:-/tmp}/..."` |
| `recipes/workflow-worktree.yaml` | Runtime-root fallback (next section) |

## The workflow runtime root

`workflow-worktree.yaml` sets the runtime root, which holds locks, logs and provenance for a workflow run:

```bash
AMPLIHACK_RUNTIME_ROOT="${AMPLIHACK_RUNTIME_ROOT:-${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/amplihack-runtime/$BRANCH_NAME}"
```

The order of preference is:

1. `AMPLIHACK_RUNTIME_ROOT`, if set.
2. `$XDG_RUNTIME_DIR/amplihack-runtime/<branch>`.
3. `$TMPDIR/amplihack-runtime/<branch>`.
4. `/tmp/amplihack-runtime/<branch>`.

The step creates the root with mode `0700` before it creates any subfolder, because the root may now fall back to a shared folder. The `tools/workflow_runtime_root.sh` helper is unchanged. For the wider runtime layout, see [Configure Workflow Runtime Isolation](../howto/configure-workflow-runtime-isolation.md).

## The guard test

The test file is `crates/amplihack-cli/tests/issue_1978_recipe_prompts_use_tmpdir.rs`. Run it with:

```bash
cargo test -p amplihack-cli --test cli_integration issue_1978_recipe_prompts_use_tmpdir::
```

The crate sets `autotests = false`, so the file is a module of the single `cli_integration` test binary (`tests/main.rs`). The main tests are:

| Test | Checks |
|---|---|
| `top_level_recipes_state_policy` | The sentence is in the `description` of `auto-drive-to-merge.yaml` and `default-workflow.yaml`. |
| `fixer_test_qa_steps_carry_policy` | Every step in [the step table](#where-the-sentence-appears) exists, has a `prompt`, and the prompt contains the marker, all required terms and the sentence verbatim. It collects every failure before it panics. |
| `no_prompt_or_skill_directs_scratch_to_literal_tmp` | No line in the scan scope matches the [detection rule](#detection-rule) unless an [allowlist](#allowlist) entry covers it. |
| `workflow_runtime_root_falls_back_through_tmpdir` | The runtime-root fallback in `workflow-worktree.yaml` goes through `${TMPDIR:-/tmp}`, and the root is created with mode `0700` before its subfolders. The old form contains no `/tmp/`, so the scanner alone cannot catch it. |
| `multitask_cleanup_targets_tmpdir_workstreams` | No multitask `rm -rf` line removes a literal `/tmp/amplihack-workstreams` path. `rm` is not scratch vocabulary, so the scanner does not see these lines. |
| `policy_sentence_does_not_trip_scanner` | The policy sentence itself passes the detection rule. |

The remaining tests check the detector on fixed example lines, the scan scope (symlinks and excluded folders), that the first line of the `default-workflow` description is unchanged, and, in memory, that the three bad edits described in the acceptance check are caught.

The test is read-only. It reads files inside the repository only, writes nothing, sets no environment variables, creates no temporary folder and does not touch `~/.claude`. It fails closed: a missing step, prompt or description, an unreadable file, an empty scan set, a stale allowlist entry or an empty reason each fail the test. Non-UTF-8 files are read lossily and reported with their path.

Each failure message names the file, the line number and the matched text. For example, inserting `pytest --basetemp /tmp/x` at the end of the `step-04-address-concerns` prompt produces:

```text
amplifier-bundle/recipes/autodrive-crusty-round.yaml:313: directs scratch output to a literal /tmp path: pytest --basetemp /tmp/x
```

## Detection rule

### Scan scope

| Included | Excluded |
|---|---|
| `amplifier-bundle/recipes/*.yaml` | Any `docker-compose/` or `templates/` folder under `skills/` (container and GitHub Actions runner contexts) |
| `amplifier-bundle/skills/**/*.md` | Symlinks. The walk uses `symlink_metadata` and skips them, so a skill linked from another skill is scanned once. |
| | `amplifier-bundle/agents/`, `amplifier-bundle/tools/*.sh`, Rust sources, `context/DISCOVERIES_ARCHIVE.md` |

### Rule

A line is flagged when all of these hold:

1. It is not a whole-line YAML comment (a line whose first non-blank character is `#` in a `.yaml` file). Markdown lines are never treated as comments, including `#` lines inside fenced shell blocks.
2. It contains `/tmp/`. The prescribed form `"${TMPDIR:-/tmp}/..."` contains no `/tmp/`, so it passes this condition without any special case.
3. It contains one of the scratch words on the same line. The list matches `SCRATCH_VOCAB` in the test exactly: `basetemp`, `tmp_path`, `pytest`, `mutat`, `scratch`, `cp -r`, `worktree`, `clone`, `mktemp`, `--shadow-log`, `mkdir`, `> /tmp/`. The last entry is a redirection into `/tmp/`; it also matches `>> /tmp/`. `clone` matches `git clone`.
4. No allowlist entry matches it.

The word list limits the rule to lines that tell an agent where to write. Prose that mentions `/tmp/` for another reason, such as a path the runtime reads, is not flagged. `rm` is not in the list.

The following lines are left unchanged on purpose. They pass because they contain no scratch word:

| File | Line | Why it is left |
|---|---|---|
| `skills/transcript-viewer/SKILL.md` | 310: `ls -t /tmp/*.agent*.log` | Reads log files the runtime writes |
| `skills/dev-orchestrator/SKILL.md` | 144: `/tmp/amplihack-session-trees/{tree_id}.json` | Documents a path the Rust runtime hardcodes |
| `recipes/smart-execute-routing.yaml` | 397: `rm -f /tmp/smart-orch-ws-*.json` | Cleans up files the runtime itself writes |

Examples:

| Line | Result |
|---|---|
| `pytest --basetemp /tmp/pr272/bt1` | Flagged (`basetemp`) |
| `git worktree add /tmp/wt649impl -b fix` | Flagged (`worktree`) |
| `cargo mutants --in-place > /tmp/mutants.log` | Flagged (redirection) |
| `pytest --basetemp "${TMPDIR:-/tmp}/bt1"` | Not flagged (contains no `/tmp/`) |
| `# old: pytest --basetemp /tmp/x` in a `.yaml` file | Not flagged (YAML comment) |
| The runtime keeps its socket in /tmp/amplihack.sock. | Not flagged (no scratch vocabulary) |

## Allowlist

The allowlist is a `const` table in the test. Each entry is `(relative path, line substring, reason)`.

| Path | Reason category |
|---|---|
| `skills/eval-recipes-runner/SKILL.md` (Dockerfile `RUN git clone ... /tmp/amplihack`) | Path inside a container |
| `skills/shadow-testing/SKILL.md` (`git clone ... /tmp/test` run inside the container) | Path inside a container |
| `skills/code-atlas/tests/test_security_controls.md` (`> /tmp/test.env`) | Security fixture quoting `/tmp` as an attack example |
| `skills/code-atlas/tests/test_security_controls.md` (`>> /tmp/test.env`) | Security fixture quoting `/tmp` as an attack example |

Constraints on entries, all enforced by the test:

- The reason is not empty.
- The substring is at least 12 characters long and is neither `/tmp/` nor `> /tmp`, so one entry cannot hide every hit in a file.
- Every entry matches at least one flagged line. An entry that matches nothing is stale and fails the test.

Only these categories qualify: paths inside a container; cleanup of a file the runtime itself writes; test fixtures or security documents that quote `/tmp` as an attack example; and documentation of a path the Rust runtime hardcodes, recorded as a follow-up issue. Anything that tells an agent where to put its own output is rewritten, not allowlisted.

## Adding or renaming a step

- **New fixer, test or QA step.** Add the policy sentence to its prompt and add `(recipe, step id)` to `POLICY_STEPS` in the test.
- **Renamed step.** Update `POLICY_STEPS`. The test does not skip a missing step id; it fails with the recipe and the old id.
- **New skill example that writes scratch output.** Use the forms in [Writing scratch paths](#writing-scratch-paths-in-skills-and-recipes). The scanner flags a literal `/tmp/` write target.

Running auto-drive and default-workflow lanes read the installed copy of the bundle. They use the updated prompts only after `amplihack install` has been run again.

## Troubleshooting

**`fixer_test_qa_steps_carry_policy` reports a missing step.** A step in `POLICY_STEPS` was renamed or removed. Find the new id with `grep -n '  - id:' amplifier-bundle/recipes/<recipe>.yaml` and update the table.

**`fixer_test_qa_steps_carry_policy` reports a missing term.** The sentence was edited. Restore it from [The policy sentence](#the-policy-sentence); it must be verbatim.

**The scanner flags a line you believe is safe.** Rewrite it with `"${TMPDIR:-/tmp}/..."` if it tells an agent where to write. Add an allowlist entry only if it fits one of the listed categories, and give a reason.

**The test reports a stale allowlist entry.** The line it covered was removed or rewritten. Delete the entry.

**A recipe no longer parses after adding the sentence.** The paragraph is outside the prompt's block scalar or at the wrong indentation. Indent it to match the surrounding prompt lines.

## Related documentation

- [Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md) - Where each run's `TMPDIR` is and how it is cleaned up
- [Configure Workflow Runtime Isolation](../howto/configure-workflow-runtime-isolation.md) - Where workflow runtime output is written
- [Auto Drive To Merge Reference](auto-drive-to-merge.md) - The crusty and merge rounds whose fixer steps carry the policy
- [Recipe Executor Environment](recipe-executor-environment.md) - The environment each recipe step receives
