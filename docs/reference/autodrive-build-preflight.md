---
title: Build Preflight Decision
description: how autodrive-build step-01 reports PR facts and step-02 decides from them whether to build a new PR or reuse an existing one (#2552).
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Build preflight decision (#2552)

`autodrive-build` opens with a preflight agent step, `step-01-build-preflight`,
that looks for an existing pull request for the work. The agent reports what it
found. It does not decide whether to build. The condition on
`step-02-default-workflow` makes that decision from the reported facts, so the
outcome does not depend on how an agent reads a rule.

Previously the preflight prompt asked the agent to set `should_run` and gave two
rules that disagreed: one required "an observed OPEN PR with verified worktree
and private state" for `should_run=true`, the other required `should_run=true`
when no PR existed. A fresh issue lane with no PR could therefore skip the build.

## Contents

- [Outcomes](#outcomes)
- [Preflight report fields](#preflight-report-fields)
- [Step-02 condition](#step-02-condition)
- [Worktree and private state](#worktree-and-private-state)
- [Behaviour test](#behaviour-test)
- [State key checks (#1680)](#state-key-checks-1680)
- [Related](#related)

## Outcomes

Each case has one outcome.

| What the preflight observed | `pr_found` | `pr_state` | `blockers` | Step-02 (`default-workflow`) | Result |
| --- | --- | --- | --- | --- | --- |
| No PR after a complete search | `false` | `NONE` | empty | runs | A new PR is built |
| An existing OPEN PR | `true` | `OPEN` | any | skipped | Step-03 reuses the PR |
| An existing MERGED PR | `true` | `MERGED` | any | skipped | Step-03 observes the merged state; nothing is rebuilt |
| Search could not complete (no access, failed or ambiguous lookup) | `unknown` | `UNKNOWN` | named | skipped | Step-03 reports the blockers |

A closed PR that was never merged does not count as found. Inability to read the
hosting service is never treated as the absence of a PR.

## Preflight report fields

The preflight returns one JSON object. It contains no `should_run` field.

```json
{
  "pr_found": "true|false|unknown",
  "pr": "number or empty",
  "pr_url": "url or empty",
  "pr_state": "OPEN|MERGED|NONE|UNKNOWN",
  "branch": "head branch",
  "head_sha": "observed SHA",
  "base_branch": "observed base",
  "base_sha": "observed SHA",
  "state_dir": "absolute private directory or empty",
  "repo": "absolute linked worktree or empty",
  "review_dir": "absolute main checkout or empty",
  "reason": "concrete observations",
  "blockers": [],
  "attempts": []
}
```

| Field | Meaning |
| --- | --- |
| `pr_found` | `true` for an OPEN or MERGED PR, `false` only after a complete search found none, `unknown` when the search could not complete. |
| `pr`, `pr_url` | Number and URL of the found PR; empty when none was found. |
| `pr_state` | `OPEN`, `MERGED`, `NONE` (complete search, no PR) or `UNKNOWN` (incomplete search). |
| `state_dir`, `repo`, `review_dir` | Set when a PR is found and reused; empty otherwise. No later step reads them when a new PR is built. |
| `blockers` | Named reasons the lookup, worktree or state could not be established. |

The output is stored as `build_preflight` and passed whole to step-03.

## Step-02 condition

```yaml
- id: "step-02-default-workflow"
  # Decided here from the preflight's reported facts, not by the agent (#2552).
  condition: "build_preflight.pr_found == 'false' and build_preflight.pr_state != 'OPEN' and build_preflight.pr_state != 'MERGED' and not build_preflight.blockers"
```

The lane builds only when all four parts hold. Anything else skips the build:

- A missing field resolves to null and fails `pr_found == 'false'`.
- Malformed preflight output leaves every field null, so nothing is built.
- A contradictory report, such as `pr_found=false` with `pr_state=OPEN`, is not built.
- A found PR with blockers is not built; step-03 reuses the PR and reports the blockers.

The condition is a fixed expression over context fields. The recipe condition
evaluator looks up identifiers in the context; it never inserts agent text into
the expression.

Step-03 keeps its own post-build rule and its own checks that the PR is open,
in the same repository, and matched to its worktree.

## Worktree and private state

Verifying the linked worktree and creating private state apply only when an
existing PR is found and reused:

- The head branch's linked worktree is resolved with `git worktree list` and
  compared with the observed PR head. The launch checkout is never used in its
  place, and a dirty worktree is never replaced.
- Private state is created outside the worktree with
  `amplihack orch helper autodrive state --directory <absolute-path>`.

When no PR exists, the preflight creates no worktree and no state. The PR is not
known yet, so state keyed by a branch name such as `main` would collide across
lanes (#1680). Step-03 keys the state once phase 1 has opened the PR.

## Behaviour test

`crates/amplihack-cli/tests/issue_2552_build_preflight_decision.rs` runs the
real `autodrive-build` recipe in the in-process recipe runner with a fake agent
adapter. The adapter holds an in-memory PR table that decides what the fake
preflight reports; no `gh` binary or network is used. Every fake response also
includes `should_run=false`, so the tests fail if the build decision is ever
taken from the agent again.

| Test | PR table | Expected |
| --- | --- | --- |
| `a_fresh_lane_with_no_pr_runs_default_workflow` | empty | Step-02 runs |
| `a_closed_unmerged_pr_does_not_stop_the_build` | CLOSED, unmerged PR | Step-02 runs |
| `a_lane_with_an_open_pr_reuses_it_without_building` | OPEN, then MERGED, PR 7 | Step-02 skipped; step-03 completes and its prompt names PR 7 |
| `a_lookup_that_could_not_complete_does_not_build` | lookup refused | Step-02 skipped; the blocker reaches step-03 |

The open-PR test runs once with an OPEN PR and once with a MERGED PR, and makes
the same checks on both passes, including that the step-03 prompt names PR 7.
Step-03 receives `build_preflight` whole in either state, so the PR number is
in its prompt whether the PR is open or merged.

Each test is kept apart from the user's installation in three ways. Recipes are
resolved only from a temporary directory (`with_recipe_search_dirs` with
`with_default_search(false)`), agents only from a temporary directory
(`AgentResolver::new(Some(...))`), and no test calls `std::env::set_var`, so
tests running in parallel do not affect one another.

Run them with:

```bash
cargo test -p amplihack-cli --test cli_integration issue_2552
```

Changing `autodrive-build.yaml` changes its entry in
`amplifier-bundle/recipes/_recipe_manifest.json` (the first 16 hex characters
of the file's SHA-256). Recompute it after the last edit:

```bash
sha256sum amplifier-bundle/recipes/autodrive-build.yaml | cut -c1-16
```

`issue_1802` fails when the manifest entry is stale.

## State key checks (#1680)

`tests/issue_1680_fresh_run_state_key.sh` reads `autodrive-build.yaml` and
checks the step-01 and step-03 contract that keeps two fresh runs in one
repository from sharing state. CI runs it in the Lint & Format job; pre-push
does not.

```bash
bash tests/issue_1680_fresh_run_state_key.sh   # exits 0 when every check passes
```

PR #2583 (#2552) reworded the step-01 prompt, and three step-01 phrase checks
were removed rather than rewritten, because the fresh-run decision is a
behaviour and the Rust tests above cover it. Each removed check is covered
elsewhere:

| Removed step-01 check | Covered by |
| --- | --- |
| The returned JSON carries `state_dir` | The step-01 shape check, the [preflight report fields](#preflight-report-fields), and the step-03 `build_result` `state_dir` check |
| A fresh preflight does not create branch-keyed state | The `issue_2552_build_preflight_decision.rs` tests and the step-03 check that a fresh run's state is keyed by the PR it resolved |
| State is private and outside the worktree | `amplihack orch helper autodrive state`, which creates the directory with mode 0700 and refuses unsafe ownership or modes, and the step-03 check of the same clause |

## Related

- [Auto Drive To Merge Reference](./auto-drive-to-merge.md) - the drive that `autodrive-build` feeds
- [Auto-Drive-Light Recipe](./auto-drive-light.md) - its preflight wording is tracked separately on #2539
- [Workflow Terminal State](./workflow-terminal-state.md) - what step-02's `default-workflow` run reports back to step-03
