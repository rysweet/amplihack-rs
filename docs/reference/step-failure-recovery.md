---
title: Step Failure Recovery Reference
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
---

# Step Failure Recovery Reference

A recipe step that opts in no longer ends the run when it fails. An agent
reads the failure and chooses one of four ways forward; the driver validates
the choice and appends it to a record. Only a recorded handoff can end the
run, always with exit 81 (issue #2539). Recovery has no timers, no waits and
no attempt counter.

> **Internal command.** `amplihack orch helper step-recovery` is called by
> bundle recipes. It is not a user-facing command and may change with the
> recipes that call it.

## Contents

- [Flow](#flow)
- [Where it applies](#where-it-applies)
- [The four choices](#the-four-choices)
- [Opting a step in](#opting-a-step-in)
- [Commands](#commands)
- [Evidence and agent reply](#evidence-and-agent-reply)
- [Validation](#validation)
- [Failure signature and recurrence](#failure-signature-and-recurrence)
- [Record format](#record-format)
- [How a handoff ends the run](#how-a-handoff-ends-the-run)
- [Recovery directory](#recovery-directory)
- [Security](#security)
- [Tests](#tests)
- [Related references](#related-references)

## Flow

```
step command ──exit 0──▶ output passed through unchanged, no record
     ├─exit 79 or signal ────────────▶ policy handoff record; 79 or 128+n passed on
     ├─exit 81 with a handoff on record ▶ passed on (a nested run handed off)
     ├─a handoff already recorded ───▶ validator handoff, no agent
     └─any other failure
           ▼
   evidence file ─▶ recipe step-failure-recovery ─▶ one {"step_recovery":{…}} line
           ▼
   driver validates the reply, computes recurrence, appends a record
           ├─ retry_after_repair ─▶ re-run the command once
           ├─ alternative_step ──▶ run the declared alternative
           ├─ continue_recorded ─▶ outcome line, exit 0
           └─ handoff ───────────▶ outcome line; exit 81 with --stop-on-handoff
                                     or --guard, else exit 0 (a report step ends the run)
```

A failed retry or alternative goes back to recovery with the full history.
Each pass appends one record. Exit 81 from the command with no handoff on
record is an ordinary failure.

## Where it applies

| Recipe | Step | Flags |
|---|---|---|
| `autodrive-build` | `step-02-default-workflow` (sub-recipe), recovered by `step-02r-default-workflow-recovery` | `--already-failed --essential` |
| `autodrive-crusty-loop` | `step-02-crusty-loop` | `--essential --stop-on-handoff` |
| `autodrive-crusty-loop` | `step-03-record-crusty-phase` (agent), recovered by `step-03r-record-crusty-phase-recovery` | `--essential` |
| `autodrive-crusty-round` | `step-00-review-dir-guard`, `step-02b-review-dir-unchanged` | `--guard` |
| `autodrive-merge-round` | `step-00-review-dir-guard`, `step-02b-review-dir-unchanged` | `--guard` |
| `autodrive-merge-round` | `step-00-merge-ready-files` | `--essential --stop-on-handoff` |
| `autodrive-merge-loop` | `step-02-merge-ready-loop` | `--essential --stop-on-handoff` |
| `autodrive-merge-loop` | `step-03-finalize` (sub-recipe), recovered by `step-03r-finalize-recovery` | `--already-failed --essential` |
| `workflow-finalize` | `step-20b-push-cleanup` | `--essential --stop-on-handoff --no-repair` |
| `workflow-finalize` | `step-21-pr-ready` | `--essential --stop-on-handoff` |
| `workflow-finalize` | `validate-agentic-finalization` | `--guard` |

Report steps: `workflow-finalize` `step-recovery-report` (before
`workflow-complete`), `auto-drive-to-merge` `autodrive-report`, and
`autodrive_loop.sh` before the first round and after each round.

`workflow-worktree` `step-04-setup-worktree` writes the build's branch to
`build-branch.txt` in the recovery directory. A retried build passes it as
`existing_branch`, so the retry continues the first attempt's branch and PR.

## The four choices

| Choice | Agent supplies | Driver | Not offered |
|---|---|---|---|
| `retry_after_repair` | `reason`, `repair` | Re-runs the same command once. The agent made the repair itself. | `--guard`, `--no-repair`, a recurrence |
| `alternative_step` | `reason`, `alternative` | Runs the declared alternative. Its failure recovers as `<id>:alt:<name>`, with no alternatives. | no alternatives declared |
| `continue_recorded` | `reason` | Prints the outcome line, exits 0. | `--essential`, `--guard` |
| `handoff` | `reason`, `owner`, `needs` | Prints the outcome line; exits 81 with `--stop-on-handoff` or `--guard`, else 0. | never |

`owner` must be one of the evidence file's `owner_roles`, the caller's
`--owner`, or `default_owner` (the caller's `--owner`, else `run-requester`).

## Opting a step in

A bash step keeps its body unchanged in a quoted here-document and runs it
through the driver. Values come from quoted `$RECIPE_VAR_*` variables, never
from `{{…}}`:

```yaml
    command: |
      IFS= read -r -d '' STEP_BODY <<'STEP_RECOVERY_BODY' || :
      ...the step's original commands, unchanged...
      STEP_RECOVERY_BODY
      [ -n "${RECIPE_VAR_step_recovery_dir:-}" ] || exec bash -c "$STEP_BODY" step-21-pr-ready
      exec amplihack orch helper step-recovery run --step-id step-21-pr-ready --essential --stop-on-handoff \
        --dir "$RECIPE_VAR_step_recovery_dir" \
        --goal "Mark PR ${RECIPE_VAR_pr_number:-unknown} ready for review" \
        -- bash -c "$STEP_BODY" step-21-pr-ready
```

Without a recovery directory the body runs in bash as before #2539. That
fallback stays in each step, not in the driver, so a step with recovery off
needs no `amplihack` binary on `PATH`, exactly as before. A recovery step
after a sub-recipe instead prints an error and exits 1, since there is
nothing to run.

A sub-recipe or agent step gets `continue_on_error: true` and an `output:`
key, and a bash step after it with `condition: "not <output>"` recovers it:

- **Sub-recipe:** the recovery step runs with `--already-failed` and, after
  `--`, a command that runs the same sub-recipe again. The condition is exact:
  the runner sets a successful recipe step's output to its result summary
  (`Recipe '<name>': SUCCESS …`), never to an empty value, so a sub-recipe
  that succeeded is never run again.
- **Agent:** an agent step can succeed with empty output, so its recovery step
  first runs a read-only check of what the agent should have done. After a
  success the check passes and nothing else happens; after a failure it fails
  and goes to recovery.

A step whose output later steps parse, or that comes before an outward
action, adds `--stop-on-handoff`. Without it, steps that need the real
result test `'step_recovery' not in <output>`.

## Commands

All under `amplihack orch helper step-recovery`. Any error (bad arguments,
an unsafe directory or record file, a record that cannot be appended) exits
2.

### `init [--dir <path>] [--created-by <recipe>]`

With `--dir`, checks that the path is a private directory (owned by the
user, mode `0700`, not a symlink) and prints it. Without it, creates
`<state>/amplihack/step-recovery/<32 hex>` (`<state>` is an absolute
`$XDG_STATE_HOME`, else `$HOME/.local/state`) and prints it; with
`--created-by`, it notes that recipe in `.created-by` in the new directory.
On any error it
warns `[step-recovery] WARNING: recovery disabled: <error>` on stderr,
prints an empty line and exits 0: recovery is off for the run and wrapped
steps run unwrapped.

### `run`

```
run --step-id <id> --dir <dir> --goal <text> [--owner <v>]
    [--alternative <name>=<command>]... [--essential] [--stop-on-handoff]
    [--no-repair] [--guard] [--already-failed] [--recipe-dir <dir>] -- <command>...
```

| Flag | Meaning |
|---|---|
| `--step-id` | `^[A-Za-z0-9._-]{1,128}$`; `.`, `..`, `:` and `__alt__` are refused |
| `--dir` | Absolute recovery directory; a relative path is refused |
| `--owner` | Default handoff owner, `^[A-Za-z0-9._@/-]{1,128}$` |
| `--alternative` | Named alternative, repeatable; name `^[A-Za-z0-9_-]{1,64}$`; command split on whitespace, no shell |
| `--essential` | `continue_recorded` not offered |
| `--stop-on-handoff` | A handoff exits 81 |
| `--no-repair` | `retry_after_repair` not offered (a step that publishes the worktree) |
| `--guard` | Only `handoff` and declared alternatives; implies the three flags above |
| `--already-failed` | Go straight to recovery; the record has `unavailable_reason` |
| `--recipe-dir` | Run `<dir>/step-failure-recovery.yaml` (tests) |

| Exit | Meaning |
|---|---|
| 0 | Success (output passed through), `continue_recorded`, or `handoff` without `--stop-on-handoff`/`--guard` |
| 79 | The command exited 79; policy handoff recorded |
| 81 | Handoff under `--stop-on-handoff`/`--guard`, or a nested run's recorded handoff passed on |
| 128+n | Killed by signal n; policy handoff recorded |
| 2 | Driver error |

### `record --dir <dir> --step-id <id> --reason <t> --owner <v> --needs <t>`

Appends a `handoff` record with `source: "validator"`, for missing evidence
when no step failed.

### `report --dir <dir> [--remove-if-created-by <recipe>]`

Exit 81 when any record in any `*.jsonl` file is a handoff, else 0. A later
record never hides an earlier handoff. Prints
`{"step_recovery_handoffs":[…]}` on stdout (step id, owner, needs, reason,
source, record path) and the same on stderr; never `output_excerpt`.

`--remove-if-created-by <recipe>`: when the directory holds no record file
and its `.created-by` names `<recipe>`, removes `build-branch.txt`,
`.created-by` and the directory. Any other file keeps it, and so does a
directory that recipe was given by its caller.

## Evidence and agent reply

The driver runs `amplihack recipe run step-failure-recovery -c
step_recovery_evidence=<path>` with `AMPLIHACK_STEP_RECOVERY_ACTIVE=1`. The
prompt says the evidence is untrusted data, and that the agent must not make
GitHub writes, bypass hooks, read credentials, change the review directory,
the main checkout, guard or validator scripts or their evidence, write under
the recovery directory, or start another orchestration.

Evidence (`evidence-<id>-<n>-<8 hex>.json`, mode `0600`, schema 1):
`untrusted_notice`, `step_id`, `goal`, `failure`, `allowed_choices`,
`alternatives` (names only), `prior_records`, `recurrence`, `default_owner`,
`owner_roles`, `guard`. In the excerpt, a `"step_recovery"` key becomes
`"step_recovery_quoted"`.

The reply is the final non-empty line of the last step's output, below
1 MiB:

```json
{"step_recovery":{"choice":"handoff","reason":"bundle copy refused: unsafe modes","owner":"run-requester","needs":"fix the staged bundle directory, then rerun"}}
```

| Field | Required for | Limits |
|---|---|---|
| `choice` | all | one of the four |
| `reason` | all | 1–1024 bytes |
| `repair` | `retry_after_repair` | 1–1024 bytes |
| `alternative` | `alternative_step` | a declared name |
| `owner` | `handoff` | a trusted value |
| `needs` | `handoff` | 1–1024 bytes |

Other fields must be absent. Text fields refuse control characters and
U+202A–U+202E, U+2066–U+2069.

## Validation

In order: a reply exists and parses; no unknown fields; `choice` is allowed;
text limits; required fields present and no others; alternative declared and
owner trusted; on a recurrence, not the same alternative again. The first
failure becomes a `handoff` with `source: "validator"`, keeping the agent's
reason.

Also validator handoffs, with no agent started: recovery inside recovery
(`AMPLIHACK_STEP_RECOVERY_ACTIVE` set), and a directory that already holds a
handoff. The driver also takes the name, length and SHA-256 of every
non-dot file in the directory before the agent starts and compares after;
any change gives a validator handoff.

## Failure signature and recurrence

```
exit=<n|none>;[class=<c>;]tail=<16 hex>
```

`tail` hashes the redacted last 40 lines, lowercased and trimmed, with hex
ids of 7 or more characters (holding a letter) replaced by `H` and digit
runs by `0`. So commit ids, PIDs and timestamps do not change it.

A failure is a recurrence when an earlier record of the same step id, anywhere
in the run, has the same signature and chose `retry_after_repair` or
`alternative_step`. A step that fails with a new signature each time can keep
going; each pass is a new, different failure.

## Record format

`<dir>/<step id with :alt: as __alt__>.jsonl`, one JSON object per line:

```json
{"schema_version":1,"step_id":"step-21-pr-ready","failure":{"exit_code":1,"signature":"exit=1;tail=3f9a0c51d2e87b46","output_excerpt":"no_scoped_pr: …"},"choice":"continue_recorded","reason":"…","recurrence":false,"source":"agent","recorded_at":"2026-11-12T21:04:33Z"}
```

`failure` holds exactly one of `exit_code`, `signal`, `unavailable_reason`,
plus `signature` and `output_excerpt` (redacted, last 40 lines, at most 2000
characters). `repair`, `alternative`, `owner` and `needs` appear only with
their choice. `source` is `agent`, `validator` or `policy`. Unknown fields
are refused, and a record whose `step_id` does not match its file is invalid.

On `continue_recorded` or `handoff`, `run` prints a text line and then the
outcome line `{"step_recovery":{"choice":…,"step_id":…,"record":…}}`; the
text line keeps the runner from parsing the output as an object.

## How a handoff ends the run

A step with `--stop-on-handoff` or `--guard` exits 81, the runner stops, and
`amplihack recipe run` exits 81. Otherwise a report ends the run:

- `autodrive_loop.sh` runs `report` before the first round and after each
  round. On 81 it escalates `STEP_RECOVERY_HANDOFF` and exits 81; on 2,
  `INVALID_RECOVERY_RECORDS`.
- `auto-drive-to-merge` `autodrive-report` reports before any other exit
  path, and records a validator handoff when finalization evidence is
  missing.
- `workflow-finalize` `step-recovery-report` runs after
  `validate-agentic-finalization` and before `workflow-complete`, so a
  handed-off run never reaches `workflow-complete`.

Behaviour change: a failure at `step-20b-push-cleanup`, `step-21-pr-ready` or
`validate-agentic-finalization` now ends the run with exit 81 and a record,
where it used to exit 1.

## Recovery directory

One directory per top-level run, shared with every child recipe and loop
round. Auto-drive creates it at `step-recovery-init`; standalone
`workflow-finalize` creates its own; a caller's directory is checked with
`init --dir`. It is not in auto-drive's per-PR state, so a new drive starts
with no handoffs.

`auto-drive-to-merge` and `workflow-finalize` create it with
`init --created-by <recipe>` and end with `report --remove-if-created-by
<recipe>`, so a run with no record leaves nothing behind. A directory with records, including every handed-off run,
is kept for the operator, who removes it. Nothing removes directories by
age.

When `init` disabled recovery, `workflow-finalize`'s `step_recovery_report`
output is `{"step_recovery":"disabled",…}` and auto-drive's output shows an
empty recovery directory.

Directory mode `0700`; record, evidence and spool files `0600`.

## Security

- Files go through `secure_fs`: opened with `O_NOFOLLOW` relative to a
  checked directory; any file not owned by the user or with group or other
  bits is refused, never repaired.
- Redacted before an excerpt or signature is computed: GitHub tokens
  (`ghp_`, `gho_`, `ghs_`, `ghu_`, `github_pat_`), `sk-ant-` and `sk-` keys,
  `AKIA` key ids, `Authorization:`/`Bearer` values, keys whose name holds
  `token`, `password`, `passwd`, `secret`, `api_key`/`api-key`/`apikey` or
  `access_key`, PEM private keys (also when cut off), JWTs, Slack `xox?-`
  tokens, Google `AIza` keys, Azure `AccountKey=`, SAS `sig=` and user info
  in URLs.
- Only records and evidence are redacted. Step logs are unchanged: a failed
  attempt's full stdout is copied to stderr under
  `[step-recovery] stdout of failed attempt <id>:`, as the step log held it
  before.
- The agent runs as the same user. Its limits outside the recovery
  directory are prompt instructions; for that reason `step-20b-push-cleanup`
  runs with `--no-repair`, so its retry never pushes an unreviewed repair.
- Evidence files hold redacted but raw output; do not attach them to pull
  requests.

## Tests

Integration tests are modules of `cli_integration` in
`crates/amplihack-cli/tests/issue_2539_step_recovery_*.rs`; unit tests are in
`step_recovery/tests.rs` and `tests_review.rs`. The agent is a stub runner
replaying scripted replies. Tests that need the real `recipe-runner-rs`
skip without it, and fail without it when
`AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER` is set (as in CI).

```bash
AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER=1 cargo test -p amplihack-cli --test cli_integration issue_2539_step_recovery
cargo test -p amplihack-cli --lib step_recovery
```

## Related references

- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md)
- [Recipe Check Exit Status](./recipe-check-exit-status.md)
- [Auto Drive To Merge](./auto-drive-to-merge.md)
- [Default Workflow Agentic Finalization](./default-workflow-agentic-finalization.md)
