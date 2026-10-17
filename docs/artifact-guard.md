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
- [Run worktrees in the index](#run-worktrees-in-the-index)
- [Allowlist configuration](#allowlist-configuration)
- [Workflow and pre-commit coverage](#workflow-and-pre-commit-coverage)
- [Output isolation](#output-isolation)
- [Workflow runtime cleanup preflight](#workflow-runtime-cleanup-preflight)
- [Intended Rust API](#intended-rust-api)
- [Fixing violations](#fixing-violations)
- [Worked example: cache leftovers do not block a commit or publish](#worked-example-cache-leftovers-do-not-block-a-commit-or-publish)

## Behavior

Artifact Guard:

1. Scan repo-relative paths only.
2. Check staged paths before commit, and refuse any index entry that records a
   run worktree. See [Run worktrees in the index](#run-worktrees-in-the-index).
3. Check tracked and untracked paths before broad staging and workflow
   publication.
4. Check ignored-present artifact paths only in the `worktree` and `all` modes,
   which are local-hygiene scans. The commit- and publication-gate modes
   (`pre-commit`, `pre-publish`) never block on ignored-present artifacts.
5. Separate artifacts the change under review introduced from artifacts the
   repository already carried, and block the gate modes only on the former. See
   [Provenance](#provenance-this-change-vs-the-repositorys-history).
6. Fail closed on invalid configuration, invalid paths, Git failures, and unsafe
   allowlist entries.
7. Print actionable remediation before exiting, naming the offending paths and
   the exact commands that clear them. Never print a command that deletes a
   directory holding live worktrees.
8. Leave the repository unchanged.

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
| `2` | The guard could not complete because configuration, paths, mode, allowlist, or Git state was invalid, or because `git worktree list` failed or printed records the guard cannot trust. Nothing but the error and a hint line is printed. |

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

Configuration errors exit with code `2` and print one error line and one hint
line on stderr:

```text
Artifact Guard configuration error: artifact guard allowlist error: /repo/.amplihack-artifact-allowlist:4 unsafe allowlist entry `/tmp/plugin.js`: absolute paths are not allowed
Fix the repository path, Git state, mode, allowlist, or baseline and rerun the guard.
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
| Run worktree gitlinks (`run-worktree-gitlink`) | A gitlink under `worktrees/`, or at the path of a registered worktree such as `wt/review` | Not applicable: the rule reads the index. Blocks in every mode and cannot be allowlisted. See [Run worktrees in the index](#run-worktrees-in-the-index). |
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

## Run worktrees in the index

The `run-worktree-gitlink` rule refuses a commit whose index records a run
worktree. Git records a directory that holds another checkout as a *gitlink*:
an index entry with mode `160000` that stores a commit ID instead of file
contents. A committed run-worktree gitlink publishes a pointer to a commit no
clone can fetch, and the `git add -A` that staged it usually staged other
unwanted files with it.

### How a run worktree reaches the index

Issue #1528 found run worktrees under `worktrees/` staged in a repository's
main checkout, although that checkout's `.gitignore` excluded `worktrees/`.
Ignore rules stop `git add -A` and named paths, so something forced the add:

1. Auto-drive rounds ran in the launch checkout. `auto-drive-to-merge.yaml`
   passed `repo_path: "."` to both loops, and each round's `step-04` agent ran
   there, on `main`, with a prompt that said: guard, `git add -A`, commit, push.
2. The pull request's files live under that checkout's ignored
   `worktrees/<branch>/`. `git add -A` staged nothing, and naming a path was
   refused as ignored, so the agent forced it. A forced add records the nested
   checkout as a gitlink, or, as `-A -f` or `-f .`, sweeps up every ignored file
   under the pathspec, such as `.claude/runtime/`, `node_modules/.vite/` and an
   aborted worktree copy with no `.git`.
3. The guard ran only before `git add`, so it never saw what was staged.
4. Once a gitlink is in the index, the ignore rule no longer applies to it, and
   every later `git add -A` refreshes it to the worktree's current commit.

The forced add, reproduced:

```bash
git init -b main shop-api && cd shop-api
printf 'worktrees/\n' > .gitignore
git add .gitignore && git commit -m init
git worktree add worktrees/feat/pr-1530 -b feat/pr-1530
git add -A                                 # stages nothing: worktrees/ is ignored
git add -f worktrees/feat/pr-1530          # forced: stages a gitlink
git ls-files -s worktrees
# 160000 <commit of feat/pr-1530> 0	worktrees/feat/pr-1530
amplihack hygiene artifact-guard --repo . --mode pre-commit   # exit 1: run-worktree-gitlink
```

The same gitlink appears without `-f` in a checkout whose `.gitignore` does not
cover `worktrees/` (Git warns `adding embedded git repository` and stages it).

Auto-drive rounds now commit only inside the pull request's worktree, and run
this guard after staging; see
[Where phases 2 and 3 run](reference/auto-drive-to-merge.md#where-phases-2-and-3-run).
The rule refuses the entry however it got there, so it does not depend on that
fix.

### What the rule flags

The rule reads the whole index with `git ls-files -s -z` and keeps entries
whose mode is exactly `160000`. Each path is reported once, even when a merge
conflict gives it several stages. Because it reads the index rather than the
staged diff, it also catches a gitlink already committed in `HEAD`.

A gitlink at repo-relative path `p` is flagged when either condition holds:

| Condition | Example | Catches |
| --- | --- | --- |
| (a) The first component of `p` is `worktrees`, compared ignoring ASCII case | `worktrees/feat/pr-1530`, `Worktrees/feat/pr-1530` | Every run worktree in the standard location, registered or not |
| (b) `p` equals, or lies under, the repo-relative path of a worktree listed by `git worktree list --porcelain` | `wt/review`, `.worktrees/review` | A registered worktree anywhere else in the repository |

For (b), the scanned top level and every listed worktree path are
canonicalized. A path that cannot be canonicalized, such as a *prunable*
worktree whose directory is gone, is used as Git printed it. Listed paths
outside the top level, and the top level itself, are dropped. The rest become
repo-relative paths with `/` separators. `p` must equal one of them or lie under
one, component by component. The same sorted, de-duplicated list is the
report's `registered_worktrees` field.

Every scan reads `git worktree list --porcelain` first, in every mode, whether
or not the index holds a gitlink. The CLI also uses the list to decide which
roots it may never print a delete command for, so a list it cannot trust ends
the scan; see [When the worktree list cannot be read](#when-the-worktree-list-cannot-be-read).
The ignored-present scan in `worktree` and `all` modes uses the same list to
skip files inside registered worktrees (issue #857).

Each flagged path produces one violation:

| Field | Value |
| --- | --- |
| `rule_id` | `run-worktree-gitlink` |
| `source` | `staged` when the entry differs from `HEAD` (it is listed by `git diff --cached`), otherwise `tracked` |
| `provenance` | Always `introduced` |

The rule claims its paths: a flagged gitlink is not reported again under
another rule, such as `nested-worktree` for `worktrees/…` or `build-artifact`
for `build/…`. Gitlink violations are listed before all others.

The rule is active in every mode: `pre-commit`, `pre-publish`, `staged`,
`all` and `worktree`. It reads the index, not the staged diff, so even
`--mode worktree` reports a committed gitlink.

### What the rule does not flag

| Case | Why | Rule that applies instead |
| --- | --- | --- |
| A real submodule outside `worktrees/` that is not a registered worktree | It is repository content, not a run worktree | None |
| A submodule inside a pull request's worktree, with the guard run in that worktree | Relative to that worktree's top level its path is not under `worktrees/`, and the main checkout and sibling worktrees lie outside the scanned top level | None |
| Regular files under `worktrees/`, such as a copy with no `.git` | They are not gitlinks | `nested-worktree` |

### Nothing softens the rule

- **The allowlist does not apply.** The guard never consults it for this rule,
  so even an exact entry such as `worktrees/feat/pr-1530` has no effect. The
  allowlist is repository content that an agent working on the pull request
  can edit; it cannot authorize committing that agent's own worktree.
- **Provenance does not apply.** The violation is always `introduced`, so
  `--preexisting report` (the default), `--baseline`,
  `AMPLIHACK_ARTIFACT_GUARD_BASELINE` and
  `AMPLIHACK_ARTIFACT_GUARD_PREEXISTING` do not change the result.
- **There is no switch.** No flag or environment variable turns the rule off.

### When the worktree list cannot be read

If `git worktree list --porcelain` fails or prints records the guard cannot
trust, the guard exits `2` in every mode instead of guessing. It prints no
table and no commands, only the configuration error and the hint line:

```text
Artifact Guard configuration error: artifact guard could not list worktrees: ambiguous worktree records
Fix the repository path, Git state, mode, allowlist, or baseline and rerun the guard.
```

| Situation | Error text |
| --- | --- |
| Git exits non-zero, is killed, or cannot be started | `artifact guard git command failed: git worktree list --porcelain …`, with Git's exit code and stderr (escaped) |
| The output has no `worktree` records (Git always lists the main working tree) | `artifact guard could not list worktrees: no worktree records` |
| A `worktree` record's path is not absolute | `artifact guard could not list worktrees: a worktree record is not an absolute path` |
| A `worktree` line arrives before the previous record's `HEAD` or `bare` line, or a non-blank line comes before the first record | `artifact guard could not list worktrees: ambiguous worktree records` |

The last three are fixed strings that never contain Git's output.

Records are separated by blank lines and start with `worktree <path>`. The
`HEAD`, `bare`, `branch`, `detached`, `locked` and `prunable` lines are
accepted, and any other line inside a record is ignored. Prunable and locked
worktrees still count. The guard does not use
`-z`, which needs Git 2.36.

A failed list is never treated as an empty one: that would hide every
registered worktree outside `worktrees/` from condition (b). The ambiguity
check also covers a worktree path that contains a newline followed by
`worktree <path>`, which Git prints as two records. A forged record can only
make the guard refuse, never hide a gitlink, because the whole scan fails.

### Output and remediation

The refusal for a gitlink staged in the main checkout at
`/home/dev/src/shop-api`:

```text
Artifact Guard blocked 1 prohibited artifact path(s) introduced by this change in /home/dev/src/shop-api (mode: pre-commit).
Baseline: origin/main (4ccd1977a1b2).

source           provenance     path                                             rule
staged           introduced     worktrees/feat/pr-1530                           run-worktree-gitlink

Remediation:
  - A run worktree is a live checkout recorded as a gitlink: drop it from the index only, never delete it, and commit from inside the PR worktree. The allowlist does not apply to run-worktree-gitlink.

Exact commands for the paths above:
  git rm --cached -f -- 'worktrees/feat/pr-1530'   # index only: keeps the worktree and its files; -f only overrides git's up-to-date check
  amplihack hygiene artifact-guard --repo '/home/dev/src/shop-api' --mode pre-commit   # re-run to confirm

First violation detail:
  worktrees/feat/pr-1530: run worktree recorded as a gitlink; drop the entry from the index only (git rm --cached -f keeps the worktree and its files) and commit from inside the PR worktree; cannot be allowlisted
```

| Exit | Meaning |
| --- | --- |
| `0` | No blocking violation |
| `1` | At least one blocking violation. A `run-worktree-gitlink` violation always blocks. |
| `2` | A scan or configuration error, including an untrusted worktree list. No report and no commands are printed. |

- Gitlink rows lead the table, and gitlink lines lead the command list. There
  is no separate section.
- The gitlink bullet is printed only when at least one blocking violation is a
  `run-worktree-gitlink`. The general bullets are printed only when at least
  one blocking violation is something else.
- Each gitlink gets its own command line, never grouped under an artifact
  root such as `worktrees`.

Run the printed line from the checkout the guard scanned. It removes the entry
and leaves the worktree registered and on disk:

```bash
git rm --cached -f -- 'worktrees/feat/pr-1530'
amplihack hygiene artifact-guard --repo . --mode pre-commit   # exit 0
git worktree list                                             # worktrees/feat/pr-1530 is still listed
```

Then make the change you meant to commit from inside the worktree
(`cd worktrees/feat/pr-1530`), where its files are ordinary tracked files.

#### Why `git rm --cached -f`

| Part | Why |
| --- | --- |
| `--cached` | Touches the index only. The worktree directory, its files, its uncommitted work and its registration stay. |
| `-f` | Needed when the staged commit differs from both `HEAD` and the worktree's current commit, which is the normal state once a run commits after the entry was staged. Without it Git stops with `error: the following file has staged content different from both the file and the HEAD`. With `--cached`, `-f` still touches only the index. |

These are never printed for a gitlink:

| Command | Why not |
| --- | --- |
| `git restore --staged -- <path>` | It resets the entry to `HEAD`, so a gitlink already committed stays in the index. |
| `git rm --cached -- <path>` | Without `-f` it fails in the state described above. |
| `rm -rf -- <path>` | It deletes the run's checkout and its uncommitted work. |

For a gitlink already committed in `HEAD`, the removal is a staged deletion
(`D  wt/review` in `git status --short`), and the directory becomes untracked
again (`?? wt/`). Ignore it in the same change, or the next `git add -A` stages
it again:

```bash
git rm --cached -f -- 'wt/review'
printf 'wt/\n' >> .gitignore
git add .gitignore
git commit -m 'chore: drop the wt/review gitlink and ignore wt/'
```

#### Paths Git would read as a pattern

Git reads a pathspec that starts with `:` or contains `*`, `?` or `[` as a
pattern, so `git rm --cached -f -- 'wt*'` would also untrack `wtA` and
`wtB/deep/file.js`. For such a path, every `git rm` line the guard prints,
gitlink or root-grouped, carries the `:(literal)` magic prefix, and Git
matches the path exactly:

```text
  git rm --cached -f -- ':(literal)wt*'   # index only: keeps the worktree and its files; -f only overrides git's up-to-date check
  git rm -r --cached -- ':(literal)a[1]/dist'   # drop from the change, keep on disk
  rm -rf -- 'a[1]/dist'               # or delete the leftover outright
```

`rm -rf` is not Git and always takes the plain path. The `Clean up in a
dedicated change` line appends to `.gitignore` only for a plain root; for a
pattern-like root it untracks and commits without touching `.gitignore`.

#### Roots that are never deleted

Other violations keep their usual root-grouped pair: an index-only
`git rm -r --cached -- '<root>'` line, then `rm -rf -- '<root>'`. The guard
replaces the `rm -rf` line with a comment when the root is protected. The
registered case is checked first:

| Root | Comment printed instead of `rm -rf` |
| --- | --- |
| Holds a registered worktree, as `worktrees` or `build` does when `build/review` is registered | `# never delete '<root>': it holds registered worktree(s) — see git worktree list` |
| Its first component is `worktrees`, compared ignoring ASCII case, and nothing under it is registered | `# never delete '<root>': it is reserved for run worktrees — inspect it and remove leftovers by hand` |

This applies in every mode and to every rule. Run in a main checkout,
`rm -rf -- 'worktrees'` would delete every live run worktree. A directory under
`worktrees/` whose registration was removed, or never completed, can still hold
a run's uncommitted work, so it is protected too.

With a gitlink at `build/review` (a registered worktree) and a staged
`build/x.o`:

```text
Exact commands for the paths above:
  git rm --cached -f -- 'build/review'   # index only: keeps the worktree and its files; -f only overrides git's up-to-date check
  git rm -r --cached -- 'build'   # drop from the change, keep on disk
  # never delete 'build': it holds registered worktree(s) — see git worktree list
  amplihack hygiene artifact-guard --repo '/home/dev/src/shop-api' --mode pre-commit   # re-run to confirm
```

Run the lines in order. The root-grouped line has no `-f`, so it can fail on a
gitlink that the line before it has not yet removed.

#### Quoting

Every value interpolated under `Exact commands` is shell-quoted: wrapped in
single quotes, with each `'` written as `'\''`. A branch-derived path such as
`worktrees/feat/o'brien-fix` prints as `'worktrees/feat/o'\''brien-fix'`, and a
path containing `$(…)` is pasted without being expanded.

#### Unsafe characters

A path is unsafe when it contains a control character (U+0000–U+001F,
U+007F–U+009F, such as newline, tab or escape), a bidirectional or format
character (such as U+202E, U+2066, U+200E, U+200F or U+061C), or U+FFFD, which
stands in for bytes that are not UTF-8. Such a path is never put in a command.
The violation still blocks, and in place of its command the guard prints:

```text
  # no command for worktrees/o\u{202e}b: the path contains an unsafe character; inspect it and fix it by hand
```

Wherever unsafe text is printed (table path cells, this comment,
`First violation detail`, the repository path in the report header and the
clean line, and configuration errors, which can carry Git's stderr), it is
escaped: a newline prints as `\n`, a tab as `\t`, a carriage return as `\r`,
and any other as `\u{…}`, for example `\u{1b}`. The raw character is never
printed. When the repository path itself is unsafe, the re-run line is left
out. Errors raised before the scan starts, such as an invalid `--mode`, print
no hint line.

### Limitations

- **Worktree paths that contain a newline are not supported.** Without `-z`,
  Git prints such a path across two lines. The guard then exits `2` on an
  ambiguous record in every mode, which can block an unrelated commit until
  the worktree is moved or removed.
- **Condition (a) ignores case.** `Worktrees/` and `worktrees/` are the same
  directory on a case-insensitive filesystem such as the macOS default, so
  both are caught. A genuine submodule at `Worktrees/…` is refused on every
  filesystem; rename it.
- **Condition (b) compares canonical paths.** A prunable worktree registered
  through a symlinked parent directory is compared by the path Git recorded and
  can be missed. Condition (a) still catches it under `worktrees/`.
- **The rule runs where the guard runs.** That is the bundled recipe steps,
  plus the `artifact-guard` pre-commit hook in a repository that configures it,
  as this one does. `git commit --no-verify` skips the hook, and no CI job
  checks for gitlinks.

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

No entry applies to the `run-worktree-gitlink` rule. See
[Nothing softens the rule](#nothing-softens-the-rule).

## Workflow and pre-commit coverage

Artifact Guard runs before and after every broad staging operation in the
bundled recipes, not just publish and finalize. A guard before `git add -A`
cannot see a run-worktree gitlink that the add itself creates, so every commit
block runs guard, `git add -A`, guard, commit (issue #1528). The guarded recipe
set is every recipe that invokes `git add -A`:

| Recipe | Guard placement |
| --- | --- |
| `workflow-finalize.yaml` | Before and after final broad staging in `step-20b-push-cleanup`, which runs only at the top of the linked task worktree and skips with a warning otherwise. See [Workflow Runtime Artifacts: lifecycle integration](reference/workflow-runtime-artifacts.md#lifecycle-integration). |
| `workflow-publish.yaml` | Before and after publication staging, and before and after broad staging in the outside-in fix loop (one chained line ending in `\|\| { …; exit 1; }`) |
| `workflow-refactor-review.yaml` | Before and after broad staging |
| `workflow-tdd.yaml` | Before and after broad staging |
| `workflow-pr-review.yaml` | Before and after broad staging |
| `consensus-publish.yaml` | Before and after broad staging |
| `consensus-pr-feedback.yaml` | Before and after broad staging |
| `autodrive-crusty-round.yaml` | Before and after `git add -A` in the `step-04` commit block, which runs only inside the pull request's worktree. See [Auto Drive To Merge: the round commit block](reference/auto-drive-to-merge.md#the-round-commit-block). |
| `autodrive-merge-round.yaml` | Same as `autodrive-crusty-round.yaml` |

In bash steps, which run under `set -e`, the post-add guard shares the add
line: `git add -A; amplihack hygiene artifact-guard --repo . --mode pre-publish`.
Both guards use the same `--mode`. No bundled recipe or tool force-adds
(`git add -f`), since a forced add is how an ignored run worktree becomes a
gitlink.

Future recipe changes must preserve the rule: any new `git add -A` or equivalent
broad-staging step that commits needs an Artifact Guard gate immediately before
it and again between it and the commit, and each gate must be able to stop the
step.

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
staged, tracked, and untracked-but-not-ignored paths. It does not block on
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
| `worktrees/` | Remove when it is exactly under the active task worktree and not tracked source. Keep it, with a warning, while it holds a registered worktree or when `git worktree list` fails (issue #1528). | Block as `nested-worktree`; a gitlink blocks as `run-worktree-gitlink`. |
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

/// Rule id for a gitlink that records a run worktree (issue #1528).
pub const RUN_WORKTREE_GITLINK: &str = "run-worktree-gitlink";

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
    /// Repo-relative paths of the worktrees `git worktree list --porcelain`
    /// registers inside the scanned top level, prunable ones included.
    /// Sorted and de-duplicated; the top level itself and worktrees outside
    /// it are left out. Filled in every mode.
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
| Worktree list errors | `git worktree list --porcelain` prints untrustworthy records, in any mode: `ArtifactGuardError::WorktreeList(String)`, displayed as `artifact guard could not list worktrees: <payload>`, where the payload is one of three fixed strings. A failing Git command is a repository error. No partial report is returned. |

## Fixing violations

For a blocked commit:

```bash
amplihack hygiene artifact-guard --repo . --mode staged
git restore --staged dist/plugin.js
```

Then move the build output to an isolated location or remove the local artifact
if it is not needed.

For a `run-worktree-gitlink` refusal, never delete the path: drop the entry from
the index only, as shown in [Output and remediation](#output-and-remediation).

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

## Review expectations

Review these changes carefully:

1. New or changed `.amplihack-artifact-allowlist` entries.
2. Changes to prohibited rules.
3. Recipe edits around `git add -A`, publication, finalization, or PR creation.
4. Pre-commit changes that remove `pass_filenames: false`.
5. Build, plugin, or runtime changes that redirect outputs into the parent
   worktree.

Reviewers should verify that generated and runtime outputs are isolated, guard
failures are visible, and allowlist entries are not used as substitutes for
proper output placement.

## Related documentation

- [Recipe CLI Reference](reference/recipe-cli-reference.md)
- [Pre-Commit Diagnostics](claude/agents/amplihack/specialized/pre-commit-diagnostic.md)
- [Developing amplihack](DEVELOPING_AMPLIHACK.md)
