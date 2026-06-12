# Dev Orchestrator — Execution Reference

Extended reference for recipe runner execution modes, environment variables,
error recovery, and enforcement details. The primary execution instructions
are in [SKILL.md](./SKILL.md).

#### Durable Execution (tmux) — optional

Use tmux **only** when:

- The agent runtime may kill background processes after a timeout (e.g., some
  Claude Code hosted environments)
- You need to survive SSH disconnects or terminal closures
- You want to detach and monitor a long-running recipe interactively

```bash
LOG_FILE=$(mktemp /tmp/recipe-runner-output.XXXXXX.log)
chmod 600 "$LOG_FILE"
tmux new-session -d -s recipe-runner \
  "cd /path/to/repo && env -u CLAUDECODE \
   AMPLIHACK_HOME=/path/to/amplihack \
   amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
     -c task_description='TASK_DESCRIPTION_HERE' \
     -c repo_path='.' \
     --verbose 2>&1 | tee $LOG_FILE"
echo "Recipe runner log: $LOG_FILE"
```

- `chmod 600 "$LOG_FILE"` — keeps the log file private
- `tmux new-session -d` — detached session, no timeout, survives disconnects
- Monitor with: `tail -f "$LOG_FILE"` or `tmux attach -t recipe-runner`

**Restarting a stale tmux session**: Some runtimes (e.g. Copilot CLI) block
`tmux kill-session` because it does not target a numeric PID. Use one of these
shell-policy-safe alternatives instead:

```bash
# Option A (preferred): use a unique session name per run to avoid collisions
tmux new-session -d -s "recipe-$(date +%s)" "..."

# Option B: locate the tmux server PID and terminate with numeric kill
tmux list-sessions -F '#{pid}' 2>/dev/null | xargs -I{} kill {}

# Option C: let tmux itself handle it — send exit to all panes
tmux send-keys -t recipe-runner "exit" Enter 2>/dev/null; sleep 1
```

If using Option A, update the `tail -f` / `tmux attach` commands to use the
same session name.

**The recipe runner is the required execution path for Development and
Investigation tasks.** Always try `smart-orchestrator` first.

### Development routing invariant

Development classification always routes to `default-workflow`; model-provided recipe fields do not override that invariant.

Hybrid decompositions route each workstream by its own normalized classification.

The invariant is enforced in the native orchestration helper when workstream configuration is materialized. This makes routing deterministic even when an agent omits `recipe`, emits an empty recipe, or incorrectly suggests another recipe for a Development workstream.

**Required environment variables** for the recipe runner:

- `AMPLIHACK_HOME` — Auto-detected from the current working directory by
  walking parent directories for an `amplifier-bundle/` folder, with fallback
  to `~/.amplihack`. If auto-detection fails, set manually to the directory
  containing `amplifier-bundle/`.
- Preserve `AMPLIHACK_AGENT_BINARY` — nested workflow agents read this env var
  to stay on the caller's active binary (for example, Copilot in Copilot CLI).
- Unset `CLAUDECODE` — required so nested Claude Code sessions can launch.

**Fallback: Direct recipe invocation when smart-orchestrator fails.**

Always try `smart-orchestrator` first — it handles classification, decomposition,
and routing automatically. However, if `smart-orchestrator` fails at the
**infrastructure level** (e.g., 0 workstreams from decomposition, missing env
vars, Rust binary version mismatch), you MAY invoke the specific workflow
recipe directly based on your classification:

| Classification | Direct Recipe            | When to Use                             |
| -------------- | ------------------------ | --------------------------------------- |
| Investigation  | `investigation-workflow` | smart-orchestrator decomposition failed |
| Development    | `default-workflow`       | smart-orchestrator decomposition failed |
| Q&A (complex)  | `qa-workflow`            | Q&A needing multi-step research         |
| Consensus      | `consensus-workflow`     | Critical decisions needing validation   |

Example:

```bash
amplihack recipe run amplifier-bundle/recipes/investigation-workflow.yaml \
  -c task_description="TASK_DESCRIPTION_HERE" \
  -c repo_path="."
```

This is NOT a license to bypass `smart-orchestrator`. Only use direct
invocation after `smart-orchestrator` has failed at an infrastructure level
(not because the task seems "too simple" or "too specific").

**Handling hollow success** (recipe completes but agents produce no findings):

If a recipe returns SUCCESS but the agent outputs indicate the agents could
not access the codebase or produced empty/generic results (e.g., "no codebase
exists", "cannot proceed without a target"), this is a **hollow success**.
In this case:

1. Check that `repo_path` and `AMPLIHACK_HOME` are correct
2. Verify the working directory is the repo root
3. Retry with explicit file paths in the `task_description`
4. If retries also produce hollow results, report the infrastructure
   failure to the user with specifics

**Common rationalizations that are NOT acceptable:**

- "Let me first understand the codebase" — the recipe does that in Step 0
- "I'll follow the workflow steps manually" — NO, the recipe enforces them
- "The recipe runner might not work" — try it first, report errors if it fails
- "This is a simple task" — simple or complex, the recipe runner handles both
- "The recipe succeeded but didn't do anything useful, so I'll do it myself"
  — this is hollow success; retry with better context first

**Q&A and Operations only** may bypass the recipe runner:

- Q&A: Respond directly (analyzer agent)
- Operations: Builder agent (direct execution, no workflow steps)

### Error Recovery: Adaptive Strategy (NOT Degradation)

When `smart-orchestrator` fails, **failures must be visible and surfaced** —
never swallowed or silently degraded. The recipe handles error recovery
automatically via its built-in adaptive strategy steps, but if you observe
a failure outside the recipe, follow this protocol:

**1. Surface the error with full context:**

Report the exact error, the step that failed, and the log output. Never say
"something went wrong" — always include the specific failure details.

**2. File a bug with reproduction details:**

For infrastructure failures (import errors, missing env vars, binary not found,
decomposition producing invalid output), file a GitHub issue:

```bash
gh issue create \
  --title "smart-orchestrator infrastructure failure: <one-line summary>" \
  --body "<full error context, reproduction command, env details>" \
  --label "bug"
```

**3. Evaluate alternative strategies:**

If `smart-orchestrator` fails at the infrastructure level (not because the task
is wrong), you MAY invoke the specific workflow recipe directly. This is an
**adaptive strategy** — it must be announced explicitly, not done silently:

| Classification | Direct Recipe            | When Permitted                                      |
| -------------- | ------------------------ | --------------------------------------------------- |
| Investigation  | `investigation-workflow` | smart-orchestrator failed at parse/decompose/launch |
| Development    | `default-workflow`       | smart-orchestrator failed at parse/decompose/launch |

Example:

```bash
# ANNOUNCE the strategy change first — never do this silently
echo "[ADAPTIVE] smart-orchestrator failed at parse-decomposition: <error>"
echo "[ADAPTIVE] Switching to direct investigation-workflow invocation"
amplihack recipe run amplifier-bundle/recipes/investigation-workflow.yaml \
  -c task_description="TASK_DESCRIPTION_HERE" \
  -c repo_path="."
```

**This is NOT a license to bypass smart-orchestrator.** Always try it first.
Direct invocation is only permitted when smart-orchestrator fails at the
infrastructure level. "The task seems simple" is NOT an infrastructure failure.

**4. Detect hollow success:**

A recipe can complete structurally (all steps exit 0) but produce empty or
meaningless results — agents reporting "no codebase found" or reflection
marking ACHIEVED when no work was done. After execution, check that:

- Round results contain actual findings or code changes (not "I could not access...")
- PR URLs or concrete outputs are present for Development tasks
- At least one success criterion was verifiably evaluated

If results are hollow, report this to the user with the specific empty outputs.
Do not declare success when agents produced no meaningful work.

### Required Environment Variables

The recipe runner requires these environment variables to function:

| Variable                   | Purpose                                           | Default         |
| -------------------------- | ------------------------------------------------- | --------------- |
| `AMPLIHACK_HOME`           | Root of amplihack installation (for asset lookup) | Auto-detected   |
| `AMPLIHACK_AGENT_BINARY`   | Which agent binary to use (claude, copilot, etc.) | Set by launcher |
| `AMPLIHACK_MAX_DEPTH`      | Max recursion depth for nested sessions           | `3`             |
| `AMPLIHACK_NONINTERACTIVE` | Set to `1` to skip interactive prompts            | Unset           |

If `AMPLIHACK_HOME` is not set and auto-detection fails, `parse-decomposition`
and `activate-workflow` will fail with asset lookup errors. Set it to
the directory containing `amplifier-bundle/`.

### After Execution: Reflect and verify

After execution completes, verify the goal was achieved. If not:

- For missing information: ask the user
- For fixable gaps: re-invoke with the remaining work description
- For infrastructure failures: file a bug and try adaptive strategy

### Enforcement: PostToolUse Workflow Guard

A PostToolUse hook (`amplihack-hooks`) actively monitors every
tool call after this skill is invoked. It tracks:

- Whether `/dev` or `dev-orchestrator` was called (sets a flag)
- Whether the recipe runner was actually executed (clears the flag)
- How many tool calls have passed without workflow evidence

If 3+ tool calls pass without evidence of recipe runner execution, the hook
emits a hard WARNING. This is not a suggestion — it means you are violating
the mandatory workflow. State is stored in `/tmp/amplihack-workflow-state/`.
