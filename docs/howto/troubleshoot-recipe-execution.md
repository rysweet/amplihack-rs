# How to Troubleshoot Recipe Execution Failures

Use this guide when a recipe step fails unexpectedly — a shell step hangs, an agent step produces no file changes, or a prerequisite tool is missing.

## Contents

- [Shell step hangs waiting for input](#shell-step-hangs-waiting-for-input)
- [Agent step completes but changes nothing](#agent-step-completes-but-changes-nothing)
- [Shell step fails with "python3 not found"](#shell-step-fails-with-python3-not-found)
- [Workflow classification routes to the wrong type](#workflow-classification-routes-to-the-wrong-type)
- [Agent step killed by step timeout](#agent-step-killed-by-step-timeout)
- [Update completes but assets are stale](#update-completes-but-assets-are-stale)
- [Install fails after switching to the Rust repository](#install-fails-after-switching-to-the-rust-repository)
- [Step-08c fails with WORKTREE_SETUP_WORKTREE_PATH not set](#step-08c-fails-with-worktree_setup_worktree_path-not-set)

---

## Shell step hangs waiting for input

**Symptom:** A `recipe run` invocation stalls indefinitely on a shell step. No output appears. Killing the process shows the step was waiting for TTY input from a tool like `npm`, `apt`, or a git credential helper.

**Cause:** The child shell process inherited an environment that signaled interactive mode. Tools like `apt` and `npm` prompt for confirmation unless told otherwise.

**Fix:** The recipe executor now injects five non-interactive environment variables into every shell step automatically:

| Variable | Value | Purpose |
|----------|-------|---------|
| `HOME` | inherited or `/root` | Prevents `~`-expansion failures |
| `PATH` | inherited or `/usr/local/bin:/usr/bin:/bin` | Ensures basic tool lookup |
| `NONINTERACTIVE` | `1` | Generic non-interactive signal |
| `DEBIAN_FRONTEND` | `noninteractive` | Suppresses dpkg/apt prompts |
| `CI` | `true` | Suppresses interactive prompts in npm, yarn, pip |

No recipe YAML changes are required. All shell steps receive these variables.

**Verify the fix is active:**

```sh
# Create a one-step recipe that prints CI env vars
cat > /tmp/env-check.yaml << 'EOF'
name: env-check
steps:
  - id: check
    type: shell
    command: "env | grep -E '^(CI|NONINTERACTIVE|DEBIAN_FRONTEND)='"
EOF

amplihack recipe run /tmp/env-check.yaml
# Expected output includes:
#   CI=true
#   NONINTERACTIVE=1
#   DEBIAN_FRONTEND=noninteractive
```

---

## Agent step completes but changes nothing

**Symptom:** An agent step runs, appears to succeed, but produces no file modifications. The agent's output may reference files by relative path without knowing where the working directory is.

**Cause:** The agent backend was not receiving the recipe's working directory or non-interactive flag in its context map. The agent would default to its own notion of "current directory," which may differ from the recipe's `working_dir`.

**Fix:** The executor now augments the context passed to every agent step with two entries:

| Context key | Value | Purpose |
|-------------|-------|---------|
| `working_directory` | The recipe's configured `working_dir` | Tells the agent where to read/write files |
| `NONINTERACTIVE` | `1` | Prevents the agent from attempting interactive prompts |

These entries are injected only when the recipe step's own context does not already define them, so explicit overrides in recipe YAML still work.

**Verify in a recipe:**

```yaml
name: agent-test
steps:
  - id: write-file
    type: agent
    agent: claude
    prompt: "Create a file called hello.txt containing 'Hello from agent step'"
```

After running, `hello.txt` should appear in the recipe's working directory, not in some other location.

---

## Shell step fails with "python3 not found"

**Symptom:** A recipe step that references `python3` or `python ` in its command fails immediately with:

```
Error: Shell step 'step-id' requires python3 but it is not installed or not on PATH.
Recipe steps should use deterministic Rust tools instead of Python sidecars.
```

**Cause:** The executor performs a pre-flight check for Python availability before executing any shell step whose command mentions `python3` or `python `. This prevents long-running recipes from wasting hours before failing at a late step that needs Python.

**Resolution options:**

1. **Install Python 3** on the machine and ensure it is on `PATH`:

   ```sh
   # Ubuntu/Debian
   sudo apt-get install -y python3

   # macOS
   brew install python@3

   # Verify
   python3 --version
   ```

2. **Rewrite the step** to avoid the Python dependency. The error message recommends using Rust-native tools. For example, replace a Python JSON-processing script with `jq` or a purpose-built Rust binary.

3. **Use a Docker image** that includes Python if the recipe must run Python:

   ```sh
   docker run --rm -v "$PWD:/work" -w /work python:3.12-slim \
     amplihack recipe run my-recipe.yaml
   ```

---

## Workflow classification routes to the wrong type

**Symptom:** A development task like "Add an agentic disk-cleanup loop. Extend `src/cmd_cleanup.rs`" gets classified as `Ops` instead of `Default`, causing the wrong workflow to execute.

**Cause:** Single-word OPS keywords (like `cleanup`, `manage`) matched as substrings in code paths (`cmd_cleanup.rs`) and task descriptions. This triggered false-positive OPS classification.

**Fix:** OPS keywords are now multi-word phrases that require specific operational context:

| Old keyword (removed) | New phrase (required) |
|------------------------|----------------------|
| `cleanup` | `disk cleanup`, `clean up temp` |
| `manage` | `manage repos`, `repo management` |
| `delete` | `delete files` |
| `organize` | `organize files` |

A task description must contain the full phrase to match OPS. Single words like `cleanup` appearing inside code references no longer trigger misclassification.

**Verify classification:**

```sh
# This should classify as Default (development), not Ops
amplihack classify "Add an agentic disk-cleanup loop. Extend src/cmd_cleanup.rs with a new function."
# Expected: workflow=Default

# This should classify as Ops
amplihack classify "disk cleanup on staging servers"
# Expected: workflow=Ops
```

---

## Agent step killed by step timeout

**Symptom:** An agent step (architecture design, large refactoring, complex analysis) appears to fail mid-thought with a timeout error.

**Cause:** A user-supplied `--step-timeout` override (or `AMPLIHACK_STEP_TIMEOUT` in the environment) is forcing a per-step ceiling on agent steps. The bundled recipes under `amplifier-bundle/recipes/` no longer set per-step timeouts on agent steps — agent reasoning is highly variable and aborting mid-thought corrupts orchestrator state ([issue #439](https://github.com/rysweet/amplihack-rs/issues/439)). If you are seeing a timeout on an agent step, something outside the recipe is imposing it.

**Fix:** Remove or relax the override.

```sh
# Drop the --step-timeout flag entirely (recommended for agent-heavy work)
amplihack recipe run recipe.yaml \
  -c task_description="Complex task"

# Or explicitly disable timeouts for the run
amplihack recipe run recipe.yaml \
  -c task_description="Very long task" \
  --step-timeout 0

# Or raise the ceiling to a generous floor (e.g., for a CI guard rail)
amplihack recipe run recipe.yaml \
  -c task_description="CI run" \
  --step-timeout 1800
```

If `AMPLIHACK_STEP_TIMEOUT` is set in your shell or CI environment, unset it (`unset AMPLIHACK_STEP_TIMEOUT`) or override it on the command line with `--step-timeout 0`.

**Note:** A handful of bash steps in bundled recipes still carry `timeout_seconds: 1800`. Those are the network-I/O steps (`gh api`, `git fetch`, `curl`) where a stuck socket could hang indefinitely. The 1800-second floor is an availability guardrail, not a work bound — if one of those fires, the underlying network call has hung and the right fix is to investigate the network failure, not to extend the timeout.

**Note:** `--step-timeout` overrides per-step `timeout_seconds` only. It does not affect recipe-level `default_step_timeout` (e.g., `quality-loop.yaml`) or the `max_runtime` budget in multitask workstreams.

---

## Update completes but assets are stale

**Symptom:** After running `amplihack update`, the binary version is correct but skills, hooks, or bundled assets show old behavior. Running `amplihack install` manually fixes it.

**Cause:** The update command replaced the binary but did not re-stage framework assets from the new binary's embedded `amplifier-bundle/`.

**Fix:** The update flow now calls `ensure_framework_installed()` immediately after the binary swap. If asset re-staging fails, the update still succeeds (the binary is already replaced) but prints a warning:

```
⚠️  Binary updated but framework asset refresh failed: <error>
   Run `amplihack install` to refresh assets manually.
```

**Verify the fix:**

```sh
# Update to latest
amplihack update

# Check that assets match the new version
amplihack --version
ls ~/.amplihack/.claude/
# Asset files should have recent timestamps matching the update time
```

---

## Install fails after switching to the Rust repository

**Symptom:** Installation via `amplihack install` fails because the downloaded archive or cloned repository has a different directory structure than expected. The installer cannot find a `.claude/` directory at the repository root.

**Cause:** The Python repository used `.claude/` as its framework root marker. The Rust repository uses `amplifier-bundle/`. The root-detection function needed to accept both layouts.

**Fix:** `find_framework_repo_root()` now accepts either `.claude/` or `amplifier-bundle/` as a valid repository root marker. This handles:

- Legacy Python repository archives (contain `.claude/`)
- Current Rust repository archives (contain `amplifier-bundle/`)
- Mixed layouts where both directories exist

**Verify:**

```sh
# Clone the Rust repo and confirm install works
git clone --depth 1 https://github.com/rysweet/amplihack-rs /tmp/test-install
amplihack install --local /tmp/test-install
# Should succeed regardless of whether .claude/ or amplifier-bundle/ is present
```

---

## Checksum verification fails intermittently

**Symptom:** `amplihack install` fails with a checksum verification error during binary download, but succeeds on retry.

**Cause:** The SHA-256 checksum file download used a single HTTP GET without retry logic. Transient network errors caused the checksum fetch to fail even when the main binary download succeeded.

**Fix:** Checksum verification now uses `http_get_with_retry()`, which retries with exponential backoff (up to 3 attempts). The retry logic applies only to the checksum fetch, not the binary download itself (which already had retries).

**If it still fails after retries:**

```sh
# Check network connectivity to GitHub releases
curl -I https://github.com/rysweet/amplihack-rs/releases/latest

# Manual install with local checkout bypasses all downloads
git clone https://github.com/rysweet/amplihack-rs /tmp/amplihack-local
amplihack install --local /tmp/amplihack-local
```

---

## Step-08c fails with WORKTREE_SETUP_WORKTREE_PATH not set

**Symptom:** During an orchestrator-driven workflow (fixing an issue, implementing a feature), the recipe fails at step-08c with:

```
WORKTREE_SETUP_WORKTREE_PATH: step-08c requires worktree_setup.worktree_path
from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and
propagated outputs
```

This blocks the entire issue-fixing loop. Any task routed through `smart-orchestrator` → `default-workflow` → `workflow-tdd` fails at the no-op guard.

**Cause:** A sub-recipe in the default-workflow chain has `context: {}` instead of declaring `worktree_setup` in its context block. The recipe runner only forwards context variables that the child recipe explicitly declares. Without the declaration, `worktree_setup` is silently dropped at the recipe boundary, and step-08c cannot read `WORKTREE_SETUP_WORKTREE_PATH`.

**Fix:** Every post-worktree sub-recipe (`workflow-tdd`, `workflow-refactor-review`, `workflow-precommit-test`, `workflow-publish`, `workflow-pr-review`, `workflow-finalize`) declares `worktree_setup: ""` and `allow_no_op: false` in its `context:` block. This ensures the recipe runner threads the value from `default-workflow` through to the step that needs it.

**Verify the fix:**

```sh
# Run the propagation regression tests
python -m pytest amplifier-bundle/tools/test_default_workflow_fixes.py::TestWorktreeSetupPropagation479 -v

# All four tests should pass:
#   test_post_worktree_sub_recipes_declare_worktree_setup
#   test_post_worktree_sub_recipes_declare_allow_no_op
#   test_default_workflow_declares_worktree_setup
#   test_smart_execute_routing_forwards_worktree_setup
```

**If you are writing a new sub-recipe** that runs after `workflow-worktree` (step 04), add these keys to your recipe's `context:` block:

```yaml
context:
  worktree_setup: ""
  allow_no_op: false
```

See [worktree_setup Context Propagation](../reference/worktree-setup-propagation.md) for the complete propagation chain and design rationale.

---

## Related

- [Run a Recipe End-to-End](./run-a-recipe.md) — Normal recipe execution workflow
- [Run amplihack in Non-interactive Mode](./run-in-noninteractive-mode.md) — CI and headless environments
- [Install amplihack for the First Time](./first-install.md) — Bootstrap from scratch
- [Environment Variables](../reference/environment-variables.md) — All variables read or injected by amplihack
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — Architecture of the step execution pipeline
- [worktree_setup Context Propagation](../reference/worktree-setup-propagation.md) — Full reference for the worktree_setup propagation chain
