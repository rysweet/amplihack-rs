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
- [Portability evidence](#portability-evidence)

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
| Creation fails and the scoped lookup finds an eligible open PR | Exit zero with `existing-open-pr` and the adopted identity. |
| Creation succeeds but returns an invalid identity | Nonzero exit with `FAILED_PR_IDENTITY`, empty `pr_url` and `pr_number`, and a fixed result-parsing diagnostic. |
| Creation fails and the scoped lookup finds no eligible open PR | The original nonzero status, `FAILED_PR_CREATE`, legacy state `create-pr-failed`, and `terminal_status=failure`. |

## Created PR identity

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
discards all partial output. A body-file or private-capture setup failure
reports its own operation and status. A failed collision lookup can add context
while the original creation evidence remains available.

The publisher interprets the create subprocess's private stdout capture using
its wait status. The capture holds the candidate URL on success or one safe
diagnostic on failure. The retry helper emits that diagnostic after cleaning
its attempt captures. The publisher reports the operation and observed exit
status when the failure capture is empty. Retry warnings use stderr, and the
shared helper retains its regular output contract for other callers.

The publisher owns the outer captures. On exit and catchable cancellation it
reaps its child before unlinking those captures. The retry helper owns its
per-attempt captures, restores caller traps, and preserves the failure status.
Captures use `mktemp` with an explicit template under the selected temporary
directory and a restrictive umask.

## Portability evidence

The shell path targets macOS Bash 3.2 and BSD utilities.

- **Linux with Bash 3.2.** The publishing fixtures run with GNU Bash 3.2.0.
  The delivery-input helper stores its existing URL pattern in a variable
  because Bash 3.2 rejects its inline grouping syntax before publishing can run.
- **Apple `mktemp` behavior reproduced on Linux.** The Apple utility source was
  compiled on Linux x86_64 with an adapter for the Darwin `confstr` lookup.
  With a writable `TMPDIR` and a missing Darwin temp directory, an unqualified
  `mktemp` failed, while a positional template under `TMPDIR` succeeded.
  The old publisher helper failed in that reproducer. The corrected helper
  returned the fixture's PR URL under Bash 3.2.0. The publisher, retry helper,
  and body writer use explicit templates. The
  [directory-precedence fixture](../../tests/integration/issue_2306_publish/recovery.rs)
  exercises the body writer's scratch files and cleanup.
  [Apple `mktemp` source](https://github.com/apple-oss-distributions/shell_cmds/blob/shell_cmds-278/mktemp/mktemp.c#L94-L115)
- **Native macOS coverage.** Native macOS execution of the publishing fixtures
  and reproducer remains unverified.
- **Historical cause.** The failing commands in software-factory #180, #122,
  #109, and #114 remain unidentified. The reproduced directory-precedence defect
  establishes a portability problem. Attribution of those four failures remains
  unverified.

See [Read a draft PR publishing result](../tutorials/read-draft-pr-publishing-results.md)
for examples based on the publishing fixtures.
