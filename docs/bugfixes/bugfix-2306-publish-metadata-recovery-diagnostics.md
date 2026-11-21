---
title: "Draft PR metadata recovery and diagnostics"
description: "GitHub draft PR title and body fallbacks, identity validation, and structured publishing errors."
doc_type: reference
last_updated: 2026-11-20
review_schedule: as-needed
owner: amplihack
issue: 2306
---

# Draft PR metadata recovery and diagnostics

In `workflow-publish`, `step-16-create-draft-pr` creates a GitHub draft PR after
a successful push when the workflow still needs to publish. The step recovers
from optional metadata failures and reports terminal publishing failures to the
recipe runner for recovery.

- [Invocation and configuration](#invocation-and-configuration)
- [Title and body recovery](#title-and-body-recovery)
- [Publishing result](#publishing-result)
- [Created PR identity](#created-pr-identity)
- [Terminal diagnostics and cleanup](#terminal-diagnostics-and-cleanup)
- [Portability](#portability)

## Invocation and configuration

Metadata recovery runs automatically as part of GitHub publishing. The existing
recipe context supplies its inputs.

| Input | Use |
| --- | --- |
| `worktree_setup` / `repo_path` | Locate the checkout. The current branch and origin identify the PR head and repository. |
| `base_branch` | Select the target explicitly and pass it to `gh pr create --base`. An empty value uses remote-base discovery for comparisons and lets GitHub choose the base on creation. |
| `issue_number` | Identify the linked numeric issue. Empty input or a valid `local-…` reference means the PR has no linked issue. Malformed input fails before provider calls. |
| `task_description` | Supply the first nonblank task line as a title source. |
| `local_testing_gate` / `precommit_results` | Supply optional validation evidence for the PR body. |

The GitHub path requires Git, `jq`, an authenticated GitHub CLI, and an eligible
checkout and branch. Protected-branch, strict-delivery, scoped lookup, and
duplicate-claim checks still apply. See the [workflow delivery inputs](../reference/workflow-delivery-inputs.md)
and [publish and finalize outcomes](../operations/workflow-resilience.md).

## Title and body recovery

Normal title selection uses the first usable source in this order.

1. The linked issue title.
2. The first nonblank line of the run's task description.
3. The changed-file template, `Update <scope>` or
   `Update <scope> with <count> changed files`.

See the [merge-base diff and title rules](bugfix-1968-publish-pr-merge-base-title.md)
for how the step chooses scope and counts changed files.

If derivation fails or produces a blank or suffix-only title, the publisher
makes a recovery request with `gh issue view --json title` for the validated
numeric issue. If that request cannot provide a usable string title, the
publisher builds `Update <branch> (#N)` with Bash builtins. For example, the
issue #2306 test branch falls back to
`Update feat/issue-2306-publish (#2306)`. Without a linked issue, the fallback
is `Update <branch>`. Unsafe branch text is replaced with a safe value.
Titles retain the 200-byte limit, including the issue suffix, and preserve
UTF-8 integrity, credential redaction, and reference defusing.

The body retains usable changed-file, commit, and validation summaries. If rich
body construction fails, the publisher writes a short summary and includes
`Refs #N` when a validated numeric issue is available. The normal
diff-based policy governs closing references.

If sanitizing the optional template scope fails, the publisher warns with the
original status before using `branch changes` in the body. For example, status
76 produces `WARNING: optional scope sanitization failed (exit 76)`. The
publisher discards the unsafe scope and any partial sanitizer output. GitHub
receives the body through a private `--body-file`.

## Publishing result

When the publisher emits a result, stdout contains one JSON object. On a
successful step, the runner parses that object into `context.pr_publish_result`
in its JSON report. For a failed step, read
`step_results[].error`, the serialized `StepResult.error`. The publisher's
raw failure JSON is available on stdout when calling the script directly.
If the runner retains that stdout, its JSON report lists the path under
`diagnostic_artifacts`. Human-readable warnings and terminal errors use stderr.
See the [recipe runner revision](../../claude-plugin/recipe-runner.rev).

| Field | Meaning |
| --- | --- |
| `state` / `legacy_state` | Publishing outcome and its compatibility value. |
| `terminal_status` | Terminal classification, including `success` and `failure`. |
| `pr_url` / `pr_number` | Created or adopted PR identity on success. Check the outcome before using these fields. The number is a string. |
| `base_branch` | Explicit target passed on creation, or the base reported for an adopted PR. An empty string is returned after creation when GitHub selects the base. |
| `branch_diff_status` | The result of the branch diff check. |
| `message` | Human-readable outcome or terminal diagnostic. |

| Outcome | Observable result |
| --- | --- |
| A draft is created and its identity is valid | Exit zero, with `FOLLOWUP_CREATED` and `terminal_status=success` for a follow-up draft. |
| The final provider attempt fails and the scoped lookup finds an eligible open PR | Exit zero with `existing-open-pr` and the adopted identity. |
| Creation succeeds but returns an invalid identity | Nonzero exit with `FAILED_PR_IDENTITY`, empty `pr_url` and `pr_number`, and a fixed result-parsing diagnostic. |
| The final provider attempt fails and the scoped lookup finds no eligible open PR | The original nonzero status, `FAILED_PR_CREATE`, legacy state `create-pr-failed`, and `terminal_status=failure`. |
| A local setup or wait fails, or the failure metadata is missing | The original nonzero status, `FAILED_PR_CREATE`, and empty `pr_url` and `pr_number`. The publisher skips the collision lookup. |

## Created PR identity

The publisher passes `--repo github.com/<owner>/<repo>` to draft creation and
best-effort label edits. Both writes use the repository validated from the
checkout's `origin`. The linked issue may belong to another repository.

For a host classified as GitHub, the publisher sets the expected repository
from `origin` before any provider call, with no default and no guard. If
`origin` does not parse into an owner and repository, the publisher stops with
`FAILED_PR_IDENTITY` and no write is attempted. Owner and repository names
contain only ASCII letters, digits, `.`, `_` and `-`.

Other hosts leave this path earlier. Azure DevOps takes its own path. Any other
host finishes with state `non-github` and a success status, and no provider
write is attempted.

A successful `gh pr create` must return a single
`https://github.com/<owner>/<repo>/pull/<number>` URL for the expected
repository. ASCII case folding applies only to the owner and repository
comparison. The literal prefix and final `/pull/` delimiter are
case-sensitive. Using the final delimiter allows a repository named `pull`.

The number must contain only ASCII digits and include at least one nonzero
digit. Leading zeroes are preserved, so `099` is returned as `099`. A foreign
repository, malformed URL, extra text, or invalid number causes
`FAILED_PR_IDENTITY`. The publisher keeps the rejected value private and
returns without making another create attempt.

## Terminal diagnostics and cleanup

If GitHub rejects creation and scoped recovery finds no eligible PR, the
publisher includes the failing operation, original exit status, and sanitized
terminal diagnostic in its JSON `message` field and stderr. The pinned runner
carries that evidence into the failed step's serialized
`StepResult.error`. The [pinned runner test helper](../howto/run-tests-against-the-pinned-recipe-runner.md)
executes the actual Step 16 command. For example, when a transient warning precedes a rejection
with exit 23 and `distinctive final creation rejection` on stderr, the terminal
error includes `gh pr create`, exit 23, and the final rejection. The terminal
diagnostic has its own size limit, independent of the retry warnings.

Provider stderr is redacted before its 500-byte limit is applied. Empty stderr
gets an explicit explanation. A failed sanitizer yields fixed safe text and
discards all partial output. A body-file setup, private capture setup, or retry
wait failure reports its own operation and status. A failed wait stops Step 16
before the next create attempt. A failed collision lookup can add context while
the original creation evidence remains available.

### Diagnostic captures

The publisher creates a private directory with `mktemp -d` under `TMPDIR` (or
`/tmp` when unset) and a `077` umask, then creates two empty files in it under
the same umask. The retry helper writes the terminal diagnostic to exactly one
of them.

| File | Holds | Effect on recovery |
| --- | --- | --- |
| `provider` | The final failed attempt's sanitized GitHub rejection. | A non-empty file starts the scoped collision lookup. |
| `local` | The operation and status of a local failure, for example `body-file write failed (exit 1)`. | The lookup is skipped, because no create request reached GitHub. |

The file name records where the failure happened. No status record, prefix or
delimiter is written to stdout or parsed back. Only the attempt that ends the
run writes a diagnostic, and retry warnings go to stderr, so a warning cannot
displace the final rejection. The message uses `provider` first, then `local`.
If both are empty it reads `terminal diagnostic unavailable` with the observed
exit status. The publisher reads the candidate PR URL from private stdout after
a successful create.

### Cleanup

The publisher owns the stdout and stderr captures and the diagnostics directory.
On exit and on `INT`, `TERM` and `HUP` (statuses 130, 143 and 129) it reaps its
child, removes both files, and removes the directory. The retry helper owns its
per-attempt captures and child, removes them before returning, restores the
caller's traps, and preserves the original failure status. See the
[retry helper contract](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md#terminal-diagnostics-for-gh-pr-create).
The publisher creates its captures, diagnostics directory and body file under
`TMPDIR`, then `/tmp`. The retry helper creates its per-attempt captures under
`GH_RETRY_TMPDIR`, then `TMPDIR`, then `/tmp`. Each is created with `mktemp`
from an explicit template.

## Portability

The shell path targets macOS Bash 3.2 and BSD utilities.

- **Temporary files.** Every `mktemp` call passes an explicit template under
  `TMPDIR`, or under `GH_RETRY_TMPDIR` for the retry helper's captures. An
  unqualified `mktemp` on macOS depends on a per-user Darwin
  directory that can be missing. See the
  [Apple `mktemp` source](https://github.com/apple-oss-distributions/shell_cmds/blob/shell_cmds-278/mktemp/mktemp.c#L94-L115).
  The [directory-precedence fixture](../../tests/integration/issue_2306_publish/recovery.rs)
  exercises the body writer's scratch files and cleanup.
- **Pattern matching.** `delivery_run_base` holds its PR URL pattern in a
  variable rather than writing it inline on the right of `=~`. Other bundled
  tools still use the inline form, so treat this as the shape to copy when
  touching that function, not as a swept rule.
- **Case folding.** The repository comparison uses `nocasematch` under
  `LC_ALL=C`, which Bash 3.2 supports.

See [Read a draft PR publishing result](../tutorials/read-draft-pr-publishing-results.md)
for examples based on the publishing fixtures.
