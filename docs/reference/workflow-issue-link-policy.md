---
title: Workflow issue-link policy
description: Caller policy, publication keywords, recovery and preservation of published GitHub issue references.
doc_type: reference
---

# Workflow issue-link policy

`pr_issue_link` controls whether workflow publication uses `Closes` or `Refs`
for the selected issue. It applies to the step-15 commit message and the PR
body. Issue discovery, change classification and provider-specific issue
formatting retain their existing authority.

## Contents

- [Configuration](#configuration)
- [Classification and publication](#classification-and-publication)
- [Context propagation and recovery](#context-propagation-and-recovery)
- [Published-reference preservation](#published-reference-preservation)
- [Squash messages](#squash-messages)
- [Internal command contract](#internal-command-contract)
- [Failures](#failures)

## Configuration

The recipe input is `-c pr_issue_link=auto|refs|closes`. The accepted
values are exactly the three lowercase strings in this table:

| Policy | Substantive changes | Planning-only changes |
| --- | --- | --- |
| Omitted | `Closes #N` | `Refs #N` |
| `auto` | `Closes #N` | `Refs #N` |
| `refs` | `Refs #N` | `Refs #N` |
| `closes` | `Closes #N` | `Refs #N` |

`#N` denotes the issue already selected by the workflow. `refs` keeps that
issue open when the PR merges. `closes` requests a closing keyword subject to
the classification rule; it cannot turn a planning artefact into completed
work. Ordinary documentation can be substantive when it delivers the fix.

An explicitly empty value is invalid. Whitespace, mixed case, unsupported
values and non-Unicode values are also invalid. Values are neither trimmed
nor converted to lowercase, and there are no aliases. Invalid input never
falls back to `auto`.

This is recipe context, with no separate top-level CLI option. The
[usage guide](../howto/keep-workflow-issues-open.md) gives invocation examples.

## Classification and publication

The existing classifier determines whether changed paths are substantive,
planning artefacts or generated files. At least one substantive path permits
`Closes` under `auto` or `closes`. Planning-only, generated-only and empty
changes use `Refs`. Planning artefacts with generated version or lockfile
changes still use `Refs`, including under explicit `closes`.

Failed or unusable classification also selects `Refs`. Output emitted before
a classifier failure is discarded; partial output cannot authorise closure.
Policy validation precedes interpretation of classification evidence.

Step 15 classifies staged changes for its commit message. PR publication
classifies the branch changes independently for its body. Both use the same
policy and keyword operation, so equivalent evidence produces the same
keyword. Their evidence is not cached or shared: staged changes and the full
branch can legitimately classify differently.

The policy changes the keyword, not issue identity. In strict delivery,
`closing_reference` still specifies the identity: a local `#N` or a qualified
`owner/repo#N`. For example, `refs` retains a cross-repository identity as
`Refs rysweet/amplihack-rs#2550`. Existing local tracking and Azure DevOps
formatting and handling remain unchanged. No GitHub issue is created merely
to satisfy this policy.

The existing minimal-body fallback retains the selected policy. Failure to
execute the required Rust policy operation stops publication; it is distinct
from uncertain classification. Existing hooks and protected-branch and push
guards still apply.

## Context propagation and recovery

Each boundary declares `pr_issue_link: auto` and carries the caller's value
through this nesting:

```text
auto-drive-to-merge
└── autodrive-build
    └── default-workflow
        └── workflow-publish
            └── workflow_publish_pr.sh
```

The auto-drive and build mappings forward it explicitly. Workflow phases
retain parent-context inheritance. A directly invoked recipe receives its
own declared default when the caller omits the key.

Shell integration and the Rust policy operation read the authoritative
runner export, `RECIPE_VAR_pr_issue_link`. An unrelated inherited variable
such as `PR_ISSUE_LINK` or `pr_issue_link` cannot override it. A new nested
runner uses its own context and exported value rather than an enclosing
process's stale environment.

`default-workflow` persists the policy in its recovery context. Recovery
restores and validates the saved value from the runner's admitted source
before replay compares its inputs. Explicit caller context takes precedence.
Older records without the key receive
`auto`; an invalid saved value fails rather than reverting to a default.
This is separate from merge authorisation and grants no permission to merge.

Normal workflow entry, publication-phase entry and immediate script entry
validate the policy. Invalid values fail before version edits, staging,
commits, pushes, publication body-file writes or PR creation.

## Published-reference preservation

GitHub body replacements in finalise, auto-drive merge rounds and merge-ready
preserve the keyword and identity text of each existing operative issue
reference. These edits do not reclassify the diff or recalculate the keyword
from a later policy value. Summary, description and readiness evidence may
still change.

For each published reference, the edit operation:

- restores it if the candidate body omits it;
- replaces a conflicting candidate keyword with the published spelling and
  meaning, retaining the published identity text;
- places the preserved reference above the readiness-report boundary, so it
  remains part of the description used for squash merging.

For a PR in `rysweet/amplihack-rs`, these edits preserve the published text:

| Published reference | Candidate reference | Preserved reference |
| --- | --- | --- |
| `Refs #2550` | `Closes #2550` | `Refs #2550` |
| `Refs #2550` | Omitted | `Refs #2550` |
| `Fixes rysweet/amplihack-rs#2550` | `Refs #2550` | `Fixes rysweet/amplihack-rs#2550` |

Recognised operative references use `Refs` or GitHub's closing-keyword family:
`close`, `closes`, `closed`, `fix`, `fixes`, `fixed`, `resolve`, `resolves` and
`resolved`. Closing keywords are case-insensitive. Fenced examples and inline
code are excluded; a command example in code does not become an issue-closing
instruction.

Supported identities are local references such as `#2550`, qualified
references such as `rysweet/amplihack-rs#2550`, and canonical GitHub issue URLs
such as `https://github.com/rysweet/amplihack-rs/issues/2550`. Local identities
resolve against the explicit PR repository. Repository names compare
case-insensitively, so equivalent notations identify the same issue while
the published text is retained. URL recognition checks the host and path;
lookalike hosts do not identify GitHub issues.

Parsing is bounded. An ambiguous published baseline is rejected before any
edit. Conflicting or malformed pre-existing references have no general repair
guarantee; this operation preserves established meaning rather than resolving
uncertain intent.

The editor reads the current body, constructs the preserved candidate,
submits it once with `gh pr edit --body-file -`, then reads it back once.
Every GitHub operation names the explicit repository and PR and has a
30-second subprocess timeout, including stdin delivery for edits. It verifies
both the submitted body and its operative references. A timed-out edit or
readback reports that the update may already have occurred. There are no
retries or automatic rollback. Concurrent edits are not atomic with
this sequence; a readback discrepancy remains a visible failure.

Azure DevOps description updates keep their existing path.

## Squash messages

The integration uses the explicit squash-message helper from the
trusted launch bundle. The subject comes from the PR title; the body comes from the
published description, excluding the readiness report. Preserved references
stay above the first report boundary: a line beginning with
`<!-- merge-ready-report` or `## Merge readiness`, with ASCII case and a
trailing carriage return ignored.

A substantive PR published with `refs` therefore carries `Refs #N` into the
actual squash message body. The merge receives the body on standard input
through `--body-file -` and retains `--match-head-commit` protection. The
policy changes neither merge criteria nor merge authorisation.

## Internal command contract

The `amplihack workflow issue-reference` command group provides these
operations to the workflow integration.

| Operation | Inputs | Successful result |
| --- | --- | --- |
| `validate` | Policy from `RECIPE_VAR_pr_issue_link`; absence means `auto`. | Exit 0 with empty stdout. |
| `keyword` | The same policy plus classifier `--classify` output on stdin. | Exit 0 with exactly `Closes\n` or `Refs\n` on stdout. |
| `edit-body` | Explicit validated GitHub repository, positive PR number and readable UTF-8 candidate-file arguments. | One read-edit-readback operation preserving published references. |

```bash
amplihack workflow issue-reference validate
printf 'substantive src/main.rs\n' | amplihack workflow issue-reference keyword
amplihack workflow issue-reference edit-body \
  --repo owner/repository --pr 42 --candidate-file "$CANDIDATE_FILE"
```

Classifier evidence consists of one `<class> <path>` line per changed path,
where class is `substantive`, `planning` or `generated` and path is non-empty.
Empty, oversized (over 1 MiB) or unusable evidence selects `Refs`. Callers
must discard all classifier stdout when classification exits unsuccessfully,
then pass empty evidence. A failed Rust operation stops publication.

Diagnostics go to stderr. Invalid invocation or policy returns 2. Execution
or preservation failure returns 1. Successful operations return 0. An invalid
policy cannot be hidden by empty or unusable classifier evidence.

Candidate files belong beneath private invocation temporary directories with
restrictive permissions. Body contents reach GitHub through stdin, not shell
interpolation. Diagnostics escape invalid values and do not log complete
bodies, classifier payloads, credentials or environment contents.
Candidate and published bodies are limited to 65,536 bytes. A candidate whose
Markdown would hide a restored reference fails before submission.

## Failures

| Failure | Visible behaviour |
| --- | --- |
| Invalid current or recovered policy | Status 2; identifies the invalid policy before publication side effects. |
| Failed or unusable classifier evidence | Conservative `Refs`; no partial classification output authorises closure. |
| Required Rust operation unavailable or fails | Publication stops; no permissive helper fallback. |
| Invalid repository, PR number or candidate-file arguments | Invocation fails before GitHub mutation. |
| Unreadable or invalid UTF-8 candidate file | Fails without editing the PR. |
| Baseline read fails or references are ambiguous | Fails before submitting an edit. |
| GitHub edit fails | Reports failure; no automatic retry or rollback. |
| Readback fails or differs after submission | Status 1 and an explicit warning that the update may already have occurred. |

After an uncertain edit outcome, inspect the live body before starting a new
operation. A failure does not prove that GitHub left the body unchanged.

See also [delivery inputs](workflow-delivery-inputs.md),
[auto-drive configuration](auto-drive-to-merge.md#configuration) and
[PR recovery](../howto/recover-existing-pr-with-default-workflow.md).
