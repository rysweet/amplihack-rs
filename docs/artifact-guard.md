# Artifact Guard

**Status:** Implemented. The base guard (staged, tracked, and untracked
scanning across all modes) is implemented, and the issue #928 *ignored-present
narrowing* documented here — restricting ignored-present scans to the
`worktree` and `all` modes via `ArtifactGuardMode::scans_ignored_present()` — is
now in effect. `pre-commit` and `pre-publish` no longer block on ignored-present
artifacts. See
[Worked example](#worked-example-cache-leftovers-do-not-block-a-commit-or-publish)
for the behavior.

Artifact Guard prevents generated, runtime, cache, and build artifacts from
leaking into the parent repository worktree during agent and plugin workflows.
It is a blocking safety gate for broad staging, pre-commit, and publication
paths. It reports violations with remediation guidance and never deletes,
moves, unstages, or rewrites files.

## Contents

- [Behavior](#behavior)
- [Provenance: this change vs the repository's history](#provenance-this-change-vs-the-repositorys-history)
- [Command-line interface](#command-line-interface)
- [Default prohibited rules](#default-prohibited-rules)
- [Registered worktrees in the index](#registered-worktrees-in-the-index)
- [Allowlist configuration](#allowlist-configuration)
- [Workflow and pre-commit coverage](#workflow-and-pre-commit-coverage)
- [Output isolation](#output-isolation)
- [Workflow runtime cleanup preflight](#workflow-runtime-cleanup-preflight)
- [Intended Rust API](#intended-rust-api)
- [Fixing violations](#fixing-violations)
- [Worked example: cache leftovers do not block a commit or publish](#worked-example-cache-leftovers-do-not-block-a-commit-or-publish)
- [Worked example: a run worktree staged in the launch checkout](#worked-example-a-run-worktree-staged-in-the-launch-checkout)

## Behavior

Artifact Guard:

1. Scan repo-relative paths only.
2. Check staged paths before commit.
3. Check tracked and untracked paths before broad staging and workflow
   publication.
4. Check the whole index for entries at the path of another registered Git
   worktree of the same repository, whatever `.gitignore` says. See
   [Registered worktrees in the index](#registered-worktrees-in-the-index).
5. Check ignored-present artifact paths only in the `worktree` and `all` modes,
   which are local-hygiene scans. The commit- and publication-gate modes
   (`pre-commit`, `pre-publish`) never block on ignored-present artifacts.
6. Separate artifacts the change under review introduced from artifacts the
   repository already carried, and block the gate modes only on the former. See
   [Provenance](#provenance-this-change-vs-the-repositorys-history).
7. Fail closed on invalid configuration, invalid paths, Git failures, and unsafe
   allowlist entries.
8. Print actionable remediation before exiting, naming the offending paths and
   the exact commands that clear them. Never print a command that deletes a
   live worktree.
9. Leave the repository unchanged.

### Ignored-present scope

An *ignored-present* artifact is a path that exists in the worktree, is
untracked, and is excluded by `.gitignore` (for example an untracked
`.pytest_cache/` or `node_modules/` directory). Because Git will never stage,
commit, or publish an ignored path, such a path cannot leak through a commit or
a publication step. The guard therefore treats ignored-present artifacts as a
**local-hygiene** concern, not a **commit/publish-gate** concern:

| Mode | Purpose | Scans ignored-present? |
| --- | --- | --- |
| `pre-commit` | Block a commit | No |
| `pre-publish` | Block a publication / broad-staging gate | No |
| `staged` | Inspect the index only | No |
| `worktree` | Local leftover hygiene | Yes |
| `all` | Manual full safety scan | Yes |

This is a deliberate fail-*open* narrowing for a class of paths that are outside
the guard's threat model. Anything that could actually be committed or published
— staged, tracked, or untracked-but-not-ignored paths — is still blocked
fail-closed in every gate mode. In particular, an untracked cache directory that
is **not** covered by `.gitignore` is still a violation under `pre-commit` and
`pre-publish`, because it could be swept into a `git add -A`.

Before this narrowing, a gitignored, untracked `.pytest_cache/` or
`node_modules/` directory left behind by a normal test or install run caused
`amplihack hygiene artifact-guard --mode pre-publish` (and `--mode pre-commit`)
to fail closed at the end of a workflow, even though those directories could
never be committed or published. See issue #928.

`.gitignore` reduces Git noise, and it is not a general authorization mechanism:
the `worktree` and `all` modes still report ignored-present dependency trees,
runtime directories, or cache directories so you can clean up local
parent-worktree pollution. The commit and publication gates simply do not treat
that unreachable pollution as a blocking condition.

`target/` is intentionally special-cased. Normal Rust commands create an ignored
`target/` directory in the repository, and the guard must not make ordinary
`cargo test`, `cargo clippy`, or pre-commit usage hostile. By default:

| `target/` source | Result |
| --- | --- |
| Staged | Violation |
| Tracked | Violation |
| Untracked because it is not ignored | Violation |
| Ignored-present only | Not a violation |

Workflow steps that need strict build-output isolation should set
`CARGO_TARGET_DIR` to an isolated location instead of relying on the default
repo-local `target/` directory.

## Provenance: this change vs the repository's history

Every violation carries a **provenance**:

| Provenance | Meaning | Gate modes | Audit modes |
| --- | --- | --- | --- |
| `introduced` | The change under review staged, committed, or left this path — it is in the diff, or a broad `git add -A` is about to put it there | **Block** | Block |
| `pre-existing` | The path is committed on the baseline and untouched by the change under review, so it cannot enter that change's diff | **Report, do not block** | Block |

Provenance is measured against a **baseline** revision: the merge base of `HEAD`
and the first of `@{upstream}`, `origin/HEAD`, `origin/main`, `origin/master`,
`main`, `master` that resolves, or the revision given to `--baseline` /
`AMPLIHACK_ARTIFACT_GUARD_BASELINE`. A path is `introduced` when it appears in
`git diff <baseline> HEAD` or in `git diff HEAD` (the working tree and index),
and `pre-existing` otherwise. Staged, untracked, and ignored-present paths are
always `introduced`: they are live worktree state the change is carrying now,
not repository history.

If a baseline was explicitly requested and does not resolve, that is a
configuration error (exit `2`) rather than a silent fallback. If no baseline
resolves at all — a repository with no upstream and no `main`/`master` — the
report says so and treats the tracked index as pre-existing, because the guard
will not assert a provenance it could not measure.

### Why the gates do not block on a pre-existing condition

Issue #1422: a `smart-orchestrator` run spent ninety minutes designing,
implementing, testing, and verifying a change to pin dependency versions. It
made three commits, logged `{"verdict": "WORK_VERIFIED"}`, and was then killed
at `checkpoint-after-implementation` because the repository it was working in
had 668 `node_modules` paths committed into Git long before the run started. The
finding was true. The run had not caused it, its brief told it not to make
unrelated changes, and none of the three printed remedies fit: removing 668
tracked files was already a separate open pull request, moving them was the same
edit by another name, and "a narrow reviewed allowlist entry" is not 668 lines.
The run then hit the same wall four more times.

A pre-existing tracked artifact is *already on the baseline*. No commit or
publish gate can keep it out of anything, because it is not in the change's
diff. Blocking there protects nothing and discards the verified work standing in
front of it. This is the same reasoning that narrowed the gates away from
ignored-present paths in issue #928, applied along the provenance axis instead
of the reachability axis.

The condition never stops being reported. `--mode all` and `--mode worktree`
audit the whole repository and still exit `1` on it, the gate modes print the
full pre-existing report on stderr before passing, and
`--preexisting block` (or `AMPLIHACK_ARTIFACT_GUARD_PREEXISTING=block`) restores
whole-repository enforcement for repositories that want it.

## Command-line interface

```bash
amplihack hygiene artifact-guard \
  --repo <path> \
  --mode <mode> \
  [--allowlist <path>]
```

| Option | Description | Default |
| --- | --- | --- |
| `--repo <path>` | Repository worktree to scan. The path must resolve inside a Git repository. | current directory |
| `--mode <mode>` | Scan mode: `pre-commit`, `pre-publish`, `all`, `staged`, or `worktree`. | `pre-commit` |
| `--allowlist <path>` | Optional allowlist file. Relative paths resolve from the repository root. | `.amplihack-artifact-allowlist` when present |
| `--baseline <rev>` | Revision the change under review is measured against for provenance. Env: `AMPLIHACK_ARTIFACT_GUARD_BASELINE`. | first resolvable of `@{upstream}`, `origin/HEAD`, `origin/main`, `origin/master`, `main`, `master` |
| `--preexisting <policy>` | What the gate modes do about pre-existing artifacts: `report` (print in full, do not block) or `block`. Ignored by the audit modes, which always fail closed. Env: `AMPLIHACK_ARTIFACT_GUARD_PREEXISTING`. | `report` |

| Mode | Sources checked | Typical use |
| --- | --- | --- |
| `pre-commit` | Staged, tracked, and untracked (committable candidates) | Local pre-commit hook |
| `pre-publish` | Staged, tracked, and untracked (committable candidates) | Workflow broad-staging and PR/finalize gates |
| `all` | Staged, tracked, untracked, and ignored-present artifact candidates | Manual full safety scan |
| `staged` | Staged paths only | Diagnose why a commit is blocked |
| `worktree` | Tracked, untracked, and ignored-present artifact candidates | Check local leftovers before cleanup or staging |

`pre-commit` and `pre-publish` are commit/publish gates. They only scan paths
that could actually reach a commit or publication (staged, tracked, and
untracked-but-not-ignored), so gitignored cache leftovers such as
`.pytest_cache/` or `node_modules/` never block them. Use `all` or `worktree`
when you also want to surface ignored-present local leftovers for cleanup. Use
`staged` only for focused debugging because it does not detect ignored or
untracked leftovers.

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | No blocking prohibited artifacts were found (a pre-existing condition may still have been reported) |
| `1` | Blocking prohibited artifacts were found |
| `2` | The guard could not complete because configuration, paths, mode, allowlist, or Git state was invalid |

Violation output (this example is from a `--mode all` scan, which is the only
class of mode that reports an `ignored-present` source alongside `staged` and
`untracked`):

```text
Artifact Guard blocked 3 prohibited artifact path(s) introduced by this change in /repo (mode: all).
Baseline: origin/main (1f0c2b9a4d31).

source           provenance     path                  rule
staged           introduced     dist/plugin.js        plugin-bundle
ignored-present  introduced     node_modules/         node-modules
untracked        introduced     .cache/               cache-artifact

Remediation:
  - Remove local artifact leftovers from the parent worktree.
  - Move generated, plugin, cache, and runtime output into an ignored isolated directory outside the parent worktree.
  - If intentional source material, add a narrow reviewed entry to .amplihack-artifact-allowlist.

Exact commands for the paths above:
  git rm -r --cached -- 'dist'   # drop from the change, keep on disk
  rm -rf -- 'dist'               # or delete the leftover outright
  ...
```

A pre-existing condition is printed on stderr and does **not** set a non-zero
exit in the gate modes:

```text
Artifact Guard: 668 pre-existing prohibited artifact path(s) in /repo — NOT blocking this pre-publish gate.
These paths are committed in the baseline main (735b3633e9cd) and are untouched by this change, so they cannot enter its diff. This run did not create them (issue #1422).

  node_modules/  — 668 path(s), rule node-modules
      node_modules/pkg0/f120.js
      ... and 663 more under node_modules/

Way forward (none of these is required to finish the current change):
  - Clean up in a dedicated change: git rm -r --cached -- 'node_modules' && echo 'node_modules/' >> .gitignore && git commit -m 'chore: untrack node_modules'
  - List every path and fail closed on the whole condition: amplihack hygiene artifact-guard --repo /repo --mode all
  - Enforce it here anyway: --preexisting block (or AMPLIHACK_ARTIFACT_GUARD_PREEXISTING=block).
```

When one introduced path lands inside a tree that also holds pre-existing
entries, the refusal lists that path individually instead of collapsing to
`git rm -r --cached node_modules`, which would untrack the pre-existing tree as
a side effect — the unrelated change the run was told not to make.

Configuration errors exit with code `2`:

```text
Artifact Guard configuration error:
  .amplihack-artifact-allowlist:4 rejects absolute paths: /tmp/plugin.js

Fix the allowlist entry or run without the allowlist.
```

## Default prohibited rules

Artifact Guard rejects these paths by default when they appear in the parent
repository worktree:

| Rule | Examples | Ignored-present behavior |
| --- | --- | --- |
| Dependency trees | `node_modules/`, `packages/*/node_modules/` | Blocked in `worktree`/`all` only |
| Plugin bundles | `dist/plugin.js`, `*/dist/plugin.js` | Blocked in `worktree`/`all` only |
| Claude runtime | `.claude/runtime/` | Exempt as untracked/ignored-present; blocked only when **staged or tracked** (except the launcher-owned files — see below) |
| Nested worktrees | `worktrees/` | Blocked in `worktree`/`all` only |
| Registered worktrees (`registered-worktree`) | Any index entry at a path listed by `git worktree list` inside the repository: `worktrees/feat/pr-1530`, `.worktrees/review`, `wt/review` | Never scanned as ignored-present; blocked when staged, and when tracked according to provenance. Cannot be allowlisted. |
| Cache directories | `.cache/`, `.npm/`, `.pnpm-store/`, `.yarn/cache/`, `.turbo/`, `.parcel-cache/`, `.pytest_cache/` | Blocked in `worktree`/`all` only |
| Build output | `dist/`, `build/`, `coverage/`, `.next/`, `out/`, `logs/`, `outputs/`, `index.scip` | Blocked in `worktree`/`all` only |
| Rust build output | `target/` | Blocked only when staged, tracked, or untracked |
| Generated indexes and logs | `index.scip`, generated runtime logs, generated output directories | Blocked in `worktree`/`all` only |

The **Ignored-present behavior** column describes what happens when a prohibited
path exists in the worktree but is untracked and gitignored. These paths are
only blocked in the local-hygiene modes (`worktree`, `all`). In the commit and
publication gates (`pre-commit`, `pre-publish`), the same path is not blocked as
ignored-present, because it cannot be committed or published. Every rule still
blocks fail-closed in **all** modes when the path is staged, tracked, or
untracked-but-not-ignored.

Rules match normalized repo-relative paths using `/` separators. The guard does
not need to read artifact file contents; path-level scanning is the intended
contract.

### Built-in `.claude/runtime/` exemption

The amplihack launcher, session tracker, and every agent's PostToolUse metrics
hook write bookkeeping into `<repo>/.claude/runtime/` continuously as a normal,
unavoidable part of launching and running an agent: launcher context, session
logs, metrics (`metrics/post_tool_use_metrics.jsonl`, appended on every tool
call), locks, and power-steering state all land here while a recipe runs. This
subtree is `.gitignore`d, tool-generated runtime state.

The guard exempts the whole `.claude/runtime/` subtree **when it appears as
untracked or ignored-present output** — the form this unavoidable runtime state
always takes. This holds in every mode, including the fail-closed `pre-commit`
and `pre-publish` gates:

| Path | Untracked / ignored-present | Staged or tracked |
| --- | --- | --- |
| `.claude/runtime/` (whole subtree) | Exempt | Blocked as `claude-runtime` |
| `.claude/runtime/launcher_context.json` | Exempt | Exempt (launcher-owned) |
| `.claude/runtime/sessions.jsonl` | Exempt | Exempt (launcher-owned) |

Deliberately committing runtime state into the published tree (a *staged* or
*tracked* `.claude/runtime/` path) is genuine pollution and is still blocked —
except the two launcher-owned bookkeeping files, which are exempt in all git
sources because the launcher itself may stage them.

These are built-in implicit exemptions, not `.amplihack-artifact-allowlist`
entries (and `.claude/runtime` cannot be broadly allowlisted anyway — it is a
root-prohibited exemption). Because the untracked/ignored exemption lives in
`rule_for_path`, it also removes that runtime output from the `worktree`/`all`
audit modes; that is intentional, since always-present regenerated runtime state
is not actionable pollution.

Before this exemption, files under
`.claude/runtime/` (originally the launcher's own `launcher_context.json`, and
later the metrics file that agents append to on every tool call) failed the
end-of-run `pre-publish` guard, which left `recipe-runner-rs` and its child
agents hung after the work was already committed and pushed (issue #807).

## Registered worktrees in the index

The `registered-worktree` rule refuses a commit whose index holds another Git
worktree of the same repository. Git stages a worktree directory as a *gitlink*
(an index entry with mode `160000`). A worktree staged that way publishes a
pointer to a commit no clone can fetch, and the rest of the change rides along
with it.

This is how issue #1528 happened. An auto-drive run launched from a
repository's main checkout ran its agents in that checkout, where the pull
request's files were reachable only through the gitignored `worktrees/<branch>`
directory. Git refused the plain `git add` and printed
`hint: Use -f if you really want to add them.` The agent followed the hint, and
`git add -f` staged the run worktree as a gitlink in the main checkout's index.
`.gitignore` does not protect a path that is added with `-f`.

### What counts as a registered worktree

A registered worktree is any entry in `git worktree list --porcelain` whose
path is strictly inside the scanned repository's top level. Each entry is
matched by both its canonical path and the path Git recorded, so a *prunable*
entry (its directory is gone) still counts. The location does not matter:
`worktrees/feat/pr-1530`, `.worktrees/review` and `wt/review` all match.

The scanned repository's own top level never matches. When the guard runs
inside a linked worktree, the main checkout is an ancestor of the scan, not
inside it, so it is not reported.

### Which index entries match

| Index entry | Source | Provenance | Gate modes |
| --- | --- | --- | --- |
| Gitlink (`160000`) at a registered worktree path, staged (differs from `HEAD`) | `staged` | `introduced` | **Block**, under every `--preexisting` policy |
| Any other staged path at or under a registered worktree path | `staged` | `introduced` | **Block**, under every `--preexisting` policy |
| Gitlink at a registered worktree path, unchanged since `HEAD` | `tracked` | From the baseline, as for any tracked path | Block if `introduced`; follow `--preexisting` if `pre-existing` |

Paths match on a path boundary only: the entry equals the worktree path or
starts with the worktree path followed by `/`. With `wt/x` registered,
`wt/xy/notes.md` is not a `registered-worktree` entry.

The rule runs in every mode that scans staged paths: `pre-commit`,
`pre-publish`, `staged` and `all`. It runs before the other staged checks, so a
path reported as `registered-worktree` is not reported again as
`nested-worktree`. The scan reads `git worktree list --porcelain`,
`git ls-files -s -z` and `git diff --cached --name-only -z`, and writes
nothing.

These are not matched:

| Case | Why | Rule that applies instead |
| --- | --- | --- |
| A submodule declared in `.gitmodules` that is not a registered worktree | It is a real submodule, not a run worktree | None |
| Loose files from an unregistered copy under `worktrees/` (a copy with no `.git`) | Nothing is registered at that path | `nested-worktree` |
| Ignored files inside a registered worktree under `worktrees/` | They belong to another checkout, not to this repository's index (issue #857) | None |

### It cannot be allowlisted

An `.amplihack-artifact-allowlist` entry that matches a registered worktree
path has no effect on this rule. The allowlist is repository content: an agent
working in the pull request's worktree can edit it, so it cannot be the thing
that authorizes committing that agent's own worktree.

### Output and remediation

```text
Artifact Guard blocked 1 prohibited artifact path(s) introduced by this change in /home/dev/src/shop-api (mode: pre-commit).
Baseline: origin/main (4ccd1977a1b2).

source           provenance     path                                             rule
staged           introduced     worktrees/feat/pr-1530                           registered-worktree

Remediation:
  - Remove local artifact leftovers from the parent worktree.
  - Move generated, plugin, cache, and runtime output into an ignored isolated directory outside the parent worktree.
  - If intentional source material, add a narrow reviewed entry to .amplihack-artifact-allowlist.
  - A registered worktree is a live checkout: unstage it, never delete it, and commit from inside it. The allowlist does not apply to registered-worktree.

Exact commands for the paths above:
  git restore --staged -- 'worktrees/feat/pr-1530'   # unstage only; keep the directory
  # keep 'worktrees' on disk: it holds the live worktree(s) worktrees/feat/pr-1530
  amplihack hygiene artifact-guard --repo /home/dev/src/shop-api --mode pre-commit   # re-run to confirm

First violation detail:
  worktrees/feat/pr-1530: registered git worktree in this repository's index; unstage it and keep the directory (it is a live checkout), then commit from inside that worktree. Cannot be allowlisted.
```

When a grouped root is, contains, or sits inside a registered worktree, the
guard prints one index-only command per path instead of the usual
`git rm -r --cached` / `rm -rf` pair for the root. This applies to every
violation under that root, whatever its rule, so an unregistered leftover at
`worktrees/feat/old/` next to a live `worktrees/feat/pr-1530` never produces
`rm -rf -- 'worktrees'`.

| Source of the violation | Command printed |
| --- | --- |
| `staged` | `git restore --staged -- '<path>'   # unstage only; keep the directory` |
| `tracked` | `git rm --cached -- '<path>'   # untrack only; keep the directory` |
| `untracked`, `ignored-present` | No command. A comment names the path and says to remove it by hand only if it is not a live worktree. |

Roots that hold no registered worktree keep their current output.

The commands are chosen because the obvious alternatives fail or destroy work:

- `rm -rf` on a live worktree deletes the run's uncommitted work.
- `git rm` without `--cached` on a gitlink tries to remove the linked
  worktree's files.
- `git rm --cached` on a *newly staged* gitlink fails whenever the worktree has
  any local change, which a run worktree mid-run nearly always has:
  `error: the following file has staged content different from both the file
  and the HEAD`. `git restore --staged` removes the new entry from the index in
  every case. For a gitlink that is already committed, `git restore --staged`
  would put it back, so `tracked` entries get `git rm --cached`, which works
  even when the worktree is dirty.

Every printed path, and the `--repo` value in the re-run line, is shell-quoted
(a `'` becomes `'\''`), so a path containing a quote or a space can be pasted
as is. Plain paths print exactly as before.

The guard itself stays read-only. After a refusal the index is left as it was,
so the staged state can be inspected before anything is unstaged.

Matching is byte-wise and case-sensitive. On a case-insensitive filesystem, an
entry staged under a different spelling of a worktree path is not matched by
this rule.

## Allowlist configuration

The default allowlist path is `.amplihack-artifact-allowlist`. This file is
repo-controlled configuration and should be reviewed like a security-sensitive
change because it can permit generated artifacts that the guard would otherwise
block.

Format:

```text
# Blank lines and comments are ignored.
# Entries are repo-relative and use / separators.

tests/fixtures/plugin-output/dist/plugin.js
docs/fixtures/generated-manifests/*.json
examples/minimal-node-project/node_modules/.package-lock.json
```

Matching semantics:

| Rule | Behavior |
| --- | --- |
| Path root | Entries are relative to the repository root |
| Separators | `/` only; Windows-style `\` separators are rejected |
| Case | Case-sensitive matching |
| Exact paths | `tests/fixtures/dist/plugin.js` matches only that path |
| `*`, `?` | Supported within a path segment |
| `**` | Supported across path segments |
| Directory entries | Must use an explicit suffix such as `tests/fixtures/output/**`; a bare directory does not imply recursive matching |
| Duplicates | Duplicate entries are allowed but normalized to one effective rule |
| Comments | Lines beginning with `#` after optional whitespace are ignored |

Valid entries are narrow and intentional:

```text
tests/fixtures/plugin-output/dist/plugin.js
tests/fixtures/plugin-output/dist/**
examples/generated-output/build/expected-manifest.json
```

Invalid entries fail closed with exit code `2`:

```text
/absolute/path
../outside-repo
node_modules/
node_modules/**
**/node_modules/**
dist/*
dist/
dist/**
*.log
build/**
*
**/*
```

Directory allowlists are accepted only for narrow fixture or example paths. They
must not exempt a default prohibited directory directly at the repository root or
across the whole repository.

No entry applies to the `registered-worktree` rule. The guard does not reject
such an entry, because it may still be needed for another rule, but it never
suppresses a `registered-worktree` violation. See
[It cannot be allowlisted](#it-cannot-be-allowlisted).

## Workflow and pre-commit coverage

Every bundled commit path runs Artifact Guard. Where it runs depends on who
does the staging.

**Deterministic bash steps guard, then stage.** The next command is a literal
`git add -A`, which stages exactly the paths a gate mode has just scanned:
staged, tracked, and untracked-but-not-ignored. It never stages an ignored
path. Guarding first therefore sees everything the add can bring in, and a
refusal leaves the index untouched.

| Recipe | Step | Mode | Order |
| --- | --- | --- | --- |
| `workflow-publish.yaml` | `step-14g-artifact-guard` | `pre-publish` | Standalone gate before publication |
| `workflow-publish.yaml` | `step-15-commit-push` | `pre-publish` | Guard → `git add -A` → commit |
| `workflow-finalize.yaml` | `step-20a-artifact-guard` | `pre-publish` | Standalone gate before finalization |
| `workflow-finalize.yaml` | `step-20b-push-cleanup` | `pre-publish` | Guard → `git add -A` → commit |
| `workflow-tdd.yaml` | `checkpoint-after-implementation` | `pre-publish` | Guard → `git add -A` |
| `workflow-pr-review.yaml` | `step-18c-push-feedback-changes` | `pre-publish` | Guard → `git add -A` |
| `workflow-refactor-review.yaml` | `checkpoint-after-review-feedback` | `pre-publish` | Guard → `git add -A` |
| `consensus-publish.yaml` | `step9-commit` | `pre-publish` | Guard → `git add -A` |
| `consensus-pr-feedback.yaml` | `step12-push-updates` | `pre-publish` | Guard → `git add -A` |

**Agent commit paths stage, then guard, then commit.** An agent can run any
`git add`, and Git itself suggests `-f` when a path is ignored. A forced add
stages ignored paths (a `worktrees/` gitlink, `node_modules/.vite`,
`.claude/runtime`) that a gate mode does not look for before staging, because
an ignored path cannot normally reach a commit. Only a guard that reads the
index after staging, immediately before `git commit`, sees them. In issue
#1528 the guard ran first, then the agent staged with `-f`, then committed, and
the guard never saw what was committed.

| Recipe | Step | Mode | Order |
| --- | --- | --- | --- |
| `autodrive-crusty-round.yaml` | `step-04-address-concerns` (agent) | `pre-commit` | `autodrive_pr_worktree.sh commit`: `git add -A` → guard → `git commit` → push |
| `autodrive-merge-round.yaml` | `step-04-address-blockers` (agent) | `pre-commit` | `autodrive_pr_worktree.sh commit`: `git add -A` → guard → `git commit` → push |
| `workflow-publish.yaml` | `step-16b-outside-in-fix-loop` (agent prompt) | `pre-publish` | `git add -A && amplihack hygiene artifact-guard … && git commit` |

The auto-drive helper also refuses to stage anywhere but the pull request's own
worktree. See
[Where phases 2 and 3 run](reference/auto-drive-to-merge.md#where-phases-2-and-3-run).
When the guard refuses there, the helper exits `3` and leaves the index as it
is, so the refusal can be inspected and the printed commands followed.

A deterministic step's guard still catches anything an agent staged earlier in
the same worktree: the gate modes always scan staged entries, and the
`registered-worktree` rule scans the whole index.

Future recipe changes must preserve both rules:

- A deterministic `git add -A` or equivalent broad-staging step needs an
  Artifact Guard gate immediately before it.
- A commit that an agent makes goes through a path that runs the guard after
  staging and immediately before `git commit`. Agent instructions never tell an
  agent to run `git add -f` or `--force`, or to stage anything under
  `worktrees/` or another worktree.

Before those gates run, bundled workflows also run the narrow workflow runtime
preflight documented in [Workflow Runtime Artifacts Reference](reference/workflow-runtime-artifacts.md).
That preflight removes only known workflow-owned `.claude/runtime` and
root-level `worktrees/` leftovers from the active task worktree. In
amplihack-managed task worktrees, root-level `worktrees/` is reserved for
workflow-owned nested scratch worktrees; tracked source under that path is a
repository layout conflict and must fail closed rather than be deleted.
Artifact Guard itself remains non-mutating and still fails on every unexpected
artifact.

The checked-in pre-commit hook scans the repository's committable state —
staged, tracked, and untracked-but-not-ignored paths. It runs at `git commit`
time, after all staging, so it sees the final index, including anything staged
with `-f`. It does not block on
ignored-present leftovers (that is what `--mode worktree`/`all` are for). It is
defined in `.pre-commit-config.yaml`, and that file is the source of truth for
the hook contract:

```yaml
- repo: local
  hooks:
    - id: artifact-guard
      name: amplihack artifact guard
      entry: bash -c 'CARGO_TARGET_DIR="${TMPDIR:-/tmp}/amplihack-precommit-target" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'
      language: system
      pass_filenames: false
      always_run: true
```

`pass_filenames: false` is required. Git normally passes only staged filenames to
pre-commit hooks, which would miss untracked artifact leftovers that a later
`git add -A` could sweep into the commit. Artifact Guard must inspect repository
state itself.

Run the hook through pre-commit:

```bash
pre-commit run artifact-guard --all-files
```

Or run the same guard command directly from a source checkout:

```bash
CARGO_TARGET_DIR="${TMPDIR:-/tmp}/amplihack-precommit-target" \
  cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit
```

Contract tests parse the hook entry as shell tokens so legal Cargo option
ordering does not matter. These source-checkout forms are equivalent for the
Artifact Guard contract:

```bash
CARGO_TARGET_DIR="${TMPDIR:-/tmp}/amplihack-precommit-target" \
  cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit

CARGO_TARGET_DIR="${TMPDIR:-/tmp}/amplihack-precommit-target" \
  cargo run --locked --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit

CARGO_TARGET_DIR="${TMPDIR:-/tmp}/amplihack-precommit-target" \
  cargo run --bin amplihack --locked -- hygiene artifact-guard --repo . --mode pre-commit
```

`--locked` is a Cargo option, not an Artifact Guard argument, even when it
appears between `cargo run` and `--bin`. `CARGO_TARGET_DIR` must still isolate
build output outside the repository — outside, not specifically on `/tmp`. The
forms above set it inline because they are run by hand; the checked-in hook
instead runs through `scripts/git-hook-cargo.sh`, which defaults it to a
per-checkout directory under `${XDG_CACHE_HOME:-$HOME/.cache}/amplihack` (issues
#1381 and #1440) and leaves an explicitly set `CARGO_TARGET_DIR` alone.

## Output isolation

The preferred fix for a violation is output isolation, not allowlisting.
Allowlisting is only for intentional checked-in fixtures or reviewed generated
artifacts.

These locations are intentionally not prohibited by default:

```text
<repo>/.amplihack/runtime/
<repo>/.amplihack/cache/
<repo>/.amplihack/generated/
<git-common-dir>/.claude/runtime/
/tmp/amplihack-<purpose>-<id>/
```

Use `CARGO_TARGET_DIR` for workflow-owned Rust builds that need to avoid the
repo-local `target/` directory:

```bash
CARGO_TARGET_DIR=.amplihack/cache/cargo-target cargo test --workspace
```

Avoid writing generated output directly to:

```text
<repo>/node_modules/
<repo>/dist/plugin.js
<repo>/worktrees/
<repo>/build/
```

## Workflow runtime cleanup preflight

`default-workflow` and recovery flows use external runtime roots for generated
agent state, provenance, logs, metrics, and reflection output. The runtime root
contract is documented in [Workflow Runtime Isolation](features/workflow-runtime-isolation.md).

As defense-in-depth, workflow lifecycle steps run
`preflight_known_workflow_runtime_artifacts "$worktree"` before checkpoint,
broad staging, publish, pre-commit-related staging, and finalization/status
gates. The preflight is intentionally narrower than Artifact Guard:

| Path | Preflight behavior | Artifact Guard behavior if still present |
| --- | --- | --- |
| `.claude/runtime` | Remove when it is exactly under the active task worktree. | Exempt when untracked/ignored; blocked as `claude-runtime` only if staged or tracked. |
| `worktrees/` | Remove when it is exactly under the active task worktree and not tracked source. | Block as `nested-worktree`; an index entry at a registered worktree path blocks as `registered-worktree`. |
| `.claude/settings.json` | Preserve. | Not blocked by the runtime rule. |
| Unrelated untracked files | Preserve. | Block when they match prohibited artifact rules or dirty-worktree gates. |

The preflight is not an allowlist. It is a cleanup step for workflow-owned
runtime paths that should have been isolated outside the worktree. If cleanup
fails or the paths remain afterward, the lifecycle gate fails visibly.

## Intended Rust API

The guard core should live in `amplihack_utils::artifact_guard` so CLI commands,
recipes, and tests share one implementation. This is the intended public shape;
implementation may rename fields only if this document is updated in the same
change.

```rust
pub struct ArtifactGuardConfig {
    pub repo_path: PathBuf,
    pub mode: ArtifactGuardMode,
    pub allowlist_path: Option<PathBuf>,
}

pub enum ArtifactGuardMode {
    All,
    Staged,
    Worktree,
    PreCommit,
    PrePublish,
}

impl ArtifactGuardMode {
    /// Whether this mode scans staged (indexed) paths.
    fn scans_staged(self) -> bool;

    /// Whether this mode scans tracked and untracked worktree paths.
    fn scans_worktree(self) -> bool;

    /// Whether this mode scans ignored-present artifacts (gitignored,
    /// untracked paths that exist in the worktree).
    ///
    /// Returns `true` only for the local-hygiene modes `All` and `Worktree`.
    /// The commit/publish gates `PreCommit`, `PrePublish`, and `Staged` return
    /// `false`, because a gitignored path can never be committed or published
    /// and is outside the guard's threat model for those gates.
    ///
    /// Concretely: `matches!(self, Self::All | Self::Worktree)`.
    fn scans_ignored_present(self) -> bool;
}

pub enum ArtifactSource {
    Staged,
    Tracked,
    Untracked,
    IgnoredPresent,
}

pub enum ArtifactProvenance {
    /// The change under review staged, committed, or left this path.
    Introduced,
    /// Committed on the baseline and untouched by the change under review.
    PreExisting,
}

pub struct ArtifactViolation {
    pub path: String,
    pub source: ArtifactSource,
    pub rule_id: String,
    pub message: String,
    pub provenance: ArtifactProvenance,
}

pub struct ArtifactGuardReport {
    pub repo_root: PathBuf,
    pub mode: ArtifactGuardMode,
    pub violations: Vec<ArtifactViolation>,
    pub baseline: Option<ArtifactBaseline>,
    pub preexisting_policy: PreExistingPolicy,
    /// Repo-relative paths (no trailing `/`) of every registered Git worktree
    /// strictly inside `repo_root`, from `git worktree list --porcelain`.
    /// The CLI uses it to never print a deleting command for a root that holds
    /// a live worktree.
    pub registered_worktrees: Vec<String>,
}

impl ArtifactGuardReport {
    /// Violations that must fail the caller (exit 1).
    fn blocking_violations(&self) -> Vec<&ArtifactViolation>;
    /// Pre-existing violations reported without failing a gate mode.
    fn advisory_violations(&self) -> Vec<&ArtifactViolation>;
    fn blocks(&self) -> bool;
}

pub fn run_artifact_guard(
    config: ArtifactGuardConfig,
) -> Result<ArtifactGuardReport, ArtifactGuardError>;
```

`run_artifact_guard` resolves the repository root, validates the allowlist,
collects candidate paths from Git, applies prohibited rules, applies allowlist
exceptions, and returns a structured report. It must not mutate the repository.

Errors are fail-closed:

| Error class | Examples |
| --- | --- |
| Repository errors | `--repo` is not a Git worktree, Git command fails |
| Path errors | Path escapes repo root, path cannot be normalized |
| Mode errors | Unknown CLI mode |
| Allowlist errors | Unreadable file, absolute path, parent traversal, broad exemption |

## Fixing violations

For a blocked commit:

```bash
amplihack hygiene artifact-guard --repo . --mode staged
git restore --staged dist/plugin.js
```

Then move the build output to an isolated location or remove the local artifact
if it is not needed.

For a commit blocked by `registered-worktree`, unstage the entry and leave the
directory alone. It is a live checkout, possibly holding a run's uncommitted
work:

```bash
git restore --staged -- 'worktrees/feat/pr-1530'   # newly staged entry
git rm --cached -- 'worktrees/feat/pr-1530'        # entry already committed
```

Never `rm -rf` the path or its `worktrees/` parent, and never run `git rm`
without `--cached` on it. Make the change you meant to commit from inside that
worktree (`cd worktrees/feat/pr-1530`), where its files are ordinary tracked
files and no `-f` is needed.

For ignored leftovers before publication:

```bash
amplihack hygiene artifact-guard --repo . --mode all
```

Use `--mode all` (or `--mode worktree`) to surface ignored-present leftovers.
The `pre-commit` and `pre-publish` gates deliberately do not report them,
because gitignored, untracked paths cannot be committed or published. If the
guard reports `node_modules/`, `.pytest_cache/`, `.cache/`, or another
ignored-present artifact under those hygiene modes, relocate or remove the local
output. Do not treat `.gitignore` as approval to keep parent-worktree pollution
— clean it up locally even though it will not block a commit or a publish.

For a pre-existing condition the gate reported without blocking, nothing is
required to finish the current change. Clean it up in its own change:

```bash
amplihack hygiene artifact-guard --repo . --mode all   # full audit, exits 1
git rm -r --cached -- node_modules
printf 'node_modules/\n' >> .gitignore
git commit -m 'chore: untrack vendored node_modules'
```

Do not widen the allowlist to cover it. A directory-wide entry such as
`node_modules/**` is rejected on purpose (see
[Allowlist configuration](#allowlist-configuration)); provenance, not a broad
exemption, is what keeps the pre-existing tree from stopping unrelated work.

For an intentional fixture, add the narrowest allowlist entry that preserves the
test:

```text
tests/fixtures/plugin-output/dist/plugin.js
```

Commit the allowlist change with the fixture. Reviewers should confirm the
artifact is necessary, deterministic, and safe to keep in the repository.

## Worked example: cache leftovers do not block a commit or publish

This example reproduces the scenario from issue #928. A normal test or install
run leaves gitignored, untracked cache directories in the worktree. Those
directories can never be committed or published, so the commit and publication
gates must pass. The exit codes below assume the #928 narrowing is applied —
that is, `ArtifactGuardMode::scans_ignored_present()` returns `true` only for
`All` and `Worktree`.

Set up a repository whose `.gitignore` excludes common cache directories, then
create those directories as untracked leftovers:

```bash
cd my-repo
printf '.pytest_cache/\nnode_modules/\n' >> .gitignore
mkdir -p .pytest_cache node_modules/some-dep
touch .pytest_cache/CACHEDIR.TAG node_modules/some-dep/index.js
```

`git status --ignored` confirms both directories are ignored and untracked. The
commit and publication gates pass (exit code `0`) because ignored-present paths
are outside their scope:

```bash
amplihack hygiene artifact-guard --repo . --mode pre-commit   # exit 0
amplihack hygiene artifact-guard --repo . --mode pre-publish  # exit 0
```

The local-hygiene modes still surface the same leftovers (exit code `1`) so you
can clean them up before they accumulate:

```bash
amplihack hygiene artifact-guard --repo . --mode worktree     # exit 1
amplihack hygiene artifact-guard --repo . --mode all          # exit 1
```

The gates remain fail-closed for anything that *could* reach a commit. A cache
directory that is not covered by `.gitignore` is untracked-but-committable, so
it still blocks `pre-commit` and `pre-publish`:

```bash
git rm -r --cached --ignore-unmatch . >/dev/null 2>&1 || true
: > .gitignore   # stop ignoring the cache dirs
amplihack hygiene artifact-guard --repo . --mode pre-publish  # exit 1
```

Likewise, staging or tracking a prohibited artifact blocks every gate mode:

```bash
git add -f dist/plugin.js
amplihack hygiene artifact-guard --repo . --mode pre-commit   # exit 1
amplihack hygiene artifact-guard --repo . --mode pre-publish  # exit 1
```

| Scenario | `pre-commit` / `pre-publish` | `worktree` / `all` |
| --- | --- | --- |
| Gitignored, untracked `.pytest_cache/`, `node_modules/` | Pass (exit 0) | Flag (exit 1) |
| Untracked cache dir **not** in `.gitignore` | Flag (exit 1) | Flag (exit 1) |
| Staged or tracked artifact this change introduced | Flag (exit 1) | Flag (exit 1) |
| Tracked artifact committed on the baseline, untouched here | Report, pass (exit 0) | Flag (exit 1) |

## Worked example: a pre-existing tracked tree does not stop unrelated work

This example reproduces issue #1422. The repository has `node_modules`
committed into Git long before the current change began, and the change under
review only pins dependency versions:

```bash
git checkout -b task/pin-dependency-versions origin/main
$EDITOR package.json && git commit -am 'pin dependency versions'
amplihack hygiene artifact-guard --repo . --mode pre-publish   # exit 0
```

The gate prints all 668 pre-existing paths, grouped under `node_modules/`, with
the baseline they were measured against and a way forward — and passes, because
the tracked tree is already on `origin/main` and cannot enter this change's
diff. The moment the change adds an artifact of its own, the gate refuses:

```bash
git add -f dist/plugin.js && git commit -m 'oops'
amplihack hygiene artifact-guard --repo . --mode pre-publish   # exit 1
```

The refusal names `dist/plugin.js`, marks it `introduced`, and prints the exact
`git rm -r --cached -- 'dist'` that clears it. The pre-existing tree is still
listed above it, still not the reason for the refusal.

To audit or enforce the whole condition anyway:

```bash
amplihack hygiene artifact-guard --repo . --mode all                      # exit 1
amplihack hygiene artifact-guard --repo . --mode pre-publish --preexisting block  # exit 1
```

## Worked example: a run worktree staged in the launch checkout

This example reproduces issue #1528 in a scratch repository. The main checkout
is on `main`, ignores `worktrees/`, and has two linked worktrees: one in the
usual place and one outside `worktrees/`.

```bash
git init -b main shop-api && cd shop-api
printf 'worktrees/\n' > .gitignore
git add -A && git commit -m init
git worktree add worktrees/feat/pr-1530 -b feat/pr-1530
git worktree add wt/review -b review
```

A plain `git add` of the ignored worktree is refused, with the hint the agent
followed:

```text
$ git add worktrees/feat/pr-1530
The following paths are ignored by one of your .gitignore files:
worktrees
hint: Use -f if you really want to add them.
```

Forcing it stages a gitlink. `wt/review` is not ignored, so a plain `git add`
stages it the same way:

```bash
git add -f worktrees/feat/pr-1530
git add wt/review
git ls-files -s
# 100644 48bafbcc… 0	.gitignore
# 160000 ffad6555… 0	worktrees/feat/pr-1530
# 160000 ffad6555… 0	wt/review
```

Both gate modes refuse, and name both paths with rule `registered-worktree`:

```bash
amplihack hygiene artifact-guard --repo . --mode pre-commit    # exit 1
amplihack hygiene artifact-guard --repo . --mode pre-publish   # exit 1
```

An allowlist entry does not change that:

```bash
printf 'wt/review\n' > .amplihack-artifact-allowlist
amplihack hygiene artifact-guard --repo . --mode pre-commit    # exit 1, still names wt/review
rm .amplihack-artifact-allowlist
```

The printed commands unstage both entries and leave both worktrees in place:

```bash
git restore --staged -- 'worktrees/feat/pr-1530' 'wt/review'
amplihack hygiene artifact-guard --repo . --mode pre-commit    # exit 0
git worktree list                                               # all three still listed
```

Run from inside a linked worktree, the guard does not report the main
checkout, which is registered too:

```bash
cd worktrees/feat/pr-1530
amplihack hygiene artifact-guard --repo . --mode pre-commit    # exit 0
```

| Scenario | `pre-commit` / `pre-publish` |
| --- | --- |
| Staged gitlink to a registered worktree under `worktrees/` | Block (exit 1), `registered-worktree` |
| Staged gitlink to a registered worktree outside `worktrees/` | Block (exit 1), `registered-worktree` |
| Same, with an allowlist entry for the path | Block (exit 1) |
| `.gitmodules` submodule that is not a registered worktree | Pass (exit 0) |
| Staged loose files from an unregistered copy under `worktrees/` | Block (exit 1), `nested-worktree` |
| Guard run inside the linked worktree; main checkout registered | Pass (exit 0) |
| Committed gitlink on the baseline, untouched by this change | Report, pass (exit 0) under `--preexisting report` |

## Review expectations

Review these changes carefully:

1. New or changed `.amplihack-artifact-allowlist` entries.
2. Changes to prohibited rules.
3. Recipe edits around `git add -A`, publication, finalization, or PR creation.
4. Agent prompts that commit: they must stage before the guard, never mention
   `git add -f` except to forbid it, and commit only in the worktree they were
   given.
5. Pre-commit changes that remove `pass_filenames: false`.
6. Build, plugin, or runtime changes that redirect outputs into the parent
   worktree.

Reviewers should verify that generated and runtime outputs are isolated, guard
failures are visible, and allowlist entries are not used as substitutes for
proper output placement.

## Related documentation

- [Recipe CLI Reference](reference/recipe-cli-reference.md)
- [Auto Drive To Merge: where phases 2 and 3 run](reference/auto-drive-to-merge.md#where-phases-2-and-3-run) — the PR worktree and the commit helper that runs this guard after staging
- [Pre-Commit Diagnostics](claude/agents/amplihack/specialized/pre-commit-diagnostic.md)
- [Developing amplihack](DEVELOPING_AMPLIHACK.md)
