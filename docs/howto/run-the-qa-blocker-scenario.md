---
title: Run the QA Blocker Scenario
description: Check, run and read the outside-in scenario that shows a QA measurement blocker is recorded and does not end an auto-drive round.
last_updated: 2026-11-20
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Run the QA Blocker Scenario

Use this guide to check, run and read
`tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh`. For what each
case asserts and why, see the
[scenario reference](../reference/qa-blocker-not-stop-scenario.md).

You need Git, `bash`, the standard text tools, and an `amplihack` binary. The
scenario also needs the pinned recipe runner, found through
`RECIPE_RUNNER_RS_PATH` or as `recipe-runner-rs` on `PATH`. The script resolves
it once, before any command runs, and passes the absolute path on, so moving
`HOME` into the scratch tree does not change which runner is used. It must not
find `gadugi-test` on its private search path: the script checks this and stops
at set-up if it does.

## 1. Check the script without running it

Where `/tmp` is shared or limited, do not run the script. Run the static
checks below and read the CI result. Running the whole `gadugi-test` suite on a
shared host emptied `/tmp` on 12 November, and the script creates its scratch
root under `TMPDIR`. Check the script from the repository root:

```bash
bash -n tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
shellcheck -S style tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
```

Both commands print nothing when the script is sound. Then confirm the script
holds none of the constructs the project excludes from scenarios:

```bash
grep -n -E 'set -x|sleep|timeout|curl|wget|rm -rf' \
  tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
```

Expect matches in comments that state the scenario uses none of these, and
one `rm -rf` inside the `EXIT` trap that removes the scratch root. A match in
any other executable line is a defect to repair.

## 2. Read the CI result

Open the `Install Smoke Test` job of the pull request's `CI` run and find the
step `Outside-in: QA blockers are named, not stops (issue #2569)`. Its log
ends with one of:

```text
ALL_CASES_PASSED
```

```text
SCENARIO_FAILED
```

Record the run identifier and the step result when you report the outcome. A
failure in a different step or job is a separate matter and says nothing
about this scenario.

## 3. Run it by hand on a developer machine

Use a private `TMPDIR` that you own, so the scratch root stays out of the
shared `/tmp`:

```bash
export TMPDIR="$HOME/tmp/qa-2569"
mkdir -p -m 700 "$TMPDIR"
AMPLIHACK_2569_QA_BIN="$(command -v amplihack)" \
  bash tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
```

On success the last line is `ALL_CASES_PASSED` and the exit status is `0`.
With `AMPLIHACK_2569_QA_BIN` set, the script writes nothing outside its scratch
root. Neither `~/.claude` nor `~/.amplihack` of the user running it is read or
written, and the `EXIT` trap removes the scratch root. With the variable unset,
the script builds the binary with `cargo` first, which takes longer and also
writes `target/` in the checkout.

To run the same cases through the harness, use the descriptor:

```bash
gadugi-test run tests/gadugi/scenarios/issue-2569-qa-blocker-not-stop.yaml
```

The descriptor runs the script, expects exit status `0`, and checks that
`ALL_CASES_PASSED` and each of the script's `PASS` lines are on standard
output. It sets no timeout.

## 4. Read a failure

A failing run prints one line for each failed assertion and ends with
`SCENARIO_FAILED`. Each line names the case, the expected value and the
observed value of one field:

```text
FAIL: case-c gadugi_scenario_dir_state: expected operator-missing, observed operator
```

Match the case to its section of the reference, then use the table below.

| Failure names | Likely cause | Where to look |
| --- | --- | --- |
| `set-up paths`, `set-up trees` or `set-up gadugi-test-absent` | The scratch root, checkout, tools directory or skill directory holds a character the recipe's path check rejects (the line says `unusable:` and names which); the scratch root overlaps the checkout; or `gadugi-test` was found on the private search path. | Use a `TMPDIR` and checkout path made of plain characters, keep `TMPDIR` outside the checkout, and check that no `gadugi-test` link was added to the stand-in directory. See [set-up checks](../reference/qa-blocker-not-stop-scenario.md#set-up-checks). |
| Any run's `exit-status` | `recipe run autodrive-merge-evidence` ended with a failure, so the round did not carry on. | Run the same recipe by hand with the same variables and read the failing step. |
| Any run's `agent-calls` | The `claude` stand-in was not called once for each of the recipe's three agent steps, so the round ended early, or the recipe's agent steps changed. | The `type: "agent"` steps in `amplifier-bundle/recipes/autodrive-merge-evidence.yaml`. |
| Any run's `mode` | The evidence folder is no longer `0700` or the record file is no longer `0600`, or no record was written. | [The record file](../reference/auto-drive-qa.md#the-record-file). |
| Any run's `record` or `case-c record-shape` | The record is missing, is not one JSON object, or (case C) has a line break or a control byte. | The measurement step `step-02a-qa-inputs` and the tool `amplifier-bundle/tools/autodrive_qa_evidence.sh`. |
| `case-a` `qa_status` or `qa_reason` | The record read `PASS`, was empty, or carried a reason outside the closed set. | [The status values](../reference/auto-drive-qa.md#the-status-values). |
| `case-a` `gadugi_status` | The absent `gadugi-test` was not measured as `NOT_INSTALLED`. | The `gadugi-test` section of `amplifier-bundle/tools/autodrive_qa_evidence.sh`. |
| `case-b` `leg-1`, `leg-3` or `leg-4`: `qa_base_source`, `qa_base` or `gh-calls` | The base resolution changed: a value other than the recipe's, or a guessed one, was used, or the lookup was called where it should not be. | [Resolving the measurement base](../reference/auto-drive-qa.md#resolving-the-measurement-base). |
| `case-b` `leg-1` or `leg-2`: `qa_command_paths` | The change set was not measured against the base the record names. | `autodrive_qa_scope.sh` in `amplifier-bundle/tools`. |
| `case-b` `leg-2`: `qa_base_source` or `gh-calls` | The lookup no longer calls `gh pr view <n> --repo github.com/<owner>/<name> --json baseRefName --jq .baseRefName` in that form, so the stand-in did not answer it and the record read `none`. | `qa_pr_base` in `amplifier-bundle/tools/autodrive_round_evidence.sh`. |
| `case-b` `leg-4`: `record-hostile-text` or `injected-file` | The branch-name check no longer rejects the value, the record carries it, or a substitution reached a shell. | The `base_ok` function in `step-02a-qa-inputs`, in `amplifier-bundle/recipes/autodrive-merge-evidence.yaml`. |
| `case-c` `gadugi_required` | The fixture no longer requires `gadugi-test`, so the state would not be consulted. | [The record's `gadugi_required` field](../reference/auto-drive-qa.md#the-measured-record). |
| `case-c` `gadugi_scenario_dir_state` | The state for an operator-named missing directory changed. | The scenario directory states in `amplifier-bundle/tools/autodrive_qa_evidence.sh`. |

Do not repair a failure by weakening an assertion. If the product's behaviour
changed on purpose, change the assertion to one of equal power and say why in
the pull request.

## 5. Add a case

Add a case when a new named blocker or base source joins the record. Keep to
the shape of the existing cases:

1. Create a fresh scratch repository for the case, with the Git variables from
   `scenario_env()` applied.
2. Run the recipe only through `scenario_env()`. Add a variable for that
   command alone when the case needs one.
3. Pass `repo_path` explicitly, and an `autodrive_qa_evidence` path inside a
   `0700` folder.
4. Assert on the record's fields, the exit status and the stand-ins' call
   logs, counting calls and never reading prompt text. Do not assert on the
   runner's own output, which may echo the context values it was given, or on
   a bash step's standard error, which the runner does not forward.
5. Name the case, the expected value and the observed value in each failure
   line, and print neither the environment nor the call logs.
6. Write each label as a literal at its call site, add a matching `PASS`
   entry to `tests/gadugi/scenarios/issue-2569-qa-blocker-not-stop.yaml`, and
   check the script again with the commands in
   [step 1](#1-check-the-script-without-running-it).

Do not add a sleep, a timeout, a retry or a polling loop. The stand-ins return
at once, so none is needed.

## Related

- [QA blocker scenario reference](../reference/qa-blocker-not-stop-scenario.md)
- [Auto-drive QA (Criterion 1)](../reference/auto-drive-qa.md)
- [Record a criterion-1 QA judgement by hand](record-criterion-1-qa-judgement.md)
- [CI pipeline](../reference/ci-pipeline.md)
