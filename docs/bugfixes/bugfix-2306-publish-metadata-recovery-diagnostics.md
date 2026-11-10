---
title: "Draft PR metadata recovery and diagnostics"
description: "GitHub draft metadata recovery, portable body transport, and sanitized errors through the pinned recipe runner."
doc_type: reference
last_updated: 2026-11-10
review_schedule: as-needed
owner: amplihack
issue: 2306
---

# Draft PR metadata recovery and diagnostics

Step `step-16-create-draft-pr` creates a GitHub draft when optional title or
body derivation fails. Terminal creation failures retain the operation, original
exit status, and sanitized GitHub CLI stderr in both the structured message and
the pinned recipe runner's serialized `StepResult.error`. Empty stderr receives
an explicit explanation.

Implementation and local behavior tests are complete. Delivery through required
CI, review, and merge remains for the subsequent workflow steps. The historical
trigger on the four reported macOS runs remains unverified.

This reference defines the publishing contract for issue #2306. It complements
the [existing title selection rules](bugfix-1968-publish-pr-merge-base-title.md)
and [publish and finalize outcomes](../operations/workflow-resilience.md).

GitHub issue-title recovery and `--body-file` transport apply to the GitHub path.
Azure DevOps retains its existing branch-based titles, `AB#N` work-item references,
`az repos pr create --description` transport, and provider outcomes. This contract
does not extend GitHub recovery behavior to Azure DevOps.

## Contents

- [Invocation and configuration](#invocation-and-configuration)
- [Title selection and recovery](#title-selection-and-recovery)
- [Body recovery](#body-recovery)
- [Results and diagnostics](#results-and-diagnostics)
- [Portability and diagnosis evidence](#portability-and-diagnosis-evidence)
- [Local validation evidence](#local-validation-evidence)
- [Historical attribution and subsequent delivery](#historical-attribution-and-subsequent-delivery)
- [Behavioral verification contract](#behavioral-verification-contract)

## Invocation and configuration

The publish recipe runs step 16 after a successful push when publishing is
required and no earlier terminal success has been established. Recovery is
automatic; it adds no configuration switch or public API.

| Input | Contract |
| --- | --- |
| `worktree_setup` / `repo_path` | Resolve the working tree through the existing worktree helper. Missing helpers, an unusable directory, or an invalid Git repository produce actionable failures before publication. |
| Current branch and origin | Existing helpers establish the PR head, base, repository, and ownership. Detached HEAD, protected branches, and invalid identity remain rejected. |
| `issue_number` / `ISSUE_NUMBER` | A validated numeric value identifies a linked issue. Empty input or an accepted `local-…` tracking reference means no linked issue; malformed input fails before provider calls. |
| `task_description` / `RECIPE_VAR_task_description` | The first nonblank task line supplies the normal secondary title source, after removing a leading Markdown heading marker. Inherited bare `TASK_DESCRIPTION` is not a source. |
| `RECIPE_VAR_local_testing_gate` / `RECIPE_VAR_precommit_results` | Available validation evidence enriches the body. Failure to format it does not prevent a draft. |

Git, `jq`, GitHub CLI authentication, required helpers, and mandatory publishing
lookups retain their existing prerequisites. Metadata recovery does not bypass
these requirements. Duplicate-claim checks retain their existing advisory
lookup-error behavior and reject a confirmed conflicting claim. Other provider
paths retain their existing outcomes.

## Title selection and recovery

Normal selection uses the first usable source:

1. The linked issue's title.
2. The first nonblank line of this run's task description.
3. The changed-file template: `Update <scope>` or
   `Update <scope> with <count> changed files`.

An unavailable issue title during normal selection still allows a valid task
title. Scope selection continues to prefer substantive files over generated
files, and file counts and body listings use the existing merge-base diff rules.

Each derivation operation is checked separately. A failed command, failed
normalization, or unusable completed title activates recovery. Empty text,
whitespace-only text, and an issue suffix without substantive title text are
unusable. Appending ` (#2306)` cannot turn an empty base into a valid title.

Recovery follows two stages:

1. Independently request the validated numeric issue's title using
   `gh issue view --json title` and the validated repository identity. Accept
   only a JSON object containing a usable string `title`. Normalize, sanitize,
   defuse references, and bound the result before adding the issue suffix.
2. If lookup, parsing, normalization, or validation fails, construct
   `Update <branch> (#N)` with Bash builtins. This fallback does not depend on
   the failed normalizer, another optional Git query, or an external formatter.
   Unsafe branch content is replaced with a neutral component rather than
   copied into the title.

Missing `gh`, a failed lookup, malformed JSON, missing or non-string `title`, and
blank titles cannot block the final fallback. Missing `gh` remains a terminal
failure when the mandatory creation operation requires it. Without a linked
numeric issue, recovery skips the issue lookup and produces `Update <branch>`;
it invents neither a numeric reference nor a suffix for local tracking IDs.

All accepted titles preserve credential redaction, control removal, reference
defusing, UTF-8 integrity, and the 200-byte limit including the validated issue
suffix. Normal issue and task titles retain conventional-prefix behavior.
Derived references to other issues cannot become closing or claim signals.
Diagnostics identify the title source or failed operation without echoing titles.

The following title examples use issue #2306 and branch
`feat/issue-2306-amplihack-rs-2306-p1`:

| Available metadata | Result |
| --- | --- |
| Usable issue title `Fix draft PR diagnostics` | `fix: Fix draft PR diagnostics (#2306)` through normal selection |
| Normal issue lookup unavailable; task line `Document PR recovery` | `docs: Document PR recovery (#2306)` through normal selection |
| Derivation command fails; recovery issue title `Fix draft PR diagnostics` | A sanitized issue-derived title ending in ` (#2306)`; draft creation continues |
| Derivation fails; recovery returns `{"title":42}` or `{"title":"   "}` | `Update feat/issue-2306-amplihack-rs-2306-p1 (#2306)` |

## Body recovery

Changed paths, diff statistics, commit summaries, validation excerpts, behavior,
and risk descriptions are optional body content. Their formatting is guarded
independently, retaining successful sections where practical. If richer body
construction fails, builtin `printf` produces a minimal useful summary and
`Refs #N` for a validated numeric issue. A fallback never invents validation
results or earns a closing keyword merely because metadata failed.

Empty and local issue inputs retain the no-linked-issue behavior. Rich bodies
retain the existing diff-based closing-reference policy and mention handling.

On the GitHub path, body transport continues through `workflow_body_file.sh` and a private
`--body-file`; body text is not placed in command arguments. Temporary-file
creation and writing are mandatory operations: either failure stops publishing
with its operation and status or failure condition.

## Results and diagnostics

The behavior tests execute the actual step through the pinned runner and inspect
its serialized failure, rather than relying only on helper stderr.

Stdout remains reserved for the existing structured publishing result, parsed
as `pr_publish_result` with `parse_json: true`. Its fields remain `state`,
`legacy_state`, `terminal_status`, `pr_url`, `pr_number`, `branch_diff_status`,
and `message`. Human-readable warnings and diagnostics use stderr.

| Outcome | Result |
| --- | --- |
| Recovery followed by successful draft creation | Exit zero; `FOLLOWUP_CREATED`, legacy `create-new-pr`, and `terminal_status=success`, with the created PR identity. |
| Creation fails; scoped collision recovery finds an eligible open PR | Existing-open-PR success with that PR's identity. Repository, head, base, ownership, and existing scope checks still apply. |
| Creation fails; no eligible open PR can be adopted | Nonzero step exit; `FAILED_PR_CREATE`, legacy `create-pr-failed`, and `terminal_status=failure`. |
| Already open, already merged, closed after merge, or empty branch | Existing classifications and successful terminal behavior remain intact. |
| Mandatory gate fails or a closed unmerged PR still has a diff | Existing failure behavior remains enforced with an actionable diagnostic. |

A terminal command diagnostic contains the stable operation name, original exit
status, and sanitized stderr. Empty stderr receives an explicit explanation.
For example, a fake `gh pr create` exiting 42 with `draft creation denied` must
produce an error containing `gh pr create`, `42`, and `draft creation denied`.
With empty stderr it must still contain the operation, status, and an
empty-stderr explanation. This is a content contract, not an exact message format.

Creation evidence is captured separately from stdout and retained while the
scoped collision lookup runs. A failed collision lookup adds context; it cannot
replace the original creation diagnostic. On `FAILED_PR_CREATE`, creation
evidence appears in stderr and the structured `message`, and survives into the
runner's serialized step error. Pre-helper failures also identify the operation
and status or condition. A final-exit backstop prevents unexplained nonzero
exits without duplicating an existing terminal diagnostic.

The shared retry helper retains retry classification, timing, successful stdout,
and the original command status. Its terminal paths report diagnostics before
cleanup. Diagnostic formatting or cleanup failures cannot overwrite that status
or disturb caller traps. No timers or polling delays are added.

### Diagnostic privacy

Stderr is untrusted data. Sanitization removes terminal controls and redacts URL
userinfo, GitHub tokens, supported `sk-` credential formats, and AWS access key
identifiers before final truncation. If sanitization fails after partial output,
the partial output is discarded and fixed safe text replaces it. Neither output
channel nor serialized errors may contain the captured secrets.

The integration review also reproduced adjacent AWS identifiers sharing a
separator: the original pattern consumed that separator and left the second
identifier visible. Both title and retry sanitizers now re-scan matches, with
coverage for title recovery, direct diagnostics, and serialized runner errors.

Temporary capture files are created atomically with private permissions and
cleaned up on every exit. Shell metacharacters remain literal data in quoted
arguments and fixed `printf` formats. Diagnostics do not dump command arguments,
environment values, title text, raw parser errors, or shell traces.

## Portability and diagnosis evidence

The implementation must support macOS Bash 3.2 and BSD utility behavior without another
language runtime, new GNU-only dependencies, or host configuration. Conditional
function calls explicitly check meaningful internal commands: Bash's `errexit`
behavior in conditional contexts cannot serve as failure detection. Assignment
failure statuses are captured in the `else` arm before subsequent commands can
overwrite them.

The historical command behind the four reported macOS failures is unknown.
A blank-title hypothesis alone does not establish it, and `timeout` was present
on the reported host. An injected failure establishes recovery behavior rather
than a macOS portability diagnosis. Neither `head -c` nor another utility is
identified as the cause without platform-backed evidence.

### Reproduced portability defect and correction

The concrete defect is in **body-file transport**, not a proven blank-title
failure: unqualified `mktemp` in `gh_pr_create_with_retry` and `write_body_file`
can disregard the caller's designated temporary directory on macOS.

Apple's [shell_cmds-278 mktemp implementation](https://github.com/apple-oss-distributions/shell_cmds/blob/shell_cmds-278/mktemp/mktemp.c#L94-L115)
turns a no-argument invocation into `-t tmp`, then prefers a successful
`confstr(_CS_DARWIN_USER_TEMP_DIR)` lookup over `TMPDIR`. An explicit positional
template bypasses that directory selection. The
[current Apple implementation](https://github.com/apple-oss-distributions/shell_cmds/blob/main/mktemp/mktemp.c#L120-L175)
also chooses the Darwin directory when `tmpdir` is unset. This matters when a
process is given a writable `TMPDIR` and the host default is unavailable or
outside its permitted filesystem. GNU `mktemp` uses `TMPDIR` for this invocation.

The correction uses explicit `"${TMPDIR:-/tmp}/<purpose>.XXXXXX"` templates in
the publisher **and** `workflow_body_file.sh`. Both files must change: otherwise
the writer's two internal scratch files retain the same failure. The shared
retry and provider captures likewise use private, atomic templates. There is no
host-specific setting, new runtime dependency, or change to body transport.

Reproducer platform: Linux x86_64, GCC 15.3.0; Apple `shell_cmds-278/mktemp.c`,
SHA-256 `1c1fa511e9e8ab7692ef1a5d78578845fabb90c8941a8f1ec927dff85fd9da85`.
This is a source-backed utility adapter, **not native Darwin execution**. The
Apple implementation was compiled with `__APPLE__` and a tiny adapter for the
Darwin-only `confstr` lookup. The adapter returns the chosen directory; option
parsing, directory precedence, file creation, error status, and stderr come
from Apple's unmodified implementation. Its complete adapter is:

```c
#include <stdlib.h>
#include <string.h>
size_t apple_temp_confstr(int name, char *buf, size_t len) {
    const char *path = getenv("TEST_DARWIN_USER_TEMP_DIR");
    if (!path) return 0;
    size_t need = strlen(path) + 1;
    if (len) { strncpy(buf, path, len); buf[len - 1] = 0; }
    return need;
}
```

Compile the downloaded Apple source and this adapter with:

```sh
gcc -D_GNU_SOURCE -D__APPLE__ -D_CS_DARWIN_USER_TEMP_DIR=65536 \
  -Dconfstr=apple_temp_confstr apple-mktemp.c darwin-confstr.c -o apple-mktemp
mkdir -p caller-tmp
export TMPDIR="$PWD/caller-tmp"
export TEST_DARWIN_USER_TEMP_DIR="$PWD/missing"
./apple-mktemp
./apple-mktemp "$TMPDIR/workflow.XXXXXX"
```

With `missing` absent, the first operation exited **1**, with empty stdout and
`mkstemp failed on .../missing/tmp.<random>: No such file or directory` on
stderr. The second exited **0**, creating a private file in `caller-tmp`.
Running the actual old `gh_pr_create_with_retry` function under Bash 3.2.0
with this Apple utility adapter exited **1**, producing no PR URL. Running the
corrected function with the same adapter and a fake `gh` exited **0** and
returned `https://github.com/owner/repo/pull/99`. No GitHub write occurred.

The behavior test
`apple_mktemp_directory_precedence_cannot_override_the_caller_tmpdir` models
exactly this documented selection rule. It failed before the correction at
`gh pr create stderr temp creation failed (exit 1)` and passed afterwards,
including all inner body scratch files and their cleanup.

No concrete GNU-only title command was established. Apple supports `head -c`,
and title helpers also ran successfully under real Bash 3.2.0. The metadata
recovery tests prove fault tolerance independently of this body-path diagnosis.
The source-backed reproducer proves a general compatibility defect; it does
**not** establish that any reported Mac had an unavailable Darwin temp directory.

### Local validation evidence

- The previous TDD run had 25 failed and 8 passed tests. After implementation,
  the metadata suite has **34 passed, zero failed or skipped**, including the
  new Apple directory-precedence case and malformed JSON-stream cases.
- The six runner tests execute the actual step copied from
  `workflow-publish.yaml` through revision
  `4d5187eaf95836a549e6e68725f8f853d0dc7034`, with Cargo installation provenance
  checked against `claude-plugin/recipe-runner.rev`. Assertions require
  `StepResult.status=failed` and an error containing `gh pr create`, `23`, and
  `distinctive publish rejection`, or the explicit empty-stderr explanation.
  The tests also verify credential redaction, successful recovery with parsed
  results, and a pre-helper `git branch` failure with status 79. No runner
  production code or pin change was needed.
- A local GNU Bash 3.2.0(3)-release was built from the official
  [Bash 3.2 source](https://ftp.gnu.org/gnu/bash/bash-3.2.tar.gz), SHA-256
  `26c99025b59e30779300b68adb764f824974d267a4d7cc1b347d14a2393f9fb4`.
  The metadata suite ran with this executable first on PATH. This verifies
  Bash syntax and semantics, not Darwin libc or every BSD utility.
- The existing publishing, scope, provider, local-issue, finalization and
  terminal-state modules passed **71 tests** in an isolated Rust harness.
  The shared body-file suite passed **10 tests**, including byte-for-byte
  transport, private permissions, cleanup, UTF-8 truncation and caller survival.
- `bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh`
  passed, including the established title-source, template-summary, merge-base,
  retry, terminal and worktree checks. ShellCheck at warning severity,
  `git diff --check`, and the recipe brick-budget check passed.
- `pre-commit run --all-files` passed every hook, including Artifact Guard,
  the brick budget, `cargo fmt --all --check`, and
  `cargo clippy --all-targets -- -D warnings`, with Rust 1.97.0. Validation used
  explicit pinned compiler paths and a private build cache. An earlier attempt
  exposed ambient Cargo 1.99.1 versus Clippy 1.97.0 mixing metadata; that cache
  was removed with `cargo clean`. A temporary-storage quota interruption was
  resolved before the successful final run.

The ordinary repository invocation for the new suite is:

```sh
cargo test -p amplihack --test integration workflow_publish_pr_metadata
pre-commit run --all-files
```

During this step the same source was also executed through an isolated Cargo
harness to avoid coupling the behavior tests to unrelated workspace builds.
All fixtures use temporary repositories, fake executables and local refs.
No test opens a GitHub PR, sleeps, or contacts GitHub.

### Historical attribution and subsequent delivery

Original logs identifying the failed command in software-factory #180, #122,
#109, and #114 were not supplied. Attribution remains **unverified**. Neither
the injected metadata failures nor the reproduced temp-directory defect proves
the cause of those runs.

Subsequent delivery steps must record the PR containing `Fixes #2306`, required
CI, final all-files pre-commit result, relevant review resolutions, and merge
commit. No force-push or hook bypass is authorized. This implementation step
does not claim that publication, CI, or merge has happened.

## Behavioral verification contract

Behavior tests use temporary repositories, local base refs, isolated
configuration, and fake executables on `PATH`. They make no GitHub writes and
use permanent nonretryable failures without test sleeps.

| Scenario | Required observable behavior |
| --- | --- |
| Successful issue, task, and changed-file sources | Preserve title precedence, sanitization, counts, and draft arguments. |
| Injected title-derivation command failure | Reach `gh pr create --draft` with the recovery issue title and report success. |
| Empty, whitespace-only, or suffix-only derived title | Enter recovery rather than publish an unusable title. |
| Unavailable, failed, malformed, missing, non-string, empty, or whitespace-only recovery title | Reach the safe branch fallback and create a draft successfully. |
| Persistent normalizer failure or malicious branch text | Produce a safe bounded title independently; execute no injected shell text. |
| Optional body formatter failure | Retain available sections or publish a minimal summary with only the appropriate issue reference. |
| Distinctive or empty creation stderr | Preserve the operation, original status, and sanitized message or empty-stderr explanation through the failed step. |
| Eligible and unrelated collision PRs | Adopt only the eligible open PR; retain creation evidence on failure, including collision-lookup failure. |
| Mandatory gate rejection and existing terminal states | Preserve identity, duplicate, no-diff, merged, closed, and provider behavior. |
| Credentials, controls, and sanitizer failure after partial output | No secret or unsafe partial output reaches stdout, stderr, or serialized errors. |
| Temporary capture/body-file creation, writing, or cleanup failure | Diagnose mandatory failure, preserve original status, private permissions, and cleanup behavior. |

Coverage extends the existing reliability shell suite and the integration
suites `workflow_publish_resilience.rs`, `workflow_publish_pr_metadata_test.rs`,
and `workflow_publish_terminal_gate.rs` under `tests/integration/`.

Step-error verification extracts the actual step-16 command from
[`workflow-publish.yaml`](../../amplifier-bundle/recipes/workflow-publish.yaml)
into a minimal temporary recipe with its required context and executes it through
the runner pinned by
[`claude-plugin/recipe-runner.rev`](../../claude-plugin/recipe-runner.rev).
Assertions inspect the failed serialized `StepResult.error`, including credential
redaction, distinctive stderr, and empty stderr. Helper stderr alone or a skipped
runner execution does not establish this boundary contract.

Relevant publishing, retry, scope, finalization, local-issue, provider, and
terminal-state regressions remain in the existing suites and CI. Delivery also
requires the repository's all-files pre-commit checks. No new CI job or broad
workflow rewrite is part of this contract.
