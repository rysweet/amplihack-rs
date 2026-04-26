# Recipe Resilience: Branch Sanitization & Sub-Recipe Recovery

**Type**: Explanation (Understanding-Oriented)

Two resilience improvements to the amplihack recipe runner: branch name
sanitization and sub-recipe agentic recovery.

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

## Configuration

Neither feature introduces new configuration knobs. Branch sanitization and
recovery are always active.

## Related

- [Auto Mode](../concepts/auto-mode.md) — autonomous agentic loop documentation
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — how recipes execute
- [Run a Recipe](../howto/run-a-recipe.md) — step-by-step recipe usage
