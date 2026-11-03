# Tracking Issue Selection Reference (Issues #1595, #1887)

> [Home](../index.md) > Reference > Tracking Issue Selection

How `workflow-prep` `step-03-create-issue` chooses the tracking issue for a
run on a GitHub repository, how that choice reaches `step-03b`, and how a
caller can fix the host type or turn off issue creation.

## Contents

- [Summary](#summary)
- [Selection order](#selection-order)
- [Explicit issue number](#explicit-issue-number)
- [Candidates from the task description](#candidates-from-the-task-description)
- [Candidate validation](#candidate-validation)
- [Helper function contract](#helper-function-contract)
- [Diagnostics](#diagnostics)
- [Behaviour by example](#behaviour-by-example)
- [Azure DevOps and local tracking](#azure-devops-and-local-tracking)
- [Caller overrides](#caller-overrides)
- [Known limitations](#known-limitations)
- [Tests](#tests)
- [Related documentation](#related-documentation)

## Summary

On a `github` host, step-03 hands the choice to
`select_github_tracking_issue` in
`amplifier-bundle/tools/workflow_issue_tracking.sh`. The function:

1. honours an explicit `issue_number` context value whenever one is supplied;
2. otherwise reuses a number from the task description only when it is an
   open issue, not a pull request, in the current repository;
3. otherwise returns without output on stdout, and step-03 continues with its existing
   title search and `gh issue create` path.

Before this change, step-03 ignored `issue_number` on GitHub and reused the
first `#N` anywhere in the task text. `gh issue view N` succeeds for pull
requests and closed issues, so a task that mentioned "runner fixes #161" was
attached to an old merged pull request, and a task that cited closed issue
#1578 as background was attached to that issue and then refused by step-03c.

## Selection order

| Order | Source | Condition for reuse | Lookup |
| ----- | ------ | ------------------- | ------ |
| 1 | `issue_number` context value | Digits after stripping one leading `#` | `gh issue view N --json url` |
| 2 | Marked reference in the task text (`Issue #N`, `Fixes #N`, ...) | Open issue, not a PR, URL ends in `/issues/N` | `gh api repos/{owner}/{repo}/issues/N` |
| 3 | Bare `#N` in the task text | Same as order 2 | Same as order 2 |
| 4 | Open issue whose title matches the task | Unchanged existing behaviour | `gh issue list --search` |
| 5 | New issue | Unchanged existing behaviour | `gh issue create` |

At most five task-text candidates are looked up. The first accepted
candidate wins.

## Explicit issue number

Pass the number as recipe context:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix step-03 issue selection. Background: PR #1585, #1578." \
  -c issue_number=1592
```

Step-03 reads it as `EXISTING_ISSUE_NUMBER="${ISSUE_NUMBER:-}"`.

| `issue_number` value | Result on `github` |
| -------------------- | ------------------ |
| `1592` or `#1592` | `gh issue view 1592` runs; its URL is printed |
| `1592`, but `gh issue view` exits non-zero (timeout, rate limit, authentication) | `WARN: could not verify issue #1592 (gh exit RC); falling back to local tracking`, then local metadata with `tracking_reference=local-issue-1592` and `issue_number=1592`, exit 0 |
| `1592`, the view succeeds but its URL is not a single-line `https://…/issues/1592` (a PR, another repository) | `WARN: explicit issue_number #1592 could not be resolved as an issue in this repository …`, then the same local metadata, exit 0 |
| `abc`, `12a`, `##1592`, `1592\nissue_number=9`, or more than 9 digits | Fixed warning on stderr (the value is not echoed); treated as unset for selection |
| unset or empty | Task-text candidates are considered |

Only one leading `#` is stripped, so `##1592` is rejected rather than read as
1592.

"Treated as unset" applies to selection only. On the local fallback paths
(`other` hosts, outside a git repository, or a failed repository lookup),
`derive_local_tracking_id` still reads the raw `issue_number` value. The
anchored pattern described under
[`derive_local_tracking_id`](#derive_local_tracking_id) is what stops such a
value from adding lines to the metadata.

An explicit number is never filtered for state or for being a pull request.
The user's choice stands. If the issue is closed or already claimed by a
merged pull request, step-03c refuses the run as it does today.

When the explicit lookup fails, step-03 does not search titles or create an
issue. The run keeps the number the user chose rather than attaching to an
unrelated issue. A returned `/pull/N` URL is never passed on as an issue URL.

## Candidates from the task description

`_tracking_issue_candidates` reads `TASK_DESC` and makes no network calls.

### Input handling

- Only the first 65536 characters are examined (bash substring, not
  `head -c`, so that `pipefail` never sees `SIGPIPE`).
- Matching is case-insensitive.

### Masked forms (never candidates)

| Form | Example | Reason |
| ---- | ------- | ------ |
| `PR #N`, `PR#N` | `PR #1585` | Names a pull request |
| `pull request #N`, `pull #N` | `pull request #77` | Names a pull request |
| `owner/repo#N` | `rysweet/recipe-runner-rs#161` | Another repository |
| `AB#N` | `AB#4242` | Azure Boards work item |
| `#N` followed by a word character | `#123abc` (hex colour) | Not an issue reference |
| `#N` preceded by a word character | `abc#12` | Not an issue reference; the whole token is dropped, so `abc#12` never yields 12 |

### Marked references

A marked reference is a keyword, then optional spaces, then an optional `:`,
then optional spaces, then `#N`. It marks the number as one the task asks to
work on. The keywords are:

| Keyword group | Forms |
| ------------- | ----- |
| issue | `issue` |
| fix | `fix`, `fixes`, `fixed` |
| close | `close`, `closes`, `closed` |
| resolve | `resolve`, `resolves`, `resolved` |

Examples that match: `Issue #1595`, `issue#5`, `fixes #42`, `Resolves: #30`,
`closes : #7`.

A keyword and its `#N` must be on the same line. A marker split across a line
break is treated as a bare reference.

### Ordering and limits

1. Marked references, in order of appearance.
2. Bare references, in order of appearance.
3. Duplicates removed; the first occurrence keeps its position.
4. Each number must match `^[1-9][0-9]{0,8}$`, so `#0`, `#007` and numbers of
   ten or more digits are dropped.
5. The list is cut to five entries.

A marker only affects ordering. A marked number that fails validation is
rejected like any other.

```text
Task text: "See #20 first. Fixes #30."
Candidates: 30, 20

Task text: "Issue #1595 ... see #1578"
Candidates: 1595, 1578

Task text: "Follow-up to PR #12 and rysweet/other#13, AB#14"
Candidates: (none)
```

## Candidate validation

`_github_open_issue_url N` makes one read-only call:

```bash
timeout 30 gh api "repos/{owner}/{repo}/issues/N" \
  --jq '[.state, (.pull_request != null), .html_url] | @tsv'
```

`{owner}/{repo}` is filled in by `gh` from the current repository, so the
lookup and any later `gh issue create` resolve to the same repository. No
`--hostname`, `--repo`, `-X` or field flags are passed.

A candidate is accepted only when every check passes:

| Check | Required value |
| ----- | -------------- |
| Exit status | `0` |
| `state` | `open` |
| PR flag | `false` |
| `html_url` | a single-line `https://…/issues/N` URL (rejects issues transferred elsewhere); the same check as the explicit path |

A timeout, a 404, an authentication or rate-limit failure, or output in an
unexpected shape rejects the candidate. If every candidate is rejected, the
function logs the numbers it checked on stderr, and step-03 falls through to
the title search and then creates an issue. When `gh api` itself exits
non-zero (a timeout, 404, auth or rate-limit failure), stderr also gets
`WARN: could not verify candidate #N (gh exit RC)`, with gh's own stderr still
discarded; closed issues, PRs and bad URLs are rejected without a warning.

## Helper function contract

These functions live in `amplifier-bundle/tools/workflow_issue_tracking.sh`
(its header lists every function, including the internal
`_tracking_issue_masked_text`, `_tracking_issue_refs` and
`_is_github_issue_url`), are sourced by step-03, and read `EXISTING_ISSUE_NUMBER` and `TASK_DESC` from
the caller's scope. None of them calls `exit`. All are bash 3.2 compatible.

### `select_github_tracking_issue`

| Return code | stdout | Meaning |
| ----------- | ------ | ------- |
| `0` | One issue URL | Explicit number or accepted candidate |
| `0` | Local metadata block | Explicit number whose lookup failed (see above) |
| `1` | Empty | Nothing selected; step-03 searches and creates |

Step-03 calls it in one line, in place of the old `gh issue view
"$REF_ISSUE_NUM"` line:

```bash
GH_TRACKING=$(select_github_tracking_issue) && { printf '%s\n' "$GH_TRACKING"; exit 0; }
```

### `_tracking_issue_candidates`

Prints up to five `N kind` lines in priority order, where `kind` is `marked`
or `bare`. A number that appears in both forms is printed once, as `marked`.
The selection log uses this kind, so the label always matches the reference
that picked the number. Makes no network calls. The caller reads it through
process substitution, so its exit status is not used.

### `_github_open_issue_url N`

Prints the issue URL and returns 0 when `N` passes every validation check.
Returns 1 with no output otherwise, including when `N` is not all digits.

### `derive_local_tracking_id`

Unchanged in purpose. Its `local-issue-N` / `local-ab-N` pass-through pattern
is now anchored as `^local-(issue|ab)-([0-9]+)$`, so a value containing a
newline cannot add `key=value` lines to the metadata that step-03b parses.

## Diagnostics

All diagnostics go to stderr. stdout carries only the URL or the metadata
block. `gh` stderr is discarded. The task text is never logged.

| Message | When |
| ------- | ---- |
| `INFO: tracking issue #1592 selected (explicit)` | Explicit number resolved |
| `INFO: tracking issue #1595 selected (marked)` | Marked candidate accepted |
| `INFO: tracking issue #20 selected (bare)` | Bare candidate accepted |
| `INFO: no task-text candidate accepted as an open issue (checked: 1595 1578); falling back to title search` | Every candidate was rejected; step-03 continues with the title search |
| `WARN: could not verify issue #1592 (gh exit 124); falling back to local tracking` | `gh issue view` failed; the exit code tells a timeout (124) from an auth, rate-limit or network failure. gh's own stderr is not shown |
| `WARN: explicit issue_number #1592 could not be resolved as an issue in this repository ...` | `gh issue view` succeeded but did not return this repository's issue URL |
| `WARN: ...` with fixed text, no value | `issue_number` was not numeric |

Search the step-03 log for `selected (bare)` to find runs that attached to an
issue mentioned only in passing, and for `no task-text candidate accepted` to
find runs that created or reused an issue after rejecting every reference.

## Behaviour by example

Scenarios run against a GitHub repository where #1592 and #1595 are open
issues, #1578 and #42 are closed issues, #161 and #1585 are merged pull
requests, and #162 does not exist.

| `issue_number` | Task text | Tracking issue |
| -------------- | --------- | -------------- |
| `1592` | `See PR #1585 and #1578 for background.` | #1592 |
| unset | `Issue #1595: fix selection. See #1578.` | #1595 |
| unset | `Fixes #42` (closed) | new issue |
| unset | `Fixes #161` (PR) | new issue |
| unset | `Follows #1585.` (PR) | new issue |
| unset | `Related to #1578.` (closed) | new issue |
| unset | `See #1592 first. Fixes #1595.` | #1595 |
| unset | `PR #1592, rysweet/amplihack-rs#1592, AB#1592` | new issue; no `gh api` lookup |
| unset | `Upgrade the runner; runner fixes #161 and #162.` | new issue; branch is not `feat/issue-161-...` |
| unset | `Add a --json flag to recipe list.` | title search, then new issue |

In each reuse row, step-03b reads back the same number as `issue_number`.
By default step-04 then names the branch `${BRANCH_PREFIX:-feat}/issue-<N>-<slug>`;
a branch named in the task text, or an existing branch or pull request for the
task, takes precedence. The tests check only the step-03b read-back.

## Azure DevOps and local tracking

The selection described above applies only to `github` hosts.

| Host | `issue_number=763` | Task-text reference |
| ---- | ------------------ | ------------------- |
| `azdo` | Prints `AB#763` | `REF_ISSUE_NUM` (first `AB#N`, otherwise first `#N`), unchanged |
| `other` | `local-issue-763` | Used by `derive_local_tracking_id`, unchanged |
| not a git repository | `local-issue-763` | Used by `derive_local_tracking_id`, unchanged |

## Caller overrides

A caller can fix the host type and turn off issue creation through two recipe
context keys (issue #1887). Both are declared in `workflow-prep.yaml`,
`default-workflow.yaml` and `smart-orchestrator.yaml`, with an empty default.
They are read only from recipe context (`-c` or a parent recipe), never from
the task description or the requirements text.

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1887 on rysweet/amplihack-rs" \
  -c issue_number=1887 \
  -c create_issue=false
```

```bash
amplihack recipe run default-workflow \
  -c task_description="Tidy the release notes script" \
  -c remote_host_type=other
```

### `remote_host_type`

`step-02d-detect-remote-host` trims the value and folds it to lower case
under `LC_ALL=C`, then checks it before any `git` call.

| Value after trimming and folding | Result |
| -------------------------------- | ------ |
| `github`, `azdo`, `other` | Kept. stderr gets `INFO: Remote host type: <v> (caller-provided)`; stdout is `<v>`; `git remote get-url` is not run |
| `azure-devops` | Normalised to `azdo`, then kept as above |
| empty or unset | Detected from `origin` as before; output is byte-identical to earlier releases |
| anything else | stderr gets `WARN: unrecognised remote_host_type ignored; detecting from origin (accepted: github\|azdo\|azure-devops\|other)`, then detection runs |

The check runs before the "not a git repository" branch, so a valid caller
value is also kept outside a repository. Without one, `other` remains the
fallback there.

The WARN line never repeats the value. It lists the accepted values. Values
with newlines, NUL bytes or look-alike Unicode characters never match the
allowed list and take the WARN path. The remote URL is never printed.

Step-03 still checks the host type it receives against `github`, `azdo` and
`other`, so a value that bypasses step-02d is handled as before.

### `create_issue`

`create_issue` is an input. It is unrelated to step-03's `issue_creation` output.

`issue_creation_disabled` in
`amplifier-bundle/tools/workflow_issue_tracking.sh` decides whether step-03
may create an issue or work item.

| Value after trimming and folding | Result |
| -------------------------------- | ------ |
| `false`, `0`, `no`, `off` | Creation suppressed on every host |
| `true`, `1`, `yes` | Creation allowed |
| empty or unset | Creation allowed; behaviour is unchanged |
| anything else | Creation allowed; stderr gets a fixed WARN that does not repeat the value |

So `False`, `NO` and ` off ` all suppress creation, while `falsey` and `o ff`
do not.

When creation is suppressed, step-03 prints
`INFO: create_issue=false; issue creation disabled, using local tracking` to
stderr, writes the local metadata block (with a `tracking_reference=local-…`
line) to stdout, and exits 0. Step-03b reads that block as it does for
`other` hosts.

### Order of precedence in step-03

1. An explicit `issue_number`. On GitHub it goes through
   [`select_github_tracking_issue`](#select_github_tracking_issue); on AzDO a
   numeric value is printed as `AB#N` without any `az` call.
2. Read-only reuse of a reference in the task text (GitHub candidates; the
   AzDO `REF_ISSUE_NUM` lookup with `az boards work-item show`).
3. `create_issue=false`: local tracking.
4. The existing host logic: title search and `gh issue create` on GitHub,
   `az boards work-item create` on AzDO.

An explicit `issue_number` therefore takes precedence over
`create_issue=false`. Nothing is created, and the explicit-number paths in
[Explicit issue number](#explicit-issue-number) and
[Azure DevOps and local tracking](#azure-devops-and-local-tracking) apply
unchanged. On GitHub this includes the fallback to local metadata when
`gh issue view` fails or the number resolves outside this repository.

### Behaviour by host with `create_issue=false`

| Host | Still runs | Never runs |
| ---- | ---------- | ---------- |
| `github` | `gh issue view`, `gh api repos/{owner}/{repo}/issues/N` | `gh issue list --search`, `gh label create`, `gh issue create`, the temporary file for the issue body |
| `azdo` | `az boards work-item show` (not when `issue_number` is numeric, which prints `AB#N` directly) | `az account show`, `az ad signed-in-user show`, `az boards work-item create`, the temporary file for `az` errors |
| `other` | nothing new; tracking was already local | any `gh` or `az` call |

Each gate sits before the first command in its branch that creates data or
writes the task description to a temporary file.

`create_issue=false` does not make the run offline. The read-only lookups in
steps 1 and 2 above still call `gh` or `az` with the caller's credentials.
Callers that want no provider calls at all should pass
`remote_host_type=other`.

### Interaction with issue-creation opt-in (#1885)

Issue #1881 and pull request #1885 add an opt-in for issue creation with a
default of `create_issue: "false"`. Once that default is in place, the gate
described here applies it to every host, so Azure DevOps work items are also
no longer created unless the caller passes `create_issue=true`. On GitHub,
#1885's `issue_creation_opted_in` check sits at the same point in step-03 as
this gate.

### Caller override examples

| `remote_host_type` | `create_issue` | `issue_number` | Origin | Outcome |
| ------------------ | -------------- | -------------- | ------ | ------- |
| `other` | unset | unset | GitHub | step-03 receives `other`; local metadata; no `gh` call |
| unset | `false` | unset | GitHub | host `github`; no title search or creation; local metadata |
| `azdo` | `false` | unset | Azure DevOps | no `az boards work-item create`; local metadata |
| unset | `false` | `42` | GitHub | issue #42 reused if `gh issue view` confirms it, otherwise local metadata for #42; nothing created |
| ` Azure-DevOps ` | unset | unset | GitHub | host `azdo`; org and project parsing fails, so step-03 uses local tracking |
| `gitlab` | unset | unset | GitHub | WARN, then host `github` from detection |

## Known limitations

- **Incidental open issues can be reused.** When no marked reference passes,
  a bare `#N` that is an open issue in this repository is still reused. A task
  that mentions an unrelated open issue in passing can attach to it, and
  step-03c does not catch this. Use `issue_number` or a marker such as
  `Issue #N` to avoid it.
- **A bare-selected issue can be closed on merge.** On GitHub, `step-15-commit-push` writes
  `Closes #N` into the commit message when the diff contains more than planning
  documents (`workflow_issue_reference.sh --keyword`). If a bare `#N` selected
  an unrelated open issue, merging the pull request closes that issue. A
  follow-up proposes using `Refs #N` for bare selections.
- **Lookup latency.** Each lookup has a 30-second timeout, so five failing
  lookups can delay the create path by up to 150 seconds. Normal lookups
  take well under a second.
- **API failure creates an issue.** If `gh api` fails for every candidate
  because of authentication, scope or rate limits, the run creates a new issue
  after the title search, which may duplicate an existing one.
- **Fallback paths outside this selection.** The step-03b last-resort fallback
  still takes the first `#N` in the task text, and `derive_local_tracking_id`
  still reads task-text numbers on the local and repository-failure paths.
  Neither is reached on a GitHub path that prints an `issues/N` URL.
- **A caller-provided `github` follows `gh`'s repository choice.** With
  `remote_host_type=github` on a repository whose `origin` is not on GitHub,
  `gh` resolves the repository itself and may pick another remote, such as a
  public upstream fork parent, and create the issue there. Pass
  `create_issue=false` when the target repository is uncertain.
- **A caller-provided `azdo` on a GitHub origin** cannot parse an organisation
  and project, so step-03 falls back to local tracking.

## Tests

- `tests/integration/issue_1595_tracking_issue_selection_test.rs` runs the
  shipped step-03 and step-03b bodies against a fake `gh` that logs every
  call. Each `gh api` lookup is answered from `GH_API_<N>=open|closed|pr|transferred|nohttps|garbage`;
  an unset variable gives a 404. `GH_VIEW_EXIT` makes `gh issue view` fail
  with a given exit code.
  The candidate cases include `abc#12` (no candidate), `issue#5` and
  `Resolves: #30` (both marked), and `##1592` as an explicit value (rejected).
- `tests/integration/issue_684_host_aware_workflow_test.rs`
  (`step_03_preserves_github_path`) checks that step-03 calls
  `select_github_tracking_issue` and that the helper contains `gh issue view`
  and `gh api`.
- `tests/integration/issue_815_804_local_tracking_extract_test.rs` and
  `tests/integration/issue_815_804_local_tracking_scope_args_test.rs` check
  that the local metadata format and `derive_local_tracking_id` still behave
  as before, apart from the anchored pattern.
- `tests/integration/workflow_prep_requirements_sanitizer_test.rs` checks that
  `workflow-prep` still filters launcher preference notices out of the task
  requirements after step-03 is edited.
- `tests/integration/issue_1887_caller_host_type_create_issue_test.rs` runs
  step-02d and step-03 under `isolated("bash")` with fake `git`, `gh` and `az`
  binaries that log every call and exit 7 on unexpected ones. It chains
  step-02d's stdout into step-03 as `RECIPE_VAR_remote_host_type`, as the
  runner does, and covers caller values, the `azure-devops` alias, the WARN
  fallback (which never repeats the value), `create_issue=false` on GitHub and AzDO, the
  accepted spellings, and `issue_number` taking precedence. It also checks
  that no remote URL or token reaches stdout or stderr. To prove precedence
  on AzDO, it chains `step_02d_normalises_azure_devops` into step-03 with
  `issue_number` set, because step-03 matches only `azdo)`.
  The unit tests `issue_creation_disabled_rejects_other_values` and
  `unrecognised_create_issue_warns_without_echo` cover the `create_issue`
  predicate on its own.
- `tests/issue_1423_bash4_constructs.sh` checks the helper for bash 4 constructs.
- `scripts/check-brick-budget.sh` checks that `workflow-prep.yaml` stays within
  the phase sub-recipe line limit after the step-03 change.

Run them with:

```bash
cargo test --test integration issue_1595
cargo test --test integration issue_1887
cargo test --test integration issue_684
cargo test --test integration issue_815_804
cargo test --test integration workflow_prep_requirements_sanitizer
bash tests/issue_1423_bash4_constructs.sh
scripts/check-brick-budget.sh
```

## Related documentation

- [Step 03 Host-Aware Tracking Idempotency](recipe-step-03-idempotency.md)
- [Issue Deduplication](issue-dedup.md) - how recipes avoid duplicate GitHub issues, including the title search that `create_issue=false` skips
- [Workflow Issue Extraction Reference](workflow-issue-extraction.md) - how step-03b turns the step-03 output into `issue_number`
- [Local Tracking Issue-Number Extraction](../recipes/issue-815-804-local-tracking-issue-number.md) - the local metadata format used when an explicit lookup fails
