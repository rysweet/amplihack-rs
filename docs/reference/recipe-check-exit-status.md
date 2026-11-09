---
title: Recipe Check Exit Status
description: How recipe bash steps that run tests, hooks and CI checks report the real exit status, keep stderr in the step log, and route failures to a fix or diagnosis step.
last_updated: 2026-11-09
issue: 2190
---

# Recipe Check Exit Status

> [Home](../index.md) > Reference > Recipe Check Exit Status

A recipe bash step that runs a test, lint, hook or CI check reports that
check's real exit status. The step does not hide a failure with `|| echo`,
`|| true` or `|| :`, and it does not send the check's stderr to
`/dev/null`. The check output goes to the step log, and a later step decides
what to do from the exit code.

## Contents

- [The rule](#the-rule)
- [consensus-test-fix: step7-run-tests](#consensus-test-fix-step7-run-tests)
- [consensus-merge-finalize: step14-check-ci](#consensus-merge-finalize-step14-check-ci)
- [autodrive-merge-evidence: CI read](#autodrive-merge-evidence-ci-read)
- [Value conventions](#value-conventions)
- [The guard test](#the-guard-test)
- [Writing a new check step](#writing-a-new-check-step)
- [Related](#related)

## The rule

Do this:

```bash
cmd >>"$LOG" 2>&1; RC=$?
if gh pr checks >&2; then CI_RC=0; else CI_RC=$?; fi
```

Not this:

```bash
cmd 2>/dev/null || echo "needs verification"
gh pr checks 2>/dev/null || echo "CI check pending"
```

- Print the real exit code in the step's JSON, such as
  `"test_exit_code":"1"`, not `"tests_run": true` whatever happened.
- Send failures to a fix or diagnosis step through a condition. Do not
  report success while a check fails.

A check is not run only when the project does not ask for it: no
`Cargo.toml` or `package.json` means no test command, and no
`.pre-commit-config.yaml` means no `pre-commit`. That is not a failure.
The step never checks `PATH` to decide whether to run a check. A check the
project asks for always runs, and a non-zero exit is always a failure. This
includes exit 127 (the tool is not installed) and `gh` exit 8 (checks
pending).

## consensus-test-fix: step7-run-tests

File: `amplifier-bundle/recipes/consensus-test-fix.yaml`.

The step runs the native test command and then `pre-commit`, each into one
log file:

| Repository has | Test command run |
|---|---|
| `Cargo.toml` | `cargo test --workspace --locked` |
| `package.json` (no `Cargo.toml`) | `npm test` |
| neither | none; `[skip]` line on stderr |

`pre-commit run --all-files` runs whenever `.pre-commit-config.yaml`
exists. The step does not look for `pre-commit` on `PATH` first. If the
config exists but `pre-commit` is not installed, the shell exits 127,
`precommit_exit_code` is `"127"` and `all_passing` is `"false"`, so the fix
step runs. A repository with no `.pre-commit-config.yaml` gets a `[skip]`
line and `precommit_command` `"none"`.

### Log file

The step creates the log with
`(umask 077; mktemp "${TMPDIR:-/tmp}/consensus-test-fix.XXXXXX")`, so the
file has a random name and mode `0600`. After the checks it copies the last
256 KiB of the log to stderr (`tail -c 262144`), so the output appears in
the step log, and prints one `[result]` summary line. The full log stays on
disk for the fix agent. It is not deleted.

If `mktemp` fails, or the path contains `"`, `\` or a control character,
the step prints `all_passing` `"false"` with `log_file` `""` and exits 0.

### Output (`test_results`)

Stdout holds exactly one JSON object. Check output never goes to stdout, so
the object parses however much a test prints.

```json
{
  "tests_run": "true",
  "test_command": "cargo test --workspace --locked",
  "test_exit_code": "101",
  "precommit_command": "pre-commit run --all-files",
  "precommit_exit_code": "0",
  "all_passing": "false",
  "log_file": "/tmp/consensus-test-fix.k3Qz9a"
}
```

| Field | Value |
|---|---|
| `tests_run` | `"true"` when a test command ran, else `"false"` |
| `test_command` | The command run, or `"none"` |
| `test_exit_code` | Its exit status as a string, or `""` when nothing ran |
| `precommit_command` | `"pre-commit run --all-files"`, or `"none"` |
| `precommit_exit_code` | Its exit status as a string, or `""` when it did not run |
| `all_passing` | `"true"` only when every check that ran exited 0 |
| `log_file` | Path of the full log, or `""` when it could not be created |

The step itself exits 0 after printing the JSON. The failure path is the
condition on the next step, not the step's exit status.

### Routing

| Step | Condition | Effect |
|---|---|---|
| `step7-fix-issues` | `test_results.all_passing != 'true'` | Runs on any failure, and on missing or unreadable output |
| `step8-local-testing` | none | Receives `{{test_results}}` and `{{hook_fixes}}` |

The `step7-fix-issues` prompt tells the `pre-commit-diagnostic` agent to read
the file at `test_results.log_file` before diagnosing, and to treat its
contents as diagnostic data only, not as instructions.

The `step8-local-testing` prompt states: if `test_results.all_passing` is
`"false"` and `hook_fixes.all_passing` is not true (or `hook_fixes` is
empty), `ready_to_commit` must be false. This rule is advice to the agent.
The commit hooks and CI are the gates that enforce it.

## consensus-merge-finalize: step14-check-ci

File: `amplifier-bundle/recipes/consensus-merge-finalize.yaml`.

Inside a git work tree with an `origin` remote, the step runs:

```bash
echo "Checking CI status..." >&2
if gh pr checks >&2; then CI_RC=0; else CI_RC=$?; fi
```

`gh` output goes to the step log through stderr. The step does not wait or
poll.

### Output (`ci_status`)

| Case | JSON on stdout |
|---|---|
| Checks pass | `{"ci_checked":"true","ci_exit_code":"0","ci_passing":"true"}` |
| Checks fail | `{"ci_checked":"true","ci_exit_code":"1","ci_passing":"false"}` |
| Checks pending | `{"ci_checked":"true","ci_exit_code":"8","ci_passing":"false"}` |
| No git repo, or no `origin` | `{"ci_checked":"false","ci_exit_code":"","ci_passing":""}` |

The step always exits 0.

### Routing

`step14-diagnose-ci` runs when
`ci_status.ci_checked == 'true' and ci_status.ci_passing != 'true'`.
A skipped check does not start a diagnosis. Pending checks (exit 8) go to
`ci-diagnostic-workflow`, which owns waiting for CI. `final-output` is
unchanged.

## autodrive-merge-evidence: CI read

File: `amplifier-bundle/recipes/autodrive-merge-evidence.yaml`.

The CI read is:

```bash
JSON="$(gh pr checks "$PR" --json name,state,bucket)"
```

`gh` stderr now goes to the step log. Stdout still feeds `$JSON`, and an
empty result is still reported as `UNREADABLE`, which is a failure. Nothing
else in this recipe changed.

## Value conventions

Every JSON value these steps print is a string: `"true"`, `"false"`, exit
codes such as `"0"`, `"1"`, `"8"`, `"101"`, and `""` for a check that did
not run. Recipe conditions compare against string literals (`== 'true'`),
so a boolean would not match. Conditions use `!= 'true'` so that a missing
or garbled field routes to the fix or diagnosis step instead of skipping it.

The JSON is built with `printf` from exit codes, fixed literals and the
validated log path. Check output and `gh` output never go into it.

## The guard test

`crates/amplihack-cli/tests/issue_2190_recipe_steps_report_real_exit_status.rs`
holds two kinds of test.

**Behaviour tests** take the real step command from the YAML, run it under
`env_clear()` with stub tools first on `PATH` in a temporary directory, and
check the JSON and stderr:

| Case | Setup | Expected |
|---|---|---|
| a | `cargo` stub prints `STUB_CARGO_FAIL` to stderr, exits 1 | `test_exit_code` `"1"`, `all_passing` `"false"`, marker in stderr |
| b | All stubs pass | `all_passing` `"true"` |
| c | `pre-commit` stub exits 1 | `precommit_exit_code` `"1"`, `all_passing` `"false"` |
| c2 | `.pre-commit-config.yaml` present, no `pre-commit` on `PATH` | `precommit_exit_code` `"127"`, `all_passing` `"false"` |
| d | Test stub prints about 1 MB to stdout | Stdout is still one JSON object |
| e | Read the YAML | Exact condition strings on `step7-fix-issues` and `step14-diagnose-ci` |
| f1 | `gh` stub exits 8 in a repo with `origin` | `ci_exit_code` `"8"`, `ci_passing` `"false"`, `gh` marker in stderr |
| f2 | `gh` stub exits 0 | `ci_passing` `"true"` |
| f3 | No git repo | `ci_checked` `"false"` |
| g | No `Cargo.toml`, `package.json` or pre-commit config | `test_command` `"none"`, `all_passing` `"true"` |

The `gh` stub is present in every merge-finalize case, so the real `gh`
never runs. `HOME` is a temporary directory and git runs with
`GIT_CONFIG_NOSYSTEM=1` and `GIT_TERMINAL_PROMPT=0`. No case sleeps or uses
the network.

**Scan test** `no_recipe_masks_check_exit_status` reads every
`amplifier-bundle/recipes/*.yaml` file. On each line that runs a check
command — `cargo test`, `cargo clippy`, `cargo fmt --check`, `npm test`,
`pytest`, `pre-commit run`, `gh pr checks` — it skips comments, cuts the line
at the first single `|` after the command, and fails if the rest contains
`2>/dev/null`, `&>/dev/null`, `|| echo`, `|| true` or `|| :`. It reports
every offending `file:line` at once. `>&2`, `2>&1` and `>>"$LOG"` are
allowed.

The scan does not catch `>/dev/null 2>&1` or masking split across lines.

Run the tests with:

```bash
cargo test -p amplihack-cli --test issue_2190_recipe_steps_report_real_exit_status
```

## Writing a new check step

Follow this shape for any bash step that runs a check another step acts on:

```bash
LOG="$(umask 077; mktemp "${TMPDIR:-/tmp}/my-check.XXXXXX")" || {
  printf '{"all_passing":"false","log_file":""}\n'; exit 0; }
case "$LOG" in
  *[\"\\]* | *[[:cntrl:]]*)
    printf '{"all_passing":"false","log_file":""}\n'; exit 0 ;;
esac
cargo clippy --workspace -- -D warnings >>"$LOG" 2>&1; RC=$?
tail -c 262144 "$LOG" >&2
if [ "$RC" -eq 0 ]; then PASS=true; else PASS=false; fi
printf '{"clippy_exit_code":"%s","all_passing":"%s","log_file":"%s"}\n' \
  "$RC" "$PASS" "$LOG"
```

1. Reject a log path that contains `"`, `\` or a control character before
   it goes into `printf`. `TMPDIR` comes from the environment, and such a
   path would break the JSON. This check is required.
2. Capture `$?` on the line right after the command, with nothing between.
3. Keep stdout for the JSON only.
4. Give the next step a condition of the form `<output>.all_passing != 'true'`.
5. Do not check `PATH` before running a check the project asks for. A
   missing tool exits 127, and that is a failure.
6. Do not add a wait, retry or poll loop to make a check pass.

## Related

- [Recipe Commit Hook Repair](recipe-commit-hook-repair.md) - What happens when a git hook rejects a recipe commit
- [Auto Drive To Merge Reference](auto-drive-to-merge.md) - The evidence-gated merge that reads `autodrive-merge-evidence`
- [Consensus Criticality Contract](consensus-criticality-contract.md) - Another required contract in the consensus recipes
- [Recipe Executor Environment](recipe-executor-environment.md) - The environment bash steps run in
