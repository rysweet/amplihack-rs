# Hygiene Cleanup

`amplihack hygiene cleanup` reclaims local disk safely. It is conservative,
opt-in, and dry-run by default.

## Safety boundary

`amplihack hygiene cleanup` must never delete:

- the current repository target directory,
- the current worktree or any active `git worktree list` entry,
- dirty worktrees,
- worktrees with unpushed commits,
- running or locked sessions,
- recent session artifacts,
- paths that cannot be canonicalized or classified,
- anything during a dry run.

Only paths classified as `candidate` in a reviewed plan may be deleted, and only
when `--apply` is explicitly set.

## Quick start

Preview stale worktree cleanup:

```bash
amplihack hygiene cleanup --worktrees --older-than 14d
```

Preview all supported cleanup categories for a repository:

```bash
amplihack hygiene cleanup --all --older-than 30d --repo .
```

Apply a reviewed cleanup plan:

```bash
amplihack hygiene cleanup --worktrees --cargo-targets --sessions \
  --older-than 30d \
  --apply
```

Without `--apply`, the command prints the candidate actions and exits without
deleting anything.

## Cleanup categories

| Category flag | Candidates | Always skipped |
| --- | --- | --- |
| `--worktrees` | Stale Git worktrees under recognized worktree roots. | Current worktree, active `git worktree list` entries, dirty worktrees, worktrees with unpushed commits, non-canonical paths. |
| `--cargo-targets` | Detached `target/` directories not belonging to the current repository. | The current repository `target/`, targets inside active worktrees, targets younger than `--older-than`. |
| `--sessions` | Old session state under amplihack/Copilot/agent session roots. Alias: `--session-artifacts`. | Running sessions, locked sessions, current session, recent artifacts, artifacts with live process owners. |

The scanner must canonicalize every path before comparison. If a path cannot be
canonicalized or classified, it is reported as `skipped_ambiguous` and is not
deleted.

## CLI reference

```text
amplihack hygiene cleanup [OPTIONS]
```

| Option | Default | Description |
| --- | --- | --- |
| `--worktrees` | off | Include stale worktree candidates. |
| `--cargo-targets` | off | Include detached Cargo `target/` candidates. |
| `--sessions` | off | Include stale session artifact candidates. Alias: `--session-artifacts`. |
| `--all` | off | Include all cleanup categories. Equivalent to the three category flags. |
| `--older-than <DURATION>` | required for deletion | Minimum candidate age. Supports `h`, `d`, and `w` suffixes, such as `48h`, `14d`, or `8w`. |
| `--repo <PATH>` | current directory | Repository whose active paths must be protected. |
| `--apply` | off | Delete approved candidates. Without this flag the command is a dry run. |
| `--format text|json` | `text` | Output format. JSON is intended for automation and CI evidence. |
| `--include-skipped` | off | Include skipped candidates in text output. JSON always includes skipped counts. |

At least one cleanup category is required. `--apply` also requires
`--older-than`; destructive cleanup without an age threshold is rejected.

## Safety classification

Each discovered path must receive exactly one classification:

| Classification | Meaning |
| --- | --- |
| `candidate` | Eligible for deletion when `--apply` is set. |
| `skipped_current_repo` | Inside the repository passed with `--repo` or the current working directory. |
| `skipped_active_worktree` | Listed by `git worktree list` or contains active worktree metadata. |
| `skipped_dirty` | Contains uncommitted changes or untracked files that are not ignored build output. |
| `skipped_unpushed` | Contains commits not reachable from its configured upstream. |
| `skipped_running_session` | Session lock or live process marker indicates active use. |
| `skipped_recent` | Younger than `--older-than`. |
| `skipped_ambiguous` | Cannot be safely classified. |
| `deleted` | Deleted during an `--apply` run. |

Only `candidate` paths can become `deleted`.

## Validation contract

The implementation must include coverage for these safety rules:

| Rule | Required validation |
| --- | --- |
| Current repository is protected | A target inside `--repo` is classified as `skipped_current_repo` and is not deleted under `--apply`. |
| Current worktree is protected | The process working tree and any active Git worktree are classified as skipped. |
| Dirty worktrees are protected | Uncommitted or untracked non-build-output changes prevent deletion. |
| Unpushed work is protected | Commits not reachable from upstream prevent deletion. |
| Recent artifacts are protected | Candidates younger than `--older-than` are `skipped_recent`. |
| Dry run is non-destructive | The same candidate set is reported without deleting files when `--apply` is absent. |
| Apply deletes only candidates | `--apply` deletes only paths already classified as `candidate`; skipped paths remain. |
| Ambiguous paths fail closed | Non-canonical or unclassified paths become `skipped_ambiguous`. |

## Output examples

Dry run:

```text
$ amplihack hygiene cleanup --worktrees --older-than 14d
mode: dry-run
repo: /home/azureuser/src/amplihack-rs

category   action     age   path
worktree   candidate  31d   /home/azureuser/src/amplihack-rs/worktrees/old/issue-512
worktree   skipped    2d    /home/azureuser/src/amplihack-rs/worktrees/feat/current

summary: 1 candidate, 1 skipped, 0 deleted, 0 errors
```

Apply:

```text
$ amplihack hygiene cleanup --worktrees --older-than 14d --apply
mode: apply
deleted: /home/azureuser/src/amplihack-rs/worktrees/old/issue-512
summary: 0 candidates, 1 skipped, 1 deleted, 0 errors
```

JSON:

```bash
amplihack hygiene cleanup --all --older-than 30d --format json
```

```json
{
  "mode": "dry-run",
  "repo": "/home/azureuser/src/amplihack-rs",
  "summary": {
    "candidates": 3,
    "skipped": 8,
    "deleted": 0,
    "errors": 0
  },
  "items": [
    {
      "category": "cargo-targets",
      "classification": "candidate",
      "path": "/home/azureuser/src/old-crate/target",
      "age_seconds": 3456000
    }
  ]
}
```

## Automation pattern

Use dry-run JSON in scheduled automation. Require a human or policy gate before
adding `--apply`.

```bash
amplihack hygiene cleanup --all --older-than 30d --format json > cleanup-plan.json
jq '.summary' cleanup-plan.json
```

For CI or fleet jobs, keep deletion opt-in:

```bash
if jq -e '.summary.errors == 0 and .summary.candidates <= 20' cleanup-plan.json; then
  amplihack hygiene cleanup --all --older-than 30d --apply
fi
```

## Configuration

No global configuration is required. The command uses explicit CLI flags for
destructive behavior so cleanup cannot be enabled accidentally by inherited
environment.

`NODE_OPTIONS=--max-old-space-size=32768` may be set in the caller environment
for workflows that also launch Node-based agents. The cleanup command does not
require Node and does not inspect or mutate `NODE_OPTIONS`.

## Failure behavior

Cleanup must fail closed:

- Missing category flags produce a usage error.
- `--apply` without `--older-than` is rejected.
- Permission errors are reported per path and do not cause nearby paths to be
  guessed or force-deleted.
- Ambiguous paths are skipped, not deleted.
- The command exits non-zero when any requested deletion fails.
