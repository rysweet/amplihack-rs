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
takes the title from the issue rather than from a file-count template. The
helper functions carry their own contracts in comments; this page records the
problem, the decisions and the remaining risks.

## Problem

Three defects combined.

1. **Two-dot diff.** The changed-file list and diff stat came from
   `git diff "${BASE_REF}..HEAD"`, which compares the tip of the base with
   `HEAD`. Once `main` moved on, every file changed on `main` since the branch
   point appeared as a branch change, and could set the title's scope.
2. **Count after truncation.** `CHANGED_COUNT` was counted after `head -40`, so
   it never exceeded 40.
3. **Template title.** The title was always
   `Update <scope> with <N> changed files (#N)`, which said nothing about the
   change and repeated the wrong count.

For example, a branch that changed three files under `src/` and
`amplifier-bundle/recipes/`, cut before `main` gained
`.github/workflows/ci-1968.yml`, was titled
`Update .github with 4 changed files (#1968)`. It is now titled
`fix: workflow_publish_pr.sh titles PRs wrongly from a two-dot diff (#1968)`,
and its body lists the three branch files under `3 files changed`.

## Requirements and outcomes

| ID | Requirement | Outcome |
| --- | --- | --- |
| A1 | Use the task description when the issue title is unavailable | Met, reading `RECIPE_VAR_task_description` only. The bare `TASK_DESCRIPTION` fallback was dropped: the runner omits it for large values and it can be inherited from an outer run (#1684). |
| A2 | List and count files against the merge base | Met on the GitHub and Azure DevOps paths |
| A3 | Report the true count; show at most 40 entries | Met |
| A4 | Title from the issue, then the task, then the template | Met on the GitHub path |
| A5 | Derived text cannot claim or close other issues | Met |
| A6 | Derived text is treated as data | Met |
| A7 | A failed lookup does not fail the publish | Met |
| A8 | No change to no-diff, closed-obsolete and already-merged classification | Met. A branch with no net change against a moved base is still classed as having a diff; this predates #1968 and is out of scope. |

## Title sources

On the GitHub path the title comes from the first source that yields text:

| Order | Source | Prefixed and defused |
| --- | --- | --- |
| 1 | The issue title, from `gh issue view` | yes |
| 2 | The first non-blank line of `RECIPE_VAR_task_description` | yes |
| 3 | The #929 template with the true count | no |

Stderr names the source only (`PR title source: issue|task|template`), never
the title text. Every title ends with ` (#<issue>)`.

## Design decisions

- **Diff range.** `DIFF_RANGE` is `${BASE_REF}...HEAD` when a merge base exists,
  otherwise `${BASE_REF}..HEAD` with a warning on stderr (shallow clones).
- **One diff.** The changed-file list and diff stat are computed once, straight
  after `DIFF_RANGE`, and shared by both providers. A failed `git diff
  --name-only` or `git diff --stat` prints a warning and continues with an empty
  value.
- **Unchanged ranges.** `BRANCH_DIFF_STATUS`, `COMMITS_AHEAD` and
  `RECENT_COMMITS` keep `${BASE_REF}..HEAD`; for `rev-list` and `log` two dots
  already mean the branch's own commits.
- **Full list, cut display.** The untruncated list feeds the count, the #929
  scope, the `## Risk` section and the `Closes`/`Refs` choice. Only the body's
  display list stops at 40, followed by `- ... and <N-40> more`.
- **Normalisation.** Keep the first 8 KiB, turn CR, LF and TAB into spaces,
  delete other C0 bytes and DEL, redact credentials, collapse spaces, drop
  trailing full stops. A title of only full stops is therefore no title, and the
  next source is tried. Control bytes go before redaction so they cannot split a token. All `sed` and `tr`
  calls run with `LC_ALL=C` so bytes that are not valid UTF-8 cannot stop BSD
  `sed`.
- **Redaction.** One function, `redact_credentials`, shared with provider
  stderr: URL userinfo, `gh[pousr]_` and `github_pat_` tokens, `sk-`, `sk-ant-`
  and `sk-proj-` keys, and AWS `AKIA`/`ASIA` key IDs. The `sk-` and AWS
  patterns require a boundary, so `disk-usage-...` is not redacted.
- **Type prefix.** A leading verb decides (`fix` forms to `fix`; `add`,
  `implement`, `support`, `introduce` to `feat`; `document`, `docs` to `docs`;
  `test`, `tests` to `test`; `refactor` to `refactor`). Otherwise the first
  whole-word group to match, in the order fix, docs, test, refactor, feat. An
  existing conventional prefix is kept, but a scope made only of an issue
  reference is dropped: `fix(#5): x` becomes `fix: x`.
- **Issue references.** Every reference in derived text becomes `issue M`:
  `#M`, `(#M)`, `issue #M`, `GH-M`, `owner/repo#M` and issues URLs. No
  `#<digits>` survives, so a closing keyword has nothing to act on however it is
  written, and the claim check never sees a `(#M)` claim. The PR's own
  ` (#N)` suffix is added afterwards and is the only reference in the title.
- **Mentions.** In the body Summary only, each `@name` or `@org/team` outside a
  code span is put in a code span, which GitHub does not notify. Titles do not
  notify and are left alone.
- **Cap.** At most 200 bytes including the ` (#N)` suffix, never splitting a
  UTF-8 sequence, with trailing spaces and full stops removed. The suffix is
  always kept because the claim check relies on it.
- **Summary.** `Workflow-generated PR for #N: <title without suffix>.` is built
  from the capped title, so it shares the title's bound. The template keeps
  `Concise workflow-generated PR for <scope>.`
- **Lookup.** `timeout 30 gh issue view N --repo <owner/repo> --json title`,
  standard input `/dev/null`, stderr discarded, one attempt and no retry. With
  no repository identity no call is made. A non-zero exit is reported only as
  `NOTE: gh issue view for #N exited <status> (#1968)`, and output that jq
  cannot parse only as `... is not JSON (jq exited <status>)`.
- **Azure DevOps.** Uses the shared list, stat and body format. Its title stays
  `Update <branch> (AB#<work-item>)`.
- **Recipe.** `workflow-publish.yaml` declares `task_description: ""` so a
  nested run never reads an inherited `RECIPE_VAR_task_description`.

### Examples

| Input (issue or task text) | PR title |
| --- | --- |
| `Add merge-base diff to publish` | `feat: Add merge-base diff to publish (#1968)` |
| `Add tests for the broken parser` | `feat: Add tests for the broken parser (#1968)` |
| `Document why the build fails on macOS` | `docs: Document why the build fails on macOS (#1968)` |
| `fix(#5): x` | `fix: x (#1968)` |
| ``Fixes **#7** `#8` (#9)`` | ``fix: Fixes **issue 7** `issue 8` (issue 9) (#1968)`` |
| `Fix the parser.` | `fix: Fix the parser (#1968)` |
| (none, 45 files) | `Update workflow recipes with 45 changed files (#1968)` |

A hostile issue title, ``Fixes #7 ((#999)) $(touch PWNED) `id` %s %n <ESC>[31m @someone``,
becomes ``fix: Fixes issue 7 ((issue 999)) $(touch PWNED) `id` %s %n [31m @someone (#1968)``.
The ESC byte is gone, nothing claims or closes #7 or #999, the shell text is
never executed, and the body Summary shows `` `@someone` ``.

## Interaction with the PR scope lookup

`scoped_pr_lookup` passes `--expected-pr-title-prefix Update` to
`workflow_pr_scope.sh` unless `EXPECTED_PR_TITLE_PREFIX` or
`PR_EXPECTED_TITLE_PREFIX` is set. Derived titles usually do not start with
`Update`. The prefix only breaks ties between open PRs with the same head and
base, and the lookup then falls back to its primary key, as before.

## Test matrix

`assert_pr_title_from_issue_on_moved_base` in
`amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh` builds a
bare origin and a clone; the branch commits N files, then a second clone pushes
`.github/workflows/ci-1968.yml` to `main`. A fake `gh` answers `auth`,
`pr list`, `issue view --json title` and `pr create`, logs every call, and exits
99 on anything else. Every case clears GitHub tokens, task variables and
`HOME`, so nothing reaches the network or the user's configuration.

| Case | Set-up | Checks |
| --- | --- | --- |
| 1 | N = 3, lookup answered | No `.github` in title or body; `3 files changed`; title from the issue with `fix:`; source logged, title text not logged |
| 2 | N = 45 | `45 files changed`, 40 entries, `- ... and 5 more` |
| 3 | Lookup fails, empty, or not JSON; task set | Title from the task; Summary equals the title without suffix; `exited 1` on stderr, not gh's message |
| 3b | Task line of about 500 bytes | Title at most 200 bytes; Summary equals the capped title |
| 4 | No issue title, no task, bare decoys set | Template with the true count; decoys ignored |
| 5 | Helpers sourced through `WORKFLOW_PUBLISH_PR_LIB_ONLY` | Prefix rows, including the verb rule and issue-scope removal; defusal rows for emphasis, code spans, link text, a non-breaking space, nested parentheses, `GH-M`, `owner/repo#M` and URLs; no `#<digits>` before the suffix; mention rows; redaction rows for each pattern and the boundary cases; 8 KiB bound; cap on UTF-8 and trailing full stops; body list at 0, 1, 40, 41 and 45 under `pipefail` |
| 6 | Hostile issue title | Exit 0; no `(#999)`; no closing keyword before `#7` or `#999`; no `#<digits>` before the suffix; no ESC; shell text literal; `` `@someone` `` in the body; no `PWNED` file |
| 7a | `lookup_issue_title 1968 ""` | No output, status 0, no `gh` call |
| 7b | Origin rejected by `parse_github_repo_identity` | `FAILED_PR_IDENTITY`; no `issue view` and no `pr create` |

Run it with:

```sh
env -u RECIPE_VAR_task_description bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
```

## Known risks

- **Issue author chooses the title.** On a public repository anyone who can
  open an issue sets the PR title. It is sanitised, and the issue is the
  intended source.
- **No lookup time limit without `timeout`.** On hosts without `timeout` (for
  example macOS without coreutils) the lookup has no bound beyond `gh`'s own.
- **Unrecognised secrets.** Azure DevOps PATs and other keys without a
  distinctive prefix are not redacted.
- **Token at the 8 KiB bound.** A token that crosses the input bound is cut
  before redaction and can leave a fragment such as `ghp_ABCD`. This needs over
  7 KB of text before the token, so it applies only to task descriptions; issue
  titles are at most 256 characters.
- **Mentions in titles.** An `@name` from the issue stays in the title. Titles
  do not notify, but the squash commit subject carries it.
- **Mentions in odd code spans.** The body's mention rule tracks single
  backticks; a mention inside an unbalanced or multi-backtick span may be
  wrapped incorrectly.
- **Unicode controls.** Bidirectional controls and UTF-8-encoded C1 characters
  survive, because `tr` in the C locale works on single bytes.
- **Markup.** HTML and Markdown in titles are left to GitHub's sanitisation.
- **Unredacted gate output.** `RECIPE_VAR_local_testing_gate` and
  `RECIPE_VAR_precommit_results` still reach the body without redaction, as
  before.

## Related

- [Bug Fix #929: PR titles and no-merge directives](bugfix-929-pr-title-and-no-merge.md)
  introduced the scope rules the template title still uses.
- [Default workflow overview](../workflows.md)
