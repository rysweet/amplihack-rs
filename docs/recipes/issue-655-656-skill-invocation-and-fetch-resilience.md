# Rust CLI Invocation & Resilient Git Fetch (Issues #655, #656)

Two reliability improvements to the default-workflow recipe infrastructure:
retroactive documentation of the Rust CLI invocation fix (already on `main`),
and a new resilient `git fetch` in workspace preparation (the code change in
this PR).

---

## 1. Rust CLI Invocation in SKILL.md (Issue #655) — Retroactive

> **Note**: This section documents a fix that already landed on `main` in a
> prior commit. No SKILL.md changes are made in this PR. The documentation
> is provided here for completeness and to establish the invocation reference
> table for future agents.

### Problem (resolved)

The `default-workflow` skill file (`docs/claude/skills/default-workflow/SKILL.md`)
previously contained Python-based recipe runner invocation instructions:

```python
# STALE — removed in prior commit
from amplihack.recipes import run_recipe_by_name
result = run_recipe_by_name('default-workflow', ...)
```

And a shell variant using `python3 -c` to import the same function.

The recipe runner has been a Rust binary (`recipe-runner-rs`) since v0.9.x.
The Python package never existed in this repository. Agents following the
stale instructions would waste multiple turns searching for a Python module
before discovering the Rust CLI.

### Current State

The Execution Instructions section documents the correct CLI:

```bash
amplihack recipe run default-workflow \
  -c task_description="TASK_DESCRIPTION_HERE" \
  -c repo_path="."
```

With verbose output:

```bash
cd /path/to/repo && amplihack recipe run default-workflow \
  -c task_description="TASK_DESCRIPTION_HERE" \
  -c repo_path="." \
  --verbose
```

### Invocation Reference

| Method | Command | When to Use |
|---|---|---|
| Via dev-orchestrator (preferred) | `Skill(skill="dev-orchestrator")` or `/dev <task>` | Most tasks — adds goal-seeking, decomposition, error recovery |
| Direct (standalone) | `amplihack recipe run default-workflow -c ...` | When dev-orchestrator is unavailable or explicit standalone needed |
| **Removed** | ~~`run_recipe_by_name('default-workflow', ...)`~~ | Never — Python API does not exist |
| **Removed** | ~~`python3 -c "from amplihack.recipes import ..."`~~ | Never — Python package does not exist |

### Files Changed

| File | Change |
|---|---|
| `docs/claude/skills/default-workflow/SKILL.md` | Execution Instructions section: Python invocation → Rust CLI syntax (resolved prior to this PR; documented here retroactively) |

**Note**: The stale Python instructions were removed in an earlier commit on
`main`. This section provides retroactive documentation of the fix and the
correct invocation reference for future agents.

### Configuration

No configuration required. The `amplihack` CLI resolves the recipe runner
binary automatically via `$AMPLIHACK_HOME` and `$PATH`.

---

## 2. Resilient Git Fetch in Workspace Preparation (Issue #656) — Code Change

### Problem

`step-01-prepare-workspace` in `amplifier-bundle/recipes/workflow-prep.yaml`
runs `git fetch --all --no-tags` inside a `&&`-chain. If the fetch fails
(exit 128), the entire step aborts — preventing the workflow from proceeding
even though the local branch state is sufficient for all subsequent steps.

The most common failure scenario is Azure DevOps (ADO) remotes where no git
credential helper is configured:

```
fatal: could not read Username for 'https://dev.azure.com/...': No such device or address
```

This happens when:
- The repository has an `origin` pointing to `dev.azure.com` or `visualstudio.com`
- `az login` is active (Azure CLI works) but no git credential helper is wired
- Only the GitHub credential helper (`gh auth git-credential`) is configured
- SSH keys are configured for GitHub but not ADO

### Solution

The `git fetch --all --no-tags` command is extracted from the `&&`-chain into
a standalone block that captures the exit code. On failure:

1. A `WARNING` is emitted (never silent)
2. The remote URL is checked for ADO patterns (`dev.azure.com`, `visualstudio.com`)
3. ADO-specific remediation steps are printed
4. The step **continues with local state** instead of aborting

### Behavior Matrix

| Fetch Result | Remote Type | Behavior |
|---|---|---|
| Success (exit 0) | Any | Normal flow — "Fetched latest from all remotes" |
| Failure (exit 128) | ADO (`dev.azure.com` / `visualstudio.com`) | WARNING + ADO remediation steps + continue |
| Failure (exit 128) | Non-ADO (GitHub, GitLab, etc.) | WARNING + generic diagnostic + continue |
| Failure (other exit) | Any | WARNING + continue |

### ADO Remediation Output

When fetch fails against an ADO remote, the step prints:

```
WARNING: git fetch failed (exit 128) — continuing with local state.
TIP: Remote appears to be Azure DevOps. To fix git authentication:
  1. Run: az login
  2. Run: az devops configure --defaults organization=https://dev.azure.com/YOUR_ORG
  3. Install GCM: https://github.com/git-ecosystem/git-credential-manager
  4. Or set a PAT: git config credential.helper '!f() { echo "password=YOUR_PAT"; }; f'
```

For non-ADO remotes:

```
WARNING: git fetch failed (exit 128) — continuing with local state.
Check remote connectivity and credential configuration.
```

### Security Considerations

- The remote URL is **never echoed directly** to avoid leaking embedded PATs
  (e.g., `https://user:TOKEN@dev.azure.com/...`). Only the URL pattern match
  result (`ADO` vs `non-ADO`) is used in output.
- Exit codes are numeric-only, safe to print.
- No credential caching or automatic fix attempts — detect and warn only.
- Remediation messages reference only public tooling (`az login`,
  `git-credential-manager`).

### Files Changed

| File | Change |
|---|---|
| `amplifier-bundle/recipes/workflow-prep.yaml` | `step-01-prepare-workspace`: extracted `git fetch` from `&&`-chain into resilient block with exit-code capture and ADO detection |

### Configuration

No new configuration variables. The existing `SKIP_PRE_AGENT_VALIDATION`
variable is unrelated and continues to control the optional pre-agent
validation gate that follows the fetch step.

### Testing

The fix preserves all test-required string literals in the step:

| Literal | Purpose | Preserved |
|---|---|---|
| `git fetch` | Verify fetch is attempted | ✅ |
| `git status` | Verify status check runs | ✅ |
| `git branch --show-current` | Verify branch detection | ✅ |
| `requires a git repo` | Verify non-repo error message | ✅ |
| `git init` | Verify remediation hint | ✅ |
| `rerun from a checkout` | Verify remediation hint | ✅ |

---

## Relationship to Other Resilience Fixes

These fixes follow the same pattern as prior workflow resilience work:

| Issue | Step | Pattern | Doc |
|---|---|---|---|
| #647 | Steps 19c, 20b, 21 | Resilient `cd` fallback chain | `docs/recipes/issue-647-resilient-worktree-cleanup.md` |
| #624 | Step 8c | Verdict synonym normalization | `docs/recipes/P1_WORKFLOW_RELIABILITY_FIXES.md` |
| **#655** | **Skill file** | **Stale invocation syntax → Rust CLI** (retroactive) | **This document** |
| **#656** | **Step 1** | **Hard-fail fetch → warn-and-continue** (this PR) | **This document** |

The shared principle: steps that perform auxiliary operations (fetching,
label creation, directory navigation) must not abort the workflow when
the primary operation (coding, testing, committing) can proceed without them.
