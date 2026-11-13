---
title: Step Failure Recovery Reference
last_updated: 2026-11-12
review_schedule: quarterly
owner: workflow-team
---

# Step Failure Recovery Reference

A recipe step that opts in no longer ends the run when it fails. Its failure
goes to an agent, which reads the step's id, exit status, output and the run's
goal, then chooses one of four ways forward. The choice and its reason are
written to a record. Only a recorded handoff can end the run, always with
exit 81 and the record's path: at the step itself for guard and
stop-on-handoff steps, otherwise at a report step (issue #2539).

Recovery acts on events and evidence. It has no timers, no waits and no
attempt counter. It stops looping when the same failure comes back after a
repair, and that is decided from the records, not by counting.

> **Internal command.** `amplihack orch helper step-recovery` is called by
> bundle recipes. Its flags and output are documented here so recipe authors
> can opt steps in; it is not a user-facing command and may change with the
> recipes that call it.

## Contents

- [How recovery works](#how-recovery-works)
- [Where it applies](#where-it-applies)
- [The four choices](#the-four-choices)
- [Opting a step in](#opting-a-step-in)
- [`step-recovery` commands](#step-recovery-commands)
- [The recovery recipe](#the-recovery-recipe)
- [Agent reply](#agent-reply)
- [Validation](#validation)
- [Failure signature](#failure-signature)
- [Recurrence rule](#recurrence-rule)
- [Record format](#record-format)
- [Outcome line](#outcome-line)
- [How a handoff ends the run](#how-a-handoff-ends-the-run)
- [Exit 79, 81 and signals](#exit-79-81-and-signals)
- [Recovery directory](#recovery-directory)
- [Security](#security)
- [Runner gaps](#runner-gaps)
- [Tests](#tests)
- [Related references](#related-references)

## How recovery works

```
step command ──exit 0──▶ output passed through unchanged, no record
     │
     ├─exit 79 or signal──▶ policy handoff record, exit code passed on (no agent)
     ├─exit 81, handoff on record─▶ nested run already handed off: passed on (no agent)
     │
     ├─a handoff is already recorded in this run──▶ validator handoff (no agent)
     │
     └─any other failure
           │
           ▼
   evidence file (redacted, 0600)
           │
           ▼
   recipe step-failure-recovery  ──▶ one {"step_recovery":{…}} line
           │
           ▼
   driver validates the reply, computes recurrence, appends a record
           │
           ├─ retry_after_repair ─▶ re-run the command once, at once
           ├─ alternative_step ──▶ run the declared alternative
           ├─ continue_recorded ─▶ print outcome line, exit 0
           └─ handoff ───────────▶ print outcome line; exit 81 with
                                     --stop-on-handoff or --guard (the
                                     recipe stops here), else exit 0
                                     ...auto-drive loop stops after the round
                                     ...report step exits 81
```

If a retry or an alternative fails, the driver goes back to recovery with the
full history for that step. Each pass appends one record.

Records cover the whole run. One recovery directory serves a top-level run,
every child recipe it starts and every auto-drive loop round, so a failure
repaired in round 1 that comes back in round 7 is a
[recurrence](#recurrence-rule), and its retry is refused.

The driver is `amplihack orch helper step-recovery run`. It spawns the wrapped
command directly as an argument vector, with no shell. It writes the command's
standard output to a private spool file and keeps only the tail in memory, so
a large output does not grow memory. Standard error is kept for the excerpt
and also passed through to the step log. When an attempt fails, its whole
standard output is copied to the step log (standard error) under the line
`[step-recovery] stdout of failed attempt <step id>:`, so a failed attempt's
output is not lost when a retry later succeeds.

## Where it applies

| Recipe | Step | Form | Notes |
|---|---|---|---|
| `autodrive-build.yaml` | `step-02-default-workflow` | sub-recipe | Recovered by the following bash step `step-02r-default-workflow-recovery`, with `--essential` |
| `autodrive-crusty-loop.yaml` | `step-02-crusty-loop` | bash | `--essential --stop-on-handoff`: the crusty loop is never skipped, and a handoff ends the drive here |
| `autodrive-crusty-loop.yaml` | `step-03-record-crusty-phase` | agent | Recovered by `step-03r-record-crusty-phase-recovery`, with `--essential`; a retry re-runs the check that the crusty phase is recorded |
| `autodrive-crusty-round.yaml` | `step-00-review-dir-guard` | bash | `--guard` |
| `autodrive-crusty-round.yaml` | `step-02b-review-dir-unchanged` | bash | `--guard` (the `round-index-check`) |
| `autodrive-merge-round.yaml` | `step-00-review-dir-guard` | bash | `--guard` |
| `autodrive-merge-round.yaml` | `step-00-merge-ready-files` | bash | `--essential --stop-on-handoff`: a handoff never reaches the `parse_json` consumers as a file list |
| `autodrive-merge-round.yaml` | `step-02b-review-dir-unchanged` | bash | `--guard` (the `round-index-check`) |
| `autodrive-merge-loop.yaml` | `step-02-merge-ready-loop` | bash | `--essential --stop-on-handoff` |
| `autodrive-merge-loop.yaml` | `step-03-finalize` | sub-recipe | Recovered by `step-03r-finalize-recovery`, with `--essential`; a retry runs `autodrive-finalize` again |
| `workflow-finalize.yaml` | `step-20b-push-cleanup` | bash | `--essential --stop-on-handoff --no-repair`: the step commits and pushes the worktree, so a repair would be published without review; a failure here can only hand off |
| `workflow-finalize.yaml` | `step-21-pr-ready` | bash | `--essential --stop-on-handoff`; no alternative: the ready mutation has no REST fallback. A handoff never reaches `step-22-ensure-mergeable` |
| `workflow-finalize.yaml` | `validate-agentic-finalization` | bash | `--guard` |
| `workflow-finalize.yaml` | `step-recovery-report` | report | Runs after `validate-agentic-finalization` and before `workflow-complete`; exits 81 on a handoff |
| `auto-drive-to-merge.yaml` | `autodrive-report` | report | Exits 81 on a handoff |
| `workflow-worktree.yaml` | `step-04-setup-worktree` | bash | Under an auto-drive, also writes the build's branch to `build-branch.txt` in the recovery directory, for a retried build |
| `autodrive_loop.sh` | before the first round and after each round | report | Stops the loop with exit 81 when the run holds a handoff |

`workflow-finalize.yaml` also gains a first step, `step-recovery-init`, which
creates the recovery directory when the caller did not pass one. Because
`step-recovery-report` runs before `workflow-complete`, a run with a handoff
ends at exit 81 and never reaches `workflow-complete`. As a second guard,
`workflow-complete` runs the same report first and stops with its exit status
when the records hold a handoff. A `continue_recorded` outcome does not block
success.

`auto-drive-to-merge.yaml` gains a first step of the same name and passes the
directory to `autodrive-build`, both loops and, through them, every round. Its
loop steps now also need a `build_result`, so a build that ended in a handoff
goes straight to `autodrive-report`.

The review-directory guards and `validate-agentic-finalization` run with
`--guard`: a step that checks something may only report, so the agent can
neither skip it nor repair what it checks, and a handoff ends the round or
the run at that step, before any agent work after it (#1528). The merge-ready
file resolver, both loop steps and the two finalize steps before outward
actions run with `--essential --stop-on-handoff`. The other converted steps
exit 0 on a handoff; only `autodrive-build` `step-03-resolve-pr` is then gated by a
`condition:`, and the loop or report step ends the run.

If `step-recovery-init` fails (for example a `HOME` path that holds a
symlink), it warns on standard error, outputs an empty directory and exits 0.
Recovery is then off for the run: every wrapped step runs its body
unwrapped and fails exactly as it did before recovery existed.

Default-workflow publish steps are not converted yet; they follow in later
pull requests.

## The four choices

| Choice | Agent supplies | What the driver does |
|---|---|---|
| `retry_after_repair` | `reason`, `repair` | Not offered for steps run with `--guard`. The agent has already made the repair inside its recovery step, with its normal tools and under the usual rules (no hook bypass, no edits to protected files). `repair` says what it changed. The driver re-runs the same command once, straight away. If that fails, recovery runs again with the new failure and all earlier records. |
| `alternative_step` | `reason`, `alternative` | `alternative` must be the name of an `--alternative` the caller declared. The driver runs that command. If it fails, recovery runs under the step id `<id>:alt:<name>`, and an alternative is never offered alternatives of its own. |
| `continue_recorded` | `reason` | The item is skipped and recorded. The driver prints the [outcome line](#outcome-line) and exits 0. Not offered for steps run with `--essential` or `--guard`. |
| `handoff` | `reason`, `owner`, `needs` | The driver prints the outcome line. With `--stop-on-handoff` or `--guard` it exits 81, so the recipe ends at this step and `amplihack recipe run` exits 81. Otherwise it exits 0 and later steps run; an auto-drive loop stops after the round, and the run ends at the [report step](#how-a-handoff-ends-the-run) with exit 81. |

`owner` must be a trusted value: a role from the evidence file's
`owner_roles`, the caller's `--owner`, or the evidence file's `default_owner`.
The default owner is the caller's `--owner` when given, otherwise the role
`run-requester`. The converted steps pass no `--owner`, so their default
owner is `run-requester`.

## Opting a step in

### Bash steps

Wrap the existing command. Everything after `--` is the command, run as an
argument vector. Read every value from quoted `"$RECIPE_VAR_*"` variables;
never splice `{{…}}` into the command.

The bundle recipes keep a multi-line step body unchanged. They read it into a
variable with a quoted here-document and run it with `bash -c "$STEP_BODY"`,
so no value is spliced into the body. Without a recovery directory the body
runs as before, which keeps a recipe usable on its own:

```yaml
    command: |
      IFS= read -r -d '' STEP_BODY <<'STEP_RECOVERY_BODY' || :
      ...the step's original commands, unchanged...
      STEP_RECOVERY_BODY
      [ -n "${RECIPE_VAR_step_recovery_dir:-}" ] || exec bash -c "$STEP_BODY" step-21-pr-ready
      exec amplihack orch helper step-recovery run --step-id step-21-pr-ready \
        --dir "$RECIPE_VAR_step_recovery_dir" \
        --essential --stop-on-handoff \
        --goal "Mark PR ${RECIPE_VAR_pr_number:-unknown} ready for review" \
        -- bash -c "$STEP_BODY" step-21-pr-ready
```

The goal names the pull request, so the agent knows which run it serves.

Before:

```yaml
  - id: "step-03-finalize"
    type: "bash"
    command: |
      "$TOOLS_DIR/autodrive_finalize.sh" --pr "$RECIPE_VAR_pr_number"
```

After:

```yaml
  - id: "step-03-finalize"
    type: "bash"
    command: |
      amplihack orch helper step-recovery run \
        --step-id step-03-finalize \
        --dir "$RECIPE_VAR_step_recovery_dir" \
        --goal "Finalize PR $RECIPE_VAR_pr_number for merge" \
        --owner "$RECIPE_VAR_pr_author" \
        -- "$TOOLS_DIR/autodrive_finalize.sh" --pr "$RECIPE_VAR_pr_number"
    output: "finalize_result"
```

On success the step's output is exactly what the command printed, so existing
parsers of `finalize_result` keep working. A step that comes before an
outward action, or whose output later steps parse, adds `--stop-on-handoff`
(or `--guard` for a check), so a handoff ends the recipe there. Without it,
on `continue_recorded` or `handoff` the output is the outcome line instead,
and steps that need the real result check for it:

```yaml
    condition: "finalize_result and 'step_recovery' not in finalize_result"
```

To offer an alternative, declare it by name. Only declared names can be
chosen, and the agent never supplies a command:

```yaml
        --alternative "rest-head-read=gh api repos/$RECIPE_VAR_repo/pulls/$RECIPE_VAR_pr_number --jq .head.sha" \
```

The `<command>` side of `--alternative <name>=<command>` is split on
whitespace into an argument vector, with no shell and no quoting, so an
argument cannot contain a space. When one must, put the command in a script
under `amplifier-bundle/tools/` and name the script as the alternative.
Declare only reads as alternatives to reads; a GitHub write never falls back
to another API.

### Sub-recipe and agent steps

The runner drops a failed sub-recipe's or agent step's output, so the driver
cannot wrap it. Instead:

1. Give the step `continue_on_error: true` and an `output:` key.
2. Add a bash step straight after it whose `condition:` is `not <output_key>`.
   It calls `run --already-failed` and passes, after `--`, a command that runs
   the same operation again.

In `autodrive-build.yaml` the recipe step stores its result as
`workflow_result`, which `step-03-resolve-pr` already reads. The recovery step
stores its result as `build_recovery`; `build_result` stays the output of
`step-03-resolve-pr`.

```yaml
  - id: "step-02-default-workflow"
    condition: "build_preflight.should_run == 'true'"
    type: "recipe"
    recipe: "default-workflow"
    continue_on_error: true
    context:
      # ...existing context unchanged, plus:
      step_recovery_dir: "{{step_recovery_dir}}"
    output: "workflow_result"

  - id: "step-02r-default-workflow-recovery"
    type: "bash"
    condition: "build_preflight.should_run == 'true' and not workflow_result"
    command: |
      amplihack orch helper step-recovery run \
        --already-failed \
        --essential \
        --step-id step-02-default-workflow \
        --dir "$RECIPE_VAR_step_recovery_dir" \
        --goal "Build a PR for: $RECIPE_VAR_task_description" \
        -- amplihack recipe run default-workflow \
             -c "task_description=$RECIPE_VAR_task_description" \
             -c "repo_path=$RECIPE_VAR_repo_path" \
             -c "step_recovery_dir=$RECIPE_VAR_step_recovery_dir" \
             -c no_merge=true -c should_merge=false -c finalization_handoff=true
    output: "build_recovery"

  - id: "step-03-resolve-pr"
    type: "agent"
    condition: "not build_recovery or 'step_recovery' not in build_recovery"
    # prompt reads {{workflow_result}} and {{build_recovery}} (see below)
    output: "build_result"
```

The example is shortened. The bundle's step-02r uses the here-document form
above, and takes the task and requirements from `step-01b`'s output, with the
no-merge directive, as step-02 does. A retry must continue
the branch and PR the first attempt created. The first attempt's
`workflow-worktree` writes its branch to `build-branch.txt` in the recovery
directory (`step-04-setup-worktree`), and when the drive was given no
`existing_branch`, step-02r passes that branch as `existing_branch`. Without
it, `workflow_worktree_deconflict.sh` would give the retry a new branch, and
so a second PR. The agent cannot change the command, so the choice of branch
is not left to it.

With `--already-failed`, the first failure is not run; the driver goes
straight to recovery. Its record sets `exit_code` to `null` and
`unavailable_reason` to `"runner does not expose failed sub-recipe status"`
rather than inventing a status. A retry or alternative then runs the command
after `--` like a normal wrapped step. `--essential` is set because a build
cannot be skipped, so the only outcome line this step can print is a handoff.

`step-03-resolve-pr` then sees one of three cases:

| `workflow_result` | `build_recovery` | `step-03-resolve-pr` |
|---|---|---|
| set | empty (step-02r skipped) | Runs and reads `workflow_result`, as today |
| empty | Output of the successful retry or alternative | Runs and reads `build_recovery` in place of `workflow_result`. Its prompt says: when `workflow_result` is empty, the build result is `build_recovery`. |
| empty | A handoff outcome line | Skipped by its `condition:`; `build_result` stays empty and the run ends at the report step with exit 81 |

The nested `default-workflow` gets the same `step_recovery_dir`, so a handoff
inside it is already in the run's records. If it ends with exit 81, the
driver passes 81 on without starting an agent (see
[Exit 79, 81 and signals](#exit-79-81-and-signals)).

### Passing the recovery directory

Steps take the directory from the recipe variable `step_recovery_dir`.
Auto-drive creates one directory per run with `step-recovery init` and passes
it on with `-c step_recovery_dir=…` to its child recipes, including every
round recipe started by `autodrive_loop.sh`. See
[Recovery directory](#recovery-directory).

### Recursion budget

Each recovery adds a nested recipe run of one step. The nested `recipe run`
is a separate process and does not count toward the parent's
`recursion.max_total_steps`, so no cap was raised. The added steps fit inside
the existing caps.

## `step-recovery` commands

All four subcommands are under `amplihack orch helper step-recovery`.

### `init`

```
step-recovery init [--dir <path>]
```

With `--dir`, checks that the path is an existing private directory (owned by
the current user, mode `0700`, not a symlink) and prints it. Without it,
creates `<state>/amplihack/step-recovery/<32 hex>` with mode `0700` and
prints the path. `<state>` is `$XDG_STATE_HOME` when it is set and absolute,
otherwise `$HOME/.local/state`. It is never under `/tmp`.

`init` never stops a run. On any error (an unsafe or symlinked path, a
directory it cannot create) it writes
`[step-recovery] WARNING: recovery disabled: <error chain>` to standard
error, prints an empty line and exits 0. Recipes treat an empty directory as
"recovery off", so the wrapped steps run unwrapped. Leftover directories are
not cleaned up yet.

### `run`

```
step-recovery run --step-id <id> --dir <recovery_dir> --goal <text>
                  [--owner <value>] [--alternative <name>=<command>]...
                  [--essential] [--stop-on-handoff] [--no-repair] [--guard]
                  [--already-failed] [--recipe-dir <dir>]
                  -- <command> [args...]
```

| Flag | Meaning |
|---|---|
| `--step-id` | Matches `^[A-Za-z0-9._-]{1,128}$`. `:` and `__alt__` are refused; only the driver builds `<id>:alt:<name>` ids, whose record file is `<id>__alt__<name>.jsonl`. `.` and `..` are refused. |
| `--dir` | Absolute path to the recovery directory. A relative path is refused. |
| `--goal` | The run's goal in one line, given to the agent. |
| `--owner` | Default handoff owner: a role, or a value from the run context, matching `^[A-Za-z0-9._@/-]{1,128}$`. Default `run-requester`. |
| `--alternative` | A named alternative command. Repeatable. Names match `^[A-Za-z0-9_-]{1,64}$`. |
| `--essential` | The step must not be skipped: `continue_recorded` is removed from the allowed choices. |
| `--stop-on-handoff` | A recorded handoff exits 81 instead of 0, so the recipe stops at this step. `continue_recorded` and a successful retry still exit 0. |
| `--no-repair` | `retry_after_repair` is removed from the allowed choices. For a step that publishes the worktree, whose retry would otherwise push a repair no review has seen. |
| `--guard` | The step checks something and may only report: only `handoff` and declared alternatives are offered, and the evidence has `guard: true`. Implies `--essential`, `--stop-on-handoff` and `--no-repair`. |
| `--already-failed` | Skip the first run and go straight to recovery (sub-recipe and agent steps). |
| `--recipe-dir` | Run `<dir>/step-failure-recovery.yaml` by path instead of the bundle's. A recipe's own directory is searched first, so its sub-recipes also come from `<dir>`. Used by tests and fixture recipes. |

| Exit | Meaning |
|---|---|
| 0 | The command succeeded (output passed through), or the outcome is `continue_recorded`, or `handoff` without `--stop-on-handoff` or `--guard` (outcome line printed) |
| 79 | The command exited 79; a policy handoff was recorded and 79 passed on |
| 81 | A handoff was recorded under `--stop-on-handoff` or `--guard` (outcome line printed), or the command was a nested run that ended in a handoff (81 passed on, no agent) |
| 128+n | The command was killed by signal n; a policy handoff was recorded |
| 2 | Bad arguments, an unsafe recovery directory or record file, or a record could not be appended |

### `record`

```
step-recovery record --dir <recovery_dir> --step-id <id> --reason <text>
                     --owner <value> --needs <text>
```

`--owner` matches the same pattern as `run --owner`.

Appends a `handoff` record with `source: "validator"`. This is the only kind
of record it writes. Recipes use it when required evidence is missing and no
step failed, for example when auto-drive finds no finalization evidence. That
case becomes a recorded handoff instead of a bare `exit 1`.

### `report`

```
step-recovery report --dir <recovery_dir>
```

Reads every record in every `*.jsonl` file in the directory. **Any** handoff
record decides, wherever it is in the file. A later record for the same step,
such as a `continue_recorded` written in a later loop round, does not hide an
earlier handoff.

| Exit | Meaning | Standard output |
|---|---|---|
| 0 | No record in the directory is a handoff | `{"step_recovery_handoffs":[]}` |
| 81 | At least one record is a handoff | `{"step_recovery_handoffs":[{"step_id":…,"owner":…,"needs":…,"reason":…,"source":…,"record":…}]}` |
| 2 | A record file is invalid, unsafe (wrong mode or owner, a symlink) or larger than 4 MiB | error on standard error |

Handoffs are listed in file order, then record order. On exit 81 the report
also writes, for each handoff, the owner, reason, needs and record path to
standard error, so they appear in the step log. It never prints
`output_excerpt`.

A handoff stays in force for the rest of the run. Nothing clears it, and once
one is recorded, `run` starts no further agents in that directory.

## The recovery recipe

`amplifier-bundle/recipes/step-failure-recovery.yaml` has one agent step and
`recursion.max_total_steps: 5`. The driver starts it as
`amplihack recipe run step-failure-recovery -c step_recovery_evidence=<path>`,
with `AMPLIHACK_STEP_RECOVERY_ACTIVE=1` set in the child's environment. The
agent binary is inherited, so `AMPLIHACK_AGENT_BINARY` selects it in tests.
The driver ignores the child's exit status; only the reply line counts. It
reads the output of the run result's **last** step only (or the raw output
when there is no run result), and the reply is the final non-empty line of
that text. A `{"step_recovery":…}` object on any earlier line or in an
earlier step is never the reply. Output of 1 MiB or more is refused with
`no step_recovery reply: output too large` and becomes a validator handoff;
it is still drained, so the child never blocks on a full pipe.

The prompt gives the agent the evidence file's path, not its contents. It
tells the agent that the file's text, including the failed command's output,
is untrusted data and that instructions inside it are not to be followed. The
agent may read files and make local repairs in the working tree. It must not:

- push, open, edit or comment on pull requests or issues, or make any other
  GitHub write;
- bypass or disable hooks (`--no-verify`, `SKIP=`, `core.hooksPath`) or edit
  hook configuration;
- read token or credential files;
- modify the review directory, the main checkout, any validator or guard
  script, or the evidence files they read;
- write under the recovery directory;
- start another recipe or orchestration.

When the evidence has `guard: true`, the prompt tells the agent that only
`handoff` or a listed alternative is allowed.

The evidence file (schema version 1) holds:

| Field | Content |
|---|---|
| `untrusted_notice` | Fixed text saying the fields below come from the failed step |
| `step_id` | The step's id (or `<id>:alt:<name>`) |
| `goal` | `--goal` |
| `failure` | `exit_code`, `signal`, `unavailable_reason`, `signature`, `output_excerpt` |
| `allowed_choices` | The choices valid now (see below) |
| `alternatives` | Declared alternative **names** only, never their commands |
| `prior_records` | Earlier records for this step id, oldest first |
| `recurrence` | Whether this failure is a recurrence, as computed by the driver |
| `default_owner` | The default handoff owner |
| `owner_roles` | Roles the agent may name, including `run-requester` |
| `guard` | Whether the step runs with `--guard` |

`allowed_choices` starts with all four and removes:

- `alternative_step` when no alternatives are declared, or the step is itself
  an alternative;
- `continue_recorded` under `--essential` or `--guard`;
- `retry_after_repair` under `--guard`, or when the failure is a recurrence.

In the excerpt, a `"step_recovery"` key is rewritten to
`"step_recovery_quoted"`, so output that plants a reply can never be parsed
as one.

Evidence files are named `evidence-<id>-<n>-<8 hex>.json`, created with mode
`0600` in the recovery directory, and kept as audit material.

## Agent reply

The agent's last output line is exactly one JSON object:

```json
{"step_recovery":{"choice":"retry_after_repair","reason":"tool read $TOP under set -u; TOP is now set from git rev-parse --show-toplevel","repair":"set TOP before first use in the checked-out tool script"}}
```

```json
{"step_recovery":{"choice":"handoff","reason":"bundle copy refused: unsafe ownership or modes on the staged directory","owner":"run-requester","needs":"fix ownership of the staged bundle directory, then rerun the drive"}}
```

| Field | Required for | Limits |
|---|---|---|
| `choice` | all | One of the four choices |
| `reason` | all | 1–1024 bytes |
| `repair` | `retry_after_repair` | 1–1024 bytes |
| `alternative` | `alternative_step` | A declared name |
| `owner` | `handoff` | A trusted value (see [The four choices](#the-four-choices)) |
| `needs` | `handoff` | 1–1024 bytes; what the owner must do |

Fields not listed for a choice must be absent. The agent does not send
`recurrence` or `source`; the driver sets both. Text fields reject control
characters and the bidi characters U+202A–U+202E and U+2066–U+2069.

## Validation

The driver finds the reply with the same JSON extraction used for other agent
replies (the last object with a `step_recovery` field) and checks, in order:

1. A reply line exists and parses as JSON.
2. It has no unknown fields.
3. `choice` is in `allowed_choices`.
4. Text fields are within their limits and have no control characters.
5. The fields required by the choice are present, and no others.
6. `alternative`, if any, is a declared name; `owner`, if any, is trusted.
7. Repeated alternative: when the failure is a recurrence, the same
   alternative chosen before for this signature is refused. (A retry on a
   recurrence is already refused by check 3, because `retry_after_repair` is
   not in `allowed_choices`.)

The first failed check turns the outcome into a `handoff` with
`source: "validator"`, owner `run-requester` (or the caller's `--owner`), and
a reason that names the failed check followed by the agent's own reason, cut
to 1024 bytes.

The driver also turns these into validator handoffs without consulting the
agent's choice:

- `AMPLIHACK_STEP_RECOVERY_ACTIVE` is already set: recovery is running inside
  recovery, so no agent is started.
- The recovery directory already holds a handoff record from any step. The
  run is already handed off, so no agent is started; the new record's reason
  names the step and record file of the earlier handoff.
- The recovery directory changed while the agent ran. The driver takes the
  name, length and SHA-256 of every file in it (dot files, which are
  transient spool and temporary files, excepted) before starting the agent
  and compares them after. A removed, added or altered file of any step
  gives a validator handoff.

## Failure signature

```
exit=<n|none>;[class=<c>;]tail=<16 hex>
```

- `exit` is the exit code, or `none` when it is unavailable.
- `class` is the failure class from the recipe failure classifier, when it
  applies.
- `tail` is the first 8 bytes, in hex, of the SHA-256 of the redacted output
  tail (its last 40 lines) after normalisation: lowercased, lines trimmed,
  blank lines dropped, runs of 7 or more hex digits that hold at least one
  letter replaced by `H`, then digit runs replaced by `0`. A run of digits
  alone is a number, not an id, so a 7-digit PID normalises like a 3-digit
  one.

So the same failure with different commit ids, PIDs or timestamps has the
same signature, and a different error message has a different one.

Example: `exit=1;tail=3f9a0c51d2e87b46`.

## Recurrence rule

A failure is a **recurrence** when an earlier record for the same step id has
the same signature and chose `retry_after_repair` or `alternative_step`. The
driver computes this; the agent cannot set it. "Earlier" means anywhere in the
run: records are kept for the whole run, across child recipes and auto-drive
loop rounds, so a failure repaired in round 1 that returns in round 7 is a
recurrence.

On a recurrence the agent must choose `handoff`, or `continue_recorded` when
the item is not essential. It may also choose a different declared
alternative. If it chooses retry (refused by check 3) or the same alternative
again (refused by check 7), the validator records a handoff with
`source: "validator"` and keeps the agent's reason.

The rule reacts to an event, a repeat after a repair. It counts nothing.

**Known limit.** A step that fails with a different signature every time can
go round again and again. Each round needs a new, different failure, which is
evidence that something changed. Stopping that case would need a count, and
counts are not used. If you see this in a run, the records show each
signature in turn.

## Record format

Each step's records are in `<recovery_dir>/<sanitised step id>.jsonl`, one
JSON object per line, appended. Sanitising replaces `:alt:` with `__alt__`;
names are never shortened. A record whose sanitised `step_id` does not match
its file name is invalid (exit 2), and a step's history holds only records
whose `step_id` equals it.

```json
{"schema_version":1,"step_id":"step-21-pr-ready","failure":{"exit_code":1,"signature":"exit=1;tail=3f9a0c51d2e87b46","output_excerpt":"no_scoped_pr: no pull request found for this branch"},"choice":"continue_recorded","reason":"the branch PR exists and is already ready for review; the scope lookup missed it","recurrence":false,"source":"agent","recorded_at":"2026-11-12T21:04:33Z"}
```

| Field | Type | Notes |
|---|---|---|
| `schema_version` | integer | `1` |
| `step_id` | string | Unsanitised id |
| `failure.exit_code` | integer or null | null with `unavailable_reason` or `signal` |
| `failure.signal` | integer, optional | Set when the command was killed by a signal |
| `failure.unavailable_reason` | string, optional | Why the status is unknown (runner gap) |
| `failure.signature` | string | See [Failure signature](#failure-signature) |
| `failure.output_excerpt` | string | Redacted; last 40 lines, at most 2000 characters |
| `choice` | string | One of the four choices |
| `reason` | string | 1–1024 bytes |
| `repair` | string, optional | With `retry_after_repair` |
| `alternative` | string, optional | With `alternative_step` |
| `owner` | string, optional | With `handoff` |
| `needs` | string, optional | With `handoff` |
| `recurrence` | boolean | Computed by the driver |
| `source` | string | `agent`, `validator` or `policy` |
| `recorded_at` | string | RFC 3339 UTC. Evidence only; nothing waits on it |

Records are parsed with unknown fields refused, and the fields must agree:
`owner` and `needs` only with `handoff`; exactly one of `exit_code`,
`signal`, `unavailable_reason` explains the status.

## Outcome line

On `continue_recorded` or `handoff`, `run` prints a short text line and then
the outcome line, which is always the last line of the step's output:

```
step-recovery: handoff recorded for step-21-pr-ready
{"step_recovery":{"choice":"handoff","step_id":"step-21-pr-ready","owner":"run-requester","needs":"check why the ready mutation is refused","reason":"…","source":"agent","record":"<state>/amplihack/step-recovery/<run>/step-21-pr-ready.jsonl"}}
```

Later steps test for `step_recovery` in the output to tell a skipped or
handed-off item from a real result. The text line is there for the runner:
it parses a step output that is a single JSON object into an object, and
`'step_recovery' in <output>` then does not match. With the text line first,
the output stays a string and the substring test works.

## How a handoff ends the run

Steps run with `--stop-on-handoff` or `--guard` end the recipe at the step
with exit 81: the runner fails the step, no later step runs, and
`amplihack recipe run` exits 81 when a failed step ended with 81. Other
steps exit 0 on a handoff, so the runner reaches later steps; only
`autodrive-build` `step-03-resolve-pr` is gated by `condition:`. Any
handoff in the records gives exit 81 at a report, not the last record of
each step. Three places run `step-recovery report`:

- `autodrive_loop.sh`, at the end of every crusty and merge round, after the
  round ends and the review directory is checked, and before the round record
  is read, the loop-health evaluator runs or the next round starts.
  On exit 81 the loop escalates `STEP_RECOVERY_HANDOFF` with the record path
  and exits 81, so no further round starts and no new agent meets the same
  step. On exit 2 it escalates `INVALID_RECOVERY_RECORDS` and exits 2.
  The loop step passes 81 on without starting an agent, and the runner ends
  the run at that loop step; nothing retries it. The loop's own report names
  the owner, reason and record.
- `auto-drive-to-merge.yaml`: `autodrive-report`, before any other exit path.
  Missing finalization evidence with no record is first written as a
  validator handoff with `record`.
- `workflow-finalize.yaml`: `step-recovery-report` (skipped when init
  disabled recovery), after
  `validate-agentic-finalization` and **before** `workflow-complete`. The
  finalization helper exits 1 on any non-success state, so a report placed
  after it would never run. With the report first, a handed-off run ends with
  exit 81 at the report, and `workflow-complete` runs only when no handoff is
  recorded.

The report prints the owner, reason, needs and record path and exits with
`STEP_RECOVERY_HANDOFF_EXIT`, which is **81**. The constant is defined once
in the Rust `step_recovery` module, and `autodrive_loop.sh` reads it from the
report's exit status rather than repeating the number. 81 is not used by any
other amplihack exit path (70–77, 79, 80, 86, 91 and 97–99 are taken).

The converted steps have no bare `exit 1` paths left: every failure ends as
success, a record, exit 79, exit 81 or `128+n`.

## Exit 79, 81 and signals

Exit 79 means a policy refused the operation. It is final and is never sent
to the agent. The driver writes a `handoff` record with `source: "policy"`
and reason `terminal policy refusal (exit 79)`, then exits 79.

Exit 81 means a nested run, such as the `default-workflow` started by
`step-02r-default-workflow-recovery`, already ended in a handoff that is in
the run's records. When the recovery directory holds a handoff record, the
driver writes no new record, starts no agent and exits 81. When it holds
none, the 81 did not come from a recorded handoff, and the driver treats it
as an ordinary failure and starts recovery.

A command killed by a signal (for example SIGTERM when the run is cancelled)
gets a `policy` handoff record and the driver exits `128+n` (143 for
SIGTERM). No evidence is built and no agent starts, so a cancelled run never
begins a repair.

## Recovery directory

| Caller | Directory |
|---|---|
| Auto-drive | Created by `step-recovery init` when the run starts; passed to every child recipe and loop round |
| workflow-finalize, with `-c step_recovery_dir=…` | That directory, checked by `init --dir` |
| workflow-finalize, standalone | Created by `step-recovery-init` under `$XDG_STATE_HOME` or `$HOME/.local/state` |

There is one directory per top-level run, shared by everything that run
starts. It is not kept in auto-drive's per-PR state directory, because a
later drive of the same PR would then inherit the earlier drive's handoffs
and refuse to start any agent. A new drive starts with an empty directory;
the old one stays as audit material. Nothing removes recovery directories
or limits how long they are kept: every auto-drive and every standalone
`workflow-finalize` run leaves one under
`$XDG_STATE_HOME/amplihack/step-recovery` (or `$HOME/.local/state/…`).
Remove old ones by hand; a retention rule is a follow-up.

The directory is mode `0700`. Record, evidence and spool files are mode
`0600`.

## Security

- All file access goes through the `secure_fs` helpers: paths are opened
  relative to a checked directory descriptor with `O_NOFOLLOW`, and a file or
  directory not owned by the current user, or with any group or other
  permission bits, is refused. Unsafe paths are refused, never repaired with
  `chmod`.
- Secrets are redacted before an excerpt or signature is computed: GitHub
  tokens (`ghp_`, `gho_`, `ghs_`, `ghu_`, `github_pat_`), `sk-ant-` keys,
  `sk-` keys of 20 or more characters, `AKIA` access key ids,
  `Authorization:`/`Bearer` values, any key whose name contains `token`,
  `password`, `passwd`, `secret`, `api_key`/`api-key`/`apikey` or
  `access_key` (so `GITHUB_TOKEN=`, `AWS_SECRET_ACCESS_KEY=`, JSON
  `"token": "…"` and `x-api-key:` are all covered), PEM private keys
  (including a key cut off by the excerpt), JWTs, Slack `xox?-` tokens,
  Google `AIza` API keys, Azure `AccountKey=` values, SAS `sig=` values,
  and user info in URLs. Environment and `RECIPE_VAR_*` values are never
  written.
- Only records and evidence are redacted. The full standard output of a
  failed attempt still goes to the step log, as it did before step recovery.
- The report and the outcome line never print `output_excerpt`. Text
  fields reject control and bidi characters, which blocks terminal escape
  injection and reordered text.
- The agent's reply is treated as untrusted: closed set of choices,
  alternatives by name only, owners only from trusted values, and recurrence
  computed by the driver.
- Evidence files are audit material. Do not attach them to pull requests as
  they are; they hold redacted but still raw command output.
- **Same-user limit.** The agent runs as the same user as the driver. It is
  told not to write under the recovery directory, and the driver compares
  every file in the directory before and after the agent runs, so a changed
  or deleted record of any step becomes a validator handoff. The agent can
  still change files outside the directory, such as the worktree: the limits
  in the recovery prompt are instructions, not enforcement. For that reason
  `step-20b-push-cleanup`, which commits and pushes the worktree, runs with
  `--no-repair`, so no unreviewed repair is ever pushed by its retry.
- **Exit status.** Failures at `step-20b-push-cleanup`, `step-21-pr-ready`
  and `validate-agentic-finalization` now end the run with exit 81 (a
  recorded handoff) instead of 1.

## Runner gaps

These recipe runner limits shape the design. They need changes in the runner
repository and are out of scope here:

1. A step has no `on_failure` hook that calls a recovery step.
2. A later step cannot read a failed step's output, standard error or exit
   status. Hence `--already-failed` and `unavailable_reason`.
3. `hooks.on_error` gets no step id, error or exit code, has a fixed 30 s
   timeout, and cannot change what runs next.
4. A step cannot be re-run by id, and the runner has no jumps. Hence the retry
   loop lives in the Rust driver.
5. `recovery_on_failure` uses an attempt counter (`MAX_RECOVERY_ATTEMPTS = 2`)
   and string matching, and records nothing.

## Tests

All files are modules of the `cli_integration` test binary, in
`crates/amplihack-cli/tests/`. Shared fixtures are in
`tests/support/step_recovery.rs` and `tests/support/step_recovery_check.rs`.

| File | Covers |
|---|---|
| `issue_2539_step_recovery_driver.rs` | Refused bundle copy → handoff; unset variable → `retry_after_repair` succeeds; GraphQL head read refused → REST `alternative_step`; `no_scoped_pr` with the PR present → `continue_recorded`, or a handoff to the caller's owner |
| `issue_2539_step_recovery_policy.rs` | Success passes output through with no record; exit 79 → policy handoff and exit 79; exit 81 → passed on, no agent; SIGTERM → exit 143 with no agent; `--already-failed` records `unavailable_reason`; a failed alternative recovers under `<id>:alt:<name>` |
| `issue_2539_step_recovery_recurrence.rs` | Recurrence with the agent choosing handoff, choosing retry (refused by check 3), and repeating an alternative (refused by check 7); a recurrence across two `run` calls sharing one directory (loop rounds); a different failure is not a recurrence; an existing handoff starts no agent |
| `issue_2539_step_recovery_validation.rs` | Invalid replies → validator handoff; the agent's reason and the caller's owner kept; untrusted owner; `--essential`; only the final reply line counts; nested recovery; record tampering; token redaction |
| `issue_2539_step_recovery_report.rs` | `record`; `report` exits 81 when a handoff is followed by a `continue_recorded` for the same step; handoff order; invalid or unsafe record files → exit 2 |
| `issue_2539_step_recovery_init.rs` | `init` with and without `--dir`; relative `XDG_STATE_HOME`; an unsafe `--dir` disables recovery (empty value, warning, exit 0); unsafe directory, relative `--dir` and bad step ids (including `__alt__`) make `run` exit 2 |
| `issue_2539_step_recovery_stop.rs` | The real `step-21-pr-ready` and crusty-round `step-00-review-dir-guard` commands, run through the real runner: a handoff ends the recipe with 81 and the next step never runs; a `--guard` step refuses `retry_after_repair` (exit 81); `--stop-on-handoff` keeps exit 0 for `continue_recorded`; init under a `HOME` whose `.local` is a symlink disables recovery and the wrapped step runs unwrapped |
| `issue_2539_step_recovery_recipes.rs` | Every converted recipe and `step-failure-recovery.yaml` loads and validates; each converted step runs through the driver with its own id; `autodrive-build` step-02/02r/03 wiring; `workflow-finalize` reports after validation and before `workflow-complete`, and its report step exits 81 on a handoff |
| `issue_2539_step_recovery_fixture.rs` | A fixture recipe using both opt-in forms, run by path from a fixture directory with the real runner: recovered steps finish, a handoff skips dependent steps and ends at the report |
| `issue_2539_step_recovery_loop.rs` | `autodrive_loop.sh` with a fake round that records a handoff: the loop exits 81 after that round, starts no further round and no evaluator, also when the round itself exited 81; invalid records stop it with exit 2 |

Unit tests for the signature, step-id rules, redaction and record invariants
are in `crates/amplihack-cli/src/commands/orch/step_recovery/tests.rs`; the
review fixes (token forms, the `__alt__` clash, where the reply is read from,
the output limit, quoted evidence, guard choices, owner and bidi checks) are
in `tests_review.rs` next to it.

Each test runs with a temporary `HOME`, `TMPDIR`, `XDG_STATE_HOME` and
`AMPLIHACK_HOME`, clears inherited `RECIPE_VAR_*` and `REPO_PATH`, sets
`AMPLIHACK_AGENT_BINARY`, uses fake `gh` and guard tools, and checks that a
valid record exists and the run ended in exit 0 or exit 81 with its record.

In the driver tests the agent is played by a stub recipe runner
(`RECIPE_RUNNER_RS_PATH`) that replays a scripted reply, so they do not need
`recipe-runner-rs` installed. The fixture-recipe tests need the real runner
(`RECIPE_RUNNER_QA_BINARY`, else `recipe-runner-rs` on `PATH`); without one
they say so and return, unless `AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER` is set,
when they fail. The loop tests need `jq`, and fail without it under the same
variable.

Run them with:

```bash
cargo test -p amplihack-cli --test cli_integration issue_2539_step_recovery
cargo test -p amplihack-cli --lib step_recovery
```

## Related references

- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md) — the narrower
  repair loop for rejected git hooks, which this mechanism does not replace
- [Recipe Check Exit Status](./recipe-check-exit-status.md) — how check steps
  report real exit codes
- [Auto Drive To Merge](./auto-drive-to-merge.md) — the recipe whose loops
  use recovery first
- [Default Workflow Agentic Finalization](./default-workflow-agentic-finalization.md)
  — the finalization that `validate-agentic-finalization` checks
