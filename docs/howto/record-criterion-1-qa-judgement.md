---
title: Record a Criterion-1 QA Judgement by Hand
description: Reproduce auto-drive's criterion-1 QA judgement by hand.
last_updated: 2026-11-21
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Record a Criterion-1 QA Judgement by Hand

Use this guide to reproduce [auto-drive's QA judgement](../reference/auto-drive-qa.md)
by hand. Standalone [`/merge-ready`](../../amplifier-bundle/skills/merge-ready/SKILL.md)
exempts only documentation-only changes from its scenario requirement.

Start in the PR's linked worktree at the head you want to assess, with
`origin` pointing to the repository the PR targets. You need Git, an
authenticated GitHub CLI, `jq`, and `AMPLIHACK_HOME` pointing to a trusted
installed bundle outside that worktree.

## 1. Resolve the PR target and measure

Run these commands from the PR worktree. The selected suite runs code from
the branch. Use an isolated test environment without publishing credentials
or agent profiles. Resolve the target while authenticated, then run the suite
in that isolated environment.

```bash
pr="$(gh pr view --json baseRefName,headRefOid)" &&
  base="$(jq -er '.baseRefName' <<<"$pr")" &&
  head="$(jq -er '.headRefOid' <<<"$pr")" &&
  test "$(git rev-parse HEAD)" = "$head" &&
  git fetch origin "refs/heads/$base:refs/remotes/origin/$base"
```

For a PR targeting `release/1.2`, Git fetches `release/1.2` after verifying
that the checkout matches the PR head. Carry the observed `base` and `head`
into the test environment and continue after the commands succeed. Run in
the PR worktree with a private `TMPDIR`.

```bash
tool="$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_qa_evidence.sh"
test "$(git rev-parse HEAD)" = "$head" &&
  state="$(mktemp -d "${TMPDIR:?Set a private TMPDIR}/criterion-1.XXXXXX")" &&
  REPO_PATH="$PWD" AUTODRIVE_QA_BASE="$base" AUTODRIVE_QA_HEAD="$head" \
    AUTODRIVE_QA_EVIDENCE="$state/qa-evidence.json" bash "$tool" &&
  jq '{qa_base, head_sha, docs_only, qa_command_paths, qa_command, qa_exit_code}' \
    "$state/qa-evidence.json"
```

Expect the chosen target in `qa_base` and the tested commit in `head_sha`.
A declared command that passes has `qa_exit_code` `"0"`. If the command
fails, record the cause and repair or follow up on it. If the tool produces
no output, diagnose why and use the actual diff and other available
evidence. For a drive's saved record, check that it belongs to the assessed
head, round, and target before using a QA declaration. See
[how measurement is stored](../reference/auto-drive-qa.md#the-record-file).

## 2. Decide what the change requires

Read the linked issue and the complete diff against the PR target.

```bash
git diff --name-status "origin/$base...$head"
git diff "origin/$base...$head"
```

The first command lists changed paths. The second shows the corresponding
patch. Use `docs_only` as a clue and check what users, operators, and calling
programs observe. For a `not_user_facing` judgement, cite the changed files
and explain why the complete change leaves behavior unchanged. A YAML
comment-only edit to `amplifier-bundle/recipes/auto-drive-light.yaml`, for
example, can measure as `docs_only=false` and still qualify because the
parsed recipe is identical. If `docs_only` is unknown or absent, also name
the source of the change set in your notes, such as the diff against the
observed PR target.

## 3. Exercise a user-visible change

Find the repository's outside-in coverage for the user's entry point. Add
or extend a scenario where that coverage misses the change. For Gadugi,
run only scenarios the PR adds or changes. Verify that each selected
scenario is a regular file inside the approved directory, stage each file
alone outside the repository, and validate before running it by its YAML
`name:`. Give test runs a private `TMPDIR` and give any `tmux` call a
private socket. The full Gadugi suite belongs in CI. In amplihack-rs,
`tests/gadugi/` runs in CI. See
[scenario judgement](../reference/auto-drive-qa.md#scenario-judgement).

If the operator-selected scenario directory is missing, use a suitable
tracked repository directory and record the fallback. Diagnose failures as
product, scenario, or environment failures. Fix the product or test as
needed and rerun the affected coverage. Record which scenario ran, the
tested commit, the result, and any repair. For an applicable repository
declaration, include the declared command's outcome and the named CI
check's outcome at the assessed head when the declaration names one.

## 4. Record a judgement or hand off

Write the judgement in the readiness report. Describe the user-visible
effect and passing coverage, or cite the changed files and explain why the
change leaves behavior unchanged. Include any unresolved blocker. Omit
raw test output, credentials, environment values, and private paths from
the report.

If only an operator can resolve a blocker, record the step, failure,
`choice: handoff`, and the reason you need help. Name a person or team
as `owner` and state the action they need to take in `needs`. Use the
[step failure recovery contract](../reference/step-failure-recovery.md).

To check that a blocker in the measurement is recorded without ending the
round, see [run the QA blocker scenario](run-the-qa-blocker-scenario.md).
