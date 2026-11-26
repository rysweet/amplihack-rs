---
title: Consensus Criticality Contract
description: The required is_critical_code input to consensus-workflow, the values it accepts, the three checkpoints that enforce it and the errors they report.
last_updated: 2026-11-08
issue: 1966
---

# Consensus Criticality Contract

> [Home](../index.md) > Reference > Consensus Criticality Contract

`is_critical_code` is a required input to `consensus-workflow`. It selects
which Step 5 route `consensus-tdd-impl` runs: the standard builder route, or
the N-version route (three independent builders, a comparison, a synthesis
and a vote). The caller must supply it. No recipe, default or agent step sets
it.

The workflow fails with a named error when the input is missing or invalid,
and again when the selected route did not run to completion. A run that
succeeds has run exactly one complete implementation route and every review
phase. This implements cc factory lesson 29
(required-consensus-implementation-route).

For the workflow itself, see
[Step 5 of the Consensus Workflow](../claude/workflow/CONSENSUS_WORKFLOW.md#step-5-implement-the-solution-n-version-for-critical-code).

## Contents

- [Required Input](#required-input)
- [Accepted Values](#accepted-values)
- [Errors](#errors)
- [Checkpoints](#checkpoints)
- [Route Outputs](#route-outputs)
- [Entry Points](#entry-points)
- [Example](#example)
- [Out of Contract](#out-of-contract)
- [Shared Helper](#shared-helper)
- [Tests](#tests)
- [Related](#related)

---

## Required Input

| Name | Required | Selects the route | Meaning |
|---|---|---|---|
| `is_critical_code` | yes | yes | `true` runs the N-version route; `false` runs the standard route. |
| `criticality` | no | no | Optional free text passed to prompts, for example `"touches token refresh"`. It never satisfies the requirement and never selects a route. |

Four recipes declare `is_critical_code: ""` in their context:
`consensus-workflow`, `consensus-tdd-impl`, `consensus-publish` and
`consensus-merge-finalize`. The other consensus recipes, including
`consensus-requirements-debate`, do not declare it. The empty default counts
as missing, so a run that does not pass the value fails
before any phase starts. Nothing inside the workflow can supply a value:

- no parent recipe passes a default;
- no context block declares a non-empty default;
- no step declares `output: is_critical_code`.

No agent step reads or writes `is_critical_code`, so model output cannot
select or skip the N-version route.

## Accepted Values

The value is read as text from `RECIPE_VAR_is_critical_code` and compared by
exact, case-sensitive match. It is not trimmed or case-folded, and there is
no default.

| Value as the recipe reads it | Result |
|---|---|
| `true` | N-version route |
| `false` | Standard route |
| absent or empty (`''`) | Missing-input error |
| anything else, for example `' true'`, `True`, `FALSE`, `1`, `yes`, `maybe` | Invalid-input error |

Absent and empty cannot be told apart through `RECIPE_VAR_is_critical_code`,
so both are treated as missing.

In recipe context, from a `-c` argument or a context file, a JSON Boolean is
accepted because it reads back as the text `true` or `false`. A workstreams
file is stricter; see [Workstreams field](#workstreams-field). On the command
line, `recipe-runner-rs` parses `-c` values as JSON
first and also maps the exact spellings `True` and `False` to Booleans
(`parse_context_value` in the runner's `src/main.rs`). As a result:

| Command-line argument | Value the recipe reads | Result |
|---|---|---|
| `-c is_critical_code=true` | `true` | N-version route |
| `-c is_critical_code=false` | `false` | Standard route |
| `-c is_critical_code=True` | `true` (runner Boolean) | N-version route |
| `-c is_critical_code=1` | `1` (runner number) | Invalid-input error |
| `-c is_critical_code=TRUE` | `TRUE` | Invalid-input error |

The capitalised strings `True` and `False` are rejected when they arrive any
other way: from a workstreams file, a context file or a parent recipe's
string value.

## Errors

Each error is one line on stderr. The prefix in the first column is stable;
tests match it exactly. The rest of the line is informational. Any value
echoed back is cleaned first: under `LC_ALL=C`, every byte outside
`A-Z a-z 0-9 . _ -` and space becomes `?`, and the result is cut to 64 bytes.
This removes escape sequences, 8-bit control bytes and newlines.

| Code | Stable prefix | Full line | Exit |
|---|---|---|---|
| E-MISSING | `ERROR: missing required input is_critical_code` | `ERROR: missing required input is_critical_code (pass -c is_critical_code=true or -c is_critical_code=false)` | 1 |
| E-INVALID | `ERROR: invalid is_critical_code` | `ERROR: invalid is_critical_code '<value>'; expected 'true' or 'false'` | 1 |
| E-NOROUTE | `ERROR: no implementation route ran` | `ERROR: no implementation route ran (is_critical_code='<value>')` | 1 |
| E-INCOMPLETE | `ERROR: implementation route incomplete:` | `ERROR: implementation route incomplete: <standard\|nversion> missing <output>` | 1 |
| E-BOTH | `ERROR: both implementation routes produced outputs` | `ERROR: both implementation routes produced outputs` | 1 |

Two infrastructure failures exit with status 2 so that they cannot be
mistaken for a contract failure:

| Code | Line | Exit |
|---|---|---|
| E-READ | `ERROR: could not read context value '<name>' (helper exit <rc>)` | 2 |
| Helper not found | `ERROR: amplifier-bundle/tools/consensus_criticality.sh not found or not readable` | 2 |

E-READ means `write_context_value` returned a status other than 0 (present)
or 3 (not set). A missing helper usually means the installed bundle is older
than the recipes; reinstall the bundle.

## Checkpoints

Three Bash steps enforce the contract. None of them has a `condition`, so
none can be skipped, and each fails the run on any error.

| # | Recipe | Step ID | Position | Checks | Errors |
|---|---|---|---|---|---|
| 1 | `consensus-workflow` | `validate-criticality-input` | Step 0, before `consensus-preflight` | The input is present and exactly `true` or `false`. | E-MISSING, E-INVALID |
| 2 | `consensus-tdd-impl` | `step5-validate-criticality` | After `step4-write-tests`, before both routes | The same check, so a direct run of this phase is protected in the same way. | E-MISSING, E-INVALID |
| 3 | `consensus-tdd-impl` | `step5-require-complete-route` | After `step5-nversion-vote`, before the Step 6 expert panel | The input again, then that exactly one route ran to completion. | E-MISSING, E-INVALID, E-READ, E-NOROUTE, E-INCOMPLETE, E-BOTH |

When checkpoint 1 fails, no phase starts. When checkpoint 2 fails, neither
route starts. Checkpoint 3 runs whatever the earlier steps did, so a run in
which every route condition evaluated to false, or an agent step returned
nothing, fails there instead of reporting success.

The route steps themselves are unchanged. `step5-standard-implementation`
runs when `is_critical_code == 'false'`, and the seven `step5-nversion-*`
steps run when `is_critical_code == 'true'`. The requirements debate, design
consensus, the Step 6 expert panel, PR review, philosophy review and the
merge-finalize gates keep their step IDs and conditions.

## Route Outputs

Each route step's `output` is its completion marker. Checkpoint 3 reads them
through `write_context_value` from
`amplifier-bundle/tools/recipe_context_value.sh`.

| Route | Outputs that must all be non-empty |
|---|---|
| Standard (`false`) | `implementation` |
| N-version (`true`) | `critical_sections`, `nversion_v1`, `nversion_v2`, `nversion_v3`, `nversion_comparison`, `implementation`, `n_version_implementations` |

A value counts as present when the helper exits 0 with a non-empty value. An
empty value or exit 3 (not set) counts as absent. Any other exit is E-READ.

Checkpoint 3 evaluates in this order and stops at the first failure:

1. The input is missing or invalid: E-MISSING or E-INVALID.
2. An output cannot be read: E-READ.
3. None of the selected route's outputs is present: E-NOROUTE.
4. Some, but not all, are present: E-INCOMPLETE, naming the first missing
   output in table order.
5. The value is `false` and any N-version-only output (every N-version output
   except `implementation`) is present: E-BOTH.

Both routes write `implementation`, so a mixed run can only be detected when
the value is `false`. When it is `true`, checkpoint 2 has already confirmed
the value, so the standard step's condition is false and that step is
skipped.

### Reported route

On success, checkpoint 3 prints one confirmation line to stderr:

```text
implementation route complete: standard
```

or

```text
implementation route complete: nversion
```

Its stdout is only the route word (`standard` or `nversion`), which the
runner stores as the output `implementation_route`. This is the only step in
the consensus recipes that declares `output: implementation_route`. The
commit and PR text written by `consensus-publish` does not include it.

## Entry Points

| Entry point | How `is_critical_code` arrives |
|---|---|
| `consensus-workflow` (top level) | The caller passes `-c is_critical_code=true` or `-c is_critical_code=false`. Checkpoint 1 rejects anything else. Downstream phases read the same `RECIPE_VAR_is_critical_code`. |
| `consensus-tdd-impl` run directly | The caller passes the value. Checkpoints 2 and 3 apply. |
| Workstreams file (`amplihack multitask run`, `amplihack orch run`) | The optional `is_critical_code` field on a workstream entry, which must be a JSON string. The launcher writes it to the workstream's `context.json` unchanged. |
| `amplihack orch` decomposition | A `Consensus` workstream entry may carry `is_critical_code`. A JSON string is copied unchanged; a JSON Boolean becomes `"true"` or `"false"`; any other type is left out. |

### Workstreams field

```json
[
  {
    "issue": 1966,
    "branch": "feat/issue-1966-token-refresh",
    "task": "Validate refresh tokens before reuse.",
    "recipe": "consensus-workflow",
    "is_critical_code": "true"
  }
]
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `is_critical_code` | string | no | Must be a JSON string. A JSON Boolean or any other type makes the whole workstreams file fail to parse, and no workstream starts. Copied byte for byte into `context.json` when present. Not trimmed, case-folded or defaulted. When absent, `context.json` has no `is_critical_code` key and a `consensus-workflow` run fails with E-MISSING. |

Write `"is_critical_code": "true"`, not `"is_critical_code": true`. The
launcher does not convert types, so it never normalises a value before the
recipe sees it. Beyond the type check, it does not validate the value;
validation stays in the recipe.
`context.json` is written with mode `0600`. The value only ever goes into
JSON, never into shell text.

`smart-orchestrator` does not yet supply the field, so a parallel
`Consensus` workstream it creates fails with E-MISSING. This is intended:
the workstream stops with a named error instead of skipping the N-version
route.

## Example

Run the full workflow for security-sensitive work:

```bash
amplihack recipe run consensus-workflow \
  -c task_description="Validate refresh tokens before reuse" \
  -c is_critical_code=true \
  -c criticality="touches token refresh"
```

Run only the implementation phase on the standard route:

```bash
amplihack recipe run consensus-tdd-impl \
  -c is_critical_code=false \
  -c final_requirements="Rename the --json flag to --output-format json" \
  -c design_consensus="$(cat design.md)"
```

The run ends with:

```text
implementation route complete: standard
```

Omitting the value fails before any phase:

```bash
amplihack recipe run consensus-workflow \
  -c task_description="Rename the --json flag to --output-format json"
# ERROR: missing required input is_critical_code (pass -c is_critical_code=true or -c is_critical_code=false)
```

A misspelt value fails the same way:

```bash
amplihack recipe run consensus-workflow \
  -c task_description="Rename the --json flag to --output-format json" \
  -c is_critical_code=TRUE
# ERROR: invalid is_critical_code 'TRUE'; expected 'true' or 'false'
```

## Out of Contract

The following are accepted limits and are not enforced:

- **Running downstream phases directly.** `consensus-publish` and
  `consensus-merge-finalize` declare `is_critical_code: ""` and read
  `RECIPE_VAR_is_critical_code` only to report it. They do no validation of
  their own. In a composed run they start only after checkpoint 3 has
  passed. A direct run of either, with any value or none, is outside this
  contract.
- **Seeded route outputs.** A direct caller who puts non-empty route outputs
  into the starting context (for example `-c implementation=done` with
  `-c is_critical_code=false`) can satisfy checkpoint 3 without the route
  doing work. Inside `consensus-workflow` no step before
  `consensus-tdd-impl` sets any route output name. A new upstream step that
  does would weaken checkpoint 3 and must be reviewed against this page.
- **Helper lookup.** The lookup snippet that finds the helpers searches
  `AMPLIHACK_HOME`, `REPO_PATH`, the current directory, `~/.copilot` and
  `~/.amplihack`. Searching the current directory is tracked as a separate
  issue.
- **Very large values.** A value large enough to exceed the operating
  system's argument limit fails in the runner (E2BIG) before any checkpoint
  sees it.

## Shared Helper

All three checkpoints source `amplifier-bundle/tools/consensus_criticality.sh`,
the only place that checks the input and the route. Each checkpoint first
locates the bundle with the shared lookup snippet, checks
`[ -r "$H/consensus_criticality.sh" ]` and exits 2 if the helper is missing.

| Function | Returns | Sets |
|---|---|---|
| `cc_validate_input` | 0, or 1 with E-MISSING or E-INVALID | nothing |
| `cc_require_route` | 0; 1 with E-NOROUTE, E-INCOMPLETE or E-BOTH (or E-MISSING, E-INVALID); 2 with E-READ | `CC_ROUTE` to `standard` or `nversion`, on success only |

Rules the helper follows:

- `CC_ROUTE` is cleared on entry to `cc_require_route`.
- All messages go to stderr.
- Probes run in the function's own shell with the exit status captured
  directly, never in a pipe or a loop subshell, so exit 2 is not lost.
- No `eval`; the caller's shell options are left unchanged; every variable
  uses the `cc_` prefix.
- No checkpoint command contains a `{{is_critical_code}}` or
  `{{criticality}}` template. Values are read only from `RECIPE_VAR_*` and
  printed only through `printf '%s'`.

Checkpoint 3 uses it as follows:

```bash
cc_require_route || exit $?
echo "implementation route complete: $CC_ROUTE" >&2
printf '%s' "$CC_ROUTE"
```

## Tests

The contract is tested by `tests/integration/issue_1966_consensus_criticality.rs`;
the launcher field is tested in
`crates/amplihack-cli/src/commands/multitask/issue_1966_criticality.rs`.

The end-to-end tests in `issue_1966_consensus_criticality.rs` need a local
`recipe-runner-rs`, found through `RECIPE_RUNNER_RS_PATH` or `PATH`. If none
is found they fail with a message naming both; they do not skip or pass
silently. The static checks and the helper unit test do not need the runner.

## Related

- [Consensus Workflow](../claude/workflow/CONSENSUS_WORKFLOW.md): the
  workflow this contract belongs to
- [`orch run` workstreams schema](./orch-run-command.md#workstreams-json-schema):
  the `is_critical_code` workstream field
- [Recipe Context Environment](./recipe-context-environment.md): how context
  values reach Bash steps as `RECIPE_VAR_*`
- [Large Payload Delivery](./large-payload-delivery.md#reading-a-context-value-in-a-bash-step):
  `write_context_value` and its exit codes
- [Recipe Runner Audit](./recipe-runner-audit.md): finding F-COND-4
