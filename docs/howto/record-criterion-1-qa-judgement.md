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
evidence. Use a QA declaration from a drive's saved record only when that
record describes this PR's change at the head you are assessing. See
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

If the operator-selected scenario directory is missing, use the
repository's own scenario directory, inside the repository, and record the
fallback. Diagnose failures as
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

## 5. Check that a blocker does not end the round

`tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh` drives the
shipped binary through `autodrive-merge-evidence` and checks that a QA
measurement blocker is recorded and the round carries on. CI runs it in the
`Install Smoke Test` job, in the step `Outside-in: QA blockers are named, not
stops (issue #2569)`; the log ends with `ALL_CASES_PASSED` or
`SCENARIO_FAILED`. Read that result rather than running the script where
`/tmp` is shared or limited.

To run it by hand, give it a private `TMPDIR` you own. The script needs the
pinned recipe runner, through `RECIPE_RUNNER_RS_PATH` or as
`recipe-runner-rs` on `PATH`, and it stops at set-up if `gadugi-test` is on
its private search path.

```bash
export TMPDIR="$HOME/tmp/qa-2569"
mkdir -p -m 700 "$TMPDIR"
AMPLIHACK_2569_QA_BIN="$(command -v amplihack)" \
  bash tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
```

With `AMPLIHACK_2569_QA_BIN` set, the script writes nothing outside its
scratch root, reads neither `~/.claude` nor `~/.amplihack`, and removes the
scratch root on exit. The same cases run through the harness with
`gadugi-test run tests/gadugi/scenarios/issue-2569-qa-blocker-not-stop.yaml`.

A failing run prints one line per failed assertion, naming the case, the
expected value and the observed one, and ends with `SCENARIO_FAILED`. Each
case's comment in the script says what it measures and which file owns the
behaviour. Do not repair a failure by weakening an assertion: where the
behaviour changed on purpose, change the assertion to one of equal power and
say why in the pull request.
