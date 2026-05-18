# Recipe Resilience: Branch Sanitization, Worktree Bases & Publish Safety

**Type**: Explanation (Understanding-Oriented)

Resilience improvements to amplihack recipe execution: branch name
sanitization, default-workflow worktree base resolution, publish/PR safety, and
sub-recipe agentic recovery.

## Branch Name Sanitization

### Problem

`step-04-setup-worktree` creates a git worktree using a branch name derived
from `task_description`. Raw multi-line task descriptions (common when pasting
from issue bodies) produce invalid branch names:

```
fatal: 'fix login bug\nwith oauth' is not a valid branch name
```

### Solution: Sanitization Pipeline

The task description passes through a sanitization pipeline:

```
newlines -> spaces
-> strip whitespace
-> lowercase
-> replace invalid chars with hyphens
-> collapse consecutive hyphens
-> truncate to 60 characters
-> strip trailing hyphens/dots
-> validate with git check-ref-format --branch
-> fallback if invalid
```

### Examples

| Input                                        | Branch Slug                                 |
| -------------------------------------------- | ------------------------------------------- |
| `Fix login bug`                              | `fix-login-bug`                             |
| `Fix authentication bug\nThis affects oauth` | `fix-authentication-bug-this-affects-oauth`  |
| `Add User Authentication`                    | `add-user-authentication`                   |
| `fix: auth/login (oauth2)`                   | `fix-auth-login-oauth2`                     |
| 120 'a' characters                           | truncated to 60 chars                       |
| `!@#$%^&*()`                                 | fallback: `{prefix}/issue-{n}-task`         |

### Fallback

If the sanitized slug is empty or fails `git check-ref-format`, the branch
name falls back to `{prefix}/issue-{issue_number}-task`.

### Security

The `task_description` is always passed via a named environment variable
(`$TASK_DESC`), never interpolated into the shell command string, preventing
shell injection.

## Default Workflow Reliability

### Worktree Base Resolution

`workflow-worktree` no longer assumes `origin/main`. It resolves the remote base
ref in this order:

1. `origin/HEAD`
2. `origin/master`
3. `origin/develop`

`origin/HEAD` is resolved through Git and accepted only when its target verifies
as a remote-tracking ref under `refs/remotes/origin/`. That means repositories
whose default branch is `main`, `master`, `develop`, or another configured
remote default work without manual intervention. If `origin/HEAD` is unavailable,
the workflow checks `origin/master` and then `origin/develop`.

The first valid ref is used for new branch creation, existing branch base
checks, and diagnostic output. If none of the supported base sources exists, the
workflow fails closed with a clear error instead of guessing a local branch or
using local `HEAD` bootstrap behavior.

### Publish and PR Commands

`workflow-publish` does not wrap its GitHub CLI publish or pull-request paths in
shell `timeout` or `gtimeout` commands. Timeout control belongs to the recipe
runner through recipe metadata or `amplihack recipe run --step-timeout`.

`gh` command failures remain explicit: command output is captured, the exit code
is checked, and non-zero results fail the recipe step with the captured output.

### Optional Design Spec

PR creation treats `design_spec` / `DESIGN_SPEC` as optional context. When both
are present, `design_spec` takes precedence over `DESIGN_SPEC`. Missing or empty
design-spec input is normalized to empty content before shell expansion, so
`set -u` cannot produce an unbound-variable failure. When no design spec is
available, the PR body is still created and the design-spec section is omitted.

### Example

```bash
# Works even when the repository default branch is master or develop.
amplihack recipe run default-workflow \
  -c task_description="Fix workflow reliability" \
  -c repo_path=.
```

```bash
# Works without design_spec/DESIGN_SPEC.
amplihack recipe run workflow-publish \
  -c issue_number=573 \
  -c task_description="Fix workflow reliability" \
  -c repo_path=.
```

### Validation

Regression tests cover:

- non-`main` remote base branch selection for `origin/master`
- non-`main` remote base branch selection for `origin/develop`
- absence of shell timeout wrappers around `gh pr create` and publish-path `gh` commands
- PR creation under `set -u` with missing `design_spec` and `DESIGN_SPEC`

## Sub-Recipe Agentic Recovery

### Problem

When a sub-recipe step failed, `RecipeRunner._execute_sub_recipe()` raised
`StepExecutionError` immediately. The entire workflow halted with no recovery
opportunity, even for transient failures.

### Solution: Recovery Agent Dispatch

On sub-recipe failure, the runner invokes `_attempt_agent_recovery()` before
raising. The recovery agent receives full failure context and can either
complete the work or signal the failure as unrecoverable.

### Recovery Flow

```
sub-recipe fails
    |
    v
_attempt_agent_recovery()
    |
    +-- no adapter -----------> return None (no recovery)
    +-- adapter raises -------> return None (log warning)
    +-- empty response -------> return None
    +-- "UNRECOVERABLE" ------> return None (log warning)
    +-- non-empty response ---> return recovery_output (success)
```

### Recovery Prompt

The recovery agent receives:

- Sub-recipe name
- Names of failed steps
- Original error message
- First 500 characters of partial outputs
- Redacted context summary (up to 20 keys, 80 chars each)

### Signaling Unrecoverable Failures

The recovery agent includes `UNRECOVERABLE` in its response:

```
UNRECOVERABLE: the test environment requires Docker which is not installed
```

Any other non-empty response is treated as successful recovery.

### Security

- `partial_outputs` truncated to 500 characters before prompt construction
- Context keys matching `token`, `secret`, `password`, `key` are redacted
- Uses existing adapter credentials (no new auth surface)

## Late-Stage Worktree Cleanup Resilience

### Problem

Steps 19c (zero-BS verification), 20b (push-cleanup), and 21 (pr-ready) in the
default workflow `cd` into `WORKTREE_SETUP_WORKTREE_PATH` using `${VAR:?}`
guards. When the worktree directory has already been removed — by the agent, a
prior cleanup step, or an external process — the `cd` fails with exit code 1,
aborting the recipe after work is complete and pushed.

### Solution: Resilient Fallback Chain

Late-stage steps use a three-tier directory resolution:

1. `WORKTREE_SETUP_WORKTREE_PATH` — preferred (worktree still exists)
2. `REPO_PATH` — fallback (repo root, available via context propagation)
3. `$(pwd)` — final (current directory)

Each fallback emits a `WARNING` to stderr. Early-stage steps (15, 16, 18c) that
genuinely require worktree files keep their hard-fail `${VAR:?}` guards.

| Step | Phase | Behavior |
|------|-------|----------|
| 15, 16, 18c | review | Hard-fail (`:?` guard) |
| 19c | verification | Resilient (fallback chain) |
| 20b | finalize | Resilient (fallback chain) |
| 21 | finalize | Resilient (fallback chain) |

No `|| true`, `set +e`, or `>/dev/null` suppression is used. The fallback is
explicit `if [ -d ]` conditional logic with `set -euo pipefail` preserved.

See [Issue #647 — Resilient Worktree Cleanup](../recipes/issue-647-resilient-worktree-cleanup.md)
for the full specification and test coverage.

## Configuration

These features introduce no new configuration knobs. Branch sanitization,
worktree base resolution, publish timeout-wrapper removal, optional design-spec
handling, sub-recipe recovery, and late-stage worktree cleanup resilience are
always active.

## Related

- [Auto Mode](../concepts/auto-mode.md) — autonomous agentic loop documentation
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — how recipes execute
- [Run a Recipe](../howto/run-a-recipe.md) — step-by-step recipe usage
- [Issue #647 — Resilient Worktree Cleanup](../recipes/issue-647-resilient-worktree-cleanup.md) — full detail on late-stage fallback
