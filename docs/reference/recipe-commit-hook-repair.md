---
title: Recipe Commit Hook Repair Reference
last_updated: 2026-10-25
review_schedule: quarterly
owner: workflow-team
---

# Recipe Commit Hook Repair Reference

When a git `pre-commit`, `commit-msg` or `pre-push` hook rejects a commit or
push made by a recipe-owned step, the step no longer ends the run. The hook's
output goes to a fixing agent, the step re-stages and retries the same git
operation, and the [loop-health evaluator](./loop-health-evaluator.md) decides
after each failed retry whether the loop is still making progress
(issue #1671).

Hooks are never bypassed and never weakened. `--no-verify`, `SKIP=`,
`core.hooksPath` overrides and edits to hook configuration are forbidden, and
a round in which the agent changes hook configuration ends the step.

This page concerns **git hooks**. It has nothing to do with Claude Code hooks
(`settings.json` `hooks`), which are described in
[Hook Specifications](./hook-specifications.md).

## Contents

- [Where it applies](#where-it-applies)
- [Launcher environment and agent environment](#launcher-environment-and-agent-environment)
- [Classifying a failure](#classifying-a-failure)
- [The repair loop](#the-repair-loop)
- [Evidence](#evidence)
- [The fixing recipe](#the-fixing-recipe)
- [Agent output contract](#agent-output-contract)
- [Refusal rules](#refusal-rules)
- [Staging rules](#staging-rules)
- [`hook_repair_run`](#hook_repair_run)
- [`stage_workflow_changes`](#stage_workflow_changes)
- [Result line](#result-line)
- [Step-15 reason grammar](#step-15-reason-grammar)
- [Exit code 79 is terminal](#exit-code-79-is-terminal)
- [Environment variables](#environment-variables)
- [Residual risks and detection limits](#residual-risks-and-detection-limits)
- [Tests](#tests)
- [Related references](#related-references)

## Where it applies

| Recipe | Step | Operations repaired |
|---|---|---|
| `workflow-tdd.yaml` | `checkpoint-after-implementation` | commit |
| `workflow-refactor-review.yaml` | `checkpoint-after-review-feedback` | commit |
| `workflow-publish.yaml` | `step-15-commit-push` | commit and push |

Each of these steps sources two helpers from `amplifier-bundle/tools/`. In
`checkpoint-after-implementation` the first add is the scoped add from #1672,
which stages only the paths captured before validation; its untracked paths
pass through `workflow_stage_filter`, the same exclusions as
`stage_workflow_changes`, which is then used for the agent's changes between
rounds. The staging helper is loaded before staging. The driver and the git
identity helper are loaded only once something is staged, so a checkpoint
with nothing to commit does not fail because either is missing.

| File | Function | Replaces |
|---|---|---|
| `workflow_stage_changes.sh` | `stage_workflow_changes` | `git add -A` |
| `workflow_hook_repair.sh` | `hook_repair_run` | a bare guarded commit, or `git push` |

The commit itself still goes through `commit_with_pre_commit_guard` from
`workflow_checkpoint_commit.sh`, so `PRE_COMMIT_ALLOW_NO_CONFIG=1` handling is
unchanged. Step-15's commit, which used a plain `git commit -m`, now goes
through the same guarded wrapper.

A checkpoint call site reads:

```bash
stage_workflow_changes
hook_repair_run commit checkpoint-after-implementation \
  --output "$commit_output_file" \
  -- commit_with_pre_commit_guard -m "$msg"
```

Step-15's push reads:

```bash
hook_repair_run push step-15-commit-push \
  --fix-message "fix: address {hook_ids} pre-push rejection${ISSUE_NUMBER:+ (#$ISSUE_NUMBER)}" \
  --output "$push_stderr_file" \
  -- git push
```

The checks that run before step-15's push are untouched: the #978 divergence
check, the no-upstream check and the already-pushed check. After a push that
succeeds, `verify_pushed_ref.sh` (#1269) still runs.

## Launcher environment and agent environment

Recipe-owned commit and push steps are bash steps. They run with the
environment of the process that launched `amplihack recipe run`, not with the
environment of any agent that ran earlier in the recipe. Variables an agent
sets in its own shell, such as a toolchain on `PATH`, a virtualenv, or a
token that a hook needs, do not exist when the hook runs.

A hook that depends on such a variable fails in the bash step even though it
passed when the agent ran it. No code change fixes that. The fixing agent is
told to recognise this case, classify it as `environment`, report
`not_fixable_in_code` and change nothing. With no diff and identical hook
output, the evaluator returns `STUCK` and the step ends with the agent's
classification in the evidence. See
[Agent output contract](#agent-output-contract).

Running commit and push steps under the agent's environment is outside the
scope of this mechanism and is tracked in issue #1673.

## Classifying a failure

`hook_repair_run` repairs only failures it can attribute to a hook. Any other
failure returns the command's own exit code, and the call site's existing
failure path runs exactly as before. A false negative therefore restores the
behaviour from before #1671.

The hooks directory is resolved with `git rev-parse --git-path hooks`, which
honours `core.hooksPath`, and made absolute. Git configuration is only read,
never written.

**Commit.** All of the following must hold:

- the command exits non-zero
- something is staged (`git diff --cached --quiet` fails)
- an executable `pre-commit` or `commit-msg` hook exists in the hooks directory

**Push.** All of the following must hold:

- the command exits non-zero
- an executable `pre-push` hook exists in the hooks directory
- the output contains `failed to push some refs`
- none of git's own lines contains any of the excluded markers below

Git's own lines are the output lines that begin, after any leading
whitespace, with `fatal:`, `error:`, `ssh:`, `remote:`, `! ` or `To `. Only
those lines are searched for the markers, so a pre-push hook whose tests
print `ConnectionError` or `timed out` on a line of their own is still
classed as a hook rejection. The prefix is all that is tested: a hook line
that itself begins with `error:`, `fatal:` or `remote:` and contains a
marker, such as `error: request timed out`, makes the push count as an
ordinary failure. It is then not repaired, which fails safe. On success the
push output is printed to the step log.

| Excluded marker | Meaning |
|---|---|
| `[rejected]` | remote refused the update |
| `[remote rejected]` | server-side hook or protection |
| `non-fast-forward` | upstream moved |
| `fetch first` | upstream moved |
| `Authentication failed` | credentials |
| `Permission denied` | credentials or access |
| `Could not resolve host` | network |
| `unable to access` | network or TLS |
| `Connection refused` | network |
| `Connection reset` | network |
| `timed out` | network |
| `Could not read from remote repository` | network, credentials or access |

A remote-side rejection appears only on git's own lines (`remote:` and the
` ! [remote rejected]` summary), so this rule loses none of the cases the
markers exist for.

**Hook ids.** Lines of the form `- hook id: <id>` (the `pre-commit` framework's
format) are parsed. Only ids matching `^[A-Za-z0-9._-]{1,64}$` are kept, at
most 20, sorted and de-duplicated. If none survive, the id list is `unknown`.
Hook managers other than `pre-commit` (Husky, Lefthook, hand-written hooks)
always produce `unknown`; the loop still runs, but reasons and fix messages
say less.

## The repair loop

The first rejection always goes to the agent. The evaluator is consulted only
after a retry has failed.

```
run operation ──ok──────────────────────────────────────────► status ok, rc 0
     │
     ├─ not a hook rejection ──────────────────────────► status not_hook, rc = command rc
     │
     ▼
┌─ round N ──────────────────────────────────────────────────────────────┐
│ 1. snapshot hook config, git config and HEAD ── fails ─► status stuck  │
│ 2. write round-N.evidence (redacted)                                   │
│ 3. amplihack recipe run hook-rejection-repair.yaml   (the agent)       │
│ 4. stage_workflow_changes                                              │
│ 5. artifact-guard --mode pre-publish ── blocks ─► status stuck         │
│ 6. second snapshot (fails ─► stuck); compare ── changed ─► refused     │
│ 7. push only: commit the agent's fix as a new commit (nested repair)   │
│ 8. retry the same operation ── ok ─────────────────► status ok, rc 0   │
│                              └─ not a hook rejection ► status not_hook │
│ 9. loop-health-evaluator.yaml (loop_health_enforce=false)              │
│      CONTINUE ─► round N+1                                             │
│      STUCK / missing / malformed ─► status stuck                       │
│      DONE ─► status evaluator_done_unresolved                          │
└────────────────────────────────────────────────────────────────────────┘
```

There is no iteration cap. `ROUND` is a label used in evidence file names and
in `loop_history`; termination is the evaluator's decision, as in
`autodrive_loop.sh` (#1337). Rounds run one after another at constant nesting
depth, so a long loop does not climb the session-tree depth ceiling.

A successful git operation is the only success signal. `DONE` while the
operation still fails is treated as failure with the reasoning "evaluator
ended the loop while the hook still rejects".

**Retries.** A commit retry runs the same `commit_with_pre_commit_guard -m
"$msg"` with the same message. A push retry first commits the agent's changes
as a new commit using the `--fix-message` text, then runs the same push
command. The original commit is never amended, so the history shows what the
agent changed. The fix commit goes through the commit path of the same
driver, nested one level only; a fix commit that is itself rejected is
repaired in the same way, and if that inner loop fails the outer step fails
with the inner status.

If the agent produced no change to commit before a push retry, no fix commit
is made and the push is retried as is.

**Artifact Guard after a round.** The step runs `artifact-guard --mode
pre-publish` before the first attempt, but the agent's session writes files
into the worktree after that. Each round therefore runs the guard again on
what `stage_workflow_changes` staged, before the refusal comparison and the
retry. When the guard blocks, its report goes to stderr and the loop ends as
`stuck` with the reasoning "artifact guard blocked the agent's changes"; the
operation is not retried.

### Finding the evaluator

The driver runs the evaluator from the bundle it was loaded from, never from
the worktree under repair: `<bundle>/recipes/loop-health-evaluator.yaml`,
where `<bundle>` is the parent of the driver's own directory
(`amplifier-bundle/tools/workflow_hook_repair.sh` gives
`amplifier-bundle/recipes/`). This is the rule `autodrive_loop.sh` uses.
`HOOK_REPAIR_HEALTH_RECIPE` overrides the path; see
[Environment variables](#environment-variables).

The driver checks the path with `amplihack recipe show` once a failure has
been classified as a hook rejection and before round 1 starts. If it does not
resolve, no agent runs: the status is `stuck`, `HOOK_REPAIR_ROUNDS` is `0`
and the reason is `loop-health-evaluator does not resolve`. A failure that is
not a hook rejection never triggers the check.

### Evaluator inputs

The driver calls `loop-health-evaluator.yaml` with these values and reads the
verdict as `autodrive_loop.sh` does: only the line straight after the last
completed `step-04-enforce-loop-verdict` status line in the run output is
trusted. The recipe runner prints that line as `    Output: LOOP_HEALTH: <verdict> ...`;
after the prefix is removed, the text must match
`^(CONTINUE|DONE|STUCK)( |$)`. A `LOOP_HEALTH:` line anywhere else in the
output, for example one planted in hook output, is ignored. A missing or
malformed verdict, or a non-zero exit other than 79, counts as `STUCK`.

| Input | Value |
|---|---|
| `loop_name` | `hook-repair:<step-id>:<op>` |
| `loop_round_label` | `ROUND`, the round number; a label only |
| `loop_repo_path` | absolute path of the task worktree |
| `loop_agent_working_dir` | `HR_DIR`, which lies outside the worktree (#1528) |
| `loop_health_enforce` | `false` |
| `loop_last_round_output` | the last 16 KiB of the retry's redacted output, passed as a file |
| `loop_findings_current` | sorted hook ids from the retry |
| `loop_findings_previous` | sorted hook ids from the previous attempt |
| `loop_baseline_ref` | HEAD at the start of the round |
| `loop_child_exit_code` | the retry's exit code |
| `loop_round_verdict` | the agent's reported status: `fixed`, `not_fixable_in_code` or `unreported` |
| `loop_clean_token` | `HOOK_PASSED` |
| `loop_history` | the last 20 lines of the round history: one line per round with the round number, output hash, whether the output changed, agent status and diffstat (`unknown` when a tree could not be written); passed as a file |

The two inputs marked "passed as a file" are written to
`HR_DIR/eval-<round>.output` and `HR_DIR/eval-<round>.history` and handed to
the runner with `--context-from-file`, so the hook output never appears on a
command line where `ps` could show it. The other inputs are `-c` arguments.

`loop_clean_token` is `HOOK_PASSED`, a value `loop_round_verdict` never
holds. The evaluator is consulted only after a retry has failed, so no round
it sees is clean, and the token makes `DONE` illegal under the evaluator's
own rules. A round becomes clean only when the git operation succeeds, and
the driver then returns `ok` without calling the evaluator. If the evaluator
returns `DONE` regardless, the status is `evaluator_done_unresolved`.

The agent's evaluator step starts in `HR_DIR`, never in the worktree, as
#1528 requires; it reads the worktree through `loop_repo_path`.

The output hashes in `loop_history` let the evaluator see that consecutive
rounds produced identical output with no diff, which is how environment-class
rejections end. Only the last 20 lines are passed, so the input stays
bounded however long the loop runs; the full history is kept in
`HR_DIR/history`.

## Evidence

Each round writes `round-<n>.evidence` into a private directory, `HR_DIR`:

- created with `mktemp -d` under `umask 077` (mode 700)
- required to lie outside the worktree; otherwise `hook_repair_run` returns 2
- deleted when the status is `ok` or `not_hook`
- kept when the status is `stuck`, `refused`, `evaluator_done_unresolved` or
  `terminal`, and reported in `HOOK_REPAIR_EVIDENCE_DIR`; for every kept
  status except `terminal` it is also reported in the result line (see
  [Result line](#result-line))

The evidence file holds:

| Section | Content |
|---|---|
| `operation` | `commit` or `push` |
| `step` | the step id |
| `exit_code` | the failing command's exit code |
| `hook_ids` | the sanitised id list |
| `staged` | `git diff --cached --name-status` |
| `status` | `git status --porcelain` |
| `output` | the combined stdout and stderr of the command, redacted |

Raw agent logs stay inside `HR_DIR`. Everything that leaves the driver, on
stderr or in the evidence file, is redacted first. Redaction covers:

- credentials embedded in URLs (`https://user:token@host`)
- GitHub tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`)
- `Authorization:` header values and `Bearer` tokens
- `sk-ant-`, `sk-`, `xox[abprs]-` keys, and AWS access key ids (`AKIA`,
  `ASIA` and the other four-letter prefixes)
- GitLab (`glpat-`), Google API (`AIza`) and npm (`npm_`) tokens, and JWTs
- Azure connection-string `AccountKey=` and `SharedAccessKey=` values, and
  SAS `sig=` parameters
- PEM private key blocks
- the `Secret:` and `Match:` lines that gitleaks prints for a finding
- `key=value` and `key: value` pairs, with the value bare or in single or
  double quotes, and JSON- or YAML-style `"key": "value"` pairs, whose key
  contains `token`, `secret`, `password`, `passwd`, `credential`, or
  `api_key`, `access_key` or `private_key` (with `_`, `-` or nothing between
  the words) in any case

Over-redaction is accepted: a path such as `src/token.py:12:` loses its line
number.

## The fixing recipe

`amplifier-bundle/recipes/hook-rejection-repair.yaml` is registered in
`_recipe_manifest.json`. It contains a single agent step and no bash step.

| Field | Value |
|---|---|
| step id | `fix-hook-rejection` |
| agent | `amplihack:pre-commit-diagnostic` |
| `working_dir` | `{{hook_repair_worktree}}` |
| timeout | none (#439) |

Context inputs:

| Input | Value |
|---|---|
| `hook_repair_worktree` | absolute path of the task worktree |
| `hook_repair_evidence_file` | absolute path of `round-<n>.evidence` |
| `hook_repair_operation` | `commit` or `push` |
| `hook_repair_step` | the calling step id |
| `hook_repair_hook_ids` | the sanitised id list, comma-separated |

The hook output reaches the agent only through the evidence file, not through
`-c`, so a large hook report cannot exceed the argument limit. The 16 KiB
output tail given to the evaluator is not passed to this recipe. It will not
be until it is confirmed that the recipe runner renders templates in a single
pass (SR-10), so that hook output containing `{{...}}` cannot be expanded a
second time.

The prompt tells the agent to:

- read the evidence file and fix the cause in the code or artefacts
- run the failing hook locally if that helps, but make no git writes: no
  `commit`, `push`, `stash`, `reset`, `checkout`, `update-ref` or `config`
- never use `--no-verify` or `-n` on commit, `SKIP=`, `core.hooksPath`,
  `git -c` hook overrides, hook allowlists, or suppressions added only to
  silence the hook
- never edit hook configuration (see [Refusal rules](#refusal-rules))
- report `not_fixable_in_code` and change nothing when the cause is the
  environment rather than the repository

The prompt contains no routing-marker strings, so the
`issue_1333_leaf_steps_do_not_orchestrate` guard is not triggered.

## Agent output contract

The agent's last line must be:

```json
{"hook_repair":{"status":"fixed","class":"code","summary":"ran cargo fmt on crates/amplihack-cli/src/lib.rs"}}
```

| Field | Values | Used for |
|---|---|---|
| `status` | `fixed`, `not_fixable_in_code` | `loop_round_verdict`, `loop_history` |
| `class` | `code`, `artefact`, `environment` | evidence only |
| `summary` | free text | evidence only |

Any other `status`, or a missing line, is recorded as `unreported`. `class`
and `summary` never appear in a reason string, a commit message or the
result line.

`not_fixable_in_code` does not stop the loop by itself. The driver still
retries and still asks the evaluator; the evaluator remains the only
authority that ends the loop.

## Refusal rules

Before the agent runs, the driver records a snapshot. After the agent has run
and its changes are staged, it records a second snapshot and compares them.
Any difference ends the step at once with status `refused`, exit 1, no retry
and no evaluator call.

The snapshot records, for each path, its SHA-256, its symlink target (and,
when the link resolves to a file, that file's SHA-256), or its absence. An
edit to the script a symlinked hook points at, such as
`.git/hooks/pre-commit -> ../../scripts/pre-commit.sh`, is therefore a
change:

| Group | Paths |
|---|---|
| pre-commit | `.pre-commit-config.yaml`, `.pre-commit-hooks.yaml` |
| Husky | `.husky/**` |
| Lefthook | `lefthook*.yml`, `lefthook*.yaml`, `.lefthook*` |
| lint-staged | `.lintstagedrc*`, `lint-staged.config.*` |
| Secret-scanner allowlists | `.secrets.baseline`, `.gitleaksignore`, `.gitleaks.toml`, `.trufflehogignore`, `.talismanrc` |
| Hooks directory | the directory itself, resolved afresh for each snapshot so a `core.hooksPath` change at any scope shows, and every file in it, following a symlinked directory |
| Git configuration | hash of `git config --local --list` and `git config --worktree --list`, and a hash of the hook-related entries at every scope (`core.hookspath`, `core.fsmonitor`, `hook.*`, `include.path`, `includeIf.*.path`, `push.*`) from `git config --show-origin --list` |
| HEAD | `git rev-parse HEAD` |

The snapshot is taken at the top of the worktree, whatever the caller's
current directory. The hook-related global entries are filtered so that
unrelated global changes, such as a credential helper set by another session,
do not refuse a round. `package.json` is not in the set, because the agent may
need to change it legitimately; see
[Residual risks](#residual-risks-and-detection-limits).

A snapshot that cannot be written, or is empty, ends the loop at once with
status `stuck` and the reason "refusal snapshot could not be taken". Without
it the comparison could not detect a change. A `find` that fails while
listing the hook files, for example on a directory the step cannot read,
fails the snapshot in the same way, rather than leaving that file out of
both snapshots.

After staging, the lines the round added to the index are searched for
inline secret-scanner suppressions: `pragma: allowlist` (detect-secrets),
`gitleaks:allow`, `trufflehog:ignore`, `talisman:ignore`, `nosemgrep` and
`ggignore`. A match ends the step with status `refused`. When the index
trees before and after the round cannot both be written, the whole staged
diff against HEAD is searched instead, which fails closed. A diff that cannot
be produced at all refuses the round with `scanner-suppression-check-failed`.
A hook file, or the target of a symlinked hook, that cannot be hashed fails
the snapshot, which ends the loop as `stuck`.

Deleting `.pre-commit-config.yaml` counts as a change. Without this rule a
deletion would make `commit_with_pre_commit_guard` set
`PRE_COMMIT_ALLOW_NO_CONFIG=1` and the hook would pass trivially.

| Difference | Reason |
|---|---|
| any configuration, allowlist, hooks-directory or git-config entry | `hook-config-modified: <paths>` |
| HEAD only | `agent-moved-head` |
| an added line with an inline secret-scanner suppression | `scanner-suppression-added: <paths>` |
| the staged diff for the suppression check could not be produced | `scanner-suppression-check-failed` |

Changes to these files made by the task itself before the step ran are not
refused; only changes within a round are compared.

## Staging rules

`stage_workflow_changes` stages:

1. tracked modifications and deletions, with `git add -u` run at the top of
   the worktree (no pathspec, so it also succeeds when nothing is tracked yet)
2. untracked, non-ignored files from
   `git ls-files --others --exclude-standard -z`, except those excluded below

An untracked path is excluded when, at any depth:

- a path component is `node_modules` or `__pycache__`
- a path component ends in `.egg-info`
- the file name is `uv.lock`
- a path component is `target` and that directory holds Cargo's
  `CACHEDIR.TAG`

A `target` directory at the top of the repository is always excluded.
Elsewhere a directory named `target` without `CACHEDIR.TAG`, such as
`src/target/`, is source and is staged.

and also, at the top of the repository only, when the path is under
`.claude/runtime/` or `.amplihack/`. An agent session started in the
worktree, such as a repair round, writes its session log, metrics and code
graph there; outside amplihack-rs those directories are often not ignored.

Excluded paths are printed on stderr, one per line, prefixed
`stage_workflow_changes: skipped `. Files the repository already tracks are
staged when modified, including a tracked `uv.lock` or a tracked file under
`.claude/runtime/` or `.amplihack/`.

Untracked paths are passed with `git --literal-pathspecs add
--pathspec-from-file=<list> --pathspec-file-nul`, so a file named `*` or `:(glob)x`
is treated as a name. On git older than 2.25 the helper falls back to
`xargs -0 git --literal-pathspecs add --`.

The `artifact-guard --mode pre-publish` calls are unchanged.
`stage_workflow_changes` now runs between the call before staging and the
call after it in all three steps, so the second call checks what the helper
staged. A test covers this for each step; see [Tests](#tests).

## `hook_repair_run`

```
hook_repair_run <commit|push> <step-id> [--fix-message <msg>] [--output <file>] -- <cmd...>
```

| Argument | Rule |
|---|---|
| `commit` / `push` | the operation; anything else is a usage error |
| `<step-id>` | must match `^[a-z0-9-]{1,64}$` |
| `--fix-message <msg>` | push only, required for push; one line with no control characters; the literal `{hook_ids}` is replaced by the comma-separated id list |
| `--output <file>` | also writes the final attempt's combined output to `<file>`, so the call site can keep reading it |
| `-- <cmd...>` | the command, run in the current shell so sourced functions such as `commit_with_pre_commit_guard` resolve |

The command is never passed through `eval` or `sh -c`, and
`"$AMPLIHACK_BIN"` is always quoted, with each context value passed as its
own `-c` argument or, for the evaluator's hook output and history, its own
`--context-from-file` argument.

`hook_repair_run` handles the arguments, the first attempt and the loop. Two
helpers in the same file each take one part of a round:
`hook_repair_push_commit` commits the agent's fix as a new commit on push, and
`hook_repair_evaluate` runs the evaluator and reads its verdict. Both work on
`hook_repair_run`'s local variables and are not called from anywhere else.

**Return codes:**

| rc | `HOOK_REPAIR_STATUS` | Meaning |
|---|---|---|
| 0 | `ok` | the operation succeeded, first time or after repair |
| command's rc | `not_hook` | the failure was not a hook rejection |
| 1 | `stuck` | the evaluator returned `STUCK`, or no usable verdict, or Artifact Guard blocked what a round staged |
| 1 | `evaluator_done_unresolved` | the evaluator returned `DONE` while the hook still rejects |
| 1 | `refused` | the agent changed hook configuration or HEAD |
| 79 | `terminal` | a nested recipe run reported orchestration unavailable |
| 2 | `usage` | invalid arguments, or `HR_DIR` inside the worktree |

**Variables set on every return:**

| Variable | Content |
|---|---|
| `HOOK_REPAIR_STATUS` | one of the statuses above |
| `HOOK_REPAIR_HOOK_IDS` | comma-separated sanitised ids, or `unknown` |
| `HOOK_REPAIR_REASON` | `hook-config-modified: <paths>`, `agent-moved-head`, or the evaluator's reasoning, redacted |
| `HOOK_REPAIR_ROUNDS` | number of agent rounds run, `0` when no repair was attempted |
| `HOOK_REPAIR_EVIDENCE_DIR` | the kept `HR_DIR`, or empty when it was deleted |

The function writes nothing to stdout, so step-15 still prints exactly one
`commit_result` line. It installs no traps, keeps all loop state in `local`
variables, and never exports or unsets `SKIP`, `PRE_COMMIT_*`, `GIT_*`,
`HUSKY` or `LEFTHOOK*`.

## `stage_workflow_changes`

```
stage_workflow_changes
```

Takes no arguments. Returns 0 on success or the failing `git` exit code.
Writes nothing to stdout, installs no traps, and running it twice in a row
stages the same set.

## Result line

When the loop ends with status `stuck`, `refused` or
`evaluator_done_unresolved`, the driver prints the redacted output of the
last attempt on stderr, followed by one line:

```json
{"hook_repair_result":{"version":1,"status":"stuck","operation":"commit","step":"checkpoint-after-implementation","hook_ids":["cargo-clippy"],"loop_verdict":"STUCK","reasoning":"two rounds with identical output and no diff","rounds":2,"evidence_dir":"/tmp/hook-repair.Xq3b9T"}}
```

| Field | Content |
|---|---|
| `version` | `1` |
| `status` | `stuck`, `refused` or `evaluator_done_unresolved` |
| `operation` | `commit` or `push` |
| `step` | the step id |
| `hook_ids` | sanitised ids; `["unknown"]` when none parsed |
| `loop_verdict` | `CONTINUE`, `DONE`, `STUCK`, or empty for `refused` |
| `reasoning` | evaluator reasoning or refusal reason, redacted |
| `rounds` | agent rounds run |
| `evidence_dir` | the kept `HR_DIR` |

Control characters, `"` and `\` are stripped from every string field, so the
line is always valid JSON.

The output is printed once. The checkpoint steps print the command's output
themselves only when the driver did not, that is for any status other than
`stuck`, `refused` or `evaluator_done_unresolved`.

On push, a fix commit that fails has its own nested repair, which prints its
own result line. The last `hook_repair_result` line is the result of the
operation; any earlier line belongs to the nested fix commit.

Status `terminal` prints no result line. The driver returns 79 at once, and
the evidence directory is reported only in `HOOK_REPAIR_EVIDENCE_DIR`. See
[Exit code 79 is terminal](#exit-code-79-is-terminal).

## Step-15 reason grammar

Step-15's `commit_result` line keeps its shape. On failure, `pushed` is
`false` and `reason` is one of:

| Reason | When |
|---|---|
| `commit-failed` | commit failed and was not a hook rejection (new; see below) |
| `push-failed` | push failed and was not a hook rejection (unchanged) |
| `orchestration-unavailable` | status `terminal`; the step exits 79 |
| `hook-rejected-<status>:<op>:<ids>` | the repair loop failed; `<status>` is `stuck`, `refused` or `evaluator_done_unresolved`, `<op>` is `commit` or `push`, `<ids>` is the comma-separated id list |

For example: `hook-rejected-stuck:push:cargo-test,check-brick-budget`.

The driver builds these reasons in `hook_repair_step15_reason <op>`, which
prints `orchestration-unavailable` or `hook-rejected-<status>:<op>:<ids>` from
the last `hook_repair_run`, and returns 1 when the failure was not a hook
rejection, so that the step falls back to `commit-failed` or `push-failed`.

`commit-failed` is new. Before #1671 a failed `git commit -m` in step-15 ended
the step through its EXIT trap with the initial reason `aborted`. That case
now reports `commit-failed`. `aborted` remains the reason for any other early
exit that sets no reason of its own.

The reason is built only from the status, the operation and the sanitised
ids. The agent's summary and the hook output never appear in it.

## Exit code 79 is terminal

If any nested `amplihack recipe run` (the fixing recipe or the evaluator)
exits 79, or prints `BLOCKED_TERMINAL orchestration_unavailable`, the driver
returns 79 at once without retrying. The detection is the same as
`autodrive_loop.sh`. Checkpoint steps pass 79 through as their exit code.
Step-15 exits 79 with `reason` `orchestration-unavailable`, and its EXIT trap
still prints `commit_result`. See
[Session-Tree Recursion Control](./session-tree-recursion-control.md).

A planted `BLOCKED_TERMINAL` marker in hook output fails closed: the step
stops rather than continuing.

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `AMPLIHACK_BIN` | `amplihack` | binary used for nested `recipe run` calls |
| `WORKFLOW_HOOK_REPAIR_HELPER` | `workflow_hook_repair.sh` in the directory of `RUNTIME_ARTIFACT_HELPER`, then under `amplifier-bundle/tools/` in `AMPLIHACK_HOME`, `REPO_PATH`, the worktree, `~/.copilot` and `~/.amplihack` | path to the driver |
| `WORKFLOW_STAGE_CHANGES_HELPER` | `workflow_stage_changes.sh` in the same directory as the driver | path to the staging helper |
| `HOOK_REPAIR_ROUND_RECIPE` | `<bundle>/recipes/hook-rejection-repair.yaml` | path to the fixing recipe |
| `HOOK_REPAIR_HEALTH_RECIPE` | `<bundle>/recipes/loop-health-evaluator.yaml` | path to the evaluator |

`<bundle>` is the parent of the driver's directory, as described in
[Finding the evaluator](#finding-the-evaluator). The two recipe overrides were
added by #1671 and are not in its design; they exist so tests can point the
driver at stub recipes. The helper overrides exist so tests can point at the
repository's own copies.

Each of these variables can make the step run a different program. Anyone who
can set them in the step's environment can already run arbitrary code there,
so they give no further access.

## Residual risks and detection limits

The refusal check compares files and HEAD. It guards against an over-eager
agent, not a hostile one, and does not detect:

- an agent that pushes, stashes, runs `reset --soft` or `update-ref` and
  leaves HEAD where it was
- deletion of the offending file, or adding it to `.gitignore`
- linter suppressions inside source files (`#[allow(...)]`, `# noqa`,
  `// eslint-disable`), which the prompt forbids and review is expected to
  catch
- a weakened hook through what it runs rather than its configuration: a
  `repo: local` hook's `entry:` script, `package.json` scripts, the
  pre-commit cache or tools on `PATH`; in-repository changes still appear in
  the fix commit's diff
- a `SKIP` variable already set in the launcher's environment

A pre-push hook that prints its own `error:` or `fatal:` line with an
excluded marker, such as `error: request timed out`, is classed as
`not_hook`, and the step fails as it did before #1671.

The evaluator is the loop's only terminator, so one that keeps returning
`CONTINUE` runs the loop indefinitely. Its prompt includes the redacted hook
output, so planted text can steer it; steering towards `DONE` or `STUCK`
fails closed. This is tracked in #1675.

`terminal_refusal` in `autodrive_loop.sh`, from which
`hook_repair_terminal_refusal` was copied, still misses the marker under
`pipefail` when a large log follows it. #1674 tracks it.

## Tests

| Test | Covers |
|---|---|
| `amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh` | staging and exclusions, classification, id sanitising, redaction, verdict parsing, refusal cases, usage errors, result-line JSON, and the extracted checkpoint and step-15 bodies against a stub `AMPLIHACK_BIN` |
| `tests/integration/issue_1671_hook_rejection_repair_test.rs` | runs the shell test from `cargo test` |

The stub agent is driven by `STUB_AGENT_ACTION`, a space-separated list of
actions consumed one per agent round (the last repeating), each exactly `fix`,
`noop`, `edit-pre-commit-config` or `move-head`; `STUB_AGENT_BODY`, optional
bash run in the worktree after the action, which the refusal cases use; and
`STUB_VERDICTS`, a space-separated list of verdicts returned in order. The
test honours `WORKFLOW_HOOK_REPAIR_HELPER` and `WORKFLOW_STAGE_CHANGES_HELPER`,
and `ISSUE_1671_KEEP_WORK=1` keeps its temporary directory for debugging.

The behaviours below are fixed by this page rather than by the design. Each
has its own case in the shell test:

| Behaviour | Section |
|---|---|
| the fixing recipe receives no output tail; the hook output reaches it only through the evidence file | [The fixing recipe](#the-fixing-recipe) |
| the result line carries `version` and `evidence_dir`, and `terminal` prints none | [Result line](#result-line) |
| a push is retried unchanged, with no fix commit, when the agent changed nothing | [The repair loop](#the-repair-loop) |
| a rejected fix commit is repaired one level deep, and the step fails with the inner status | [The repair loop](#the-repair-loop) |
| `--fix-message` is required for push and `{hook_ids}` is its only placeholder | [`hook_repair_run`](#hook_repair_run) |
| skipped paths are printed with the prefix `stage_workflow_changes: skipped ` | [Staging rules](#staging-rules) |
| `key=value` and `key: value` redaction covers exactly the listed keys | [Evidence](#evidence) |
| the terminal marker and push classification hold under `set -o pipefail` with large output | [Classifying a failure](#classifying-a-failure) |
| a global `core.hooksPath` change, a lint-staged config, or an edit seen from a subdirectory is refused | [Refusal rules](#refusal-rules) |
| a snapshot that cannot be written ends the step as `stuck` | [Refusal rules](#refusal-rules) |
| an edit to a symlinked hook's target, or an added inline secret-scanner suppression, is refused | [Refusal rules](#refusal-rules) |
| a nested `target/` is skipped only with `CACHEDIR.TAG`, and staging works with nothing tracked | [Staging rules](#staging-rules) |
| the evaluator receives at most 20 history lines, and `HR_DIR/history` keeps them all | [Evaluator inputs](#evaluator-inputs) |
| the evaluator's argv holds neither the hook output nor the history | [Evaluator inputs](#evaluator-inputs) |
| a checkpoint with nothing staged succeeds when the driver is missing | [Where it applies](#where-it-applies) |
| a failed checkpoint prints the hook output once | [Result line](#result-line) |
| push markers are matched only on git's own lines | [Classifying a failure](#classifying-a-failure) |
| the evaluator's inputs, including `loop_clean_token` `HOOK_PASSED` and `loop_agent_working_dir` `HR_DIR` | [Evaluator inputs](#evaluator-inputs) |
| an unresolvable evaluator ends the step as `stuck` before any agent runs | [Finding the evaluator](#finding-the-evaluator) |
| a failed step-15 commit reports `commit-failed`, not `aborted` | [Step-15 reason grammar](#step-15-reason-grammar) |
| `artifact-guard --mode pre-publish` still runs after `stage_workflow_changes` in each of the three steps | [Staging rules](#staging-rules) |
| agent runtime state under `.claude/runtime/` and `.amplihack/` is not committed, and the guard runs again after each round | [The repair loop](#the-repair-loop) |

```bash
bash amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh
cargo test --locked -p amplihack --test integration issue_1671_hook_rejection_repair::
```

## Related references

- [Loop-Health Evaluator Reference](./loop-health-evaluator.md): the
  `CONTINUE`/`DONE`/`STUCK` contract the loop relies on
- [Auto Drive To Merge Reference](./auto-drive-to-merge.md): the
  `autodrive_loop.sh` loop this driver is modelled on
- [Recipe Executor Environment](./recipe-executor-environment.md): what
  environment bash steps receive
- [Session-Tree Recursion Control](./session-tree-recursion-control.md): the
  exit-79 contract
- [Publish Lockfile Sync](./workflow-publish-lockfile-sync.md): other
  step-15 behaviour
