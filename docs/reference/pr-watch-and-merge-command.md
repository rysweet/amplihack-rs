# amplihack pr watch-and-merge — Command Reference

## Synopsis

```
amplihack pr watch-and-merge <PR_NUMBER> [OPTIONS]
```

## Description

Polls the CI checks on a GitHub pull request until they reach a terminal state, then merges the PR if all checks pass. The command uses `gh pr checks` with structured JSON output to determine check status and `gh pr merge` to perform the merge.

This is useful after pushing a branch when you want to merge as soon as CI is green, without manually watching the GitHub UI. The command handles transient network errors with automatic retry, reports failing check names and URLs on failure, and exits cleanly in all cases.

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `PR_NUMBER` | Yes | The pull request number to watch (e.g. `352`) |

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `--squash` | Yes (default) | Squash-merge the PR |
| `--rebase` | No | Rebase-merge the PR |
| `--merge` | No | Create a merge commit |
| `--admin` | No | Use administrator privileges to merge, bypassing branch protection |
| `--delete-branch` | No | Delete the remote branch after merging |
| `--interval <SECONDS>` | `30` | Seconds between poll cycles. Minimum value: `5` |

Only one of `--squash`, `--rebase`, or `--merge` may be specified. If none is given, `--squash` is used.

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All checks passed and the PR was merged |
| `1` | One or more checks failed — PR was NOT merged |
| `2` | `gh` CLI is not installed or not authenticated |
| `3` | The `gh pr merge` command failed (e.g. merge conflict, branch protection) |

## Preflight Check

Before polling, the command runs `gh auth status` to verify that the `gh` CLI is installed and authenticated. If this fails, the command prints a diagnostic message and exits with code `2`:

```
Error: gh CLI is not authenticated. Run `gh auth login` first.
```

## Check State Mapping

The command parses `gh pr checks <num> --json name,state,detailsUrl` and classifies each check:

| gh state | Classification |
|----------|---------------|
| `SUCCESS` | Passed |
| `NEUTRAL` | Passed |
| `SKIPPED` | Passed |
| `FAILURE` | Failed |
| `ERROR` | Failed |
| `PENDING` | Still running |
| `QUEUED` | Still running |
| `IN_PROGRESS` | Still running |

The poll loop continues while any check is in a "still running" state. When all checks reach a terminal state, the command either merges (all passed) or exits with failure details.

## Output

### Progress (stderr)

Each poll cycle prints a status line to stderr:

```
⏳ Waiting for checks on PR #352... (attempt 3, next check in 30s)
```

### Success (stdout)

```
✅ All checks passed. Merging PR #352 with squash strategy...
✓ PR #352 merged.
```

### Empty Checks (stderr warning, then merge)

```
⚠️ No checks found for PR #352. Proceeding to merge.
✓ PR #352 merged.
```

### Check Failure (stderr)

```
✗ CI checks failed on PR #352:
  - build (linux) — https://github.com/owner/repo/actions/runs/123456
  - test (windows) — https://github.com/owner/repo/actions/runs/123457
```

## Transient Error Retry

If `gh pr checks` or `gh pr merge` returns a non-zero exit code that is NOT a check failure (e.g. network timeout, rate limit), the command retries up to 3 times with exponential backoff:

| Attempt | Delay |
|---------|-------|
| 1 | 5 seconds |
| 2 | 15 seconds |
| 3 | 45 seconds |

After 3 failed retries, the command exits with the last error. Check failures (deterministic) are never retried — only transient `gh` command errors.

## Examples

### Squash-merge after checks pass (default)

```sh
amplihack pr watch-and-merge 352
```

### Rebase-merge with admin privileges

```sh
amplihack pr watch-and-merge 352 --rebase --admin
```

### Fast polling with branch cleanup

```sh
amplihack pr watch-and-merge 352 --interval 10 --delete-branch
```

### Use in a script

```sh
# Push and watch — PR must already exist
git push -u origin feat/my-feature && amplihack pr watch-and-merge "$(gh pr view --json number -q .number)" --delete-branch

# Push, create PR, and watch in one line (new branches)
git push -u origin feat/my-feature && amplihack pr watch-and-merge "$(gh pr create --fill --json number -q .number)" --delete-branch
```

## Prerequisites

- `gh` CLI installed and authenticated (`gh auth status` must succeed)
- The PR must exist and be open
- The authenticated user must have merge permissions (or use `--admin`)
