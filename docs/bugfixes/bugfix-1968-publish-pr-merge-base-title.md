---
title: "Bug Fix #1968: merge-base diffs and issue-derived titles in the publish step"
type: explanation
issue: 1968
files:
  - amplifier-bundle/tools/workflow_publish_pr.sh
  - amplifier-bundle/recipes/workflow-publish.yaml
  - amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
---

# Bug Fix #1968: merge-base diffs and issue-derived titles in the publish step

> **Issue:** [#1968](https://github.com/rysweet/amplihack-rs/issues/1968)

The publish step (`amplifier-bundle/tools/workflow_publish_pr.sh`, step 16 of
`default-workflow`) builds the pull request title and body. It now measures the
branch against its merge base, reports the full number of changed files, and
takes the title from the issue rather than from a file-count template.

## Contents

- [Cause](#cause)
- [Diff range](#diff-range)
- [Changed-file count and body list](#changed-file-count-and-body-list)
- [Title sources](#title-sources)
- [Title processing](#title-processing)
- [Issue title lookup](#issue-title-lookup)
- [Summary line](#summary-line)
- [Azure DevOps path](#azure-devops-path)
- [Behaviour that is unchanged](#behaviour-that-is-unchanged)
- [Configuration](#configuration)
- [Helper functions](#helper-functions)
- [Examples](#examples)
- [Security considerations](#security-considerations)
- [Regression coverage](#regression-coverage)
- [Files changed](#files-changed)
- [Related](#related)

## Cause

Three defects combined.

1. **Two-dot diff.** The changed-file list and the diff stat came from
   `git diff "${BASE_REF}..HEAD"`. For `git diff`, two dots compare the tip of
   the base with `HEAD`. Once `main` moved on after the branch was cut, every
   file changed on `main` since the branch point appeared in the PR as if the
   branch had changed it. A branch that touched only `src/` could be titled
   after `.github` because a workflow file had landed on `main`.
2. **Count after truncation.** `CHANGED_COUNT` was counted from the output of
   `head -40`, so it could never exceed 40.
3. **Template title.** The title was always
   `Update <scope> with <N> changed files (#N)`, which said nothing about the
   change and repeated the wrong count.

## Diff range

The script computes `DIFF_RANGE` once, straight after `BRANCH_DIFF_STATUS`:

| Condition | `DIFF_RANGE` |
| --- | --- |
| `git merge-base "$BASE_REF" HEAD` succeeds | `${BASE_REF}...HEAD` (three dots: changes since the merge base) |
| No merge base (for example, a shallow clone) | `${BASE_REF}..HEAD`, with a warning on stderr |

The fallback warning reads:

```text
WARNING: no merge base between origin/main and HEAD (shallow clone?); listing changed files with the two-dot range origin/main..HEAD, which may include base-branch changes (#1968)
```

`DIFF_RANGE` feeds the changed-file list and the diff stat on both the GitHub
and the Azure DevOps paths. No other computation uses it.

## Changed-file count and body list

`CHANGED_FILES_ALL` is the complete output of
`git diff --name-only "$DIFF_RANGE"`. Nothing truncates it. It is the only input
to:

- `CHANGED_COUNT` (the number of non-empty lines)
- the #929 substantive-scope loop used by the template title
- the `## Risk` section of the body
- the `--keyword` input to `workflow_issue_reference.sh`, which chooses
  `Closes` or `Refs`

Only the displayed list is cut. `format_changed_files_body` reads the full list
on stdin and prints the `## Changed files` section content:

```text
45 files changed
- amplifier-bundle/recipes/file-01.yaml
- ...
- src/file-40.rs
- ... and 5 more
```

| Changed files | First line | Entries | Trailing line |
| --- | --- | --- | --- |
| 0 | none | `- No changed files detected by git diff` | none |
| 1 | `1 file changed` | 1 | none |
| 2 to 40 | `<N> files changed` | all | none |
| more than 40 | `<N> files changed` | first 40 | `- ... and <N-40> more` |

The `## Diff stat` section is `git diff --stat "$DIFF_RANGE" | tail -20`. Its
last line is git's own summary, which also reports the full total.

## Title sources

On the GitHub path the base title comes from the first source that yields
non-empty text:

| Order | Source | Prefixed and defused | Diagnostic |
| --- | --- | --- | --- |
| 1 | The issue title, read with `gh issue view` | yes | `PR title source: issue` |
| 2 | The first non-blank line of `RECIPE_VAR_task_description` | yes | `PR title source: task` |
| 3 | The template `Update <scope>` or `Update <scope> with <N> changed files` | no | `PR title source: template` |

The diagnostic goes to stderr and names the source only. It never prints the
title text. When the issue lookup yields nothing, the script also prints
`NOTE: no usable title from issue #<issue>; trying the task description (#1968)`
before falling back.

The template is the #929 template with the true count. It is reached only when
the issue lookup yields nothing and the task description is empty.

Every title ends with ` (#<issue>)`. The claim check in
`workflow_issue_claim_check.sh` uses that suffix to recognise the PR as the
claim on the issue, so the cap never removes it.

## Title processing

Sources 1 and 2 pass through four steps in this order. The template skips the
first three and goes straight to the cap.

### 1. Normalisation

`normalise_pr_title_text`:

1. keeps the first 1024 bytes of its input
2. removes credentials with `redact_credentials` (userinfo in `http(s)` URLs,
   `gh[pousr]_` tokens and `github_pat_` tokens); this is the same filter
   `sanitize_provider_stderr` uses
3. turns CR, LF and TAB into spaces
4. deletes the remaining C0 control bytes and DEL, which removes ESC and so any
   terminal escape sequence
5. collapses runs of spaces and trims both ends

Bytes from `0x80` upwards pass through unchanged.

### 2. Conventional type prefix

`conventional_pr_title` leaves text alone when it already starts with a
conventional prefix, that is when it matches `^[a-z]+(\([^)]*\))?!?: `.

Otherwise it lower-cases a copy and looks for whole words from the groups below,
in this order. The first group with a match supplies the prefix `<type>: `. If
no group matches, the text is returned unchanged.

| Order | Type | Words |
| --- | --- | --- |
| 1 | `fix` | fix, fixes, fixed, bug, bugs, regression, broken, wrong, wrongly, incorrect, incorrectly, crash, crashes, fails, failing |
| 2 | `docs` | docs, documentation, document |
| 3 | `test` | test, tests, testing |
| 4 | `refactor` | refactor, refactors, refactoring |
| 5 | `feat` | add, adds, implement, implements, support, introduce, introduces |

A whole word is bounded on each side by the start or end of the text or by a
character outside `[[:alnum:]_]`. So `address` does not match `add`, `latest`
does not match `test`, and `test_helper` does not match `test` either, because
`_` counts as part of the word.

No scope is inferred, and the original wording is kept. `Fix issue 929` becomes
`fix: Fix issue 929`.

### 3. Issue-reference defusal

The title and summary carry text that the workflow did not write.
`defuse_issue_refs` applies two rewrites, in this order, to stop that text from
acting on other issues:

| Order | Pattern in the text | Rewritten to | Why |
| --- | --- | --- | --- |
| 1 | `(#M)` | `#M` | The claim check treats `(#M)` in a PR title as a claim on issue M. |
| 2 | A closing keyword, an optional `:`, zero or more whitespace characters, then `#M`, `owner/repo#M` or an issues URL | the keyword and any `:`, one space, `issue `, then the reference | GitHub closes the referenced issue on merge when it sees this pattern in a PR body or a squash commit subject, and the claim check reads it as a claim. |

The second pattern mirrors the body-keyword test in
`workflow_issue_claim_check.sh`:

```text
(^|[^A-Za-z0-9_])(close[sd]?|fix(e[sd])?|resolve[sd]?):?[[:space:]]*(#|<owner/repo>#|https?://<host>/<owner/repo>/issues/)<M>
```

The keywords are close, closes, closed, fix, fixes, fixed, resolve, resolves
and resolved, matched case-insensitively. A keyword counts only at the start of
the text or after a character outside `[A-Za-z0-9_]`, so `prefix#7` is left
alone. Because the whitespace may be absent, `Fixes#7` and `fixes:#7` are
references and are defused. Unlike the claim check, the rewrite matches any
`owner/repo`, not only the current repository.

| Input | Output |
| --- | --- |
| `Closes owner/repo#3` | `Closes issue owner/repo#3` |
| `Fixes#7` | `Fixes issue #7` |
| `fixes:#7` | `fixes: issue #7` |
| `Closes (#7)` | `Closes issue #7` |
| `prefix#7` | `prefix#7` |

The `(#M)` rewrite runs first because it can create a reference:
`Closes (#7)` becomes `Closes #7`, which the second rewrite then turns into
`Closes issue #7`.

The same order affects conventional titles that use an issue as their scope.
An issue titled `fix(#5): x` already has a conventional prefix, so step 2 leaves
it alone. The first rewrite gives `fix#5: x`, which is itself a closing
reference, and the second gives `fix issue #5: x`. The scope is lost. This is
accepted: keeping `fix(#5)` would claim issue 5, and `fix#5` would still be
read as a claim.

`defuse_issue_refs` runs after the prefix step, so a `fix: ` prefix placed in
front of a leading `#M` is defused too: `#9 crashes on start` becomes
`fix: #9 crashes on start` and then `fix: issue #9 crashes on start`.

No other wording changes. The script's own ` (#N)` suffix is added afterwards,
and the body's `Closes #N` or `Refs #N` line comes from
`workflow_issue_reference.sh`, so neither is defused.

### 4. Cap

`cap_pr_title <base> <issue-number>` prints a title of at most 200 bytes:

1. The suffix is ` (#<issue-number>)`.
2. The base is cut to `200` minus the suffix length, in bytes.
3. An incomplete trailing UTF-8 sequence (a lead byte followed by fewer
   continuation bytes than it announces) is removed with a C-locale `sed`
   expression, so a multibyte character is never split. The same step removes
   a sequence split by the 1024-byte input bound. No `iconv` is needed.
4. Trailing spaces are removed and the suffix is appended.

## Issue title lookup

`lookup_issue_title <issue-number> <repo-identity>` is the only new network
call. It is advisory.

- With an empty repository identity it returns at once, prints nothing and makes
  no `gh` call. This stops `gh` from resolving `GH_REPO` or another remote and
  returning a different repository's issue. On the GitHub path the script has
  already stopped with `FAILED_PR_IDENTITY` by then, so this guard is a second
  line of defence.
- Otherwise it runs, as an argument array with no `eval`:

  ```sh
  timeout 30 gh issue view 1968 --repo rysweet/amplihack-rs --json title </dev/null 2>/dev/null
  ```

  `timeout 30` is omitted when `timeout` is not installed.
- The output is parsed with the fixed `jq` filter
  `if type=="object" then ((.title // "") | tostring) else "" end`.
- A non-zero exit, a timeout, empty output, `[]`, invalid JSON or an empty
  `.title` all yield no output. The function always returns 0 and the publish
  continues to the next title source.
- The call is not retried and does not go through `_gh_retry_core`.

The lookup runs on the GitHub path only, after the issue claim check and before
the title is built.

## Summary line

When the title comes from the issue or the task description, the `## Summary`
section reads:

```text
Workflow-generated PR for #1968: fix: workflow_publish_pr.sh titles PRs wrongly from a two-dot diff.
```

The text after `#<issue>: ` is the base title after normalisation, prefixing
and defusal, before the cap and without the ` (#N)` suffix. The title can
contain colons of its own, as in `fix: ...`, so this prefix is the boundary.
The line is written with `printf`, and the title is passed to it only as a `%s`
argument, never as part of the format string. When the template is used, the
summary keeps its previous form, `Concise workflow-generated PR for <scope>.`

## Azure DevOps path

The Azure DevOps path uses `DIFF_RANGE` for its changed-file list and diff stat,
and `format_changed_files_body` for the `## Changed files` section, so it shows
the same count and the same 40-entry list. Its title stays
`Update <branch> (AB#<work-item>)` and is not looked up.

## Behaviour that is unchanged

| Item | Range | Reason |
| --- | --- | --- |
| `BRANCH_DIFF_STATUS` (`git diff --quiet`) | `${BASE_REF}..HEAD` | It decides the no-diff, closed-obsolete and already-merged outcomes covered by the #962 and #1060 suites. |
| `COMMITS_AHEAD` (`git rev-list --count`) | `${BASE_REF}..HEAD` | For `rev-list`, two dots already mean commits on the branch only. |
| `RECENT_COMMITS` (`git log`) | `${BASE_REF}..HEAD` | Same as `COMMITS_AHEAD`. |

The PR is still created through `gh pr create` with the title as the single
argument after `--title` and the body written to a mode-0600 file passed with
`--body-file`.

### Interaction with the PR scope lookup

`scoped_pr_lookup` passes `--expected-pr-title-prefix Update` to
`workflow_pr_scope.sh` unless `EXPECTED_PR_TITLE_PREFIX` or
`PR_EXPECTED_TITLE_PREFIX` is set. Titles from the issue or the task description
usually do not start with `Update`. The prefix only breaks ties when several
open PRs share the same head and base; in that case the lookup falls back to its
primary key, as it did before.

## Configuration

There are no new settings. The publish step reads one existing recipe value.

`amplifier-bundle/recipes/workflow-publish.yaml` declares the task description
next to the other run-scoped inputs. Only the end of the `context:` block is
shown:

```yaml
context:
  # ... (other context keys unchanged)
  # #1561/#1684/#1968: declared so a nested run never reads an inherited RECIPE_VAR_<key>.
  repo_path: ""
  version_bump: ""
  local_testing_gate: ""
  precommit_results: ""
  task_description: ""
```

The script reads `RECIPE_VAR_task_description` only. It does not read the bare
`TASK_DESCRIPTION` or `ISSUE_TITLE` names: the runner omits the uppercase alias
for large values, and a bare name can be inherited from an outer run (#1684).
`issue_1561_step_outputs_prefer_recipe_var` guards both points.

There is no environment variable that overrides the issue title. Tests supply it
through a fake `gh` instead.

## Helper functions

The helpers are defined above the `WORKFLOW_PUBLISH_PR_LIB_ONLY` seam, so they
can be loaded without running the publish flow:

| Function | Input | Output |
| --- | --- | --- |
| `redact_credentials` | stdin | stdin with credentials replaced |
| `normalise_pr_title_text TEXT` | argument | normalised single line |
| `conventional_pr_title TEXT` | argument | text with a `<type>: ` prefix where one applies |
| `defuse_issue_refs TEXT` | argument | text with issue references defused |
| `cap_pr_title BASE ISSUE` | arguments | `BASE (#ISSUE)`, at most 200 bytes |
| `format_changed_files_body` | newline-separated paths on stdin | `## Changed files` section content |
| `first_task_line` | `RECIPE_VAR_task_description` | first non-blank line, trailing CR removed |
| `lookup_issue_title ISSUE REPO` | arguments | issue title, or nothing |

All of them return 0. None of them use `eval`, `sh -c` or `echo` on derived
text.

```sh
WORKFLOW_PUBLISH_PR_LIB_ONLY=1 . amplifier-bundle/tools/workflow_publish_pr.sh

conventional_pr_title "workflow_publish_pr.sh titles PRs wrongly from a two-dot diff"
# fix: workflow_publish_pr.sh titles PRs wrongly from a two-dot diff

conventional_pr_title "Add merge-base diff to publish"
# feat: Add merge-base diff to publish

defuse_issue_refs "x (#5) y"
# x #5 y

defuse_issue_refs "fix: #9 bad"
# fix: issue #9 bad

defuse_issue_refs "Fixes#7"
# Fixes issue #7

defuse_issue_refs "Closes (#7)"
# Closes issue #7

cap_pr_title "Add merge-base diff to publish" 1968
# Add merge-base diff to publish (#1968)

printf 'src/a.rs\nsrc/b.rs\n' | format_changed_files_body
# 2 files changed
# - src/a.rs
# - src/b.rs
```

## Examples

### Branch cut before `main` gained a workflow file

The branch commits three files under `src/` and `amplifier-bundle/recipes/`.
After it was cut, `main` gained `.github/workflows/ci-1968.yml`. Issue #1968 is
titled `workflow_publish_pr.sh titles PRs wrongly from a two-dot diff`.

**Before:**

```text
Update .github with 4 changed files (#1968)
```

The `.github` file came from `main`, sorted first and set the scope. The body
listed it as a branch change.

**After:**

```text
fix: workflow_publish_pr.sh titles PRs wrongly from a two-dot diff (#1968)
```

The body lists the three branch files under `3 files changed`, and nothing in
the title or body mentions `.github`.

### Issue lookup fails

`gh issue view` fails (rate limit, missing permission or no network) and the
task description starts with `Add merge-base diff to publish`. The publish
succeeds with:

```text
feat: Add merge-base diff to publish (#1968)
```

### No issue title and no task description

Both sources are empty. The template is used with the true count:

```text
Update workflow recipes with 3 changed files (#1968)
```

### Hostile issue title

An issue on a public repository is titled:

```text
Fixes #7 (#999) $(touch PWNED) `id` %s %n <ESC>[31m @someone
```

The resulting PR title is:

```text
fix: Fixes issue #7 #999 $(touch PWNED) `id` %s %n [31m @someone (#1968)
```

The ESC byte is gone, neither `#7` nor `#999` follows a closing keyword, and
`(#999)` no longer claims issue 999. The shell fragments stay literal text and
are never executed.

## Security considerations

The issue title is untrusted: anyone who can open an issue on a public
repository controls it. The first line of the task description is operator text
that is now published. Both are treated as data:

- They pass through the 1024-byte bound, credential redaction, control-byte
  removal, defusal and the UTF-8-safe cap before use.
- Derived text appears only as a `%s` argument to `printf` and on the left of
  `=~`. Patterns and the `jq` filter are fixed literals.
- The title reaches `gh` as one argument after `--title`, so a leading `--`
  cannot be read as an option.
- The lookup uses the existing `gh` login. No token is read or printed, `gh`
  stderr is discarded and raw `gh` output is never echoed. Standard input is
  `/dev/null` (#1764).
- The source diagnostic names the source, never the title.
- The body file is still created under `umask 077` and removed on every exit
  path.

Known residual risks:

- `@mentions` copied from an issue title into the body notify those users.
- Bidirectional controls and UTF-8-encoded C1 characters survive normalisation,
  because `tr` in the C locale works on single bytes.
- HTML and Markdown in titles are left to GitHub's own sanitisation.
- `RECIPE_VAR_local_testing_gate` and `RECIPE_VAR_precommit_results` still go
  into the body without credential redaction, as before.
- File names come from the branch's own commits and are not a new input.

## Regression coverage

`amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh` adds
`assert_pr_title_from_issue_on_moved_base`, called directly after
`assert_pr_title_ignores_lockfiles`.

The fixture is a bare origin and a clone. The branch `feat/issue-1968-title`
commits N files under `src/` and `amplifier-bundle/recipes/`. A second clone
then pushes `.github/workflows/ci-1968.yml` to `main`. The test repository
fetches, runs `git remote set-head origin main` and points `origin` at
`https://github.com/example/repo.git`. The expected count is
`git diff --name-only origin/main...HEAD | wc -l`.

`install_publish_1968_fake_gh` answers `auth`, `pr list` (`[]`),
`issue view ... --json title` and `pr create`, recording the title and body.
It appends every invocation's arguments to a call log. Any other call exits 99.

| Case | Set-up | Checks |
| --- | --- | --- |
| 1 | N = 3, lookup answered | No `.github` in title or body; body has `3 files changed` and 3 equals the expected count; title is `fix: workflow_publish_pr.sh titles PRs wrongly from a two-dot diff (#1968)` and does not match `Update .* with [0-9]+ changed files` |
| 2 | N = 45 | Body has `45 files changed` and `... and 5 more`; 40 is never reported as the count |
| 3 | Lookup fails, task description `Add merge-base diff to publish` | Publish succeeds; title is `feat: Add merge-base diff to publish (#1968)` |
| 4 | Lookup fails, no task description | Title is the template with the true count |
| 5 | Helpers sourced through `WORKFLOW_PUBLISH_PR_LIB_ONLY` | Prefix order; word boundaries; `defuse_issue_refs` gives `x #5 y` for `x (#5) y`, `fix: issue #9 bad` for `fix: #9 bad`, `Closes issue owner/repo#3` for `Closes owner/repo#3`, `Fixes issue #7` for `Fixes#7`, `fixes: issue #7` for `fixes:#7`, `Closes issue #7` for `Closes (#7)` and `fix issue #5: x` for `fix(#5): x`; `prefix#7` and reference-free text unchanged; valid UTF-8 from `cap_pr_title` on 300 bytes of `é` |
| 6 | Hostile issue title | Exit 0; no `(#999)`; `issue #7` present; no closing keyword before `#7` or `#999`; shell fragments literal; no ESC byte; title ends ` (#1968)`; no `PWNED` file anywhere under the work directory |
| 7a | `lookup_issue_title 1968 ""` called directly through `WORKFLOW_PUBLISH_PR_LIB_ONLY` | No output, return status 0, and the call log stays empty |
| 7b | Full publish run on the case 1 fixture with `REMOTE_HOST_TYPE=github`, after `git remote set-url origin https://example.invalid/acme/widgets.git`; `parse_github_repo_identity` rejects that URL | Non-zero exit with `FAILED_PR_IDENTITY`; no `issue view` line in the call log; no `pr create` call |

Case 7a tests the guard inside `lookup_issue_title`. A full run cannot reach that
guard with an empty identity, because the script stops with
`FAILED_PR_IDENTITY` before the title is built. Case 7b shows that ordering: a
rejected origin ends the publish before any issue lookup.

Every case runs in a subshell with `HOME` under the test's work directory and
unsets `GH_TOKEN`, `GITHUB_TOKEN`, `GH_REPO`, `GH_HOST`, `GH_ENTERPRISE_TOKEN`,
`TASK_DESCRIPTION`, `RECIPE_VAR_task_description`, `ISSUE_TITLE`,
`RECIPE_VAR_local_testing_gate` and `RECIPE_VAR_precommit_results`. The fake
`gh` is first on `PATH`, so `timeout 30 gh` resolves to it. Nothing touches the
actual `~/.claude` or `~/.amplihack`.

The #929 cases get the same isolation with `HOME="${WORK}/home-929"` and no task
description. Their fake `gh` exits 99 on `issue view`, so they exercise the
template path and show that a failed lookup does not fail the publish.

Run the suite with:

```sh
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
```

## Files changed

| File | Change |
| --- | --- |
| `amplifier-bundle/tools/workflow_publish_pr.sh` | `DIFF_RANGE`; full changed-file list and count; issue, task and template title order; title helpers; issue lookup; new summary line |
| `amplifier-bundle/recipes/workflow-publish.yaml` | Declares `task_description: ""` in `context:` |
| `amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh` | #1968 regression block; stricter isolation for the #929 cases |

## Related

- [Bug Fix #929: PR titles and no-merge directives](bugfix-929-pr-title-and-no-merge.md)
  introduced the scope rules that the template title still uses.
- [Default workflow overview](../workflows.md)
