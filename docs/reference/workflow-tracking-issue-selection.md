# Tracking Issue Selection Reference (Issues #1595, #1881)

> [Home](../index.md) > Reference > Tracking Issue Selection

How `workflow-prep` `step-03-create-issue` chooses the tracking issue for a
run on a GitHub repository, when it creates a new GitHub issue, and how that
choice reaches `step-03b`.

## Contents

- [Summary](#summary)
- [Selection order](#selection-order)
- [Explicit issue number](#explicit-issue-number)
- [Candidates from the task description](#candidates-from-the-task-description)
- [Candidate validation](#candidate-validation)
- [Local tracking by default](#local-tracking-by-default)
- [Exact-title reuse](#exact-title-reuse)
- [Bounded issue creation](#bounded-issue-creation)
- [Helper function contract](#helper-function-contract)
- [Issue creation helper contract](#issue-creation-helper-contract)
- [Diagnostics](#diagnostics)
- [Behaviour by example](#behaviour-by-example)
- [Azure DevOps and local tracking](#azure-devops-and-local-tracking)
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
3. otherwise returns without output on stdout.

What step-03 does next depends on the `create_issue` context value (#1881):

- **Unset, `false` or anything other than `true`/`1`/`yes`** (the default):
  step-03 uses [local tracking](#local-tracking-by-default). It makes no
  `gh issue list`, `gh label create` or `gh issue create` call.
- **`true`, `1` or `yes`:** step-03 reuses an open issue with
  [exactly the same title](#exact-title-reuse) if one exists. Otherwise it
  [creates one](#bounded-issue-creation), waiting at least one minute plus
  jitter on a secondary rate limit and giving up after a bounded time. If
  step-03 cannot check for an existing issue, it uses local tracking rather
  than create one blind.

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
| 4 | `create_issue` is not true | Always | None; local tracking metadata |
| 5 | Open issue in this repository whose title equals the normalised task title | Exact string match | `gh issue list --search` plus a `jq` equality check |
| 6 | New issue | No match found | `gh issue create`, bounded retries |

Orders 1–3 run whatever `create_issue` says, so an explicit `issue_number`
or a validated `#N` is reused without the opt-in.

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
function logs the numbers it checked on stderr, and step-03 moves on to the
`create_issue` check. When `gh api` itself exits
non-zero (a timeout, 404, auth or rate-limit failure), stderr also gets
`WARN: could not verify candidate #N (gh exit RC)`, with gh's own stderr still
discarded; closed issues, PRs and bad URLs are rejected without a warning.

## Local tracking by default

When nothing was selected and `create_issue` is not true, step-03 prints one
line on stderr, writes the local metadata block and exits 0:

```text
INFO: no tracking issue selected and create_issue is not set; using local tracking (pass -c create_issue=true to create a GitHub issue)
tracking_system=local
tracking_reference=local-3f9a1c0b7d2e
tracking_issue=local-3f9a1c0b7d2e
issue_creation=local-tracking
```

The reference comes from `derive_local_tracking_id`: a number in the task
text gives `local-issue-N`, otherwise a 12-character hash of the task text.
Step-03b parses the block as it does on every other local path.

### Opting in

Pass `create_issue` as recipe context:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add a --json flag to recipe list" \
  -c create_issue=true
```

| `create_issue` value | Meaning |
| -------------------- | ------- |
| `true`, `TRUE`, `1`, `yes`, ` Yes ` | Opt in. Surrounding whitespace is trimmed and case is ignored |
| unset, empty, `false`, `0`, `no` | Local tracking |
| `on`, `y`, `enabled`, any other text | Local tracking (fails closed) |

`create_issue: "false"` is declared in the context of `workflow-prep.yaml`,
`default-workflow.yaml` and `smart-orchestrator.yaml`. Sub-recipes receive the
value through context merging, the same way they receive `issue_number`.

A decomposed run hands the value to its parallel workstreams as well:
`launch-parallel-round-1` in `smart-execute-routing.yaml` exports it as
`AMPLIHACK_CREATE_ISSUE`, and multitask copies a non-empty value into each
workstream's `create_issue` context (`build_resume_context` in
`crates/amplihack-cli/src/commands/multitask/command_builder.rs`). Each
workstream then applies the same opt-in, exact-title reuse and backoff. When
the variable is unset, workstreams use local tracking.

Step-03 reads the value only from `RECIPE_VAR_create_issue`. Text in the task
description or requirements such as "create_issue=true" or "please open an
issue" has no effect.

> **Privacy.** With `create_issue=true`, the task description and the
> requirements are published in the body of a GitHub issue. On a public
> repository anyone can read them. The local default keeps them on the
> machine.

## Exact-title reuse

With the opt-in on, step-03 first looks for an open issue with the same
title. This replaces the old fuzzy `--search` match, which reused the first
search hit even when its title was different.

### Title normalisation

Step-03 builds one title and uses it for both the lookup and `--title`:

1. CR and LF become spaces.
2. Other control characters are removed.
3. The result is cut to 200 characters.
4. Leading and trailing whitespace is trimmed.

GitHub trims the titles it stores, so step 4 is what lets a title that ends
in a space after the cut match its own earlier issue.

### Lookup

```bash
GH_PROMPT_DISABLED=1 timeout 30 gh issue list --state open --limit 100 \
  --search='"Add a --json flag to recipe list" in:title' \
  --json number,title,url </dev/null
```

The search phrase is the title with double quotes removed and each backslash
replaced by a space, wrapped in quotes
and followed by `in:title`, so search qualifiers in the task text are
treated as words.

`REPO` comes from `load_issue_identity`, which step-03 calls once before the
first lookup. It stores it in `ISSUE_REPO_NWO` (cleared when the helper is
sourced, so a value from the environment is never used), and every later
lookup in the run reuses it:

```bash
GH_PROMPT_DISABLED=1 timeout 30 gh repo view --json nameWithOwner --jq .nameWithOwner </dev/null
```

If the call fails or prints a value of the wrong form, the lookup does not
run `gh issue list` and returns 2. A later lookup tries the call again.

The results are then filtered with `jq -r --arg t "$TITLE" --arg r "$REPO"`
and this program:

```jq
[ .[]
  | select(.title == $t)
  | select((.url | split("/")) as $p
      | ($p | length) == 7 and $p[0] == "https:" and $p[1] == ""
        and ($p[2] | test("\\A[A-Za-z0-9.-]+\\z")) and $p[5] == "issues"
        and ($p[6] | test("\\A[0-9]+\\z"))
        and (($p[3] + "/" + $p[4]) | ascii_downcase) == ($r | ascii_downcase))
][0].url // empty
```

Every result is checked, not only the first one with the right title. A
match must:

- have a title equal to the normalised title, character for character;
- have a URL of the form `https://<host>/<owner>/<repo>/issues/<N>` with no
  newline or other stray characters;
- belong to the current repository: `<owner>/<repo>` equals `REPO`, ignoring
  case.

Who opened the issue is not checked. The requirement in #1881 is an open
issue with the same title in the same repository, and the gh account on a
host can change between two runs of the same task; an author check would
make the second run create a duplicate. It would also need `gh api user`,
which a GitHub App installation token cannot call. An unrelated issue is
reused only if someone opens one whose title is identical, character for
character, to the title generated from the task description.

A result that fails the URL or repository check is skipped, and the next
result with the same title is tried. `gh issue list` never returns pull
requests, and `--state open` excludes closed issues.

The lookups that run during backoff, before each retry of a rate-limited
create, call `find_open_issue_by_exact_title TITLE recent`. That form drops
`--search` and lists the 100 newest open issues:

```bash
GH_PROMPT_DISABLED=1 timeout 30 gh issue list --state open --limit 100 \
  --json number,title,url </dev/null
```

GitHub search indexes new issues after a delay, so a create that GitHub
accepted but reported as rate-limited could otherwise be missed and the
retry would make a duplicate. A plain listing includes the new issue at once.
The same `jq` filter is applied.

When the lookup fails, `find_open_issue_by_exact_title` prints
`WARNING: exact-title lookup failed; <reason>` on stderr itself before it
returns 2, followed by the failed `gh` call's stderr passed through
`sanitize_cli_output`.

Before the first create, step-03 calls `find_open_issue_before_create`. It
runs the search lookup and, only if that fails, the recent listing. The
search API has a much smaller per-minute quota than the core API, so it is
the call most likely to fail when many runs start at once; the recent
listing uses the core quota. A duplicate from a parallel run of the same task
would be among the newest 100 open issues. If both lookups fail, step-03
prints

```text
WARNING: could not check for an open issue with this title; not creating one blind (a duplicate is worse than local tracking); using local tracking
```

and writes the local metadata block. The duplicates in #1881 came from
separate runs of the same task, so a create that skips the check could
duplicate another run's issue even though this run has created nothing.

| Lookup result | Before the first create | Before a retry |
| ------------- | ----------------------- | -------------- |
| Match | URL printed, exit 0, no create | URL printed, exit 0, no further create |
| No match | Create | Retry the create |
| Lookup failed (gh error, timeout, bad JSON, `repo view` failed) | Search failed: try the recent listing. Both failed: local tracking, no create | No create this round; wait again (see below), then look up again |

A retry only follows a lookup that succeeded and found nothing. When the
lookup before a retry fails, that round makes no `gh issue create` call. The
helper waits the next delay instead: the last `retry-after` value if the
failed create gave one, otherwise the next jittered delay for a secondary
limit, or the next reset wait for a primary limit. That wait counts against
the same budget and is checked the same way as any other, so a run of failed
lookups also ends in [giving up](#giving-up).

A create that
GitHub accepted but reported as failed is therefore found and reused, not
created a second time.

## Bounded issue creation

`create_github_issue_bounded` in
`amplifier-bundle/tools/workflow_issue_create.sh` runs one
`gh label create workflow:default` call, then the create loop. Each create is:

```bash
GH_PROMPT_DISABLED=1 timeout 60 gh issue create --title="$TITLE" \
  --body-file="$BODY_FILE" --label=workflow:default </dev/null
```

### Error classes

The output of a failed create is classified in this order. The first match
wins.

| Class | Output contains | Action |
| ----- | --------------- | ------ |
| `auth` | `HTTP 401`, a standalone `401`, `bad credentials`, `requires authentication`, `not logged in` | `ERROR`, exit 1. No retry, no sleep, no lookup |
| `secondary` | `secondary rate limit`, `abuse detection` or `submitted too quickly` (any case), with or without an HTTP status. This covers gh's GraphQL messages such as `GraphQL: was submitted too quickly (createIssue)` | [Jittered backoff](#secondary-rate-limit-backoff) |
| `primary` | `api rate limit`, `rate limit already exceeded`, `rate limit exceeded`, or a `429` without secondary-limit text | [Wait for the reset](#primary-rate-limit), same time budget |
| `not_found` | `Could not resolve to a Repository`, `repository not found`, `repository … inaccessible`, `HTTP 404` | `WARNING`, local metadata, exit 0 |
| `label` | `could not add label`, or `label` followed on the same line by `not found` or `does not exist`. Other output that mentions a label, such as `HTTP 422: Validation Failed (Label)`, is `other` | One more create without `--label` |
| `other` | Anything else, including a bare `403` or a permission `403` | `ERROR`, exit 1 |

### Create timed out

If `timeout 60` stops `gh issue create` (exit code 124), GitHub may still have
created the issue. The helper runs one `find_open_issue_by_exact_title TITLE
recent` lookup. If it finds the issue, the helper prints its URL and returns
0. Otherwise it prints
`ERROR: GitHub issue creation failed. gh issue create timed out after 60s and no open issue with this title was found.`
and returns 1. It does not retry the create.

The create without `--label` runs at most once per run and only for the
`label` class. A rate-limited or auth-failed create never triggers it, so
each attempt is exactly one `gh issue create` call.

### Secondary rate limit backoff

| Variable | Default | Meaning |
| -------- | ------- | ------- |
| `AMPLIHACK_ISSUE_BACKOFF_SECONDARY_MIN_SECS` | `60` | Floor added to every secondary-limit wait that has no `retry-after` |
| `AMPLIHACK_ISSUE_BACKOFF_BASE_SECS` | `5` | Starting delay of the jitter added to the floor |
| `AMPLIHACK_ISSUE_BACKOFF_MAX_DELAY_SECS` | `60` | Cap on one computed delay |
| `AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS` | `300` | Total wait budget for the run |

Each must be a positive integer of at most six digits
(`^[1-9][0-9]{0,5}$`). An unset or empty value uses the default. Any other
value, including `0`, also uses the default and prints
`WARNING: ignoring invalid NAME (N characters); using default D` to stderr.
Only the length is printed, so a token put in one of these variables by
mistake does not reach the log.

GitHub's guidance for a secondary rate limit without a `retry-after` header
is to wait at least one minute before retrying, then back off exponentially;
retrying sooner can extend the restriction. For backoff round *n* (starting
at 0) the wait is the floor plus an "equal jitter" delay:

```text
d    = min(MAX_DELAY, BASE * 2^n)        # exponent capped at 30
j    = d/2 + RANDOM % (d/2 + 1)          # at least 1
wait = SECONDARY_MIN + j
```

With the defaults, round 0 waits 62–64 s, round 1 65–70 s, round 2 70–80 s,
round 3 80–100 s and round 4 onward 90–120 s. The jitter spreads parallel
workers apart. Within the 300 s budget a worker that stays limited makes
at most five creates, usually four, and then
[gives up](#giving-up) to local tracking.

If the output contains `retry-after: N` (any case, 1–6 digits), the wait is
`N` seconds instead. It is neither jittered nor capped by `MAX_DELAY`, because
GitHub requires at least that long. gh's error output usually leaves out
response headers, so this applies only when `retry-after:` text appears in
that output; in most runs the floor plus jitter is what applies. A
`retry-after: 0` is raised to 1. The
Retry-After wait still counts against `MAX_TOTAL`. It is never shortened to
fit: if `N` is more than the budget that is left, the helper gives up at once
without sleeping.

Each wait is logged:

```text
WARNING: gh issue create hit a GitHub secondary rate limit; backing off 68s (round 1, 63/300s used)
```

### Primary rate limit

The wait is the GraphQL reset time from `gh api rate_limit` plus 3 s, at
least 5 s. If `gh api rate_limit` fails or returns a reset epoch that is not
1–12 digits, the wait is 5 s and
`WARNING: could not read GitHub rate-limit reset; waiting 5s` goes to stderr,
followed by gh's sanitised stderr when the call failed. The wait counts against the same
`AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS` budget. If the reset is further away
than the budget that is left, the step gives up at once without sleeping.

### Giving up

Time is the sum of planned waits, not wall-clock time. Before each wait the
helper checks `used + wait <= MAX_TOTAL`. If the next wait would exceed the
budget, it does not sleep. It prints:

```text
WARNING: GitHub issue creation gave up after 287s (AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=300); falling back to local tracking.
```

then writes the local metadata block and exits 0. Step-03b reads it as it
reads any local tracking block, and the workflow carries on. Total sleep in
step-03 is never more than `MAX_TOTAL`. There is no unbounded loop.

### Shortening the budget

To fall back to local tracking sooner on a busy account, lower the total
budget. A budget below the 60 s floor gives up on the first secondary limit
without sleeping:

```bash
AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=30 \
amplihack recipe run default-workflow \
  -c task_description="Add a --json flag to recipe list" \
  -c create_issue=true
```

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
| `1` | Empty | Nothing selected; step-03 checks `create_issue` |

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

## Issue creation helper contract

`amplifier-bundle/tools/workflow_issue_create.sh` is sourced by step-03 from
the same directory as `workflow_issue_tracking.sh`. If it is missing, step-03
prints `ERROR: issue creation helper not found: …/workflow_issue_create.sh`
and exits 2. Its header lists every function.

The file sets no shell options and installs no `trap`. It works under the
step's `set -euo pipefail`. Diagnostics go to stderr; stdout carries only a
URL or the metadata block. It uses no `eval` or `sh -c`, and every user value
reaches `gh` as one argument in `--flag=value` form. Every `gh` call runs as
`GH_PROMPT_DISABLED=1 timeout N gh … </dev/null` (30 s for list, label,
`rate_limit` and `repo view`; 60 s for create), so a login prompt can never
stall the step. All `gh` output that reaches stderr goes through
`sanitize_cli_output`. The issue body is never logged.

### `issue_creation_opted_in VALUE`

Returns 0 when `VALUE`, trimmed and lower-cased, is `true`, `1` or `yes`.
Returns 1 otherwise. No output.

### `load_issue_identity`

Sets `ISSUE_REPO_NWO` (`owner/repo`, from `gh repo view`) when it is empty,
and returns 0. When the call fails or prints a value of the wrong form, it
prints `WARNING: could not determine this repository (repo view exited RC)`
and gh's sanitised stderr, and returns 1. Call it in the step's own shell,
not in `$(…)`, or the value is lost when the subshell exits. Step-03 calls it
once before the first lookup and `create_github_issue_bounded` calls it again
(a no-op when the value is set), so a run makes one `gh repo view` call
however many retries it makes. The helper never calls `gh api user`.

### `find_open_issue_by_exact_title TITLE [recent]`

Calls `load_issue_identity` first, so it also works on its own.
Without a second argument the lookup uses `--search`. With `recent` it lists
the 100 newest open issues instead; step-03 uses this form during backoff.

| Return code | stdout | Meaning |
| ----------- | ------ | ------- |
| `0` | One issue URL | Open issue in this repository with exactly this title, whoever opened it |
| `1` | Empty | No match |
| `2` | Empty | The lookup failed: gh error or timeout, output not a JSON array, or `load_issue_identity` failed. The function itself prints `WARNING: exact-title lookup failed; <reason>` on stderr |

Return codes 1 and 2 print nothing on stdout. Callers must not treat them
alike: 2 means nothing is known about existing issues.

### `find_open_issue_before_create TITLE`

The lookup step-03 makes before its first create. It calls
`find_open_issue_by_exact_title TITLE` and, when that returns 2, prints
`INFO: exact-title search failed; checking the newest open issues instead`
and calls `find_open_issue_by_exact_title TITLE recent`. It returns the last
lookup's code and URL. Step-03 creates only on 1; on 2 it uses local
tracking.

### `classify_issue_create_error OUTPUT`

Prints one of `auth`, `secondary`, `primary`, `not_found`, `label`, `other`
(see [Error classes](#error-classes)). Reads at most the first 65536 bytes.
`OUTPUT` is only ever matched against fixed patterns, never used as one.

### `create_github_issue_bounded TITLE BODY_FILE LABEL`

| Return code | stdout | Meaning |
| ----------- | ------ | ------- |
| `0` | One issue URL | Created, or found by a lookup before a retry or after a timed-out create |
| `0` | Local metadata block | `not_found`, or gave up after the budget |
| `1` | Empty | `auth` or `other`, or a timed-out create whose issue was not found; `ERROR` and the sanitised gh output on stderr |

After a successful create, only the last line of gh's output that is a valid
issue URL is printed.

### Internal helpers

| Function | Purpose |
| -------- | ------- |
| `_issue_backoff_int NAME DEFAULT` | Reads an `AMPLIHACK_ISSUE_BACKOFF_*` value, or the default when it is not `^[1-9][0-9]{0,5}$` (with a WARNING when it was set) |
| `_issue_retry_after OUTPUT` | Prints `N` from `retry-after: N` (1–6 digits), or nothing |
| `_issue_jitter_delay ROUND BASE MAX_DELAY` | Prints the equal-jitter delay for a round, at least 1 |
| `_issue_secondary_wait ROUND BASE MAX_DELAY FLOOR` | Prints `FLOOR` plus the jitter delay: the wait for a secondary limit without `retry-after` |
| `_issue_wait_for_class CLASS OUTPUT ROUND BASE MAX_DELAY [FLOOR]` | Prints the planned wait for one round: `retry-after` when printed, otherwise the secondary wait or the primary reset wait |
| `_issue_primary_wait` | Prints the wait until the GraphQL reset plus 3 s, at least 5 |
| `_issue_gh_stderr FILE RC` | Prints a failed gh call's stderr through `sanitize_cli_output`, then deletes the file |

Step-03 calls the helpers like this:

```bash
issue_creation_opted_in "${RECIPE_VAR_create_issue:-}" || { echo "INFO: …" >&2; emit_local_metadata; exit 0; }
load_issue_identity || true
rc=0; FOUND_URL=$(find_open_issue_before_create "$ISSUE_TITLE") || rc=$?
[ "$rc" -eq 0 ] && { printf '%s\n' "$FOUND_URL"; exit 0; }
[ "$rc" -eq 1 ] || { echo "WARNING: could not check …; using local tracking" >&2; emit_local_metadata; exit 0; }
create_github_issue_bounded "$ISSUE_TITLE" "$ISSUE_BODY_FILE" "workflow:default"; exit $?
```

## Diagnostics

All diagnostics go to stderr. stdout carries only the URL or the metadata
block. When a `gh` call made by the creation helper fails, its stderr follows
the warning, passed through `sanitize_cli_output`; successful calls print
nothing. The task text is never logged.

| Message | When |
| ------- | ---- |
| `INFO: tracking issue #1592 selected (explicit)` | Explicit number resolved |
| `INFO: tracking issue #1595 selected (marked)` | Marked candidate accepted |
| `INFO: tracking issue #20 selected (bare)` | Bare candidate accepted |
| `INFO: no task-text candidate accepted as an open issue (checked: 1595 1578); falling back to title search` | Every candidate was rejected; step-03 checks `create_issue` next |
| `INFO: no tracking issue selected and create_issue is not set; using local tracking …` | Default path; no GitHub write |
| `INFO: reusing open issue with identical title: https://…/issues/N` | Exact-title lookup matched |
| `WARNING: exact-title lookup failed; …` | Lookup returned 2. Before the first create a failed search is followed by the recent listing; during backoff the helper waits |
| `INFO: exact-title search failed; checking the newest open issues instead` | Search lookup failed before the first create |
| `WARNING: could not check for an open issue with this title; not creating one blind …; using local tracking` | Both lookups before the first create failed; no create |
| `WARNING: gh issue create hit a GitHub secondary rate limit; backing off Ns (round R, U/Ms used)` | Secondary limit wait |
| `WARNING: gh issue create hit the GitHub API rate limit; waiting Ns for reset (U/Ms used)` | Primary limit wait |
| `WARNING: ignoring invalid NAME (N characters); using default D` | An `AMPLIHACK_ISSUE_BACKOFF_*` value was set but invalid; the value itself is not printed |
| `WARNING: could not determine this repository (repo view exited RC)` | `gh repo view` failed; the lookup returns 2 |
| `INFO: gh issue create timed out but the issue exists; reusing https://…/issues/N` | Create timed out; the lookup found the issue |
| `INFO: gh label create workflow:default exited RC (the label may already exist); continuing` | Label create failed; gh's sanitised stderr follows |
| `WARNING: could not read GitHub rate-limit reset; waiting 5s` | Primary limit, but the reset time was unreadable |
| `WARNING: GitHub issue creation gave up after Us (AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=M); falling back to local tracking.` | Budget used up |
| `ERROR: GitHub issue creation failed.` | `auth` or `other` class; sanitised gh output follows |
| `ERROR: GitHub issue creation failed. gh issue create timed out after 60s and no open issue with this title was found.` | Create timed out and the lookup did not find the issue |
| `WARN: could not verify issue #1592 (gh exit 124); falling back to local tracking` | `gh issue view` failed; the exit code tells a timeout (124) from an auth, rate-limit or network failure. gh's own stderr is not shown |
| `WARN: explicit issue_number #1592 could not be resolved as an issue in this repository ...` | `gh issue view` succeeded but did not return this repository's issue URL |
| `WARN: ...` with fixed text, no value | `issue_number` was not numeric |

Search the step-03 log for `selected (bare)` to find runs that attached to an
issue mentioned only in passing, and for `no task-text candidate accepted` to
find runs that rejected every reference. Search for `gave up after` to find
runs that asked for an issue but ended on local tracking.

## Behaviour by example

Scenarios run against a GitHub repository where #1592 and #1595 are open
issues, #1578 and #42 are closed issues, #161 and #1585 are merged pull
requests, #162 does not exist, and open issue #1700 is titled
`Add a --json flag to recipe list`. `create_issue` is unset unless the row
says otherwise.

| `issue_number` | Task text | Tracking issue |
| -------------- | --------- | -------------- |
| `1592` | `See PR #1585 and #1578 for background.` | #1592 |
| unset | `Issue #1595: fix selection. See #1578.` | #1595 |
| unset | `Fixes #42` (closed) | local tracking |
| unset | `Fixes #42` (closed), `create_issue=true` | new issue |
| unset | `Fixes #161` (PR), `create_issue=true` | new issue |
| unset | `Follows #1585.` (PR) | local tracking |
| unset | `Related to #1578.` (closed) | local tracking |
| unset | `See #1592 first. Fixes #1595.` | #1595 |
| unset | `PR #1592, rysweet/amplihack-rs#1592, AB#1592`, `create_issue=true` | new issue; no `gh api` lookup |
| unset | `Upgrade the runner; runner fixes #161 and #162.`, `create_issue=true` | new issue; branch is not `feat/issue-161-...` |
| unset | `Add a --json flag to recipe list` | local tracking; no `gh` write |
| unset | `Add a --json flag to recipe list`, `create_issue=true` | #1700 (exact title) |
| unset | `Add a --json flag to recipe list.`, `create_issue=true` | new issue (title differs by the full stop) |
| unset | `Add a --json flag`, `create_issue=true`, first create gets a secondary limit, lookup then finds the issue | the found issue; one create only |
| unset | `Add a --json flag`, `create_issue=true`, every create gets a secondary limit | local tracking after the budget |
| unset | `Add a --json flag`, `create_issue=true`, create returns `HTTP 401` | step fails, exit 1, no retry |

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
- **API failure creates an issue when opted in.** If `gh api` fails for
  every candidate because of authentication, scope or rate limits, and
  `create_issue` is true, the run goes on to the exact-title lookup and may
  create an issue that duplicates the one the task mentioned.
- **Search lag.** GitHub search indexes new issues after a short delay. The
  first lookup uses search, so an issue created seconds earlier by another
  run may not be found, and a duplicate may be created. Lookups during
  backoff list recent issues without search, so a create that GitHub
  accepted but reported as failed is always found before the retry.
- **An identical title opened by someone else is reused.** Reuse does not
  check the author, so an issue that another account opened with exactly the
  generated title becomes the tracking issue and can be closed by the
  `Closes #N` that step-15 writes. This needs a character-for-character match
  with the title built from the task description.
- **A failed lookup ends in local tracking.** When both the search and the
  recent listing fail before the first create, the opted-in run makes no
  issue. Search the log for `not creating one blind` to find such runs.
- **Older duplicates during a search outage.** When search fails, the
  fallback listing covers the 100 newest open issues only, so a same-title
  issue older than that is missed and a second one is created.
- **Only the first 100 results are compared.** A title whose search returns
  more than 100 open issues can miss its exact match.
- **Wall-clock time can exceed the budget.** The budget counts sleeps only.
  Each `gh` call adds up to its own timeout on top.
- **Fallback paths outside this selection.** The step-03b last-resort fallback
  still takes the first `#N` in the task text, and `derive_local_tracking_id`
  still reads task-text numbers on the local and repository-failure paths.
  Neither is reached on a GitHub path that prints an `issues/N` URL.

## Tests

- `tests/integration/issue_1881_local_default_tracking_test.rs` runs the
  shipped step-03 body with a fake `gh`, `git` and `sleep` first on `PATH`,
  `env_clear` and a temporary `HOME`; it never calls GitHub. The fake `gh`
  logs its argv, answers `issue list` from `GH_LIST_JSON`, plays scripted
  create responses and fails if `GH_PROMPT_DISABLED` is not set. The fake
  `sleep` logs its argument and returns at once. It covers the local default,
  the opt-in values, exact-title reuse (including an other-repository
  result with the same title that is skipped in favour of a later
  current-repository one, and a same-title issue by another account that is
  reused), one `gh repo view` call and no `gh api user` call per run across
  retries, a token that cannot read `/user`, a failed search followed by the
  recent listing, local tracking with no create when both first lookups
  fail, a timed-out create with and without a
  matching issue, sanitised gh stderr on failed lookups and label creates,
  a backoff warning that prints only the value's length,
  the lookup before each retry, a failed lookup
  during backoff that makes no create that round, the lookup-failure
  `WARNING` on stderr, the 401 path, the label fallback, secondary-limit
  delays (each the floor plus a jitter delay in `[1, cap]`, at least 60 s
  with the defaults, sum within the budget, not all equal), Retry-After
  (including one longer than the budget left, which gives up without
  sleeping), the give-up fallback, and
  static checks that step-03 has no `while` loop, that
  `launch-parallel-round-1` forwards `create_issue` to workstreams, calls
  `create_github_issue_bounded`, and stays within 400 lines.
- `tests/integration/issue_1595_tracking_issue_selection_test.rs` runs the
  shipped step-03 and step-03b bodies against a fake `gh` that logs every
  call. Each `gh api` lookup is answered from `GH_API_<N>=open|closed|pr|transferred|nohttps|garbage`;
  an unset variable gives a 404. `GH_VIEW_EXIT` makes `gh issue view` fail
  with a given exit code. Cases that expect a create set
  `RECIPE_VAR_create_issue=true`; one case checks the local default.
  The candidate cases include `abc#12` (no candidate), `issue#5` and
  `Resolves: #30` (both marked), and `##1592` as an explicit value (rejected).
- `tests/integration/issue_684_host_aware_workflow_test.rs`
  (`step_03_preserves_github_path`) checks that step-03 calls
  `select_github_tracking_issue` and `create_github_issue_bounded`, and that
  the helpers contain `gh issue view`, `gh api`, `timeout 30 gh label create`
  and `timeout 60 gh issue create`.
- `tests/integration/default_workflow_decomposition_test.rs`
  (`workflow_prep_step_03_has_github_path_inside_host_conditional`) checks
  that `create_github_issue_bounded` is called after `HOST_TYPE` and that
  `workflow_issue_create.sh` contains `gh issue create`.
- `tests/integration/issue_815_804_local_tracking_extract_test.rs` and
  `tests/integration/issue_815_804_local_tracking_scope_args_test.rs` check
  that the local metadata format and `derive_local_tracking_id` still behave
  as before, apart from the anchored pattern.
- `tests/integration/workflow_prep_requirements_sanitizer_test.rs` checks that
  `workflow-prep` still filters launcher preference notices out of the task
  requirements after step-03 is edited.
- `tests/issue_1423_bash4_constructs.sh` checks the helper for bash 4 constructs.
- `scripts/check-brick-budget.sh` checks that `workflow-prep.yaml` stays within
  the phase sub-recipe line limit after the step-03 change.

Run them with:

```bash
cargo test --test integration issue_1881
cargo test --test integration issue_1595
cargo test --test integration issue_684
cargo test --test integration issue_815_804
cargo test --test integration workflow_prep_requirements_sanitizer
cargo test --test integration default_workflow_decomposition
bash amplifier-bundle/recipes/tests/test-gh-rate-limit-backoff.sh
bash tests/issue_1423_bash4_constructs.sh
scripts/check-brick-budget.sh
```

## Related documentation

- [Step 03 Host-Aware Tracking Idempotency](recipe-step-03-idempotency.md)
- [Issue Deduplication Reference](issue-dedup.md) - how recipes avoid duplicate issues
- [Workflow Issue Extraction Reference](workflow-issue-extraction.md) - how step-03b turns the step-03 output into `issue_number`
- [Local Tracking Issue-Number Extraction](../recipes/issue-815-804-local-tracking-issue-number.md) - the local metadata format used when an explicit lookup fails
